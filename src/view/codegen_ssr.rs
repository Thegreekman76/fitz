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
//! ## MVP scope (11.6.b)
//!
//! - Single-component `.fitzv` file (multi-component
//!   composition comes in 11.6.d).
//! - State fields of any type that classic Fitz accepts
//!   (`Int` / `Float` / `Bool` / `Str` / `Nullable<T>` / ...).
//! - Event bodies with **literal-only RHS** in assignments
//!   (`count = 0`, `msg = "hi"`, `flag = true`). BinOp
//!   arithmetic (`count + 1`), function calls, etc. rejected
//!   with a 11.6.c pointer.
//! - Multiple `Stmt::Assign` per event body: OK — the emitter
//!   accumulates the mutations and builds one struct literal.
//! - Template with `Text` / `Interpolation` / `Element` /
//!   `Static` attrs / `Interpolation` attrs / `Event` attrs.
//! - `{#if}` / `{#for}` / `<style scoped>` deferred to 11.6.c.
//! - `<Child />` composition deferred to 11.6.d.
//! - `<slot />` deferred to Phase 11.7+.
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
    if let Some(style) = &component.style {
        return Err(SsrEmitError {
            message: format!(
                "component `{}` declares a `<style {}>` block — style handling \
                 in the SSR emitter is deferred to Phase 11.6.c (inline `<style>` \
                 tag in HTML output).",
                component.name,
                match style {
                    super::expand::ExpandedStyle::Scoped { .. } => "scoped",
                    super::expand::ExpandedStyle::Global { .. } => "global",
                }
            ),
            context: format!("component `{}`", component.name),
        });
    }

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
/// take the assigned value (with bare identifier RHS rewritten
/// as `state.<field>`); untouched fields carry over from
/// `state.<field>`.
///
/// 11.6.b MVP restriction: only `Stmt::Assign` with
/// `AssignTarget::Ident(field_name)` and a literal RHS
/// (`Expr::Int` / `Expr::Str` / `Expr::Bool` / `Expr::Float` /
/// `Expr::Null`) is supported. BinOp / function calls / other
/// expressions in the RHS reject with a 11.6.c pointer.
///
/// A bare identifier RHS that names an in-scope state field is
/// accepted (rewritten as `state.<field>`) — `count = other`
/// where both are state fields is a valid MVP case.
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
                        "event `{}` body contains a non-assignment statement — 11.6.b MVP only \
                         supports single- or multi-`Stmt::Assign` bodies. Other statement kinds \
                         (`if` / `for` / `return` / expression statements) deferred to Phase 11.6.c.",
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

/// Format the RHS of an event body assignment. 11.6.b MVP
/// accepts:
/// - Literal `Expr::Int` / `Expr::Str` / `Expr::Bool` /
///   `Expr::Float` / `Expr::Null`
/// - Bare `Expr::Ident(field_name)` where `field_name` is a
///   declared state field — rewritten as `state.<field_name>`.
///
/// Everything else rejects with a 11.6.c pointer.
fn format_event_rhs(
    expr: &Expr,
    state_field_names: &[&str],
    component: &ExpandedComponent,
    event: &ExpandedEventHandler,
) -> SsrEmitResult<String> {
    match expr {
        Expr::Int(n, _) => Ok(format!("{n}")),
        Expr::Float(f, _) => Ok(format!("{f}")),
        Expr::Bool(b, _) => Ok(b.to_string()),
        Expr::Str(s, _) => Ok(format!("{s:?}")),
        Expr::Null(_) => Ok("null".to_string()),
        Expr::Ident(name, _) => {
            if state_field_names.contains(&name.as_str()) {
                Ok(format!("state.{name}"))
            } else {
                Err(SsrEmitError {
                    message: format!(
                        "event `{}` body RHS references identifier `{}` which is not a \
                         declared state field. Free-var references in event bodies are \
                         deferred to Phase 11.6.c (which will also lower BinOp / function \
                         calls).",
                        event.name, name
                    ),
                    context: format!("component `{}` event `{}`", component.name, event.name),
                })
            }
        }
        // Everything else: BinOp, UnaryOp, Call, StrInterp, Field
        // access, Index, FnExpr, struct literals — deferred to
        // 11.6.c which will handle the general expression lowering.
        _ => Err(SsrEmitError {
            message: format!(
                "event `{}` body RHS uses a non-literal expression — 11.6.b MVP only \
                 accepts literal values or bare state-field identifiers. Arithmetic \
                 (`count + 1`), string interpolation, function calls, etc. deferred to \
                 Phase 11.6.c.",
                event.name
            ),
            context: format!("component `{}` event `{}`", component.name, event.name),
        }),
    }
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

/// Format a template interpolation expression. 11.6.b MVP
/// accepts bare state-field identifiers (rewritten as
/// `state.<field>`) and rejects everything else with a
/// 11.6.c pointer.
///
/// The rejection here is deliberately narrow: real templates
/// use richer interpolations (`{state.name.upper()}`,
/// `{state.count + 1}`, function calls) which need proper
/// expression lowering. That lands in 11.6.c together with
/// the event body's expression lowering.
fn format_template_interpolation(
    expr: &Expr,
    state_field_names: &[&str],
    component: &ExpandedComponent,
) -> SsrEmitResult<String> {
    match expr {
        Expr::Ident(name, _) if state_field_names.contains(&name.as_str()) => {
            Ok(format!("state.{name}"))
        }
        _ => Err(SsrEmitError {
            message: "template interpolation is not a bare state-field identifier — 11.6.b \
                     MVP only accepts `{<field>}` where `<field>` is a declared state field. \
                     Richer interpolations (`state.field.upper()`, arithmetic, function \
                     calls) deferred to Phase 11.6.c."
                .to_string(),
            context: format!("component `{}` template interpolation", component.name),
        }),
    }
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

/// Print an `Expr` as classic Fitz source text — restricted
/// to the shapes we accept as state field defaults or event
/// body RHS. Extends [`format_event_rhs`] with the identifier
/// case for the *default* case (defaults reference `null`,
/// literals, or other consts — never bare state fields, which
/// would be circular).
///
/// Rejects everything else the same way — 11.6.b MVP is
/// literal-only for defaults, richer expressions land in 11.6.c.
fn format_expr_source(
    expr: &Expr,
    component_name: &str,
    context_label: &str,
) -> SsrEmitResult<String> {
    match expr {
        Expr::Int(n, _) => Ok(format!("{n}")),
        Expr::Float(f, _) => Ok(format!("{f}")),
        Expr::Bool(b, _) => Ok(b.to_string()),
        Expr::Str(s, _) => Ok(format!("{s:?}")),
        Expr::Null(_) => Ok("null".to_string()),
        _ => Err(SsrEmitError {
            message: format!(
                "{context_label} for component `{component_name}` is not a literal — \
                 11.6.b MVP only accepts literal defaults (`Int` / `Float` / `Bool` / \
                 `Str` / `null`). Richer expressions deferred to Phase 11.6.c."
            ),
            context: format!("component `{component_name}` {context_label}"),
        }),
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

    #[test]
    fn emit_rejects_binop_rhs_citing_11_6_c() {
        // `count = count + 1` — BinOp deferred.
        let src = r#"component X {
  state { count: Int = 0 }
  event bump() { count = count + 1 }
  <template><div>{count}</div></template>
}"#;
        let file = parse_expand(src);
        let err = emit_module_ssr(&file).unwrap_err();
        assert!(err.message.contains("non-literal"), "msg: {}", err.message);
        assert!(err.message.contains("11.6.c"), "msg: {}", err.message);
    }

    #[test]
    fn emit_rejects_scoped_style_citing_11_6_c() {
        let src = r#"component X {
  state {}
  <template><div>hi</div></template>
  <style scoped>
    .foo { color: red; }
  </style>
}"#;
        let file = parse_expand(src);
        let err = emit_module_ssr(&file).unwrap_err();
        assert!(err.message.contains("scoped"), "msg: {}", err.message);
        assert!(err.message.contains("11.6.c"), "msg: {}", err.message);
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
        // `{other}` where `other` is not a state field.
        let src = r#"component X {
  state { count: Int = 0 }
  <template><span>{other}</span></template>
}"#;
        let file = parse_expand(src);
        let err = emit_module_ssr(&file).unwrap_err();
        assert!(
            err.message.contains("not a bare state-field identifier"),
            "msg: {}",
            err.message
        );
        assert!(err.message.contains("11.6.c"), "msg: {}", err.message);
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
}
