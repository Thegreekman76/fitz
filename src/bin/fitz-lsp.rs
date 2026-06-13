// fitz-lsp — Language Server Protocol for Fitz (Phase 9.x).
//
// Separate bin from the main CLI (`fitz`) by ecosystem convention:
// rust-analyzer, gopls, tsserver, pyright are all separate binaries
// that the client (VSCode extension, Neovim plugin, etc.) spawns as
// a child process and communicates with via stdio + JSON-RPC.
//
// Status:
// - 9.x.1.a: initialize/initialized/shutdown handshake.
// - 9.x.1.b: did_open/did_change/did_close + live diagnostics over
//   the `parse_with_recovery → check_program` pipeline. The logic
//   lives in `fitz::lsp` (lib, unit-testable); this bin is just the
//   thin layer that maps tower-lsp handlers to the pipeline.
//
// Build: `cargo build --features lsp` (the `tower-lsp` dep is
// opt-in; `required-features = ["lsp"]` avoids link errors in the
// default flow).

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::{
    CompletionOptions, CompletionParams, CompletionResponse, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DocumentFormattingParams,
    GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, InitializedParams, MessageType, OneOf, Position, Range,
    ServerCapabilities, ServerInfo, SignatureHelp, SignatureHelpOptions, SignatureHelpParams,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, Url,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

use fitz::ast::Program;
use fitz::lsp::{
    check_source_with_types, completion_at_position_with_uri, definition_for_position,
    fitz_errors_to_diagnostics_with_source, hover_for_position,
    make_definition_location_with_source, make_hover_with_range, resolve_cross_module_definition,
    signature_help_at_position, utf16_to_unicode_char,
};
use fitz::types::{DefinitionInfo, TypeEnv, TypeInfo};

/// Per-open-document state. Persists the last text received by
/// `did_open`/`did_change`, the parsed AST (Phase 9.x.4 — scope-level
/// completion enumerates top-level), and the side-tables of the last
/// check: `TypeEnv` (resolves nominal names for hover/completion),
/// `TypeInfo` (per-node type — hover, after-dot completion),
/// `DefinitionInfo` (use → declaration — go-to-definition).
//
// `text` is read by the `completion` handler (9.x.4.b) to detect the
// context by walking backwards from the cursor. `program` is also
// used by completion (scope-level).
#[derive(Debug)]
struct DocumentState {
    text: String,
    program: Program,
    type_env: TypeEnv,
    type_info: TypeInfo,
    def_info: DefinitionInfo,
}

#[derive(Debug)]
struct Backend {
    client: Client,
    /// Per-open-document state. Populated in `did_open`, fully
    /// updated in `did_change` (we announce `FULL` sync), and cleared
    /// in `did_close`. `parking_lot::Mutex` (not `tokio`) because the
    /// critical sections are ~microseconds: lock, get/insert, unlock
    /// — no awaits inside. The checker pipeline runs outside the lock.
    documents: Arc<Mutex<HashMap<Url, DocumentState>>>,
}

impl Backend {
    /// Runs the LSP-style pipeline over `text`, persists the result
    /// in `documents`, and publishes the diagnostics. Returns
    /// nothing — the notification is fire-and-forget.
    async fn check_and_publish(&self, uri: Url, text: String, version: Option<i32>) {
        let (program, type_env, type_info, def_info, errors) = check_source_with_types(&text);
        // LSPy — diagnostics with the exact Range of the symbol
        // under the cursor (no 1-char dummy). Requires the source
        // to extract the ident at each position.
        let diagnostics = fitz_errors_to_diagnostics_with_source(&errors, &text);
        self.documents.lock().insert(
            uri.clone(),
            DocumentState {
                text,
                program,
                type_env,
                type_info,
                def_info,
            },
        );
        self.client
            .publish_diagnostics(uri, diagnostics, version)
            .await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _params: InitializeParams) -> LspResult<InitializeResult> {
        // Capabilities: we announce `FULL` sync — we receive the
        // whole document on each `did_change` and re-run the
        // pipeline. The `INCREMENTAL` alternative requires keeping
        // the buffer and applying edits, more efficient for large
        // files but adds complexity. FULL is the reasonable default
        // for the MVP; migration is a perf decision we leave for
        // when real pressure shows up.
        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "fitz-lsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: ServerCapabilities {
                // v0.13.2 — we omit `position_encoding` so the
                // client assumes the LSP spec's UTF-16 default. The
                // v0.9.51 attempt to announce `utf-8` broke against
                // `vscode-languageclient@9.0.1`, which hard-codes
                // `generalCapabilities.positionEncodings = ['utf-16']`
                // (client.js:1370) and rejects any server encoding
                // other than `utf-16` or `undefined` (client.js:835).
                // The handshake failed with "Unsupported position
                // encoding (utf-8)" before being able to speak
                // JSON-RPC, leaving the 0.13.1 extension unusable on
                // fresh VSCode.
                //
                // Full migration: `position_to_offset` /
                // `offset_to_position` in `src/lsp.rs` now count
                // UTF-16 code units (via `ch.len_utf16()`) to match
                // the spec. For the `TypeInfo` / `DefinitionInfo`
                // lookup that's still indexed by Unicode chars of
                // the lexer, the `hover` / `goto_definition`
                // handlers translate `pos.character` with
                // `utf16_to_unicode_char(text, line, char_utf16)`
                // before calling `hover_for_position` /
                // `definition_for_position`. Supports
                // Supplementary Multilingual Plane chars (emoji,
                // advanced math symbols) without off-by-one.
                // Cosmetic residual debt:
                // `make_definition_location` and
                // `ident_range_from_def` return LSP Range in
                // Unicode chars instead of UTF-16 — but because the
                // def lines are always ASCII in the part BEFORE the
                // ident (keywords + identifiers are ASCII per lexer
                // rules), char_unicode == char_utf16 in practice.
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                // Phase 9.x.2 — we announce we respond to
                // `textDocument/hover` with the type of the node
                // under the cursor. The lookup heuristic and the
                // response format live in `fitz::lsp`.
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                // Phase 9.x.3 — we announce we respond to
                // `textDocument/definition`. We return a single
                // `Location` (no multi-definition — Fitz has no
                // overloading), hence `OneOf::Left(true)` (simple
                // form) instead of `DefinitionOptions`.
                definition_provider: Some(OneOf::Left(true)),
                // Phase 9.x.4 — we announce contextual completion.
                // `trigger_characters = [".".into(), "@".into()]`
                // makes VSCode invoke completion automatically after
                // a `.` (after-dot case) or `@` (AfterAt case,
                // v0.10.12 — list of decorators). For normal typing,
                // the client invokes on its own. `resolve_provider:
                // false` because we send all the info in the item
                // (we don't use `completionItem/resolve` for lazy
                // details).
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".into(), "@".into()]),
                    resolve_provider: Some(false),
                    ..CompletionOptions::default()
                }),
                // V3 (2026-06-05) — we announce
                // `textDocument/formatting` delegating to the
                // existing `fitz fmt` formatter
                // (`src/fmt.rs::format_source`). The VSCode client
                // with `editor.formatOnSave = true` invokes the
                // `formatting` handler on save. Without it, the user
                // had to run `fitz fmt` by hand or configure an
                // external formatter pointing at the binary.
                document_formatting_provider: Some(OneOf::Left(true)),
                // V4 (2026-06-05) — we announce
                // `textDocument/signatureHelp` with trigger chars
                // `(` and `,`. When typing `f(` or `f(a, ` the
                // client invokes the handler which shows the
                // signature of the top-level fn with the current
                // param highlighted. MVP: only user-defined fns
                // from the program; builtins remain minor debt.
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".into(), ",".into()]),
                    retrigger_characters: None,
                    work_done_progress_options: Default::default(),
                }),
                ..ServerCapabilities::default()
            },
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "fitz-lsp inicializado")
            .await;
    }

    async fn shutdown(&self) -> LspResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        let version = params.text_document.version;
        self.check_and_publish(uri, text, Some(version)).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        // With `TextDocumentSyncKind::FULL`, the spec guarantees a
        // single entry in `content_changes` whose `text` is the
        // entire document (no `range`). We take the last one as a
        // defense against clients that send multiple events.
        let text = params
            .content_changes
            .into_iter()
            .last()
            .map(|c| c.text)
            .unwrap_or_default();
        self.check_and_publish(uri, text, Some(version)).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.lock().remove(&uri);
        // LSP convention: on close we send a publish with an empty
        // list so VSCode clears the file's markers.
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn hover(&self, params: HoverParams) -> LspResult<Option<Hover>> {
        // We resolve everything under the lock — we clone the
        // minimum (one `Type` and format against the env) so we can
        // release it before returning. The env is not cloned;
        // `make_hover` runs inside the lock because it only reads
        // and serializes to a string. No awaits → no deadlock risk.
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let documents = self.documents.lock();
        let state = match documents.get(&uri) {
            Some(s) => s,
            None => return Ok(None),
        };
        // v0.13.2 — `pos.character` arrives from the client as
        // UTF-16 code units (LSP spec default). `hover_for_position`
        // and `make_hover_with_range` expect Unicode chars
        // (TypeInfo indexes by lexer chars, `ident_range_at_position`
        // uses `Vec<char>`). We translate once with the helper and
        // pass char_unicode to both. For code without SMP (all real
        // code in practice) the translation is the identity.
        let char_unicode = utf16_to_unicode_char(&state.text, pos.line, pos.character);
        // LSPy — hover with Range of the symbol under the cursor so
        // VSCode highlights the token instead of only showing the
        // isolated tooltip.
        let hover = hover_for_position(&state.type_info, pos.line, char_unicode).map(|ty| {
            // v0.10.32 (Tier D.2) — we pass `program` so the LSP can
            // augment the hover with the CREATE TABLE SQL if the
            // type is a `@table` type.
            make_hover_with_range(
                ty,
                &state.type_env,
                &state.program,
                &state.text,
                pos.line,
                char_unicode,
            )
        });
        Ok(hover)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> LspResult<Option<GotoDefinitionResponse>> {
        // We resolve `(uri, pos) → def_span` under the lock. For
        // local defs we return the Location of the open doc. LSPx
        // mini-batch — if def_span points at a
        // Stmt::Import/FromImport, we try to resolve the target
        // module and point at the actual declaration in that file.
        // The name of the ident under the cursor is extracted
        // heuristically from the line — the alphanum "word" under
        // the position is enough.
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let documents = self.documents.lock();
        let state = match documents.get(&uri) {
            Some(s) => s,
            None => return Ok(None),
        };
        // v0.13.2 — we translate UTF-16 → Unicode chars once (same
        // reason as in `hover`). `definition_for_position` looks up
        // DefinitionInfo indexed by lexer Unicode chars;
        // `ident_under_cursor` uses `Vec<char>` and `char_idx`
        // directly.
        let char_unicode = utf16_to_unicode_char(&state.text, pos.line, pos.character);
        let Some(def_span) = definition_for_position(&state.def_info, pos.line, char_unicode)
        else {
            return Ok(None);
        };
        // Attempt cross-module resolution if the def_span matches a
        // Stmt::Import / Stmt::FromImport of the program. We
        // extract the name of the ident under the cursor from the
        // document line.
        let target_name = ident_under_cursor(&state.text, pos.line as usize, char_unicode as usize);
        let cross = target_name
            .as_deref()
            .and_then(|name| resolve_cross_module_definition(&state.program, &uri, def_span, name));
        // LSPy — Location with exact Range of the ident on the def
        // line. For local defs, source = open doc; for cross-
        // module, source = target file (we re-read from the FS).
        let location = match cross {
            Some((target_uri, target_span)) => {
                let target_source = target_uri
                    .to_file_path()
                    .ok()
                    .and_then(|p| std::fs::read_to_string(p).ok());
                make_definition_location_with_source(
                    target_uri,
                    target_span,
                    target_source.as_deref(),
                )
            }
            None => make_definition_location_with_source(uri.clone(), def_span, Some(&state.text)),
        };
        Ok(Some(GotoDefinitionResponse::Scalar(location)))
    }

    async fn completion(&self, params: CompletionParams) -> LspResult<Option<CompletionResponse>> {
        // Context detection + TypeInfo/Program lookup under the
        // lock. The `completion_at_position` helper is pure
        // function; no awaits inside the lock.
        let uri = params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let documents = self.documents.lock();
        let state = match documents.get(&uri) {
            Some(s) => s,
            None => return Ok(None),
        };
        let items = completion_at_position_with_uri(
            &state.text,
            &state.program,
            &state.type_info,
            &state.type_env,
            pos.line,
            pos.character,
            // v0.9.47 — pass the doc_uri so the `from <mod> import |`
            // context resolves the target module file and
            // enumerates its exports.
            Some(&uri),
        );
        Ok(Some(CompletionResponse::Array(items)))
    }

    /// V3 (2026-06-05) — `textDocument/formatting`. Delegates to
    /// the pure-function formatter `fitz::fmt::format_source` that
    /// already exists (Phase 9.z.1) and returns ONE `TextEdit` that
    /// replaces the entire document
    /// (`(0,0)..(end_of_doc)` → formatted text). If the doc has
    /// parser errors, `format_source` returns `Err` and we return
    /// `Ok(None)` silently — we don't abort the user's save.
    ///
    /// Standard pattern for non-incremental formatters
    /// (rust-analyzer, black, prettier do the same). We use
    /// `TextDocumentSyncKind::FULL` so `state.text` always has the
    /// full content.
    async fn formatting(
        &self,
        params: DocumentFormattingParams,
    ) -> LspResult<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        let documents = self.documents.lock();
        let state = match documents.get(&uri) {
            Some(s) => s,
            None => return Ok(None),
        };
        let formatted = match fitz::fmt::format_source(&state.text) {
            Ok(s) => s,
            Err(_) => return Ok(None),
        };
        // If the output matches the input bit-by-bit, we emit no
        // edits — avoids marking the doc as modified by save
        // without real changes.
        if formatted == state.text {
            return Ok(Some(Vec::new()));
        }
        // Range of the entire doc: from (0,0) to
        // (last_line, last_col). Positions in UTF-16 code units
        // (LSP default). For the end of the doc, we compute
        // last_line + last_col of state.text.
        let (end_line, end_char_utf16) = end_position_utf16(&state.text);
        let edit = TextEdit {
            range: Range {
                start: Position::new(0, 0),
                end: Position::new(end_line, end_char_utf16),
            },
            new_text: formatted,
        };
        Ok(Some(vec![edit]))
    }

    /// V4 (2026-06-05) — `textDocument/signatureHelp`. Detects the
    /// enclosing `Call` in the document, identifies the callee by
    /// name, and resolves the signature against top-level fns of
    /// the `Program`. MVP: only user-defined fns; builtins and
    /// method calls remain minor debt.
    ///
    /// Walkback heuristic: counts `(`/`)` to find the unbalanced
    /// `(` of the enclosing call, and `,` at depth 0 for
    /// `active_parameter`. Doesn't respect strings or comments
    /// (minor debt — rare pattern in practice).
    async fn signature_help(
        &self,
        params: SignatureHelpParams,
    ) -> LspResult<Option<SignatureHelp>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let documents = self.documents.lock();
        let state = match documents.get(&uri) {
            Some(s) => s,
            None => return Ok(None),
        };
        // v0.13.2 — UTF-16 → Unicode chars once (same reason as in
        // `hover`/`goto_definition`).
        let char_unicode = utf16_to_unicode_char(&state.text, pos.line, pos.character);
        Ok(signature_help_at_position(
            &state.text,
            &state.program,
            pos.line,
            char_unicode,
        ))
    }
}

/// V3 (2026-06-05) — computes the LSP `Position` of the end of the
/// document in UTF-16 code units (LSP spec default). Returns
/// `(last_line, last_col_utf16)` pointing BEFORE the last emitted
/// char (or `(0, 0)` for an empty doc). Follows the LSP convention
/// where the `end` of a Range is exclusive.
fn end_position_utf16(text: &str) -> (u32, u32) {
    if text.is_empty() {
        return (0, 0);
    }
    let mut line: u32 = 0;
    let mut col_utf16: u32 = 0;
    for ch in text.chars() {
        if ch == '\n' {
            line += 1;
            col_utf16 = 0;
        } else {
            col_utf16 += ch.len_utf16() as u32;
        }
    }
    (line, col_utf16)
}

/// LSPx mini-batch — extracts the identifier (run of alphanum + `_`
/// chars) under the cursor `(line, character)` (both 0-based LSP).
/// Used by `goto_definition` to name the symbol to resolve cross-
/// module. If the cursor falls on a non-ident char, it looks for the
/// run immediately to the LEFT. Returns `None` if no ident is
/// adjacent.
fn ident_under_cursor(text: &str, line_idx: usize, char_idx: usize) -> Option<String> {
    let line = text.lines().nth(line_idx)?;
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let is_ident_char = |c: char| c.is_alphanumeric() || c == '_';
    let cursor = char_idx.min(chars.len());

    // If the cursor is on or adjacent to an ident char, find the
    // bounds of the run.
    let mut start = cursor;
    while start > 0 && is_ident_char(chars[start - 1]) {
        start -= 1;
    }
    let mut end = cursor;
    while end < chars.len() && is_ident_char(chars[end]) {
        end += 1;
    }
    if start == end {
        return None;
    }
    Some(chars[start..end].iter().collect())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // current_thread is enough for the LSP: the server is I/O-bound
    // (reads stdin, writes stdout, dispatches handlers) and doesn't
    // need work stealing from the multi-thread scheduler. Keeps the
    // binary small and leaves the runtime decision orthogonal to
    // the HTTP CLI's, which DOES need multi-thread (Phase F17).
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(|client| Backend {
        client,
        documents: Arc::new(Mutex::new(HashMap::new())),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
