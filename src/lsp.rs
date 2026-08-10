// lsp.rs — Language Server Protocol logic (Phase 9.x.1.b).
//
// Lives in the lib (`src/lib.rs` exposes it as `pub mod lsp` behind
// the `lsp` feature) instead of inside the bin so it is unit-testable:
// cargo does not handle `#[cfg(test)]` well in `src/bin/*.rs`.
// The bin `src/bin/fitz-lsp.rs` consumes this via `use fitz::lsp::...`.

use crate::ast::{Expr, Program, Span, Stmt};
use crate::error::FitzError;
use crate::lexer::tokenize;
use crate::parser::{parse as parse_strict, parse_with_recovery};
use crate::types::{
    check_program, check_with_env, extract_auth_provider_signature, extract_background_fn_names,
    resolve_program_with_env, DefinitionInfo, Type, TypeEnv, TypeInfo,
};

use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, Diagnostic, DiagnosticSeverity, Documentation, Hover,
    HoverContents, InsertTextFormat, Location, MarkupContent, MarkupKind, ParameterInformation,
    ParameterLabel, Position, Range, SignatureHelp, SignatureInformation, Url,
};

/// LSP-style pipeline over `source`: tokenizes, parses with recovery,
/// type-checks, and returns the combined list of errors. The name
/// "lsp-style" distinguishes it from `fitz check`/`fitz run`, which use
/// strict `parser::parse` and abort at the first parser error.
///
/// Lexer errors abort the pipeline (there is no AST to check). Parser
/// and checker always return their errors alongside whatever they were
/// able to recover.
///
/// This variant discards the side-tables and the AST returned by
/// `check_program` (for consumers that only need diagnostics). For
/// hover / go-to-definition / completion, use `check_source_with_types`.
pub fn check_source(source: &str) -> Vec<FitzError> {
    let (_program, _env, _type_info, _def_info, errors) = check_source_with_types(source);
    errors
}

/// Session B — Phase 11.8.a (2026-07-18): LSP diagnostics pipeline for
/// `.fitzv` view files. Runs the view lexer → view parser → view
/// expand → view check pipeline and converts each error type
/// (`ViewParseError`, `ExpandError`, `CheckError`) to `FitzError` shape
/// so the existing `fitz_errors_to_diagnostics_with_source` (which the
/// bin's `check_and_publish` already uses) works unchanged.
///
/// **Error kind mapping**:
/// - `ViewParseError` → `ErrorKind::InvalidSyntax` (parser-level).
/// - `ExpandError`    → `ErrorKind::InvalidSyntax` (still a shape issue).
/// - `CheckError`     → `ErrorKind::TypeMismatch` (type-check level).
///
/// **Error policy**: the pipeline is short-circuiting per-layer — if the
/// view parser fails, we don't run expand/check; if expand fails, we
/// don't run check. This matches the CLI's `fitz check` behaviour on
/// `.fitzv` files and produces cleaner diagnostics (no cascade errors
/// from an already-broken tree).
///
/// **Cross-module resolution**: when the caller passes a `base_dir`
/// (via [`check_view_source_with_base_dir`]), the SFC's
/// `from X import Y` graph IS walked — imported sibling `.fitzv`
/// components are loaded (transitively) so a cross-file `<Child />`
/// validates against its real surface instead of surfacing as a false
/// "unknown component". The bare [`check_view_source`] keeps the
/// single-file behaviour (no base_dir → no sibling loading) for
/// callers without file context (REPL, unit tests).
pub fn check_view_source(source: &str) -> Vec<FitzError> {
    check_view_source_with_base_dir(source, None)
}

/// Base_dir-aware view diagnostics — the cross-file variant of
/// [`check_view_source`]. When `base_dir` is `Some`, the imported
/// sibling `.fitzv` components are loaded via
/// `crate::view::load_imported_components` (over the transitive import
/// closure) and fed to `check_with_imported_components`, so a cross-file
/// `<Child />` — including an aliased or transitively-reached one —
/// resolves instead of being reported as unknown. When `base_dir` is
/// `None`, it falls back to the single-file `check` (byte-for-byte with
/// the pre-cross-file behaviour).
///
/// Structural parallel of the classic
/// [`check_source_with_types_and_base_dir`] + `pre_scan_imported_*_lsp`
/// design, substituting `load_imported_components` (component surfaces)
/// for the auth-provider / background-fn extractors. Same silent-fallback
/// error policy: a sibling that is missing / fails to parse is skipped,
/// and a genuinely-missing component still surfaces as an "unknown
/// component" checker error.
pub fn check_view_source_with_base_dir(
    source: &str,
    base_dir: Option<&std::path::Path>,
) -> Vec<FitzError> {
    use crate::error::ErrorKind;
    // Layer 1 — view lexer + parser.
    let raw = match crate::view::parse(source) {
        Ok(f) => f,
        Err(e) => {
            return vec![FitzError {
                kind: ErrorKind::InvalidSyntax,
                line: e.line,
                column: e.column,
                message: e.message,
                hint: None,
            }];
        }
    };
    // Layer 2 — expand.
    let expanded = match crate::view::expand::expand(&raw) {
        Ok(f) => f,
        Err(e) => {
            return vec![FitzError {
                kind: ErrorKind::InvalidSyntax,
                line: e.loc.line,
                column: e.loc.column,
                message: format!("{} ({})", e.message, e.context),
                hint: None,
            }];
        }
    };
    // Layer 3 — type-check. Returns Vec<CheckError> — accumulates,
    // doesn't short-circuit. Cross-file when a base_dir is known.
    let check_errors = match base_dir {
        Some(bd) => {
            // Walk the `.fitzv` import graph (transitive) and load the
            // imported sibling components. Best-effort: a load failure
            // yields an empty registry, so composition falls back to the
            // single-file behaviour rather than crashing the request.
            let all_imports = crate::view::collect_transitive_view_imports(&expanded.imports, bd);
            let imported =
                crate::view::load_imported_components(&all_imports, bd).unwrap_or_default();
            crate::view::check::check_with_imported_components(&expanded, imported.components())
        }
        None => crate::view::check::check(&expanded),
    };
    check_errors
        .into_iter()
        .map(|e| FitzError {
            kind: ErrorKind::TypeMismatch {
                expected: String::new(),
                found: String::new(),
            },
            line: e.loc.line,
            column: e.loc.column,
            message: format!("{} ({})", e.message, e.context),
            hint: None,
        })
        .collect()
}

/// Session B — Phase 11.8.b (2026-07-18): completion inside a `.fitzv`
/// source. Offers:
/// - **Template directives** — `{#if}`, `{#for}`, `{#else}`, `{/if}`,
///   `{/for}` when the cursor is preceded by `{` at the last non-blank
///   char before the cursor (or `{#` when the user is mid-typing).
/// - **Event decorators** — `@click`, `@submit` when the cursor is
///   preceded by `@` inside an HTML tag position (bare `@` at cursor —
///   we don't fully parse the tag, so we accept any `@` prefix).
/// - **State field names** of the component enclosing the cursor line
///   as `FIELD` items — matches the SFC's template + event body
///   scoping where bare state fields resolve to `state.<name>`.
/// - **Event handler names** of the same component as `EVENT` items
///   for `data-flv-click="..."` style refs (bare — the user typing the
///   handler name inside a quoted string).
///
/// **Scope**: MVP — does NOT do fine-grained context detection (we
/// don't distinguish "inside template" vs "inside state block" vs
/// "outside component"). The suggestions are always accurate but may
/// appear in slightly off contexts. Fine-grained routing lives as
/// Session B follow-up if false-positive noise surfaces in practice.
///
/// **Cross-component / imports**: NOT surfaced by this MVP — those
/// would need the same K-4 import-name aggregation the SSR emitter
/// does, plus scope routing per template/event body. Deferred.
pub fn completion_at_position_view(source: &str, line: u32, character: u32) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // Line-preceding text lets us dispatch on `{`, `{#`, `@` prefixes
    // without needing a full lexer pass. `line` is 0-based here (LSP
    // convention).
    let line_text = source.lines().nth(line as usize).unwrap_or("");
    let cursor_col = (character as usize).min(line_text.chars().count());
    let prefix: String = line_text.chars().take(cursor_col).collect();
    let trimmed = prefix.trim_end();

    // (1) `{` or `{#` → template directive completions.
    if trimmed.ends_with("{#") || trimmed.ends_with('{') {
        items.extend(view_directive_completions());
    }

    // (2) `@` at cursor (inside a tag) → event decorator completions.
    if trimmed.ends_with('@') {
        items.extend(view_event_decorator_completions());
    }

    // (3) Always available: state field names + event handler names
    // of the enclosing component. Bare identifiers in a template
    // (`{count}`) or event body (`count = count + 1`) match against
    // these directly. We accept the extra noise in offset positions
    // for the MVP — filtering by "inside template" vs "inside state
    // block" is a fine-grained routing debt.
    //
    // **Robust to partial parses** — the user typing an unterminated
    // `{` in a template makes `view::parse` reject, but we still want
    // to offer completions in that context. We use a lightweight
    // heuristic pass over the source to extract state field names +
    // event names WITHOUT going through the parser. Trade-off: catches
    // extra idents (comments containing `<name>: <type>`, etc.); the
    // false positives are cosmetic — the classic checker downstream
    // catches misuses.
    let (state_fields, event_names) =
        heuristic_state_and_events_for_line(source, line as usize + 1);
    for name in state_fields {
        items.push(CompletionItem {
            label: name.clone(),
            kind: Some(CompletionItemKind::FIELD),
            detail: Some("state field".to_string()),
            ..Default::default()
        });
    }
    for name in event_names {
        items.push(CompletionItem {
            label: name.clone(),
            kind: Some(CompletionItemKind::EVENT),
            detail: Some("event handler".to_string()),
            ..Default::default()
        });
    }

    items
}

/// Session B — Phase 11.8.b (2026-07-18): heuristic extraction of
/// state field names + event handler names from the enclosing
/// component's `state { ... }` and `event <name>(...)` blocks,
/// robust to partially-broken sources (unterminated templates,
/// mid-typing braces, etc.). Runs on the raw source text without
/// invoking the view parser.
///
/// Algorithm:
/// 1. Walk lines forward, tracking the last-seen `component <Name> {`
///    header up to (and including) `cursor_line`.
/// 2. Inside that component's span (defined as the range up to the
///    next `component` header or EOF), collect:
///    - `<name>: <type>` shapes inside `state { ... }` block for
///      state field names.
///    - `event <name>(...)` header lines for event names.
/// 3. Return the two vectors.
///
/// Trade-offs: catches idents from comments or nested blocks. False
/// positives are cosmetic (extra items in the completion list);
/// false negatives (missing valid items) only happen for very
/// contrived shapes not seen in practice.
fn heuristic_state_and_events_for_line(
    source: &str,
    cursor_line: usize,
) -> (Vec<String>, Vec<String>) {
    let mut state_fields: Vec<String> = Vec::new();
    let mut event_names: Vec<String> = Vec::new();
    let mut in_component: Option<()> = None;
    let mut component_starts_at: usize = 0;
    let mut in_state_block: bool = false;
    let mut state_brace_depth: i32 = 0;

    for (idx, line) in source.lines().enumerate() {
        let line_number = idx + 1; // 1-based
        let trimmed = line.trim_start();

        // Component header — flush + start.
        if trimmed.starts_with("component ") {
            if line_number > cursor_line {
                break;
            }
            state_fields.clear();
            event_names.clear();
            in_component = Some(());
            component_starts_at = line_number;
            in_state_block = false;
            state_brace_depth = 0;
            continue;
        }
        if in_component.is_none() {
            continue;
        }

        // State block detection — enter on line containing `state {`.
        if !in_state_block && trimmed.starts_with("state") && line.contains('{') {
            in_state_block = true;
            state_brace_depth = line.matches('{').count() as i32 - line.matches('}').count() as i32;
            // Handle `state { count: Int = 0 }` all on one line.
            if state_brace_depth == 0 {
                extract_state_fields_from_line(line, &mut state_fields);
                in_state_block = false;
                continue;
            }
            continue;
        }
        if in_state_block {
            let opens = line.matches('{').count() as i32;
            let closes = line.matches('}').count() as i32;
            state_brace_depth += opens - closes;
            extract_state_fields_from_line(line, &mut state_fields);
            if state_brace_depth <= 0 {
                in_state_block = false;
            }
            continue;
        }

        // Event handler header — `event <name>(...)` on its own line.
        if trimmed.starts_with("event ") {
            if let Some(rest) = trimmed.strip_prefix("event ") {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    event_names.push(name);
                }
            }
        }
    }

    let _ = component_starts_at;
    (state_fields, event_names)
}

/// Extract `<ident>: <...>` state field names from a single line.
/// Handles the two common shapes: `count: Int = 0` and
/// `title: Str = "hi"`. The regex-free approach is simpler + faster.
fn extract_state_fields_from_line(line: &str, out: &mut Vec<String>) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    // Skip the `state {` header itself.
    if trimmed.starts_with("state ") || trimmed.starts_with("state{") {
        return;
    }
    // Look for `<ident>:` at the start of the trimmed line.
    let mut chars = trimmed.chars().peekable();
    let mut name = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_alphanumeric() || c == '_' {
            name.push(c);
            chars.next();
        } else {
            break;
        }
    }
    if name.is_empty() {
        return;
    }
    // The next non-whitespace char should be `:` for it to count as
    // a state field.
    for c in chars {
        if c.is_whitespace() {
            continue;
        }
        if c == ':' {
            out.push(name);
        }
        return;
    }
}

/// The 5 SFC template directives Fitz recognises today (§9.b of the
/// view plan). Emitted as CompletionItems with `SNIPPET` insertText
/// where useful so tabstops land inside the placeholder.
fn view_directive_completions() -> Vec<CompletionItem> {
    fn snippet(label: &str, insert: &str, detail: &str) -> CompletionItem {
        CompletionItem {
            label: label.to_string(),
            kind: Some(CompletionItemKind::SNIPPET),
            insert_text: Some(insert.to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            detail: Some(detail.to_string()),
            ..Default::default()
        }
    }
    vec![
        snippet(
            "#if",
            "#if $1}\n  $0\n{/if",
            "template directive — conditional block",
        ),
        snippet(
            "#for",
            "#for $1 in $2}\n  $0\n{/for",
            "template directive — iteration",
        ),
        snippet(
            "#else",
            "#else",
            "template directive — else branch of `{#if}`",
        ),
        snippet("/if", "/if", "template directive — close `{#if}`"),
        snippet("/for", "/for", "template directive — close `{#for}`"),
    ]
}

/// The event decorators Fitz recognises on HTML tags today. Post
/// §9.dd + §9.ee the parser accepts arbitrary `@<ident>=...` attrs,
/// but the framework runtime only wires `@click` and `@submit` —
/// surface those two to steer users toward the supported set.
fn view_event_decorator_completions() -> Vec<CompletionItem> {
    fn attr(label: &str, insert: &str, detail: &str) -> CompletionItem {
        CompletionItem {
            label: label.to_string(),
            kind: Some(CompletionItemKind::EVENT),
            insert_text: Some(insert.to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            detail: Some(detail.to_string()),
            ..Default::default()
        }
    }
    vec![
        attr("click", "click=\"$1\"", "event attr — bind DOM click"),
        attr("submit", "submit=\"$1\"", "event attr — bind form submit"),
    ]
}

// Note: initial MVP used a parser-backed `component_containing_line`
// helper, but the raw view parser rejects partially-broken sources
// (unterminated template braces, mid-typing shapes), which is the
// exact context where the user needs completion the most. Replaced
// with `heuristic_state_and_events_for_line` above — regex-free
// line-by-line pass that survives arbitrary partial input.

/// Dispatches on the URI's file extension:
/// - `.fitzv` → view pipeline via `check_view_source_with_base_dir`
///   (base_dir derived from the URI so cross-file `<Child />` resolves).
/// - anything else → classic Fitz pipeline via `check_source`.
///
/// The URI is the primary discriminator; if the URI has no
/// discernible extension (rare — REPL, stdin), the classic pipeline
/// runs by default. Distinguishing on file NAME (rather than
/// content-sniffing) matches how the CLI's `fitz check` /
/// `fitz run` dispatch on the entry path's extension.
pub fn check_source_by_uri(uri: &Url, source: &str) -> Vec<FitzError> {
    if uri_is_fitzv(uri) {
        let base_dir = uri
            .to_file_path()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        check_view_source_with_base_dir(source, base_dir.as_deref())
    } else {
        check_source(source)
    }
}

/// True iff the URI's path ends with `.fitzv` (case-sensitive to
/// match cross-platform filesystem conventions of the source-tree
/// side; case-insensitive filesystems still work because the URI
/// carries the on-disk name verbatim).
pub fn uri_is_fitzv(uri: &Url) -> bool {
    uri.path().ends_with(".fitzv")
}

/// Session B — Phase 11.8.d (2026-07-18): go-to-definition inside a
/// `.fitzv` source. Returns a `Location` pointing at the declaration
/// line of:
/// - The state field declaration (`<name>: <type> = ...` inside the
///   `state { ... }` block) when the cursor is over a bare state
///   field reference in the template or an event body.
/// - The event handler declaration (`event <name>() { ... }`) when
///   the cursor is over the handler name (e.g. inside a
///   `data-flv-click="inc"` string).
///
/// Returns `None` when the identifier under the cursor doesn't match
/// any known state field or event handler in the enclosing
/// component.
///
/// The `uri` is used to build the returned `Location` — the same
/// `.fitzv` file (single-file go-to-def; cross-component refs are
/// out of scope for the MVP).
pub fn definition_at_position_view(
    uri: &Url,
    source: &str,
    line: u32,
    character: u32,
) -> Option<Location> {
    // Extract identifier under cursor (mirrors hover).
    let line_text = source.lines().nth(line as usize)?;
    let chars: Vec<char> = line_text.chars().collect();
    let cursor = (character as usize).min(chars.len());
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut start = cursor;
    while start > 0 && is_ident(chars[start - 1]) {
        start -= 1;
    }
    let mut end = cursor;
    while end < chars.len() && is_ident(chars[end]) {
        end += 1;
    }
    if start == end {
        return None;
    }
    let ident: String = chars[start..end].iter().collect();

    // Find the declaration line inside the enclosing component's
    // state block or event blocks.
    let cursor_line_1_based = line as usize + 1;
    let (decl_line, decl_col) =
        heuristic_declaration_position(source, cursor_line_1_based, &ident)?;

    let range = Range {
        start: Position::new(decl_line, decl_col),
        end: Position::new(decl_line, decl_col + ident.chars().count() as u32),
    };
    Some(Location {
        uri: uri.clone(),
        range,
    })
}

/// Heuristic — find the declaration position of `ident` inside the
/// component enclosing `cursor_line`. Returns `(line, column)` both
/// 0-based (LSP convention) pointing at the `<ident>` in either
/// `<ident>: <type>` (state field decl) or `event <ident>(...)`
/// (event handler decl).
fn heuristic_declaration_position(
    source: &str,
    cursor_line: usize,
    ident: &str,
) -> Option<(u32, u32)> {
    let mut in_component = false;
    let mut in_state_block = false;
    let mut state_brace_depth: i32 = 0;
    let mut best: Option<(u32, u32)> = None;

    for (idx, line) in source.lines().enumerate() {
        let line_number = idx + 1;
        if line_number > cursor_line {
            // Don't scan past the cursor's component to avoid
            // matching a same-named field of a later component.
            let trimmed = line.trim_start();
            if trimmed.starts_with("component ") {
                break;
            }
        }
        let trimmed = line.trim_start();

        if trimmed.starts_with("component ") {
            in_component = true;
            in_state_block = false;
            state_brace_depth = 0;
            best = None;
            continue;
        }
        if !in_component {
            continue;
        }
        // State block open.
        if !in_state_block && trimmed.starts_with("state") && line.contains('{') {
            in_state_block = true;
            state_brace_depth = line.matches('{').count() as i32 - line.matches('}').count() as i32;
            // Same-line state — check field decls now.
            if let Some(col) = find_state_field_position(line, ident) {
                best = Some((idx as u32, col));
            }
            if state_brace_depth == 0 {
                in_state_block = false;
            }
            continue;
        }
        if in_state_block {
            let opens = line.matches('{').count() as i32;
            let closes = line.matches('}').count() as i32;
            state_brace_depth += opens - closes;
            if let Some(col) = find_state_field_position(line, ident) {
                best = Some((idx as u32, col));
            }
            if state_brace_depth <= 0 {
                in_state_block = false;
            }
            continue;
        }
        // Event handler decl on this line.
        if let Some(rest) = trimmed.strip_prefix("event ") {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if name == ident {
                // Compute the column of `<name>` on the raw line.
                let idx_ident = line.find("event ").map(|i| i + "event ".len())?;
                best = Some((idx as u32, idx_ident as u32));
            }
        }
    }
    best
}

/// If `line` contains `<ident>: ...`, return the column of `<ident>`.
fn find_state_field_position(line: &str, ident: &str) -> Option<u32> {
    let trimmed = line.trim_start();
    let leading = line.len() - trimmed.len();
    let after_ident = trimmed.strip_prefix(ident)?;
    // The next non-whitespace char must be `:` for it to count as
    // a state field decl.
    let after_ident_trimmed = after_ident.trim_start();
    if after_ident_trimmed.starts_with(':') {
        Some(leading as u32)
    } else {
        None
    }
}

/// Session B — Phase 11.8.c (2026-07-18): hover inside a `.fitzv`
/// source. Returns a markdown-formatted `Hover` when the cursor is
/// over an identifier that matches:
/// - A state field of the enclosing component → shows
///   `<name>: <type_expr_raw> — state field of <Component>`.
/// - An event handler of the enclosing component → shows
///   `event <name>() — event handler of <Component>`.
///
/// Otherwise returns `None` (no hover). MVP scope — does NOT do
/// TypeInfo-based per-node hover (that requires wiring the classic
/// checker over the emitted classic Fitz source, which is a bigger
/// piece of work — Session B follow-up if false-negatives surface).
///
/// The identifier extraction reuses `ident_under_cursor` (already
/// used by the classic-Fitz hover/go-to-def paths). The `line` /
/// `character` are LSP-style 0-based.
pub fn hover_at_position_view(source: &str, line: u32, character: u32) -> Option<Hover> {
    // 1. Extract the identifier under the cursor via the line's
    //    ident-char sweep. We do this inline instead of reusing
    //    `ident_range_at_position` because we want the STRING, not
    //    just the range.
    let line_text = source.lines().nth(line as usize)?;
    let chars: Vec<char> = line_text.chars().collect();
    let cursor = (character as usize).min(chars.len());
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut start = cursor;
    while start > 0 && is_ident(chars[start - 1]) {
        start -= 1;
    }
    let mut end = cursor;
    while end < chars.len() && is_ident(chars[end]) {
        end += 1;
    }
    if start == end {
        return None;
    }
    let ident: String = chars[start..end].iter().collect();
    // Bail out on Fitz keywords / SFC block markers that would trip
    // false positives on hover.
    let is_keyword = matches!(
        ident.as_str(),
        "component" | "state" | "event" | "from" | "import" | "as" | "template" | "style"
    );
    if is_keyword {
        return None;
    }

    // 2. Heuristic component scan — same helper used by
    //    completion_at_position_view. Returns the state field names
    //    + event handler names of the component enclosing the cursor
    //    line. To get the state field's *type*, do a second pass
    //    matching `<ident>: <type_expr>` inside the same span.
    let cursor_line_1_based = line as usize + 1;
    let (state_fields, event_names) =
        heuristic_state_and_events_for_line(source, cursor_line_1_based);

    // 3a. State field?
    if state_fields.iter().any(|n| n == &ident) {
        let (component_name, type_hint) =
            heuristic_state_field_type_for(source, cursor_line_1_based, &ident);
        let body = match type_hint {
            Some(t) => format!(
                "```fitz\n{ident}: {t}\n```\n\n**state field** of `{}`",
                component_name.as_deref().unwrap_or("component")
            ),
            None => format!(
                "```fitz\n{ident}\n```\n\n**state field** of `{}`",
                component_name.as_deref().unwrap_or("component")
            ),
        };
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: body,
            }),
            range: None,
        });
    }

    // 3b. Event handler?
    if event_names.iter().any(|n| n == &ident) {
        let component_name = heuristic_component_name_for_line(source, cursor_line_1_based)
            .unwrap_or_else(|| "component".to_string());
        let body =
            format!("```fitz\nevent {ident}()\n```\n\n**event handler** of `{component_name}`");
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: body,
            }),
            range: None,
        });
    }

    None
}

/// Heuristic — extract the `<type_expr_raw>` associated with a state
/// field named `field_name` from the component enclosing
/// `cursor_line`. Returns `(component_name, Option<type_hint>)`.
fn heuristic_state_field_type_for(
    source: &str,
    cursor_line: usize,
    field_name: &str,
) -> (Option<String>, Option<String>) {
    let mut component_name: Option<String> = None;
    let mut current_component_name: Option<String> = None;
    let mut in_state_block = false;
    let mut state_brace_depth: i32 = 0;
    let mut type_hint: Option<String> = None;

    for (idx, line) in source.lines().enumerate() {
        let line_number = idx + 1;
        let trimmed = line.trim_start();
        if line_number > cursor_line {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("component ") {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                current_component_name = Some(name.clone());
                component_name = Some(name);
                in_state_block = false;
                state_brace_depth = 0;
                type_hint = None;
            }
            continue;
        }
        if !in_state_block && trimmed.starts_with("state") && line.contains('{') {
            in_state_block = true;
            state_brace_depth = line.matches('{').count() as i32 - line.matches('}').count() as i32;
            if state_brace_depth == 0 {
                if let Some(t) = extract_field_type_from_line(line, field_name) {
                    type_hint = Some(t);
                }
                in_state_block = false;
            }
            continue;
        }
        if in_state_block {
            let opens = line.matches('{').count() as i32;
            let closes = line.matches('}').count() as i32;
            state_brace_depth += opens - closes;
            if let Some(t) = extract_field_type_from_line(line, field_name) {
                type_hint = Some(t);
            }
            if state_brace_depth <= 0 {
                in_state_block = false;
            }
        }
    }
    let _ = current_component_name;
    (component_name, type_hint)
}

/// Match a `<field_name>: <type_expr>` shape on the line and return
/// the type_expr (trimmed, and truncated at `=` or end-of-line).
fn extract_field_type_from_line(line: &str, field_name: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix(field_name)?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let (type_part, _) = rest.split_once('=').unwrap_or((rest, ""));
    let cleaned = type_part.trim().trim_end_matches(',').trim();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_string())
    }
}

/// Look up the name of the component enclosing `cursor_line`.
fn heuristic_component_name_for_line(source: &str, cursor_line: usize) -> Option<String> {
    let mut current: Option<String> = None;
    for (idx, line) in source.lines().enumerate() {
        let line_number = idx + 1;
        if line_number > cursor_line {
            break;
        }
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("component ") {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                current = Some(name);
            }
        }
    }
    current
}

/// LSP-style pipeline retaining `Program`, `TypeEnv`, `TypeInfo`, and
/// `DefinitionInfo`. Variant of `check_source` for consumers that need
/// the AST (Phase 9.x.4 — scope-level autocomplete enumerates
/// top-level), the env for resolving nominal names (hover/completion),
/// the per-node type side-table (hover, after-dot autocomplete), and
/// the per-use definition side-table (go-to-definition).
///
/// If the pipeline aborts before the checker (lexer error), Program
/// is empty and so are the side-tables.
///
/// **Single-file mode**: this entry point does NOT load the import
/// graph. For a cross-module-aware variant that resolves
/// `@auth_provider`/`@background` declared in imported `.fitz`
/// modules, use `check_source_with_types_and_base_dir` (the LSP bin
/// uses it with the open document's URI).
pub fn check_source_with_types(
    source: &str,
) -> (Program, TypeEnv, TypeInfo, DefinitionInfo, Vec<FitzError>) {
    check_source_with_types_and_base_dir(source, None)
}

/// LSP-style pipeline that ALSO walks the import graph of the open
/// document (single-level, parallel to
/// `main.rs::pre_scan_imported_auth_provider`/`pre_scan_imported_background_fns`).
///
/// **Why**: without this, the LSP runs the checker on the open module
/// in isolation and emits false positives for cross-module decorators
/// — `@authenticated`/`@admin`/`@requires("role")` against an
/// `@auth_provider` declared in `auth.fitz`, or `spawn(<imp_fn>(...))`
/// against an `@background` fn declared in a sibling module. The
/// codegen and the interpreter both load the import graph and resolve
/// these correctly; the LSP needs to do the same to match their
/// diagnostics.
///
/// **Scope**: when `base_dir` is `Some`, walks each `Stmt::Import` /
/// `Stmt::FromImport` of the open module, resolves the corresponding
/// `.fitz` file relative to `base_dir`, parses it, extracts the
/// cross-module signal via
/// `types::extract_auth_provider_signature` /
/// `types::extract_background_fn_names`, and feeds it into the
/// `TypeEnv` via `set_imported_auth_provider` /
/// `add_imported_background_fns` BEFORE running `check_with_env`. The
/// checker's existing fallbacks (`collect_auth_provider` /
/// `collect_background_fns`) then resolve cross-module references
/// without emitting false positives.
///
/// **Error policy**: module read/parse errors are silenced (silent
/// fallback — parallel to `pre_scan_imported_auth_provider` in
/// `main.rs`). The LSP's job is to enrich the env, not to validate
/// imports — that responsibility lives in the codegen/runtime loader.
///
/// **Single level of depth**: parallel to W12/B10 in main.rs, this
/// does NOT recurse into transitive imports. The 90% case
/// (handler-per-feature + provider in `auth.fitz`) is covered;
/// transitive resolution lives as documented residual debt.
///
/// **Dependencies**: this variant does NOT consult `dep_registry`
/// (the LSP doesn't have access to the manifest). For LSP users with
/// path deps from `fitz.toml`, the workaround is to declare imports
/// with relative paths in the open document. Same limitation as the
/// existing `resolve_cross_module_definition` / `from_import_completions`.
pub fn check_source_with_types_and_base_dir(
    source: &str,
    base_dir: Option<&std::path::Path>,
) -> (Program, TypeEnv, TypeInfo, DefinitionInfo, Vec<FitzError>) {
    let tokens = match tokenize(source) {
        Ok(t) => t,
        Err(e) => {
            return (
                Vec::new(),
                TypeEnv::default(),
                TypeInfo::new(),
                DefinitionInfo::new(),
                vec![e],
            );
        }
    };
    let (program, mut errors) = parse_with_recovery(tokens);

    // When the caller knows where the open document lives on disk,
    // pre-scan its direct imports for cross-module `@auth_provider`
    // and `@background` fns, then enrich the TypeEnv via the same
    // setters used by `main.rs::check_program_with_pyi_stubs_and_deps`.
    // Falls back to the single-file path when `base_dir` is `None`
    // (REPL, unit tests, callers without file context).
    if let Some(bd) = base_dir {
        let (mut env, resolve_errors) =
            resolve_program_with_env(&program, TypeEnv::new(), Vec::new());
        if let Some(provider) = pre_scan_imported_auth_provider_lsp(&program, bd) {
            env.set_imported_auth_provider(provider);
        }
        let bg_names = pre_scan_imported_background_fns_lsp(&program, bd);
        if !bg_names.is_empty() {
            env.add_imported_background_fns(bg_names);
        }
        let (env, type_info, def_info, mut type_errors) =
            check_with_env(&program, env, resolve_errors);
        errors.append(&mut type_errors);
        (program, env, type_info, def_info, errors)
    } else {
        let (env, type_info, def_info, mut type_errors) = check_program(&program);
        errors.append(&mut type_errors);
        (program, env, type_info, def_info, errors)
    }
}

/// LSP variant of `main.rs::pre_scan_imported_auth_provider` (W12).
/// Walks `Stmt::Import` / `Stmt::FromImport`, resolves each module
/// to a `.fitz` file relative to `base_dir`, parses it strictly
/// (since the imported module is read off-disk and we only care
/// about extracting the provider signature — recoverable errors
/// inside it are not the local checker's concern), and returns the
/// first `@auth_provider` found.
///
/// Returns `None` if no imported module declares a provider OR if
/// reading/parsing failed silently (deliberate — see
/// `check_source_with_types_and_base_dir` for the error policy).
fn pre_scan_imported_auth_provider_lsp(
    program: &Program,
    base_dir: &std::path::Path,
) -> Option<crate::types::ImportedAuthProvider> {
    for stmt in program {
        let (path_segments, module_binding_name) = match stmt {
            Stmt::Import { path, alias, .. } => {
                if path.first().map(String::as_str) == Some("python") {
                    continue;
                }
                let binding = alias
                    .clone()
                    .or_else(|| path.last().cloned())
                    .unwrap_or_default();
                (path.clone(), binding)
            }
            Stmt::FromImport { path, .. } => {
                if path.first().map(String::as_str) == Some("python") {
                    continue;
                }
                let binding = path.last().cloned().unwrap_or_default();
                (path.clone(), binding)
            }
            _ => continue,
        };
        let Some(file_path) = resolve_import_file_path_lsp(&path_segments, base_dir) else {
            continue;
        };
        let Ok(source_raw) = std::fs::read_to_string(&file_path) else {
            continue;
        };
        // Phase 11.6.d — `.fitzv` transparent handling.
        let source = if crate::view::is_fitzv_extension(&file_path) {
            match crate::view::transform_fitzv_source(&source_raw, &file_path) {
                Ok(s) => s,
                Err(_) => continue,
            }
        } else {
            source_raw
        };
        let Ok(tokens) = tokenize(&source) else {
            continue;
        };
        let Ok(module_program) = parse_strict(tokens) else {
            continue;
        };
        if let Some(mut provider) =
            extract_auth_provider_signature(&module_program, &module_binding_name)
        {
            // v0.37.3 — the provider's `User` type may be imported into the
            // provider's own module (`auth.fitz` does `from models import
            // User`). Follow the provider module's imports to resolve the
            // `role: Str` field so `@admin`/`@requires` don't show a false
            // diagnostic. Parallel to the codegen fix in
            // `pre_scan_imported_auth_provider_for_loader`.
            if !provider.has_role_field {
                let provider_base = file_path.parent().unwrap_or(base_dir);
                if lsp_role_field_across_module_imports(
                    &module_program,
                    &provider.user_type_name,
                    provider_base,
                ) {
                    provider.has_role_field = true;
                }
            }
            return Some(provider);
        }
    }
    None
}

/// v0.37.3 — LSP variant of the codegen
/// `resolve_role_field_across_module_imports`: follows a module's DIRECT
/// imports to find a sibling that declares `type <type_name> { ... role:
/// Str }`. Only direct imports: Fitz has no re-export, so the provider's
/// `User` is always imported directly from its declaring module (a
/// "transitive" `from A import User` where A only re-imported it errors).
fn lsp_role_field_across_module_imports(
    module_program: &Program,
    type_name: &str,
    base_dir: &std::path::Path,
) -> bool {
    for stmt in module_program {
        let path_segments = match stmt {
            Stmt::Import { path, .. } | Stmt::FromImport { path, .. } => {
                if path.first().map(String::as_str) == Some("python") {
                    continue;
                }
                path.clone()
            }
            _ => continue,
        };
        let Some(file_path) = resolve_import_file_path_lsp(&path_segments, base_dir) else {
            continue;
        };
        let Ok(source_raw) = std::fs::read_to_string(&file_path) else {
            continue;
        };
        let source = if crate::view::is_fitzv_extension(&file_path) {
            match crate::view::transform_fitzv_source(&source_raw, &file_path) {
                Ok(s) => s,
                Err(_) => continue,
            }
        } else {
            source_raw
        };
        let Ok(tokens) = tokenize(&source) else {
            continue;
        };
        let Ok(imported_program) = parse_strict(tokens) else {
            continue;
        };
        if crate::types::type_decl_has_role_field(&imported_program, type_name) {
            return true;
        }
    }
    false
}

/// LSP variant of `main.rs::pre_scan_imported_background_fns` (B10).
/// Walks direct imports and collects the names of top-level `@background`
/// fns declared in each imported module. Silent fallback on
/// read/parse errors (same policy as `pre_scan_imported_auth_provider_lsp`).
fn pre_scan_imported_background_fns_lsp(
    program: &Program,
    base_dir: &std::path::Path,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for stmt in program {
        let path_segments = match stmt {
            Stmt::Import { path, .. } => {
                if path.first().map(String::as_str) == Some("python") {
                    continue;
                }
                path.clone()
            }
            Stmt::FromImport { path, .. } => {
                if path.first().map(String::as_str) == Some("python") {
                    continue;
                }
                path.clone()
            }
            _ => continue,
        };
        let Some(file_path) = resolve_import_file_path_lsp(&path_segments, base_dir) else {
            continue;
        };
        let Ok(source_raw) = std::fs::read_to_string(&file_path) else {
            continue;
        };
        // Phase 11.6.d — `.fitzv` transparent handling.
        let source = if crate::view::is_fitzv_extension(&file_path) {
            match crate::view::transform_fitzv_source(&source_raw, &file_path) {
                Ok(s) => s,
                Err(_) => continue,
            }
        } else {
            source_raw
        };
        let Ok(tokens) = tokenize(&source) else {
            continue;
        };
        let Ok(module_program) = parse_strict(tokens) else {
            continue;
        };
        out.extend(extract_background_fn_names(&module_program));
    }
    out
}

/// LSP-only resolver: maps the segments of an `import`/`from`
/// statement to a `.fitz` file path relative to `base_dir`. Mirror
/// of `main.rs::resolve_import_file_path` minus the `dep_registry`
/// resolution — the LSP doesn't have access to the project manifest
/// (parallel to existing `resolve_cross_module_definition` /
/// `from_import_completions`). Returns `None` if the candidate path
/// doesn't exist on disk.
fn resolve_import_file_path_lsp(
    segments: &[String],
    base_dir: &std::path::Path,
) -> Option<std::path::PathBuf> {
    if segments.is_empty() {
        return None;
    }
    let mut dir = base_dir.to_path_buf();
    for seg in &segments[..segments.len().saturating_sub(1)] {
        dir.push(seg);
    }
    // Phase 11.6.d — try `.fitz` first, `.fitzv` as fallback.
    let last = segments.last()?;
    crate::view::resolve_module_file_candidates(&dir, last)
}

/// Converts a list of `FitzError` into LSP `Diagnostic`s. Pure
/// function — does not touch the server or I/O. The test suite covers
/// the mapping rules (1-based Fitz → 0-based LSP, no position →
/// degenerate range, hint → concatenated onto the message).
pub fn fitz_errors_to_diagnostics(errors: &[FitzError]) -> Vec<Diagnostic> {
    errors
        .iter()
        .map(|e| error_to_diagnostic(e, None))
        .collect()
}

/// LSPy mini-batch — variant with source text that computes the exact
/// Range for errors whose position coincides with an identifier. Used
/// by the LSP bin, which has the doc text. The old public signature is
/// kept as a wrapper.
pub fn fitz_errors_to_diagnostics_with_source(
    errors: &[FitzError],
    source: &str,
) -> Vec<Diagnostic> {
    errors
        .iter()
        .map(|e| error_to_diagnostic(e, Some(source)))
        .collect()
}

fn error_to_diagnostic(err: &FitzError, source: Option<&str>) -> Diagnostic {
    // Fitz uses a 1-based convention for line/column (the lexer starts
    // at `line: 1, column: 1`). LSP uses 0-based in `Position`. When
    // line == 0 && column == 0 is the "no position" sentinel (see
    // `FitzError::Display` in `error.rs`); we map it to a degenerate
    // range at the start of the document.
    let range = if err.line == 0 && err.column == 0 {
        Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        }
    } else {
        let line = (err.line.saturating_sub(1)) as u32;
        let col = (err.column.saturating_sub(1)) as u32;
        // LSPy mini-batch — if we have source and the position falls
        // on an ident, expand the range to the full ident. Otherwise
        // fallback to 1 char.
        let fallback = Range {
            start: Position::new(line, col),
            end: Position::new(line, col + 1),
        };
        match source {
            Some(text) => {
                ident_range_at_position(text, line as usize, col as usize).unwrap_or(fallback)
            }
            None => fallback,
        }
    };

    // We concatenate the hint onto the message because LSP has no
    // dedicated field for suggestions. VSCode renders `\n` in the
    // tooltip; the format matches how `FitzError::Display` emits it
    // on the CLI.
    let mut message = err.message.clone();
    if let Some(hint) = &err.hint {
        message.push_str("\n  Hint: ");
        message.push_str(hint);
    }

    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("fitz".into()),
        message,
        ..Diagnostic::default()
    }
}

// ---------------------------------------------------------------------------
// Hover (Phase 9.x.2)
// ---------------------------------------------------------------------------

/// Finds the type of the node "under the cursor" given an LSP position
/// (0-based). Pragmatic MVP heuristic: filters the `TypeInfo` entries
/// whose line matches the cursor and whose column is less than or
/// equal to the cursor's, and returns the `Type` with the maximum
/// column (the `Expr` closest to the left on the same line).
///
/// **Why a heuristic and not an exact range**: Fitz `Span`s today
/// only store the start of the node (debt S1.Pattern/TypeExpr).
/// Without `end_span`, we can't say "the cursor is inside node X";
/// we assume the last Expr that started before the cursor on the same
/// line is the most likely one. Covers 90% of the case (cursor on or
/// immediately after an identifier/literal). Refinable once nodes
/// carry a full span.
///
/// **Collisions in `TypeInfo`**: when two distinct `Expr`s share a
/// span (typically a `BinOp` and its first operand), `TypeInfo` only
/// keeps the last one written — inherited from F16. In practice the
/// type of the "larger" Expr tends to be what the user wants to see
/// on hover.
///
/// **v0.13.2 — encoding of `character`**: this function expects
/// `character` in **Unicode chars of the lexer** (not UTF-16 code
/// units as they come from the LSP client). The responsible caller
/// is the bin backend (`src/bin/fitz-lsp.rs`), which translates the
/// client's `position.character` with
/// `utf16_to_unicode_char(text, line, char_utf16)` before calling
/// here. For code without SMP chars (all real code in practice) the
/// translation is the identity.
pub fn hover_for_position(type_info: &TypeInfo, line: u32, character: u32) -> Option<&Type> {
    // LSP 0-based → Fitz 1-based.
    let target_line = (line as usize) + 1;
    let target_col = (character as usize) + 1;
    type_info
        .iter()
        .filter(|(key, _)| key.0 == target_line && key.1 <= target_col)
        .max_by_key(|(key, _)| key.1)
        .map(|(_, ty)| ty)
}

/// Builds the LSP `Hover` response from the `Type` found under the
/// cursor. The type is rendered as a Fitz code block in markdown —
/// VSCode shows it with native syntax highlighting.
///
/// Legacy variant without Range. Kept for compatibility with pre-LSPy
/// call sites. `make_hover_with_range` replaces it with the Range
/// computed from the ident under the cursor.
pub fn make_hover(ty: &Type, env: &TypeEnv) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("```fitz\n{}\n```", ty.display(env)),
        }),
        range: None,
    }
}

/// LSPy mini-batch — version with a Range computed from the symbol
/// under the cursor. The range covers exactly the identifier, so
/// VSCode highlights the token instead of only showing the tooltip.
/// If there is no ident at the cursor position, `range = None`
/// (fallback).
///
/// v0.10.32 (Tier D.2) — if the type is a `Type::Nominal(id)` with
/// `@table` metadata, we append the emitted `CREATE TABLE` SQL to
/// the markdown (below the type display). Useful to debug the SQL
/// shape without opening `fitz db diff` or reviewing the migration
/// manually.
pub fn make_hover_with_range(
    ty: &Type,
    env: &TypeEnv,
    program: &crate::ast::Program,
    text: &str,
    line: u32,
    character: u32,
) -> Hover {
    let range = ident_range_at_position(text, line as usize, character as usize);
    let mut value = format!("```fitz\n{}\n```", ty.display(env));
    // v0.10.32 (Tier D.2) — append CREATE TABLE SQL if applicable.
    if let Some(sql) = try_table_create_sql(ty, env, program) {
        value.push_str(
            "\n\n---\n\n**`CREATE TABLE` emitted** (via `fitz db diff/migrate`):\n\n```sql\n",
        );
        value.push_str(&sql);
        value.push_str("\n```");
    }
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range,
    }
}

/// v0.10.32 (Tier D.2) — if `ty` is a `Type::Nominal(id)` with a
/// registered `TableMetadata`, builds the `CREATE TABLE` SQL that
/// `fitz db diff/migrate` would emit for that type. Returns `None`
/// for non-`@table` types or if schema construction fails (typo in
/// `@belongs_to`, etc.).
///
/// Reuses `migrations::schema_from_program` +
/// `migrations::create_table_sql_for` to produce SQL identical to
/// `fitz db diff` — no divergence between what the LSP shows and what
/// the migrator emits.
fn try_table_create_sql(ty: &Type, env: &TypeEnv, program: &crate::ast::Program) -> Option<String> {
    let type_id = match ty {
        Type::Nominal(id) => *id,
        _ => return None,
    };
    // Check that it is @table (otherwise there is no SQL to show).
    let table_meta = env.table_metadata(type_id)?;
    let target_sql_name = table_meta.sql_name.clone();
    let target_schema = table_meta.schema.clone();
    // Build the entire Schema. If it fails (typo in relations, etc.),
    // skip the augment — returning None leaves the hover with just the
    // type. The user sees the checker errors elsewhere.
    let schema = crate::migrations::schema_from_program(program, env).ok()?;
    // Find the matching table and emit the CREATE.
    let table = schema
        .tables
        .iter()
        .find(|t| t.name == target_sql_name && t.schema == target_schema)?;
    Some(crate::migrations::create_table_sql_for(table))
}

// ---------------------------------------------------------------------------
// Go-to-definition (Phase 9.x.3)
// ---------------------------------------------------------------------------

/// Finds the `Span` of the declaration of the ident under the cursor
/// given an LSP position (0-based). Same heuristic as
/// `hover_for_position`: filters `DefinitionInfo` entries whose line
/// matches the cursor and whose column is less than or equal to the
/// cursor's, and returns the `def_span` with the maximum column (the
/// ident closest to the left on the same line).
///
/// The returned span points at the position of the declaration
/// (1-based Fitz). The caller converts it to an LSP `Range` (0-based)
/// via `make_definition_location`.
///
/// **v0.13.2 — encoding of `character`**: this function expects
/// `character` in **Unicode chars of the lexer** (not UTF-16 code
/// units as they come from the LSP client). The responsible caller
/// is the bin backend (`src/bin/fitz-lsp.rs`), which translates the
/// client's `position.character` with
/// `utf16_to_unicode_char(text, line, char_utf16)` before calling
/// here. Same trade-off as `hover_for_position`.
pub fn definition_for_position(
    def_info: &DefinitionInfo,
    line: u32,
    character: u32,
) -> Option<Span> {
    // LSP 0-based → Fitz 1-based.
    let target_line = (line as usize) + 1;
    let target_col = (character as usize) + 1;
    def_info
        .iter()
        .filter(|(key, _)| key.0 == target_line && key.1 <= target_col)
        .max_by_key(|(key, _)| key.1)
        .map(|(_, def_span)| *def_span)
}

/// Legacy variant: 1-character range (no source context).
/// `make_definition_location_with_source` replaces it when source
/// text is available to compute the exact end.
pub fn make_definition_location(uri: Url, def_span: Span) -> Location {
    let line = (def_span.line.saturating_sub(1)) as u32;
    let col = (def_span.column.saturating_sub(1)) as u32;
    Location {
        uri,
        range: Range {
            start: Position::new(line, col),
            end: Position::new(line, col + 1),
        },
    }
}

/// LSPy mini-batch — variant with exact Range. If `source` is
/// available and `def_span` points to an ident, we compute the end
/// by reading the ident from the source line. Otherwise, fallback to
/// the 1-char range. Covers cross-module: the caller passes the
/// source of the target file (not of the open document).
pub fn make_definition_location_with_source(
    uri: Url,
    def_span: Span,
    source: Option<&str>,
) -> Location {
    let line0 = (def_span.line.saturating_sub(1)) as u32;
    let col0 = (def_span.column.saturating_sub(1)) as u32;
    let range = match source {
        Some(text) => ident_range_from_def(text, def_span).unwrap_or_else(|| Range {
            start: Position::new(line0, col0),
            end: Position::new(line0, col0 + 1),
        }),
        None => Range {
            start: Position::new(line0, col0),
            end: Position::new(line0, col0 + 1),
        },
    };
    Location { uri, range }
}

/// LSPy mini-batch — extracts the LSP `Range` of the identifier that
/// starts at the `def_span` position in the source. Reads the line,
/// finds the first run of ident chars (alphanum + `_`) that starts
/// at or near the column, and returns its 0-based range. Returns
/// None if there is no ident at that position (typedef/let/fn keyword
/// in the span, or stmt span with an arbitrary Stmt::Expr inside).
fn ident_range_from_def(source: &str, def_span: Span) -> Option<Range> {
    let line_idx = def_span.line.saturating_sub(1);
    let col_idx = def_span.column.saturating_sub(1);
    let line = source.lines().nth(line_idx)?;
    let chars: Vec<char> = line.chars().collect();
    let is_ident_char = |c: char| c.is_alphanumeric() || c == '_';

    // The def_span may point at the keyword (`let`, `fn`, `type`) or
    // at the ident itself. We look for the first ident starting from
    // col_idx, skipping keywords + whitespace.
    let mut i = col_idx.min(chars.len());
    // Skip keyword tokens (`let`, `fn`, `type`, `static`, `async`).
    for kw in ["let ", "fn ", "type ", "static ", "async fn ", "async "] {
        let kw_chars: Vec<char> = kw.chars().collect();
        if chars.len().saturating_sub(i) >= kw_chars.len()
            && chars[i..i + kw_chars.len()] == kw_chars[..]
        {
            i += kw_chars.len();
            break;
        }
    }
    // Skip whitespace.
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    // Find the ident run.
    if i >= chars.len() || !is_ident_char(chars[i]) {
        return None;
    }
    let start = i;
    while i < chars.len() && is_ident_char(chars[i]) {
        i += 1;
    }
    let end = i;
    if start == end {
        return None;
    }
    Some(Range {
        start: Position::new(line_idx as u32, start as u32),
        end: Position::new(line_idx as u32, end as u32),
    })
}

/// LSPy mini-batch — LSP Range of the identifier UNDER the cursor
/// (not "starting at" like `ident_range_from_def`). For hover: the
/// cursor may be in the middle of an ident; we want the full range
/// of the ident, not the one starting at the cursor's column.
fn ident_range_at_position(source: &str, line_idx: usize, char_idx: usize) -> Option<Range> {
    let line = source.lines().nth(line_idx)?;
    let chars: Vec<char> = line.chars().collect();
    let is_ident_char = |c: char| c.is_alphanumeric() || c == '_';
    let cursor = char_idx.min(chars.len());

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
    Some(Range {
        start: Position::new(line_idx as u32, start as u32),
        end: Position::new(line_idx as u32, end as u32),
    })
}

/// LSPx mini-batch — cross-module go-to-definition. If `def_span`
/// points at a `Stmt::Import` or `Stmt::FromImport` of `program`,
/// resolves the target file, parses it, and looks for the actual
/// declaration of the symbol. Returns `(Url of the target module,
/// Span of the decl inside the module)`. If resolution fails (path
/// does not exist, symbol not found, etc.), returns `None` — the
/// caller uses the local Location as fallback.
///
/// `doc_uri` is the URI of the open document; we use it only to
/// resolve the `base_dir` from which relative imports are looked up.
/// The result URI is that of the target module.
///
/// `target_name` is the ident under the cursor at the moment of the
/// goto-def. For `import foo` the name may be `foo` (namespace);
/// for `from foo import X, Y` it must be `X` or `Y`.
pub fn resolve_cross_module_definition(
    program: &Program,
    doc_uri: &Url,
    target_span: Span,
    target_name: &str,
) -> Option<(Url, Span)> {
    // Only makes sense if the doc is a `file://`.
    let doc_path = doc_uri.to_file_path().ok()?;
    let base_dir = doc_path.parent()?;

    // Look for the Stmt::Import / Stmt::FromImport whose span matches.
    // `module_path` = segments of the imported path.
    // `target_item` = name of the symbol to look up in the target
    //                 module, or None if it is a namespace `import`
    //                 (points at the top of the module).
    let (module_path, target_item): (Vec<String>, Option<String>) = {
        let mut found: Option<(Vec<String>, Option<String>)> = None;
        for stmt in program {
            match stmt {
                Stmt::Import { path, span, .. } if *span == target_span => {
                    let last = path.last()?;
                    if target_name == last {
                        found = Some((path.clone(), None));
                    }
                    break;
                }
                Stmt::FromImport {
                    path, names, span, ..
                } if *span == target_span => {
                    let item = names.iter().find_map(|(n, alias)| {
                        if alias.as_deref() == Some(target_name) || n == target_name {
                            Some(n.clone())
                        } else {
                            None
                        }
                    });
                    if let Some(it) = item {
                        found = Some((path.clone(), Some(it)));
                    }
                    break;
                }
                _ => {}
            }
        }
        found?
    };

    // Resolve the path to a real `.fitz` (or `.fitzv`) file.
    // Loader convention: `path = ["foo", "bar"]` →
    // `<base>/foo/bar.fitz`. Phase 11.6.d: also try `.fitzv`
    // as fallback for cross-module go-to-def on view SFCs.
    if module_path.is_empty() {
        return None;
    }
    let mut target_dir = base_dir.to_path_buf();
    for comp in &module_path[..module_path.len().saturating_sub(1)] {
        target_dir.push(comp);
    }
    let last = module_path.last()?;
    let target_path = crate::view::resolve_module_file_candidates(&target_dir, last)?;
    let target_path = target_path.canonicalize().ok()?;

    // Parse the target file and look up the declaration.
    let source_raw = std::fs::read_to_string(&target_path).ok()?;
    // Phase 11.6.d — `.fitzv` transparent handling.
    let source = if crate::view::is_fitzv_extension(&target_path) {
        crate::view::transform_fitzv_source(&source_raw, &target_path).ok()?
    } else {
        source_raw
    };
    let tokens = tokenize(&source).ok()?;
    let (target_program, _errs) = parse_with_recovery(tokens);

    let target_decl_span = match target_item {
        Some(item) => find_top_level_decl(&target_program, &item)?,
        // Namespace import: point at the first stmt of the module (top).
        None => target_program.first().map(|s| s.span())?,
    };

    let target_uri = Url::from_file_path(&target_path).ok()?;
    Some((target_uri, target_decl_span))
}

/// Looks for a top-level declaration with the given name in a
/// module's AST. Covers `Stmt::FnDef`, `Stmt::TypeDef`, and
/// `Stmt::Assign` with an Ident target (module consts). Returns the
/// span of the declaration for cross-module go-to-def.
fn find_top_level_decl(program: &Program, name: &str) -> Option<Span> {
    use crate::ast::AssignTarget;
    for stmt in program {
        match stmt {
            Stmt::FnDef { name: n, span, .. } if n == name => return Some(*span),
            Stmt::TypeDef { name: n, span, .. } if n == name => return Some(*span),
            Stmt::Assign {
                target: AssignTarget::Ident(n, _),
                span,
                ..
            } if n == name => {
                return Some(*span);
            }
            _ => {}
        }
    }
    None
}

// ---------------------------------------------------------------------------
// LSPz mini-batch (v0.9.47) — Completion in `from <mod> import |`
// ---------------------------------------------------------------------------

/// Enumerates the exportable symbols (fns, types, consts) of the
/// module identified by `mod_path`, relative to `doc_uri`. Loader
/// convention: `mod_path = ["foo"]` → `<base>/foo.fitz`;
/// `["sub", "utils"]` → `<base>/sub/utils.fitz` (1 dir nesting).
/// Returns an empty list if the file does not exist or does not parse.
pub fn from_import_completions(doc_uri: &Url, mod_path: &[String]) -> Vec<CompletionItem> {
    use crate::ast::AssignTarget;
    if mod_path.is_empty() {
        return Vec::new();
    }
    let doc_path = match doc_uri.to_file_path() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    let Some(base_dir) = doc_path.parent() else {
        return Vec::new();
    };
    // Phase 11.6.d — try `.fitz` first, `.fitzv` as fallback.
    let mut target_dir = base_dir.to_path_buf();
    for comp in &mod_path[..mod_path.len().saturating_sub(1)] {
        target_dir.push(comp);
    }
    let Some(last) = mod_path.last() else {
        return Vec::new();
    };
    let target_path = match crate::view::resolve_module_file_candidates(&target_dir, last) {
        Some(p) => p,
        None => return Vec::new(),
    };
    let source_raw = match std::fs::read_to_string(&target_path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    // Phase 11.6.d — `.fitzv` transparent handling.
    let source = if crate::view::is_fitzv_extension(&target_path) {
        match crate::view::transform_fitzv_source(&source_raw, &target_path) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        }
    } else {
        source_raw
    };
    let tokens = match tokenize(&source) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let (program, _errs) = parse_with_recovery(tokens);
    let mut items: Vec<CompletionItem> = Vec::new();
    for stmt in &program {
        match stmt {
            Stmt::FnDef {
                name,
                params,
                return_type,
                is_async,
                ..
            } => {
                let params_str = params
                    .iter()
                    .map(|p| {
                        let ty = p
                            .type_
                            .as_ref()
                            .map(|t| t.display_name())
                            .unwrap_or_else(|| "Any".into());
                        format!("{}: {}", p.name, ty)
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let ret_str = return_type
                    .as_ref()
                    .map(|t| t.display_name())
                    .unwrap_or_else(|| "Any".into());
                let prefix = if *is_async { "async fn" } else { "fn" };
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: Some(format!("{}({}) -> {}", prefix, params_str, ret_str)),
                    ..CompletionItem::default()
                });
            }
            Stmt::TypeDef { name, .. } => {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::CLASS),
                    ..CompletionItem::default()
                });
            }
            Stmt::Assign {
                target: AssignTarget::Ident(name, _),
                ..
            } => {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::CONSTANT),
                    ..CompletionItem::default()
                });
            }
            _ => {}
        }
    }
    items
}

// ---------------------------------------------------------------------------
// Contextual autocomplete (Phase 9.x.4)
// ---------------------------------------------------------------------------

/// Context resolved in `detect_completion_context`. Determines which
/// kind of completion to return.
#[derive(Debug, PartialEq)]
enum CompletionContext {
    /// `obj.` or `obj.partial` — the receiver is an identifier whose
    /// type we are looking up. We carry:
    /// - `recv_name`: for the "look up top-level by name" fallback
    ///   when TypeInfo doesn't have the ident (typical case: the
    ///   parser aborted the whole stmt because of the orphan `.`,
    ///   debt F15 sub-stmt recovery).
    /// - `recv_line`/`recv_col`: Fitz 1-based position of the START
    ///   of the receiver, for TypeInfo lookup when present.
    AfterDot {
        recv_name: String,
        recv_line: usize,
        recv_col: usize,
    },
    /// LSPz mini-batch (v0.9.47) — `from <mod> import |` or
    /// `from <mod> import X, |` — the cursor is inside the import
    /// list of a `from`. We list the exportable symbols of the
    /// target module. `mod_path` are the segments of the module
    /// (`["foo"]` or `["sub", "utils"]`).
    FromImportList { mod_path: Vec<String> },
    /// v0.10.12 — `@` or `@<prefix>` — the cursor is typing a
    /// decorator (after the `@`, before the `(` or a newline). We
    /// list the closed set of language decorators with useful
    /// snippets. VSCode filters client-side by `<prefix>`. Covers the
    /// 4 groups:
    ///   - HTTP routing: `@get`/`@post`/`@put`/`@delete`/`@server`/`@header`
    ///   - Middleware/CORS: `@middleware`/`@cors`
    ///   - Auth: `@authenticated`/`@admin`/`@auth_provider`
    ///   - WS + Jobs: `@ws`/`@cron`/`@background`/`@test`
    ///   - ORM: `@table`/`@primary`/`@column`/`@unique`/`@index`/
    ///     `@db_default`/`@hidden`/`@belongs_to`/`@has_one`/`@has_many`/
    ///     `@renamed_from` (v0.10.17)
    AfterAt,
    /// Any other context — we list top-level + builtins + keywords.
    ScopeLevel,
}

/// Main completion endpoint (Phase 9.x.4). Inspects the text to
/// detect whether the cursor is after a `.` (after-dot) or not
/// (scope-level), and returns the appropriate `CompletionItem` list.
///
/// **Scope-level**: enumerates Program top-level (`let`, `fn`, `type`,
/// `import` bindings) + builtins (`print`/`len`/`sleep`/`cors`) +
/// language keywords. NOT scope-aware: we don't enumerate local vars
/// and params as a function of cursor position (MVP debt — requires
/// a checker refactor to expose per-stmt scopes). VSCode filters by
/// prefix client-side; the user can type local vars even when they
/// don't appear in the list.
///
/// **After-dot**: identifies the receiver (a single identifier before
/// the `.`), looks up its type in `TypeInfo` by the start position of
/// the receiver, dispatches by type:
/// - `Nominal(id)` → type fields via `TypeEnv.info(id)`.
/// - `List<T>` → 6 built-in methods.
/// - `Map<K, V>` → 5 built-in methods.
/// - `Str` → 3 methods.
/// - `Any`/`PyAny`/other → empty list.
///
/// Chain `a.b.c.` stays as visible debt — only supports
/// `<ident>.<prefix?>`.
pub fn completion_at_position(
    text: &str,
    program: &Program,
    type_info: &TypeInfo,
    type_env: &TypeEnv,
    line: u32,
    character: u32,
) -> Vec<CompletionItem> {
    completion_at_position_with_uri(text, program, type_info, type_env, line, character, None)
}

/// LSPz mini-batch (v0.9.47) — variant with optional `doc_uri` for
/// resolving the target module file in the `from <mod> import |`
/// context. The bin backend (`fitz-lsp.rs`) uses it to pass the URI
/// of the open document; the rest of the consumers (existing tests,
/// external tools) can use `completion_at_position` directly — with
/// `doc_uri = None`, the `FromImportList` context returns an empty
/// list (without a URI we can't resolve the module file).
pub fn completion_at_position_with_uri(
    text: &str,
    program: &Program,
    type_info: &TypeInfo,
    type_env: &TypeEnv,
    line: u32,
    character: u32,
    doc_uri: Option<&Url>,
) -> Vec<CompletionItem> {
    let ctx = match detect_completion_context(text, line, character) {
        Some(c) => c,
        None => return Vec::new(),
    };
    match ctx {
        CompletionContext::AfterDot {
            recv_name,
            recv_line,
            recv_col,
        } => after_dot_completions(
            program, type_info, type_env, &recv_name, recv_line, recv_col,
        ),
        CompletionContext::FromImportList { mod_path } => match doc_uri {
            Some(uri) => from_import_completions(uri, &mod_path),
            None => Vec::new(),
        },
        CompletionContext::AfterAt => decorator_completions(),
        CompletionContext::ScopeLevel => {
            // LSPy.4 mini-batch — pass the cursor line (1-based) to
            // include local vars/params of the enclosing scope.
            let cursor_line_fitz = (line as usize) + 1;
            scope_level_completions(program, type_env, cursor_line_fitz)
        }
    }
}

/// Walks backwards from the cursor in the text. If it finds
/// `<ident>.<partial_prefix?>`, returns `AfterDot` with the position
/// of the start of the receiver. Otherwise, `ScopeLevel`. Returns
/// `None` if the position is not valid (past the end of the text).
fn detect_completion_context(text: &str, line: u32, character: u32) -> Option<CompletionContext> {
    let offset = position_to_offset(text, line, character)?;
    let bytes = text.as_bytes();
    // Skip the prefix the user already typed (identifier chars before
    // the cursor).
    let mut i = offset;
    while i > 0 && is_ident_continue(bytes[i - 1]) {
        i -= 1;
    }
    // v0.10.12 — If there is a `@` right before the prefix, AfterAt
    // context (typing a decorator name). Covers both `@|` (cursor
    // immediately after `@`, empty prefix) and `@get|` (prefix
    // "get"). VSCode filters client-side by the typed prefix, so we
    // always return the full decorator list.
    //
    // It takes priority over after-dot: the `@` char cannot be part
    // of an ident chain (`a.b.c`), so what comes after `@` is ALWAYS
    // a decorator name.
    if i > 0 && bytes[i - 1] == b'@' {
        return Some(CompletionContext::AfterAt);
    }
    // If there is a `.` right before, after-dot context.
    if i > 0 && bytes[i - 1] == b'.' {
        let dot_pos = i - 1;
        let mut j = dot_pos;
        // v0.9.47 — chain a.b.c.: walks back-to-front capturing
        // `<ident>(.<ident>)*` to support compound receivers.
        // Phase 10 debt QB — extended to support balanced parens
        // inside chains: `User.where(fn(u) => true).` captures the
        // whole chain, skipping `(...)` when they appear in the way.
        // The resulting recv_name does NOT include the parens (we
        // only capture the outermost `<ident>(.<ident>)*` segments);
        // the type lookup via TypeInfo is done by the START position
        // of the first ident, so matching works if TypeInfo has an
        // Expr registered at that position.
        while j > 0 {
            let c = bytes[j - 1];
            if is_ident_continue(c) || c == b'.' {
                j -= 1;
            } else if c == b')' {
                // Balanced paren skip — scan back to the matching `(`.
                let mut depth = 1;
                let mut k = j - 1;
                while k > 0 && depth > 0 {
                    k -= 1;
                    match bytes[k] {
                        b')' => depth += 1,
                        b'(' => depth -= 1,
                        _ => {}
                    }
                }
                if depth != 0 {
                    // Unbalanced — abort the chain walk.
                    break;
                }
                j = k;
            } else {
                break;
            }
        }
        // Validate shape: the receiver must not start with `.` nor
        // contain consecutive `..`. If valid, we return `None` for
        // the chain (falls back to ScopeLevel).
        if j < dot_pos {
            // For recv_name, take the part BEFORE the first `(`
            // (if any), since we want `User.where` not
            // `User.where(...)`. The TypeInfo lookup uses the start
            // position of the chain.
            let raw = std::str::from_utf8(&bytes[j..dot_pos]).unwrap_or("");
            let recv_name = match raw.find('(') {
                Some(p) => raw[..p].trim_end_matches('.').to_string(),
                None => raw.to_string(),
            };
            if recv_name.starts_with('.') || recv_name.ends_with('.') || recv_name.contains("..") {
                // Not a valid chain — fallback to ScopeLevel.
            } else {
                let (recv_line_lsp, recv_col_lsp_utf16) = offset_to_position(text, j);
                // v0.13.2 — `offset_to_position` returns UTF-16 code
                // units (LSP default), but the heuristic lookup
                // against `TypeInfo` indexes by Unicode chars of the
                // lexer. We translate before building the AfterDot.
                // For ASCII recv_name (lexer rule: identifiers are
                // ASCII only) the difference only shows up if the
                // line has SMP chars BEFORE the identifier (rare:
                // comment with emoji + ident, or string + ident,
                // etc.).
                let recv_col_unicode =
                    utf16_to_unicode_char(text, recv_line_lsp, recv_col_lsp_utf16);
                return Some(CompletionContext::AfterDot {
                    recv_name,
                    recv_line: (recv_line_lsp as usize) + 1,
                    recv_col: (recv_col_unicode as usize) + 1,
                });
            }
        }
    }
    // LSPz mini-batch (v0.9.47) — `from <mod> import |` or
    // `from <mod> import X, |`. We walk backwards from the cursor
    // skipping whitespace + identifiers + commas until the first
    // token that doesn't fit. If what precedes is `import` (with
    // whitespace before it) preceded by `from <mod_path>`,
    // FromImportList context with `mod_path` segmented by `.`.
    if let Some(mod_path) = detect_from_import_list_context(text, line, character) {
        return Some(CompletionContext::FromImportList { mod_path });
    }
    Some(CompletionContext::ScopeLevel)
}

/// LSPz mini-batch — detects the `from <mod_path> import ...|`
/// pattern. Walks backwards from the cursor position, skipping the
/// typed prefix + any previous `<ident>,?\s*`, until it finds the
/// `import` keyword and a preceding `from <ident(.<ident>)*>`.
/// Returns `mod_path` segmented by `.` or `None` if the context does
/// not match.
fn detect_from_import_list_context(text: &str, line: u32, character: u32) -> Option<Vec<String>> {
    let offset = position_to_offset(text, line, character)?;
    let bytes = text.as_bytes();
    // Skip typed prefix.
    let mut i = offset;
    while i > 0 && is_ident_continue(bytes[i - 1]) {
        i -= 1;
    }
    // Skip previous items of the list: `<ident>,?\s*`.
    loop {
        // Skip whitespace.
        while i > 0 && matches!(bytes[i - 1], b' ' | b'\t') {
            i -= 1;
        }
        // Skip optional comma.
        if i > 0 && bytes[i - 1] == b',' {
            i -= 1;
            // After the comma there must be whitespace + ident
            // backwards (another list item).
            while i > 0 && matches!(bytes[i - 1], b' ' | b'\t') {
                i -= 1;
            }
            let id_end = i;
            while i > 0 && is_ident_continue(bytes[i - 1]) {
                i -= 1;
            }
            if i == id_end {
                // No ident before the comma — invalid pattern.
                return None;
            }
            continue;
        }
        break;
    }
    // Here we must have `import` + whitespace.
    if i < 6 || &bytes[i - 6..i] != b"import" {
        return None;
    }
    i -= 6;
    // Whitespace + module path: `<ident>(.<ident>)*`.
    while i > 0 && matches!(bytes[i - 1], b' ' | b'\t') {
        i -= 1;
    }
    let mod_end = i;
    // Walk the path back-to-front: ident chars + `.`.
    while i > 0 {
        let c = bytes[i - 1];
        if is_ident_continue(c) || c == b'.' {
            i -= 1;
        } else {
            break;
        }
    }
    let mod_start = i;
    if mod_start == mod_end {
        return None;
    }
    let mod_str = std::str::from_utf8(&bytes[mod_start..mod_end]).ok()?;
    // Validate shape: ident(.ident)* (does not start/end with `.`,
    // no two consecutive dots).
    if mod_str.starts_with('.') || mod_str.ends_with('.') || mod_str.contains("..") {
        return None;
    }
    // Whitespace + `from`.
    while i > 0 && matches!(bytes[i - 1], b' ' | b'\t') {
        i -= 1;
    }
    if i < 4 || &bytes[i - 4..i] != b"from" {
        return None;
    }
    // The `from` must be at the start of the line (or only preceded
    // by whitespace). We don't enforce this strictly; in Fitz `from`
    // only appears as a top-level stmt, so in practice it always
    // matches.
    let mod_path: Vec<String> = mod_str.split('.').map(|s| s.to_string()).collect();
    Some(mod_path)
}

/// Converts an LSP `(line, character)` (0-based) to a byte offset
/// within `text`. Returns `None` if the position is past the end of
/// the text.
///
/// **v0.13.2** — counts **UTF-16 code units** (LSP spec default for
/// `positionEncoding`). The `character` that comes from the VSCode
/// client is the offset in UTF-16 code units from the start of the
/// line, and this function translates it to a UTF-8 byte offset
/// within the text. For chars of the Supplementary Multilingual
/// Plane (emoji, advanced math symbols) `ch.len_utf16() == 2`
/// (surrogate pair); for everything else (ASCII + BMP)
/// `len_utf16() == 1`.
///
/// History: v0.9.51 counted Unicode chars (equivalent to `len_utf16
/// == 1`) and the server announced `positionEncoding: utf-8`. That
/// broke against `vscode-languageclient@9.0.1`, which hard-codes
/// `general.positionEncodings = ['utf-16']` (client.js:1370) and
/// rejects any server encoding other than `utf-16` in
/// `client.js:835`. The 0.13.1 extension handshake failed with
/// "Unsupported position encoding (utf-8)" before being able to
/// speak JSON-RPC, leaving the extension unusable on fresh VSCode.
/// v0.13.2 migrates the counting to UTF-16 code units (compatible
/// with any standard LSP client) and closes the debt entirely.
///
/// Tolerance: if `character` lands in the middle of a surrogate pair
/// (misbehaved client), we use `>=` to return the offset at the end
/// of the invalid char instead of None. VSCode does not generate
/// that case in practice.
fn position_to_offset(text: &str, line: u32, character: u32) -> Option<usize> {
    let mut offset = 0usize;
    let mut current_line = 0u32;
    let mut current_utf16 = 0u32;
    for ch in text.chars() {
        if current_line == line && current_utf16 >= character {
            return Some(offset);
        }
        if ch == '\n' {
            current_line += 1;
            current_utf16 = 0;
        } else {
            current_utf16 += ch.len_utf16() as u32;
        }
        offset += ch.len_utf8();
    }
    if current_line == line && current_utf16 >= character {
        return Some(offset);
    }
    None
}

/// Inverse of `position_to_offset` — used to locate the LSP position
/// of a point in the text given in bytes (typically the start of a
/// receiver to look up in TypeInfo). The returned `character` is in
/// **UTF-16 code units** (LSP spec default), parallel to
/// `position_to_offset`.
fn offset_to_position(text: &str, offset: usize) -> (u32, u32) {
    let mut current_line = 0u32;
    let mut current_utf16 = 0u32;
    let mut current_offset = 0usize;
    for ch in text.chars() {
        if current_offset >= offset {
            break;
        }
        if ch == '\n' {
            current_line += 1;
            current_utf16 = 0;
        } else {
            current_utf16 += ch.len_utf16() as u32;
        }
        current_offset += ch.len_utf8();
    }
    (current_line, current_utf16)
}

/// **v0.13.2** — Converts an LSP `character` from the client (UTF-16
/// code units, spec default) to 1-based Unicode chars of the lexer,
/// given the document `text`. Necessary because `TypeInfo` and
/// `DefinitionInfo` index by Unicode chars (`column += 1` per
/// non-newline char in `lexer.rs::advance`) while the standard LSP
/// client sends positions in UTF-16 code units.
///
/// For ASCII + BMP — all real code (ASCII identifiers per lexer
/// rules, normal strings) — `ch.len_utf16() == 1` and this function
/// is the identity. For chars in the Supplementary Multilingual
/// Plane (emoji, advanced math symbols) where
/// `ch.len_utf16() == 2`, it "collapses" the surrogate pair into 1
/// Unicode char.
///
/// If `char_utf16` lands in the middle of a surrogate pair
/// (misbehaved client — VSCode does not generate that case), returns
/// the char_unicode at the end of the surrogate (the position "after
/// the invalid char"), which is what's most useful for the
/// subsequent heuristic lookup.
///
/// It is `pub` because the bin backend (`src/bin/fitz-lsp.rs`) uses
/// it to translate the client's `position.character` before calling
/// `hover_for_position` / `definition_for_position`, which expect
/// Unicode chars of the lexer.
pub fn utf16_to_unicode_char(text: &str, line: u32, char_utf16: u32) -> u32 {
    let mut current_line = 0u32;
    let mut current_utf16 = 0u32;
    let mut current_unicode = 0u32;
    for ch in text.chars() {
        if current_line == line && current_utf16 >= char_utf16 {
            return current_unicode;
        }
        if ch == '\n' {
            if current_line == line {
                return current_unicode;
            }
            current_line += 1;
            current_utf16 = 0;
            current_unicode = 0;
        } else {
            current_utf16 += ch.len_utf16() as u32;
            current_unicode += 1;
        }
    }
    current_unicode
}

/// Characters valid in the middle of a Fitz identifier: ASCII
/// alphanumeric + underscore. Matches the lexer definition.
fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Generates completions for after-dot: looks up the receiver's
/// type and dispatches by type.
///
/// **Receiver type resolution** with two fallbacks:
/// 1. **Heuristic TypeInfo lookup**: filters entries whose line is
///    the same as the receiver's and whose col is <= recv_col,
///    returns the one with maximum col. Works when the
///    `Expr::Ident(recv_name, span)` remained in the AST (the
///    `let r = foo.<cursor>` case where foo parses fine even with
///    a broken Field).
/// 2. **Walk Program by name**: if TypeInfo did not return a type,
///    we walk top-level `Stmt::Assign` looking for
///    `target == recv_name` and look at the `value` type in
///    TypeInfo. Covers the typical case of the user typing `obj.`
///    at the end of the buffer — the parser abandons the whole
///    stmt because of the orphan `.` (debt F15 sub-stmt recovery),
///    so the Expr::Ident doesn't reach TypeInfo, but the previous
///    `let obj = ...` does have its value typed.
///
/// Types covered: `Nominal` (fields), `List` (6 methods), `Map`
/// (5 methods), `Str` (3 methods). Others return an empty list.
/// v0.10.12 — Completions for the `AfterAt` context (cursor typing
/// a decorator after `@`). Returns the closed list of language
/// decorators grouped by family, with `${N:label}` snippets where
/// `${0}` marks the final cursor position post-completion.
///
/// VSCode filters client-side by the typed prefix, so we always
/// return the full list — the user sees `@ge` → `@get`, `@post`,
/// `@put`, `@delete` (filtered to `@get` by prefix match).
///
/// **Snippets**:
/// - Decorators with a typical arg (`@get("/path")`,
///   `@table("name")`) emit `name("${1:placeholder}")` with tabstop.
/// - Decorators without args (`@hidden`, `@primary`, `@test`,
///   `@authenticated`, `@admin`, `@background`) emit the bare name
///   without parens.
/// - Decorators with multiple optional args (`@server`, `@cors`)
///   emit `name(${1:args})` with a single editable placeholder.
/// - Relation decorators (`@belongs_to`, `@has_one`, `@has_many`)
///   emit `name("${1:Target}", via="${2:fk}")` with two tabstops.
fn decorator_completions() -> Vec<CompletionItem> {
    use tower_lsp::lsp_types::InsertTextFormat;

    // Each tuple: (label, snippet, detail, doc)
    // - label is what appears in the VSCode list.
    // - snippet uses ${N:placeholder} syntax for tabstops.
    // - detail is the short signature.
    // - doc is the description of what it does.
    let entries: &[(&str, &str, &str, &str)] = &[
        // HTTP routing
        (
            "get",
            "get(\"${1:/path}\")",
            "@get(path) — HTTP GET handler",
            "Registers an HTTP GET handler. Path with `{param}` for path params.",
        ),
        (
            "post",
            "post(\"${1:/path}\")",
            "@post(path) — HTTP POST handler",
            "Registers an HTTP POST handler. Body deserialized into the leftover param's type.",
        ),
        (
            "put",
            "put(\"${1:/path}\")",
            "@put(path) — HTTP PUT handler",
            "Registers an HTTP PUT handler. Body deserialized into the leftover param's type.",
        ),
        (
            "delete",
            "delete(\"${1:/path}\")",
            "@delete(path) — HTTP DELETE handler",
            "Registers an HTTP DELETE handler.",
        ),
        (
            "server",
            "server(${1:3000})",
            "@server(port, host?, ws_heartbeat_secs?, ...)",
            "Configures the HTTP listener.\n\n\
             **Positional args**: `port` (Int 1-65535), `host` (Str IP literal, default \"127.0.0.1\").\n\n\
             **Kwargs**: `port=<Int>`, `host=<Str>` (v0.15.13+ — same parameters as the positionals, conflict if both are passed), \
             `docs=<Bool>` (default true), `api_version=<Str>`, \
             `ws_heartbeat_secs=<Int>` (default 30), `shutdown_timeout_secs=<Int>` (default 30), \
             `observability=<Bool>` (default true), `prometheus=<Bool>` (default false — opt-in for the /metrics endpoint).\n\n\
             **Canonical Docker pattern**: `@server(host=\"0.0.0.0\", port=8080, prometheus=true)`. \
             The `127.0.0.1` default does not accept connections from the Docker network.",
        ),
        (
            "header",
            "header(\"${1:Header-Name}\")",
            "@header(name) — handler param bound from a header",
            "The handler param receives the value of the HTTP header. Only Str or Str?.",
        ),
        // Middleware / CORS
        (
            "middleware",
            "middleware(${1:fn_name})",
            "@middleware(fn) — stackable before the route decorator",
            "Chain of middlewares executed in order. `return null` continues, `return <status> {...}` short-circuits.",
        ),
        (
            "cors",
            "cors()",
            "@cors() o @cors({allow_origin: \"...\", ...})",
            "CORS for the route. No args: permissive defaults. With map: override allow_origin/methods/headers/max_age.",
        ),
        // Auth
        (
            "authenticated",
            "authenticated",
            "@authenticated — handler protected by the provider",
            "Validates bearer token via the @auth_provider singleton. The first leftover param receives the authenticated User.",
        ),
        (
            "admin",
            "admin",
            "@admin — protected handler + role == \"admin\"",
            "Equivalent to @authenticated + check `user.role == \"admin\"`. Returns 403 if not admin.",
        ),
        (
            "requires",
            "requires(\"${1:editor}\")",
            "@requires(role) — custom RBAC (Phase 9.w.1.iter2)",
            "Handler protected by a specific role. Stackable: `@requires(\"editor\")` (one role); \
             `@requires(\"editor\") @requires(\"publisher\")` (OR — matches any). \
             Implies auth (runs the provider). Requires `role: Str` on the User type. \
             Returns 403 if the user's role does not match.",
        ),
        (
            "auth_provider",
            "auth_provider",
            "@auth_provider — singleton token resolver",
            "Marks the fn as the auth provider. Receives Map<Str,Str> headers, returns Result<User>.",
        ),
        // WS + Jobs
        (
            "ws",
            "ws(\"${1:/path}\")",
            "@ws(path) — WebSocket endpoint",
            "Async fn with first param typed WsConn<T>. T is the message type marshalled to/from the client.",
        ),
        (
            "cron",
            "cron(\"${1:0 */5 * * * *}\")",
            "@cron(expr) — periodic job",
            "Cron expression (5/6/7 Unix fields). Sync or async. No params, return Null/Result/Future. \
             Optional kwargs (iter2): `tz=\"IANA/Name\"` (default UTC), \
             `retry={max: N, backoff: \"exponential\"|\"linear\"|\"constant\", initial_secs: I, max_secs: M}`, \
             `catch_up=true|false` (default false), \
             `store=db` (persists runs in fitz_cron_jobs/fitz_cron_runs).",
        ),
        (
            "background",
            "background",
            "@background — marks fn as spawnable via spawn(fn(...))",
            "Opt-in marker. Enables the fire-and-forget `spawn(fn(args))` call typed as Future<T>. \
             Optional kwargs: `tz=\"IANA/Name\"`, `retry={...}` (same shape as @cron), and (v0.37.7) \
             `store=db` + `catch_up=true|false` — persist each spawn in fitz_bg_jobs, catch_up marks \
             orphaned rows failed at boot.",
        ),
        (
            "test",
            "test",
            "@test — registers as a unit test (fitz test)",
            "No params. Bodies may use assert/assert_eq/assert_ne/assert_throws builtins.",
        ),
        // Phase 11.11 — server functions (client-WASM ↔ server RPC).
        (
            "rpc",
            "rpc",
            "@rpc — server function callable from a .fitzv client",
            "Bare decorator, must be `async`, must return `Result<T>`. Mounted as `POST /__rpc/<name>` \
             on the server; a `.fitzv` that imports it calls it with `.await?` and the compiler emits a \
             fetch stub. Not combinable with @get/@post/@ws/@cron/@background/@auth_provider.",
        ),
        // Fase 12.1 (v0.12.0) — Health checks K8s.
        (
            "healthz",
            "healthz",
            "@healthz — liveness probe (auto-mount GET /healthz)",
            "Singleton. No params. Return Bool / Result<Null> / Result<Bool> (sync or async). \
             Maps Bool true / Ok / Null → 200; Bool false / Err → 503. With no @healthz declared, \
             the server auto-mounts GET /healthz with a default 200 response.",
        ),
        (
            "readyz",
            "readyz",
            "@readyz — readiness probe (auto-mount GET /readyz)",
            "Singleton. No params. Return Bool / Result<Null> / Result<Bool> (sync or async). \
             During SIGTERM/graceful shutdown, returns 503 immediately (K8s stops routing) without invoking \
             the handler. With no @readyz declared, the server auto-mounts GET /readyz with a default \
             200 response.",
        ),
        // v0.11.0 (Fase 13) — CLI builder.
        (
            "command",
            "command(\"${1:name}\", desc=\"${2:description}\")",
            "@command(name, desc=) — declares fn as a CLI command",
            "The binary produced by `fitz build` parses argv and dispatches. Return type must be Int (exit code). Params without default = positional args; with default = flags. Bool with default false → bool flag.",
        ),
        // Phase 12.7 — observability decorators over user fns.
        (
            "trace",
            "trace(name=\"${1:span_name}\")",
            "@trace(name=) — opens a tracing::span on each call",
            "On user fns (not HTTP/WS — auto-instrumentation from Phase 12.3 covers those). Without name, uses the fn name. Zero overhead if no subscriber is installed. Combinable with @metric.",
        ),
        (
            "metric",
            "metric(name=\"${1:metric_name}\")",
            "@metric(name=) — records histogram + counter per call",
            "On user fns (not HTTP/WS). Emits `<name>_duration_seconds` (histogram) and `<name>_calls_total` (counter) when the scope is dropped. Without name, uses the fn name. Combinable with @trace.",
        ),
        // Phase 12.8 — feature flag decorator over fns.
        (
            "flag",
            "flag(\"${1:flag-name}\")",
            "@flag(\"name\") — gates the fn by feature flag",
            "On HTTP/WS handlers or regular fns. If the flag is off (default), HTTP/WS return 404. Defaults in `fitz.toml [flags]`; runtime override via env var `FITZ_FLAG_<UPPERCASE>`. Combinable with auth + RBAC.",
        ),
        // ORM
        (
            "table",
            "table(\"${1:table_name}\")",
            "@table(\"name\") — type → Postgres table",
            "Enables ORM read/write methods on the type. Requires @primary on some field.",
        ),
        (
            "primary",
            "primary",
            "@primary — field is the PK",
            "On a field. Exactly one per type. Composite PKs not supported in MVP.",
        ),
        (
            "column",
            "column(\"${1:sql_name}\")",
            "@column(sql_name) — override the SQL name of the field",
            "By default the ORM uses the Fitz name of the field. With @column you override the SQL name.",
        ),
        (
            "unique",
            "unique",
            "@unique — UNIQUE constraint (field-level without args, or type-level with positional cols — v0.10.29)",
            "On a field without args: marks the field as UNIQUE in CREATE TABLE. On the `type` (v0.10.29): `@unique(col1, col2, ..., name=\"optional\")` — composite UNIQUE shortcut, ergonomic alias of `@index(unique=true)`. Accepts bare idents or Str with commas.",
        ),
        (
            "check_constraint",
            "check_constraint(\"${1:expr}\")",
            "@check_constraint(\"sql_expr\", name?) — declarative CHECK constraint (v0.10.29)",
            "On the `type` with `@table`: emits `CHECK (<expr>)` in CREATE TABLE. The expr is passed literally to SQL — Postgres validates on INSERT/UPDATE. Stackable. No drift check from the migrator (minor debt) — use `db.exec(\"ALTER TABLE ... DROP/ADD CONSTRAINT\")` for changes.",
        ),
        (
            "index",
            "index",
            "@index(col, ..., unique?, name?, where_?, using?) — index declared on the type (v0.10.27+)",
            "On the `type` with `@table`: declares indexes auto-emitted by `fitz db diff/migrate`. Composite (multi-col), unique (`unique=true`), partial (`where_=<expr>`), name override (`name=\"...\"`), method override (`using=\"gin\"|\"gist\"|\"brin\"|\"hash\"|\"spgist\"` — v0.10.28; btree default).",
        ),
        (
            "db_default",
            "db_default",
            "@db_default — DB assigns the value (skips INSERT)",
            "ORM skips the field on INSERT, Postgres applies its DEFAULT (typical: timestamps, UUIDs gen_random_uuid()). v0.10.16: optionally accepts a Str arg with the SQL expression — `@db_default(\"NOW()\")` — which `fitz db diff` emits automatically in CREATE TABLE / ADD COLUMN.",
        ),
        (
            "hidden",
            "hidden",
            "@hidden — field invisible to HTTP JSON I/O",
            "Skipped by __to_fitz_json (not exposed to the client) and __FromFitzJson (rejects extras). Useful for password_hash, tokens.",
        ),
        (
            "belongs_to",
            "belongs_to(\"${1:Target}\")",
            "@belongs_to(\"Type\", on_delete?, on_update?)",
            "On a FK field. Supports kwargs on_delete=\"cascade\"/\"set_null\"/\"restrict\"/\"no_action\".",
        ),
        (
            "has_one",
            "has_one(\"${1:Target}\", via=\"${2:fk}\")",
            "@has_one(\"Type\", via=\"fk_field\", on_delete?)",
            "Virtual field (does not go to the DB). The target hosts the FK. For `.preload(...)`.",
        ),
        (
            "has_many",
            "has_many(\"${1:Target}\", via=\"${2:fk}\")",
            "@has_many(\"Type\", via=\"fk_field\", on_delete?)",
            "Virtual List<Target>. The target hosts the FK. For `.preload(...)`.",
        ),
        (
            "renamed_from",
            "renamed_from(\"${1:old_name}\")",
            "@renamed_from(\"old_name\") — safe rename (v0.10.17)",
            "Transient decorator so `fitz db diff` emits `ALTER TABLE ... RENAME COLUMN/TABLE` instead of DROP + ADD (preserves data). On a field: column rename. On the `type` (together with `@table`): table rename. Delete it after applying the migration.",
        ),
        // Phase 4 Y-B (v0.20.0) — LiveView components (fitz-liveviews).
        // Registered as language-level decorators here in Fitz core so
        // the checker validates shape; the runtime dispatch lives in the
        // `fitz-liveviews` framework (imported as a lib dep).
        (
            "live_component",
            "live_component(\"${1:component_name}\")",
            "@live_component(\"name\") — declares a type as a LiveView component",
            "On a `type`. Registers the type as a stateful component. Exactly one string-literal arg (the component name used to reference it from templates via `component(\"name\", \"instance-id\")`). Combine with `@render_for(\"name\")` on a render fn and `@on(\"name\", \"event\")` on event handlers. Since Fitz core v0.20.1, the compiler auto-generates the boot `flv_register(...)` call from these decorators — every field must declare a default, and `flv_register` must be imported from `fitz_liveviews`.",
        ),
        (
            "render_for",
            "render_for(\"${1:component_name}\")",
            "@render_for(\"name\") — render fn for a live component",
            "On a fn. Marks it as the renderer of the named component. Signature: `fn(state: T) -> Html` (or `-> Str`), where T is the type with matching `@live_component(\"name\")`. Called by the framework when the component is mounted or its state changes.",
        ),
        (
            "on",
            "on(\"${1:component_name}\", \"${2:event_name}\")",
            "@on(\"component\", \"event\") — event handler for a live component",
            "On a fn. Registers a handler for a client-side event on the named component. Signature: `fn(state: T, payload: Map<Str, Str>) -> T`, returning the new state. The framework routes `data-flv-click`/`data-flv-submit`/... events with matching event name.",
        ),
    ];

    entries
        .iter()
        .map(|(label, snippet, detail, doc)| CompletionItem {
            label: label.to_string(),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some((*detail).to_string()),
            documentation: Some(tower_lsp::lsp_types::Documentation::String(
                (*doc).to_string(),
            )),
            insert_text: Some((*snippet).to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        })
        .collect()
}

fn after_dot_completions(
    program: &Program,
    type_info: &TypeInfo,
    type_env: &TypeEnv,
    recv_name: &str,
    recv_line: usize,
    recv_col: usize,
) -> Vec<CompletionItem> {
    // Phase 9.w.1.b — built-in modules `jwt` and `hash` (native auth).
    // Type-lookup bypass: they type as `Any` in the checker (MVP
    // decision, no dedicated `Type::Module`), so the by-type dispatch
    // does not identify them. We resolve by receiver name here,
    // before touching `type_info`. If the user shadows `jwt` or
    // `hash` with their own `let`, we'd still show these methods —
    // accepted MVP trade-off, refinable post-9.w if it becomes a
    // real issue.
    match recv_name {
        "jwt" => {
            return method_items(&[
                (
                    "encode",
                    "fn(payload: Map, secret: Str, alg: Str?) -> Str".into(),
                ),
                (
                    "decode",
                    "fn(token: Str, secret: Str, alg: Str?) -> Result<Map>".into(),
                ),
            ]);
        }
        "hash" => {
            return method_items(&[
                ("password", "fn(plain: Str) -> Str".into()),
                ("verify", "fn(plain: Str, hashed: Str) -> Bool".into()),
            ]);
        }
        // Phase 9.w.1.iter2.b — built-in module `auth` (token
        // blacklist). Same bypass as jwt/hash/log: types as `Any` in
        // the checker. The 3 builtins are async + return Result, and
        // require a `db` connected in scope (first arg DbConn).
        "auth" => {
            return method_items(&[
                (
                    "blacklist",
                    "fn(db: DbConn, jti: Str, expires_at: Int) -> Future<Result<Null>>".into(),
                ),
                (
                    "is_blacklisted",
                    "fn(db: DbConn, jti: Str) -> Future<Result<Bool>>".into(),
                ),
                (
                    "cleanup_expired",
                    "fn(db: DbConn) -> Future<Result<Int>>".into(),
                ),
            ]);
        }
        // Phase 12.3.a.1 — built-in module `log` (structured
        // logging). Same bypass as jwt/hash/db: types as `Any` in
        // the checker (heterogeneous kwargs are not expressible as
        // `Type::Function`), so the by-type dispatch does not
        // identify it — we resolve by name here. 4 levels parallel
        // to `tracing`.
        "log" => {
            return method_items(&[
                ("info", "fn(msg: Str, **kvs) -> Null".into()),
                ("warn", "fn(msg: Str, **kvs) -> Null".into()),
                ("error", "fn(msg: Str, **kvs) -> Null".into()),
                ("debug", "fn(msg: Str, **kvs) -> Null".into()),
            ]);
        }
        // Phase 10.1 — built-in module `db` for Postgres. Like
        // `jwt`/`hash`, it types as `Any` in the checker (no
        // dedicated `Type::Module` in MVP), so the by-type dispatch
        // does not detect it. We resolve by name here.
        "db" => {
            return method_items(&[("connect", "async fn(url: Str) -> Result<DbConn>".into())]);
        }
        // O1/O3 (v0.32.0) — `sql` module: SQL-expression helpers for the
        // ORM, used inside `.update({...})` values and `.where(...)`
        // predicates. Dedicated namespace so the conventional
        // `let db = db.connect(...)` binding doesn't shadow it.
        "sql" => {
            return method_items(&[
                (
                    "now",
                    "fn() -> SqlExpr  // NOW() — for .update values / .where predicates".into(),
                ),
                (
                    "raw",
                    "fn(sql: Str) -> SqlExpr  // raw SQL fragment, e.g. sql.raw(\"streak + 1\")"
                        .into(),
                ),
            ]);
        }
        // Phase 12.8 — built-in module `flags` (feature flags).
        "flags" => {
            return method_items(&[
                ("is_enabled", "fn(name: Str) -> Bool".into()),
                ("list", "fn() -> List<Str>".into()),
            ]);
        }
        // Mini-fase HTTP client (2026-06-18) — built-in module `http`
        // (outbound HTTP client). Same bypass as jwt/hash/log/auth/db:
        // types as `Any` in the checker (MVP, refinable post-mini-fase),
        // so the by-type dispatch does not identify it. The 6 builtins
        // are async + return Result<HttpClientResponse>. Body for post/
        // put accepts Str | Map<Str,Any> | Bytes (helper dispatches
        // at runtime + sets Content-Type: application/json for Map).
        "http" => {
            return method_items(&[
                (
                    "get",
                    "fn(url: Str) -> Future<Result<HttpClientResponse>>  // GET request"
                        .into(),
                ),
                (
                    "head",
                    "fn(url: Str) -> Future<Result<HttpClientResponse>>  // HEAD (no body, useful for health checks)"
                        .into(),
                ),
                (
                    "post",
                    "fn(url: Str, body: Str|Map|Bytes) -> Future<Result<HttpClientResponse>>  // POST"
                        .into(),
                ),
                (
                    "put",
                    "fn(url: Str, body: Str|Map|Bytes) -> Future<Result<HttpClientResponse>>  // PUT"
                        .into(),
                ),
                (
                    "delete",
                    "fn(url: Str) -> Future<Result<HttpClientResponse>>  // DELETE"
                        .into(),
                ),
                (
                    "request",
                    "fn(opts: Map) -> Future<Result<HttpClientResponse>>  // low-level: method/url/timeout_ms/headers/body/follow_redirects"
                        .into(),
                ),
            ]);
        }
        // Mini-tanda SMTP builtin (2026-06-19) — built-in module `smtp`
        // (outbound mail). Same bypass as http/jwt/hash/log/auth/db:
        // typed as `Any` in the checker (MVP). The single builtin is
        // async + returns Result<SmtpResult>. Config via env vars
        // (SMTP_HOST/PORT/USER/PASSWORD/FROM/TLS) at first send.
        "smtp" => {
            return method_items(&[(
                "send",
                "fn(opts: Map) -> Future<Result<SmtpResult>>  // to/from/subject + body|body_text|body_html"
                    .into(),
            )]);
        }
        _ => {}
    }

    // Phase 10.3 — static ORM methods on `TableName.` when the type
    // has `@table`. `recv_name` is the type identifier (`User.`,
    // `Order.`, etc.); we resolve via `type_env.lookup` +
    // `table_metadata`. If it matches, we return the 3 ORM statics
    // (all/where/insert) BEFORE falling back to the heuristic type
    // lookup (which types `User.` as `Value::Type`, not as
    // `Type::Nominal`).
    if let Some(id) = type_env.lookup(recv_name) {
        if type_env.table_metadata(id).is_some() {
            return method_items(&[
                (
                    "all",
                    format!("async fn(db: DbConn) -> Result<List<{}>>", recv_name),
                ),
                (
                    "where",
                    "fn(predicate: fn(row) -> Bool) -> QueryBuilder".into(),
                ),
                (
                    "first",
                    format!("async fn(db: DbConn) -> Result<{}>", recv_name),
                ),
                ("count", "async fn(db: DbConn) -> Result<Int>".into()),
                (
                    "insert",
                    format!(
                        "async fn(db: DbConn, row: {}) -> Result<{}>",
                        recv_name, recv_name
                    ),
                ),
                (
                    "bulk_insert",
                    format!(
                        "async fn(rows: List<{}>, db: DbConn, batch_size?: Int) -> Result<Int>",
                        recv_name
                    ),
                ),
            ]);
        }
    }

    // Fallback 1: heuristic TypeInfo lookup (max col <= recv_col on
    // the same line).
    let recv_type = type_info
        .iter()
        .filter(|(key, _)| key.0 == recv_line && key.1 <= recv_col)
        .max_by_key(|(key, _)| key.1)
        .map(|(_, ty)| ty.clone());

    // Fallback 2: walk top-level by name, look at the type of the
    // `value` of the let with `target == recv_name`. Covers the case
    // where the parser abandoned the whole stmt because of an orphan
    // `.`.
    let recv_type = recv_type.or_else(|| {
        program.iter().find_map(|stmt| {
            if let Stmt::Assign {
                target: crate::ast::AssignTarget::Ident(name, _),
                value,
                ..
            } = stmt
            {
                if name == recv_name {
                    return type_info.type_at(value.span()).cloned();
                }
            }
            None
        })
    });

    let Some(ty) = recv_type else {
        return Vec::new();
    };
    match &ty {
        Type::Nominal(id) => {
            // Type fields + custom methods (R.3). `info()` panics if
            // the id does not exist — should not happen (the checker
            // validates).
            //
            // Vp mini-batch — private fields (`_field`) do NOT appear
            // in `instance.`: they are only accessible from inside
            // the type body, where the LSP doesn't need to suggest
            // them separately because they are already locals of the
            // fn.
            let info = type_env.info(*id);
            let mut items = Vec::new();
            if let Some(fs) = info.fields.as_ref() {
                for f in fs {
                    if f.name.starts_with('_') {
                        continue;
                    }
                    items.push(CompletionItem {
                        label: f.name.clone(),
                        kind: Some(CompletionItemKind::FIELD),
                        detail: Some(f.type_.display(type_env)),
                        ..CompletionItem::default()
                    });
                }
            }
            // V.5 mini-batch — custom methods (R.3) now appear after
            // fields. Up mini-batch — `NominalMethod` now includes
            // `param_names` parallel to `params`, so the signature
            // shows `fn(x: Int, y: Int) -> Float` instead of
            // `fn(Int, Int) -> Float` (better autocomplete UX).
            //
            // St mini-batch — static methods do NOT appear here:
            // they are invoked as `Type.method()`, not as
            // `instance.method()`. We filter `is_static`.
            //
            // Vm mini-batch — private methods (`_method`) also do
            // not appear in `instance.`: only accessible from inside
            // the type body. Parallel to the fields filter (Vp).
            for m in info
                .methods
                .iter()
                .filter(|m| !m.is_static && !m.name.starts_with('_'))
            {
                // Combine param_names with params to form
                // `x: Int, y: Float`. If for some reason the lengths
                // don't match (defensive), fall back to the old
                // format.
                let params_str = if m.param_names.len() == m.params.len() {
                    m.params
                        .iter()
                        .zip(m.param_names.iter())
                        .map(|(t, n)| format!("{}: {}", n, t.display(type_env)))
                        .collect::<Vec<_>>()
                        .join(", ")
                } else {
                    m.params
                        .iter()
                        .map(|t| t.display(type_env))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let prefix = if m.is_async { "async fn" } else { "fn" };
                let detail = format!("{}({}) -> {}", prefix, params_str, m.ret.display(type_env));
                items.push(CompletionItem {
                    label: m.name.clone(),
                    kind: Some(CompletionItemKind::METHOD),
                    detail: Some(detail),
                    ..CompletionItem::default()
                });
            }
            items
        }
        // Math + Mb9 mini-batch — methods on Int/Float primitives.
        // Int: abs/to_str/to_str_base. Float: abs/to_str/is_nan/
        // is_finite.
        Type::Int => method_items(&[
            ("abs", "fn() -> Int".into()),
            ("to_str", "fn() -> Str".into()),
            (
                "to_str_base",
                "fn(base: Int) -> Str  // base ∈ {2, 8, 10, 16}".into(),
            ),
            // v0.10.32 (Tier D.1) — numeric ORM operators.
            (
                "is_in",
                "fn(values: List<Int>) -> Bool  // (ORM .where) SQL IN".into(),
            ),
            (
                "between",
                "fn(lo: Int, hi: Int) -> Bool  // (ORM .where) SQL BETWEEN".into(),
            ),
            (
                "is_null",
                "fn() -> Bool  // (ORM .where) SQL IS NULL".into(),
            ),
            (
                "is_not_null",
                "fn() -> Bool  // (ORM .where) SQL IS NOT NULL".into(),
            ),
        ]),
        Type::Float => method_items(&[
            ("abs", "fn() -> Float".into()),
            ("to_str", "fn() -> Str".into()),
            ("is_nan", "fn() -> Bool".into()),
            ("is_finite", "fn() -> Bool".into()),
            // v0.10.32 (Tier D.1) — numeric ORM operators.
            (
                "is_in",
                "fn(values: List<Float>) -> Bool  // (ORM .where) SQL IN".into(),
            ),
            (
                "between",
                "fn(lo: Float, hi: Float) -> Bool  // (ORM .where) SQL BETWEEN".into(),
            ),
            (
                "is_null",
                "fn() -> Bool  // (ORM .where) SQL IS NULL".into(),
            ),
            (
                "is_not_null",
                "fn() -> Bool  // (ORM .where) SQL IS NOT NULL".into(),
            ),
        ]),
        Type::List(t) => method_items(&[
            ("push", format!("fn({}) -> Null", t.display(type_env))),
            ("pop", format!("fn() -> Result<{}>", t.display(type_env))),
            (
                "map",
                format!("fn(fn({}) -> U) -> List<U>", t.display(type_env)),
            ),
            (
                "filter",
                format!(
                    "fn(fn({}) -> Bool) -> List<{}>",
                    t.display(type_env),
                    t.display(type_env)
                ),
            ),
            (
                "find",
                format!(
                    "fn(fn({}) -> Bool) -> Result<{}>",
                    t.display(type_env),
                    t.display(type_env)
                ),
            ),
            ("len", "fn() -> Int".into()),
            // S.3 mini-batch: `sort` and `reverse` mutate in-place
            // and return Null. `contains(v)` takes a T and returns
            // Bool.
            ("sort", "fn() -> Null".into()),
            ("reverse", "fn() -> Null".into()),
            ("contains", format!("fn({}) -> Bool", t.display(type_env))),
            // It mini-batch — Python-style iterators.
            (
                "enumerate",
                format!("fn() -> List<(Int, {})>", t.display(type_env)),
            ),
            (
                "zip",
                format!("fn(List<U>) -> List<({}, U)>", t.display(type_env)),
            ),
            (
                "chain",
                format!(
                    "fn(List<{}>) -> List<{}>",
                    t.display(type_env),
                    t.display(type_env)
                ),
            ),
            // Mb mini-batch — flatten + sort_by.
            (
                "flatten",
                "fn() -> List<U>  // requiere List<List<U>>".to_string(),
            ),
            (
                "sort_by",
                format!(
                    "fn(fn({}, {}) -> Int) -> Null",
                    t.display(type_env),
                    t.display(type_env)
                ),
            ),
            // Lx mini-batch — functional predicates.
            (
                "any",
                format!("fn(fn({}) -> Bool) -> Bool", t.display(type_env)),
            ),
            (
                "all",
                format!("fn(fn({}) -> Bool) -> Bool", t.display(type_env)),
            ),
            (
                "count",
                format!("fn(fn({}) -> Bool) -> Int", t.display(type_env)),
            ),
            (
                "find_index",
                format!("fn(fn({}) -> Bool) -> Result<Int>", t.display(type_env)),
            ),
            // Ex2 mini-batch — flat_map + first / last.
            (
                "flat_map",
                format!("fn(fn({}) -> List<U>) -> List<U>", t.display(type_env),),
            ),
            ("first", format!("fn() -> Result<{}>", t.display(type_env))),
            ("last", format!("fn() -> Result<{}>", t.display(type_env))),
            // Mb2 mini-batch — numeric reductions.
            (
                "min",
                format!(
                    "fn() -> Result<{}>  // List<Int> o List<Float>",
                    t.display(type_env)
                ),
            ),
            (
                "max",
                format!(
                    "fn() -> Result<{}>  // List<Int> o List<Float>",
                    t.display(type_env)
                ),
            ),
            (
                "sum",
                format!(
                    "fn() -> {}  // List<Int> o List<Float>",
                    t.display(type_env)
                ),
            ),
            // Mb3 mini-batch — fold + product + to_map.
            (
                "reduce",
                format!(
                    "fn(init: Acc, fn(Acc, {}) -> Acc) -> Acc",
                    t.display(type_env),
                ),
            ),
            (
                "product",
                format!(
                    "fn() -> {}  // List<Int> o List<Float>",
                    t.display(type_env)
                ),
            ),
            (
                "to_map",
                "fn() -> Map<K, V>  // requiere List<(K, V)>".into(),
            ),
            // Mb4 mini-batch — unique + partition.
            (
                "unique",
                format!(
                    "fn() -> List<{}>  // dedup preservando orden",
                    t.display(type_env)
                ),
            ),
            (
                "partition",
                format!(
                    "fn(fn({}) -> Bool) -> (List<{}>, List<{}>)",
                    t.display(type_env),
                    t.display(type_env),
                    t.display(type_env),
                ),
            ),
            // Mb5 mini-batch — group_by + zip_with + max_by/min_by.
            (
                "group_by",
                format!(
                    "fn(fn({}) -> K) -> Map<K, List<{}>>",
                    t.display(type_env),
                    t.display(type_env),
                ),
            ),
            (
                "zip_with",
                format!(
                    "fn(List<U>, fn({}, U) -> V) -> List<V>",
                    t.display(type_env),
                ),
            ),
            (
                "max_by",
                format!(
                    "fn(fn({}) -> Int) -> Result<{}>",
                    t.display(type_env),
                    t.display(type_env),
                ),
            ),
            (
                "min_by",
                format!(
                    "fn(fn({}) -> Int) -> Result<{}>",
                    t.display(type_env),
                    t.display(type_env),
                ),
            ),
            // Mb6 mini-batch — scan + windows.
            (
                "scan",
                format!(
                    "fn(init: Acc, fn(Acc, {}) -> Acc) -> List<Acc>",
                    t.display(type_env),
                ),
            ),
            (
                "windows",
                format!("fn(n: Int) -> List<List<{}>>", t.display(type_env),),
            ),
            // Mb7 mini-batch — take/drop/init/tail/intersperse/cycle.
            (
                "take",
                format!("fn(n: Int) -> List<{}>", t.display(type_env)),
            ),
            (
                "drop",
                format!("fn(n: Int) -> List<{}>", t.display(type_env)),
            ),
            (
                "init",
                format!(
                    "fn() -> List<{}>  // all but the last",
                    t.display(type_env)
                ),
            ),
            (
                "tail",
                format!(
                    "fn() -> List<{}>  // all but the first",
                    t.display(type_env)
                ),
            ),
            (
                "intersperse",
                format!(
                    "fn(sep: {}) -> List<{}>",
                    t.display(type_env),
                    t.display(type_env)
                ),
            ),
            (
                "cycle",
                format!("fn(n: Int) -> List<{}>", t.display(type_env)),
            ),
            // Mb8 mini-batch — starts_with/ends_with/insert_at/remove_at/zip_to_map.
            (
                "starts_with",
                format!("fn(prefix: List<{}>) -> Bool", t.display(type_env)),
            ),
            (
                "ends_with",
                format!("fn(suffix: List<{}>) -> Bool", t.display(type_env)),
            ),
            (
                "insert_at",
                format!(
                    "fn(idx: Int, v: {}) -> List<{}>",
                    t.display(type_env),
                    t.display(type_env)
                ),
            ),
            (
                "remove_at",
                format!("fn(idx: Int) -> List<{}>", t.display(type_env)),
            ),
            (
                "zip_to_map",
                format!("fn(values: List<V>) -> Map<{}, V>", t.display(type_env)),
            ),
            // Mb9 mini-batch — split_at(i): splits the list in two at idx.
            (
                "split_at",
                format!(
                    "fn(idx: Int) -> (List<{}>, List<{}>)",
                    t.display(type_env),
                    t.display(type_env),
                ),
            ),
        ]),
        Type::Map(k, v) => method_items(&[
            (
                "get",
                format!(
                    "fn({}) -> Result<{}>",
                    k.display(type_env),
                    v.display(type_env)
                ),
            ),
            ("has", format!("fn({}) -> Bool", k.display(type_env))),
            ("keys", format!("fn() -> List<{}>", k.display(type_env))),
            ("values", format!("fn() -> List<{}>", v.display(type_env))),
            ("len", "fn() -> Int".into()),
            // Ex mini-batch — functional transformations.
            (
                "filter",
                format!(
                    "fn(fn({}, {}) -> Bool) -> Map<{}, {}>",
                    k.display(type_env),
                    v.display(type_env),
                    k.display(type_env),
                    v.display(type_env),
                ),
            ),
            (
                "map_values",
                format!(
                    "fn(fn({}) -> U) -> Map<{}, U>",
                    v.display(type_env),
                    k.display(type_env),
                ),
            ),
            // Ex2 mini-batch — merge (last-write-wins).
            (
                "merge",
                format!(
                    "fn(Map<{}, {}>) -> Map<{}, {}>",
                    k.display(type_env),
                    v.display(type_env),
                    k.display(type_env),
                    v.display(type_env),
                ),
            ),
            // Up mini-batch — immutable update (last-write-wins
            // over a single key).
            (
                "update",
                format!(
                    "fn({}, fn({}) -> {}) -> Map<{}, {}>",
                    k.display(type_env),
                    v.display(type_env),
                    v.display(type_env),
                    k.display(type_env),
                    v.display(type_env),
                ),
            ),
            // Mb2 mini-batch — keys_sorted: sorted keys.
            (
                "keys_sorted",
                format!(
                    "fn() -> List<{}>  // K comparable (Int/Float/Str/Bool)",
                    k.display(type_env),
                ),
            ),
            // Mb3 mini-batch — entries: (K, V) pairs in insertion order.
            (
                "entries",
                format!(
                    "fn() -> List<({}, {})>",
                    k.display(type_env),
                    v.display(type_env),
                ),
            ),
            // Mb4 mini-batch — invert: swap K ↔ V.
            (
                "invert",
                format!(
                    "fn() -> Map<{}, {}>",
                    v.display(type_env),
                    k.display(type_env),
                ),
            ),
            // Mb6 mini-batch — merge_with: merge with callback.
            (
                "merge_with",
                format!(
                    "fn(Map<{}, {}>, fn({}, {}) -> {}) -> Map<{}, {}>",
                    k.display(type_env),
                    v.display(type_env),
                    v.display(type_env),
                    v.display(type_env),
                    v.display(type_env),
                    k.display(type_env),
                    v.display(type_env),
                ),
            ),
            // Mb7 mini-batch — with: functional update.
            (
                "with",
                format!(
                    "fn({}, {}) -> Map<{}, {}>",
                    k.display(type_env),
                    v.display(type_env),
                    k.display(type_env),
                    v.display(type_env),
                ),
            ),
            // Mb9 mini-batch — has_value: checks if V is present.
            ("has_value", format!("fn({}) -> Bool", v.display(type_env),)),
            // v0.10.32 (Tier D.1) — ORM operators over Map (jsonb).
            // Only valid inside `.where(closure)` of the ORM; the
            // evaluator intercepts them to emit Postgres jsonb
            // operators (`?`, `?&`, `?|`, `@>`, `#>`, `#>>`).
            (
                "has_key",
                "fn(k: Str) -> Bool  // (ORM .where) jsonb ? — \"k\" existe en el top-level".into(),
            ),
            (
                "has_all_keys",
                "fn(keys: List<Str>) -> Bool  // (ORM .where) jsonb ?& — todas existen".into(),
            ),
            (
                "has_any_keys",
                "fn(keys: List<Str>) -> Bool  // (ORM .where) jsonb ?| — alguna existe".into(),
            ),
            (
                "contains_json",
                "fn(patch: Map<Str, Any>) -> Bool  // (ORM .where) jsonb @> — superset".into(),
            ),
            (
                "has_path",
                "fn(path: List<Str>) -> Bool  // (ORM .where) jsonb #> — path nested existe".into(),
            ),
            (
                "path_text",
                "fn(path: List<Str>) -> Str  // (ORM .where) jsonb #>> path → text".into(),
            ),
            (
                "path_int",
                "fn(path: List<Str>) -> Int  // (ORM .where) jsonb #>> + cast bigint".into(),
            ),
            (
                "path_float",
                "fn(path: List<Str>) -> Float  // (ORM .where) jsonb #>> + cast float8".into(),
            ),
            (
                "path_bool",
                "fn(path: List<Str>) -> Bool  // (ORM .where) jsonb #>> + cast boolean".into(),
            ),
        ]),
        Type::Str => method_items(&[
            ("upper", "fn() -> Str".into()),
            ("lower", "fn() -> Str".into()),
            ("len", "fn() -> Int".into()),
            ("to_int", "fn() -> Result<Int>".into()),
            // v0.10.32 (Tier D.1) — ORM operators over Str. They only
            // take effect inside `.where(closure)` of the ORM — the
            // evaluator intercepts them and translates to SQL.
            // Outside the ORM, calling them on a Str raises a runtime
            // error. We document with `(ORM .where)` in the detail so
            // the user distinguishes them from regular Str methods.
            (
                "is_in",
                "fn(values: List<Str>) -> Bool  // (ORM .where) SQL IN".into(),
            ),
            (
                "like",
                "fn(pattern: Str) -> Bool  // (ORM .where) SQL LIKE — case-sensitive".into(),
            ),
            (
                "ilike",
                "fn(pattern: Str) -> Bool  // (ORM .where) SQL ILIKE — case-insensitive".into(),
            ),
            (
                "matches",
                "fn(query: Str) -> Bool  // (ORM .where) SQL @@ to_tsquery — full-text".into(),
            ),
            (
                "plainto_matches",
                "fn(query: Str) -> Bool  // (ORM .where) SQL @@ plainto_tsquery — plain text".into(),
            ),
            (
                "is_null",
                "fn() -> Bool  // (ORM .where) SQL IS NULL".into(),
            ),
            (
                "is_not_null",
                "fn() -> Bool  // (ORM .where) SQL IS NOT NULL".into(),
            ),
            (
                "between",
                "fn(lo: Str, hi: Str) -> Bool  // (ORM .where) SQL BETWEEN".into(),
            ),
            // S.1 / S.2 mini-batch: small Str methods. `contains`
            // and `starts_with`/`ends_with` take a `Str` and return
            // Bool. `split` returns List<Str>. `trim` takes no args.
            // `replace` takes two Strs. `repeat` an Int.
            ("contains", "fn(s: Str) -> Bool".into()),
            ("starts_with", "fn(s: Str) -> Bool".into()),
            ("ends_with", "fn(s: Str) -> Bool".into()),
            ("split", "fn(sep: Str) -> List<Str>".into()),
            ("trim", "fn() -> Str".into()),
            ("trim_start", "fn() -> Str".into()),
            ("trim_end", "fn() -> Str".into()),
            ("replace", "fn(old: Str, new: Str) -> Str".into()),
            ("repeat", "fn(n: Int) -> Str".into()),
            // Ex mini-batch — search.
            ("find", "fn(sub: Str) -> Result<Int>".into()),
            ("index_of", "fn(sub: Str) -> Result<Int>".into()),
            ("last_index_of", "fn(sub: Str) -> Result<Int>".into()),
            // Mb2 mini-batch — padding.
            ("pad_start", "fn(width: Int, ch: Str) -> Str".into()),
            ("pad_end", "fn(width: Int, ch: Str) -> Str".into()),
            // Mb3 mini-batch — chars: List<Str> with each char.
            ("chars", "fn() -> List<Str>".into()),
            // Mb4 mini-batch — split_at: splits at char idx → (Str, Str).
            ("split_at", "fn(idx: Int) -> (Str, Str)".into()),
            // Mb5 mini-batch — lines + is_empty.
            ("lines", "fn() -> List<Str>".into()),
            ("is_empty", "fn() -> Bool".into()),
            // Mb7 mini-batch — repeat_with: repeat with separator.
            ("repeat_with", "fn(n: Int, sep: Str) -> Str".into()),
            // Mb8 mini-batch — left/right/center.
            ("left", "fn(n: Int) -> Str".into()),
            ("right", "fn(n: Int) -> Str".into()),
            ("center", "fn(width: Int, ch: Str) -> Str".into()),
            // Mb9 mini-batch — swap_case/title/is_alpha/is_digit/is_numeric.
            ("swap_case", "fn() -> Str".into()),
            ("title", "fn() -> Str".into()),
            ("is_alpha", "fn() -> Bool".into()),
            ("is_digit", "fn() -> Bool".into()),
            ("is_numeric", "fn() -> Bool".into()),
        ]),
        // T mini-batch (tuples): after `t.` we suggest the field
        // indices as numeric labels (`0`, `1`, ...) with the element
        // type as detail. rust-analyzer style. VSCode shows the
        // labels in the list; the user types the number to insert it.
        Type::Tuple(items) => items
            .iter()
            .enumerate()
            .map(|(i, ty)| CompletionItem {
                label: i.to_string(),
                kind: Some(CompletionItemKind::FIELD),
                detail: Some(ty.display(type_env)),
                ..CompletionItem::default()
            })
            .collect(),
        // Bytes mini-batch — methods of the `Bytes` primitive.
        Type::Bytes => method_items(&[
            ("len", "fn() -> Int".into()),
            ("is_empty", "fn() -> Bool".into()),
            ("to_str", "fn() -> Result<Str>".into()),
        ]),
        // Ir mini-batch — Range exposes enumerate/zip/chain/len.
        // It's the subset that makes sense for a numeric iterable;
        // anything else requires materializing to `List<Int>` first.
        Type::Range => method_items(&[
            ("enumerate", "fn() -> List<(Int, Int)>".into()),
            ("zip", "fn(List<U>) -> List<(Int, U)>".into()),
            ("chain", "fn(List<Int>) -> List<Int>".into()),
            ("len", "fn() -> Int".into()),
            // Rg mini-batch — step_by(n) materializes with step.
            ("step_by", "fn(n: Int) -> List<Int>".into()),
        ]),
        // F13.D — universal methods over `Type::Any` for dynamic
        // type-check on heterogeneous values.
        Type::Any => method_items(&[
            ("as_int", "fn() -> Result<Int>".into()),
            ("as_float", "fn() -> Result<Float>".into()),
            ("as_str", "fn() -> Result<Str>".into()),
            ("as_bool", "fn() -> Result<Bool>".into()),
            ("as_bytes", "fn() -> Result<Bytes>".into()),
            ("type_name", "fn() -> Str".into()),
        ]),
        // 9.w.2 — Typed WebSockets. `WsConn<T>` exposes 4 methods:
        // recv/send/broadcast (parameterized over recv/send) + close.
        //
        // 9.w.2-binary-frames: if the type = Bytes, recv/send/
        // broadcast operate with raw `Message::Binary` frames (not
        // JSON-marshalled); the detail clarifies it so the dev does
        // not get confused.
        //
        // 9.w.2-wsconn-bidir (v0.9.38): `recv` and `send` may be
        // different types for `WsConn<In, Out>`. The detail takes
        // each separately.
        Type::WsConn { recv, send } => {
            let recv_is_bytes = matches!(recv.as_ref(), Type::Bytes);
            let send_is_bytes = matches!(send.as_ref(), Type::Bytes);
            let recv_disp = recv.display(type_env);
            let send_disp = send.display(type_env);
            let recv_note = if recv_is_bytes {
                "  // expects Message::Binary from the client"
            } else {
                "  // JSON-marshalled text frame from the client"
            };
            let send_note = if send_is_bytes {
                "  // emits raw Message::Binary"
            } else {
                ""
            };
            let bcast_note = if send_is_bytes {
                "  // binary broadcast to ALL clients of the endpoint"
            } else {
                "  // to ALL clients of the endpoint"
            };
            method_items(&[
                (
                    "recv",
                    format!("fn() -> Result<{}>{}", recv_disp, recv_note),
                ),
                (
                    "send",
                    format!("fn(msg: {}) -> Result<Null>{}", send_disp, send_note),
                ),
                (
                    "broadcast",
                    format!("fn(msg: {}) -> Result<Null>{}", send_disp, bcast_note,),
                ),
                ("close", "fn() -> Result<Null>".into()),
            ])
        }
        // Phase 10.1 — `DbConn` (native Postgres driver). The query
        // and exec methods are async; close is idempotent. Phase 10.7
        // (v0.10.14) adds `transaction(fn(tx) -> Result<T>)` with
        // auto-commit/rollback.
        Type::DbConn => method_items(&[
            (
                "query",
                "async fn(sql: Str, args: List<Any>) -> Result<List<DbRow>>".into(),
            ),
            (
                "exec",
                "async fn(sql: Str, args: List<Any>) -> Result<Int>  // rows affected".into(),
            ),
            ("close", "async fn() -> Result<Null>".into()),
            ("is_closed", "async fn() -> Bool".into()),
            (
                "transaction",
                "async fn(fn(tx: DbConn) -> Result<T>) -> Result<T>  // BEGIN/COMMIT/ROLLBACK auto"
                    .into(),
            ),
        ]),
        // v0.10.22 — `DbRow` (raw row from `db.query`). Typed
        // extraction methods that return `Result<T>` with a clear
        // error if the col doesn't exist, is NULL, or the PG type
        // doesn't match.
        Type::DbRow => method_items(&[
            ("get_int", "fn(col: Str) -> Result<Int>".into()),
            ("get_str", "fn(col: Str) -> Result<Str>".into()),
            ("get_float", "fn(col: Str) -> Result<Float>".into()),
            ("get_bool", "fn(col: Str) -> Result<Bool>".into()),
            ("len", "fn() -> Int  // number of columns in the row".into()),
        ]),
        // v0.10.24 — `Date` instance methods. Extraction + conversion
        // + custom format with chrono specifiers (%Y, %m, %d, %A,
        // etc.). v0.10.30 Tier B — arithmetic (add_/subtract_) +
        // diff (signed Int).
        Type::Date => method_items(&[
            ("year", "fn() -> Int".into()),
            ("month", "fn() -> Int  // 1..12".into()),
            ("day", "fn() -> Int  // 1..31".into()),
            ("weekday", "fn() -> Int  // ISO 8601: 1=Mon..7=Sun".into()),
            ("to_str", "fn() -> Str  // ISO 8601 YYYY-MM-DD".into()),
            ("to_datetime", "fn() -> DateTime  // 00:00:00 UTC".into()),
            (
                "format",
                "fn(fmt: Str) -> Str  // chrono format (%Y/%m/%d/%A/...)".into(),
            ),
            (
                "add_days",
                "fn(n: Int) -> Date  // v0.10.30 — n signed; panic si overflow".into(),
            ),
            (
                "add_months",
                "fn(n: Int) -> Date  // v0.10.30 — calendar-aware, clamps day".into(),
            ),
            (
                "add_years",
                "fn(n: Int) -> Date  // v0.10.30 — = add_months(n*12)".into(),
            ),
            (
                "subtract_days",
                "fn(n: Int) -> Date  // v0.10.30 — = add_days(-n)".into(),
            ),
            (
                "subtract_months",
                "fn(n: Int) -> Date  // v0.10.30 — = add_months(-n)".into(),
            ),
            (
                "subtract_years",
                "fn(n: Int) -> Date  // v0.10.30 — = add_years(-n)".into(),
            ),
            (
                "diff_days",
                "fn(other: Date) -> Int  // v0.10.30 — signed days; self - other".into(),
            ),
            // v0.10.32 (Tier D.1) — temporal ORM operators.
            (
                "is_in",
                "fn(values: List<Date>) -> Bool  // (ORM .where) SQL IN".into(),
            ),
            (
                "between",
                "fn(lo: Date, hi: Date) -> Bool  // (ORM .where) SQL BETWEEN".into(),
            ),
            (
                "is_null",
                "fn() -> Bool  // (ORM .where) SQL IS NULL".into(),
            ),
            (
                "is_not_null",
                "fn() -> Bool  // (ORM .where) SQL IS NOT NULL".into(),
            ),
        ]),
        // v0.10.24 — `DateTime` instance methods. Same set as Date +
        // hour/minute/second/timestamp + `.date()` extraction.
        // v0.10.30 Tier B — sub-second + calendar arithmetic + diff
        // + timezone display (to_local / in_tz IANA).
        Type::DateTime => method_items(&[
            ("year", "fn() -> Int".into()),
            ("month", "fn() -> Int  // 1..12".into()),
            ("day", "fn() -> Int  // 1..31".into()),
            ("hour", "fn() -> Int  // 0..23".into()),
            ("minute", "fn() -> Int  // 0..59".into()),
            ("second", "fn() -> Int  // 0..59".into()),
            ("timestamp", "fn() -> Int  // Unix epoch seconds".into()),
            ("to_str", "fn() -> Str  // ISO 8601 with Z (UTC)".into()),
            ("date", "fn() -> Date  // extracts the date part".into()),
            ("format", "fn(fmt: Str) -> Str  // chrono format".into()),
            (
                "add_seconds",
                "fn(n: Int) -> DateTime  // v0.10.30 — Duration::seconds(n)".into(),
            ),
            (
                "add_minutes",
                "fn(n: Int) -> DateTime  // v0.10.30".into(),
            ),
            (
                "add_hours",
                "fn(n: Int) -> DateTime  // v0.10.30".into(),
            ),
            (
                "add_days",
                "fn(n: Int) -> DateTime  // v0.10.30 — Duration::days(n)".into(),
            ),
            (
                "add_months",
                "fn(n: Int) -> DateTime  // v0.10.30 — calendar-aware".into(),
            ),
            (
                "add_years",
                "fn(n: Int) -> DateTime  // v0.10.30 — = add_months(n*12)".into(),
            ),
            (
                "subtract_seconds",
                "fn(n: Int) -> DateTime  // v0.10.30".into(),
            ),
            (
                "subtract_minutes",
                "fn(n: Int) -> DateTime  // v0.10.30".into(),
            ),
            (
                "subtract_hours",
                "fn(n: Int) -> DateTime  // v0.10.30".into(),
            ),
            (
                "subtract_days",
                "fn(n: Int) -> DateTime  // v0.10.30".into(),
            ),
            (
                "subtract_months",
                "fn(n: Int) -> DateTime  // v0.10.30".into(),
            ),
            (
                "subtract_years",
                "fn(n: Int) -> DateTime  // v0.10.30".into(),
            ),
            (
                "diff_seconds",
                "fn(other: DateTime) -> Int  // v0.10.30 — signed; self - other".into(),
            ),
            (
                "diff_minutes",
                "fn(other: DateTime) -> Int  // v0.10.30 — trunc toward 0".into(),
            ),
            (
                "diff_hours",
                "fn(other: DateTime) -> Int  // v0.10.30 — trunc toward 0".into(),
            ),
            (
                "diff_days",
                "fn(other: DateTime) -> Int  // v0.10.30 — trunc toward 0".into(),
            ),
            (
                "to_local",
                "fn() -> Str  // v0.10.30 — ISO 8601 + offset in system TZ".into(),
            ),
            (
                "in_tz",
                "fn(iana: Str) -> Result<Str>  // v0.10.30 — IANA tz name (e.g. `America/Argentina/Buenos_Aires`)".into(),
            ),
            // v0.10.32 (Tier D.1) — temporal ORM operators.
            (
                "is_in",
                "fn(values: List<DateTime>) -> Bool  // (ORM .where) SQL IN".into(),
            ),
            (
                "between",
                "fn(lo: DateTime, hi: DateTime) -> Bool  // (ORM .where) SQL BETWEEN".into(),
            ),
            (
                "is_null",
                "fn() -> Bool  // (ORM .where) SQL IS NULL".into(),
            ),
            (
                "is_not_null",
                "fn() -> Bool  // (ORM .where) SQL IS NOT NULL".into(),
            ),
        ]),
        // v0.10.24 — `Uuid` instance methods. Bounded MVP.
        Type::Uuid => method_items(&[
            (
                "to_str",
                "fn() -> Str  // canonical xxx-xxx-xxx-xxx-xxx".into(),
            ),
            ("is_nil", "fn() -> Bool".into()),
        ]),
        // Phase 10.3+ — ORM `QueryBuilder<Row>`. Chain methods
        // preserve the QB; terminals return Result<...>.
        Type::QueryBuilder(row) => {
            let row_disp = row.display(type_env);
            method_items(&[
                (
                    "where",
                    format!(
                        "fn(closure: fn({}) -> Bool) -> QueryBuilder<{}>",
                        row_disp, row_disp
                    ),
                ),
                (
                    "order_by",
                    format!(
                        "fn(closure: fn({}) -> Any) -> QueryBuilder<{}>",
                        row_disp, row_disp
                    ),
                ),
                ("limit", format!("fn(n: Int) -> QueryBuilder<{}>", row_disp)),
                (
                    "offset",
                    format!("fn(n: Int) -> QueryBuilder<{}>", row_disp),
                ),
                (
                    "group_by",
                    format!(
                        "fn(closure: fn({}) -> Any) -> QueryBuilder<{}>",
                        row_disp, row_disp
                    ),
                ),
                (
                    "all",
                    format!("async fn(db: DbConn) -> Result<List<{}>>", row_disp),
                ),
                (
                    "first",
                    format!("async fn(db: DbConn) -> Result<{}>", row_disp),
                ),
                ("count", "async fn(db: DbConn) -> Result<Int>".into()),
                (
                    "sum",
                    format!(
                        "async fn(closure: fn({}) -> Float, db: DbConn) -> Result<Float>",
                        row_disp
                    ),
                ),
                (
                    "avg",
                    format!(
                        "async fn(closure: fn({}) -> Float, db: DbConn) -> Result<Float>",
                        row_disp
                    ),
                ),
                (
                    "min",
                    format!(
                        "async fn(closure: fn({}) -> Float, db: DbConn) -> Result<Float>",
                        row_disp
                    ),
                ),
                (
                    "max",
                    format!(
                        "async fn(closure: fn({}) -> Float, db: DbConn) -> Result<Float>",
                        row_disp
                    ),
                ),
                (
                    "update",
                    "async fn(db: DbConn, changes: Map<Str, Any>) -> Result<Int>  // rows affected"
                        .into(),
                ),
                (
                    "delete",
                    "async fn(db: DbConn) -> Result<Int>  // rows affected".into(),
                ),
            ])
        }
        // PyAny and the rest: no info to suggest.
        _ => Vec::new(),
    }
}

/// Fp — compact render of a default-param expression for display in
/// the CompletionItem detail. Covers primitive literals
/// (Int/Float/Str/Bool/Null), empty lists/maps, and idents. For
/// anything more complex (BinOp, FnExpr, struct lits) we emit `...`
/// as a placeholder — the user opens the fn to see the real detail.
fn render_default_expr(e: &crate::ast::Expr) -> String {
    use crate::ast::Expr;
    match e {
        Expr::Int(n, _) => n.to_string(),
        Expr::Float(f, _) => f.to_string(),
        Expr::Str(s, _) => format!("\"{}\"", s),
        Expr::Bool(b, _) => b.to_string(),
        Expr::Null(_) => "null".into(),
        Expr::Ident(n, _) => n.clone(),
        Expr::List(items, _) if items.is_empty() => "[]".into(),
        Expr::Map(items, _) if items.is_empty() => "{}".into(),
        Expr::UnaryOp { op, operand, .. } => {
            use crate::ast::UnaryOpKind;
            let pre = match op {
                UnaryOpKind::Neg => "-",
                UnaryOpKind::Not => "not ",
                UnaryOpKind::BitNot => "~",
            };
            format!("{}{}", pre, render_default_expr(operand))
        }
        _ => "...".into(),
    }
}

/// LSPy.4 mini-batch — walks stmts looking for scopes that contain
/// `cursor_line` and adds their bindings as CompletionItems.
///
/// Strategy: recursive walk. For each body-bearing stmt whose span
/// is `<= cursor_line`, we assume the cursor may be inside (with or
/// without slop for the closing `}`). We always recurse and let the
/// `cursor_line >= stmt.line` filter control. This is conservative:
/// sometimes it includes bindings of scopes that already closed
/// (acceptable false-positive — completion noise but still useful).
fn collect_local_bindings_at(stmts: &[Stmt], cursor_line: usize, items: &mut Vec<CompletionItem>) {
    for stmt in stmts {
        let start = stmt.span().line;
        // Minimal filter: the stmt cannot be after the cursor.
        if start > cursor_line {
            continue;
        }
        match stmt {
            Stmt::FnDef { params, body, .. } => {
                // Fn params are visible across the body. Cursor at
                // or after the `fn` ⇒ we add them.
                for p in params {
                    let detail = p.type_.as_ref().map(|t| t.display_name());
                    items.push(CompletionItem {
                        label: p.name.clone(),
                        kind: Some(CompletionItemKind::VARIABLE),
                        detail,
                        ..CompletionItem::default()
                    });
                }
                collect_local_bindings_at(body, cursor_line, items);
                collect_let_bindings_before(body, cursor_line, items);
            }
            Stmt::While { body, .. } | Stmt::Loop { body, .. } => {
                collect_local_bindings_at(body, cursor_line, items);
                collect_let_bindings_before(body, cursor_line, items);
            }
            Stmt::For { var, body, .. } => {
                // The for's var is local to the body. We add the
                // idents from the pattern (Ident, Wildcard, Tuple).
                use crate::ast::Pattern;
                let add_pat = |pat: &Pattern, out: &mut Vec<CompletionItem>| {
                    if let Pattern::Ident(name, _) = pat {
                        out.push(CompletionItem {
                            label: name.clone(),
                            kind: Some(CompletionItemKind::VARIABLE),
                            ..CompletionItem::default()
                        });
                    } else if let Pattern::Tuple(subs) = pat {
                        for sub in subs {
                            if let Pattern::Ident(name, _) = sub {
                                out.push(CompletionItem {
                                    label: name.clone(),
                                    kind: Some(CompletionItemKind::VARIABLE),
                                    ..CompletionItem::default()
                                });
                            }
                        }
                    }
                };
                add_pat(var, items);
                collect_local_bindings_at(body, cursor_line, items);
                collect_let_bindings_before(body, cursor_line, items);
            }
            _ => {}
        }
    }
}

/// LSPy.4 mini-batch — adds `let` bindings declared BEFORE the
/// cursor line in the same block. "Before" in the strict sense
/// (`let x = ...` on line 5 is visible from line 5 onwards). For
/// nested blocks (if/match/loop inside the body) we don't recurse —
/// those are handled by `collect_local_bindings_at`.
fn collect_let_bindings_before(
    block: &[Stmt],
    cursor_line: usize,
    items: &mut Vec<CompletionItem>,
) {
    for stmt in block {
        let line = stmt.span().line;
        if line > cursor_line {
            break;
        }
        if let Stmt::Assign {
            target: crate::ast::AssignTarget::Ident(name, _),
            ..
        } = stmt
        {
            items.push(CompletionItem {
                label: name.clone(),
                kind: Some(CompletionItemKind::VARIABLE),
                ..CompletionItem::default()
            });
        }
    }
}

/// Builds a list of Method `CompletionItem`s from a slice of
/// `(name, signature)`.
fn method_items(items: &[(&str, String)]) -> Vec<CompletionItem> {
    items
        .iter()
        .map(|(name, detail)| CompletionItem {
            label: (*name).to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some(detail.clone()),
            ..CompletionItem::default()
        })
        .collect()
}

/// Generates completions for scope-level: walks Program top-level +
/// builtins + keywords. NOT scope-aware (see doc on
/// `completion_at_position`).
fn scope_level_completions(
    program: &Program,
    type_env: &TypeEnv,
    cursor_line: usize,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // LSPy.4 — scope-aware: if the cursor falls inside the body of
    // some nested stmt (FnDef, While, Loop, For, If, Match), we add
    // its bindings to the visible scope. We walk top-down and
    // recurse only into blocks that contain the cursor.
    collect_local_bindings_at(program, cursor_line, &mut items);

    // Program top-level: let/fn/type/import.
    for stmt in program {
        match stmt {
            Stmt::Assign {
                target: crate::ast::AssignTarget::Ident(name, _),
                ..
            } => {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    ..CompletionItem::default()
                });
            }
            Stmt::FnDef {
                name,
                params,
                return_type,
                is_async,
                ..
            } => {
                // Fp — fn signature with types + defaults (when
                // present). Fp.2 — varargs prefixed with `...`.
                let params_str = params
                    .iter()
                    .map(|p| {
                        let ty = p
                            .type_
                            .as_ref()
                            .map(|t| t.display_name())
                            .unwrap_or_else(|| "Any".into());
                        let prefix = if p.varargs { "..." } else { "" };
                        let base = format!("{}{}: {}", prefix, p.name, ty);
                        if let Some(default) = &p.default {
                            format!("{} = {}", base, render_default_expr(default))
                        } else {
                            base
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let ret_str = return_type
                    .as_ref()
                    .map(|t| t.display_name())
                    .unwrap_or_else(|| "Any".into());
                let prefix = if *is_async { "async fn" } else { "fn" };
                let detail = format!("{}({}) -> {}", prefix, params_str, ret_str);
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: Some(detail),
                    ..CompletionItem::default()
                });
            }
            Stmt::TypeDef { name, .. } => {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::CLASS),
                    ..CompletionItem::default()
                });
            }
            Stmt::Import { path, alias, .. } => {
                let label = alias.clone().or_else(|| path.last().cloned());
                if let Some(name) = label {
                    items.push(CompletionItem {
                        label: name,
                        kind: Some(CompletionItemKind::MODULE),
                        ..CompletionItem::default()
                    });
                }
            }
            Stmt::FromImport { names, .. } => {
                for (n, alias) in names {
                    let label = alias.clone().unwrap_or_else(|| n.clone());
                    items.push(CompletionItem {
                        label,
                        kind: Some(CompletionItemKind::VARIABLE),
                        ..CompletionItem::default()
                    });
                }
            }
            _ => {}
        }
    }

    // Language builtins (mirrors `register_builtins` of the checker).
    for (name, detail) in [
        ("print", "fn(args...)"),
        ("len", "fn(x) -> Int"),
        (
            "to_json",
            "fn(x) -> Str  // serialize any value to a JSON string",
        ),
        ("sleep", "fn(Int) -> Future<Null>"),
        // Phase 9.w.3 — `spawn(fn_call)` fire-and-forget. The call
        // target must be marked with `@background`. Returns an
        // awaitable `Future<T>`; ignoring the Future leaves the task
        // running detached.
        ("spawn", "fn(fn_call) -> Future<T>  // requires @background"),
        ("cors", "fn(config: Map?) -> CorsConfig"),
        ("bytes", "fn(s: Str) -> Bytes"),
        ("assert", "fn(cond: Bool, msg: Str?) -> Null"),
        ("assert_eq", "fn(a, b) -> Null"),
        ("assert_ne", "fn(a, b) -> Null"),
        ("assert_throws", "fn(callback: fn() -> Any) -> Null"),
        // Bits-extras mini-batch — ops on Int as global builtins.
        (
            "popcount",
            "fn(n: Int) -> Int  // population count (bits=1)",
        ),
        (
            "leading_zeros",
            "fn(n: Int) -> Int  // count leading 0-bits",
        ),
        (
            "trailing_zeros",
            "fn(n: Int) -> Int  // count trailing 0-bits",
        ),
        (
            "rotate_left",
            "fn(n: Int, k: Int) -> Int  // rotate bits left",
        ),
        (
            "rotate_right",
            "fn(n: Int, k: Int) -> Int  // rotate bits right",
        ),
        // Math mini-batch — polymorphic numeric builtins.
        ("abs", "fn(n: Int|Float) -> Int|Float"),
        ("min", "fn(a, b) -> Int|Float  // same type"),
        ("max", "fn(a, b) -> Int|Float  // same type"),
        ("pow", "fn(base, exp) -> Float"),
        ("sqrt", "fn(x: Int|Float) -> Float"),
        ("ceil", "fn(x: Int|Float) -> Int"),
        ("floor", "fn(x: Int|Float) -> Int"),
        ("round", "fn(x: Int|Float) -> Int"),
        ("clamp", "fn(x, lo, hi) -> Int|Float  // same type"),
        // env builtin mini-phase (2026-05-22, Step 3 post-boilerplates).
        (
            "env",
            "fn(key: Str) -> Result<Str>  // env var, Err if missing",
        ),
        (
            "env_or",
            "fn(key: Str, default: Str) -> Str  // env var with default",
        ),
        (
            "load_env",
            "fn(path: Str) -> Result<Null>  // parse KEY=VALUE file",
        ),
        // Phase 12.2.a — secret/config builtins.
        (
            "secret",
            "fn(key: Str) -> Result<Secret<Str>>  // env var | /run/secrets/<key>",
        ),
        (
            "config",
            "fn(key: Str, default: T) -> T  // env var with type coercion + default",
        ),
        // 10.8.7 (v0.10.8) — cross-handler broadcast to WS clients.
        (
            "ws_broadcast",
            "fn(endpoint: Str, msg) -> Null  // broadcast JSON to WS clients",
        ),
    ] {
        items.push(CompletionItem {
            label: name.into(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(detail.into()),
            ..CompletionItem::default()
        });
    }

    // Phase 9.w.1.b — native auth modules `jwt` and `hash`, always
    // available as Value::Module in the evaluator's global env and
    // as `Any` bindings in the checker. Listed as MODULE so VSCode
    // shows them with the appropriate icon and distinguishes them
    // from fns and vars. Phase 10.1 — `db` native module for
    // Postgres, `db.connect(url) -> DbConn`, methods on QueryBuilder
    // and ORM Type when there is `@table`.
    for (name, detail) in [
        ("jwt", "module: encode, decode"),
        ("hash", "module: password, verify"),
        (
            "auth",
            "module: blacklist, is_blacklisted, cleanup_expired (token blacklist over Postgres)",
        ),
        ("db", "module: connect (Postgres native driver + ORM)"),
        (
            "sql",
            "module: now, raw (SQL expressions for ORM .update / .where)",
        ),
        (
            "log",
            "module: info, warn, error, debug (structured logging)",
        ),
        // Phase 12.8 — built-in feature flags.
        (
            "flags",
            "module: is_enabled, list (feature flags with manifest [flags] + env var override)",
        ),
        // Mini-fase HTTP client (2026-06-18) — outbound HTTP client.
        // All methods async returning Future<Result<HttpClientResponse>>.
        // Body for post/put accepts Str | Map<Str,Any> (auto-JSON) | Bytes.
        (
            "http",
            "module: get, head, post, put, delete, request (HTTP client outbound)",
        ),
        // Mini-tanda SMTP builtin (2026-06-19) — outbound SMTP.
        // `smtp.send(opts)` async returning Future<Result<SmtpResult>>.
        // Env vars: SMTP_HOST/PORT/USER/PASSWORD/FROM/TLS at first send.
        (
            "smtp",
            "module: send (SMTP outbound; env vars: SMTP_HOST/PORT/USER/PASSWORD/FROM/TLS)",
        ),
    ] {
        items.push(CompletionItem {
            label: name.into(),
            kind: Some(CompletionItemKind::MODULE),
            detail: Some(detail.into()),
            ..CompletionItem::default()
        });
    }
    // Phase 12.8 — `flag(name) -> Bool` global builtin (parallel to
    // `secret`/`config`/`env`/etc).
    items.push(CompletionItem {
        label: "flag".into(),
        insert_text: Some("flag(\"${1:flag-name}\")".into()),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        kind: Some(CompletionItemKind::FUNCTION),
        detail: Some("flag(name: Str) -> Bool".into()),
        documentation: Some(Documentation::String(
            "Queries the feature-flag registry. Defaults from manifest `[flags]` + env var override `FITZ_FLAG_<UPPERCASE>`. Default `false` if not registered.".into(),
        )),
        ..CompletionItem::default()
    });

    // Built-in types: visible as names in annotation position.
    for name in [
        "Int",
        "Float",
        "Str",
        "Bool",
        "Null",
        "Bytes",
        "Range",
        "Any",
        "List",
        "Map",
        "Result",
        "Future",
        "Request",
        "Response",
        "File",
        "PyAny",
        "WsConn",
        "DbConn",
        "DbRow",
        // v0.10.24 — native temporal types and UUID.
        "Date",
        "DateTime",
        "Uuid",
        // Mini-fase HTTP client (2026-06-18) — response struct returned
        // by all http.* methods once `.await`-ed.
        "HttpClientResponse",
    ] {
        items.push(CompletionItem {
            label: name.into(),
            kind: Some(CompletionItemKind::CLASS),
            ..CompletionItem::default()
        });
    }

    // Language keywords. VSCode renders them with a distinct icon
    // and promotes them when the user types their first letters.
    for kw in [
        "let", "fn", "if", "else", "while", "for", "loop", "match", "type", "return", "break",
        "continue", "import", "from", "as", "in", "async", "await", "and", "or", "true", "false",
        "null",
    ] {
        items.push(CompletionItem {
            label: kw.into(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..CompletionItem::default()
        });
    }

    // User-declared nominals already appear via top-level TypeDef
    // (we walk them above). If the program imports nominals via
    // `from foo import User`, they also appear via FromImport. We
    // don't duplicate from `type_env.nominals` (would be redundant
    // with what we emitted above — and would mix the declaration
    // order of the Program, which is probably what the user wants
    // first).
    let _ = type_env; // silences the warning until type_env is used here.

    items
}

// ---------------------------------------------------------------------------
// V4 (2026-06-05) — Signature help (`textDocument/signatureHelp`)
// ---------------------------------------------------------------------------

/// V4 expanded (2026-06-05) — context of the `Call` enclosing the
/// cursor. The walkback identifies whether it's a fn call (`f(...)`)
/// or a method call (`<receiver>.method(...)`).
///
/// Returned by `find_call_context` together with the active param
/// index (counting `,` at depth 0 between the `(` and the cursor).
///
/// **Limitations**: doesn't respect strings or comments —
/// `f("hola, mundo|")` may miscount `,` inside the string. Rare in
/// practice.
#[derive(Debug, Clone, PartialEq)]
pub enum CallContext {
    /// `f(...)` — user-defined fn or global builtin.
    Function { name: String },
    /// `<receiver>.method(...)` — method call. MVP only supports
    /// `<ident>.method(...)` (Ident receiver). For more complex
    /// receivers (`xs[0].method`, `f().method`), the walkback
    /// degrades to `Function { name: method }`, which fails the
    /// lookup unless it matches a user-defined fn of the same name.
    Method {
        receiver_name: String,
        method: String,
    },
}

fn find_call_context(text: &str, line: u32, character: u32) -> Option<(CallContext, u32)> {
    let offset = position_to_offset(text, line, character)?;
    let bytes = text.as_bytes();
    let mut depth: i32 = 0;
    let mut commas_at_depth_0: u32 = 0;
    let mut i = offset;
    while i > 0 {
        i -= 1;
        let b = bytes[i];
        match b {
            b')' | b']' | b'}' => depth += 1,
            b'(' if depth == 0 => {
                // Found the enclosing `(`. Extract the callee
                // backwards.
                let mut j = i;
                // Skip whitespace between the ident and the `(`.
                while j > 0 && matches!(bytes[j - 1], b' ' | b'\t') {
                    j -= 1;
                }
                // Read ident backwards (callee/method).
                let id_end = j;
                while j > 0 && is_ident_continue(bytes[j - 1]) {
                    j -= 1;
                }
                if j == id_end {
                    return None; // No ident — it's a grouping paren.
                }
                if bytes[j].is_ascii_digit() {
                    return None;
                }
                let callee_name = std::str::from_utf8(&bytes[j..id_end]).ok()?.to_string();
                // V4 expanded — detect whether it's a method call:
                // look for a `.` before the callee (with optional
                // whitespace).
                let mut k = j;
                while k > 0 && matches!(bytes[k - 1], b' ' | b'\t') {
                    k -= 1;
                }
                if k > 0 && bytes[k - 1] == b'.' {
                    // Method call. Walk back to the receiver — MVP
                    // only Ident.
                    let mut m = k - 1; // before the `.`
                    while m > 0 && matches!(bytes[m - 1], b' ' | b'\t') {
                        m -= 1;
                    }
                    let recv_end = m;
                    while m > 0 && is_ident_continue(bytes[m - 1]) {
                        m -= 1;
                    }
                    if m < recv_end && !bytes[m].is_ascii_digit() {
                        let receiver_name =
                            std::str::from_utf8(&bytes[m..recv_end]).ok()?.to_string();
                        return Some((
                            CallContext::Method {
                                receiver_name,
                                method: callee_name,
                            },
                            commas_at_depth_0,
                        ));
                    }
                    // Receiver isn't a simple Ident — fall back to
                    // Function with the method name. Probably won't
                    // resolve, but at least it won't crash.
                }
                return Some((
                    CallContext::Function { name: callee_name },
                    commas_at_depth_0,
                ));
            }
            b'(' | b'[' | b'{' => depth -= 1,
            b',' if depth == 0 => commas_at_depth_0 += 1,
            _ => {}
        }
    }
    None
}

/// V4 expanded — catalog of signatures of global builtins. Covers
/// the most common ones from M1/M2/M3 of the course. Builtins
/// gradually typed as `Type::Any` have no concrete signature in the
/// TypeEnv — the catalog is the single source of truth for signature
/// help.
///
/// Format: `(name, label, param_labels)`.
/// `label` is the full `fn name(...) -> R` text shown in the popup.
/// `param_labels` are the sub-strings corresponding to each param —
/// the LSP uses them to highlight the active param via
/// `LabelOffsets`.
const BUILTIN_SIGS: &[(&str, &str, &[&str])] = &[
    (
        "print",
        "fn print(value: Any, ...) -> Null",
        &["value: Any", "..."],
    ),
    ("len", "fn len(x: Any) -> Int", &["x: Any"]),
    ("sleep", "fn sleep(ms: Int) -> Future<Null>", &["ms: Int"]),
    ("env", "fn env(key: Str) -> Result<Str>", &["key: Str"]),
    (
        "env_or",
        "fn env_or(key: Str, default: Str) -> Str",
        &["key: Str", "default: Str"],
    ),
    (
        "load_env",
        "fn load_env(path: Str) -> Result<Null>",
        &["path: Str"],
    ),
    ("flag", "fn flag(name: Str) -> Bool", &["name: Str"]),
    ("spawn", "fn spawn(call) -> Future<T>", &["call"]),
    (
        "config",
        "fn config(key: Str, default: Any) -> Any",
        &["key: Str", "default: Any"],
    ),
    (
        "secret",
        "fn secret(key: Str) -> Secret<Str>",
        &["key: Str"],
    ),
    ("bytes", "fn bytes(value: Str) -> Bytes", &["value: Str"]),
];

/// V4 expanded — catalog of signatures of built-in methods on
/// `List<T>`. Mirror of `infer_list_method` from the checker.
/// Template methods (`map`, `filter`, `find`, etc.) show the generic
/// shape with `T`/`U` — the student gets it from context.
const LIST_METHOD_SIGS: &[(&str, &str, &[&str])] = &[
    ("push", "fn push(value: T) -> Null", &["value: T"]),
    ("pop", "fn pop() -> T", &[]),
    ("len", "fn len() -> Int", &[]),
    (
        "map",
        "fn map(f: fn(T) -> U) -> List<U>",
        &["f: fn(T) -> U"],
    ),
    (
        "filter",
        "fn filter(f: fn(T) -> Bool) -> List<T>",
        &["f: fn(T) -> Bool"],
    ),
    (
        "find",
        "fn find(f: fn(T) -> Bool) -> Result<T>",
        &["f: fn(T) -> Bool"],
    ),
    (
        "any",
        "fn any(f: fn(T) -> Bool) -> Bool",
        &["f: fn(T) -> Bool"],
    ),
    (
        "all",
        "fn all(f: fn(T) -> Bool) -> Bool",
        &["f: fn(T) -> Bool"],
    ),
    (
        "count",
        "fn count(f: fn(T) -> Bool) -> Int",
        &["f: fn(T) -> Bool"],
    ),
    (
        "find_index",
        "fn find_index(f: fn(T) -> Bool) -> Result<Int>",
        &["f: fn(T) -> Bool"],
    ),
    (
        "flat_map",
        "fn flat_map(f: fn(T) -> List<U>) -> List<U>",
        &["f: fn(T) -> List<U>"],
    ),
];

/// V4 expanded — catalog of signatures of built-in methods on `Map<K, V>`.
const MAP_METHOD_SIGS: &[(&str, &str, &[&str])] = &[
    ("get", "fn get(key: K) -> Result<V>", &["key: K"]),
    ("has", "fn has(key: K) -> Bool", &["key: K"]),
    ("keys", "fn keys() -> List<K>", &[]),
    ("values", "fn values() -> List<V>", &[]),
    ("len", "fn len() -> Int", &[]),
];

/// V4 expanded — catalog of signatures of built-in methods on `Str`.
const STR_METHOD_SIGS: &[(&str, &str, &[&str])] = &[
    ("len", "fn len() -> Int", &[]),
    ("upper", "fn upper() -> Str", &[]),
    ("lower", "fn lower() -> Str", &[]),
    ("to_int", "fn to_int() -> Result<Int>", &[]),
];

/// V4 expanded — simple heuristic to infer the "kind" of an Ident
/// receiver without going through the entire checker. Walks the
/// `Program` looking for a top-level `Stmt::Assign { target:
/// Ident(receiver_name), value }` and matches the `value` by
/// structural shape.
///
/// Returns `Some("List" | "Map" | "Str")` or `None` if it can't be
/// determined (var not found, composite value, etc.).
///
/// MVP: only covers the 3 built-in types with method dispatch.
/// Custom types (`type Foo`) remain future debt.
fn infer_builtin_receiver_kind(program: &Program, receiver_name: &str) -> Option<&'static str> {
    for stmt in program {
        if let Stmt::Assign {
            target: crate::ast::AssignTarget::Ident(n, _),
            value,
            ..
        } = stmt
        {
            if n == receiver_name {
                return match value {
                    Expr::List(_, _) | Expr::ListComp { .. } => Some("List"),
                    Expr::Map(_, _) | Expr::MapComp { .. } => Some("Map"),
                    Expr::Str(_, _) | Expr::StrInterp(_, _) => Some("Str"),
                    // Other cases: we can't infer without the full
                    // checker. Minor debt.
                    _ => None,
                };
            }
        }
    }
    None
}

/// V4 expanded — builds SignatureHelp from a catalog entry of
/// builtins/methods.
fn signature_from_catalog(label: &str, param_labels: &[&str], active_param: u32) -> SignatureHelp {
    let mut parameters = Vec::with_capacity(param_labels.len());
    for plabel in param_labels {
        // Find the offset of the param label in the full label.
        if let Some(start) = label.find(plabel) {
            let start_u32 = start as u32;
            let end_u32 = (start + plabel.len()) as u32;
            parameters.push(ParameterInformation {
                label: ParameterLabel::LabelOffsets([start_u32, end_u32]),
                documentation: None,
            });
        } else {
            // Fallback: label as a string if no offset is found.
            parameters.push(ParameterInformation {
                label: ParameterLabel::Simple((*plabel).to_string()),
                documentation: None,
            });
        }
    }
    let n_params = parameters.len() as u32;
    SignatureHelp {
        signatures: vec![SignatureInformation {
            label: label.to_string(),
            documentation: None,
            parameters: Some(parameters),
            active_parameter: None,
        }],
        active_signature: Some(0),
        active_parameter: Some(active_param.min(n_params.saturating_sub(1))),
    }
}

/// V4 — builds a `SignatureHelp` for a Call with callee `name`.
/// Looks in the `Program` for a top-level `Stmt::FnDef` with that
/// name and renders its signature. `active_param` indicates which
/// parameter is under the cursor (0-based).
///
/// MVP: only covers top-level fns of the current program. Builtins
/// (`print`, `len`, `jwt`/`hash`/etc. modules) don't appear — they
/// remain minor debt (most builtins type as gradual `Type::Any`, and
/// the concrete signatures live in `infer_*_method` by type).
pub fn signature_help_for_call(
    program: &Program,
    name: &str,
    active_param: u32,
) -> Option<SignatureHelp> {
    for stmt in program {
        if let Stmt::FnDef {
            name: fn_name,
            params,
            return_type,
            ..
        } = stmt
        {
            if fn_name != name {
                continue;
            }
            // Build label: `fn name(p1: T1, p2: T2) -> R`.
            // And a Vec<ParameterInformation> with each param's label.
            let mut label = format!("fn {}(", name);
            let mut parameters = Vec::with_capacity(params.len());
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    label.push_str(", ");
                }
                let p_start = label.len() as u32;
                label.push_str(&p.name);
                if let Some(t) = &p.type_ {
                    label.push_str(": ");
                    label.push_str(&t.display_name());
                }
                let p_end = label.len() as u32;
                parameters.push(ParameterInformation {
                    label: ParameterLabel::LabelOffsets([p_start, p_end]),
                    documentation: None,
                });
            }
            label.push(')');
            if let Some(rt) = return_type {
                label.push_str(" -> ");
                label.push_str(&rt.display_name());
            }
            let n_params = parameters.len() as u32;
            return Some(SignatureHelp {
                signatures: vec![SignatureInformation {
                    label,
                    documentation: None,
                    parameters: Some(parameters),
                    active_parameter: None,
                }],
                active_signature: Some(0),
                // Clamp so we don't run out of range (the user may
                // be typing more args than the fn accepts — the
                // checker emits the error, the LSP simply doesn't
                // highlight anything).
                active_parameter: Some(active_param.min(n_params.saturating_sub(1))),
            });
        }
    }
    None
}

/// V4 — orchestrator of the `signature_help` handler. Pure
/// function: detects the enclosing call in the `text` and resolves
/// the signature against the `Program`. Returns `None` if there is
/// no call or if the callee isn't a known top-level fn.
///
/// V4 expanded (2026-06-05) — dispatcher by call kind:
///
/// 1. `CallContext::Function { name }`:
///    - First looks up a user-defined fn in the `Program`.
///    - Then looks it up in the builtin catalog (`print`, `len`,
///      etc.).
/// 2. `CallContext::Method { receiver_name, method }`:
///    - Determines the receiver "kind" via
///      `infer_builtin_receiver_kind` (`List`/`Map`/`Str` from the
///      structural shape of the assigned value).
///    - Looks for the method signature in the corresponding
///      catalog.
pub fn signature_help_at_position(
    text: &str,
    program: &Program,
    line: u32,
    character: u32,
) -> Option<SignatureHelp> {
    let (ctx, active) = find_call_context(text, line, character)?;
    match ctx {
        CallContext::Function { name } => {
            // 1. Look up a user-defined fn in the Program.
            if let Some(sig) = signature_help_for_call(program, &name, active) {
                return Some(sig);
            }
            // 2. Builtin catalog.
            for (bname, label, param_labels) in BUILTIN_SIGS {
                if *bname == name {
                    return Some(signature_from_catalog(label, param_labels, active));
                }
            }
            None
        }
        CallContext::Method {
            receiver_name,
            method,
        } => {
            // Determine the receiver type via structural shape.
            let kind = infer_builtin_receiver_kind(program, &receiver_name)?;
            let catalog = match kind {
                "List" => LIST_METHOD_SIGS,
                "Map" => MAP_METHOD_SIGS,
                "Str" => STR_METHOD_SIGS,
                _ => return None,
            };
            for (mname, label, param_labels) in catalog {
                if *mname == method {
                    return Some(signature_from_catalog(label, param_labels, active));
                }
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    fn err_at(line: usize, column: usize, msg: &str) -> FitzError {
        FitzError::new(ErrorKind::TypeError, line, column, msg)
    }

    #[test]
    fn error_with_position_maps_to_0_based_1_character_range() {
        let errs = vec![err_at(3, 5, "tipo incompatible")];
        let diags = fitz_errors_to_diagnostics(&errs);
        assert_eq!(diags.len(), 1);
        let d = &diags[0];
        assert_eq!(d.range.start, Position::new(2, 4)); // 1-based → 0-based
        assert_eq!(d.range.end, Position::new(2, 5));
        assert_eq!(d.message, "tipo incompatible");
        assert_eq!(d.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(d.source.as_deref(), Some("fitz"));
    }

    #[test]
    fn error_without_position_maps_to_degenerate_range_at_start() {
        let errs = vec![err_at(0, 0, "without line or column")];
        let diags = fitz_errors_to_diagnostics(&errs);
        assert_eq!(diags[0].range.start, Position::new(0, 0));
        assert_eq!(diags[0].range.end, Position::new(0, 0));
    }

    #[test]
    fn error_with_hint_concatenates_suggestion_to_message() {
        let err = err_at(1, 1, "undefined variable").with_hint("did you mean `name`?");
        let diags = fitz_errors_to_diagnostics(&[err]);
        assert!(
            diags[0].message.contains("undefined variable"),
            "message base: {}",
            diags[0].message,
        );
        assert!(
            diags[0].message.contains("Hint: did you mean `name`?"),
            "message with hint: {}",
            diags[0].message,
        );
    }

    #[test]
    fn error_without_hint_does_not_add_suggestion_word() {
        let errs = vec![err_at(1, 1, "incompatible type")];
        let diags = fitz_errors_to_diagnostics(&errs);
        assert!(!diags[0].message.contains("Hint"));
    }

    #[test]
    fn empty_list_returns_empty_vec() {
        let diags = fitz_errors_to_diagnostics(&[]);
        assert!(diags.is_empty());
    }

    #[test]
    fn multiple_errors_preserve_order() {
        let errs = vec![
            err_at(1, 1, "primero"),
            err_at(5, 3, "segundo"),
            err_at(0, 0, "third without position"),
        ];
        let diags = fitz_errors_to_diagnostics(&errs);
        assert_eq!(diags.len(), 3);
        assert_eq!(diags[0].message, "primero");
        assert_eq!(diags[1].message, "segundo");
        assert_eq!(diags[2].message, "third without position");
    }

    // Tests over `check_source` — entire LSP-style pipeline.

    #[test]
    fn check_source_valid_program_emits_no_errors() {
        let src = "let x = 1\nlet y = 2\nprint(x + y)";
        let errs = check_source(src);
        assert!(errs.is_empty(), "errores inesperados: {errs:?}");
    }

    #[test]
    fn check_source_type_error_comes_from_checker() {
        let src = "let x: Int = \"texto\"";
        let errs = check_source(src);
        assert!(!errs.is_empty(), "checker should reject Int = Str");
        assert!(
            errs.iter().any(|e| matches!(e.kind, ErrorKind::TypeError)),
            "expected at least one TypeError: {errs:?}",
        );
    }

    #[test]
    fn check_source_recovery_does_not_abort_on_broken_stmts() {
        // The parser with recovery should give us partial AST +
        // errors; the checker checks what it recovered. Without
        // recovery, the pipeline would abort at the first error. The
        // smoke here is that `check_source` returns something (no
        // panic) over input with broken syntax.
        let src = "let x = ???\nlet y = 1\nlet z: Int = \"mal\"";
        let errs = check_source(src);
        assert!(!errs.is_empty(), "should have at least one parser error",);
    }

    // Tests over `check_source_with_types` — variant for hover
    // (Phase 9.x.2). The new thing vs. `check_source` is that it
    // retains the `TypeInfo` populated by F16.

    // ----- Regression: built-ins not flagged as unknown (v0.19.6+) -----
    // Bug discovered 2026-06-28 during fitzwatch work: VSCode LSP shows
    // false-positive squiggles for `smtp` (mini-tanda SMTP v0.18.0) and
    // for fields of the `Response` built-in (v0.19.0), while `fitz check`
    // CLI accepts the code without errors. Both tests below MUST pass if
    // the LSP path mirrors the CLI path (CheckCtx::new + register_builtins
    // + register_http_builtin_types).
    //
    // If they pass, the user-visible bug is H1 (stale `.vsix` with an
    // older fitz-lsp.exe bundled). If they fail, it's a code divergence
    // (H2) and that's where the fix has to land.

    #[test]
    fn lsp_repro_smtp_module_no_unknown_variable() {
        let src = r#"
async fn send_test(to: Str, body: Str) -> Result<Bool> {
    let r = smtp.send({
        "to": to,
        "subject": "t",
        "body_html": body,
    }).await?
    return Ok(true)
}
"#;
        let (_p, _env, _info, _defs, errors) = check_source_with_types(src);
        let smtp_errors: Vec<&FitzError> = errors
            .iter()
            .filter(|e| {
                let m = &e.message;
                m.contains("smtp") || m.contains("unknown variable")
            })
            .collect();
        assert!(
            smtp_errors.is_empty(),
            "LSP should not flag `smtp` as unknown variable: {smtp_errors:?}",
        );
    }

    #[test]
    fn lsp_repro_response_built_in_no_field_not_found() {
        let src = r#"
@get("/test")
async fn h() -> Result<Response> {
    return Ok(Response {
        status: 200,
        content_type: "text/plain",
        headers: { "X-Foo": "bar" },
        body: "hi",
    })
}
"#;
        let (_p, _env, _info, _defs, errors) = check_source_with_types(src);
        let response_errors: Vec<&FitzError> = errors
            .iter()
            .filter(|e| {
                let m = &e.message;
                m.contains("Response") && (m.contains("field") || m.contains("does not have"))
            })
            .collect();
        assert!(
            response_errors.is_empty(),
            "LSP should not flag fields of the `Response` built-in: {response_errors:?}",
        );
    }

    // Same as above but exercising the cross-module branch of
    // `check_source_with_types_and_base_dir(base_dir = Some(...))`,
    // which goes through `resolve_program_with_env` +
    // `pre_scan_imported_*` + `check_with_env` instead of the
    // single-file `check_program`. Both branches must register the
    // built-ins via `register_http_builtin_types` (TypeEnv) and
    // `CheckCtx::new` (registers `smtp`/`http`/etc).
    #[test]
    fn lsp_repro_cross_module_branch_also_registers_smtp_and_response() {
        // Use a non-existent base_dir — the pre-scan silently skips
        // unresolvable imports (silent-fallback policy), but the
        // registration of built-ins still happens because it lives
        // BEFORE the import walk.
        let bd = std::path::Path::new("d:/this/dir/does/not/exist");
        let src = r#"
async fn send_one(to: Str) -> Result<Bool> {
    let r = smtp.send({"to": to, "subject": "x", "body": "y"}).await?
    return Ok(true)
}

@get("/r")
fn make_resp() -> Response {
    return Response {
        status: 200,
        content_type: "text/plain",
        headers: {},
        body: "ok",
    }
}
"#;
        let (_p, _env, _info, _defs, errors) = check_source_with_types_and_base_dir(src, Some(bd));
        let relevant: Vec<&FitzError> = errors
            .iter()
            .filter(|e| {
                let m = &e.message;
                m.contains("smtp")
                    || m.contains("unknown variable")
                    || (m.contains("Response")
                        && (m.contains("field") || m.contains("does not have")))
            })
            .collect();
        assert!(
            relevant.is_empty(),
            "cross-module branch must register built-ins too: {relevant:?}",
        );
    }

    // Parallel audit — all built-in MODULES registered in
    // `CheckCtx::register_builtins` (smtp/http/jwt/hash/log/db/auth/flags)
    // must be visible from the LSP path. Single test instead of one per
    // module to keep noise low; if a module disappears from the registry,
    // the failure message names which one.
    #[test]
    fn lsp_audit_all_builtin_modules_visible_from_lsp_path() {
        for module in &[
            "smtp", "http", "jwt", "hash", "log", "db", "sql", "auth", "flags",
        ] {
            let src = format!("let m = {}\nprint(m)\n", module);
            let errs = check_source(&src);
            let unknown: Vec<&FitzError> = errs
                .iter()
                .filter(|e| {
                    let m = &e.message;
                    m.contains("unknown variable") || m.contains(*module)
                })
                .collect();
            assert!(
                unknown.is_empty(),
                "built-in module `{module}` should be visible from the LSP path: {unknown:?}",
            );
        }
    }

    // Parallel audit — all built-in NOMINAL types registered in
    // `register_http_builtin_types` (Request/Response/File/HttpClientResponse/SmtpResult)
    // must be visible AND their fields must type-check. Single test for
    // the same reason as above; uses one canonical field per nominal as
    // a smoke that the resolution went all the way.
    #[test]
    fn lsp_audit_all_builtin_nominals_visible_from_lsp_path() {
        // (type_name, sample_field) — picks one obvious field per
        // built-in nominal. If the field disappears (rename, removal),
        // the test fails and the assertion message points at it.
        let cases = &[
            ("Request", "method"),
            ("Response", "status"),
            ("File", "name"),
            ("HttpClientResponse", "status"),
            ("SmtpResult", "delivered"),
        ];
        for (ty, field) in cases {
            // Smoke: function annotated with the built-in, accesses the
            // field. If the nominal is missing or fields are not
            // registered, the checker emits errors mentioning the type
            // name or "field".
            let src = format!(
                "fn use_it(x: {ty}) -> Bool {{\n    let _ = x.{field}\n    return true\n}}\n"
            );
            let errs = check_source(&src);
            let relevant: Vec<&FitzError> = errs
                .iter()
                .filter(|e| {
                    let m = &e.message;
                    m.contains(*ty) || m.contains("field") || m.contains("nominal")
                })
                .collect();
            assert!(
                relevant.is_empty(),
                "built-in nominal `{ty}` with field `{field}` should type-check from the LSP path: {relevant:?}",
            );
        }
    }

    #[test]
    fn check_source_with_types_valid_program_returns_non_empty_type_info() {
        let src = "let x = 42\nlet y = x + 1";
        let (_program, _env, type_info, _defs, errors) = check_source_with_types(src);
        assert!(errors.is_empty(), "errores inesperados: {errors:?}");
        assert!(
            !type_info.is_empty(),
            "TypeInfo should not be empty on a program with Exprs",
        );
    }

    #[test]
    fn check_source_with_types_lexer_error_returns_empty_type_info() {
        // Unclosed string — lexer aborts before the parser/checker,
        // so `TypeInfo` can't be populated.
        let src = "let x = \"sin cerrar";
        let (_program, _env, type_info, _defs, errors) = check_source_with_types(src);
        assert!(!errors.is_empty(), "lexer should reject unclosed string");
        assert!(
            type_info.is_empty(),
            "TypeInfo should be empty if the pipeline aborts in the lexer",
        );
    }

    #[test]
    fn check_source_with_types_type_error_does_not_clear_type_info() {
        // The checker checks what it can even with errors: valid
        // Exprs end up in TypeInfo, invalid ones too with the
        // "best-effort" type.
        let src = "let x = 42\nlet y: Int = \"mal\"";
        let (_program, _env, type_info, _defs, errors) = check_source_with_types(src);
        assert!(!errors.is_empty(), "should have a TypeError");
        assert!(
            !type_info.is_empty(),
            "TypeInfo should retain types of valid Exprs despite the error",
        );
    }

    // Tests over `hover_for_position` and `make_hover` (Phase 9.x.2.b).

    #[test]
    fn hover_for_position_returns_type_at_exact_literal_position() {
        // `let x = 42` — the literal `42` starts at col 9 (1-based),
        // which is LSP col 8 (0-based). The cursor at (line=0,
        // char=8) should match the Int.
        let src = "let x = 42";
        let (_program, _env, type_info, _defs, _errs) = check_source_with_types(src);
        let ty = hover_for_position(&type_info, 0, 8);
        assert!(matches!(ty, Some(Type::Int)), "expected Int, got {ty:?}");
    }

    #[test]
    fn hover_for_position_returns_type_in_middle_of_ident_used_as_expr() {
        // The left side of a `let` is an AssignTarget, not an Expr
        // — those idents do NOT enter TypeInfo. To test the cursor
        // "in the middle of an identifier" case we need the ident
        // to be an Expr (use, not declaration):
        //
        //   let nombre = 42         (line 0)
        //   let x = nombre + 1      (line 1)
        //
        // `nombre` on line 1 starts at col 9 (1-based) = col 8
        // (0-based). Cursor in the middle (line 1, col 11 LSP / col
        // 12 Fitz) falls inside the Ident. The "max col <= cursor on
        // the same line" heuristic must return Some(_) (the type of
        // the Ident or the BinOp that shares the span — both Int).
        let src = "let nombre = 42\nlet x = nombre + 1";
        let (_program, _env, type_info, _defs, _errs) = check_source_with_types(src);
        let ty = hover_for_position(&type_info, 1, 11);
        assert!(matches!(ty, Some(Type::Int)), "expected Int, got {ty:?}");
    }

    #[test]
    fn hover_for_position_line_without_spans_returns_none() {
        // Single-line program; cursor on line 5 → no spans.
        let src = "let x = 1";
        let (_program, _env, type_info, _defs, _errs) = check_source_with_types(src);
        let ty = hover_for_position(&type_info, 5, 0);
        assert!(
            ty.is_none(),
            "expected None on line without spans, got {ty:?}"
        );
    }

    #[test]
    fn hover_for_position_cursor_before_first_token_returns_none() {
        // `   let x = 1` — cursor at col 0 is before any Expr (the
        // first Expr is `1` at col 13 (1-based)).
        let src = "   let x = 1";
        let (_program, _env, type_info, _defs, _errs) = check_source_with_types(src);
        let ty = hover_for_position(&type_info, 0, 0);
        assert!(
            ty.is_none(),
            "expected None before the first token, got {ty:?}"
        );
    }

    #[test]
    fn hover_for_position_two_lines_does_not_cross_line() {
        // We make sure the heuristic doesn't "escape" to the
        // previous line when the cursor's line is empty of spans.
        let src = "let x = 42\n   ";
        let (_program, _env, type_info, _defs, _errs) = check_source_with_types(src);
        let ty = hover_for_position(&type_info, 1, 0);
        assert!(ty.is_none(), "should not cross lines, got {ty:?}");
    }

    #[test]
    fn make_hover_emits_markdown_with_fitz_block() {
        let env = TypeEnv::default();
        let hover = make_hover(&Type::Int, &env);
        match &hover.contents {
            HoverContents::Markup(MarkupContent { kind, value }) => {
                assert_eq!(*kind, MarkupKind::Markdown);
                assert_eq!(value, "```fitz\nInt\n```");
            }
            other => panic!("expected Markup, got {other:?}"),
        }
        assert!(hover.range.is_none(), "range debe ser None hasta end_span");
    }

    #[test]
    fn make_hover_formats_composite_types_with_display() {
        let env = TypeEnv::default();
        let list_int = Type::List(Box::new(Type::Int));
        let hover = make_hover(&list_int, &env);
        if let HoverContents::Markup(MarkupContent { value, .. }) = &hover.contents {
            assert_eq!(value, "```fitz\nList<Int>\n```");
        } else {
            panic!("expected Markup");
        }
    }

    #[test]
    fn hover_end_to_end_pipeline_returns_int_for_a_literal() {
        // Combined smoke: pipeline + hover over the literal `42`.
        let src = "let x = 42";
        let (_program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let ty = hover_for_position(&type_info, 0, 8).expect("should match");
        let hover = make_hover(ty, &env);
        if let HoverContents::Markup(MarkupContent { value, .. }) = &hover.contents {
            assert_eq!(value, "```fitz\nInt\n```");
        } else {
            panic!("expected Markup");
        }
    }

    #[test]
    fn check_source_and_with_types_return_same_error_list() {
        // Sanity check: both APIs share the pipeline, the errors
        // should be equivalent (same order, same count, same
        // messages).
        let src = "let x: Int = \"mal\"\nlet y: Str = 42";
        let errs_solo = check_source(src);
        let (_program, _env, _type_info, _defs, errs_with) = check_source_with_types(src);
        assert_eq!(errs_solo.len(), errs_with.len());
        for (a, b) in errs_solo.iter().zip(errs_with.iter()) {
            assert_eq!(a.message, b.message);
            assert_eq!(a.line, b.line);
            assert_eq!(a.column, b.column);
        }
    }

    // Tests over `definition_for_position` and
    // `make_definition_location` (Phase 9.x.3.b).

    #[test]
    fn definition_for_position_returns_local_var_declaration_span() {
        // `let x = 1` on line 0, `let y = x` on line 1. The use of
        // `x` is on line 1, col 8 (0-based) — the returned
        // `def_span` must be on line 1 (1-based, the Stmt::Assign of
        // `x`).
        let src = "let x = 1\nlet y = x\n";
        let (_program, _env, _type_info, def_info, _errs) = check_source_with_types(src);
        let def_span = definition_for_position(&def_info, 1, 8).expect("uso de x debe resolver");
        assert_eq!(def_span.line, 1, "def on line 1 (1-based)");
    }

    #[test]
    fn definition_for_position_line_without_idents_returns_none() {
        let src = "let x = 1\n";
        let (_program, _env, _type_info, def_info, _errs) = check_source_with_types(src);
        assert!(definition_for_position(&def_info, 5, 0).is_none());
    }

    #[test]
    fn definition_for_position_does_not_resolve_builtin_use() {
        // `print(42)` — `print` is a builtin with def_span
        // Span::ZERO. It must not appear in DefinitionInfo (filtered
        // by policy), so the lookup returns None.
        let src = "print(42)\n";
        let (_program, _env, _type_info, def_info, _errs) = check_source_with_types(src);
        // Cursor over `print` (line 0, col 0).
        assert!(definition_for_position(&def_info, 0, 0).is_none());
    }

    #[test]
    fn make_definition_location_converts_1_based_to_0_based() {
        let uri = Url::parse("file:///test.fitz").unwrap();
        // def_span at line 3, col 5 (1-based) → LSP line 2, col 4 (0-based).
        let loc = make_definition_location(uri.clone(), Span::new(3, 5));
        assert_eq!(loc.uri, uri);
        assert_eq!(loc.range.start, Position::new(2, 4));
        assert_eq!(loc.range.end, Position::new(2, 5));
    }

    #[test]
    fn definition_end_to_end_pipeline_returns_def_location() {
        // Smoke combinado: pipeline + definition_for_position +
        // make_definition_location.
        let src = "let x = 1\nlet y = x\n";
        let (_program, _env, _type_info, def_info, _errs) = check_source_with_types(src);
        let def_span = definition_for_position(&def_info, 1, 8).expect("matchea");
        let uri = Url::parse("file:///t.fitz").unwrap();
        let loc = make_definition_location(uri, def_span);
        // The Stmt::Assign of `x` is on line 1 (1-based) → line 0
        // (0-based). Its column depends on the parser; we assume
        // col 1 (1-based, first character of `let`).
        assert_eq!(loc.range.start.line, 0);
    }

    // Tests over `completion_at_position` and private helpers
    // (Phase 9.x.4.a). Cover context detection (after-dot vs.
    // scope-level), offset conversion, and the two completion
    // paths.

    #[test]
    fn position_to_offset_and_back_are_inverses() {
        // Sanity: the composed inverse recovers the position.
        let text = "abc\nde\nfghi";
        for (line, ch) in [(0, 0), (0, 2), (1, 0), (1, 1), (2, 3)] {
            let off = position_to_offset(text, line, ch).unwrap();
            let (l, c) = offset_to_position(text, off);
            assert_eq!((l, c), (line, ch), "round-trip falla en ({line},{ch})");
        }
    }

    #[test]
    fn detect_context_scope_level_in_empty_document() {
        let ctx = detect_completion_context("", 0, 0).unwrap();
        assert_eq!(ctx, CompletionContext::ScopeLevel);
    }

    #[test]
    fn detect_context_after_dot_after_ident_and_dot() {
        // `obj.` with cursor right after the `.`.
        let text = "obj.";
        let ctx = detect_completion_context(text, 0, 4).unwrap();
        match ctx {
            CompletionContext::AfterDot {
                recv_name,
                recv_line,
                recv_col,
            } => {
                // Receiver `obj` starts at line 1, col 1 (Fitz 1-based).
                assert_eq!(recv_name, "obj");
                assert_eq!(recv_line, 1);
                assert_eq!(recv_col, 1);
            }
            other => panic!("expected AfterDot, got {other:?}"),
        }
    }

    #[test]
    fn detect_context_after_dot_translates_recv_col_with_smp_before() {
        // v0.13.2 — comment with emoji + receiver + dot. The client
        // sends the cursor at col_utf16 = post-`.` offset in UTF-16.
        // `detect_completion_context` must build AfterDot with
        // recv_col in Unicode chars (for the lexer's TypeInfo
        // lookup), not in UTF-16 units. The internal translation
        // uses the `utf16_to_unicode_char` helper.
        //
        // Text: `// 🎉 obj.`
        //   col_unicode: 0  1  2  3      4  5  6  7  8
        //                /  /     emoji        o  b  j  .
        //   col_utf16:   0  1  2  3-4    5  6  7  8  9  10
        //
        // Cursor right after the `.` → char_utf16 = 10.
        // Receiver `obj` starts at char_unicode = 5 → Fitz 1-based
        // 6. Without the internal translation, it would give
        // recv_col = 7 (utf16 post-emoji + 1), which does NOT match
        // the lexer's SpanKey.
        let text = "// 🎉 obj.";
        let ctx = detect_completion_context(text, 0, 10).unwrap();
        match ctx {
            CompletionContext::AfterDot {
                recv_name,
                recv_line,
                recv_col,
            } => {
                assert_eq!(recv_name, "obj");
                assert_eq!(recv_line, 1);
                assert_eq!(
                    recv_col, 6,
                    "recv_col in 1-based Unicode chars (not UTF-16) post-translation"
                );
            }
            other => panic!("expected AfterDot, got {other:?}"),
        }
    }

    #[test]
    fn detect_context_after_dot_with_partial_prefix() {
        // `obj.fo` with cursor at the end → the user already typed
        // "fo" of the method. The context is still AfterDot; VSCode
        // filters by the prefix client-side.
        let text = "obj.fo";
        let ctx = detect_completion_context(text, 0, 6).unwrap();
        assert!(matches!(ctx, CompletionContext::AfterDot { .. }));
    }

    #[test]
    fn detect_context_scope_level_in_middle_of_ident() {
        // `obj` without a `.` afterwards → scope-level. Cursor in
        // the middle of the ident; VSCode filters the typed prefix.
        let text = "obj";
        let ctx = detect_completion_context(text, 0, 3).unwrap();
        assert_eq!(ctx, CompletionContext::ScopeLevel);
    }

    // ---- v0.9.51 J mini-batch — UTF-8 position + F15 sub-stmt recovery ----

    #[test]
    fn position_to_offset_counts_utf16_code_units_not_unicode_chars() {
        // v0.13.2 — the server omits `positionEncoding` (LSP default
        // = UTF-16). `position_to_offset` counts UTF-16 code units
        // to match what VSCode sends.
        // Case 1: pure ASCII — col 6 points to `=`. For ASCII,
        // char_utf16 == char_unicode == byte_offset.
        let text = "let x = 42";
        let offset = position_to_offset(text, 0, 6).expect("valid offset");
        assert_eq!(&text[offset..offset + 1], "=", "col 6 en ASCII → `=`");
        // Case 2: with emoji 😀 (4 bytes UTF-8, 2 UTF-16 code units,
        // 1 Unicode char). The comment starts at col_utf16 0 = `/`,
        // `/` at col_utf16 1, ` ` at col_utf16 2, the emoji occupies
        // col_utf16 3-4 (surrogate pair), ` ` at col_utf16 5.
        let text = "// 😀 hola";
        // Cursor at col_utf16 5 (right after emoji + space) → byte
        // offset = `// ` (3) + emoji UTF-8 (4) = 7.
        let offset = position_to_offset(text, 0, 5).expect("valid offset after emoji");
        assert_eq!(
            offset, 7,
            "offset esperado tras emoji + espacio = 7 bytes UTF-8 (col_utf16 5)"
        );
        // Cursor at col_utf16 3 (start of the emoji's surrogate
        // pair) → byte offset = 3 (right after the `// `, on the
        // emoji).
        let offset = position_to_offset(text, 0, 3).expect("valid offset over the emoji");
        assert_eq!(offset, 3, "col_utf16 3 = inicio del emoji → byte offset 3");
    }

    #[test]
    fn position_to_offset_tolerates_mid_surrogate() {
        // v0.13.2 — if the client sends a position in the middle of
        // a surrogate pair (col_utf16 == 4 inside "// 😀"), our
        // counting uses `>=` and returns the offset at the END of
        // the invalid char. A well-behaved client (VSCode) doesn't
        // generate that case, but we want defensive tolerance
        // instead of None.
        let text = "// 😀 hola";
        // col_utf16 4 = middle of the surrogate pair. After the
        // emoji (which ends at col_utf16 5), the byte offset is 7.
        let offset = position_to_offset(text, 0, 4).expect("mid-surrogate tolera con >=");
        assert_eq!(offset, 7, "mid-surrogate → offset post-emoji");
    }

    #[test]
    fn offset_to_position_counts_utf16_parallel_to_position_to_offset() {
        // Round-trip: offset → position (UTF-16) → offset must
        // return the same offset (as long as the offset is on a
        // char boundary).
        let text = "let x = 1\nlet y: Str = \"😀\"\n";
        let original_offset = text.find('y').unwrap();
        let (line, character) = offset_to_position(text, original_offset);
        let recovered = position_to_offset(text, line, character).unwrap();
        assert_eq!(
            recovered, original_offset,
            "round-trip offset → position → offset debe ser idempotente"
        );
    }

    #[test]
    fn offset_to_position_emoji_returns_utf16_units() {
        // v0.13.2 — `offset_to_position` returns char_utf16 (not
        // char_unicode). For `"🎉a"`, the offset of `a` (byte 4)
        // must return (0, 2) because 🎉 takes 2 UTF-16 units.
        let text = "🎉a";
        let offset_a = text.find('a').unwrap();
        let (line, character) = offset_to_position(text, offset_a);
        assert_eq!(line, 0, "same line");
        assert_eq!(character, 2, "`a` is at char_utf16 2 (post-emoji)");
    }

    #[test]
    fn utf16_to_unicode_char_identity_for_ascii() {
        // For pure ASCII, char_utf16 == char_unicode.
        let text = "let x = 42";
        for col_utf16 in [0u32, 4, 6, 9, 10] {
            assert_eq!(
                utf16_to_unicode_char(text, 0, col_utf16),
                col_utf16,
                "ASCII puro: utf16 {col_utf16} → unicode {col_utf16}",
            );
        }
    }

    #[test]
    fn utf16_to_unicode_char_collapses_smp() {
        // For Supplementary Multilingual Plane chars (emoji), 1
        // Unicode char = 2 UTF-16 code units. The helper collapses.
        let text = "// 🎉 hola";
        // Expected mapping:
        //   col_unicode: 0  1  2  3      4  5  6  7  8  9
        //                /  /     emoji        h  o  l  a
        //   col_utf16:   0  1  2  3-4    5  6  7  8  9  10
        assert_eq!(
            utf16_to_unicode_char(text, 0, 0),
            0,
            "col_utf16 0 → unicode 0"
        );
        assert_eq!(
            utf16_to_unicode_char(text, 0, 3),
            3,
            "inicio emoji surrogate"
        );
        assert_eq!(
            utf16_to_unicode_char(text, 0, 5),
            4,
            "post-emoji → unicode 4"
        );
        assert_eq!(
            utf16_to_unicode_char(text, 0, 10),
            9,
            "end of line → unicode 9"
        );
    }

    #[test]
    fn utf16_to_unicode_char_multiline() {
        // Each line resets the counter.
        let text = "🎉\nlet x = 42";
        // Line 0: just the emoji (utf16 0..2, unicode 0..1).
        assert_eq!(
            utf16_to_unicode_char(text, 0, 2),
            1,
            "end of line 0 post-emoji"
        );
        // Line 1: pure ASCII.
        assert_eq!(utf16_to_unicode_char(text, 1, 4), 4, "line 1, col_utf16 4");
        assert_eq!(utf16_to_unicode_char(text, 1, 0), 0, "line 1, col_utf16 0");
    }

    #[test]
    fn f15_recovery_sub_stmt_preserves_field_access_with_orphan_dot() {
        // Pre-fix: `user.<EOF>` aborted the entire stmt
        // (Stmt::Error). Post-fix: `parse_with_recovery` returns an
        // AST with
        // `Stmt::Expr(Expr::Field { object: Ident("user"), field: "" })`,
        // letting the LSP use the object's type for completion.
        use crate::ast::{Expr, Stmt};
        use crate::lexer::tokenize;
        use crate::parser::parse_with_recovery;
        let src = "let user = 42\nuser.";
        let tokens = tokenize(src).expect("tokenize OK");
        let (program, errors) = parse_with_recovery(tokens);
        // At least 1 reported error (a field was expected).
        assert!(
            !errors.is_empty(),
            "recovery must report the error of the orphan `.`"
        );
        // The second stmt must be Expr::Field with an empty
        // `field`, NOT Stmt::Error.
        assert!(
            program.len() >= 2,
            "expected at least 2 stmts: the let + the user.<EOF>. Got: {} stmts",
            program.len()
        );
        let last = program.last().expect("last stmt");
        match last {
            Stmt::Expr(Expr::Field { object, field, .. }, _) => {
                assert_eq!(field, "", "field must be empty placeholder");
                assert!(
                    matches!(object.as_ref(), Expr::Ident(name, _) if name == "user"),
                    "object debe ser Ident(\"user\"), got: {:?}",
                    object
                );
            }
            other => panic!(
                "expected Stmt::Expr(Expr::Field {{ field: \"\" }}), got: {:?}",
                other
            ),
        }
    }

    #[test]
    fn f15_recovery_sub_stmt_completion_after_dot_works_on_local_var() {
        // LSP case: cursor at `user.<cursor>` inside a fn, with
        // `user: User` declared locally. Pre-fix: the whole stmt
        // was discarded, completion only saw top-level vars via
        // fallback. Post-fix: the Expr::Field preserves `user`,
        // and the TypeInfo lookup finds the object's type.
        let src = "type User { id: Int, name: Str }\n\
                   fn process() {\n  \
                     let u: User = User { id: 1, name: \"x\" }\n  \
                     u.\n\
                   }\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor on line 3 (0-based: line 3 of the source, after
        // `u.`) — col 4 (after the dot).
        let items = completion_at_position(src, &program, &type_info, &env, 3, 4);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"id"),
            "expected field `id` of User in completion, labels: {:?}",
            labels
        );
        assert!(
            labels.contains(&"name"),
            "expected field `name` of User in completion, labels: {:?}",
            labels
        );
    }

    // ---- v0.9.47 LSPz mini-batch — chain a.b.c. + from import ----

    #[test]
    fn detect_context_chain_of_two_segments_captures_complete_recv() {
        // `a.b.|` with cursor right after the second `.` → AfterDot
        // with recv_name = "a.b" (chain, not just "b").
        let text = "a.b.";
        let ctx = detect_completion_context(text, 0, 4).unwrap();
        match ctx {
            CompletionContext::AfterDot {
                recv_name,
                recv_line,
                recv_col,
            } => {
                assert_eq!(recv_name, "a.b", "recv_name should be the full chain");
                assert_eq!(recv_line, 1);
                assert_eq!(recv_col, 1, "chain start is col 1 (the `a`)");
            }
            other => panic!("expected AfterDot with chain, got {other:?}"),
        }
    }

    #[test]
    fn detect_context_chain_of_three_segments_with_partial_prefix() {
        // `obj.field.method.upper` with cursor at the end — 3-
        // segment chain + typed "upper" prefix.
        let text = "obj.field.method.upper";
        let ctx = detect_completion_context(text, 0, text.len() as u32).unwrap();
        match ctx {
            CompletionContext::AfterDot { recv_name, .. } => {
                assert_eq!(
                    recv_name, "obj.field.method",
                    "recv_name should be the chain up to before the last `.`"
                );
            }
            other => panic!("expected AfterDot, got {other:?}"),
        }
    }

    #[test]
    fn detect_context_from_import_with_cursor_after_import_keyword() {
        // `from foo import |` → FromImportList with mod_path = ["foo"].
        let text = "from foo import ";
        let ctx = detect_completion_context(text, 0, 16).unwrap();
        match ctx {
            CompletionContext::FromImportList { mod_path } => {
                assert_eq!(mod_path, vec!["foo".to_string()]);
            }
            other => panic!("expected FromImportList, got {other:?}"),
        }
    }

    #[test]
    fn detect_context_from_import_with_previous_items() {
        // `from foo import X, Y, |` → FromImportList same (previous
        // items are skipped walking back-to-front by comma + ident +
        // ws).
        let text = "from foo import X, Y, ";
        let ctx = detect_completion_context(text, 0, 22).unwrap();
        match ctx {
            CompletionContext::FromImportList { mod_path } => {
                assert_eq!(mod_path, vec!["foo".to_string()]);
            }
            other => panic!("expected FromImportList, got {other:?}"),
        }
    }

    #[test]
    fn detect_context_from_import_with_dotted_mod_path() {
        // `from sub.utils import |` → mod_path = ["sub", "utils"].
        // (English unchanged — same form.)
        let text = "from sub.utils import ";
        let ctx = detect_completion_context(text, 0, 22).unwrap();
        match ctx {
            CompletionContext::FromImportList { mod_path } => {
                assert_eq!(mod_path, vec!["sub".to_string(), "utils".to_string()]);
            }
            other => panic!("expected FromImportList, got {other:?}"),
        }
    }

    #[test]
    fn from_import_completions_returns_module_exports() {
        // Setup: tempdir with a main.fitz and a utils.fitz. The
        // helper resolves utils.fitz from main.fitz's URI and lists
        // the module's fns/types/consts.
        let tmp = tempfile::tempdir().expect("tempdir");
        let main_path = tmp.path().join("main.fitz");
        let utils_path = tmp.path().join("utils.fitz");
        std::fs::write(&main_path, "from utils import \n").unwrap();
        std::fs::write(
            &utils_path,
            "fn double(n: Int) -> Int { return n * 2 }\n\
             type User { id: Int, name: Str }\n\
             let MAX: Int = 100\n",
        )
        .unwrap();
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let items = from_import_completions(&main_uri, &["utils".to_string()]);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"double"), "falta fn `double`: {labels:?}");
        assert!(labels.contains(&"User"), "falta type `User`: {labels:?}");
        assert!(labels.contains(&"MAX"), "falta const `MAX`: {labels:?}");
    }

    #[test]
    fn from_import_completions_nonexistent_module_returns_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let main_path = tmp.path().join("main.fitz");
        std::fs::write(&main_path, "from no_existe import \n").unwrap();
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let items = from_import_completions(&main_uri, &["no_existe".to_string()]);
        assert!(items.is_empty());
    }

    #[test]
    fn completion_at_position_without_uri_does_not_complete_from_import() {
        // The `completion_at_position` wrapper (no URI) cannot
        // resolve the target module file — for FromImportList it
        // returns empty. Only the `_with_uri` wrapper covers it.
        // Guarantee: existing tests don't break because they still
        // use the original signature.
        let src = "from foo import \n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 0, 16);
        assert!(
            items.is_empty(),
            "without URI, FromImportList must return empty. Got: {items:?}"
        );
    }

    #[test]
    fn scope_level_completion_includes_top_level_and_builtins_and_keywords() {
        // Cursor at line 3 col 0 — outside any declared stmt,
        // scope-level context.
        let src = "let x = 1\nfn foo() => 0\ntype Bar { id: Int }\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 3, 0);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // Top-level (let, fn, type).
        assert!(labels.contains(&"x"), "falta var top-level `x`: {labels:?}");
        assert!(labels.contains(&"foo"), "falta fn `foo`: {labels:?}");
        assert!(labels.contains(&"Bar"), "falta type `Bar`: {labels:?}");
        // Builtins.
        assert!(labels.contains(&"print"), "falta builtin `print`");
        assert!(labels.contains(&"len"));
        // Built-in types.
        assert!(labels.contains(&"Int"));
        assert!(labels.contains(&"List"));
        // 9.w.2-binary-frames — `WsConn` now appears in scope-level
        // completions (along with List/Map/Result/Future/etc.) so
        // the dev can autocomplete it when writing `@ws` handlers.
        assert!(
            labels.contains(&"WsConn"),
            "falta tipo built-in `WsConn` en scope-level: {labels:?}"
        );
        assert!(labels.contains(&"Bytes"));
        // Keywords.
        assert!(labels.contains(&"let"));
        assert!(labels.contains(&"match"));
    }

    #[test]
    fn after_dot_on_nominal_lists_fields_of_type() {
        // `type Point { x: Int, y: Int }` + `let p = Point { x: 1, y: 2 }`
        // + ident `p` on line 2 col 0 (1-based: line 3, col 1).
        // After-dot on `p.` should list x, y.
        let src = "type Point { x: Int, y: Int }\nlet p = Point { x: 1, y: 2 }\np.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor at line 2, col 2 (0-based LSP), right after the `.`.
        let items = completion_at_position(src, &program, &type_info, &env, 2, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"x"), "falta field `x`: {labels:?}");
        assert!(labels.contains(&"y"), "falta field `y`: {labels:?}");
        // Must not include top-level: we're already in after-dot.
        assert!(
            !labels.contains(&"print"),
            "should not include builtins in after-dot"
        );
        // The kind must be FIELD.
        let item_x = items.iter().find(|i| i.label == "x").unwrap();
        assert_eq!(item_x.kind, Some(CompletionItemKind::FIELD));
    }

    #[test]
    fn after_dot_on_list_lists_built_in_methods() {
        // `let xs = [1, 2, 3]` + `xs.` on line 1.
        let src = "let xs = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["push", "pop", "map", "filter", "find", "len"] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}` of List: {labels:?}"
            );
        }
        let item_map = items.iter().find(|i| i.label == "map").unwrap();
        assert_eq!(item_map.kind, Some(CompletionItemKind::METHOD));
    }

    #[test]
    fn after_dot_on_str_lists_3_methods() {
        // User case typing `obj.` at the end of the buffer: the
        // parser abandons the entire stmt because of the orphan `.`
        // (debt F15 sub-stmt recovery), the Expr::Ident doesn't
        // reach TypeInfo. The "walk top-level by name" fallback
        // resolves the type by looking at the previous
        // `let s = "hola"`.
        let src = "let s = \"hola\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"upper"));
        assert!(labels.contains(&"lower"));
        assert!(labels.contains(&"len"));
        // No List methods.
        assert!(!labels.contains(&"push"));
    }

    // 9.w.2-binary-frames — completions on `WsConn<Bytes>`. The
    // path after `conn.` lists the 4 parametric methods over T =
    // Bytes (recv/send/broadcast/close) with detail clarifying the
    // binary mode (vs. JSON-marshalled text).

    #[test]
    fn after_dot_on_wsconn_bytes_lists_4_methods_binary_mode() {
        // Note: the trailing `\` strips leading whitespace, so line
        // 2 of the real src is `let r = conn.recv()`. We use a valid
        // call (not orphan `conn.`) so the parser doesn't abandon
        // the body and `Expr::Ident(conn)` ends up registered in
        // TypeInfo. Cursor at col 13 falls between `.` and `recv`,
        // triggering AfterDot.
        let src = "@ws(\"/raw\")\n\
                   async fn raw(conn: WsConn<Bytes>) -> Null {\n\
                   let r = conn.recv()\n\
                   return null\n\
                   }";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 2, 13);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["recv", "send", "broadcast", "close"] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}` of WsConn<Bytes>: {labels:?}"
            );
        }
        // `recv` detail must type `Result<Bytes>` and mention the
        // binary mode.
        let recv = items.iter().find(|i| i.label == "recv").expect("recv item");
        let detail = recv.detail.as_deref().unwrap_or("");
        assert!(
            detail.contains("Result<Bytes>"),
            "recv detail should type Result<Bytes>, was: {detail}"
        );
        assert!(
            detail.contains("Binary"),
            "recv detail should mention Binary when T=Bytes, was: {detail}"
        );
        // `send` detail must ask for a Bytes arg and mention raw
        // binary.
        let send = items.iter().find(|i| i.label == "send").unwrap();
        let send_detail = send.detail.as_deref().unwrap_or("");
        assert!(send_detail.contains("msg: Bytes"));
        assert!(send_detail.contains("Binary"));
    }

    #[test]
    fn after_dot_on_wsconn_bidir_recv_send_different_types() {
        // 9.w.2-wsconn-bidir — `WsConn<Str, ChatMsg>`: recv types
        // `Result<Str>`, send expects `msg: ChatMsg`.
        let src = "type ChatMsg { user: Str, text: Str }\n\
                   @ws(\"/c\")\n\
                   async fn c(conn: WsConn<Str, ChatMsg>) -> Null {\n\
                   let r = conn.recv()\n\
                   return null\n\
                   }";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 3, 13);
        let recv = items.iter().find(|i| i.label == "recv").expect("recv item");
        let recv_detail = recv.detail.as_deref().unwrap_or("");
        assert!(
            recv_detail.contains("Result<Str>"),
            "recv detail should type Result<Str> (recv=Str), was: {recv_detail}"
        );
        let send = items.iter().find(|i| i.label == "send").unwrap();
        let send_detail = send.detail.as_deref().unwrap_or("");
        assert!(
            send_detail.contains("msg: ChatMsg"),
            "send detail should require ChatMsg (send), was: {send_detail}"
        );
    }

    #[test]
    fn after_dot_on_wsconn_str_keeps_text_detail() {
        // Sanity: `WsConn<Str>` (historical path) is not
        // contaminated with the binary detail. Same shape as the
        // Bytes test — valid call + cursor between `.` and `recv`.
        let src = "@ws(\"/c\")\n\
                   async fn c(conn: WsConn<Str>) -> Null {\n\
                   let r = conn.recv()\n\
                   return null\n\
                   }";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 2, 13);
        let recv = items.iter().find(|i| i.label == "recv").expect("recv item");
        let detail = recv.detail.as_deref().unwrap_or("");
        assert!(detail.contains("Result<Str>"));
        assert!(
            !detail.contains("Message::Binary"),
            "WsConn<Str>.recv should not mention Binary, was: {detail}"
        );
    }

    #[test]
    fn after_dot_on_typeless_receiver_returns_any_methods() {
        // `desconocido.` — unresolved ident. v0.9.51 F15 sub-stmt
        // recovery: the parser now preserves the stmt as
        // `Expr::Field { object: Ident("desconocido"), field: "" }`
        // (instead of discarding it entirely). The checker types an
        // Ident without binding as `Type::Any` (gradual escape),
        // and the `Type::Any` dispatch returns the 6 universal
        // methods of F13.D
        // (as_int/as_float/as_str/as_bool/as_bytes/type_name).
        // Pre-fix returned empty because the entire stmt was
        // discarded and `TypeInfo` had no entry for the ident.
        let src = "desconocido.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 0, 12);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"as_int") && labels.contains(&"type_name"),
            "expected universal methods of Type::Any (F13.D), got: {:?}",
            labels
        );
    }

    // V.2 mini-batch (VSCode catch-up) — new Str methods
    // (S.1/S.2), List methods (S.3), and tuple field access (T.1).

    #[test]
    fn after_dot_on_str_includes_methods_from_s_mini_batch() {
        // The 7 new methods added in S.1/S.2 must appear in the
        // completion list for `Str` receivers.
        let src = "let s = \"hola\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in [
            "contains",
            "starts_with",
            "ends_with",
            "split",
            "trim",
            "trim_start",
            "trim_end",
            "replace",
            "repeat",
        ] {
            assert!(
                labels.contains(&expected),
                "missing method in Str: `{expected}` (S+Mb): {labels:?}"
            );
        }
        // Sanity: the original 3 still work.
        assert!(labels.contains(&"upper"));
        assert!(labels.contains(&"lower"));
        assert!(labels.contains(&"len"));
    }

    #[test]
    fn after_dot_on_list_includes_sort_reverse_and_contains() {
        // S.3 mini-batch: sort, reverse, contains are added to the
        // canonical List methods list.
        let src = "let xs = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["sort", "reverse", "contains"] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}` (mini-batch S.3) in List: {labels:?}"
            );
        }
        // The detail of `contains` must reflect the element type.
        let item_contains = items.iter().find(|i| i.label == "contains").unwrap();
        assert_eq!(item_contains.detail.as_deref(), Some("fn(Int) -> Bool"));
    }

    #[test]
    fn after_dot_on_list_includes_enumerate_zip_and_chain() {
        // Mini-tanda It: enumerate, zip, chain se suman a List.
        let src = "let xs = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["enumerate", "zip", "chain"] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}` (mini-batch It) in List: {labels:?}"
            );
        }
        // The detail of `enumerate` must reflect the element type.
        let item_enum = items.iter().find(|i| i.label == "enumerate").unwrap();
        assert_eq!(
            item_enum.detail.as_deref(),
            Some("fn() -> List<(Int, Int)>")
        );
    }

    #[test]
    fn up_after_dot_map_includes_update() {
        let src = "let m = {\"a\": 1}\nm.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"update"),
            "falta `update` (mini-tanda Up): {labels:?}"
        );
    }

    #[test]
    fn up_after_dot_nominal_shows_param_names_in_signature() {
        // Up mini-batch: the signature of a custom method must show
        // `fn(x: Int, y: Int)` instead of `fn(Int, Int)`.
        let src = "type Point {\n\
                       x: Int = 0\n\
                       y: Int = 0\n\n\
                       fn distance_to(other_x: Int, other_y: Int) -> Int {\n\
                           return (x - other_x) + (y - other_y)\n\
                       }\n\
                   }\n\
                   let p = Point { x: 1, y: 2 }\n\
                   p.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor right after the last `.`.
        let lines: Vec<&str> = src.split('\n').collect();
        let last_line = lines.len() as u32 - 2; // -2: discount the final empty line
        let items = completion_at_position(src, &program, &type_info, &env, last_line, 2);
        let m = items
            .iter()
            .find(|i| i.label == "distance_to")
            .expect("falta distance_to");
        let detail = m.detail.as_deref().unwrap_or("");
        assert!(
            detail.contains("other_x: Int") && detail.contains("other_y: Int"),
            "expected signature with param names, was: {detail:?}"
        );
    }

    #[test]
    fn ex2_after_dot_list_includes_flat_map_first_last() {
        let src = "let xs = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["flat_map", "first", "last"] {
            assert!(
                labels.contains(&expected),
                "falta `{expected}` (mini-tanda Ex2): {labels:?}"
            );
        }
    }

    #[test]
    fn ex2_after_dot_map_includes_merge() {
        let src = "let m = {\"a\": 1}\nm.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"merge"),
            "falta `merge` (mini-tanda Ex2): {labels:?}"
        );
    }

    #[test]
    fn ex_after_dot_str_includes_search_methods() {
        let src = "let s = \"hi\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["find", "index_of", "last_index_of"] {
            assert!(
                labels.contains(&expected),
                "falta `{expected}` (mini-tanda Ex): {labels:?}"
            );
        }
    }

    #[test]
    fn ex_after_dot_map_includes_filter_and_map_values() {
        let src = "let m = {\"a\": 1}\nm.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["filter", "map_values"] {
            assert!(
                labels.contains(&expected),
                "falta `{expected}` (mini-tanda Ex): {labels:?}"
            );
        }
    }

    #[test]
    fn vm_after_dot_hides_private_methods() {
        // Vm mini-batch: `_method` methods do NOT appear in `instance.`.
        let src = "type C {\n\
                       fn greet() -> Str { return \"hi\" }\n\
                       fn _hidden() -> Str { return \"x\" }\n\
                   }\n\
                   let c = C {}\n\
                   c.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor at line 5 col 2 (0-based), right after the `.`.
        let items = completion_at_position(src, &program, &type_info, &env, 5, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"greet"),
            "expected `greet` (public), got: {labels:?}"
        );
        assert!(
            !labels.contains(&"_hidden"),
            "method `_hidden` (private) should NOT appear, got: {labels:?}"
        );
    }

    #[test]
    fn vp_after_dot_hides_private_fields() {
        // Vp mini-batch: `_field` fields do NOT appear in
        // `instance.` — they are a private convention and only
        // accessible from methods of the same type.
        let src = "type C { name: Str = \"\", _balance: Int = 0 }\n\
                   let c = C {}\n\
                   c.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 2, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"name"),
            "expected `name` (public) in completion, got: {labels:?}"
        );
        assert!(
            !labels.contains(&"_balance"),
            "field `_balance` (private) should NOT appear in completion, got: {labels:?}"
        );
    }

    #[test]
    fn after_dot_on_list_includes_any_all_count_find_index() {
        // Lx mini-batch: 4 functional predicates on List.
        let src = "let xs = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["any", "all", "count", "find_index"] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}` (mini-batch Lx): {labels:?}"
            );
        }
        let count = items.iter().find(|i| i.label == "count").unwrap();
        assert!(
            count.detail.as_deref().unwrap_or("").contains("-> Int"),
            "expected signature with `-> Int`, got: {:?}",
            count.detail
        );
    }

    #[test]
    fn after_dot_on_list_includes_flatten_and_sort_by() {
        // Mb mini-batch: flatten + sort_by are added to List.
        let src = "let xs = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["flatten", "sort_by"] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}` (mini-batch Mb) in List: {labels:?}"
            );
        }
        let item_sort_by = items.iter().find(|i| i.label == "sort_by").unwrap();
        assert!(
            item_sort_by
                .detail
                .as_deref()
                .unwrap_or("")
                .contains("fn(Int, Int)"),
            "expected signature with `fn(Int, Int)`, got: {:?}",
            item_sort_by.detail
        );
    }

    #[test]
    fn after_dot_on_range_lists_iterators_and_len() {
        // Ir mini-batch: after `r.` on a Range, we suggest
        // enumerate/zip/chain/len (the subset that makes sense for
        // a numeric iterable).
        let src = "let r = 0..10\nr.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["enumerate", "zip", "chain", "len"] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}` (mini-batch Ir) in Range: {labels:?}"
            );
        }
        let item_enum = items.iter().find(|i| i.label == "enumerate").unwrap();
        assert_eq!(
            item_enum.detail.as_deref(),
            Some("fn() -> List<(Int, Int)>")
        );
    }

    #[test]
    fn mb2_after_dot_on_list_includes_min_max_sum() {
        // Mb2 mini-batch: List adds 3 numeric methods.
        let src = "let xs: List<Int> = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["min", "max", "sum"] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}` (mini-batch Mb2) in List: {labels:?}"
            );
        }
    }

    #[test]
    fn mb2_after_dot_on_str_includes_pad_start_and_pad_end() {
        let src = "let s: Str = \"x\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["pad_start", "pad_end"] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}` (mini-batch Mb2) in Str: {labels:?}"
            );
        }
        let pad_start = items.iter().find(|i| i.label == "pad_start").unwrap();
        assert_eq!(
            pad_start.detail.as_deref(),
            Some("fn(width: Int, ch: Str) -> Str"),
        );
    }

    #[test]
    fn mb2_after_dot_on_map_includes_keys_sorted() {
        let src = "let m: Map<Str, Int> = {\"a\": 1}\nm.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"keys_sorted"),
            "falta `keys_sorted` (mini-tanda Mb2) en Map: {labels:?}",
        );
    }

    #[test]
    fn rg_after_dot_on_range_includes_step_by() {
        let src = "let r = 0..10\nr.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"step_by"),
            "falta `step_by` (mini-tanda Rg) en Range: {labels:?}",
        );
        let step_by = items.iter().find(|i| i.label == "step_by").unwrap();
        assert_eq!(step_by.detail.as_deref(), Some("fn(n: Int) -> List<Int>"));
    }

    #[test]
    fn mb3_after_dot_on_list_includes_reduce_product_to_map() {
        // Mini-tanda Mb3: List suma reduce/product/to_map.
        let src = "let xs: List<Int> = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["reduce", "product", "to_map"] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}` (mini-batch Mb3) in List: {labels:?}"
            );
        }
    }

    #[test]
    fn mb3_after_dot_on_str_includes_chars() {
        let src = "let s: Str = \"abc\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"chars"),
            "falta `chars` (mini-tanda Mb3) en Str: {labels:?}",
        );
        let chars = items.iter().find(|i| i.label == "chars").unwrap();
        assert_eq!(chars.detail.as_deref(), Some("fn() -> List<Str>"));
    }

    #[test]
    fn mb3_after_dot_on_map_includes_entries() {
        let src = "let m: Map<Str, Int> = {\"a\": 1}\nm.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"entries"),
            "falta `entries` (mini-tanda Mb3) en Map: {labels:?}",
        );
        let entries = items.iter().find(|i| i.label == "entries").unwrap();
        assert_eq!(entries.detail.as_deref(), Some("fn() -> List<(Str, Int)>"));
    }

    #[test]
    fn mb4_after_dot_on_list_includes_unique_and_partition() {
        let src = "let xs: List<Int> = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["unique", "partition"] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}` (mini-batch Mb4) in List: {labels:?}"
            );
        }
    }

    #[test]
    fn mb4_after_dot_on_map_includes_invert() {
        let src = "let m: Map<Int, Str> = {1: \"a\"}\nm.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"invert"),
            "falta `invert` (mini-tanda Mb4) en Map: {labels:?}",
        );
        let invert = items.iter().find(|i| i.label == "invert").unwrap();
        assert_eq!(invert.detail.as_deref(), Some("fn() -> Map<Str, Int>"));
    }

    #[test]
    fn mb4_after_dot_on_str_includes_split_at() {
        let src = "let s: Str = \"abc\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"split_at"),
            "falta `split_at` (mini-tanda Mb4) en Str: {labels:?}",
        );
    }

    #[test]
    fn mb5_after_dot_on_list_includes_group_by_zip_with_max_min_by() {
        let src = "let xs: List<Int> = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["group_by", "zip_with", "max_by", "min_by"] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}` (mini-batch Mb5) in List: {labels:?}"
            );
        }
    }

    #[test]
    fn mb6_after_dot_on_list_includes_scan_and_windows() {
        let src = "let xs: List<Int> = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["scan", "windows"] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}` (mini-batch Mb6) in List: {labels:?}"
            );
        }
    }

    #[test]
    fn mb8_after_dot_on_list_includes_starts_ends_with_insert_remove_at_zip_to_map() {
        let src = "let xs: List<Int> = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in [
            "starts_with",
            "ends_with",
            "insert_at",
            "remove_at",
            "zip_to_map",
        ] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}` (mini-batch Mb8) in List: {labels:?}"
            );
        }
    }

    #[test]
    fn mb8_after_dot_on_str_includes_left_right_center() {
        let src = "let s: Str = \"x\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["left", "right", "center"] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}` (mini-batch Mb8) in Str: {labels:?}"
            );
        }
    }

    #[test]
    fn mb7_after_dot_on_list_includes_take_drop_init_tail_intersperse_cycle() {
        let src = "let xs: List<Int> = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["take", "drop", "init", "tail", "intersperse", "cycle"] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}` (mini-batch Mb7) in List: {labels:?}"
            );
        }
    }

    #[test]
    fn mb7_after_dot_on_str_includes_repeat_with() {
        let src = "let s: Str = \"x\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"repeat_with"),
            "falta `repeat_with` (mini-tanda Mb7) en Str: {labels:?}",
        );
    }

    #[test]
    fn mb7_after_dot_on_map_includes_with() {
        let src = "let m: Map<Str, Int> = {\"a\": 1}\nm.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"with"),
            "falta `with` (mini-tanda Mb7) en Map: {labels:?}",
        );
    }

    #[test]
    fn mb6_after_dot_on_map_includes_merge_with() {
        let src = "let m: Map<Str, Int> = {\"a\": 1}\nm.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"merge_with"),
            "falta `merge_with` (mini-tanda Mb6) en Map: {labels:?}",
        );
    }

    #[test]
    fn mb5_after_dot_on_str_includes_lines_and_is_empty() {
        let src = "let s: Str = \"abc\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["lines", "is_empty"] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}` (mini-batch Mb5) in Str: {labels:?}"
            );
        }
    }

    #[test]
    fn after_dot_on_tuple_lists_numeric_indices_with_type() {
        // T.1 mini-batch: after `t.` we suggest `0`, `1`, ... as
        // labels, with the field's type in `detail`.
        let src = "let t = (1, \"x\", true)\nt.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["0", "1", "2"],
            "expected labels 0/1/2, got {labels:?}"
        );
        // Each item is FIELD with detail = element type.
        let it0 = &items[0];
        let it1 = &items[1];
        let it2 = &items[2];
        assert_eq!(it0.kind, Some(CompletionItemKind::FIELD));
        assert_eq!(it0.detail.as_deref(), Some("Int"));
        assert_eq!(it1.detail.as_deref(), Some("Str"));
        assert_eq!(it2.detail.as_deref(), Some("Bool"));
    }

    #[test]
    fn after_dot_on_nominal_includes_custom_methods_r3() {
        // V.5 + R.3 mini-batch: besides fields, the custom methods
        // of the type appear in the list with METHOD kind and detail
        // showing the signature. Test covers the 3 cases: method
        // without args, with args, and async fn.
        let src = "type User {\n    id: Int\n    name: Str\n\n    fn greet() -> Str {\n        return \"hi\"\n    }\n\n    fn double(n: Int) -> Int {\n        return n * 2\n    }\n\n    async fn fetch() -> Result<Str> {\n        return Ok(\"x\")\n    }\n}\nlet u = User { id: 1, name: \"Ada\" }\nu.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Line count (0-based, one entry per `\n`):
        //   0: type User {
        //   1:     id: Int
        //   2:     name: Str
        //   3: (blank)
        //   4:     fn greet() -> Str {
        //   5:         return "hi"
        //   6:     }
        //   7: (blank)
        //   8:     fn double(n: Int) -> Int {
        //   9:         return n * 2
        //  10:     }
        //  11: (blank)
        //  12:     async fn fetch() -> Result<Str> {
        //  13:         return Ok("x")
        //  14:     }
        //  15: }
        //  16: let u = User { id: 1, name: "Ada" }
        //  17: u.
        let items = completion_at_position(src, &program, &type_info, &env, 17, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // Fields (inherited from the original case).
        assert!(labels.contains(&"id"), "falta field `id`: {labels:?}");
        assert!(labels.contains(&"name"), "falta field `name`: {labels:?}");
        // Custom methods (R.3 / V.5).
        assert!(
            labels.contains(&"greet"),
            "missing method `greet`: {labels:?}"
        );
        assert!(
            labels.contains(&"double"),
            "missing method `double`: {labels:?}"
        );
        assert!(
            labels.contains(&"fetch"),
            "missing async method `fetch`: {labels:?}"
        );
        // Kind: fields as FIELD, methods as METHOD.
        let it_id = items.iter().find(|i| i.label == "id").unwrap();
        let it_greet = items.iter().find(|i| i.label == "greet").unwrap();
        let it_double = items.iter().find(|i| i.label == "double").unwrap();
        let it_fetch = items.iter().find(|i| i.label == "fetch").unwrap();
        assert_eq!(it_id.kind, Some(CompletionItemKind::FIELD));
        assert_eq!(it_greet.kind, Some(CompletionItemKind::METHOD));
        assert_eq!(it_double.kind, Some(CompletionItemKind::METHOD));
        assert_eq!(it_fetch.kind, Some(CompletionItemKind::METHOD));
        // Detail: signature with `fn` or `async fn` prefix and
        // param types.
        assert_eq!(it_greet.detail.as_deref(), Some("fn() -> Str"));
        assert_eq!(it_double.detail.as_deref(), Some("fn(n: Int) -> Int"));
        assert_eq!(
            it_fetch.detail.as_deref(),
            Some("async fn() -> Result<Str>")
        );
    }

    // ---- Math + Mb9 mini-batch + Int/Float methods ----

    #[test]
    fn mb9_after_dot_on_str_includes_swap_case_title_is_alpha_is_digit_is_numeric() {
        let src = "let s: Str = \"x\"\ns.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["swap_case", "title", "is_alpha", "is_digit", "is_numeric"] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}` (Mb9) in Str: {labels:?}"
            );
        }
    }

    #[test]
    fn mb9_after_dot_on_list_includes_split_at() {
        let src = "let xs: List<Int> = [1, 2, 3]\nxs.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 3);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"split_at"),
            "falta `split_at` en List: {labels:?}"
        );
    }

    #[test]
    fn mb9_after_dot_on_map_includes_has_value() {
        let src = "let m: Map<Str, Int> = {\"a\": 1}\nm.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"has_value"),
            "falta `has_value` en Map: {labels:?}"
        );
    }

    #[test]
    fn after_dot_on_int_includes_abs_to_str_to_str_base() {
        let src = "let n: Int = 5\nn.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["abs", "to_str", "to_str_base"] {
            assert!(
                labels.contains(&expected),
                "falta `{expected}` en Int: {labels:?}"
            );
        }
    }

    #[test]
    fn after_dot_on_float_includes_abs_to_str_is_nan_is_finite() {
        let src = "let x: Float = 3.14\nx.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["abs", "to_str", "is_nan", "is_finite"] {
            assert!(
                labels.contains(&expected),
                "falta `{expected}` en Float: {labels:?}"
            );
        }
    }

    // ---- LSPy mini-batch — exact Range + scope-aware autocomplete ----

    #[test]
    fn lspy_ident_range_at_position_returns_ident_run() {
        let src = "let foo_bar = 42";
        // Cursor in the middle of the ident "foo_bar" (col 6 = `o`
        // of "foo").
        let range = ident_range_at_position(src, 0, 6).expect("should resolve");
        assert_eq!(range.start, Position::new(0, 4)); // start of "foo_bar"
        assert_eq!(range.end, Position::new(0, 11)); // end of "foo_bar"
    }

    #[test]
    fn lspy_ident_range_at_position_returns_none_if_no_ident() {
        let src = "let x = 42";
        // Cursor on `=` (col 6).
        assert!(ident_range_at_position(src, 0, 6).is_none());
    }

    #[test]
    fn lspy_ident_range_from_def_skips_let_keyword() {
        let src = "let foo = 42";
        // def_span points to "let" (col 1 = "l"). The helper must
        // skip "let " and return the range of "foo".
        let span = Span::new(1, 1);
        let range = ident_range_from_def(src, span).expect("should resolve");
        assert_eq!(range.start, Position::new(0, 4)); // start of "foo"
        assert_eq!(range.end, Position::new(0, 7)); // end of "foo"
    }

    #[test]
    fn lspy_ident_range_from_def_skips_fn_keyword() {
        let src = "fn greet(name: Str) -> Str { return name }";
        let span = Span::new(1, 1);
        let range = ident_range_from_def(src, span).expect("should resolve");
        assert_eq!(range.start, Position::new(0, 3)); // start of "greet"
        assert_eq!(range.end, Position::new(0, 8)); // end of "greet"
    }

    #[test]
    fn lspy_make_hover_with_range_includes_ident_range() {
        let src = "let count = 42\n";
        let ty = Type::Int;
        let env = TypeEnv::new();
        // v0.10.32 (Tier D.2) — `make_hover_with_range` now takes
        // `program: &Program` to augment the hover with CREATE
        // TABLE SQL when the type is a `@table`. For this Range
        // test we pass an empty program: ty is Int (not Nominal),
        // so the augment is silently skipped and only the Range is
        // validated.
        let empty_program: crate::ast::Program = Vec::new();
        // Cursor at col 6 (middle of "count" — "let " = 4 chars +
        // "c" + "o").
        let hover = make_hover_with_range(&ty, &env, &empty_program, src, 0, 6);
        assert!(hover.range.is_some(), "expected Range, was None");
        let r = hover.range.unwrap();
        assert_eq!(r.start, Position::new(0, 4)); // start of "count"
        assert_eq!(r.end, Position::new(0, 9)); // end of "count"
    }

    #[test]
    fn lspy_diagnostics_with_source_extends_range_to_ident() {
        let src = "let xyz = unknown_var\n";
        // Build a synthetic FitzError pointing to "unknown_var" (col 11).
        let err = FitzError::new(
            crate::error::ErrorKind::TypeError,
            1,
            11,
            "variable no definida: unknown_var",
        );
        let diagnostics = fitz_errors_to_diagnostics_with_source(&[err], src);
        assert_eq!(diagnostics.len(), 1);
        let r = diagnostics[0].range;
        assert_eq!(r.start, Position::new(0, 10));
        assert_eq!(r.end, Position::new(0, 21)); // 10 + len("unknown_var") = 21
    }

    #[test]
    fn lspy_scope_aware_completion_includes_fn_params() {
        let src = "fn greet(name: Str, age: Int) -> Str {\n    \n    return name\n}\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor on line 2 (inside greet's body). LSP uses 0-based.
        let items = completion_at_position(src, &program, &type_info, &env, 1, 4);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"name"),
            "expected param `name`: {labels:?}"
        );
        assert!(labels.contains(&"age"), "expected param `age`: {labels:?}");
    }

    #[test]
    fn lspy_scope_aware_completion_includes_let_locals() {
        let src = "fn f() -> Int {\n    let mi_var: Int = 5\n    \n    return mi_var\n}\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor on line 3 (after the let).
        let items = completion_at_position(src, &program, &type_info, &env, 2, 4);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"mi_var"),
            "expected local `mi_var`: {labels:?}"
        );
    }

    #[test]
    fn lspy_scope_aware_completion_excludes_let_locals_defined_after() {
        // A `let` on line 3 must NOT appear if the cursor is on
        // line 2 (forward references are not allowed).
        let src = "fn f() -> Int {\n    \n    let posterior: Int = 5\n    return 0\n}\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor on line 2.
        let items = completion_at_position(src, &program, &type_info, &env, 1, 4);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            !labels.contains(&"posterior"),
            "should not include later let: {labels:?}"
        );
    }

    #[test]
    fn lspy_scope_aware_completion_includes_for_var() {
        // Source on a single line to avoid parser recovery issues
        // over blank lines / orphan `}`.
        let src = "fn f() -> Int {\n    for item in [1, 2, 3] {\n        let y: Int = item\n    }\n    return 0\n}\n";
        let (program, env, type_info, _defs, errs) = check_source_with_types(src);
        // We check parsing was clean (no Error nodes).
        assert!(
            !program.iter().any(|s| matches!(s, Stmt::Error(_))),
            "parser emitted Error nodes: {errs:?}"
        );
        // Cursor inside the for body (line 3, in the let).
        let items = completion_at_position(src, &program, &type_info, &env, 2, 10);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"item"),
            "expected `item` from for: {labels:?}"
        );
    }

    // ---- LSPx mini-batch — cross-module go-to-definition ----

    #[test]
    fn lspx_cross_module_resolves_from_import() {
        // Setup: two temporary files in a single tmpdir.
        // `foo.fitz` declares `type User { ... }` and a const.
        // `app.fitz` does `from foo import User`. We check that
        // `resolve_cross_module_definition` points at the span of
        // the actual decl inside foo.fitz.
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("fitz-lspx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let foo_path = dir.join("foo.fitz");
        let app_path = dir.join("app.fitz");
        let mut f = std::fs::File::create(&foo_path).unwrap();
        writeln!(f, "type User {{ id: Int, name: Str }}").unwrap();
        writeln!(f, "let CAP: Int = 100").unwrap();
        drop(f);
        let mut a = std::fs::File::create(&app_path).unwrap();
        writeln!(a, "from foo import User, CAP").unwrap();
        writeln!(a, "let u = User {{ id: 1, name: \"x\" }}").unwrap();
        drop(a);

        let app_src = std::fs::read_to_string(&app_path).unwrap();
        let (program, _env, _ti, _di, _errs) = check_source_with_types(&app_src);
        let doc_uri = Url::from_file_path(&app_path).unwrap();

        // Look up the FromImport span (line 1 col 1).
        let import_span = program
            .iter()
            .find_map(|s| match s {
                Stmt::FromImport { span, .. } => Some(*span),
                _ => None,
            })
            .expect("should have FromImport");

        // Resolve `User`: must point at foo.fitz line 1.
        let resolved = resolve_cross_module_definition(&program, &doc_uri, import_span, "User")
            .expect("expected cross-module resolution");
        let (target_uri, target_span) = resolved;
        // target_uri is the file:// of the canonicalized foo.fitz.
        let target_path = target_uri.to_file_path().unwrap();
        assert_eq!(
            target_path.canonicalize().unwrap(),
            foo_path.canonicalize().unwrap(),
            "expected target_uri = foo.fitz, got: {:?}",
            target_path
        );
        assert_eq!(
            target_span.line, 1,
            "expected line 1 (type User), got: {}",
            target_span.line
        );

        // Resolve `CAP`: line 2 (let CAP = 100).
        let resolved_cap = resolve_cross_module_definition(&program, &doc_uri, import_span, "CAP")
            .expect("expected resolution of CAP");
        assert_eq!(
            resolved_cap.1.line, 2,
            "expected line 2 (let CAP), got: {}",
            resolved_cap.1.line
        );

        // Cleanup.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lspx_cross_module_nonexistent_name_returns_none() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("fitz-lspx-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let foo_path = dir.join("foo.fitz");
        let app_path = dir.join("app.fitz");
        std::fs::write(&foo_path, "type User { id: Int }").unwrap();
        let mut a = std::fs::File::create(&app_path).unwrap();
        writeln!(a, "from foo import User").unwrap();
        drop(a);
        let app_src = std::fs::read_to_string(&app_path).unwrap();
        let (program, _env, _ti, _di, _errs) = check_source_with_types(&app_src);
        let doc_uri = Url::from_file_path(&app_path).unwrap();
        let import_span = program
            .iter()
            .find_map(|s| match s {
                Stmt::FromImport { span, .. } => Some(*span),
                _ => None,
            })
            .unwrap();
        // `NotImported` is not in the import list → None.
        let resolved =
            resolve_cross_module_definition(&program, &doc_uri, import_span, "NotImported");
        assert!(resolved.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fp_scope_level_fn_with_default_includes_signature_and_default_in_detail() {
        let src = "fn greet(name: Str = \"amigo\") -> Str { return name }\n\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 0);
        let it = items
            .iter()
            .find(|i| i.label == "greet")
            .expect("falta greet");
        let detail = it.detail.as_deref().unwrap_or("");
        assert!(
            detail.contains("name: Str = \"amigo\""),
            "expected `name: Str = \"amigo\"` in detail, was: {}",
            detail
        );
        assert!(
            detail.contains("-> Str"),
            "expected `-> Str` in detail, was: {}",
            detail
        );
    }

    #[test]
    fn scope_level_includes_math_builtins() {
        let src = "let a = 1\n\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 0);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in [
            "abs", "min", "max", "pow", "sqrt", "ceil", "floor", "round", "clamp",
        ] {
            assert!(
                labels.contains(&expected),
                "falta builtin `{expected}`: {labels:?}"
            );
        }
    }

    // Phase 10 — LSP ORM/DB completions.

    #[test]
    fn scope_level_includes_db_module_and_dbconn_dbrow_types() {
        let src = "let a = 1\n\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 0);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // The db module appears as MODULE.
        let db_item = items
            .iter()
            .find(|i| i.label == "db")
            .expect("missing module `db`");
        assert_eq!(db_item.kind, Some(CompletionItemKind::MODULE));
        // Built-in types DbConn and DbRow appear as CLASS.
        for t in ["DbConn", "DbRow"] {
            assert!(labels.contains(&t), "falta tipo built-in `{t}`: {labels:?}");
        }
    }

    #[test]
    fn after_dot_on_db_lists_connect() {
        let src = "let x = db.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor right after the dot: line 0, col 11.
        let items = completion_at_position(src, &program, &type_info, &env, 0, 11);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"connect"), "falta `connect`: {labels:?}");
        // Does not include `decode`/`encode` (those would be jwt's)
        // — confirms the dispatch by receiver name.
        assert!(
            !labels.contains(&"encode"),
            "should not include jwt.encode: {labels:?}"
        );
    }

    #[test]
    fn after_dot_on_dbconn_lists_query_exec_close() {
        // Direct dispatch test `Type::DbConn` → query/exec/close.
        // Same pattern as `after_dot_sobre_wsconn_*`: we use a
        // complete call `conn.close()` so the parser doesn't
        // abandon the stmt and Expr::Ident(conn) remains in
        // TypeInfo. Cursor between `.` and the method triggers
        // AfterDot.
        let src =
            "async fn run(conn: DbConn) -> Null {\n  let _ = conn.close()\n  return null\n}\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor at line 1, col 15 (right after `conn.`).
        let items = completion_at_position(src, &program, &type_info, &env, 1, 15);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["query", "exec", "close", "is_closed", "transaction"] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}`: {labels:?}"
            );
        }
    }

    #[test]
    fn after_dot_on_dbrow_lists_get_int_get_str_get_float_get_bool_len() {
        // v0.10.22 — direct dispatch `Type::DbRow` → typed
        // extraction methods (get_int/get_str/get_float/get_bool) +
        // len. Pattern: param `r: DbRow` + complete call `r.len()`
        // so the parser doesn't abandon the stmt and Ident(r)
        // remains in TypeInfo.
        let src = "fn run(r: DbRow) -> Int {\n  return r.len()\n}\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor at line 1, col 11 (right after `r.`).
        let items = completion_at_position(src, &program, &type_info, &env, 1, 11);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["get_int", "get_str", "get_float", "get_bool", "len"] {
            assert!(
                labels.contains(&expected),
                "missing method `{expected}` on DbRow: {labels:?}"
            );
        }
    }

    #[test]
    fn after_dot_on_type_with_table_lists_orm_statics() {
        let src = "@table(\"users\") type User {\n  @primary\n  id: Int\n  name: Str\n}\nUser.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor after `User.` at line 5, col 5.
        let items = completion_at_position(src, &program, &type_info, &env, 5, 5);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["all", "where", "first", "count", "insert", "bulk_insert"] {
            assert!(
                labels.contains(&expected),
                "missing ORM static `{expected}`: {labels:?}"
            );
        }
        // The detail of `all` must mention `User` (the concrete type).
        let all_item = items.iter().find(|i| i.label == "all").unwrap();
        let detail = all_item.detail.as_deref().unwrap_or("");
        assert!(
            detail.contains("User"),
            "expected `User` in detail of all, was: {}",
            detail
        );
    }

    #[test]
    fn after_dot_on_query_builder_lists_chain_and_terminals() {
        // Phase 10.3+ — the QueryBuilder types as
        // `Type::QueryBuilder<Row>` and after-dot lists the chain
        // methods + terminals. Simple test: `let qb = User.where(...)`
        // separates the binding, then `qb.<cursor>` triggers the
        // clean TypeInfo heuristic (qb is a top-level reference).
        // If the dispatch works, all QB methods are in the result.
        let src = "@table(\"users\") type User {\n  @primary\n  id: Int\n  age: Int\n}\nasync fn run(db: DbConn) -> Result<List<User>> {\n  let qb = User.where(fn(u) => u.age > 18)\n  let _r = qb.all(db).await?\n  return Ok([])\n}\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Line 7 (0-based) content: `  let _r = qb.all(db).await?`
        // The `.` between `qb` and `all` is at col 14; cursor at
        // col 15 (right after the dot).
        let items = completion_at_position(src, &program, &type_info, &env, 7, 15);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // If QB completions are working, these must be present:
        for expected in [
            "where", "order_by", "limit", "offset", "all", "first", "count",
        ] {
            assert!(
                labels.contains(&expected),
                "missing QB method `{expected}`: {labels:?}"
            );
        }
        // The detail of `all` must mention the row type `User`.
        let all_item = items.iter().find(|i| i.label == "all").unwrap();
        let detail = all_item.detail.as_deref().unwrap_or("");
        assert!(
            detail.contains("User"),
            "expected `User` in detail of all, was: {}",
            detail
        );
    }

    #[test]
    fn after_dot_on_type_without_table_does_not_list_orm_statics() {
        // A type without @table must NOT offer all/where/insert.
        let src = "type Plain {\n  id: Int\n}\nPlain.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 3, 6);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // These must not appear.
        assert!(
            !labels.contains(&"all"),
            "Plain without @table should not have `all`: {labels:?}"
        );
        assert!(
            !labels.contains(&"where"),
            "Plain without @table should not have `where`: {labels:?}"
        );
    }

    // -----------------------------------------------------------------
    // Mini-fase HTTP client (2026-06-18, Bloque 4 — LSP).
    // -----------------------------------------------------------------

    #[test]
    fn http_module_appears_in_scope_level_completions() {
        // The `http` module is pre-registered as MODULE in
        // scope_level_completions (parallel to jwt/hash/auth/db/log/
        // flags). HttpClientResponse is pre-registered as a built-in
        // CLASS in TypeEnv (Bloque 2), so it also appears.
        let src = "let a = 1\n\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 0);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        let http_item = items
            .iter()
            .find(|i| i.label == "http")
            .expect("missing module `http`");
        assert_eq!(http_item.kind, Some(CompletionItemKind::MODULE));
        let detail = http_item.detail.as_deref().unwrap_or("");
        assert!(
            detail.contains("HTTP client outbound"),
            "expected `HTTP client outbound` in detail of http, was: {detail}",
        );
        // HttpClientResponse pre-registered as built-in nominal — appears
        // as CLASS in scope-level completions.
        assert!(
            labels.contains(&"HttpClientResponse"),
            "missing built-in type `HttpClientResponse`: {labels:?}"
        );
    }

    #[test]
    fn after_dot_on_sql_lists_now_and_raw() {
        // O1/O3 (v0.32.0) — `sql.` lists `now`/`raw` (ORM SQL helpers).
        let src = "let x = sql.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor right after the dot: line 0, col 12.
        let items = completion_at_position(src, &program, &type_info, &env, 0, 12);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"now"),
            "missing sql method `now`: {labels:?}"
        );
        assert!(
            labels.contains(&"raw"),
            "missing sql method `raw`: {labels:?}"
        );
        let now_item = items.iter().find(|i| i.label == "now").unwrap();
        assert_eq!(now_item.kind, Some(CompletionItemKind::METHOD));
    }

    #[test]
    fn after_dot_on_http_lists_six_methods_with_signatures() {
        // Parallel to `after_dot_on_db_lists_connect`: the dispatch
        // by receiver name fires before touching TypeInfo (the module
        // types as `Any` in the checker, MVP).
        let src = "let x = http.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor right after the dot: line 0, col 13.
        let items = completion_at_position(src, &program, &type_info, &env, 0, 13);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["get", "head", "post", "put", "delete", "request"] {
            assert!(
                labels.contains(&expected),
                "missing http method `{expected}`: {labels:?}"
            );
        }
        // METHOD kind on all 6 (method_items factory).
        let get_item = items.iter().find(|i| i.label == "get").unwrap();
        assert_eq!(get_item.kind, Some(CompletionItemKind::METHOD));
        // Detail must include the signature hint with the return type.
        let get_detail = get_item.detail.as_deref().unwrap_or("");
        assert!(
            get_detail.contains("Future<Result<HttpClientResponse>>"),
            "expected `Future<Result<HttpClientResponse>>` in detail of get, was: {get_detail}",
        );
        // post mentions the body shape (Str|Map|Bytes).
        let post_detail = items
            .iter()
            .find(|i| i.label == "post")
            .unwrap()
            .detail
            .as_deref()
            .unwrap_or("");
        assert!(
            post_detail.contains("body: Str|Map|Bytes"),
            "expected `body: Str|Map|Bytes` in detail of post, was: {post_detail}",
        );
        // request takes opts: Map.
        let request_detail = items
            .iter()
            .find(|i| i.label == "request")
            .unwrap()
            .detail
            .as_deref()
            .unwrap_or("");
        assert!(
            request_detail.contains("opts: Map"),
            "expected `opts: Map` in detail of request, was: {request_detail}",
        );
        // Does not include neighboring modules' methods (proves the
        // dispatch by receiver name is exact).
        assert!(
            !labels.contains(&"encode"),
            "should not include jwt.encode: {labels:?}"
        );
        assert!(
            !labels.contains(&"connect"),
            "should not include db.connect: {labels:?}"
        );
    }

    #[test]
    fn after_dot_on_http_client_response_lists_four_fields() {
        // `HttpClientResponse` is pre-registered as Nominal in TypeEnv
        // (Bloque 2, types.rs::register_builtin_nominals). With an
        // annotated param `r: HttpClientResponse`, after-dot on `r.`
        // falls through the Nominal dispatch and lists fields. Pattern
        // analogous to `after_dot_on_nominal_lists_fields_of_type`.
        let src = "fn show(r: HttpClientResponse) -> Int {\n  return r.status\n}\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor right after `r.` at line 1, col 11 (1-based: line 2,
        // col 11). The completion engine reads 0-based LSP coords.
        let items = completion_at_position(src, &program, &type_info, &env, 1, 11);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["status", "body", "headers", "duration_ms"] {
            assert!(
                labels.contains(&expected),
                "missing HttpClientResponse field `{expected}`: {labels:?}"
            );
        }
        let status_item = items.iter().find(|i| i.label == "status").unwrap();
        assert_eq!(status_item.kind, Some(CompletionItemKind::FIELD));
    }

    // ---- Mini-tanda SMTP builtin (2026-06-19): Bloque 4 — LSP ----

    #[test]
    fn after_dot_on_smtp_lists_send_with_signature() {
        // Paralelo a `after_dot_on_http_lists_six_methods`: dispatch
        // por nombre de receiver matchea antes de tocar TypeInfo.
        let src = "let x = smtp.\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor right after the dot: line 0, col 13.
        let items = completion_at_position(src, &program, &type_info, &env, 0, 13);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"send"),
            "missing smtp method `send`: {labels:?}"
        );
        let send_item = items.iter().find(|i| i.label == "send").unwrap();
        assert_eq!(send_item.kind, Some(CompletionItemKind::METHOD));
        let send_detail = send_item.detail.as_deref().unwrap_or("");
        assert!(
            send_detail.contains("Future<Result<SmtpResult>>"),
            "expected `Future<Result<SmtpResult>>` in detail of send, was: {send_detail}",
        );
        assert!(
            send_detail.contains("opts: Map"),
            "expected `opts: Map` in detail of send, was: {send_detail}",
        );
        // No bleed from neighboring modules.
        assert!(
            !labels.contains(&"get"),
            "should not include http.get: {labels:?}"
        );
        assert!(
            !labels.contains(&"encode"),
            "should not include jwt.encode: {labels:?}"
        );
    }

    #[test]
    fn after_dot_on_smtp_result_lists_three_fields() {
        // `SmtpResult` is pre-registered as Nominal in TypeEnv
        // (B2, types.rs::register_http_builtin_types). With an
        // annotated param `r: SmtpResult`, after-dot on `r.` falls
        // through the Nominal dispatch and lists fields.
        let src = "fn show(r: SmtpResult) -> Bool {\n  return r.delivered\n}\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor right after `r.` at line 1, col 11.
        let items = completion_at_position(src, &program, &type_info, &env, 1, 11);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["delivered", "message_id", "duration_ms"] {
            assert!(
                labels.contains(&expected),
                "missing SmtpResult field `{expected}`: {labels:?}"
            );
        }
        let delivered_item = items.iter().find(|i| i.label == "delivered").unwrap();
        assert_eq!(delivered_item.kind, Some(CompletionItemKind::FIELD));
    }

    #[test]
    fn v019_block5_after_dot_on_response_built_in_lists_5_fields() {
        // v0.19.0 — `Response` built-in is pre-registered in the
        // checker with 5 fields (status / content_type / headers /
        // body / body_bytes). The LSP after-dot completion path
        // for Nominals handles it without dedicated dispatch — the
        // shared nominal mechanism reads `env.info(id).fields`.
        // This test pins the contract.
        let src = "@get(\"/x\")\n\
                   fn h() => Response { content_type: \"X\", body: \"y\" }\n\
                   let r = Response { content_type: \"a\", body: \"b\" }\n\
                   r.\n\
                   @server(3000) fn main() => 0\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor at line 3, col 2 (0-based LSP), right after `r.`.
        let items = completion_at_position(src, &program, &type_info, &env, 3, 2);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for expected in ["status", "content_type", "headers", "body", "body_bytes"] {
            assert!(
                labels.contains(&expected),
                "missing field `{expected}` of Response built-in: {labels:?}"
            );
        }
        // All 5 should be FIELD kind.
        let status = items.iter().find(|i| i.label == "status").unwrap();
        assert_eq!(status.kind, Some(CompletionItemKind::FIELD));
    }

    #[test]
    fn v019_block5_scope_level_lists_response_built_in_as_class() {
        // `Response` ya estaba listado pre-v0.19.0 como CLASS para
        // los middlewares (`Fn() -> Response`). v0.19.0 lo upgrade a
        // type instanciable con 5 fields, pero la presentación en
        // completion sigue siendo CLASS para mantener compatibilidad
        // con el flujo usual (CompletionItemKind::CLASS aparece en
        // VSCode con icono de tipo).
        let src = "let a = 1\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        let items = completion_at_position(src, &program, &type_info, &env, 1, 0);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"Response"),
            "missing built-in type `Response`: {labels:?}"
        );
        let response_item = items.iter().find(|i| i.label == "Response").unwrap();
        assert_eq!(response_item.kind, Some(CompletionItemKind::CLASS));
    }

    #[test]
    fn scope_level_lists_smtp_module() {
        // At scope-level (no `.`), the user should see the `smtp`
        // module alongside `http`/`jwt`/`hash`/`log`/`db`.
        let src = "let x = 1\n";
        let (program, env, type_info, _defs, _errs) = check_source_with_types(src);
        // Cursor at line 1 col 0 (after the let stmt).
        let items = completion_at_position(src, &program, &type_info, &env, 1, 0);
        let smtp_item = items.iter().find(|i| i.label == "smtp");
        assert!(
            smtp_item.is_some(),
            "missing `smtp` module at scope-level: {:?}",
            items.iter().map(|i| &i.label).collect::<Vec<_>>()
        );
        let smtp_item = smtp_item.unwrap();
        assert_eq!(smtp_item.kind, Some(CompletionItemKind::MODULE));
        let detail = smtp_item.detail.as_deref().unwrap_or("");
        assert!(
            detail.contains("send"),
            "expected `send` in smtp module detail, was: {detail}"
        );
        assert!(
            detail.contains("SMTP_HOST") || detail.contains("env vars"),
            "expected env vars hint in detail, was: {detail}"
        );
    }

    // -----------------------------------------------------------------
    // Cross-module pre-scan: `@auth_provider` / `@background` resolved
    // through the LSP's `check_source_with_types_and_base_dir`. Cierra
    // the deuda "LSP false positive: @auth_provider cross-module no se
    // resuelve" (2026-06-23).
    // -----------------------------------------------------------------

    #[test]
    fn cross_module_auth_provider_resuelve_via_base_dir_sin_diagnostic_falso() {
        // Setup: tmpdir with `auth.fitz` declaring `@auth_provider`
        // + `incidents.fitz` with `@authenticated` handler. Without
        // base_dir, the checker emits "no `@auth_provider` registered"
        // (false positive). With base_dir, the pre-scan resolves the
        // provider cross-module and the diagnostic disappears.
        let tmp = tempfile::tempdir().expect("tempdir");
        let auth_path = tmp.path().join("auth.fitz");
        let incidents_path = tmp.path().join("incidents.fitz");
        std::fs::write(
            &auth_path,
            "type User { id: Int, name: Str }\n\
             @auth_provider\n\
             async fn current_user(headers: Map<Str, Str>) -> Result<User> {\n\
                 return Ok(User { id: 1, name: \"alice\" })\n\
             }\n",
        )
        .unwrap();
        let incidents_src = "from auth import User\n\
             import auth\n\
             @authenticated\n\
             @put(\"/api/incidents/{id}/status\")\n\
             async fn update_incident(id: Int, user: User) -> Result<Str> {\n\
                 return Ok(\"updated\")\n\
             }\n";
        std::fs::write(&incidents_path, incidents_src).unwrap();

        // Without base_dir: the false positive shows up.
        let errors_no_base = check_source(incidents_src);
        assert!(
            errors_no_base.iter().any(|e| e
                .message
                .contains("no `@auth_provider` registered in the program")),
            "expected `no @auth_provider registered` false positive without base_dir, got: {:?}",
            errors_no_base
                .iter()
                .map(|e| &e.message)
                .collect::<Vec<_>>()
        );

        // With base_dir: the pre-scan resolves the provider, the
        // diagnostic disappears.
        let (_program, _env, _ti, _di, errors_with_base) =
            check_source_with_types_and_base_dir(incidents_src, Some(tmp.path()));
        assert!(
            !errors_with_base.iter().any(|e| e
                .message
                .contains("no `@auth_provider` registered in the program")),
            "expected false positive to disappear with base_dir, got: {:?}",
            errors_with_base
                .iter()
                .map(|e| &e.message)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn cross_module_admin_decorator_resuelve_via_base_dir() {
        // Variant with `@admin`: the provider needs `role: Str` in
        // the User type. The pre-scan extracts `has_role_field`
        // from the source module (via `extract_auth_provider_signature`).
        let tmp = tempfile::tempdir().expect("tempdir");
        let auth_path = tmp.path().join("auth.fitz");
        let admin_path = tmp.path().join("admin_handlers.fitz");
        std::fs::write(
            &auth_path,
            "type User { id: Int, role: Str }\n\
             @auth_provider\n\
             async fn current_user(headers: Map<Str, Str>) -> Result<User> {\n\
                 return Ok(User { id: 1, role: \"admin\" })\n\
             }\n",
        )
        .unwrap();
        let admin_src = "from auth import User\n\
             import auth\n\
             @admin\n\
             @get(\"/api/admin/stats\")\n\
             async fn stats(user: User) -> Result<Str> {\n\
                 return Ok(\"ok\")\n\
             }\n";
        std::fs::write(&admin_path, admin_src).unwrap();

        let (_program, _env, _ti, _di, errors) =
            check_source_with_types_and_base_dir(admin_src, Some(tmp.path()));
        assert!(
            !errors
                .iter()
                .any(|e| e.message.contains("no `@auth_provider`")),
            "expected no false positive for @admin cross-module, got: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
        // The User type does have `role: Str` → `@admin` should
        // not complain about the missing field either.
        assert!(
            !errors.iter().any(|e| e.message.contains("`role: Str`")),
            "expected `role` check to pass cross-module, got: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn cross_module_requires_decorator_resuelve_via_base_dir() {
        // Variant with `@requires("role")` (Phase 9.w.1.iter2.a):
        // shares the validation logic with `@admin` — requires
        // `role: Str` in the User type — but expressed with a
        // configurable arg.
        let tmp = tempfile::tempdir().expect("tempdir");
        let auth_path = tmp.path().join("auth.fitz");
        let editor_path = tmp.path().join("editor.fitz");
        std::fs::write(
            &auth_path,
            "type User { id: Int, role: Str }\n\
             @auth_provider\n\
             async fn current_user(headers: Map<Str, Str>) -> Result<User> {\n\
                 return Ok(User { id: 1, role: \"editor\" })\n\
             }\n",
        )
        .unwrap();
        let editor_src = "from auth import User\n\
             import auth\n\
             @requires(\"editor\")\n\
             @post(\"/api/articles\")\n\
             async fn create_article(user: User) -> Result<Str> {\n\
                 return Ok(\"created\")\n\
             }\n";
        std::fs::write(&editor_path, editor_src).unwrap();

        let (_program, _env, _ti, _di, errors) =
            check_source_with_types_and_base_dir(editor_src, Some(tmp.path()));
        assert!(
            !errors
                .iter()
                .any(|e| e.message.contains("no `@auth_provider`")),
            "expected no false positive for @requires cross-module, got: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn cross_module_background_spawn_resuelve_via_base_dir() {
        // Variant with `@background` + `spawn(<imported_fn>(...))`
        // (B10 / v0.19.2). Without pre-scan: the checker complains
        // `spawn: fn ... is not declared with @background`. With
        // pre-scan: the imported `@background` is registered and
        // the spawn passes the static validation.
        let tmp = tempfile::tempdir().expect("tempdir");
        let worker_path = tmp.path().join("worker.fitz");
        let main_path = tmp.path().join("main.fitz");
        std::fs::write(
            &worker_path,
            "@background\n\
             async fn do_work(id: Int) -> Null {\n\
                 return null\n\
             }\n",
        )
        .unwrap();
        let main_src = "from worker import do_work\n\
             async fn trigger() -> Null {\n\
                 spawn(do_work(42))\n\
                 return null\n\
             }\n";
        std::fs::write(&main_path, main_src).unwrap();

        // Without base_dir: the spawn false positive shows up.
        let errors_no_base = check_source(main_src);
        assert!(
            errors_no_base
                .iter()
                .any(|e| e.message.contains("is not declared with `@background`")),
            "expected `not declared with @background` false positive without base_dir, got: {:?}",
            errors_no_base
                .iter()
                .map(|e| &e.message)
                .collect::<Vec<_>>()
        );

        // With base_dir: the pre-scan registers the imported
        // `@background` and the spawn passes.
        let (_program, _env, _ti, _di, errors_with_base) =
            check_source_with_types_and_base_dir(main_src, Some(tmp.path()));
        assert!(
            !errors_with_base
                .iter()
                .any(|e| e.message.contains("is not declared with `@background`")),
            "expected `@background` false positive to disappear with base_dir, got: {:?}",
            errors_with_base
                .iter()
                .map(|e| &e.message)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn cross_module_pre_scan_silent_fallback_on_missing_module() {
        // Robustness: if the imported module doesn't exist on disk,
        // the pre-scan silently skips it (the codegen/runtime loader
        // owns the "module not found" diagnostic — duplicating it
        // here would be noise). The local checker still runs and
        // emits whatever local errors it finds.
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = "from no_existe import User\n\
             let x: Int = 1\n";
        let (_program, _env, _ti, _di, errors) =
            check_source_with_types_and_base_dir(src, Some(tmp.path()));
        // Should NOT panic, should NOT include errors about reading
        // `no_existe.fitz` from us (silent fallback). Local checker
        // is allowed to warn about unresolved imports.
        for e in &errors {
            assert!(
                !e.message.contains("error reading no_existe"),
                "the pre-scan should not surface module read errors, got: {}",
                e.message
            );
        }
    }

    // -----------------------------------------------------------------------
    // Session B — Phase 11.8.a — LSP diagnostics for `.fitzv` view files
    // -----------------------------------------------------------------------

    #[test]
    fn b11_8_a_uri_is_fitzv_recognises_fitzv_extension() {
        let uri = Url::parse("file:///tmp/Foo.fitzv").unwrap();
        assert!(uri_is_fitzv(&uri));
    }

    #[test]
    fn b11_8_a_uri_is_fitzv_rejects_classic_fitz_extension() {
        let uri = Url::parse("file:///tmp/foo.fitz").unwrap();
        assert!(!uri_is_fitzv(&uri));
    }

    #[test]
    fn b11_8_a_uri_is_fitzv_rejects_other_extensions() {
        let uri = Url::parse("file:///tmp/foo.md").unwrap();
        assert!(!uri_is_fitzv(&uri));
        let uri2 = Url::parse("file:///tmp/foo.rs").unwrap();
        assert!(!uri_is_fitzv(&uri2));
    }

    #[test]
    fn b11_8_a_check_view_source_clean_component_no_errors() {
        // A valid `.fitzv` file should produce zero errors.
        let src = r#"component X {
  state { count: Int = 0 }
  <template><div>{count}</div></template>
}"#;
        let errors = check_view_source(src);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    #[test]
    fn b11_8_a_check_view_source_view_parse_error_surfaces_as_diagnostic() {
        // Deliberately broken — missing `state {}` block delimiter.
        let src = r#"component X {
  state
  <template><div>hi</div></template>
}"#;
        let errors = check_view_source(src);
        assert!(!errors.is_empty(), "expected parse error, got none");
        // The error should have line/column set to something reasonable
        // in the .fitzv file (not a synthetic 0:0).
        assert!(
            errors[0].line > 0,
            "line should be non-zero: {:?}",
            errors[0]
        );
    }

    #[test]
    fn b11_8_a_check_view_source_check_error_surfaces_with_component_context() {
        // Type-check error: state field type doesn't match its default.
        let src = r#"component X {
  state { count: Int = "not an int" }
  <template><div>{count}</div></template>
}"#;
        let errors = check_view_source(src);
        assert!(!errors.is_empty(), "expected type error");
        // The error message should carry the check context (component
        // name, state field name) appended in parens.
        let msg = &errors[0].message;
        assert!(
            msg.contains("component 'X'") || msg.contains("count"),
            "message should carry context, got: {msg}"
        );
    }

    #[test]
    fn b11_8_a_check_source_by_uri_routes_fitzv_to_view_pipeline() {
        // The URI-based dispatcher must route a `.fitzv` URI through
        // the view pipeline. Regression: if it routed through the
        // classic Fitz pipeline, the classic parser would choke on
        // `component X { ... }` (not a classic top-level construct).
        let uri = Url::parse("file:///tmp/X.fitzv").unwrap();
        let src = r#"component X {
  state { count: Int = 0 }
  <template><div>{count}</div></template>
}"#;
        let errors = check_source_by_uri(&uri, src);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    #[test]
    fn b11_8_a_check_source_by_uri_routes_classic_fitz_to_classic_pipeline() {
        // A `.fitz` URI with classic-valid content routes through the
        // classic Fitz pipeline (unchanged path).
        let uri = Url::parse("file:///tmp/hello.fitz").unwrap();
        let src = r#"let x = 42
print(x)"#;
        let errors = check_source_by_uri(&uri, src);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    // -----------------------------------------------------------------------
    // Phase 11.8 cross-file (v0.26.0) — `check_view_source_with_base_dir`
    // resolves imported sibling `<Child />` (incl. aliased + transitive)
    // instead of the single-file false "unknown component".
    // -----------------------------------------------------------------------

    #[test]
    fn view_cross_file_child_resolves_with_base_dir_unknown_without() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("Card.fitzv"),
            "component Card {\n  state { title: Str = \"\" }\n  <template><article>{title}</article></template>\n}\n",
        )
        .unwrap();
        let app_src = "from Card import Card\ncomponent App {\n  state {}\n  <template><div><Card title=\"hi\" /></div></template>\n}\n";

        // Without base_dir: single-file → cross-file child is unknown.
        let no_base = check_view_source(app_src);
        assert!(
            no_base
                .iter()
                .any(|e| e.message.contains("unknown component")),
            "expected `unknown component` without base_dir, got: {:?}",
            no_base.iter().map(|e| &e.message).collect::<Vec<_>>()
        );

        // With base_dir: the sibling Card is loaded and composition validates.
        let with_base = check_view_source_with_base_dir(app_src, Some(tmp.path()));
        assert!(
            with_base.is_empty(),
            "expected no errors with base_dir, got: {:?}",
            with_base.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn view_cross_file_aliased_child_resolves_with_base_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("Card.fitzv"),
            "component Card {\n  state { title: Str = \"\" }\n  <template><article>{title}</article></template>\n}\n",
        )
        .unwrap();
        // `from Card import Card as Row` → `<Row />` must resolve.
        let app_src = "from Card import Card as Row\ncomponent App {\n  state {}\n  <template><div><Row title=\"hi\" /></div></template>\n}\n";
        let with_base = check_view_source_with_base_dir(app_src, Some(tmp.path()));
        assert!(
            with_base.is_empty(),
            "expected aliased `<Row />` to resolve with base_dir, got: {:?}",
            with_base.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn view_cross_file_genuinely_unknown_child_still_errors_with_base_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // No sibling on disk for `<Nope />` and it is not imported.
        let app_src =
            "component App {\n  state {}\n  <template><div><Nope /></div></template>\n}\n";
        let with_base = check_view_source_with_base_dir(app_src, Some(tmp.path()));
        assert!(
            with_base
                .iter()
                .any(|e| e.message.contains("unknown component")),
            "a genuinely missing component still errors with base_dir, got: {:?}",
            with_base.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn view_cross_file_dispatch_via_uri_resolves_sibling() {
        // `check_source_by_uri` derives base_dir from the URI, so routing a
        // `.fitzv` URI through the dispatcher resolves the sibling too.
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tmp.path().join("Card.fitzv"),
            "component Card {\n  state { title: Str = \"\" }\n  <template><article>{title}</article></template>\n}\n",
        )
        .unwrap();
        let app_path = tmp.path().join("App.fitzv");
        let app_src = "from Card import Card\ncomponent App {\n  state {}\n  <template><div><Card title=\"hi\" /></div></template>\n}\n";
        std::fs::write(&app_path, app_src).unwrap();
        let uri = Url::from_file_path(&app_path).expect("file url");
        let errors = check_source_by_uri(&uri, app_src);
        assert!(
            errors.is_empty(),
            "expected dispatcher to resolve the sibling via URI base_dir, got: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    // -----------------------------------------------------------------------
    // Session B — Phase 11.8.b — LSP completions inside `.fitzv`
    // -----------------------------------------------------------------------

    #[test]
    fn b11_8_b_view_completions_after_brace_offers_directives() {
        // After `{` at line start, offer template directives.
        let src = r#"component X {
  state { count: Int = 0 }
  <template>
    <div>{
    </div>
  </template>
}"#;
        // Line 3 (0-based), column 10 — right after `<div>{`.
        let items = completion_at_position_view(src, 3, 10);
        let labels: Vec<&str> = items.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"#if"), "labels: {:?}", labels);
        assert!(labels.contains(&"#for"), "labels: {:?}", labels);
    }

    #[test]
    fn b11_8_b_view_completions_after_at_offers_event_decorators() {
        let src = r#"component X {
  state { count: Int = 0 }
  <template>
    <button @
    </button>
  </template>
}"#;
        // Line 3 (0-based), column 13 — right after `<button @`.
        let items = completion_at_position_view(src, 3, 13);
        let labels: Vec<&str> = items.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"click"), "labels: {:?}", labels);
        assert!(labels.contains(&"submit"), "labels: {:?}", labels);
    }

    #[test]
    fn b11_8_b_view_completions_offer_state_field_names_of_enclosing_component() {
        let src = r#"component App {
  state {
    count: Int = 0
    title: Str = "hi"
  }
  <template>
    <div>{
    </div>
  </template>
}"#;
        // Cursor inside the template — should offer state field names.
        let items = completion_at_position_view(src, 6, 10);
        let labels: Vec<&str> = items.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"count"), "labels: {:?}", labels);
        assert!(labels.contains(&"title"), "labels: {:?}", labels);
    }

    #[test]
    fn b11_8_b_view_completions_offer_event_names_of_enclosing_component() {
        let src = r#"component App {
  state { count: Int = 0 }
  event inc() { count = count + 1 }
  event dec() { count = count - 1 }
  <template>
    <div>{
    </div>
  </template>
}"#;
        // Cursor inside the template — should offer event names too.
        let items = completion_at_position_view(src, 5, 10);
        let labels: Vec<&str> = items.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"inc"), "labels: {:?}", labels);
        assert!(labels.contains(&"dec"), "labels: {:?}", labels);
    }

    #[test]
    fn b11_8_b_view_completions_directive_items_carry_snippet_insert_text() {
        // The `#if` directive item should be a SNIPPET with tabstops.
        let src = "component X { state {} <template>{";
        let items = completion_at_position_view(src, 0, 34);
        let if_item = items
            .iter()
            .find(|c| c.label == "#if")
            .expect("missing #if in items");
        assert_eq!(if_item.kind, Some(CompletionItemKind::SNIPPET));
        assert!(
            if_item.insert_text.is_some(),
            "snippet should carry insert_text"
        );
        assert!(
            if_item.insert_text.as_ref().unwrap().contains("{/if"),
            "if snippet should close with /if"
        );
    }

    // -----------------------------------------------------------------------
    // Session B — Phase 11.8.c — LSP hover inside `.fitzv`
    // -----------------------------------------------------------------------

    #[test]
    fn b11_8_c_view_hover_over_state_field_shows_type_and_component() {
        let src = "component App {\n  state {\n    count: Int = 0\n    title: Str = \"hi\"\n  }\n  <template><div>{count}</div></template>\n}";
        // Line 5 (0-based), column 18 — on `count` inside the
        // template interpolation `{count}`.
        let hover = hover_at_position_view(src, 5, 18).expect("hover on state field");
        let text = match hover.contents {
            HoverContents::Markup(m) => m.value,
            _ => panic!("expected markup"),
        };
        assert!(text.contains("count: Int"), "hover text: {text}");
        assert!(text.contains("state field"), "hover text: {text}");
        assert!(text.contains("App"), "hover text: {text}");
    }

    #[test]
    fn b11_8_c_view_hover_over_event_handler_shows_event_and_component() {
        let src = "component App {\n  state { count: Int = 0 }\n  event inc() { count = count + 1 }\n  <template><button data-flv-click=\"inc\">+</button></template>\n}";
        // Line 2 (0-based), column 8 — on `inc` in the event decl.
        let hover = hover_at_position_view(src, 2, 8).expect("hover on event handler");
        let text = match hover.contents {
            HoverContents::Markup(m) => m.value,
            _ => panic!("expected markup"),
        };
        assert!(text.contains("event inc"), "hover text: {text}");
        assert!(text.contains("event handler"), "hover text: {text}");
        assert!(text.contains("App"), "hover text: {text}");
    }

    #[test]
    fn b11_8_c_view_hover_over_unknown_ident_returns_none() {
        let src = "component App {\n  state { count: Int = 0 }\n  <template><div>{unknown_ident}</div></template>\n}";
        let hover = hover_at_position_view(src, 2, 18);
        assert!(hover.is_none(), "hover over unknown ident should be None");
    }

    #[test]
    fn b11_8_c_view_hover_over_whitespace_returns_none() {
        let src = "component App {\n  state { count: Int = 0 }\n}";
        // Line 0, column 0 — on `c` of `component` (a keyword).
        // `ident_under_cursor` returns the ident, but neither
        // state fields nor event names include `component`, so
        // hover returns None.
        let hover = hover_at_position_view(src, 0, 0);
        assert!(hover.is_none(), "hover on keyword should be None");
    }

    // -----------------------------------------------------------------------
    // Session B — Phase 11.8.d — LSP go-to-def inside `.fitzv`
    // -----------------------------------------------------------------------

    #[test]
    fn b11_8_d_view_goto_def_over_state_field_ref_jumps_to_state_decl() {
        let src = "component App {\n  state {\n    count: Int = 0\n  }\n  <template><div>{count}</div></template>\n}";
        let uri = Url::parse("file:///tmp/App.fitzv").unwrap();
        // Line 4 (0-based), column 20 — on `count` inside the
        // template interpolation.
        let loc =
            definition_at_position_view(&uri, src, 4, 20).expect("goto-def on state field ref");
        assert_eq!(loc.uri, uri);
        // The declaration line is `    count: Int = 0` at line 2
        // (0-based). The column should be 4 (the leading indent).
        assert_eq!(loc.range.start.line, 2);
        assert_eq!(loc.range.start.character, 4);
        // Range covers `count` — 5 chars.
        assert_eq!(loc.range.end.character, 9);
    }

    #[test]
    fn b11_8_d_view_goto_def_over_event_ref_jumps_to_event_decl() {
        let src =
            "component App {\n  state { count: Int = 0 }\n  event inc() { count = count + 1 }\n}";
        let uri = Url::parse("file:///tmp/App.fitzv").unwrap();
        // Line 2 (0-based), column 8 — on `inc` in the event decl.
        // (In practice you'd goto-def from a data-flv-click ref;
        // here we just verify the decl-line lookup works.)
        let loc =
            definition_at_position_view(&uri, src, 2, 8).expect("goto-def on event handler ref");
        assert_eq!(loc.uri, uri);
        // Event decl is line 2. Column is after `  event ` (2 + 6 = 8).
        assert_eq!(loc.range.start.line, 2);
        assert_eq!(loc.range.start.character, 8);
    }

    #[test]
    fn b11_8_d_view_goto_def_over_unknown_ident_returns_none() {
        let src = "component App {\n  state { count: Int = 0 }\n  <template><div>{unknown}</div></template>\n}";
        let uri = Url::parse("file:///tmp/App.fitzv").unwrap();
        let loc = definition_at_position_view(&uri, src, 2, 20);
        assert!(loc.is_none(), "goto-def on unknown ident should be None");
    }
}
