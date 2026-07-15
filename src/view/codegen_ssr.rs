//! Phase 11.6.b — SSR emitter (`view::emit_ssr`).
//!
//! Second backend paralleling `codegen_wasm.rs`. Consumes an
//! [`ExpandedViewFile`] and emits classic Fitz source text
//! targeting the `fitz-liveviews` framework runtime contract
//! (v0.20.0 `@live_component` + `@render_for` + `@on`
//! decorators + v0.20.1 implicit `flv_register` injection).
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
//! - `{#if}` / `{#for}` deferred to 11.6.c continuation
//!   (requires Vec<TemplatePiece> refactor).
//! - `<Child />` composition deferred to 11.6.d.
//! - `<slot />` deferred to Phase 11.7+.
//!
//! ## Visible debt: view lexer does not tokenise `.` in event body
//!
//! `event bump() { count = count.something() }` fails at
//! `view::parse` with "unexpected character `.` at the top
//! level of a component" — the view lexer refuses to tokenise
//! `.` outside of template contexts. This blocks users from
//! writing method calls or field access in event body RHS
//! source, even though the SSR walker itself supports them
//! (verified by unit tests that construct the `ExpandedComponent`
//! directly). Real fix is a view-lexer follow-up — adding `.` to
//! the accepted character set inside event body raw-capture.
//! Not part of 11.6.c scope; will be closed alongside 11.6.c's
//! template-directive continuation or a dedicated view-lexer
//! cleanup pass.
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
        emit_component_ssr_into(component, &mut out)?;
    }
    Ok(out)
}

/// Emit one component's classic Fitz source. Rarely useful in
/// isolation — [`emit_module_ssr`] is the entry point. Kept
/// pub so tests can exercise single-component fixtures without
/// wrapping in `ExpandedViewFile { components: vec![...] }`.
pub fn emit_component_ssr(component: &ExpandedComponent) -> SsrEmitResult<String> {
    let mut out = String::new();
    emit_module_header(&mut out);
    emit_component_ssr_into(component, &mut out)?;
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
        "// Generated by fitz view SSR emitter (Phase 11.6.b).\n\
         // Do NOT edit — regenerate from the source `.fitzv`.\n\
         \n\
         from fitz_liveviews import Html, html\n\
         \n",
    );
}

// ---------------------------------------------------------------------------
// Per-component emit
// ---------------------------------------------------------------------------

fn emit_component_ssr_into(component: &ExpandedComponent, out: &mut String) -> SsrEmitResult<()> {
    emit_state_type(component, out)?;
    emit_render_fn(component, out)?;
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
fn emit_render_fn(component: &ExpandedComponent, out: &mut String) -> SsrEmitResult<()> {
    let state_field_names: Vec<&str> = component.state.iter().map(|f| f.name.as_str()).collect();

    writeln!(out, "@render_for(\"{}\")", component.name).unwrap();
    writeln!(
        out,
        "fn {}_render(state: {}) -> Html {{",
        component.name, component.name
    )
    .unwrap();

    let mut html_body = String::new();

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
        html_body.push_str("<style>");
        html_body.push_str(&escape_css_braces_for_fitz_interp(css_body));
        html_body.push_str("</style>");
    }

    if let Some(template) = &component.template {
        for node in &template.roots {
            emit_template_node_to_html(node, &state_field_names, component, &mut html_body)?;
        }
    }
    writeln!(out, "  return html(\"\"\"{}\"\"\")", html_body.trim_end()).unwrap();
    writeln!(out, "}}\n").unwrap();
    Ok(())
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
/// over [`format_fitz_expr`] with the event's context label
/// baked in — the walker handles the actual grammar.
///
/// Phase 11.6.c widened this from literal-only to the full
/// walker grammar (BinOp arithmetic, function calls,
/// StrInterp, Field access, Index, List, Map, Range,
/// Ok/Err, arrow closures for `.map(...)` etc.).
fn format_event_rhs(
    expr: &Expr,
    state_field_names: &[&str],
    component: &ExpandedComponent,
    event: &ExpandedEventHandler,
) -> SsrEmitResult<String> {
    format_fitz_expr(
        expr,
        state_field_names,
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
/// - Uppercase-tag `ChildComponent` — rejected with a
///   11.6.d pointer.
/// - `If` / `For` — rejected with a 11.6.c pointer.
/// - `Slot` — rejected with a 11.7+ pointer.
fn emit_template_node_to_html(
    node: &ExpandedTemplateNode,
    state_field_names: &[&str],
    component: &ExpandedComponent,
    out: &mut String,
) -> SsrEmitResult<()> {
    match node {
        ExpandedTemplateNode::Text(s) => {
            out.push_str(s);
            Ok(())
        }
        ExpandedTemplateNode::Interpolation { expr, .. } => {
            let rendered = format_template_interpolation(expr, state_field_names, component)?;
            out.push('{');
            out.push_str(&rendered);
            out.push('}');
            Ok(())
        }
        ExpandedTemplateNode::Element {
            tag,
            attrs,
            children,
            self_closing,
            ..
        } => {
            out.push('<');
            out.push_str(tag);
            for attr in attrs {
                out.push(' ');
                emit_attr_to_html(attr, state_field_names, component, out)?;
            }
            if *self_closing {
                out.push_str(" />");
                return Ok(());
            }
            out.push('>');
            for child in children {
                emit_template_node_to_html(child, state_field_names, component, out)?;
            }
            out.push_str("</");
            out.push_str(tag);
            out.push('>');
            Ok(())
        }
        ExpandedTemplateNode::If { .. } => Err(SsrEmitError {
            message: "`{#if ...}` in a template — deferred to Phase 11.6.c (template \
                     directives lower to string concatenation)."
                .to_string(),
            context: format!("component `{}` template", component.name),
        }),
        ExpandedTemplateNode::For { .. } => Err(SsrEmitError {
            message: "`{#for ...}` in a template — deferred to Phase 11.6.c (template \
                     directives lower to string concatenation)."
                .to_string(),
            context: format!("component `{}` template", component.name),
        }),
        ExpandedTemplateNode::Slot { .. } => Err(SsrEmitError {
            message: "`<slot />` — fallback children composition is deferred to Phase \
                     11.7+ (composition wiring across parent/child)."
                .to_string(),
            context: format!("component `{}` template", component.name),
        }),
        ExpandedTemplateNode::ChildComponent { name, .. } => Err(SsrEmitError {
            message: format!(
                "child component composition `<{name} />` — deferred to Phase 11.6.d \
                 (module loader integration + inline-render for SSR)."
            ),
            context: format!("component `{}` template", component.name),
        }),
    }
}

/// Emit a single attribute to HTML. Handles the three shapes:
/// - `Static { name, value }` — `<name>="<value>"` verbatim.
/// - `Interpolation { name, expr }` — `<name>="{state.<field>}"`
///   with the state-field rewrite.
/// - `Event { event_name, handler_name }` — translated to
///   `data-flv-<event>="<handler>"` per the fitz-liveviews
///   client runtime contract.
fn emit_attr_to_html(
    attr: &ExpandedAttr,
    state_field_names: &[&str],
    component: &ExpandedComponent,
    out: &mut String,
) -> SsrEmitResult<()> {
    match attr {
        ExpandedAttr::Static { name, value, .. } => {
            // Escape `"` in value for the surrounding `"..."`.
            let escaped = value.replace('"', "\\\"");
            write!(out, "{name}=\"{escaped}\"").unwrap();
            Ok(())
        }
        ExpandedAttr::Interpolation { name, expr, .. } => {
            let rendered = format_template_interpolation(expr, state_field_names, component)?;
            write!(out, "{name}=\"{{{rendered}}}\"").unwrap();
            Ok(())
        }
        ExpandedAttr::Event {
            event_name,
            handler_name,
            ..
        } => {
            write!(out, "data-flv-{event_name}=\"{handler_name}\"").unwrap();
            Ok(())
        }
    }
}

/// Format a template interpolation expression. Thin wrapper
/// over [`format_fitz_expr`] with the template's context
/// label baked in.
///
/// Phase 11.6.c widened this from bare-state-ident-only to
/// the full walker grammar. `{state.name.upper()}`,
/// `{state.count + 1}`, function calls, etc. all work.
fn format_template_interpolation(
    expr: &Expr,
    state_field_names: &[&str],
    component: &ExpandedComponent,
) -> SsrEmitResult<String> {
    format_fitz_expr(
        expr,
        state_field_names,
        &component.name,
        "template interpolation",
    )
}

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

    #[test]
    fn emit_rejects_if_directive_citing_11_6_c() {
        let src = r#"component X {
  state { flag: Bool = false }
  <template><div>{#if flag}<span>yes</span>{/if}</div></template>
}"#;
        let file = parse_expand(src);
        let err = emit_module_ssr(&file).unwrap_err();
        assert!(err.message.contains("{#if"), "msg: {}", err.message);
        assert!(err.message.contains("11.6.c"), "msg: {}", err.message);
    }

    #[test]
    fn emit_rejects_for_directive_citing_11_6_c() {
        let src = r#"component X {
  state {}
  <template><div>{#for x in xs}<span>{x}</span>{/for}</div></template>
}"#;
        let file = parse_expand(src);
        let err = emit_module_ssr(&file).unwrap_err();
        assert!(err.message.contains("{#for"), "msg: {}", err.message);
        assert!(err.message.contains("11.6.c"), "msg: {}", err.message);
    }

    #[test]
    fn emit_rejects_child_component_citing_11_6_d() {
        let src = r#"component Parent {
  state {}
  <template><Child /></template>
}
component Child {
  state {}
  <template><span>hi</span></template>
}"#;
        let file = parse_expand(src);
        let err = emit_module_ssr(&file).unwrap_err();
        assert!(err.message.contains("<Child"), "msg: {}", err.message);
        assert!(err.message.contains("11.6.d"), "msg: {}", err.message);
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
    fn phase_11_6_c_emit_accepts_method_call_via_direct_expr_construction() {
        // The view LEXER doesn't tokenize `.` inside event body
        // context today (visible view-lexer debt, out of 11.6.c
        // scope). To exercise the SSR walker's method-call
        // support, we construct the ExpandedComponent by hand
        // instead of round-tripping through the view parser.
        use crate::ast::{AssignTarget, Expr, Span, Stmt, TypeExpr};
        use crate::view::ast::Loc;
        use crate::view::expand::{
            ExpandedComponent, ExpandedEventHandler, ExpandedStateField, ExpandedTemplate,
            ExpandedTemplateNode,
        };

        let component = ExpandedComponent {
            name: "X".to_string(),
            loc: Loc::new(1, 1),
            state: vec![ExpandedStateField {
                name: "msg".to_string(),
                type_expr: TypeExpr::Named("Str".to_string()),
                default: Expr::Str("hi".to_string(), Span::ZERO),
                loc: Loc::new(1, 1),
            }],
            events: vec![ExpandedEventHandler {
                name: "shout".to_string(),
                params: vec![],
                body: vec![Stmt::Assign {
                    target: AssignTarget::Ident("msg".to_string(), Span::ZERO),
                    type_: None,
                    value: Expr::Call {
                        callee: Box::new(Expr::Field {
                            object: Box::new(Expr::Ident("msg".to_string(), Span::ZERO)),
                            field: "upper".to_string(),
                            span: Span::ZERO,
                        }),
                        args: vec![],
                        span: Span::ZERO,
                    },
                    span: Span::ZERO,
                }],
                loc: Loc::new(1, 1),
            }],
            template: Some(ExpandedTemplate {
                roots: vec![ExpandedTemplateNode::Text("hi".to_string())],
                loc: Loc::new(1, 1),
            }),
            style: None,
        };
        let out = emit_component_ssr(&component).unwrap();
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
    fn phase_11_6_c_emit_accepts_arrow_closure_via_direct_expr_construction() {
        // Same view-lexer limitation as the method call test —
        // construct the AST directly.
        use crate::ast::{AssignTarget, Expr, Param, Span, Stmt, TypeExpr};
        use crate::view::ast::Loc;
        use crate::view::expand::{
            ExpandedComponent, ExpandedEventHandler, ExpandedStateField, ExpandedTemplate,
            ExpandedTemplateNode,
        };

        // `xs = xs.map(fn(x) => x + 1)` — a common pattern.
        let map_call = Expr::Call {
            callee: Box::new(Expr::Field {
                object: Box::new(Expr::Ident("xs".to_string(), Span::ZERO)),
                field: "map".to_string(),
                span: Span::ZERO,
            }),
            args: vec![Expr::FnExpr {
                params: vec![Param {
                    name: "x".to_string(),
                    type_: None,
                    default: None,
                    varargs: false,
                    name_span: Span::ZERO,
                }],
                body: vec![Stmt::Return(
                    Expr::BinOp {
                        op: crate::ast::BinOpKind::Add,
                        left: Box::new(Expr::Ident("x".to_string(), Span::ZERO)),
                        right: Box::new(Expr::Int(1, Span::ZERO)),
                        span: Span::ZERO,
                    },
                    Span::ZERO,
                )],
                is_async: false,
                span: Span::ZERO,
            }],
            span: Span::ZERO,
        };

        let component = ExpandedComponent {
            name: "X".to_string(),
            loc: Loc::new(1, 1),
            state: vec![ExpandedStateField {
                name: "xs".to_string(),
                type_expr: TypeExpr::Generic {
                    name: "List".to_string(),
                    args: vec![TypeExpr::Named("Int".to_string())],
                },
                default: Expr::List(vec![], Span::ZERO),
                loc: Loc::new(1, 1),
            }],
            events: vec![ExpandedEventHandler {
                name: "bump_all".to_string(),
                params: vec![],
                body: vec![Stmt::Assign {
                    target: AssignTarget::Ident("xs".to_string(), Span::ZERO),
                    type_: None,
                    value: map_call,
                    span: Span::ZERO,
                }],
                loc: Loc::new(1, 1),
            }],
            template: Some(ExpandedTemplate {
                roots: vec![ExpandedTemplateNode::Text("hi".to_string())],
                loc: Loc::new(1, 1),
            }),
            style: None,
        };
        let out = emit_component_ssr(&component).unwrap();
        assert!(
            out.contains("xs: state.xs.map(fn(x) => (x + 1)),"),
            "expected arrow closure passed to .map():\n{out}"
        );
    }

    #[test]
    fn phase_11_6_c_still_rejects_if_directive_pending_continuation() {
        // `{#if}` is deferred to the 11.6.c continuation (needs
        // Vec<TemplatePiece> refactor). Reject with clear pointer.
        let src = r#"component X {
  state { flag: Bool = false }
  <template><div>{#if flag}<span>yes</span>{/if}</div></template>
}"#;
        let file = parse_expand(src);
        let err = emit_module_ssr(&file).unwrap_err();
        assert!(err.message.contains("{#if"), "msg: {}", err.message);
    }

    #[test]
    fn phase_11_6_c_still_rejects_for_directive_pending_continuation() {
        let src = r#"component X {
  state { xs: List<Int> = [] }
  <template><ul>{#for x in xs}<li>{x}</li>{/for}</ul></template>
}"#;
        let file = parse_expand(src);
        let err = emit_module_ssr(&file).unwrap_err();
        assert!(err.message.contains("{#for"), "msg: {}", err.message);
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
}
