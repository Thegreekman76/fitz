//! WASM code generator for `.fitzv` view components — Phase 11.4.b.
//!
//! Consumes an `ExpandedComponent` (post-11.3.c pipeline: parse →
//! expand → check) and emits Rust source code that, when compiled
//! for the `wasm32-unknown-unknown` target with the deps declared
//! under the opt-in feature `client-wasm`, produces a WebAssembly
//! bundle that mounts the component into a DOM node.
//!
//! **Approach A2** — hand-rolled `wasm-bindgen` + `web-sys` directly,
//! no framework in the loop. See `docs/fase-11-plan.md` §9.l for the
//! decision analysis (why A2 over A1 framework delegation, A3 in-tree
//! runtime crate, B1 JS-vanilla, B2 JS compiler delegation).
//!
//! ## Scope of 11.4.b (POC counter subset — strictly enforced)
//!
//! Cubre exactamente lo que hace falta para que un `.fitzv` de
//! counter compile a WASM y funcione en el browser:
//!
//! - **State**: solo `Int` primitivos con default literal
//!   (`state { count: Int = 0 }`).
//! - **Event handlers**: sync, cero params, body = un solo
//!   `Stmt::Assign` a state field con RHS numérico
//!   (`Expr::Int` / `Expr::Ident` / `Expr::BinOp` con Add/Sub/Mul/Div/Mod).
//! - **Template**: `Text`, `Element` (tag estático, attrs `Static` y
//!   `Event` con `@click`), `Interpolation` con `Expr::Ident` de un
//!   state field.
//! - **Style**: `Scoped` inyectado 1x via dedup helper con
//!   `AtomicBool`, o `Global` inyectado directo. Cero style: no-op.
//!
//! Todo lo demás (If / For / Slot / attrs interpolados / handlers
//! con params / eventos que no sean `@click` / state con tipo
//! distinto a `Int`) devuelve `EmitError` con un mensaje que cita el
//! sub-fase donde se cierra (11.4.c o 11.5).
//!
//! ## Reactivity model del POC (D1 de 11.4.b)
//!
//! Naive re-render on state mutation. Cada `event fn` termina
//! llamando a `self.render()`, que limpia el DOM subtree del
//! componente y lo reconstruye desde cero usando el state actual.
//! Sin signals, sin VDOM, sin diffing. Aceptable para counter /
//! forms chicos / dashboards con updates ocasionales. Refinement a
//! signals fine-grained cae en A3 (in-tree runtime crate) — sub-fase
//! futura si la evidencia de bloat con N componentes lo empuja.
//!
//! ## API pública (D2 de 11.4.b)
//!
//! - [`emit_component`] — emite el Rust de UN componente (struct +
//!   new() + event fns + mount() + render() + style helper).
//!   NO emite `use` imports ni `#[wasm_bindgen(start)]`.
//! - [`emit_module`] — emite un módulo Rust entero: preludio con
//!   imports + N componentes. Conveniencia para 11.4.c/11.5 smoke.
//!
//! ## Testing strategy (D4 de 11.4.b)
//!
//! Todos los tests unitarios validan substrings de la string
//! emitida (paralelo bit-a-bit a `view::css_parser::tests`). NO
//! buildean el output real a WASM — eso requiere target
//! `wasm32-unknown-unknown` + `wasm-pack` que no viven en
//! `cargo test`. El browser smoke real llega en 11.4.c.
//!
//! ## Invariant 4 (isolation)
//!
//! Este módulo vive 100% adentro de `src/view/`. NO importa nada de
//! `src/codegen.rs` (el codegen server-side clásico) ni del runtime
//! HTTP. La única superficie compartida es `crate::ast::{Expr, Stmt,
//! TypeExpr, AssignTarget, BinOpKind}` — nodos AST que el
//! `expand::ExpandedComponent` ya carga como fields públicos. Cero
//! coupling nuevo con el resto del compilador.

use crate::ast::{AssignTarget, BinOpKind, Expr, Stmt, TypeExpr};
use crate::view::{
    ExpandedAttr, ExpandedComponent, ExpandedEventHandler, ExpandedStyle, ExpandedTemplateNode,
    ExpandedViewFile,
};
use std::fmt::Write;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Error surfaced while emitting WASM Rust code from an
/// `ExpandedComponent`. Every unsupported shape of the current POC
/// subset produces one of these with a `context` label that names
/// the exact component + field / handler / template node where the
/// problem lives, plus a `message` that either explains the misuse
/// or cites the sub-phase where the missing capability lands.
#[derive(Debug, Clone, PartialEq)]
pub struct EmitError {
    pub message: String,
    pub context: String,
}

impl std::fmt::Display for EmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "wasm emit error — {} ({})", self.message, self.context)
    }
}

impl std::error::Error for EmitError {}

pub type EmitResult<T> = Result<T, EmitError>;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Emit the Rust source code for ONE component. Output includes:
///
/// - `pub struct <Name>` con state fields (envueltos en
///   `RefCell<...>`) + `root: RefCell<Option<HtmlElement>>`.
/// - `impl <Name>` con `pub fn new() -> Rc<Self>`, un `fn <event>()`
///   privado por cada handler declarado, `pub fn mount(...)`, y
///   `fn render()` que reconstruye el DOM subtree del componente
///   desde el `ExpandedTemplate`.
/// - Cuando el componente tiene `<style scoped>` o `<style global>`,
///   una fn libre `__inject_style_<sanitized>()` que injecta el
///   `<style>` en `document.head` una sola vez (dedup por
///   `AtomicBool` para scoped, sin dedup para global — cada mount de
///   un componente distinto podría querer sus propias reglas
///   globales).
///
/// NO emite `use` imports ni `#[wasm_bindgen(start)]`. El caller
/// (típicamente [`emit_module`] o el CLI de 11.5) compone.
pub fn emit_component(component: &ExpandedComponent) -> EmitResult<String> {
    let mut out = String::new();
    emit_component_impl(component, &mut out)?;
    Ok(out)
}

/// Emit un módulo Rust entero ready to build con `wasm-pack build`:
///
/// - Preludio con `use` de `wasm_bindgen`, `web_sys`, `std::cell`,
///   `std::rc`, `std::sync::atomic`.
/// - Todos los componentes concatenados via [`emit_component`].
///
/// Convenience para 11.4.c smoke (que va a llamar esta fn y volcar
/// el resultado a un `src/lib.rs` de un crate WASM temporal) y para
/// 11.5 CLI wiring. En 11.5 el CLI probablemente construye su
/// propio preludio (para meter `#[wasm_bindgen(start)]` con
/// composición multi-componente); esta fn queda como default
/// razonable para tests + POCs.
pub fn emit_module(file: &ExpandedViewFile) -> EmitResult<String> {
    let mut out = String::new();
    emit_module_header(&mut out);
    for component in &file.components {
        emit_component_impl(component, &mut out)?;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Module-level preludio
// ---------------------------------------------------------------------------

fn emit_module_header(out: &mut String) {
    out.push_str(
        "// Generated by fitz view WASM emitter (Fase 11.4.b).\n\
         // Do NOT edit — regenerate from the source `.fitzv`.\n\
         \n\
         use std::cell::RefCell;\n\
         use std::rc::Rc;\n\
         use std::sync::atomic::{AtomicBool, Ordering};\n\
         use wasm_bindgen::prelude::*;\n\
         use wasm_bindgen::JsCast;\n\
         use web_sys::{Event, HtmlElement};\n\
         \n",
    );
}

// ---------------------------------------------------------------------------
// Per-component emit
// ---------------------------------------------------------------------------

fn emit_component_impl(component: &ExpandedComponent, out: &mut String) -> EmitResult<()> {
    let state_names: Vec<String> = component.state.iter().map(|f| f.name.clone()).collect();

    emit_struct_and_new(component, out)?;
    emit_event_handlers(component, &state_names, out)?;
    emit_mount_and_render(component, &state_names, out)?;
    if let Some(style) = &component.style {
        emit_style_helper(&component.name, style, out);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Struct + new()
// ---------------------------------------------------------------------------

fn emit_struct_and_new(component: &ExpandedComponent, out: &mut String) -> EmitResult<()> {
    let name = &component.name;

    writeln!(out, "pub struct {} {{", name).unwrap();
    for field in &component.state {
        let rust_ty = type_expr_to_rust(&field.type_expr).map_err(|mut e| {
            e.context = format!(
                "state field `{}` of component `{}` (type)",
                field.name, name
            );
            e
        })?;
        writeln!(out, "    {}: RefCell<{}>,", field.name, rust_ty).unwrap();
    }
    writeln!(out, "    root: RefCell<Option<HtmlElement>>,").unwrap();
    writeln!(out, "}}\n").unwrap();

    writeln!(out, "impl {} {{", name).unwrap();
    writeln!(out, "    pub fn new() -> Rc<Self> {{").unwrap();
    writeln!(out, "        Rc::new({} {{", name).unwrap();
    for field in &component.state {
        let default_rust =
            default_expr_to_rust(&field.default, &field.type_expr).map_err(|mut e| {
                e.context = format!(
                    "state field `{}` of component `{}` (default)",
                    field.name, name
                );
                e
            })?;
        writeln!(
            out,
            "            {}: RefCell::new({}),",
            field.name, default_rust
        )
        .unwrap();
    }
    writeln!(out, "            root: RefCell::new(None),").unwrap();
    writeln!(out, "        }})").unwrap();
    writeln!(out, "    }}\n").unwrap();
    // (impl block stays open — event handlers + mount + render close it)
    Ok(())
}

// ---------------------------------------------------------------------------
// Event handlers
// ---------------------------------------------------------------------------

fn emit_event_handlers(
    component: &ExpandedComponent,
    state_names: &[String],
    out: &mut String,
) -> EmitResult<()> {
    for handler in &component.events {
        emit_event_handler(&component.name, handler, state_names, out)?;
    }
    Ok(())
}

fn emit_event_handler(
    component_name: &str,
    handler: &ExpandedEventHandler,
    state_names: &[String],
    out: &mut String,
) -> EmitResult<()> {
    if !handler.params.is_empty() {
        return Err(EmitError {
            message: "event handlers with parameters are deferred to Phase 11.5".to_string(),
            context: format!(
                "event handler `{}` of component `{}`",
                handler.name, component_name
            ),
        });
    }

    writeln!(out, "    fn {}(self: &Rc<Self>) {{", handler.name).unwrap();
    for stmt in &handler.body {
        lower_stmt(stmt, state_names, "        ", out).map_err(|mut e| {
            e.context = format!(
                "event handler `{}` of component `{}` (body)",
                handler.name, component_name
            );
            e
        })?;
    }
    writeln!(out, "        self.render();").unwrap();
    writeln!(out, "    }}\n").unwrap();
    Ok(())
}

// ---------------------------------------------------------------------------
// mount() + render()
// ---------------------------------------------------------------------------

fn emit_mount_and_render(
    component: &ExpandedComponent,
    state_names: &[String],
    out: &mut String,
) -> EmitResult<()> {
    let name = &component.name;

    // ---- mount() -------------------------------------------------
    writeln!(
        out,
        "    pub fn mount(self: &Rc<Self>, selector: &str) -> Result<(), JsValue> {{"
    )
    .unwrap();
    if let Some(style) = &component.style {
        let helper = style_helper_ident(name, style);
        writeln!(out, "        {}();", helper).unwrap();
    }
    writeln!(
        out,
        "        let document = web_sys::window().unwrap().document().unwrap();"
    )
    .unwrap();
    writeln!(out, "        let root = document").unwrap();
    writeln!(out, "            .query_selector(selector)?").unwrap();
    writeln!(
        out,
        "            .ok_or_else(|| JsValue::from_str(\"mount: selector matched no element\"))?"
    )
    .unwrap();
    writeln!(out, "            .dyn_into::<HtmlElement>()?;").unwrap();
    writeln!(out, "        *self.root.borrow_mut() = Some(root);").unwrap();
    writeln!(out, "        self.render();").unwrap();
    writeln!(out, "        Ok(())").unwrap();
    writeln!(out, "    }}\n").unwrap();

    // ---- render() ------------------------------------------------
    writeln!(out, "    fn render(self: &Rc<Self>) {{").unwrap();
    writeln!(out, "        let root_ref = self.root.borrow();").unwrap();
    writeln!(out, "        let root = match root_ref.as_ref() {{").unwrap();
    writeln!(out, "            Some(r) => r,").unwrap();
    writeln!(out, "            None => return,").unwrap();
    writeln!(out, "        }};").unwrap();
    writeln!(out, "        while let Some(child) = root.first_child() {{").unwrap();
    writeln!(out, "            let _ = root.remove_child(&child);").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(
        out,
        "        let document = web_sys::window().unwrap().document().unwrap();"
    )
    .unwrap();

    if let Some(template) = &component.template {
        let mut ctx = RenderCtx::new(name, state_names);
        for root_node in &template.roots {
            emit_template_node(root_node, "root", &mut ctx, out)?;
        }
    }

    writeln!(out, "    }}").unwrap();
    writeln!(out, "}}\n").unwrap();
    Ok(())
}

// ---------------------------------------------------------------------------
// Template rendering emit
// ---------------------------------------------------------------------------

/// Ctx that walks the template tree accumulating unique var names
/// (`__el0`, `__el1`, `__t0`, `__interp0`, ...) so multiple elements
/// in the same `render()` body don't shadow each other.
struct RenderCtx<'a> {
    component_name: &'a str,
    state_names: &'a [String],
    var_counter: usize,
}

impl<'a> RenderCtx<'a> {
    fn new(component_name: &'a str, state_names: &'a [String]) -> Self {
        RenderCtx {
            component_name,
            state_names,
            var_counter: 0,
        }
    }

    fn fresh(&mut self, prefix: &str) -> String {
        let id = self.var_counter;
        self.var_counter += 1;
        format!("__{}{}", prefix, id)
    }
}

fn emit_template_node(
    node: &ExpandedTemplateNode,
    parent_var: &str,
    ctx: &mut RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    match node {
        ExpandedTemplateNode::Text(text) => {
            emit_text(text, parent_var, ctx, out);
            Ok(())
        }
        ExpandedTemplateNode::Interpolation { expr, .. } => {
            emit_interpolation(expr, parent_var, ctx, out)
        }
        ExpandedTemplateNode::Element {
            tag,
            attrs,
            children,
            ..
        } => emit_element(tag, attrs, children, parent_var, ctx, out),
        ExpandedTemplateNode::If { .. } => Err(EmitError {
            message: "`{#if ...}` — deferred to Phase 11.4.c".to_string(),
            context: format!("template of component `{}`", ctx.component_name),
        }),
        ExpandedTemplateNode::For { .. } => Err(EmitError {
            message: "`{#for ...}` — deferred to Phase 11.4.c".to_string(),
            context: format!("template of component `{}`", ctx.component_name),
        }),
        ExpandedTemplateNode::Slot { .. } => Err(EmitError {
            message: "`<slot />` composition — deferred to Phase 11.5".to_string(),
            context: format!("template of component `{}`", ctx.component_name),
        }),
    }
}

fn emit_text(text: &str, parent_var: &str, ctx: &mut RenderCtx, out: &mut String) {
    // Skip pure whitespace-only text nodes to keep the emitted DOM
    // small and readable. HTML collapses whitespace anyway.
    if text.trim().is_empty() {
        return;
    }
    let var = ctx.fresh("t");
    writeln!(
        out,
        "        let {} = document.create_text_node({});",
        var,
        rust_string_literal(text)
    )
    .unwrap();
    writeln!(
        out,
        "        {}.append_child(&{}).unwrap();",
        parent_var, var
    )
    .unwrap();
}

fn emit_interpolation(
    expr: &Expr,
    parent_var: &str,
    ctx: &mut RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    let var_interp = ctx.fresh("interp");
    let var_node = ctx.fresh("t");
    let expr_rust = lower_expr(expr, ctx.state_names)?;
    writeln!(
        out,
        "        let {} = format!(\"{{}}\", {});",
        var_interp, expr_rust
    )
    .unwrap();
    writeln!(
        out,
        "        let {} = document.create_text_node(&{});",
        var_node, var_interp
    )
    .unwrap();
    writeln!(
        out,
        "        {}.append_child(&{}).unwrap();",
        parent_var, var_node
    )
    .unwrap();
    Ok(())
}

fn emit_element(
    tag: &str,
    attrs: &[ExpandedAttr],
    children: &[ExpandedTemplateNode],
    parent_var: &str,
    ctx: &mut RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    let var = ctx.fresh("el");
    writeln!(
        out,
        "        let {} = document.create_element({}).unwrap();",
        var,
        rust_string_literal(tag)
    )
    .unwrap();

    for attr in attrs {
        match attr {
            ExpandedAttr::Static { name, value, .. } => {
                emit_static_attr(name, value, &var, out);
            }
            ExpandedAttr::Event {
                event_name,
                handler_name,
                ..
            } => {
                emit_event_attr(event_name, handler_name, &var, ctx.component_name, out)?;
            }
            ExpandedAttr::Interpolation { name, .. } => {
                return Err(EmitError {
                    message: format!(
                        "interpolated attribute `{}=\"{{...}}\"` — deferred to Phase 11.4.c",
                        name
                    ),
                    context: format!(
                        "element `<{}>` in template of component `{}`",
                        tag, ctx.component_name
                    ),
                });
            }
        }
    }

    for child in children {
        emit_template_node(child, &var, ctx, out)?;
    }

    writeln!(
        out,
        "        {}.append_child(&{}).unwrap();",
        parent_var, var
    )
    .unwrap();
    Ok(())
}

fn emit_static_attr(name: &str, value: &str, el_var: &str, out: &mut String) {
    writeln!(
        out,
        "        {}.set_attribute({}, {}).unwrap();",
        el_var,
        rust_string_literal(name),
        rust_string_literal(value)
    )
    .unwrap();
}

fn emit_event_attr(
    event_name: &str,
    handler_name: &str,
    el_var: &str,
    component_name: &str,
    out: &mut String,
) -> EmitResult<()> {
    // POC only maps `@click` — other event names deferred to 11.4.c.
    let dom_event = match event_name {
        "click" => "click",
        other => {
            return Err(EmitError {
                message: format!(
                    "event `@{}` — only `@click` supported in Phase 11.4.b (deferred to 11.4.c)",
                    other
                ),
                context: format!(
                    "event attribute on element in template of component `{}`",
                    component_name
                ),
            });
        }
    };

    writeln!(out, "        {{").unwrap();
    writeln!(out, "            let __self_clone = self.clone();").unwrap();
    writeln!(
        out,
        "            let __closure = Closure::wrap(Box::new(move |_evt: Event| {{"
    )
    .unwrap();
    writeln!(
        out,
        "                {}::{}(&__self_clone);",
        component_name, handler_name
    )
    .unwrap();
    writeln!(out, "            }}) as Box<dyn FnMut(Event)>);").unwrap();
    writeln!(
        out,
        "            {}.add_event_listener_with_callback({}, __closure.as_ref().unchecked_ref()).unwrap();",
        el_var,
        rust_string_literal(dom_event)
    )
    .unwrap();
    writeln!(out, "            __closure.forget();").unwrap();
    writeln!(out, "        }}").unwrap();
    Ok(())
}

// ---------------------------------------------------------------------------
// Style helper
// ---------------------------------------------------------------------------

/// Emit the scoped/global style injection helper as a free `fn`.
/// Scoped variants dedup via `AtomicBool` so N mounted instances of
/// the same component share one `<style>` block. Global variants
/// dedup by component name + a body hash so re-mounts of the same
/// component do not re-inject; but different components with
/// `<style global>` blocks that happen to have identical CSS still
/// inject twice, since we can't cheaply know at emit time whether
/// two Global blocks are the same. Acceptable trade-off for POC.
fn emit_style_helper(component_name: &str, style: &ExpandedStyle, out: &mut String) {
    let helper = style_helper_ident(component_name, style);
    let css = match style {
        ExpandedStyle::Scoped { css_scoped, .. } => css_scoped,
        ExpandedStyle::Global { css, .. } => css,
    };
    writeln!(out, "fn {}() {{", helper).unwrap();
    writeln!(
        out,
        "    static INJECTED: AtomicBool = AtomicBool::new(false);"
    )
    .unwrap();
    writeln!(
        out,
        "    if INJECTED.swap(true, Ordering::SeqCst) {{ return; }}"
    )
    .unwrap();
    writeln!(
        out,
        "    let document = web_sys::window().unwrap().document().unwrap();"
    )
    .unwrap();
    writeln!(out, "    let head = document.head().unwrap();").unwrap();
    writeln!(
        out,
        "    let style_el = document.create_element(\"style\").unwrap();"
    )
    .unwrap();
    writeln!(
        out,
        "    style_el.set_text_content(Some({}));",
        rust_string_literal(css)
    )
    .unwrap();
    writeln!(out, "    let _ = head.append_child(&style_el);").unwrap();
    writeln!(out, "}}\n").unwrap();
}

/// Compute the Rust fn name for the style injection helper.
/// Format: `__inject_style_<component_name>_<scope_class_sanitized>`
/// para Scoped, o `__inject_style_<component_name>_global` para
/// Global. Sanitiza hyphens del scope_class (que vienen del
/// kebab-case) reemplazándolos por `_` para producir un ident Rust
/// válido.
fn style_helper_ident(component_name: &str, style: &ExpandedStyle) -> String {
    match style {
        ExpandedStyle::Scoped { scope_class, .. } => {
            format!(
                "__inject_style_{}_{}",
                sanitize_ident(component_name),
                sanitize_ident(scope_class)
            )
        }
        ExpandedStyle::Global { .. } => {
            format!("__inject_style_{}_global", sanitize_ident(component_name))
        }
    }
}

// ---------------------------------------------------------------------------
// Expr / Stmt lowerers (mini-lowering for the POC subset)
// ---------------------------------------------------------------------------

/// Lower a Fitz `Expr` to a Rust expression string. Supports ONLY
/// the subset needed for the counter POC:
///
/// - `Int` / `Float` / `Bool` literals (Str deferred to 11.4.c since
///   the POC only inspects state fields of type Int).
/// - `Ident` referring to a state field name — emits
///   `(*self.<name>.borrow())`.
/// - `BinOp` with numeric ops (Add/Sub/Mul/Div/Mod) recursing into
///   the sub-expressions.
///
/// Todo lo demás (Str literal, StrInterp, Ident no-state, Call,
/// Field, Index, comparisons, logical ops, etc.) devuelve `EmitError`
/// citando la sub-fase donde se cierra.
fn lower_expr(expr: &Expr, state_names: &[String]) -> EmitResult<String> {
    match expr {
        Expr::Int(n, _) => Ok(format!("{}i64", n)),
        Expr::Float(n, _) => Ok(format!("{}f64", n)),
        Expr::Bool(b, _) => Ok(b.to_string()),
        Expr::Ident(name, _) => {
            if state_names.iter().any(|s| s == name) {
                Ok(format!("(*self.{}.borrow())", name))
            } else {
                Err(EmitError {
                    message: format!(
                        "identifier `{}` is not a state field — non-state refs deferred to Phase 11.4.c",
                        name
                    ),
                    context: "expression".to_string(),
                })
            }
        }
        Expr::BinOp {
            op, left, right, ..
        } => {
            let op_str = lower_binop(op)?;
            let l = lower_expr(left, state_names)?;
            let r = lower_expr(right, state_names)?;
            Ok(format!("({} {} {})", l, op_str, r))
        }
        Expr::Str(_, _)
        | Expr::StrInterp(_, _)
        | Expr::Null(_)
        | Expr::Bytes(_, _)
        | Expr::UnaryOp { .. }
        | Expr::Call { .. }
        | Expr::NamedArg { .. }
        | Expr::FnExpr { .. }
        | Expr::Field { .. }
        | Expr::Index { .. } => Err(EmitError {
            message: format!(
                "expression kind `{}` — deferred to Phase 11.4.c",
                expr_kind_name(expr)
            ),
            context: "expression".to_string(),
        }),
        _ => Err(EmitError {
            message: "unsupported expression kind — deferred to Phase 11.4.c".to_string(),
            context: "expression".to_string(),
        }),
    }
}

fn expr_kind_name(expr: &Expr) -> &'static str {
    match expr {
        Expr::Int(..) => "Int",
        Expr::Float(..) => "Float",
        Expr::Str(..) => "Str",
        Expr::StrInterp(..) => "StrInterp",
        Expr::Bool(..) => "Bool",
        Expr::Null(..) => "Null",
        Expr::Bytes(..) => "Bytes",
        Expr::Ident(..) => "Ident",
        Expr::BinOp { .. } => "BinOp",
        Expr::UnaryOp { .. } => "UnaryOp",
        Expr::Call { .. } => "Call",
        Expr::NamedArg { .. } => "NamedArg",
        Expr::FnExpr { .. } => "FnExpr",
        Expr::Field { .. } => "Field",
        Expr::Index { .. } => "Index",
        _ => "Other",
    }
}

fn lower_binop(op: &BinOpKind) -> EmitResult<&'static str> {
    match op {
        BinOpKind::Add => Ok("+"),
        BinOpKind::Sub => Ok("-"),
        BinOpKind::Mul => Ok("*"),
        BinOpKind::Div => Ok("/"),
        BinOpKind::Mod => Ok("%"),
        BinOpKind::Eq
        | BinOpKind::NotEq
        | BinOpKind::Lt
        | BinOpKind::LtEq
        | BinOpKind::Gt
        | BinOpKind::GtEq
        | BinOpKind::And
        | BinOpKind::Or
        | BinOpKind::Xor
        | BinOpKind::BitAnd
        | BinOpKind::BitOr
        | BinOpKind::BitXor
        | BinOpKind::Shl => Err(EmitError {
            message: "binary op — only arithmetic ops (+/-/*//%) supported in Phase 11.4.b, comparisons/logical/bitwise deferred to 11.4.c".to_string(),
            context: "expression".to_string(),
        }),
        _ => Err(EmitError {
            message: "unsupported binary op — deferred to Phase 11.4.c".to_string(),
            context: "expression".to_string(),
        }),
    }
}

/// Lower a Fitz `Stmt` to Rust statements appended to `out` with the
/// given `indent` prefix. Supports ONLY the subset needed for the
/// counter POC: `Stmt::Assign` with target = state field ident and
/// value = any expr accepted by [`lower_expr`].
fn lower_stmt(
    stmt: &Stmt,
    state_names: &[String],
    indent: &str,
    out: &mut String,
) -> EmitResult<()> {
    match stmt {
        Stmt::Assign { target, value, .. } => match target {
            AssignTarget::Ident(name, _) => {
                if !state_names.iter().any(|s| s == name) {
                    return Err(EmitError {
                        message: format!(
                            "assign to `{}` — only state field reassignments supported in Phase 11.4.b",
                            name
                        ),
                        context: "statement".to_string(),
                    });
                }
                let rhs = lower_expr(value, state_names)?;
                writeln!(out, "{}let __rhs = {};", indent, rhs).unwrap();
                writeln!(out, "{}*self.{}.borrow_mut() = __rhs;", indent, name).unwrap();
                Ok(())
            }
            AssignTarget::Field { .. } => Err(EmitError {
                message: "assign to field (`obj.field = ...`) — deferred to Phase 11.4.c"
                    .to_string(),
                context: "statement".to_string(),
            }),
            AssignTarget::Index { .. } => Err(EmitError {
                message: "assign to index (`xs[i] = ...`) — deferred to Phase 11.4.c".to_string(),
                context: "statement".to_string(),
            }),
        },
        _ => Err(EmitError {
            message:
                "statement kind — only `Stmt::Assign` to a state field supported in Phase 11.4.b"
                    .to_string(),
            context: "statement".to_string(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Type + default mapping
// ---------------------------------------------------------------------------

/// Map a Fitz `TypeExpr` to the corresponding Rust type string used
/// inside `RefCell<...>` for state fields. POC only accepts `Int` →
/// `i64`. Str/Bool/Float/List/Map/Nominal deferred to 11.4.c.
fn type_expr_to_rust(ty: &TypeExpr) -> EmitResult<String> {
    match ty {
        TypeExpr::Named(name) if name == "Int" => Ok("i64".to_string()),
        TypeExpr::Named(other) => Err(EmitError {
            message: format!(
                "state field type `{}` — only `Int` supported in Phase 11.4.b (deferred to 11.4.c)",
                other
            ),
            context: "type".to_string(),
        }),
        TypeExpr::Nullable(_)
        | TypeExpr::Generic { .. }
        | TypeExpr::Tuple(_)
        | TypeExpr::Function { .. } => Err(EmitError {
            message: "compound state field type — deferred to Phase 11.4.c".to_string(),
            context: "type".to_string(),
        }),
    }
}

/// Lower a default expression to Rust source code, cross-checked
/// against the declared type of the field so we can emit the right
/// suffix (`0i64` for Int). POC only accepts Int defaults.
fn default_expr_to_rust(default: &Expr, ty: &TypeExpr) -> EmitResult<String> {
    match (default, ty) {
        (Expr::Int(n, _), TypeExpr::Named(name)) if name == "Int" => Ok(format!("{}i64", n)),
        (_, TypeExpr::Named(name)) if name == "Int" => Err(EmitError {
            message: "default expression is not an `Int` literal — deferred to Phase 11.4.c"
                .to_string(),
            context: "default".to_string(),
        }),
        _ => Err(EmitError {
            message: "default expression for non-Int state field — deferred to Phase 11.4.c"
                .to_string(),
            context: "default".to_string(),
        }),
    }
}

// ---------------------------------------------------------------------------
// String helpers
// ---------------------------------------------------------------------------

/// Emit a Rust string literal for the given `&str`. Uses `{:?}`
/// formatting which produces a valid Rust string literal with the
/// necessary escapes (quotes, backslashes, control chars). Good
/// enough for the POC (CSS bodies, static attr values, text nodes,
/// tag names — none of which are user-controlled at the browser
/// runtime level).
fn rust_string_literal(s: &str) -> String {
    format!("{:?}", s)
}

/// Sanitize a name so it becomes a valid Rust identifier. Replaces
/// hyphens with underscores (the only non-ident char that appears
/// in `scope_class` post-kebab-case) and drops everything else that
/// isn't alphanumeric or underscore.
fn sanitize_ident(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else if ch == '-' {
            out.push('_');
        }
        // else: drop silently — happens for e.g. `.` if any appears
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{expand, parse, ExpandedStateField};

    /// Helper: parse + expand a `.fitzv` source into an
    /// `ExpandedViewFile`, panicking with the error's Display if
    /// either step fails. Keeps the test bodies focused on the
    /// emit output being validated.
    fn parse_expand(src: &str) -> ExpandedViewFile {
        let raw = parse(src).expect("parse succeeded");
        expand(&raw).expect("expand succeeded")
    }

    /// Simplified counter-shape source used for tests that only need
    /// the pipeline to produce a component with state Int + event
    /// handlers + template with @click + scoped style. Event bodies
    /// assign Int literals (not arithmetic) because the view lexer
    /// does not yet tokenise `+`/`-`/`*`/`/`/`%` (deuda documented in
    /// §9.m of `docs/fase-11-plan.md` — must close before 11.4.c
    /// browser smoke). Tests that specifically validate arithmetic
    /// lowering (e.g. `arithmetic_body_lowering_binop_add`) build
    /// the `ExpandedComponent` directly to bypass this lexer gap.
    fn counter_shape_src() -> &'static str {
        r#"component Counter {
  state { count: Int = 0 }
  event increment() { count = 42 }
  event decrement() { count = 0 }

  <template>
    <div class="counter">
      <button @click="decrement">reset</button>
      <span>{count}</span>
      <button @click="increment">bump</button>
    </div>
  </template>

  <style scoped>
    .counter { display: flex; gap: 8px; }
  </style>
}"#
    }

    // ---- Struct + new -------------------------------------------

    #[test]
    fn emit_struct_has_pub_struct_and_state_field() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("pub struct Counter {"),
            "struct decl:\n{}",
            out
        );
        assert!(
            out.contains("count: RefCell<i64>,"),
            "state field:\n{}",
            out
        );
        assert!(
            out.contains("root: RefCell<Option<HtmlElement>>,"),
            "root field:\n{}",
            out
        );
    }

    #[test]
    fn emit_new_returns_rc_self_with_defaults() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("pub fn new() -> Rc<Self> {"),
            "new() signature:\n{}",
            out
        );
        assert!(out.contains("Rc::new(Counter {"), "Rc::new call:\n{}", out);
        assert!(
            out.contains("count: RefCell::new(0i64),"),
            "default:\n{}",
            out
        );
        assert!(
            out.contains("root: RefCell::new(None),"),
            "root default:\n{}",
            out
        );
    }

    // ---- Event handlers -----------------------------------------

    #[test]
    fn emit_event_handler_lowers_assign_and_calls_render() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("fn increment(self: &Rc<Self>) {"),
            "handler signature:\n{}",
            out
        );
        assert!(
            out.contains("let __rhs = 42i64;"),
            "RHS lowering (literal Int):\n{}",
            out
        );
        assert!(
            out.contains("*self.count.borrow_mut() = __rhs;"),
            "assign:\n{}",
            out
        );
        assert!(out.contains("self.render();"), "auto-render call:\n{}", out);
    }

    #[test]
    fn emit_event_handler_decrement_uses_literal_zero() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("fn decrement(self: &Rc<Self>) {"),
            "decrement signature:\n{}",
            out
        );
        assert!(
            out.contains("let __rhs = 0i64;"),
            "reset lowering (literal Int 0):\n{}",
            out
        );
    }

    /// Arithmetic body (`count = count + 1`) is exactly what the
    /// counter demo of 11.4.c will need. The view lexer does NOT
    /// yet tokenise `+`/`-`/`*`/`/`/`%` so we cannot exercise it
    /// through parse→expand today. Build the `ExpandedComponent`
    /// directly to validate the emitter's arithmetic lowering
    /// path — the deuda that gates 11.4.c is separate (view
    /// lexer), documented in §9.m of `docs/fase-11-plan.md`.
    #[test]
    fn arithmetic_body_lowering_binop_add_direct() {
        use crate::ast::{Span, Stmt};
        use crate::view::ast::Loc;
        let loc = Loc { line: 1, column: 1 };
        let component = ExpandedComponent {
            name: "Counter".to_string(),
            loc,
            state: vec![ExpandedStateField {
                name: "count".to_string(),
                type_expr: TypeExpr::Named("Int".to_string()),
                default: Expr::Int(0, Span::default()),
                loc,
            }],
            events: vec![ExpandedEventHandler {
                name: "increment".to_string(),
                params: vec![],
                body: vec![Stmt::Assign {
                    target: AssignTarget::Ident("count".to_string(), Span::default()),
                    type_: None,
                    value: Expr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(Expr::Ident("count".to_string(), Span::default())),
                        right: Box::new(Expr::Int(1, Span::default())),
                        span: Span::default(),
                    },
                    span: Span::default(),
                }],
                loc,
            }],
            template: None,
            style: None,
        };
        let out = emit_component(&component).unwrap();
        assert!(
            out.contains("let __rhs = ((*self.count.borrow()) + 1i64);"),
            "arithmetic lowering (Ident + Int):\n{}",
            out
        );
        assert!(
            out.contains("*self.count.borrow_mut() = __rhs;"),
            "assign target:\n{}",
            out
        );
        assert!(out.contains("self.render();"), "auto-render:\n{}", out);
    }

    /// End-to-end pipeline test: after the view-lexer arithmetic
    /// follow-up (§9.n of `docs/fase-11-plan.md`), a full counter
    /// source with `count = count + 1` parses, expands, AND emits
    /// the expected WASM lowering. Distinct from the direct-AST
    /// tests above — this proves the whole path is unblocked, not
    /// just the emitter in isolation.
    #[test]
    fn arithmetic_body_lowering_end_to_end_via_parse_expand() {
        let src = r#"component Counter {
  state { count: Int = 0 }
  event increment() { count = count + 1 }
  event decrement() { count = count - 1 }

  <template>
    <div><span>{count}</span></div>
  </template>
}"#;
        let expanded = parse_expand(src);
        let out = emit_component(&expanded.components[0]).unwrap();
        // The full pipeline should produce the exact same lowering
        // as the direct-AST arithmetic tests.
        assert!(
            out.contains("let __rhs = ((*self.count.borrow()) + 1i64);"),
            "increment arithmetic lowering via full pipeline:\n{}",
            out
        );
        assert!(
            out.contains("let __rhs = ((*self.count.borrow()) - 1i64);"),
            "decrement arithmetic lowering via full pipeline:\n{}",
            out
        );
        assert!(
            out.contains(r#"= format!("{}", (*self.count.borrow()));"#),
            "state interpolation in template:\n{}",
            out
        );
    }

    #[test]
    fn arithmetic_body_lowering_binop_sub_direct() {
        use crate::ast::{Span, Stmt};
        use crate::view::ast::Loc;
        let loc = Loc { line: 1, column: 1 };
        let component = ExpandedComponent {
            name: "Counter".to_string(),
            loc,
            state: vec![ExpandedStateField {
                name: "count".to_string(),
                type_expr: TypeExpr::Named("Int".to_string()),
                default: Expr::Int(0, Span::default()),
                loc,
            }],
            events: vec![ExpandedEventHandler {
                name: "decrement".to_string(),
                params: vec![],
                body: vec![Stmt::Assign {
                    target: AssignTarget::Ident("count".to_string(), Span::default()),
                    type_: None,
                    value: Expr::BinOp {
                        op: BinOpKind::Sub,
                        left: Box::new(Expr::Ident("count".to_string(), Span::default())),
                        right: Box::new(Expr::Int(1, Span::default())),
                        span: Span::default(),
                    },
                    span: Span::default(),
                }],
                loc,
            }],
            template: None,
            style: None,
        };
        let out = emit_component(&component).unwrap();
        assert!(
            out.contains("let __rhs = ((*self.count.borrow()) - 1i64);"),
            "subtract lowering:\n{}",
            out
        );
    }

    // ---- Mount + render -----------------------------------------

    #[test]
    fn emit_mount_signature_and_style_helper_call() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("pub fn mount(self: &Rc<Self>, selector: &str) -> Result<(), JsValue> {"),
            "mount signature:\n{}",
            out
        );
        // Scoped style → helper name contains `_counter_c_` (kebab + hex)
        assert!(
            out.contains("__inject_style_Counter_counter_c_"),
            "style helper call:\n{}",
            out
        );
        assert!(
            out.contains(".dyn_into::<HtmlElement>()?;"),
            "dyn_into cast:\n{}",
            out
        );
    }

    #[test]
    fn emit_render_clears_and_rebuilds() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("fn render(self: &Rc<Self>) {"),
            "render signature:\n{}",
            out
        );
        assert!(
            out.contains("while let Some(child) = root.first_child() {"),
            "clear loop:\n{}",
            out
        );
        assert!(
            out.contains("let _ = root.remove_child(&child);"),
            "remove_child:\n{}",
            out
        );
    }

    #[test]
    fn emit_template_element_creates_div_with_scoped_class() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains(r#"document.create_element("div").unwrap();"#),
            "div creation:\n{}",
            out
        );
        // 11.3.c already suffixed the class attribute — the emit
        // just copies the suffixed value. Actual shape is
        // `class="<original> <original>-<scope_class>"`, and the
        // scope_class from 11.3.c is `counter-c-<8hex>` (kebab of
        // "Counter" + `-c-` + FNV-1a hash), so the full attr reads
        // `class="counter counter-counter-c-<hex>"` (original class
        // `counter` + scope suffix `counter-c-<hex>`).
        assert!(
            out.contains(r#".set_attribute("class", "counter counter-counter-c-"#),
            "scoped class attr:\n{}",
            out
        );
    }

    #[test]
    fn emit_interpolation_uses_format_and_state_borrow() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        // The exact var counter (`__interp0` vs `__interp4`) depends
        // on how many earlier elements/texts the render walker
        // allocated. Assert the format! + borrow shape without
        // pinning the counter — it changes if the template grows.
        assert!(
            out.contains(r#"= format!("{}", (*self.count.borrow()));"#),
            "interpolation format! call:\n{}",
            out
        );
    }

    #[test]
    fn emit_event_attr_wires_click_via_closure() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("let __closure = Closure::wrap(Box::new(move |_evt: Event| {"),
            "closure wrap:\n{}",
            out
        );
        assert!(
            out.contains("Counter::decrement(&__self_clone);"),
            "handler call:\n{}",
            out
        );
        assert!(
            out.contains(
                r#".add_event_listener_with_callback("click", __closure.as_ref().unchecked_ref()).unwrap();"#
            ),
            "add_event_listener:\n{}",
            out
        );
        assert!(
            out.contains("__closure.forget();"),
            "closure forget:\n{}",
            out
        );
    }

    // ---- Style helper -------------------------------------------

    #[test]
    fn emit_scoped_style_helper_dedups_with_atomic_bool() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("fn __inject_style_Counter_counter_c_"),
            "helper signature:\n{}",
            out
        );
        assert!(
            out.contains("static INJECTED: AtomicBool = AtomicBool::new(false);"),
            "dedup atomic:\n{}",
            out
        );
        assert!(
            out.contains("if INJECTED.swap(true, Ordering::SeqCst) { return; }"),
            "swap check:\n{}",
            out
        );
        // CSS body should be scoped: the `.counter` selector gets
        // suffixed to `.counter-<scope_class>` by 11.3.b, and the
        // scope_class is `counter-c-<8hex>`, so the full selector
        // reads `.counter-counter-c-<hex>` (double `counter-`).
        assert!(
            out.contains("counter-counter-c-"),
            "scoped CSS body has double-prefix (11.3.b apply_scope + 11.3.c scope class):\n{}",
            out
        );
    }

    #[test]
    fn emit_global_style_helper_named_global() {
        let src = r#"component Foo {
  state { x: Int = 0 }

  <template>
    <div></div>
  </template>

  <style global>
    body { margin: 0; }
  </style>
}"#;
        let expanded = parse_expand(src);
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("fn __inject_style_Foo_global()"),
            "global helper:\n{}",
            out
        );
        assert!(
            out.contains("body { margin: 0; }"),
            "global CSS body:\n{}",
            out
        );
    }

    #[test]
    fn emit_no_style_no_helper() {
        let src = r#"component Bare {
  state { x: Int = 0 }

  <template>
    <div></div>
  </template>
}"#;
        let expanded = parse_expand(src);
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            !out.contains("__inject_style"),
            "no style helper expected:\n{}",
            out
        );
        // mount() also should NOT call any injector
        assert!(
            !out.contains("__inject_style_Bare"),
            "no injector call in mount:\n{}",
            out
        );
    }

    // ---- Errors for the deferred subset -------------------------

    #[test]
    fn emit_rejects_str_state_field() {
        let src = r#"component Foo {
  state { name: Str = "hi" }

  <template>
    <div>{name}</div>
  </template>
}"#;
        let expanded = parse_expand(src);
        let err = emit_component(&expanded.components[0]).unwrap_err();
        assert!(
            err.message.contains("Str") && err.message.contains("11.4.c"),
            "str error:\n{}",
            err
        );
        assert!(
            err.context.contains("component `Foo`"),
            "context names component:\n{}",
            err
        );
    }

    #[test]
    fn emit_rejects_if_directive() {
        let src = r#"component Foo {
  state { x: Int = 0 }

  <template>
    <div>{#if x > 0}<span>positive</span>{/if}</div>
  </template>
}"#;
        let expanded = parse_expand(src);
        let err = emit_component(&expanded.components[0]).unwrap_err();
        assert!(err.message.contains("`{#if"), "if error:\n{}", err);
        assert!(
            err.message.contains("11.4.c"),
            "deferred citation:\n{}",
            err
        );
    }

    #[test]
    fn emit_rejects_for_directive() {
        let src = r#"component Foo {
  state { x: Int = 0 }
  event bump() { x = 1 }

  <template>
    <div>{#for n in 0..3}<span>hi</span>{/for}</div>
  </template>
}"#;
        let expanded = parse_expand(src);
        let err = emit_component(&expanded.components[0]).unwrap_err();
        assert!(err.message.contains("`{#for"), "for error:\n{}", err);
        assert!(
            err.message.contains("11.4.c"),
            "deferred citation:\n{}",
            err
        );
    }

    #[test]
    fn emit_rejects_slot() {
        let src = r#"component Foo {
  state { x: Int = 0 }

  <template>
    <div><slot /></div>
  </template>
}"#;
        let expanded = parse_expand(src);
        let err = emit_component(&expanded.components[0]).unwrap_err();
        assert!(err.message.contains("<slot"), "slot error:\n{}", err);
        assert!(err.message.contains("11.5"), "deferred citation:\n{}", err);
    }

    #[test]
    fn emit_rejects_non_click_event() {
        let src = r#"component Foo {
  state { x: Int = 0 }
  event bump() { x = 1 }

  <template>
    <input @input="bump" />
  </template>
}"#;
        let expanded = parse_expand(src);
        let err = emit_component(&expanded.components[0]).unwrap_err();
        assert!(err.message.contains("@input"), "event kind error:\n{}", err);
        assert!(
            err.message.contains("11.4.c"),
            "deferred citation:\n{}",
            err
        );
    }

    #[test]
    fn emit_rejects_handler_with_params() {
        // Direct construction — the surface parser rejects `event
        // f(x) { ... }` syntax before we can test it via source, but
        // the emitter's guard against handler params is worth
        // asserting independently as a defense in depth.
        use crate::ast::{Param, Span};
        use crate::view::ast::Loc;
        let component = ExpandedComponent {
            name: "Foo".to_string(),
            loc: Loc { line: 1, column: 1 },
            state: vec![],
            events: vec![ExpandedEventHandler {
                name: "handle".to_string(),
                params: vec![Param {
                    name: "x".to_string(),
                    type_: Some(TypeExpr::Named("Int".to_string())),
                    default: None,
                    varargs: false,
                    name_span: Span::default(),
                }],
                body: vec![],
                loc: Loc { line: 1, column: 1 },
            }],
            template: None,
            style: None,
        };
        let err = emit_component(&component).unwrap_err();
        assert!(
            err.message.contains("11.5"),
            "handler param error:\n{}",
            err
        );
        assert!(
            err.context.contains("handler `handle`"),
            "context:\n{}",
            err
        );
    }

    // ---- emit_module --------------------------------------------

    #[test]
    fn emit_module_includes_preamble_and_all_components() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_module(&expanded).unwrap();
        assert!(
            out.starts_with("// Generated by fitz view WASM emitter"),
            "preamble header:\n{}",
            &out[..200.min(out.len())]
        );
        assert!(
            out.contains("use wasm_bindgen::prelude::*;"),
            "wasm_bindgen import:\n{}",
            &out[..500.min(out.len())]
        );
        assert!(
            out.contains("use web_sys::{Event, HtmlElement};"),
            "web_sys import:\n{}",
            &out[..500.min(out.len())]
        );
        // And the component still lands after the preamble
        assert!(
            out.contains("pub struct Counter {"),
            "component decl after preamble present"
        );
    }

    // ---- sanitize_ident + rust_string_literal -------------------

    #[test]
    fn sanitize_ident_replaces_hyphens_with_underscores() {
        assert_eq!(sanitize_ident("counter-c-abc12345"), "counter_c_abc12345");
        assert_eq!(sanitize_ident("Foo"), "Foo");
        assert_eq!(sanitize_ident("with_underscore"), "with_underscore");
    }

    #[test]
    fn rust_string_literal_escapes_quotes_and_newlines() {
        assert_eq!(rust_string_literal("hola"), "\"hola\"");
        assert_eq!(
            rust_string_literal("with \"quote\""),
            "\"with \\\"quote\\\"\""
        );
        assert_eq!(rust_string_literal("line1\nline2"), "\"line1\\nline2\"");
    }
}
