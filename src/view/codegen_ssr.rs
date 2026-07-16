//! Phase 11.6.b + 11.6.c + 11.6.d — SSR emitter (`view::emit_ssr`).
//!
//! Second backend paralleling `codegen_wasm.rs`. Consumes an
//! [`ExpandedViewFile`] and emits classic Fitz source text
//! targeting the `fitz-liveviews` framework runtime contract
//! (v0.20.0 `@live_component` + `@render_for` + `@on`
//! decorators + v0.20.1 implicit `flv_register` injection).
//!
//! Phase 11.6.d adds:
//! - Same-file `<Child />` composition — the composition site
//!   emits an `Expr` piece calling `<Child>_render(<Child> {
//!   <props>, ... }).raw` and splicing the result into the
//!   parent's chain-form html body.
//! - Loader-facing entry point: the module bridge
//!   [`crate::view::transform_fitzv_source`] wraps `parse →
//!   expand → check → emit_module_ssr` so the classic loader
//!   can treat a `.fitzv` file as a drop-in classic Fitz
//!   module. See `src/view/mod.rs` for the bridge helpers.
//!
//! ## The .fitzv → fitz-liveviews mapping
//!
//! Source:
//! ```text
//! component Counter {
//!   state { count: Int = 0 }
//!   event reset() { count = 0 }
//!   <template>
//!     <div>
//!       <span>{count}</span>
//!       <button @click="reset">reset</button>
//!     </div>
//!   </template>
//! }
//! ```
//!
//! Emitted classic Fitz:
//! ```text
//! from fitz_liveviews import Html, html
//!
//! @live_component("Counter")
//! type Counter {
//!   count: Int = 0
//! }
//!
//! @render_for("Counter")
//! fn Counter_render(state: Counter) -> Html {
//!   return html("<div><span>{state.count}</span>
//!     <button data-flv-click=\"reset\">reset</button></div>")
//! }
//!
//! @on("Counter", "reset")
//! fn Counter_reset(state: Counter, payload: Map<Str, Str>) -> Counter {
//!   return Counter { count: 0 }
//! }
//! ```
//!
//! Two source-to-emit transformations documented in §9.u:
//! 1. **`@click` → `data-flv-click`** in the emitted HTML —
//!    fitz-liveviews's client runtime binds `data-flv-<event>`
//!    attrs to WebSocket event frames.
//! 2. **`{field}` → `{state.field}`** in the template
//!    interpolation — classic Fitz's string interpolation
//!    resolves the identifier against the enclosing scope,
//!    and the render fn's parameter is named `state`.
//!
//! Plus event body lowering: `.fitzv` events use mutation
//! bodies (`count = count + 1`); the `@on` fn contract is
//! `(state, payload) -> new_state`, so the emitter builds a
//! fresh struct literal from ALL state fields, substituting
//! the ones the event mutated with the assigned value (with
//! bare identifiers rewritten as `state.<field>`).
//!
//! ## MVP scope (11.6.b + 11.6.c partial)
//!
//! - Single-component `.fitzv` file (multi-component
//!   composition comes in 11.6.d).
//! - State fields of any type that classic Fitz accepts
//!   (`Int` / `Float` / `Bool` / `Str` / `Nullable<T>` / ...).
//! - Event bodies with **arbitrary expression RHS** since
//!   11.6.c — `count = count + 1`, `msg = format(...)`,
//!   `xs = state.xs.map(fn(x) => x + 1)`, etc. See
//!   [`format_fitz_expr`] for the exact grammar accepted.
//! - Multiple `Stmt::Assign` per event body: OK — the emitter
//!   accumulates the mutations and builds one struct literal.
//! - Template with `Text` / `Interpolation` / `Element` /
//!   `Static` attrs / `Interpolation` attrs / `Event` attrs.
//! - Template interpolations use the same expression walker as
//!   event body RHS — `{state.count}` (bare state), `{state.name.upper()}`,
//!   `{state.count + 1}`, function calls, etc.
//! - `<style scoped>` / `<style global>` since 11.6.c — inlined
//!   at the top of the render output as a `<style>` tag
//!   (scoped CSS carries the `-<scope>` class suffix already
//!   applied by 11.3.b's `apply_scope`).
//! - Since the 11.6.c continuation:
//!   - `{#if cond} ... {/if}` and `{#if cond} ... {#else} ... {/if}`
//!     template directives lower to a Fitz if-as-expression
//!     yielding `Str`, embedded as an `Expr` piece in the
//!     render body's chain-form html(...) argument.
//!   - `{#for x in xs} ... {/for}` lowers to
//!     `__fitz_view_str_join(<iter>.map(fn(x) => <body>))`
//!     with the `x` binding shadowing any same-named state
//!     field inside the body. The `__fitz_view_str_join(xs:
//!     List<Str>) -> Str` helper is emitted at module header
//!     time (unconditionally, so dead-code when the module
//!     has no `{#for}`).
//!   - View lexer accepts `.` in event body raw-capture
//!     context, so method calls (`count = state.count.
//!     something()`) and field access work in `.fitzv`
//!     source. `Token::Dot` was added to the view lexer and
//!     the raw-body re-serialiser.
//! - Since Phase 11.6.d:
//!   - Same-file `<Child prop="v" />` composition —
//!     `<Child prop="42" />` lowers to
//!     `<Child>_render(<Child> { prop: 42 }).raw`, embedded as
//!     an `Expr` piece. Prop coercion follows the 11.5.d
//!     subset (Str / Int / Float / Bool + `Nullable<T>` of a
//!     primitive) via
//!     [`coerce_child_prop_raw_value_to_fitz_literal`]. Cross-
//!     file composition (child in a sibling `.fitzv` imported
//!     into `main.fitz`) errors clearly and points at 11.6.e
//!     — needs the loader's expanded-file cache threaded to
//!     the parent's expand/check pipeline.
//!   - `.fitzv` module loader integration — see
//!     [`crate::view::transform_fitzv_source`] for the
//!     helper the classic loader consumes when it sees a
//!     `.fitzv` file at import resolution time.
//! - `<slot />` still deferred to Phase 11.7+.
//!
//! ## Emit shape: pretty vs chain form
//!
//! The render fn's `html(...)` argument uses one of two
//! serialisation shapes:
//!
//! - **Pretty form** (triple-string) — when every template
//!   piece is `Text` (no directives), the concatenation
//!   emits as `html("""<full HTML>""")`. Readable, closest
//!   to the source `.fitzv` shape.
//! - **Chain form** — when any directive is present, the
//!   pieces emit as `"txt1" + (expr1) + "txt2" + (expr2) +
//!   ...`. Each `Text` piece becomes a single-quoted Fitz
//!   string literal (only `"`, `\n`, `\r` escaped —
//!   backslash-prefixed sequences like `\{` / `\}` from CSS
//!   pre-escape pass through verbatim so classic Fitz reads
//!   them as literal `{` / `}`).
//!
//! ## Public API surface
//!
//! [`emit_module_ssr`] is the entry point the module loader
//! will call in 11.6.d. Returns the full emitted Fitz source
//! text (including the `from fitz_liveviews import ...` line,
//! one `@live_component` type, one `@render_for` fn, and N
//! `@on` fns per component in the file).
//!
//! [`SsrEmitError`] carries a message + context (naming the
//! offending component + event / template node) so the caller
//! can format a useful error. [`SsrEmitResult`] is the alias.

use super::expand::{
    ExpandedAttr, ExpandedComponent, ExpandedEventHandler, ExpandedTemplateNode, ExpandedViewFile,
};
use crate::ast::{AssignTarget, Expr, Stmt};
use std::fmt::Write as _;

/// An error produced by the SSR emitter. `message` is the
/// user-facing description; `context` identifies where in the
/// source the emitter was working when it failed (component
/// name, event name, template node position, etc.). Both are
/// concatenated in `Display`.
#[derive(Debug, Clone, PartialEq)]
pub struct SsrEmitError {
    pub message: String,
    pub context: String,
}

impl std::fmt::Display for SsrEmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ssr emit error — {} ({})", self.message, self.context)
    }
}

impl std::error::Error for SsrEmitError {}

pub type SsrEmitResult<T> = Result<T, SsrEmitError>;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Emit the classic Fitz source text for the entire view file.
/// Consumed by the module loader (11.6.d) when it encounters
/// a `.fitzv` file: this is what gets fed to the classic Fitz
/// lexer + parser as if it were a hand-authored `.fitz`.
///
/// The output always starts with the fitz-liveviews import,
/// followed by one `@live_component` type + one `@render_for`
/// fn + N `@on` fns per component. Blank lines separate the
/// per-component blocks for readability (the emitted source
/// is auto-generated but still developer-inspectable — the
/// counter demo baseline pattern from 11.5).
pub fn emit_module_ssr(file: &ExpandedViewFile) -> SsrEmitResult<String> {
    let mut out = String::new();
    emit_module_header(&mut out);
    for component in &file.components {
        emit_component_ssr_into(component, &file.components, &mut out)?;
    }
    Ok(out)
}

/// Emit one component's classic Fitz source. Rarely useful in
/// isolation — [`emit_module_ssr`] is the entry point. Kept
/// pub so tests can exercise single-component fixtures without
/// wrapping in `ExpandedViewFile { components: vec![...] }`.
///
/// Since Phase 11.6.d, a single-component emit sees only itself
/// as a sibling. `<Child />` composition in this path errors with
/// a clear message pointing at `emit_module_ssr` — a component
/// wanting to compose a sibling MUST be emitted through the
/// module entry.
pub fn emit_component_ssr(component: &ExpandedComponent) -> SsrEmitResult<String> {
    let mut out = String::new();
    emit_module_header(&mut out);
    let siblings = std::slice::from_ref(component);
    emit_component_ssr_into(component, siblings, &mut out)?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Module-level header
// ---------------------------------------------------------------------------

/// The `from fitz_liveviews import ...` line prepended to every
/// emitted module. The imports match what the render fn signature
/// (`-> Html`) and the render body (`html("""...""")`) need.
/// `Map` is a language built-in so it doesn't need an import;
/// `Str` likewise.
fn emit_module_header(out: &mut String) {
    out.push_str(
        "// Generated by fitz view SSR emitter (Phase 11.6.b + 11.6.c).\n\
         // Do NOT edit — regenerate from the source `.fitzv`.\n\
         \n\
         from fitz_liveviews import Html, html\n\
         \n\
         // Phase 11.6.c continuation — helper consumed by every\n\
         // `{#for x in xs} <body> {/for}` template directive: joins\n\
         // a `List<Str>` (typically the output of `xs.map(fn(x) =>\n\
         // <body as Str>)`) into a single `Str` for concatenation\n\
         // into the surrounding HTML. Classic Fitz's `List<Str>`\n\
         // built-in methods do not include `.join()`, so this\n\
         // helper is emitted at module header time. Unused when\n\
         // no `{#for}` is present — accepted as generated-code dead\n\
         // code.\n\
         fn __fitz_view_str_join(xs: List<Str>) -> Str {\n  \
             let out = \"\"\n  \
             for x in xs {\n    \
                 out = out + x\n  \
             }\n  \
             return out\n\
         }\n\
         \n",
    );
}

// ---------------------------------------------------------------------------
// Per-component emit
// ---------------------------------------------------------------------------

fn emit_component_ssr_into(
    component: &ExpandedComponent,
    siblings: &[ExpandedComponent],
    out: &mut String,
) -> SsrEmitResult<()> {
    emit_state_type(component, out)?;
    emit_render_fn(component, siblings, out)?;
    for event in &component.events {
        emit_event_fn(component, event, out)?;
    }
    Ok(())
}

/// Emit the state type + `@live_component` decorator. The
/// component's name is used verbatim as both the decorator arg
/// and the type name — matches the convention `.fitzv` writers
/// follow when they hand-write `@live_component("MetricTile") type
/// MetricTile { ... }`.
///
/// Every state field carries its declared type + default. The
/// default is emitted via the classic Fitz source form of the
/// original `Expr` — done through `format_expr_source`. Since the
/// checker already validated the shape (per 11.2.b), we trust
/// the AST here.
fn emit_state_type(component: &ExpandedComponent, out: &mut String) -> SsrEmitResult<()> {
    writeln!(out, "@live_component(\"{}\")", component.name).unwrap();
    writeln!(out, "type {} {{", component.name).unwrap();
    for field in &component.state {
        let ty = format_type_expr_source(&field.type_expr);
        let default = format_expr_source(&field.default, &component.name, "state field default")?;
        writeln!(out, "  {}: {} = {}", field.name, ty, default).unwrap();
    }
    writeln!(out, "}}\n").unwrap();
    Ok(())
}

/// Emit the `@render_for` fn — a single-return fn that builds
/// the HTML string from the template. Uses classic Fitz's
/// interpolated triple-string `html("""...""")` shape.
///
/// The return type is `Html`. Every state-field reference in
/// the template becomes `{state.<field>}` inside the emitted
/// string so classic Fitz's interpolation resolver finds it in
/// the enclosing scope.
fn emit_render_fn(
    component: &ExpandedComponent,
    siblings: &[ExpandedComponent],
    out: &mut String,
) -> SsrEmitResult<()> {
    let state_field_names: Vec<&str> = component.state.iter().map(|f| f.name.as_str()).collect();

    writeln!(out, "@render_for(\"{}\")", component.name).unwrap();
    writeln!(
        out,
        "fn {}_render(state: {}) -> Html {{",
        component.name, component.name
    )
    .unwrap();

    let mut pieces: Vec<TemplatePiece> = Vec::new();

    // Phase 11.6.c — inline `<style scoped>` / `<style global>`
    // at the top of the render output. The compiled CSS body
    // (with 11.3.b's `apply_scope` already applied for the
    // `scoped` case) gets wrapped in a `<style>` tag and
    // prepended. Braces are escaped (`{` → `\{`, `}` → `\}`)
    // because CSS `.foo { ... }` uses braces that would
    // otherwise collide with classic Fitz's string
    // interpolation `{expr}` syntax inside `html("""...""")`.
    // fitz-liveviews's HTML diff routine collapses the stable
    // `<style>` block to a no-op patch after the first render,
    // so the per-render inline cost is one-time. Real cleanup
    // (`<style>` routed to `<head>`) would need
    // framework-runtime cooperation — deferred.
    if let Some(style) = &component.style {
        let css_body = match style {
            super::expand::ExpandedStyle::Scoped { css_scoped, .. } => css_scoped,
            super::expand::ExpandedStyle::Global { css, .. } => css,
        };
        let mut buf = String::from("<style>");
        buf.push_str(&escape_css_braces_for_fitz_interp(css_body));
        buf.push_str("</style>");
        push_text(&mut pieces, &buf);
    }

    if let Some(template) = &component.template {
        for node in &template.roots {
            emit_template_node_to_pieces(
                node,
                &state_field_names,
                &[],
                component,
                siblings,
                &mut pieces,
            )?;
        }
    }
    let arg = serialize_pieces_as_html_arg(&pieces);
    writeln!(out, "  return html({arg})").unwrap();
    writeln!(out, "}}\n").unwrap();
    Ok(())
}

// ---------------------------------------------------------------------------
// Phase 11.6.c continuation — template pieces
// ---------------------------------------------------------------------------

/// A segment of the render fn's HTML output. Every template
/// node emits zero or more pieces; the final `html(...)` arg
/// is a `+`-concatenation of the pieces (chain form) or a
/// single triple-string when all pieces are `Text` (pretty form).
///
/// `{#if}` / `{#for}` directives emit as `Expr` pieces holding
/// a Fitz expression that evaluates to `Str`. All other nodes
/// emit as `Text`.
#[derive(Debug, Clone, PartialEq)]
enum TemplatePiece {
    /// Literal HTML content. May include:
    /// - Static markup (`<div>`, `</div>`, attribute names + values).
    /// - Classic Fitz `{state.<field>}` interpolation syntax that
    ///   the enclosing string literal will resolve at runtime.
    /// - Backslash-escaped `\{` / `\}` from CSS body pre-escape.
    Text(String),
    /// Fitz expression source (already lowered via
    /// [`format_fitz_expr_scoped`]) that evaluates to `Str` when
    /// executed. Wrapped in `(...)` at serialisation time.
    Expr(String),
}

/// Append literal HTML text to the piece list. Merges with the
/// preceding `Text` piece when possible so the pretty-form
/// triple-string case emits as a single contiguous string.
fn push_text(pieces: &mut Vec<TemplatePiece>, s: &str) {
    if s.is_empty() {
        return;
    }
    if let Some(TemplatePiece::Text(last)) = pieces.last_mut() {
        last.push_str(s);
    } else {
        pieces.push(TemplatePiece::Text(s.to_string()));
    }
}

/// Serialise a list of template pieces into the source-text
/// argument for `html(...)`. Two modes:
///
/// 1. **Pretty form** — when every piece is `Text`, emit one
///    triple-string `"""<concat>"""` that classic Fitz reads
///    as a single interpolated string. This is the shape the
///    counter demo, dashboard tile, and every directive-free
///    fixture uses; it's the readable case.
/// 2. **Chain form** — when any piece is `Expr`, emit a
///    `+`-concatenation: `"txt1" + (expr1) + "txt2" + (expr2)`.
///    Each `Text` piece becomes a single-quoted Fitz string
///    literal (only `"`, `\n`, `\r` escaped — CSS-pre-escaped
///    `\{` / `\}` and Fitz interp `{state.<field>}` pass
///    through verbatim). Each `Expr` piece gets wrapped in
///    parens for precedence safety.
fn serialize_pieces_as_html_arg(pieces: &[TemplatePiece]) -> String {
    if pieces.is_empty() {
        return "\"\"".to_string();
    }
    let has_expr = pieces.iter().any(|p| matches!(p, TemplatePiece::Expr(_)));
    if !has_expr {
        let mut buf = String::from("\"\"\"");
        for p in pieces {
            if let TemplatePiece::Text(s) = p {
                buf.push_str(s.trim_end_matches(char::is_whitespace));
            }
        }
        // Restore trim policy from the pre-refactor path: trim
        // only trailing whitespace (multi-piece Text output
        // already handled the whitespace between segments; the
        // outer trim just avoids a dangling newline before
        // `"""`).
        buf.push_str("\"\"\"");
        buf
    } else {
        let parts: Vec<String> = pieces
            .iter()
            .map(|p| match p {
                TemplatePiece::Text(s) => fitz_str_literal_for_chain_form(s),
                TemplatePiece::Expr(e) => format!("({e})"),
            })
            .collect();
        parts.join(" + ")
    }
}

/// Quote `s` as a classic Fitz single-quoted string literal for
/// the chain-form serialisation. Escapes `"` and `\n` / `\r`.
/// Does NOT re-escape backslash-prefixed sequences that were
/// already Fitz-legal escapes at the piece-content level (like
/// `\{` / `\}` from CSS body pre-escape). Fitz's string parser
/// reads these correctly whether they appear in a
/// triple-string or a single-quoted string.
fn fitz_str_literal_for_chain_form(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Emit one `@on(<component>, <event>)` fn. Body semantics:
/// (state, payload) -> state, always returning a fresh struct
/// literal that carries EVERY state field. Mutated fields
/// take the assigned value (RHS lowered via
/// [`format_fitz_expr`] so bare state-field idents get the
/// `state.` prefix); untouched fields carry over from
/// `state.<field>`.
///
/// Phase 11.6.b MVP restricted the RHS to literals + bare
/// state-field idents; Phase 11.6.c widened this to the full
/// walker grammar (BinOp / UnaryOp / Call / Field / Index /
/// StrInterp / List / Map / Range / Ok / Err / arrow
/// closure). Statement / control-flow shapes in the body
/// (multi-stmt with an `if` inside, `for` loops, etc.) still
/// reject — the emitter accumulates linear `Stmt::Assign`
/// mutations only. Richer bodies land alongside the template
/// directive support in the 11.6.c continuation.
fn emit_event_fn(
    component: &ExpandedComponent,
    event: &ExpandedEventHandler,
    out: &mut String,
) -> SsrEmitResult<()> {
    if !event.params.is_empty() {
        return Err(SsrEmitError {
            message: format!(
                "event `{}` declares parameters — the fitz-liveviews `@on` contract \
                 takes `(state, payload)` only. Parameters on `event` blocks are \
                 deferred to Phase 11.7+ (event bubbling with structured payloads).",
                event.name
            ),
            context: format!("component `{}` event `{}`", component.name, event.name),
        });
    }

    // Accumulate the mutations. If the same field is assigned
    // multiple times inside the body, the LAST assignment wins
    // — same semantics classic Fitz has when you re-assign the
    // same var in a linear body.
    let state_field_names: Vec<&str> = component.state.iter().map(|f| f.name.as_str()).collect();
    let mut mutations: Vec<(String, String)> = Vec::new();
    for stmt in &event.body {
        match stmt {
            Stmt::Assign {
                target,
                value,
                type_: _,
                ..
            } => {
                let field_name = match target {
                    AssignTarget::Ident(name, _) => name.clone(),
                    AssignTarget::Field { .. } => {
                        return Err(SsrEmitError {
                            message: format!(
                                "event `{}` body assigns to a field access (`obj.field = ...`) \
                                 — only direct state-field assignments (`<field> = ...`) are \
                                 supported. Deferred to Phase 11.7+.",
                                event.name
                            ),
                            context: format!(
                                "component `{}` event `{}`",
                                component.name, event.name
                            ),
                        });
                    }
                    AssignTarget::Index { .. } => {
                        return Err(SsrEmitError {
                            message: format!(
                                "event `{}` body assigns to an index (`xs[i] = ...`) — only \
                                 direct state-field assignments (`<field> = ...`) are supported. \
                                 Deferred to Phase 11.7+.",
                                event.name
                            ),
                            context: format!(
                                "component `{}` event `{}`",
                                component.name, event.name
                            ),
                        });
                    }
                };
                if !state_field_names.iter().any(|s| *s == field_name) {
                    return Err(SsrEmitError {
                        message: format!(
                            "event `{}` body assigns to `{}` which is not a declared state field. \
                             Declare it in `state {{ ... }}` first.",
                            event.name, field_name
                        ),
                        context: format!("component `{}` event `{}`", component.name, event.name),
                    });
                }
                let rhs = format_event_rhs(value, &state_field_names, component, event)?;
                mutations.push((field_name, rhs));
            }
            _ => {
                return Err(SsrEmitError {
                    message: format!(
                        "event `{}` body contains a non-assignment statement — the SSR emitter \
                         supports single- or multi-`Stmt::Assign` bodies (as of 11.6.c). Other \
                         statement kinds (`if` / `for` / `return` / expression statements) \
                         deferred to the 11.6.c template-directive continuation.",
                        event.name
                    ),
                    context: format!("component `{}` event `{}`", component.name, event.name),
                });
            }
        }
    }

    writeln!(out, "@on(\"{}\", \"{}\")", component.name, event.name).unwrap();
    writeln!(
        out,
        "fn {}_{}(state: {}, payload: Map<Str, Str>) -> {} {{",
        component.name, event.name, component.name, component.name
    )
    .unwrap();
    writeln!(out, "  return {} {{", component.name).unwrap();
    for field in &component.state {
        // If mutated, use the mutation's RHS; else carry over.
        let assigned = mutations
            .iter()
            .rev()
            .find(|(n, _)| n == &field.name)
            .map(|(_, rhs)| rhs.clone());
        match assigned {
            Some(rhs) => writeln!(out, "    {}: {},", field.name, rhs).unwrap(),
            None => writeln!(out, "    {}: state.{},", field.name, field.name).unwrap(),
        }
    }
    writeln!(out, "  }}").unwrap();
    writeln!(out, "}}\n").unwrap();
    Ok(())
}

// ---------------------------------------------------------------------------
// Event body — RHS lowering
// ---------------------------------------------------------------------------

/// Format the RHS of an event body assignment. Thin wrapper
/// over [`format_fitz_expr_scoped`] with the event's context label
/// baked in — the walker handles the actual grammar.
///
/// Phase 11.6.c widened this from literal-only to the full
/// walker grammar (BinOp arithmetic, function calls,
/// StrInterp, Field access, Index, List, Map, Range,
/// Ok/Err, arrow closures for `.map(...)` etc.).
///
/// Phase 11.6.e — the emitted `@on` fn signature is
/// `fn <Name>_<event>(state: <Name>, payload: Map<Str, Str>) -> <Name>`,
/// so `payload` is a valid free identifier inside the body. It is
/// added to the walker's `local_scope` here (paralleling closure
/// params) so RHS expressions like `payload["author"]` or
/// `payload.has("text")` emit verbatim without tripping the "not a
/// declared state field" rejection.
fn format_event_rhs(
    expr: &Expr,
    state_field_names: &[&str],
    component: &ExpandedComponent,
    event: &ExpandedEventHandler,
) -> SsrEmitResult<String> {
    format_fitz_expr_scoped(
        expr,
        state_field_names,
        &["payload"],
        &component.name,
        &format!("event `{}` body RHS", event.name),
    )
}

// ---------------------------------------------------------------------------
// Template — HTML string emission
// ---------------------------------------------------------------------------

/// Recursively emit a template node into `out` as HTML source
/// text. State-field identifiers in interpolations get
/// rewritten with the `state.` prefix so classic Fitz's
/// string interpolation resolver finds them in the render
/// fn's scope.
///
/// 11.6.b MVP handles:
/// - `Text` — appended verbatim.
/// - `Interpolation { expr }` — `{state.<field>}` when the
///   inner expr is a bare state-field identifier; otherwise
///   the full expr source (also with state-field renaming).
/// - `Element { tag, attrs, children }` — with `@click`
///   attrs translated to `data-flv-click`, `{field}` interp
///   attrs rewritten with `state.`, and static attrs emitted
///   verbatim. HTML-escaped attribute values.
/// - Since Phase 11.6.d, `ChildComponent` — composed
///   inline as `Expr("<Child>_render(<Child> { <props>,
///   ... }).raw")`, where each declared child prop takes
///   the supplied literal (coerced to the field's declared
///   type) and every other child state field is omitted so
///   the struct literal picks up the child's declared
///   default. Reject with a same-file pointer when the
///   composed name is not declared in `siblings` — the
///   only cross-file resolution today lives at the module
///   loader layer (`.fitzv` sibling imported by
///   `main.fitz`).
/// - `If` / `For` — Since Phase 11.6.c continuation, lower to
///   Fitz if-as-expression / `__fitz_view_str_join(iter.map(...))`.
/// - `Slot` — rejected with a 11.7+ pointer.
fn emit_template_node_to_pieces(
    node: &ExpandedTemplateNode,
    state_field_names: &[&str],
    local_scope: &[&str],
    component: &ExpandedComponent,
    siblings: &[ExpandedComponent],
    pieces: &mut Vec<TemplatePiece>,
) -> SsrEmitResult<()> {
    match node {
        ExpandedTemplateNode::Text(s) => {
            push_text(pieces, s);
            Ok(())
        }
        ExpandedTemplateNode::Interpolation { expr, .. } => {
            let rendered = format_fitz_expr_scoped(
                expr,
                state_field_names,
                local_scope,
                &component.name,
                "template interpolation",
            )?;
            let mut wrapped = String::from("{");
            wrapped.push_str(&rendered);
            wrapped.push('}');
            push_text(pieces, &wrapped);
            Ok(())
        }
        ExpandedTemplateNode::Element {
            tag,
            attrs,
            children,
            self_closing,
            ..
        } => {
            let mut open = String::from("<");
            open.push_str(tag);
            push_text(pieces, &open);
            for attr in attrs {
                push_text(pieces, " ");
                emit_attr_to_pieces(attr, state_field_names, local_scope, component, pieces)?;
            }
            if *self_closing {
                push_text(pieces, " />");
                return Ok(());
            }
            push_text(pieces, ">");
            for child in children {
                emit_template_node_to_pieces(
                    child,
                    state_field_names,
                    local_scope,
                    component,
                    siblings,
                    pieces,
                )?;
            }
            let mut close = String::from("</");
            close.push_str(tag);
            close.push('>');
            push_text(pieces, &close);
            Ok(())
        }
        // Phase 11.6.c continuation — `{#if cond}...{/if}` and
        // `{#if cond}...{#else}...{/if}` lower to a Fitz
        // if-as-expression that yields a `Str`. Both branches
        // recursively emit pieces; each branch's pieces
        // serialise via `serialize_pieces_as_html_arg` so
        // nested directives work naturally.
        ExpandedTemplateNode::If {
            cond,
            children,
            else_children,
            ..
        } => {
            let cond_src = format_fitz_expr_scoped(
                cond,
                state_field_names,
                local_scope,
                &component.name,
                "template `{#if}` condition",
            )?;
            let mut then_pieces: Vec<TemplatePiece> = Vec::new();
            for child in children {
                emit_template_node_to_pieces(
                    child,
                    state_field_names,
                    local_scope,
                    component,
                    siblings,
                    &mut then_pieces,
                )?;
            }
            let then_src = serialize_pieces_as_html_arg(&then_pieces);
            let else_src = match else_children {
                Some(kids) => {
                    let mut ep: Vec<TemplatePiece> = Vec::new();
                    for child in kids {
                        emit_template_node_to_pieces(
                            child,
                            state_field_names,
                            local_scope,
                            component,
                            siblings,
                            &mut ep,
                        )?;
                    }
                    serialize_pieces_as_html_arg(&ep)
                }
                None => "\"\"".to_string(),
            };
            pieces.push(TemplatePiece::Expr(format!(
                "if ({cond_src}) {{ {then_src} }} else {{ {else_src} }}"
            )));
            Ok(())
        }
        // Phase 11.6.c continuation — `{#for x in xs}...{/for}`
        // lowers to `__fitz_view_str_join(<iter>.map(fn(x) =>
        // <body as Str>))`. The `x` binding shadows any
        // same-named state field inside the body via the
        // walker's `local_scope` param.
        ExpandedTemplateNode::For {
            var,
            iter,
            children,
            ..
        } => {
            let iter_src = format_fitz_expr_scoped(
                iter,
                state_field_names,
                local_scope,
                &component.name,
                "template `{#for}` iterable",
            )?;
            let mut new_scope: Vec<&str> = local_scope.to_vec();
            new_scope.push(var.as_str());
            let mut body_pieces: Vec<TemplatePiece> = Vec::new();
            for child in children {
                emit_template_node_to_pieces(
                    child,
                    state_field_names,
                    &new_scope,
                    component,
                    siblings,
                    &mut body_pieces,
                )?;
            }
            let body_src = serialize_pieces_as_html_arg(&body_pieces);
            pieces.push(TemplatePiece::Expr(format!(
                "__fitz_view_str_join({iter_src}.map(fn({var}) => {body_src}))"
            )));
            Ok(())
        }
        ExpandedTemplateNode::Slot { .. } => Err(SsrEmitError {
            message: "`<slot />` — fallback children composition is deferred to Phase \
                     11.7+ (composition wiring across parent/child)."
                .to_string(),
            context: format!("component `{}` template", component.name),
        }),
        // Phase 11.6.d — Same-file `<Child />` composition. Look
        // up the child in the sibling list, build a struct
        // literal with each supplied prop coerced to a Fitz
        // literal, and emit the piece as
        // `Expr("<Child>_render(<Child> { <props> }).raw")`. The
        // `.raw` unwraps the `Html { raw: Str }` value that
        // fitz-liveviews's `html(...)` constructor produced,
        // yielding the child's rendered HTML as a `Str` we splice
        // into the parent's chain-form html body.
        //
        // Cross-file composition (child declared in a DIFFERENT
        // `.fitzv` imported into main.fitz) requires the
        // expand/check pipeline to see the sibling — deferred
        // until 11.6.e wires the loader's expanded-file cache
        // through the checker.
        ExpandedTemplateNode::ChildComponent { name, props, .. } => {
            let child = siblings
                .iter()
                .find(|c| &c.name == name)
                .ok_or_else(|| SsrEmitError {
                    message: format!(
                        "child component composition `<{name} />` — component `{name}` \
                         is not declared in the same `.fitzv` file. Cross-file `<Child />` \
                         composition (importing `Comp.fitzv` from `main.fitz` and \
                         composing `<Comp />` inside another component's template) is \
                         deferred to Phase 11.6.e. For now, declare both parent and \
                         child in the same `.fitzv` module."
                    ),
                    context: format!("component `{}` template", component.name),
                })?;
            let expr_src = format_child_composition(child, props, component)?;
            pieces.push(TemplatePiece::Expr(expr_src));
            Ok(())
        }
    }
}

/// Emit a single attribute to HTML. Handles the three shapes:
/// - `Static { name, value }` — `<name>="<value>"` verbatim.
/// - `Interpolation { name, expr }` — `<name>="{state.<field>}"`
///   with the state-field rewrite.
/// - `Event { event_name, handler_name }` — translated to
///   `data-flv-<event>="<handler>"` per the fitz-liveviews
///   client runtime contract.
fn emit_attr_to_pieces(
    attr: &ExpandedAttr,
    state_field_names: &[&str],
    local_scope: &[&str],
    component: &ExpandedComponent,
    pieces: &mut Vec<TemplatePiece>,
) -> SsrEmitResult<()> {
    match attr {
        ExpandedAttr::Static { name, value, .. } => {
            // Escape `"` in value for the surrounding `"..."` in
            // the emitted HTML string.
            let escaped = value.replace('"', "\\\"");
            push_text(pieces, &format!("{name}=\"{escaped}\""));
            Ok(())
        }
        ExpandedAttr::Interpolation { name, expr, .. } => {
            let rendered = format_fitz_expr_scoped(
                expr,
                state_field_names,
                local_scope,
                &component.name,
                "template attribute interpolation",
            )?;
            push_text(pieces, &format!("{name}=\"{{{rendered}}}\""));
            Ok(())
        }
        ExpandedAttr::Event {
            event_name,
            handler_name,
            ..
        } => {
            push_text(pieces, &format!("data-flv-{event_name}=\"{handler_name}\""));
            Ok(())
        }
    }
}

// Note: `format_template_interpolation` (11.6.b's dedicated
// wrapper) was removed by 11.6.c continuation. The template
// interpolation call site in `emit_template_node_to_pieces`
// calls `format_fitz_expr_scoped` directly so it can pass the
// active `local_scope` (needed for `{#for x in xs}` bodies
// where `x` shadows any same-named state field). Downstream
// consumers that need the pre-`local_scope` wrapper can use
// `format_fitz_expr` (empty local scope).

// ---------------------------------------------------------------------------
// Type + default source formatting
// ---------------------------------------------------------------------------

/// Print a `TypeExpr` as classic Fitz source text. Mirrors
/// `TypeExpr::display_name` (which does the same job) but kept
/// local so future emit-specific tweaks don't affect other
/// consumers.
fn format_type_expr_source(ty: &crate::ast::TypeExpr) -> String {
    ty.display_name()
}

/// Escape `{` and `}` in a CSS body so they survive intact when
/// inlined into a classic Fitz `html("""...""")` string. Fitz's
/// string interpolation syntax `"{expr}"` would otherwise try to
/// parse CSS `.foo { color: red; }` as an interpolated
/// expression `{ color: red; }`. Fitz recognises `\{` and `\}`
/// as literal-brace escapes; this helper does the doubling
/// (backslash-brace).
fn escape_css_braces_for_fitz_interp(css: &str) -> String {
    let mut out = String::with_capacity(css.len() + 8);
    for ch in css.chars() {
        match ch {
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Phase 11.6.d — `<Child />` composition
// ---------------------------------------------------------------------------

/// Emit a `<Child />` composition site as a Fitz expression that
/// evaluates to `Str` for chain-form html arg splicing.
///
/// The shape is:
/// ```text
/// <Child>_render(<Child> { <prop1>: <lit1>, <prop2>: <lit2>, ... }).raw
/// ```
///
/// - `<Child>_render` is the render fn the SSR emitter generates
///   for the child component.
/// - The struct literal supplies each **explicitly-mentioned**
///   prop coerced to a Fitz literal. Every unmentioned state
///   field is omitted so the struct literal picks up the child
///   type's declared default. This matches classic Fitz's rule
///   for struct literals with defaults: unmentioned fields fall
///   back to `default:` if present, error if not (Phase 3.2).
/// - `.raw` unwraps `Html { raw: Str }` (fitz-liveviews's
///   canonical type) to a bare `Str` we can concat with the
///   surrounding html body.
///
/// The MVP delegates the "which types are legal for a prop"
/// question to Phase 11.5.d's [`super::check::coerce_child_prop_raw_value`]
/// via [`coerce_child_prop_raw_value_to_fitz_literal`], which
/// enforces the same primitive subset (Str / Int / Float / Bool
/// / `Nullable<T>` of a primitive).
fn format_child_composition(
    child: &ExpandedComponent,
    props: &[super::expand::ChildComponentProp],
    parent: &ExpandedComponent,
) -> SsrEmitResult<String> {
    let child_name = &child.name;
    let mut out = format!("{child_name}_render({child_name} {{");
    for (i, prop) in props.iter().enumerate() {
        let field = child
            .state
            .iter()
            .find(|f| f.name == prop.field_name)
            .ok_or_else(|| SsrEmitError {
                message: format!(
                    "internal error: prop `{}=\"{}\"` on `<{child_name} />` \
                         matches no declared state field of `{child_name}`. The \
                         checker should have caught this — please report a bug \
                         with the offending `.fitzv` source.",
                    prop.field_name, prop.raw_value
                ),
                context: format!("component `{}` template", parent.name),
            })?;
        let literal = coerce_child_prop_raw_value_to_fitz_literal(
            &prop.raw_value,
            &field.type_expr,
            child_name,
            &prop.field_name,
        )?;
        if i > 0 {
            out.push(',');
        }
        out.push(' ');
        out.push_str(&prop.field_name);
        out.push_str(": ");
        out.push_str(&literal);
    }
    // Trailing space keeps the emitted source pretty in both
    // shapes: `Child { }` (no props → all state fields use their
    // declared defaults) and `Child { count: 42, msg: "hi" }`
    // (some props supplied).
    out.push_str(" }).raw");
    Ok(out)
}

/// Parallel to [`super::check::coerce_child_prop_raw_value`]
/// (Phase 11.5.d) but returns a **classic Fitz literal** (source
/// text) instead of a Rust literal. Used by
/// [`format_child_composition`] to build the struct literal at
/// the `<Child />` composition site.
///
/// Same accepted subset:
/// - `Str` → `"<raw>"` with `"` and `\` escaped.
/// - `Int` → `<raw>` (parsed as `i64`).
/// - `Float` → `<raw>` (parsed as `f64`).
/// - `Bool` → `true` / `false`.
/// - `Nullable<T>` → `null` when raw is literally `null`,
///   otherwise recurse on the inner type.
///
/// Rejects nominals, generics (`List<T>` / `Map<K, V>`), and
/// function types with a targeted 11.7+ pointer — those need
/// richer static-prop coercion which the MVP defers.
pub(crate) fn coerce_child_prop_raw_value_to_fitz_literal(
    raw: &str,
    type_expr: &crate::ast::TypeExpr,
    child_name: &str,
    field_name: &str,
) -> SsrEmitResult<String> {
    use crate::ast::TypeExpr as T;
    match type_expr {
        T::Named(name) => match name.as_str() {
            "Str" => {
                let mut out = String::from("\"");
                for ch in raw.chars() {
                    match ch {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\r' => out.push_str("\\r"),
                        c => out.push(c),
                    }
                }
                out.push('"');
                Ok(out)
            }
            "Int" => raw
                .parse::<i64>()
                .map(|n| n.to_string())
                .map_err(|_| SsrEmitError {
                    message: format!(
                        "expected an integer literal for `Int` field `{field_name}` of \
                         `<{child_name} />`, got `{raw}`. Use a bare integer like \
                         `{field_name}=\"42\"`."
                    ),
                    context: format!("child composition `<{child_name} />`"),
                }),
            "Float" => raw
                .parse::<f64>()
                .map(|n| {
                    // Preserve `.0` for integer-valued floats so the
                    // classic Fitz parser reads it as `Float`, not
                    // `Int`.
                    if n.fract() == 0.0 && n.is_finite() {
                        format!("{n:.1}")
                    } else {
                        format!("{n}")
                    }
                })
                .map_err(|_| SsrEmitError {
                    message: format!(
                        "expected a float literal for `Float` field `{field_name}` of \
                         `<{child_name} />`, got `{raw}`. Use a decimal like \
                         `{field_name}=\"1.5\"`."
                    ),
                    context: format!("child composition `<{child_name} />`"),
                }),
            "Bool" => match raw {
                "true" => Ok("true".to_string()),
                "false" => Ok("false".to_string()),
                _ => Err(SsrEmitError {
                    message: format!(
                        "expected `\"true\"` or `\"false\"` for `Bool` field \
                         `{field_name}` of `<{child_name} />`, got `{raw}`."
                    ),
                    context: format!("child composition `<{child_name} />`"),
                }),
            },
            other => Err(SsrEmitError {
                message: format!(
                    "prop coercion to nominal `{other}` on `<{child_name} />` — \
                     static props for user-defined types are deferred to Phase \
                     11.7+ (needs richer prop coercion in the checker). Today's \
                     MVP accepts only `Str`, `Int`, `Float`, `Bool`, and their \
                     `Nullable<T>` wrappers."
                ),
                context: format!("child composition `<{child_name} />`"),
            }),
        },
        T::Nullable(inner) => {
            if raw == "null" {
                Ok("null".to_string())
            } else {
                coerce_child_prop_raw_value_to_fitz_literal(raw, inner, child_name, field_name)
            }
        }
        T::Generic { name, .. } => Err(SsrEmitError {
            message: format!(
                "prop coercion to `{name}<...>` on `<{child_name} />` — static props \
                 for compound types (`List`, `Map`, etc.) are deferred to Phase 11.7+. \
                 Today's MVP accepts only primitives + `Nullable<T>` primitives."
            ),
            context: format!("child composition `<{child_name} />`"),
        }),
        T::Function { .. } => Err(SsrEmitError {
            message: format!(
                "prop coercion to a function type on `<{child_name} />` — passing \
                 callbacks as props is deferred to Phase 11.7+ (event bubbling from \
                 children needs framework-level plumbing). Today's MVP accepts only \
                 primitives + `Nullable<T>` primitives."
            ),
            context: format!("child composition `<{child_name} />`"),
        }),
        T::Tuple(_) => Err(SsrEmitError {
            message: format!(
                "prop coercion to a tuple type on `<{child_name} />` — tuple props \
                 are deferred to Phase 11.7+. Today's MVP accepts only primitives + \
                 `Nullable<T>` primitives."
            ),
            context: format!("child composition `<{child_name} />`"),
        }),
    }
}

/// Print an `Expr` as classic Fitz source text. Thin wrapper
/// over [`format_fitz_expr`] with no state fields in scope —
/// suitable for state field defaults where referencing another
/// state field would be circular anyway.
///
/// 11.6.c widened the accepted shape from literal-only to the
/// full walker's grammar (BinOp / UnaryOp / Call / Field /
/// Index / StrInterp / List / Map / Range / Ok / Err /
/// FnExpr-arrow). Statement / control-flow shapes still
/// reject with 11.7+ pointers.
fn format_expr_source(
    expr: &Expr,
    component_name: &str,
    context_label: &str,
) -> SsrEmitResult<String> {
    format_fitz_expr(expr, &[], component_name, context_label)
}

// ---------------------------------------------------------------------------
// Phase 11.6.c — Expression → Fitz source walker
// ---------------------------------------------------------------------------

/// Print an `Expr` as classic Fitz source text, rewriting
/// bare state-field identifiers as `state.<field>` so the
/// emitted code resolves them against the render fn's / event
/// fn's parameter named `state`.
///
/// This is the shared workhorse for:
/// - Event body RHS expressions ([`format_event_rhs`]).
/// - Template `{expr}` interpolations
///   ([`format_template_interpolation`]).
/// - State field defaults ([`format_expr_source`], which
///   passes an empty `state_field_names` slice — defaults
///   cannot reference other state fields).
///
/// **11.6.c accepts** (in addition to 11.6.b's literals +
/// bare state ident):
/// - `BinOp` — always emitted with outer parentheses to
///   preserve precedence across arbitrary nesting.
/// - `UnaryOp` — `Neg`/`Not`/`BitNot`, again with outer
///   parens.
/// - `Call` — receiver + args recursively walked. Method
///   calls (`callee` is `Expr::Field`) work naturally
///   because `Field` is one of the walked variants.
/// - `Field` — `<obj>.<field>`. `state.count` on a
///   pre-rewritten ident is idempotent.
/// - `Index` — `<obj>[<index>]`.
/// - `StrInterp` — each `StrPart::Lit` copied verbatim,
///   each `StrPart::Expr` recursively walked. Interpolated
///   segments respect the state-field rewrite.
/// - `List` — `[<a>, <b>, ...]`.
/// - `Map` — `{<k>: <v>, ...}` — heterogeneous OK because
///   we emit source text, classic Fitz will type-check.
/// - `Range` — `<start>..<end>` or `..=` with inclusive flag.
/// - `Ok`/`Err` — Result constructors.
/// - `FnExpr` — arrow form only (single `Return` statement in
///   body). Multi-statement closures deferred to Phase 11.7+.
///
/// **11.6.c rejects** with a 11.7+ pointer:
/// - `Slice` (`xs[a..b]`) — rarely useful in the SSR path.
/// - `ListComp`/`MapComp` — the whole comprehension surface.
/// - `Tuple` — tuple types don't cross the fitz-liveviews API
///   boundary cleanly today.
/// - `If`/`Match` as an expression — control flow inside
///   RHS/interpolations lands with template directive support.
/// - `StructLit` — constructing new instances of state or
///   other types. 11.6.d for the state-mutation case.
/// - `Await`/`Try`/`NamedArg` — async and error propagation
///   both make no sense inside a synchronous render body.
/// - `Bytes` — not a template-friendly type.
/// - `Ident` for a NON-state field — could be a free variable
///   the render's enclosing scope resolves (e.g. a `let` in
///   the caller), but the SSR emitter cannot know without
///   scope analysis. Deferred to 11.7+ when the loader
///   integration lands.
/// - `Error` — the parser's error-recovery sentinel; should
///   never appear in a checked AST but reject defensively.
fn format_fitz_expr(
    expr: &Expr,
    state_field_names: &[&str],
    component_name: &str,
    context_label: &str,
) -> SsrEmitResult<String> {
    format_fitz_expr_scoped(expr, state_field_names, &[], component_name, context_label)
}

/// Inner walker that also tracks a `local_scope` of identifiers
/// introduced by enclosing `FnExpr`s (closure params). Ident
/// resolution priority:
///
/// 1. `local_scope` — closure params take precedence and emit
///    verbatim (bare identifier, no rewrite).
/// 2. `state_field_names` — declared state fields rewrite as
///    `state.<field>`.
/// 3. Everything else — reject with a 11.7+ pointer (needs
///    module-loader scope resolution to know if it's a free
///    var in the caller's scope).
///
/// Kept as an inner impl so the public [`format_fitz_expr`]
/// signature stays terse for the common case (no locals).
fn format_fitz_expr_scoped(
    expr: &Expr,
    state_field_names: &[&str],
    local_scope: &[&str],
    component_name: &str,
    context_label: &str,
) -> SsrEmitResult<String> {
    match expr {
        // ---- Literals ----
        Expr::Int(n, _) => Ok(format!("{n}")),
        Expr::Float(f, _) => Ok(format!("{f}")),
        Expr::Bool(b, _) => Ok(b.to_string()),
        Expr::Str(s, _) => Ok(format!("{s:?}")),
        Expr::Null(_) => Ok("null".to_string()),

        // ---- Ident (local-scope shadow > state-field rewrite) ----
        Expr::Ident(name, _) => {
            if local_scope.contains(&name.as_str()) {
                // Closure param shadows any same-named state
                // field. Emit verbatim.
                Ok(name.clone())
            } else if state_field_names.contains(&name.as_str()) {
                Ok(format!("state.{name}"))
            } else {
                Err(SsrEmitError {
                    message: format!(
                        "identifier `{name}` in {context_label} for component \
                         `{component_name}` is not a declared state field. Free-var \
                         references need the module loader's scope resolution — \
                         deferred to Phase 11.7+."
                    ),
                    context: format!("component `{component_name}` {context_label}"),
                })
            }
        }

        // ---- BinOp / UnaryOp — outer parens for precedence safety ----
        Expr::BinOp {
            op, left, right, ..
        } => {
            let l = format_fitz_expr_scoped(
                left,
                state_field_names,
                local_scope,
                component_name,
                context_label,
            )?;
            let r = format_fitz_expr_scoped(
                right,
                state_field_names,
                local_scope,
                component_name,
                context_label,
            )?;
            let op_str = format_binop_source(op);
            Ok(format!("({l} {op_str} {r})"))
        }
        Expr::UnaryOp { op, operand, .. } => {
            let inner = format_fitz_expr_scoped(
                operand,
                state_field_names,
                local_scope,
                component_name,
                context_label,
            )?;
            let op_str = format_unaryop_source(op);
            Ok(format!("({op_str}{inner})"))
        }

        // ---- Call / Field / Index — recursive walk ----
        Expr::Call { callee, args, .. } => {
            let callee_src = format_fitz_expr_scoped(
                callee,
                state_field_names,
                local_scope,
                component_name,
                context_label,
            )?;
            let mut arg_srcs: Vec<String> = Vec::with_capacity(args.len());
            for a in args {
                arg_srcs.push(format_fitz_expr_scoped(
                    a,
                    state_field_names,
                    local_scope,
                    component_name,
                    context_label,
                )?);
            }
            Ok(format!("{callee_src}({})", arg_srcs.join(", ")))
        }
        Expr::Field { object, field, .. } => {
            let obj_src = format_fitz_expr_scoped(
                object,
                state_field_names,
                local_scope,
                component_name,
                context_label,
            )?;
            Ok(format!("{obj_src}.{field}"))
        }
        Expr::Index { object, index, .. } => {
            let obj_src = format_fitz_expr_scoped(
                object,
                state_field_names,
                local_scope,
                component_name,
                context_label,
            )?;
            let idx_src = format_fitz_expr_scoped(
                index,
                state_field_names,
                local_scope,
                component_name,
                context_label,
            )?;
            Ok(format!("{obj_src}[{idx_src}]"))
        }

        // ---- StrInterp — walk each interpolated part ----
        Expr::StrInterp(parts, _) => {
            let mut out = String::from("\"");
            for part in parts {
                match part {
                    crate::ast::StrPart::Lit(s) => {
                        // Escape `"` and `\` for the surrounding `"..."`.
                        for ch in s.chars() {
                            match ch {
                                '"' => out.push_str("\\\""),
                                '\\' => out.push_str("\\\\"),
                                c => out.push(c),
                            }
                        }
                    }
                    crate::ast::StrPart::Expr(e, _fmt) => {
                        // Format specs (FormatSpec) are deferred: the
                        // 11.6.c walker doesn't try to preserve
                        // `{x:0.2f}` shape — that surface is rarely
                        // used in template interpolations and would
                        // need dedicated formatting.
                        let inner = format_fitz_expr_scoped(
                            e,
                            state_field_names,
                            local_scope,
                            component_name,
                            context_label,
                        )?;
                        out.push('{');
                        out.push_str(&inner);
                        out.push('}');
                    }
                }
            }
            out.push('"');
            Ok(out)
        }

        // ---- List / Map / Range ----
        Expr::List(items, _) => {
            let mut srcs = Vec::with_capacity(items.len());
            for it in items {
                srcs.push(format_fitz_expr_scoped(
                    it,
                    state_field_names,
                    local_scope,
                    component_name,
                    context_label,
                )?);
            }
            Ok(format!("[{}]", srcs.join(", ")))
        }
        Expr::Map(entries, _) => {
            let mut srcs = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                let ks = format_fitz_expr_scoped(
                    k,
                    state_field_names,
                    local_scope,
                    component_name,
                    context_label,
                )?;
                let vs = format_fitz_expr_scoped(
                    v,
                    state_field_names,
                    local_scope,
                    component_name,
                    context_label,
                )?;
                srcs.push(format!("{ks}: {vs}"));
            }
            Ok(format!("{{{}}}", srcs.join(", ")))
        }
        Expr::Range {
            start,
            end,
            inclusive,
            ..
        } => {
            let start_src = format_fitz_expr_scoped(
                start,
                state_field_names,
                local_scope,
                component_name,
                context_label,
            )?;
            let end_src = format_fitz_expr_scoped(
                end,
                state_field_names,
                local_scope,
                component_name,
                context_label,
            )?;
            let op = if *inclusive { "..=" } else { ".." };
            Ok(format!("{start_src}{op}{end_src}"))
        }

        // ---- Ok / Err constructors ----
        Expr::Ok(inner, _) => {
            let inner_src = format_fitz_expr_scoped(
                inner,
                state_field_names,
                local_scope,
                component_name,
                context_label,
            )?;
            Ok(format!("Ok({inner_src})"))
        }
        Expr::Err(inner, _) => {
            let inner_src = format_fitz_expr_scoped(
                inner,
                state_field_names,
                local_scope,
                component_name,
                context_label,
            )?;
            Ok(format!("Err({inner_src})"))
        }

        // ---- FnExpr — arrow form only ----
        Expr::FnExpr {
            params,
            body,
            is_async,
            ..
        } => {
            if *is_async {
                return Err(SsrEmitError {
                    message: format!(
                        "async closure inside {context_label} for component `{component_name}` — \
                         async makes no sense inside a synchronous render body. Deferred to \
                         Phase 11.7+."
                    ),
                    context: format!("component `{component_name}` {context_label}"),
                });
            }
            // Only the arrow form (single `Return` stmt) is
            // supported. Multi-stmt closure bodies would need
            // the statement lowerer.
            if body.len() != 1 {
                return Err(SsrEmitError {
                    message: format!(
                        "closure inside {context_label} for component `{component_name}` has a \
                         multi-statement body — 11.6.c MVP only supports the arrow form \
                         (`fn(x) => <expr>`). Multi-statement closures deferred to Phase 11.7+."
                    ),
                    context: format!("component `{component_name}` {context_label}"),
                });
            }
            // Extend the local scope with this closure's params
            // so bare `Ident` refs inside the body emit verbatim
            // (rather than being rewritten as `state.<name>` or
            // rejected as free vars).
            let mut new_scope: Vec<&str> = local_scope.to_vec();
            for p in params {
                new_scope.push(p.name.as_str());
            }
            let inner = match &body[0] {
                Stmt::Return(e, _) => format_fitz_expr_scoped(
                    e,
                    state_field_names,
                    &new_scope,
                    component_name,
                    context_label,
                )?,
                _ => {
                    return Err(SsrEmitError {
                        message: format!(
                            "closure inside {context_label} for component `{component_name}` has \
                             a non-return statement — 11.6.c MVP only supports the arrow form \
                             (`fn(x) => <expr>`). Deferred to Phase 11.7+."
                        ),
                        context: format!("component `{component_name}` {context_label}"),
                    });
                }
            };
            let params_src: Vec<String> = params
                .iter()
                .map(|p| match &p.type_ {
                    Some(ty) => format!("{}: {}", p.name, ty.display_name()),
                    None => p.name.clone(),
                })
                .collect();
            Ok(format!("fn({}) => {inner}", params_src.join(", ")))
        }

        // ---- Explicit rejections with targeted pointers ----
        Expr::Try(_, _)
        | Expr::Await(_, _)
        | Expr::NamedArg { .. }
        | Expr::Match { .. }
        | Expr::If { .. }
        | Expr::StructLit { .. }
        | Expr::Slice { .. }
        | Expr::ListComp { .. }
        | Expr::MapComp { .. }
        | Expr::Bytes(_, _) => Err(SsrEmitError {
            message: format!(
                "{context_label} for component `{component_name}` uses an expression \
                 shape (`{}`) that the 11.6.c SSR walker does not yet handle. Deferred \
                 to Phase 11.7+ (or 11.6.d for struct-literal state constructors).",
                expr_kind_label(expr)
            ),
            context: format!("component `{component_name}` {context_label}"),
        }),
        Expr::Error(_) => Err(SsrEmitError {
            message: format!(
                "{context_label} for component `{component_name}` contains an `Expr::Error` \
                 recovery sentinel — the parser did not produce a valid AST. Fix upstream \
                 parse errors first."
            ),
            context: format!("component `{component_name}` {context_label}"),
        }),
        _ => Err(SsrEmitError {
            message: format!(
                "{context_label} for component `{component_name}` uses an expression \
                 shape not covered by the 11.6.c SSR walker. Deferred to Phase 11.7+."
            ),
            context: format!("component `{component_name}` {context_label}"),
        }),
    }
}

fn format_binop_source(op: &crate::ast::BinOpKind) -> &'static str {
    use crate::ast::BinOpKind::*;
    match op {
        Add => "+",
        Sub => "-",
        Mul => "*",
        Div => "/",
        Mod => "%",
        Eq => "==",
        NotEq => "!=",
        Lt => "<",
        LtEq => "<=",
        Gt => ">",
        GtEq => ">=",
        And => "and",
        Or => "or",
        Xor => "xor",
        BitAnd => "&",
        BitOr => "|",
        BitXor => "^",
        Shl => "<<",
        Shr => ">>",
    }
}

fn format_unaryop_source(op: &crate::ast::UnaryOpKind) -> &'static str {
    use crate::ast::UnaryOpKind::*;
    match op {
        Neg => "-",
        Not => "not ",
        BitNot => "~",
    }
}

fn expr_kind_label(expr: &Expr) -> &'static str {
    match expr {
        Expr::Try(_, _) => "Try (`?`)",
        Expr::Await(_, _) => "Await (`.await`)",
        Expr::NamedArg { .. } => "NamedArg (`name: value`)",
        Expr::Match { .. } => "Match",
        Expr::If { .. } => "If-as-expression",
        Expr::StructLit { .. } => "StructLit (`Type { field: value }`)",
        Expr::Slice { .. } => "Slice (`xs[a..b]`)",
        Expr::ListComp { .. } => "ListComp",
        Expr::MapComp { .. } => "MapComp",
        Expr::Bytes(_, _) => "Bytes literal",
        _ => "unsupported",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{expand, parse};

    fn parse_expand(src: &str) -> ExpandedViewFile {
        let raw = parse(src).expect("view::parse");
        expand(&raw).expect("view::expand")
    }

    // ---- Header ----------------------------------------------

    #[test]
    fn emit_module_header_imports_html_and_html_ctor() {
        let file = parse_expand("component Empty { state {} <template><div>hi</div></template> }");
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("from fitz_liveviews import Html, html"),
            "header missing fitz_liveviews import:\n{out}"
        );
    }

    // ---- State type + @live_component ------------------------

    #[test]
    fn emit_state_type_carries_live_component_decorator_and_fields() {
        let file = parse_expand(
            "component MetricTile { state { count: Int = 0 } <template><div>{count}</div></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("@live_component(\"MetricTile\")"),
            "missing @live_component decorator:\n{out}"
        );
        assert!(
            out.contains("type MetricTile {"),
            "missing type declaration:\n{out}"
        );
        assert!(
            out.contains("count: Int = 0"),
            "missing state field:\n{out}"
        );
    }

    #[test]
    fn emit_state_type_supports_multiple_fields_of_different_primitives() {
        let src = r#"component Multi {
  state {
    n: Int = 0
    msg: Str = "hi"
    flag: Bool = false
  }
  <template><div>{n}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(out.contains("n: Int = 0"));
        assert!(out.contains("msg: Str = \"hi\""));
        assert!(out.contains("flag: Bool = false"));
    }

    // ---- Render fn -------------------------------------------

    #[test]
    fn emit_render_fn_uses_component_name_snake_style_suffix() {
        let file = parse_expand(
            "component MetricTile { state { count: Int = 0 } <template><span>{count}</span></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("@render_for(\"MetricTile\")"),
            "missing @render_for decorator:\n{out}"
        );
        assert!(
            out.contains("fn MetricTile_render(state: MetricTile) -> Html {"),
            "render fn signature wrong:\n{out}"
        );
        assert!(
            out.contains("return html(\"\"\""),
            "render fn should use triple-quote html() call:\n{out}"
        );
    }

    #[test]
    fn emit_render_fn_rewrites_field_interpolation_with_state_prefix() {
        let file = parse_expand(
            "component X { state { count: Int = 0 } <template><span>{count}</span></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("<span>{state.count}</span>"),
            "`{{count}}` must be rewritten as `{{state.count}}`:\n{out}"
        );
    }

    #[test]
    fn emit_render_fn_translates_click_attr_to_data_flv_click() {
        let file = parse_expand(
            "component X { state {} <template><button @click=\"tap\">bump</button></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains(r#"data-flv-click="tap""#),
            "`@click` must translate to `data-flv-click`:\n{out}"
        );
        assert!(
            !out.contains("@click="),
            "raw `@click=` must NOT appear in the emitted HTML:\n{out}"
        );
    }

    #[test]
    fn emit_render_fn_carries_static_attrs_verbatim() {
        let file = parse_expand(
            "component X { state {} <template><div class=\"card\" id=\"main\"><span>hi</span></div></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(out.contains(r#"class="card""#));
        assert!(out.contains(r#"id="main""#));
    }

    // ---- Event fn --------------------------------------------

    #[test]
    fn emit_event_fn_signature_matches_fitz_liveviews_contract() {
        let file = parse_expand(
            "component X { state { n: Int = 0 } event reset() { n = 0 } <template><div>{n}</div></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains(r#"@on("X", "reset")"#),
            "missing @on decorator:\n{out}"
        );
        assert!(
            out.contains("fn X_reset(state: X, payload: Map<Str, Str>) -> X {"),
            "event fn signature wrong:\n{out}"
        );
    }

    #[test]
    fn emit_event_fn_returns_fresh_struct_literal_with_all_fields() {
        // Two state fields, one event mutates only ONE — the
        // emitted body must return a struct literal that
        // carries both fields (the untouched one via
        // `state.<field>`).
        let src = r#"component X {
  state {
    n: Int = 0
    msg: Str = "hi"
  }
  event bump() { n = 5 }
  <template><div>{n}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("return X {"),
            "must build a fresh struct literal:\n{out}"
        );
        assert!(
            out.contains("n: 5,"),
            "mutated field must use the assigned value:\n{out}"
        );
        assert!(
            out.contains("msg: state.msg,"),
            "untouched field must carry over from `state.<field>`:\n{out}"
        );
    }

    #[test]
    fn emit_event_fn_supports_multiple_mutations_last_write_wins() {
        // Two events with different subsets; also test that
        // an event body with N mutations does the right thing.
        let src = r#"component X {
  state {
    a: Int = 0
    b: Int = 0
  }
  event set_both() {
    a = 1
    b = 2
  }
  <template><div>{a}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(out.contains("a: 1,"));
        assert!(out.contains("b: 2,"));
    }

    #[test]
    fn emit_event_fn_rewrites_bare_state_field_rhs_as_state_prefix() {
        // `a = b` where both are state fields — the RHS `b` must
        // rewrite to `state.b`.
        let src = r#"component X {
  state {
    a: Int = 0
    b: Int = 42
  }
  event copy_b_to_a() { a = b }
  <template><div>{a}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("a: state.b,"),
            "bare RHS `b` must rewrite as `state.b`:\n{out}"
        );
    }

    // Phase 11.6.e — the emitted @on signature has `payload:
    // Map<Str, Str>`, so event body RHS is allowed to read from
    // it directly. The walker's local_scope now contains
    // "payload" so bare `payload["key"]` / `payload.has(...)`
    // pass without the "not a declared state field" rejection.
    #[test]
    fn phase_11_6_e_emit_accepts_payload_index_access_in_event_body_rhs() {
        // Payload indexing survives the walker unchanged (Index
        // over Ident where Ident is in local_scope emits as the
        // bare Fitz source).
        let src = r#"component Widget {
  state { title: Str = "" }
  event set_title() { title = payload["text"] }
  <template><span>{title}</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("title: payload[\"text\"],"),
            "expected `title: payload[\"text\"],` in emitted event fn:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_e_emit_accepts_payload_method_call_in_event_body_rhs() {
        // Method call on payload (`payload.get("author")` style)
        // walks as Call(Field(payload, "get"), ["author"]). The
        // Field object walk resolves `payload` via local_scope.
        // Assigning the result to a `Str` state field via a full
        // string interp (matches how the chat example composes
        // an author-tagged message).
        let src = r#"component Notes {
  state { last_author: Str = "" }
  event tag() { last_author = payload["author"] }
  <template><span>{last_author}</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("last_author: payload[\"author\"],"),
            "expected `last_author: payload[\"author\"],` in emitted source:\n{out}"
        );
    }

    // ---- Rejections (MVP scope guards) -----------------------

    // Phase 11.6.c widened the RHS grammar. `count = count + 1`
    // now emits `count: (state.count + 1i64)` (sort of — the
    // literal is `1` not `1i64` in Fitz source). The old
    // rejection test is inverted below to a positive check.
    #[test]
    fn phase_11_6_c_emit_accepts_binop_rhs_in_event_body() {
        let src = r#"component X {
  state { count: Int = 0 }
  event bump() { count = count + 1 }
  <template><div>{count}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // BinOp lowered with outer parens for precedence safety.
        assert!(
            out.contains("count: (state.count + 1),"),
            "expected `count: (state.count + 1),` in emitted source:\n{out}"
        );
    }

    // Phase 11.6.c inlined `<style scoped>` as a `<style>` tag
    // at the top of the render output. The old rejection is
    // replaced by a positive check.
    #[test]
    fn phase_11_6_c_emit_inlines_scoped_style_at_top_of_render_body() {
        let src = r#"component X {
  state {}
  <template><div>hi</div></template>
  <style scoped>
    .foo { color: red; }
  </style>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // The scoped CSS body carries a `-<scope>` class
        // suffix from 11.3.b's `apply_scope`. Between the
        // `<style>` tag we always emit and that suffix, we
        // can grep-match a stable substring.
        assert!(
            out.contains("<style>"),
            "expected inline `<style>` tag:\n{out}"
        );
        assert!(
            out.contains("</style>"),
            "expected closing `</style>` tag:\n{out}"
        );
        // The scoped CSS body should appear before the template
        // content in the emitted HTML string.
        let style_pos = out.find("<style>").unwrap();
        let template_pos = out.find("<div>hi</div>").unwrap();
        assert!(
            style_pos < template_pos,
            "style tag must appear before template content"
        );
    }

    #[test]
    fn phase_11_6_c_emit_inlines_global_style_with_escaped_braces() {
        let src = r#"component X {
  state {}
  <template><div>hi</div></template>
  <style global>
    body { margin: 0; }
  </style>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(out.contains("<style>"));
        // Global CSS body is passed through with `{` / `}`
        // escaped to `\{` / `\}` so classic Fitz's string
        // interpolation doesn't try to parse the CSS as
        // `{expr}`. See `escape_css_braces_for_fitz_interp`.
        assert!(
            out.contains(r"body \{ margin: 0; \}"),
            "global CSS body must appear with escaped braces:\n{out}"
        );
    }

    // Phase 11.6.c continuation — `{#if}` and `{#for}` now
    // lower to Fitz expression pieces. Old rejection tests
    // inverted below to positive checks. See
    // `phase_11_6_c_cont_*` further down for the detailed
    // shape assertions.
    #[test]
    fn phase_11_6_c_cont_emit_accepts_if_directive() {
        let src = r#"component X {
  state { flag: Bool = false }
  <template><div>{#if flag}<span>yes</span>{/if}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // The `{#if}` directive emits as an if-as-expression
        // piece in the chain-form html(...) call.
        assert!(
            out.contains("if (state.flag)"),
            "expected `if (state.flag)` in emit:\n{out}"
        );
        // Then-branch is a triple-string of the rendered
        // children; else-branch is empty string.
        assert!(
            out.contains("<span>yes</span>"),
            "then-branch content missing:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_c_cont_emit_accepts_for_directive() {
        let src = r#"component X {
  state { xs: List<Int> = [] }
  <template><ul>{#for x in xs}<li>{x}</li>{/for}</ul></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // The `{#for}` directive emits as
        // `__fitz_view_str_join(<iter>.map(fn(x) => <body>))`
        // — verified below.
        assert!(
            out.contains("__fitz_view_str_join(state.xs.map(fn(x) =>"),
            "expected join+map lowering:\n{out}"
        );
        // The `x` binding shadows any same-named state field
        // inside the body: `{x}` stays as `{x}` (not
        // `{state.x}`) inside the closure.
        assert!(
            out.contains("<li>{x}</li>"),
            "closure body must use bare `x`, not `state.x`:\n{out}"
        );
    }

    // Phase 11.6.d — `<Child />` composition (same-file MVP)
    // ---------------------------------------------------------

    #[test]
    fn phase_11_6_d_emit_composes_same_file_child_with_no_props() {
        // Baseline: `<Child />` inside `Parent` lowers to an
        // Expr piece that calls `Child_render(Child { }).raw`
        // and splices the result into the parent's html chain.
        let src = r#"component Parent {
  state {}
  <template><Child /></template>
}
component Child {
  state {}
  <template><span>hi</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).expect("same-file <Child /> composes");
        assert!(
            out.contains("Child_render(Child { }).raw"),
            "parent must call child_render on an empty struct literal:\n{out}"
        );
        // Child_render itself is a fully-emitted render fn.
        assert!(
            out.contains("fn Child_render(state: Child) -> Html {"),
            "child render fn must be emitted:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_d_emit_composes_same_file_child_with_primitive_props() {
        // Props coerce to Fitz literals: Str stays quoted, Int
        // bare, Float keeps `.0`, Bool bare.
        let src = r#"component Parent {
  state {}
  <template>
    <Card title="hello" count="42" rate="1.5" active="true" />
  </template>
}
component Card {
  state { title: Str = "", count: Int = 0, rate: Float = 0.0, active: Bool = false }
  <template><div>{title}: {count}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).expect("prop coercion accepts primitives");
        assert!(
            out.contains(
                "Card_render(Card { title: \"hello\", count: 42, rate: 1.5, active: true }).raw"
            ),
            "child composition must supply each prop as a Fitz literal:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_d_emit_omits_undeclared_props_so_defaults_apply() {
        // Only `title` supplied — `count` defaults to `0`. The
        // emitted struct literal supplies title only, letting
        // classic Fitz's default-application kick in.
        let src = r#"component Parent {
  state {}
  <template><Card title="hi" /></template>
}
component Card {
  state { title: Str = "", count: Int = 7 }
  <template><span>{title}({count})</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).expect("undeclared props use defaults");
        assert!(
            out.contains("Card_render(Card { title: \"hi\" }).raw"),
            "only supplied props must appear in the struct literal:\n{out}"
        );
        assert!(
            !out.contains("count: 0"),
            "un-supplied `count` must NOT appear (default kicks in):\n{out}"
        );
    }

    #[test]
    fn phase_11_6_d_emit_rejects_child_declared_in_a_different_file() {
        // The single-component convenience wrapper does not see
        // siblings, so a `<Child />` reference resolves to only
        // the parent itself → not found → clear cross-file error
        // pointing at 11.6.e.
        let parent_src = r#"component Parent {
  state {}
  <template><Comp /></template>
}"#;
        let file = parse_expand(parent_src);
        let err = emit_module_ssr(&file).unwrap_err();
        assert!(
            err.message.contains("<Comp"),
            "error must cite the child tag:\n{}",
            err.message
        );
        assert!(
            err.message.contains("11.6.e"),
            "error must point to Phase 11.6.e for cross-file:\n{}",
            err.message
        );
    }

    #[test]
    fn phase_11_6_d_emit_rejects_nullable_str_null_literal_and_string_prop() {
        // `Nullable<Str>` with raw `"null"` lowers to Fitz
        // `null`; any other value recurses on `Str` → quoted.
        let src = r#"component Parent {
  state {}
  <template>
    <Widget name="null" other="present" />
  </template>
}
component Widget {
  state { name: Str? = null, other: Str? = null }
  <template><span>{name}</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).expect("Nullable primitives coerce");
        assert!(
            out.contains("Widget_render(Widget { name: null, other: \"present\" }).raw"),
            "nullable str: `null` bare, other value quoted:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_d_composition_forces_chain_form_html_arg() {
        // A pure text template uses pretty triple-string form;
        // adding `<Child />` introduces an Expr piece which
        // forces chain form.
        let src = r#"component Parent {
  state {}
  <template>hello <Child /> world</template>
}
component Child {
  state {}
  <template><i>x</i></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).expect("composes");
        assert!(
            out.contains("return html(\"hello \" + (Child_render(Child { }).raw) + \" world\")"),
            "parent render must use chain form when a child is composed:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_d_module_with_composition_round_trips_through_classic_fitz() {
        // Sanity: the composed output lexes + parses cleanly
        // through classic Fitz, just like every other emitted
        // module. The check is best-effort (the emitted source
        // still needs `Html`/`html` from `fitz_liveviews` to
        // typecheck end-to-end).
        let src = r#"component Parent {
  state {}
  <template><div><Card title="hi" count="7" /></div></template>
}
component Card {
  state { title: Str = "", count: Int = 0 }
  <template><span>{title} ({count})</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).expect("composes");
        let tokens = crate::lexer::tokenize(&out)
            .unwrap_or_else(|e| panic!("emitted source failed classic lex: {e}\n\nSource:\n{out}"));
        crate::parser::parse(tokens).unwrap_or_else(|e| {
            panic!("emitted source failed classic parse: {e}\n\nSource:\n{out}")
        });
    }

    #[test]
    fn emit_rejects_slot_citing_11_7() {
        let src = r#"component X {
  state {}
  <template><slot /></template>
}"#;
        let file = parse_expand(src);
        let err = emit_module_ssr(&file).unwrap_err();
        assert!(err.message.contains("slot"), "msg: {}", err.message);
        assert!(err.message.contains("11.7"), "msg: {}", err.message);
    }

    #[test]
    fn emit_rejects_non_state_ident_in_template_interpolation() {
        // `{other}` where `other` is not a state field. Phase
        // 11.6.c's walker rejects with a 11.7+ pointer (needs
        // module-loader scope resolution to know if it's a
        // free variable in the caller's scope).
        let src = r#"component X {
  state { count: Int = 0 }
  <template><span>{other}</span></template>
}"#;
        let file = parse_expand(src);
        let err = emit_module_ssr(&file).unwrap_err();
        assert!(
            err.message.contains("not a declared state field"),
            "msg: {}",
            err.message
        );
        assert!(err.message.contains("11.7"), "msg: {}", err.message);
    }

    // ---- End-to-end round-trip -------------------------------

    #[test]
    fn emit_output_round_trips_through_classic_fitz_lexer_and_parser() {
        // The emitted Fitz source must lex + parse cleanly.
        // This is the acceptance criterion for 11.6.b: the SSR
        // emitter produces code that CLASSIC Fitz accepts as
        // input. Type-checker validation is a bonus — the
        // emitted references to fitz_liveviews types would
        // fail without the dep loaded, so we ONLY validate
        // lex + parse here.
        let src = r#"component Score {
  state { points: Int = 0 }
  event zero() { points = 0 }
  event ten() { points = 10 }
  <template>
    <div>
      <span>{points}</span>
      <button @click="zero">zero</button>
      <button @click="ten">set 10</button>
    </div>
  </template>
}"#;
        let file = parse_expand(src);
        let emitted = emit_module_ssr(&file).unwrap();

        let tokens = crate::lexer::tokenize(&emitted).unwrap_or_else(|e| {
            panic!("emitted source failed to lex:\n{emitted}\n--- err ---\n{e}")
        });
        let _program = crate::parser::parse(tokens).unwrap_or_else(|e| {
            panic!("emitted source failed to parse:\n{emitted}\n--- err ---\n{e}")
        });
    }

    #[test]
    fn emit_component_single_matches_module_wrapper() {
        // `emit_component_ssr` (single) and `emit_module_ssr`
        // (with a 1-component file) produce the same output —
        // the module entry point is just a convenience wrapper.
        let src = "component X { state { n: Int = 0 } <template><div>{n}</div></template> }";
        let file = parse_expand(src);
        let via_module = emit_module_ssr(&file).unwrap();
        let via_component = emit_component_ssr(&file.components[0]).unwrap();
        assert_eq!(via_module, via_component);
    }

    // ---- Phase 11.6.c — widened grammar coverage -------------

    #[test]
    fn phase_11_6_c_emit_accepts_arithmetic_rhs_multiple_ops() {
        let src = r#"component X {
  state {
    a: Int = 0
    b: Int = 0
  }
  event compute() {
    a = a + b * 2
    b = a - 1
  }
  <template><div>{a}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // Precedence-preserving parens on every BinOp node.
        assert!(
            out.contains("a: (state.a + (state.b * 2)),"),
            "expected precedence-preserving parens:\n{out}"
        );
        assert!(out.contains("b: (state.a - 1),"));
    }

    #[test]
    fn phase_11_6_c_emit_accepts_str_interp_rhs_with_field_rewrite() {
        let src = r#"component X {
  state {
    count: Int = 0
    msg: Str = ""
  }
  event describe() { msg = "count is {count}" }
  <template><div>{msg}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // The `{count}` inside the interpolation must rewrite
        // to `{state.count}`.
        assert!(
            out.contains("msg: \"count is {state.count}\","),
            "expected StrInterp with state-field rewrite:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_c_emit_accepts_method_call_rhs() {
        // The view lexer accepts `.` inside event body raw-capture
        // since the 11.6.c continuation (Token::Dot). This test
        // exercises the SSR walker's method-call support via a
        // natural `.fitzv` source — the AST-construction dance is
        // no longer required.
        let src = r#"component X {
  state { msg: Str = "hi" }
  event shout() { msg = msg.upper() }
  <template><div>{msg}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("msg: state.msg.upper(),"),
            "expected method call in emitted source:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_c_emit_accepts_richer_template_interpolation() {
        let src = r#"component X {
  state { count: Int = 0 }
  <template><span>Count is {count + 1}</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // `{count + 1}` in the template becomes `{(state.count + 1)}`
        // inside the emitted HTML string.
        assert!(
            out.contains("Count is {(state.count + 1)}"),
            "expected rich template interp:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_c_emit_accepts_field_access_in_template_interp() {
        let src = r#"component X {
  state { name: Str = "world" }
  <template><span>Hi {name.upper()}</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("Hi {state.name.upper()}"),
            "expected method call on state field:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_c_emit_accepts_arrow_closure_rhs() {
        // Same story as the method-call test: since the 11.6.c
        // continuation added Token::Dot to the view lexer, we
        // can exercise the closure lowering via a natural `.fitzv`
        // source (no AST-construction dance).
        let src = r#"component X {
  state { xs: List<Int> = [] }
  event bump_all() { xs = xs.map(fn(x) => x + 1) }
  <template><div>hi</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("xs: state.xs.map(fn(x) => (x + 1)),"),
            "expected arrow closure passed to .map():\n{out}"
        );
    }

    #[test]
    fn phase_11_6_c_cont_module_header_emits_str_join_helper() {
        // The `__fitz_view_str_join(xs: List<Str>) -> Str`
        // helper is prepended to every emitted module (dead
        // code when unused). Verified once here so the shape
        // is regression-guarded.
        let src = "component X { state {} <template><div>hi</div></template> }";
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("fn __fitz_view_str_join(xs: List<Str>) -> Str {"),
            "join helper signature missing:\n{out}"
        );
        assert!(
            out.contains("for x in xs"),
            "join helper loop missing:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_c_round_trip_end_to_end_with_widened_grammar() {
        // Real fixture exercising the widened grammar end-to-end.
        // Must lex + parse through classic Fitz cleanly.
        let src = r#"component Counter {
  state {
    count: Int = 0
    label: Str = "Count"
  }
  event increment() { count = count + 1 }
  event decrement() { count = count - 1 }
  event reset() { count = 0 }
  <template>
    <div class="counter">
      <span class="label">{label}: {count}</span>
      <button @click="decrement">-</button>
      <button @click="increment">+</button>
      <button @click="reset">reset</button>
    </div>
  </template>
  <style scoped>
    .counter { display: flex; gap: 8px; }
  </style>
}"#;
        let file = parse_expand(src);
        let emitted = emit_module_ssr(&file).unwrap();

        let tokens = crate::lexer::tokenize(&emitted).unwrap_or_else(|e| {
            panic!("emitted source failed to lex:\n{emitted}\n--- err ---\n{e}")
        });
        let _program = crate::parser::parse(tokens).unwrap_or_else(|e| {
            panic!("emitted source failed to parse:\n{emitted}\n--- err ---\n{e}")
        });

        // Sanity: BinOp arithmetic emitted correctly.
        assert!(emitted.contains("count: (state.count + 1),"));
        assert!(emitted.contains("count: (state.count - 1),"));
        // Sanity: `{label}: {count}` template interp with both
        // state fields rewritten.
        assert!(emitted.contains("{state.label}: {state.count}"));
        // Sanity: scoped `<style>` inline at top of render.
        assert!(emitted.contains("<style>"));
    }

    // ---- Phase 11.6.c continuation — template directives ----

    #[test]
    fn phase_11_6_c_cont_emit_if_with_else_branch() {
        let src = r#"component X {
  state { flag: Bool = false }
  <template>{#if flag}<span>on</span>{#else}<span>off</span>{/if}</template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(out.contains("if (state.flag)"), "cond missing:\n{out}");
        assert!(out.contains("<span>on</span>"), "then missing:\n{out}");
        assert!(out.contains("<span>off</span>"), "else missing:\n{out}");
    }

    #[test]
    fn phase_11_6_c_cont_emit_if_without_else_uses_empty_string() {
        let src = r#"component X {
  state { flag: Bool = false }
  <template>{#if flag}<span>on</span>{/if}</template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // No `{#else}` → else-branch is empty string literal.
        assert!(
            out.contains("} else { \"\" }"),
            "expected `else {{ \"\" }}` for no-else case:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_c_cont_for_body_uses_bare_var_not_state() {
        // Regression: inside `{#for x in state.xs}...{x}...{/for}`,
        // the `x` interpolation must emit as bare `{x}` (closure
        // param), NOT `{state.x}` (state-field rewrite would be
        // wrong).
        let src = r#"component X {
  state { items: List<Str> = [] }
  <template>{#for name in items}<li>{name}</li>{/for}</template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("state.items.map(fn(name) =>"),
            "iter should use `state.items.map(fn(name) => ...)`:\n{out}"
        );
        // `{name}` inside body stays bare — that's the closure
        // param, not a state field.
        assert!(
            out.contains("<li>{name}</li>"),
            "closure body must use bare `name`, not `state.name`:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_c_cont_directive_bearing_module_round_trips_through_classic_fitz() {
        // End-to-end: `.fitzv` with both `{#if}` and `{#for}` +
        // BinOp arithmetic + template interp + scoped style
        // emits classic Fitz source that lexes + parses cleanly.
        let src = r#"component TodoList {
  state {
    items: List<Str> = []
    show_empty: Bool = true
  }
  <template>
    <div>
      {#if show_empty}<p>(list is empty when hidden)</p>{/if}
      <ul>
        {#for name in items}<li>{name}</li>{/for}
      </ul>
    </div>
  </template>
  <style scoped>
    ul { list-style: none; }
  </style>
}"#;
        let file = parse_expand(src);
        let emitted = emit_module_ssr(&file).unwrap();
        // Lex + parse the emitted source through classic Fitz.
        let tokens = crate::lexer::tokenize(&emitted).unwrap_or_else(|e| {
            panic!("emitted source failed to lex:\n{emitted}\n--- err ---\n{e}")
        });
        let _program = crate::parser::parse(tokens).unwrap_or_else(|e| {
            panic!("emitted source failed to parse:\n{emitted}\n--- err ---\n{e}")
        });

        // Shape sanity.
        assert!(emitted.contains("fn __fitz_view_str_join("));
        assert!(emitted.contains("if (state.show_empty)"));
        assert!(emitted.contains("__fitz_view_str_join(state.items.map(fn(name) =>"));
        assert!(emitted.contains("<style>"));
    }

    #[test]
    fn phase_11_6_c_cont_all_text_template_still_uses_pretty_triple_string() {
        // Regression: a template with no directives should still
        // use the pretty triple-string form for the html(...)
        // argument (not chain form).
        let src = r#"component X {
  state { count: Int = 0 }
  <template><div><span>{count}</span></div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // Look for the triple-string form.
        assert!(
            out.contains("return html(\"\"\"<div><span>{state.count}</span></div>\"\"\")"),
            "expected pretty triple-string form:\n{out}"
        );
    }
}
