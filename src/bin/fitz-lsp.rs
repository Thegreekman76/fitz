// fitz-lsp — Language Server Protocol para Fitz (Fase 9.x).
//
// Bin separado del CLI principal (`fitz`) por convención del ecosistema:
// rust-analyzer, gopls, tsserver, pyright son todos binarios aparte que
// el cliente (extensión VSCode, plugin Neovim, etc.) spawnea como
// proceso hijo y se comunica por stdio con JSON-RPC.
//
// Estado:
// - 9.x.1.a: handshake initialize/initialized/shutdown.
// - 9.x.1.b: did_open/did_change/did_close + diagnósticos en vivo
//   sobre el pipeline `parse_with_recovery → check_program`. La
//   lógica vive en `fitz::lsp` (lib, unit-testeable); este bin es
//   solo la capa fina que mapea handlers de tower-lsp al pipeline.
//
// Build: `cargo build --features lsp` (la dep `tower-lsp` es opt-in;
// `required-features = ["lsp"]` evita errores de link en el flujo
// default).

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

/// Estado por documento abierto. Persiste el último texto recibido por
/// `did_open`/`did_change`, el AST parseado (Fase 9.x.4 — completion
/// scope-level enumera top-level), y los side-tables del último
/// chequeo: `TypeEnv` (resuelve nombres nominales para hover/
/// completion), `TypeInfo` (tipo por nodo — hover, completion
/// after-dot), `DefinitionInfo` (uso → declaración — go-to-definition).
//
// `text` lo lee el handler `completion` (9.x.4.b) para detectar el
// contexto walkeando hacia atrás desde el cursor. `program` también
// es usado por completion (scope-level).
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
    /// Estado por documento abierto. Se popula en `did_open`, se
    /// actualiza completo en `did_change` (anunciamos `FULL` sync), y
    /// se borra en `did_close`. `parking_lot::Mutex` (no `tokio`)
    /// porque las secciones críticas son ~microsegundos: lock,
    /// get/insert, unlock — sin awaits adentro. La pipeline del checker
    /// corre fuera del lock.
    documents: Arc<Mutex<HashMap<Url, DocumentState>>>,
}

impl Backend {
    /// Corre el pipeline LSP-style sobre `text`, persiste el resultado
    /// en `documents`, y publica los diagnósticos. Devuelve nada — la
    /// notificación es fire-and-forget.
    async fn check_and_publish(&self, uri: Url, text: String, version: Option<i32>) {
        let (program, type_env, type_info, def_info, errors) = check_source_with_types(&text);
        // LSPy — diagnostics con Range exacto del símbolo bajo el
        // cursor (no 1-char dummy). Requiere el source para extraer
        // el ident en cada posición.
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
        // Capabilities: anunciamos `FULL` sync — recibimos el documento
        // entero en cada `did_change` y re-corremos el pipeline. La
        // alternativa `INCREMENTAL` requiere mantener el buffer y
        // aplicar edits, más eficiente sobre archivos grandes pero suma
        // complejidad. FULL es el default razonable para el MVP; la
        // migración es decisión de perf que dejamos para cuando aparezca
        // presión real.
        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "fitz-lsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: ServerCapabilities {
                // v0.13.2 — omitimos `position_encoding` para que el
                // cliente asuma el default UTF-16 del spec LSP. El
                // intento de v0.9.51 de anunciar `utf-8` se rompió
                // contra `vscode-languageclient@9.0.1`, que hard-codea
                // `generalCapabilities.positionEncodings = ['utf-16']`
                // (client.js:1370) y rechaza cualquier encoding del
                // server distinto de `utf-16` o `undefined`
                // (client.js:835). El handshake fallaba con
                // "Unsupported position encoding (utf-8)" antes de
                // poder hablar JSON-RPC, dejando la extensión 0.13.1
                // inservible en VSCode fresh.
                //
                // Migración completa: `position_to_offset` /
                // `offset_to_position` en `src/lsp.rs` ahora cuentan
                // UTF-16 code units (vía `ch.len_utf16()`) para
                // coincidir con el spec. Para el lookup en
                // `TypeInfo` / `DefinitionInfo` que sigue indexado
                // por chars Unicode del lexer, los handlers `hover` /
                // `goto_definition` traducen `pos.character` con
                // `utf16_to_unicode_char(text, line, char_utf16)`
                // antes de llamar a `hover_for_position` /
                // `definition_for_position`. Soporta chars del
                // Supplementary Multilingual Plane (emoji, símbolos
                // matemáticos avanzados) sin off-by-one. Deuda
                // residual cosmética: `make_definition_location` y
                // `ident_range_from_def` retornan Range LSP en chars
                // Unicode en lugar de UTF-16 — pero como las líneas
                // de def son siempre ASCII en la parte ANTES del
                // ident (keywords + identifiers son ASCII por reglas
                // del lexer), char_unicode == char_utf16 en práctica.
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                // Fase 9.x.2 — anunciamos que respondemos
                // `textDocument/hover` con el tipo del nodo bajo el
                // cursor. La heurística de lookup y el formato de
                // respuesta viven en `fitz::lsp`.
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                // Fase 9.x.3 — anunciamos que respondemos
                // `textDocument/definition`. Devolvemos un solo
                // `Location` (no multi-definición — Fitz no tiene
                // overloading), por eso `OneOf::Left(true)` (forma
                // simple) en lugar de `DefinitionOptions`.
                definition_provider: Some(OneOf::Left(true)),
                // Fase 9.x.4 — anunciamos completion contextual.
                // `trigger_characters = [".".into(), "@".into()]` hace
                // que VSCode invoque automáticamente la completion tras
                // un `.` (caso after-dot) o un `@` (caso AfterAt,
                // v0.10.12 — lista de decorators). Para typing normal,
                // el cliente invoca por su cuenta. `resolve_provider:
                // false` porque mandamos toda la info en el item (no
                // usamos `completionItem/resolve` para detalles lazy).
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".into(), "@".into()]),
                    resolve_provider: Some(false),
                    ..CompletionOptions::default()
                }),
                // V3 (2026-06-05) — anunciamos `textDocument/formatting`
                // delegando al formatter `fitz fmt` ya existente
                // (`src/fmt.rs::format_source`). El cliente VSCode con
                // `editor.formatOnSave = true` invoca el handler `formatting`
                // al guardar. Sin esto, el usuario tenía que correr `fitz fmt`
                // a mano o configurar un formatter externo apuntando al binario.
                document_formatting_provider: Some(OneOf::Left(true)),
                // V4 (2026-06-05) — anunciamos `textDocument/signatureHelp`
                // con trigger chars `(` y `,`. Al tipear `f(` o `f(a, `
                // el cliente invoca el handler que muestra la firma de la
                // fn top-level con el param actual resaltado. MVP: solo
                // fns user-defined del programa; builtins quedan deuda menor.
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
        // Con `TextDocumentSyncKind::FULL`, la spec garantiza un solo
        // entry en `content_changes` y su `text` es el documento entero
        // (sin `range`). Tomamos el último por defensa contra clientes
        // que manden múltiples eventos.
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
        // Convención LSP: al cerrar, mandamos un publish con lista
        // vacía para que VSCode limpie los marcadores del archivo.
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn hover(&self, params: HoverParams) -> LspResult<Option<Hover>> {
        // Resolvemos todo bajo el lock — clonamos lo mínimo (un `Type`
        // y formateamos contra el env) para soltarlo antes de devolver.
        // El env no se clona; el `make_hover` corre adentro del lock
        // porque solo lee y serializa a string. Sin awaits → sin
        // deadlock risk.
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let documents = self.documents.lock();
        let state = match documents.get(&uri) {
            Some(s) => s,
            None => return Ok(None),
        };
        // v0.13.2 — `pos.character` llega del cliente como UTF-16
        // code units (default del spec LSP). `hover_for_position` y
        // `make_hover_with_range` esperan chars Unicode (TypeInfo
        // indexa por chars del lexer, `ident_range_at_position` usa
        // `Vec<char>`). Traducimos una vez con el helper y pasamos
        // el char_unicode a ambas. Para código sin SMP (todo en la
        // práctica) la traducción es la identidad.
        let char_unicode = utf16_to_unicode_char(&state.text, pos.line, pos.character);
        // LSPy — hover con Range del símbolo bajo el cursor para
        // que VSCode highlightee el token en lugar de solo mostrar
        // el tooltip aislado.
        let hover = hover_for_position(&state.type_info, pos.line, char_unicode).map(|ty| {
            // v0.10.32 (Tier D.2) — pasamos `program` para que el LSP
            // pueda augmentar el hover con el CREATE TABLE SQL si el
            // tipo es un `@table` type.
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
        // Resolvemos `(uri, pos) → def_span` bajo el lock. Para defs
        // locales, devolvemos el Location del doc abierto. Mini-tanda
        // LSPx — si el def_span apunta a un Stmt::Import/FromImport,
        // intentamos resolver el módulo target y apuntar a la
        // declaración real en ese archivo. El nombre del ident bajo
        // el cursor se extrae heurísticamente desde la línea — basta
        // con la "palabra" alphanum bajo la posición.
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let documents = self.documents.lock();
        let state = match documents.get(&uri) {
            Some(s) => s,
            None => return Ok(None),
        };
        // v0.13.2 — traducimos UTF-16 → chars Unicode una vez (mismo
        // motivo que en `hover`). `definition_for_position` busca en
        // DefinitionInfo indexado por chars Unicode del lexer;
        // `ident_under_cursor` usa `Vec<char>` y `char_idx` directo.
        let char_unicode = utf16_to_unicode_char(&state.text, pos.line, pos.character);
        let Some(def_span) = definition_for_position(&state.def_info, pos.line, char_unicode)
        else {
            return Ok(None);
        };
        // Intentar cross-module resolution si el def_span coincide con
        // un Stmt::Import / Stmt::FromImport del program. Extraemos el
        // nombre del ident bajo el cursor de la línea del documento.
        let target_name = ident_under_cursor(&state.text, pos.line as usize, char_unicode as usize);
        let cross = target_name
            .as_deref()
            .and_then(|name| resolve_cross_module_definition(&state.program, &uri, def_span, name));
        // LSPy — Location con Range exacto del ident en la línea
        // del def. Para defs locales, source = doc abierto; para
        // cross-module, source = archivo target (re-leemos del FS).
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
        // Detección de contexto + lookup en TypeInfo/Program bajo el
        // lock. El helper `completion_at_position` es pure-function;
        // sin awaits dentro del lock.
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
            // v0.9.47 — pasa el doc_uri para que el contexto
            // `from <mod> import |` resuelva el archivo del módulo
            // target y enumere sus exports.
            Some(&uri),
        );
        Ok(Some(CompletionResponse::Array(items)))
    }

    /// V3 (2026-06-05) — `textDocument/formatting`. Delega al formatter
    /// pure-function `fitz::fmt::format_source` que ya existe (Fase 9.z.1)
    /// y devuelve UN `TextEdit` que reemplaza el documento entero
    /// (`(0,0)..(end_of_doc)` → texto formateado). Si el doc tiene
    /// errores de parser, `format_source` retorna `Err` y devolvemos
    /// `Ok(None)` silencioso — no abortamos el save del usuario.
    ///
    /// Patrón estándar para formatters non-incremental (rust-analyzer,
    /// black, prettier hacen igual). Usamos `TextDocumentSyncKind::FULL`
    /// así que el `state.text` siempre tiene el contenido completo.
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
        // Si el output coincide bit-a-bit con el input, no emitimos
        // edits — evita marcar el doc como modificado por save sin
        // cambios reales.
        if formatted == state.text {
            return Ok(Some(Vec::new()));
        }
        // Range del doc entero: desde (0,0) hasta (last_line, last_col).
        // Posiciones en UTF-16 code units (LSP default). Para el final
        // del doc, calculamos last_line + last_col del state.text.
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

    /// V4 (2026-06-05) — `textDocument/signatureHelp`. Detecta el `Call`
    /// enclosing en el documento, identifica el callee por nombre y
    /// resuelve la signature contra fns top-level del `Program`. MVP:
    /// solo fns user-defined; builtins y method calls quedan como deuda
    /// menor.
    ///
    /// Heurística del walkback: cuenta `(`/`)` para encontrar el `(` no
    /// balanceado del call enclosing, y `,` a depth 0 para el
    /// `active_parameter`. No respeta strings ni comments (deuda menor
    /// — patrón raro en práctica).
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
        // v0.13.2 — UTF-16 → chars Unicode una vez (mismo motivo que en
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

/// V3 (2026-06-05) — calcula la `Position` LSP del final del documento
/// en UTF-16 code units (default del spec LSP). Devuelve
/// `(last_line, last_col_utf16)` apuntando ANTES del último char
/// emitido (o `(0, 0)` para doc vacío). Sigue la convención del LSP
/// donde el `end` de un Range es exclusivo.
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

/// Mini-tanda LSPx — extrae el identificador (run de chars alphanum +
/// `_`) bajo el cursor `(line, character)` (ambos 0-based LSP). Usado
/// por `goto_definition` para nombrar el símbolo a resolver
/// cross-module. Si el cursor cae sobre un char no-ident, busca el
/// run a la IZQUIERDA inmediata. Devuelve `None` si no hay ident
/// adyacente.
fn ident_under_cursor(text: &str, line_idx: usize, char_idx: usize) -> Option<String> {
    let line = text.lines().nth(line_idx)?;
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let is_ident_char = |c: char| c.is_alphanumeric() || c == '_';
    let cursor = char_idx.min(chars.len());

    // Si el cursor está sobre o adyacente a un char ident, encontrar
    // los límites del run.
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
    // current_thread alcanza para el LSP: el server es I/O-bound (lee
    // stdin, escribe stdout, despacha handlers) y no necesita el work
    // stealing del scheduler multi-thread. Mantiene el binario chico
    // y deja la decisión de runtime ortogonal a la del CLI HTTP que
    // sí pide multi-thread (Fase F17).
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(|client| Backend {
        client,
        documents: Arc::new(Mutex::new(HashMap::new())),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
