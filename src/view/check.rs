// view/check.rs — Phase 11.2.b — type-check an `ExpandedViewFile`
// produced by `super::expand`.
//
// **Scope (mini-commit 3 of 11.2.b — closes 11.2.b entirely)**:
// state field defaults + event handler bodies + template
// interpolations + `@event="handler"` attr cross-refs.
//
// - State field defaults must be compatible with their declared
//   `TypeExpr` (mini-commit 1 rule, preserved).
// - Event handler bodies are checked as `async fn`s with the
//   component's state fields visible as let-bindings in the outer
//   scope + the handler's own params inside the fn scope
//   (mini-commit 2).
// - Template `{expr}` interpolations (both direct in text nodes and
//   inside HTML attribute values) are checked in the same env as
//   the handler bodies. Additionally, the result type must be
//   *`Str`-friendly* (something the SSR/client target can convert
//   to a string): primitives (Int/Float/Str/Bool/Null/Bytes), Range,
//   List/Map, Date/DateTime/Uuid, nominals (all types get an
//   auto-`Display` impl in codegen), and `Nullable<inner>` when
//   `inner` is `Str`-friendly. Explicitly rejected: Function,
//   Result<T,E>, Future<T>, WsConn, DbConn/DbRow, QueryBuilder,
//   Aggregated, Secret<T> — those must be unwrapped/awaited/
//   matched before rendering (mini-commit 2).
// - `@event="handler"` attrs on template elements reference a bare
//   identifier that MUST name a declared `event ...` handler in
//   the same component. When the reference is broken, the error
//   suggests the nearest declared handler (Levenshtein distance
//   ≤ 3) if one exists, or lists all available handlers when the
//   set is small (≤ 5). Cross-refs are validated even when state
//   defaults have errors — they don't depend on the classic
//   checker and users benefit from seeing both categories together
//   (mini-commit 3).
//
// **Strategy**: synth-and-delegate — build the smallest classic
// Fitz program that expresses each check we want, run
// `crate::types::check_program`, and remap emitted `FitzError`s
// back to `CheckError`s carrying the correct blob `Loc` +
// `context` label. This keeps the view checker tiny and forces the
// classic checker to remain the single source of truth for
// compatibility rules.
//
// **Cascade avoidance**: when a component has ANY state field
// default error, we skip handler + interpolation checks for that
// component. Reason: those checks would run against an env where
// the failing state field's binding still exists (the classic
// checker registers the binding with its declared type even if
// the default is bogus), but the mini-commit 1 error already told
// the user what to fix. Running the second layer would just duplicate
// the same error many times over across every handler / interpolation
// that touches the failing field. Users fix state, then re-run.

use super::ast::Loc;
use super::expand::{
    ChildComponentProp, ExpandedAttr, ExpandedComponent, ExpandedEventHandler, ExpandedStateField,
    ExpandedTemplate, ExpandedTemplateNode, ExpandedViewFile, ExpandedViewImport,
};
use crate::ast::{AssignTarget, Expr, Pattern, Span, Stmt, TypeExpr};
use crate::types::{check_program, Type};
use std::fmt;

/// A type-check error carries the classic checker's message plus a
/// `Loc` inside the `.fitzv` file and a `context` label naming the
/// component and blob (e.g. `"component 'Card': state field 'count'"`).
/// The caller can format these for the CLI, an LSP diagnostic, or a
/// build report.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckError {
    pub message: String,
    pub loc: Loc,
    pub context: String,
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "view check error at {}:{} — {} ({})",
            self.loc.line, self.loc.column, self.message, self.context
        )
    }
}

impl std::error::Error for CheckError {}

/// Type-check an expanded view file. Accumulates every error found;
/// does NOT short-circuit on the first one (unlike `expand`, which
/// aborts on the first parse error — the model there is "you can't
/// type-check what you can't parse"). Type errors are independent, so
/// we surface them all — modulo the cascade-avoidance rule described
/// at the top of the file: within a component, handler +
/// interpolation checks are skipped when state field defaults have
/// errors, because those errors' consequences would flood the
/// output.
///
/// The order of returned errors is: components in file order;
/// within a component, state fields first (declaration order), then
/// event handlers (declaration order), then template interpolations
/// (source order — depth-first walk of the template AST).
pub fn check(file: &ExpandedViewFile) -> Vec<CheckError> {
    check_with_imported_components(file, &[])
}

/// Like [`check`], but with the surfaces of cross-file `<Child />`
/// components loaded from imported `.fitzv` siblings (Phase 11.7 —
/// cross-file composition). Each imported [`ExpandedComponent`] joins the
/// component map that `check_child_components` consults, so composition of
/// an imported child (prop existence + type-compat, `@event` binding
/// existence, slot-fill) is validated against its real surface instead of
/// being reported as an unknown component.
///
/// Local components win on a name collision (a local component shadows an
/// imported one of the same name). When `imported` is empty this is
/// byte-for-byte identical to the same-file check.
///
/// Only the WASM-client build path (`fitz build --target wasm-client`)
/// supplies imported components; the SSR / classic-import path keeps
/// calling [`check`] with no cross-file surface (cross-file composition is
/// a client-WASM capability).
pub fn check_with_imported_components(
    file: &ExpandedViewFile,
    imported: &[ExpandedComponent],
) -> Vec<CheckError> {
    let mut errors = Vec::new();
    // §9.dd (2026-07-16) — Convert top-level `from X import Y` view
    // imports to classic `Stmt::FromImport` ONCE at the top of the
    // check pass. Passed to every synth-program builder so the
    // classic checker sees the imported nominals in scope (registered
    // via `resolve_program`'s `Pass 1` walk which turns FromImport
    // names into TypeEnv nominals with `fields: None` — enough for
    // `List<Message>` and `Message { ... }` struct literals to pass
    // the classic checker's shape validation).
    let import_stmts = imports_to_from_import_stmts(&file.imports);
    for component in &file.components {
        let state_before = errors.len();
        for field in &component.state {
            check_state_field(component.name.as_str(), field, &import_stmts, &mut errors);
        }
        let state_errored = errors.len() > state_before;
        // Event attr cross-refs are pure structural comparisons
        // against the declared handler set — they don't route through
        // the classic checker and don't depend on state validity, so
        // they run even when state has errors. Reporting them here
        // means a user with BOTH a broken state field AND a broken
        // `@click="undeclared"` sees both categories together.
        if let Some(template) = &component.template {
            check_template_event_attrs(component, template, &mut errors);
        }
        // Phase 11.5.d — child-component composition. Runs even
        // when state has errors on THIS component, because the
        // props being validated reference OTHER components'
        // state; the cascade concern (compound "your state is
        // broken AND props are broken") isn't relevant.
        if let Some(template) = &component.template {
            check_child_components(file, imported, component, template, &mut errors);
        }
        // Cascade avoidance — only check handlers + interpolations
        // when the state is clean, so consequential errors don't
        // pile up on top of the actual bug.
        if !state_errored {
            for handler in &component.events {
                check_event_handler(component, handler, &import_stmts, &mut errors);
            }
            if let Some(template) = &component.template {
                check_template_for_iters(component, template, &import_stmts, &mut errors);
                check_template_interpolations(component, template, &import_stmts, &mut errors);
                check_template_if_conds(component, template, &import_stmts, &mut errors);
            }
        }
    }
    errors
}

fn check_state_field(
    component_name: &str,
    field: &ExpandedStateField,
    imports: &[Stmt],
    errors: &mut Vec<CheckError>,
) {
    // Synthesise `<field.name>: <field.type_expr> = <field.default>`
    // as a single top-level Stmt::Assign. Every span is `Span::ZERO`
    // because the classic checker would use it to point at the source
    // — we intercept the emitted error and replace its position with
    // the state field's blob `Loc`.
    //
    // §9.dd — Prepend imported nominal stubs so `List<Message>`
    // (with Message declared in a sibling `.fitz`/`.fitzv` and
    // imported via `from message import Message`) resolves cleanly.
    let mut program: Vec<Stmt> = Vec::with_capacity(imports.len() + 1);
    for imp in imports {
        program.push(imp.clone());
    }
    program.push(Stmt::Assign {
        target: AssignTarget::Ident(field.name.clone(), Span::ZERO),
        type_: Some(field.type_expr.clone()),
        value: field.default.clone(),
        is_let: true,
        span: Span::ZERO,
    });
    let (_env, _info, _defs, classic_errors) = check_program(&program);
    for e in classic_errors {
        errors.push(CheckError {
            message: e.message,
            loc: field.loc,
            context: format!(
                "component '{}': state field '{}'",
                component_name, field.name
            ),
        });
    }
}

/// Check an event handler body. Synthesises a program of the shape:
///
/// ```text
/// let field1: T1 = default1
/// let field2: T2 = default2
/// async fn handler_name(params) { body }
/// ```
///
/// The classic checker resolves the state field types, records them
/// as let-bindings visible in the enclosing scope, and then enters
/// the fn body with the params in a nested scope. Reassignments in
/// the body (e.g. `is_editing = true`) are validated against the
/// annotated declared type of the field (preserved because the state
/// field lets carry `type_: Some(...)`, which marks the binding as
/// `annotated: true` in `VarBinding` — see the post-5a debt note in
/// `CLAUDE.md`).
///
/// The handler is emitted as `async fn` so `.await` and async
/// method chains work inside the body. Handler bodies that don't
/// use async are compatible with an async fn signature (unused-async
/// warnings are not emitted by the classic checker).
fn check_event_handler(
    component: &ExpandedComponent,
    handler: &ExpandedEventHandler,
    imports: &[Stmt],
    errors: &mut Vec<CheckError>,
) {
    // Include ONLY the current handler's body; the other handlers
    // are pushed as fn signatures with empty bodies so handler-to-
    // handler calls resolve without accidentally surfacing their
    // body errors here (each handler's body is checked in its own
    // pass via `check_event_handler`).
    let program = build_env_program(component, imports, Some(&handler.name), None, &[]);
    let (_env, _info, _defs, classic_errors) = check_program(&program);
    for e in classic_errors {
        errors.push(CheckError {
            message: e.message,
            loc: handler.loc,
            context: format!(
                "component '{}': event handler '{}'",
                component.name, handler.name,
            ),
        });
    }
}

/// Check every `{expr}` interpolation nested in the template — both
/// in text nodes and in HTML attribute values. Each interpolation
/// goes through TWO checks:
///
/// 1. The expression must type-check in the state-seeded env
///    (same as handler bodies).
/// 2. The resulting type must be `Str`-friendly (see
///    `is_str_friendly`).
///
/// The two checks run in ONE `check_program` pass per interpolation
/// so we can inspect the inferred type from the returned `TypeInfo`.
/// Overhead per interpolation is a full classic-checker walk over
/// `<state fields> + let __check = <interp expr>` — bounded by the
/// state field count, which is small (<20 in practice).
fn check_template_interpolations(
    component: &ExpandedComponent,
    template: &ExpandedTemplate,
    imports: &[Stmt],
    errors: &mut Vec<CheckError>,
) {
    let mut interpolations: Vec<InterpolationRef<'_>> = Vec::new();
    let mut for_scope: Vec<ForBinding<'_>> = Vec::new();
    collect_interpolations(&template.roots, &mut interpolations, &mut for_scope);

    for (idx, interp) in interpolations.iter().enumerate() {
        // The synth uses a distinctive check ident per interpolation
        // so the classic checker's TypeInfo picks up the inferred
        // type at the interp expr's own span.
        let check_var = format!("__view_interp_check_{}", idx);
        let interp_span = interp.expr.span();
        // Handlers are included in the env so `{handler_name}`
        // interpolations resolve — the resulting Function type then
        // trips the Str-friendly rule for a clear error. This
        // matches how Vue/React expose methods to templates.
        let program = build_env_program(
            component,
            imports,
            None,
            Some(Stmt::Assign {
                target: AssignTarget::Ident(check_var, Span::ZERO),
                type_: None,
                value: interp.expr.clone(),
                is_let: true,
                span: Span::ZERO,
            }),
            &interp.for_scope,
        );
        let (env, type_info, _defs, classic_errors) = check_program(&program);
        for e in classic_errors {
            errors.push(CheckError {
                message: e.message,
                loc: interp.loc,
                context: interp.context(component),
            });
        }
        // Str-friendly check. If the classic checker failed to infer
        // a type at the interp expr's span, silently skip: some
        // upstream error already surfaced above and there's no useful
        // extra info to add.
        if let Some(ty) = type_info.type_at(interp_span) {
            if !is_str_friendly(ty) {
                errors.push(CheckError {
                    message: format!(
                        "template interpolation must render to a string; type `{}` is not Str-friendly (unwrap Result with `match`, await Future, unwrap Nullable with `?`, or use `.expose()` on a Secret before interpolating)",
                        ty.display(&env)
                    ),
                    loc: interp.loc,
                    context: interp.context(component),
                });
            }
        }
    }
}

/// Check every `{#if cond}` condition. Each cond must (a) type-check
/// in the state-seeded env (same as interpolations + handler bodies),
/// and (b) resolve to `Bool` (or `Any`/`Nullable(Bool)`/`PyAny` under
/// gradual escapes). The nested `children` of an `{#if}` are already
/// walked by `collect_interpolations` + `collect_event_attrs`, so
/// interpolations inside `{#if}` bodies get checked by their own
/// passes — this function only validates the cond itself.
///
/// When the `{#if}` lives inside one or more enclosing `{#for}`
/// blocks, the cond sees the for bindings in scope — mini-commit 2
/// added the `for_scope` chain to `IfCondRef` and this pass wraps
/// the synthesised extra stmt in the corresponding `Stmt::For` chain
/// via `build_env_program`.
fn check_template_if_conds(
    component: &ExpandedComponent,
    template: &ExpandedTemplate,
    imports: &[Stmt],
    errors: &mut Vec<CheckError>,
) {
    let mut conds: Vec<IfCondRef<'_>> = Vec::new();
    let mut for_scope: Vec<ForBinding<'_>> = Vec::new();
    collect_if_conds(&template.roots, &mut conds, &mut for_scope);

    for (idx, cond_ref) in conds.iter().enumerate() {
        let check_var = format!("__view_if_cond_check_{}", idx);
        let cond_span = cond_ref.cond.span();
        let program = build_env_program(
            component,
            imports,
            None,
            Some(Stmt::Assign {
                target: AssignTarget::Ident(check_var, Span::ZERO),
                type_: None,
                value: cond_ref.cond.clone(),
                is_let: true,
                span: Span::ZERO,
            }),
            &cond_ref.for_scope,
        );
        let (env, type_info, _defs, classic_errors) = check_program(&program);
        for e in classic_errors {
            errors.push(CheckError {
                message: e.message,
                loc: cond_ref.loc,
                context: cond_ref.context(component),
            });
        }
        // Cond must be Bool (or a compatible gradual type). If we
        // can't infer the type, upstream classic errors above will
        // already have surfaced — silently skip like the interp path.
        if let Some(ty) = type_info.type_at(cond_span) {
            if !is_bool_compatible(ty) {
                errors.push(CheckError {
                    message: format!(
                        "`{{#if}}` condition must evaluate to Bool; type `{}` is not compatible (unwrap Result with `match`, await Future, or compare/negate before using in `{{#if}}`)",
                        ty.display(&env),
                    ),
                    loc: cond_ref.loc,
                    context: cond_ref.context(component),
                });
            }
        }
    }
}

/// Check every `{#for x in iter}` iter expression. Each iter must
/// type-check in the (state + outer-for-scope)-seeded env; the
/// classic `Stmt::For` checker will additionally reject non-iterable
/// types (`the `for` iterable must be List, Range or Map, received
/// ...`). The binding `x` is not exercised here — interpolations and
/// conds inside the for body carry it in their own `for_scope` and
/// get checked by their own passes.
///
/// Nested `{#for}` iters (`{#for post in user.posts}` under
/// `{#for user in users}`) work: the outer for is included in
/// `for_scope`, so `user.posts` sees `user` in scope.
fn check_template_for_iters(
    component: &ExpandedComponent,
    template: &ExpandedTemplate,
    imports: &[Stmt],
    errors: &mut Vec<CheckError>,
) {
    let mut iters: Vec<ForIterRef<'_>> = Vec::new();
    let mut for_scope: Vec<ForBinding<'_>> = Vec::new();
    collect_for_iters(&template.roots, &mut iters, &mut for_scope);

    for iter_ref in iters {
        // Synthesise `for <var> in <iter> { }` — the classic For
        // checker resolves the iter type and rejects non-iterables
        // with a clear message. Wrap in the outer for chain so nested
        // fors reference each other's bindings correctly.
        let stmt = Stmt::For {
            var: Pattern::Ident(iter_ref.var.to_string(), Span::ZERO),
            iter: iter_ref.iter.clone(),
            body: Vec::new(),
            label: None,
            span: Span::ZERO,
        };
        let program = build_env_program(component, imports, None, Some(stmt), &iter_ref.for_scope);
        let (_env, _info, _defs, classic_errors) = check_program(&program);
        for e in classic_errors {
            errors.push(CheckError {
                message: e.message,
                loc: iter_ref.loc,
                context: iter_ref.context(component),
            });
        }
    }
}

/// A `for x in iter` binding active for some region of the template
/// tree. Used by the collectors to attach an outer-scope chain to
/// every `IfCondRef` / `InterpolationRef` / `ForIterRef` so the
/// synth-and-delegate program mirrors the source's real scoping.
#[derive(Clone)]
struct ForBinding<'a> {
    var: &'a str,
    iter: &'a Expr,
}

/// Reference to an `{#if cond}` block, carrying enough context to
/// attribute errors correctly.
struct IfCondRef<'a> {
    cond: &'a Expr,
    loc: Loc,
    /// Chain of enclosing `{#for}` bindings, outer-most to inner-most.
    /// Empty for `{#if}` outside any for.
    for_scope: Vec<ForBinding<'a>>,
}

impl IfCondRef<'_> {
    fn context(&self, component: &ExpandedComponent) -> String {
        format!(
            "component '{}': template `{{#if}}` condition",
            component.name
        )
    }
}

/// Reference to a `{#for x in iter}` iter expression, carrying the
/// outer for scope (empty for a top-level for) so the synth wraps
/// the iter check in the right `Stmt::For` chain.
struct ForIterRef<'a> {
    var: &'a str,
    iter: &'a Expr,
    loc: Loc,
    for_scope: Vec<ForBinding<'a>>,
}

impl ForIterRef<'_> {
    fn context(&self, component: &ExpandedComponent) -> String {
        format!(
            "component '{}': template `{{#for {} in ...}}` iter",
            component.name, self.var,
        )
    }
}

fn collect_if_conds<'a>(
    nodes: &'a [ExpandedTemplateNode],
    out: &mut Vec<IfCondRef<'a>>,
    for_scope: &mut Vec<ForBinding<'a>>,
) {
    for node in nodes {
        match node {
            ExpandedTemplateNode::Text(_)
            | ExpandedTemplateNode::Interpolation { .. }
            | ExpandedTemplateNode::Slot { .. }
            | ExpandedTemplateNode::ChildComponent { .. } => {}
            ExpandedTemplateNode::Element { children, .. } => {
                collect_if_conds(children, out, for_scope);
            }
            ExpandedTemplateNode::If {
                cond,
                children,
                else_children,
                loc,
            } => {
                out.push(IfCondRef {
                    cond,
                    loc: *loc,
                    for_scope: for_scope.clone(),
                });
                // Recurse into both branches so nested `{#if}` conds
                // (in either the then or else side) are collected too.
                collect_if_conds(children, out, for_scope);
                if let Some(else_kids) = else_children {
                    collect_if_conds(else_kids, out, for_scope);
                }
            }
            ExpandedTemplateNode::For {
                var,
                iter,
                children,
                ..
            } => {
                for_scope.push(ForBinding {
                    var: var.as_str(),
                    iter,
                });
                collect_if_conds(children, out, for_scope);
                for_scope.pop();
            }
        }
    }
}

fn collect_for_iters<'a>(
    nodes: &'a [ExpandedTemplateNode],
    out: &mut Vec<ForIterRef<'a>>,
    for_scope: &mut Vec<ForBinding<'a>>,
) {
    for node in nodes {
        match node {
            ExpandedTemplateNode::Text(_)
            | ExpandedTemplateNode::Interpolation { .. }
            | ExpandedTemplateNode::Slot { .. }
            | ExpandedTemplateNode::ChildComponent { .. } => {}
            ExpandedTemplateNode::Element { children, .. } => {
                collect_for_iters(children, out, for_scope);
            }
            ExpandedTemplateNode::If {
                children,
                else_children,
                ..
            } => {
                collect_for_iters(children, out, for_scope);
                if let Some(else_kids) = else_children {
                    collect_for_iters(else_kids, out, for_scope);
                }
            }
            ExpandedTemplateNode::For {
                var,
                iter,
                children,
                loc,
            } => {
                out.push(ForIterRef {
                    var: var.as_str(),
                    iter,
                    loc: *loc,
                    for_scope: for_scope.clone(),
                });
                for_scope.push(ForBinding {
                    var: var.as_str(),
                    iter,
                });
                collect_for_iters(children, out, for_scope);
                for_scope.pop();
            }
        }
    }
}

/// A type is `Bool`-compatible for `{#if}` when it's the concrete
/// `Bool`, or a gradual escape (`Any`/`PyAny`), or `Nullable<Bool>`
/// (JS-style truthiness on the null branch is deliberately NOT
/// modelled — the user must handle null explicitly with `?` or a
/// match; but the checker can't distinguish "null is falsy" from
/// "programmer made a mistake" without more context, so we accept
/// `Bool?` and let the runtime's semantics decide. Same rationale
/// as classic `if` — see `types.rs`).
fn is_bool_compatible(ty: &Type) -> bool {
    match ty {
        Type::Bool | Type::Any | Type::PyAny => true,
        Type::Nullable(inner) => matches!(**inner, Type::Bool | Type::Any | Type::PyAny),
        _ => false,
    }
}

/// Build the component's synth env — state field let-bindings plus
/// every declared handler as an `async fn`. Handlers other than the
/// one identified by `include_body_for` (if any) are emitted with
/// empty bodies so their signatures resolve for cross-handler calls
/// but their bodies don't accidentally surface errors here (each
/// handler's body is checked in its own pass). If `include_body_for`
/// is `None`, every handler is emitted body-less — typical for
/// interpolation checks, which never re-check any handler body.
///
/// The classic checker's `preregister_fn_signatures` phase makes
/// handler declarations visible to every other stmt regardless of
/// declaration order, so handler-to-handler calls (mutual recursion,
/// one handler invoking another) resolve without further care.
/// §9.dd — Convert each `ExpandedViewImport` into a classic
/// `Stmt::FromImport` for injection into the view checker's synth
/// programs. Names are wrapped as `(name, None)` (no alias support
/// in the `.fitzv` view syntax MVP — aliases would require alias-
/// aware TypeEnv patching + emit-side rewriting; deferred).
fn imports_to_from_import_stmts(imports: &[ExpandedViewImport]) -> Vec<Stmt> {
    imports
        .iter()
        .map(|imp| Stmt::FromImport {
            path: imp.path.clone(),
            // Post S.1 (2026-07-17): `imp.names` is now `Vec<(String,
            // Option<String>)>` — pass through directly to the
            // classic AST which uses the same shape (PreF8.4).
            names: imp.names.clone(),
            span: Span::ZERO,
        })
        .collect()
}

fn build_env_program(
    component: &ExpandedComponent,
    imports: &[Stmt],
    include_body_for: Option<&str>,
    extra: Option<Stmt>,
    for_scope: &[ForBinding<'_>],
) -> Vec<Stmt> {
    let mut program: Vec<Stmt> =
        Vec::with_capacity(imports.len() + component.state.len() + component.events.len() + 2);
    // §9.dd — Prepend imported nominal decls so `resolve_program`'s
    // Pass 1 walk registers them as TypeEnv nominals (with
    // `fields: None`). The classic checker will then accept
    // `List<Message>` in state and `Message { ... }` struct
    // literals in event bodies without "unknown type" errors.
    for imp in imports {
        program.push(imp.clone());
    }
    for field in &component.state {
        program.push(Stmt::Assign {
            target: AssignTarget::Ident(field.name.clone(), Span::ZERO),
            type_: Some(field.type_expr.clone()),
            value: field.default.clone(),
            is_let: true,
            span: Span::ZERO,
        });
    }
    // §9.cc V-4 (2026-07-16) — When checking an event handler body,
    // synth a top-level `let payload: Map<Str, Str> = {}` so the body
    // can reference `payload.has(...)` / `payload["key"]` without
    // "unknown variable" errors from the classic checker. Parallel to
    // §9.z's SSR emitter fix (which populated `payload` in the walker's
    // event-body local_scope) — this closes the same gap on the
    // checker side. Only added for event-handler-body checks
    // (`include_body_for.is_some()`); interpolation / if-cond passes
    // call `build_env_program(..., None, ...)` so they never see
    // `payload` (`payload` has no meaning outside event body context).
    //
    // Trade-off: the synth RHS `{}` is a placeholder. At runtime the
    // real `payload` comes from the emitted fn signature (§9.z shape:
    // `fn X_event(state: X, payload: Map<Str, Str>) -> X`), not from
    // this initializer. Reassignments to `payload` inside the body
    // are accepted by the checker (mutable `let`) but would fail at
    // runtime (immutable param). Acceptable MVP gotcha; canonical
    // usage is READ-ONLY payload lookup.
    if include_body_for.is_some() {
        program.push(Stmt::Assign {
            target: AssignTarget::Ident("payload".to_string(), Span::ZERO),
            type_: Some(TypeExpr::Generic {
                name: "Map".to_string(),
                args: vec![
                    TypeExpr::Named("Str".to_string()),
                    TypeExpr::Named("Str".to_string()),
                ],
            }),
            value: Expr::Map(Vec::new(), Span::ZERO),
            is_let: true,
            span: Span::ZERO,
        });
    }
    for handler in &component.events {
        let include_body = Some(handler.name.as_str()) == include_body_for;
        program.push(Stmt::FnDef {
            name: handler.name.clone(),
            params: handler.params.clone(),
            return_type: None,
            body: if include_body {
                handler.body.clone()
            } else {
                Vec::new()
            },
            is_async: true,
            decorators: Vec::new(),
            span: Span::ZERO,
        });
    }
    if let Some(s) = extra {
        program.push(wrap_stmt_in_for_scope(s, for_scope));
    }
    program
}

/// Wrap `stmt` in a chain of `Stmt::For { ... }` matching the
/// enclosing `{#for}` scope (outer-most to inner-most). Produces:
///
/// ```text
/// for outer.var in outer.iter {
///     for inner.var in inner.iter {
///         <stmt>
///     }
/// }
/// ```
///
/// so the classic checker walks the for scopes, resolves each
/// binding's type from its iter, and then enters the innermost body
/// with all bindings visible. When `for_scope` is empty, returns
/// `stmt` unchanged.
fn wrap_stmt_in_for_scope(stmt: Stmt, for_scope: &[ForBinding<'_>]) -> Stmt {
    let mut inner = stmt;
    for binding in for_scope.iter().rev() {
        inner = Stmt::For {
            var: Pattern::Ident(binding.var.to_string(), Span::ZERO),
            iter: binding.iter.clone(),
            body: vec![inner],
            label: None,
            span: Span::ZERO,
        };
    }
    inner
}

/// Reference to a template interpolation, carrying enough context
/// to attribute errors correctly.
struct InterpolationRef<'a> {
    expr: &'a Expr,
    loc: Loc,
    /// Attribute name when this interpolation lives inside an HTML
    /// attribute value; `None` for text-node interpolations. Used
    /// only to enrich the error `context` label.
    attr_name: Option<String>,
    /// Chain of enclosing `{#for}` bindings (outer to inner). Empty
    /// for interps outside any for. Used by the checker to wrap the
    /// synth extra stmt in `Stmt::For` layers so the classic checker
    /// sees the bindings.
    for_scope: Vec<ForBinding<'a>>,
}

impl InterpolationRef<'_> {
    fn context(&self, component: &ExpandedComponent) -> String {
        match &self.attr_name {
            Some(name) => format!(
                "component '{}': template attr '{}' interpolation",
                component.name, name
            ),
            None => format!("component '{}': template interpolation", component.name),
        }
    }
}

fn collect_interpolations<'a>(
    nodes: &'a [ExpandedTemplateNode],
    out: &mut Vec<InterpolationRef<'a>>,
    for_scope: &mut Vec<ForBinding<'a>>,
) {
    for node in nodes {
        match node {
            ExpandedTemplateNode::Text(_)
            | ExpandedTemplateNode::Slot { .. }
            | ExpandedTemplateNode::ChildComponent { .. } => {}
            ExpandedTemplateNode::Interpolation { expr, loc } => {
                out.push(InterpolationRef {
                    expr,
                    loc: *loc,
                    attr_name: None,
                    for_scope: for_scope.clone(),
                });
            }
            ExpandedTemplateNode::Element {
                attrs, children, ..
            } => {
                for attr in attrs {
                    match attr {
                        ExpandedAttr::Interpolation { name, expr, loc } => {
                            out.push(InterpolationRef {
                                expr,
                                loc: *loc,
                                attr_name: Some(name.clone()),
                                for_scope: for_scope.clone(),
                            });
                        }
                        // Each `{expr}` segment of a mixed value
                        // (`class="toast toast-{kind}"`) is type-checked
                        // like a full attribute interpolation.
                        ExpandedAttr::MixedInterpolation {
                            name,
                            segments,
                            loc,
                        } => {
                            for seg in segments {
                                if let crate::view::expand::AttrValueSegment::Expr(expr) = seg {
                                    out.push(InterpolationRef {
                                        expr,
                                        loc: *loc,
                                        attr_name: Some(name.clone()),
                                        for_scope: for_scope.clone(),
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                }
                collect_interpolations(children, out, for_scope);
            }
            ExpandedTemplateNode::If {
                children,
                else_children,
                ..
            } => {
                collect_interpolations(children, out, for_scope);
                if let Some(else_kids) = else_children {
                    collect_interpolations(else_kids, out, for_scope);
                }
            }
            ExpandedTemplateNode::For {
                var,
                iter,
                children,
                ..
            } => {
                for_scope.push(ForBinding {
                    var: var.as_str(),
                    iter,
                });
                collect_interpolations(children, out, for_scope);
                for_scope.pop();
            }
        }
    }
}

/// Cross-check every `@event="handler"` attr against the component's
/// declared event handler set. Each reference to an unknown handler
/// produces one `CheckError` naming both the event and the referenced
/// handler; when there's a near-miss (Levenshtein distance ≤ 3) we
/// append a "did you mean ...?" hint pointing at the closest
/// declared handler. When the declared set is small (≤ 5) and no
/// near-miss exists, we list every available handler so the user can
/// pick without opening another file.
///
/// Runs independently of state validity — see the note in `check()`.
fn check_template_event_attrs(
    component: &ExpandedComponent,
    template: &ExpandedTemplate,
    errors: &mut Vec<CheckError>,
) {
    let mut event_attrs: Vec<EventAttrRef<'_>> = Vec::new();
    collect_event_attrs(&template.roots, &mut event_attrs);
    if event_attrs.is_empty() {
        return;
    }
    let declared: Vec<&str> = component.events.iter().map(|h| h.name.as_str()).collect();
    for attr in event_attrs {
        if declared.contains(&attr.handler_name) {
            continue;
        }
        let hint = suggestion_for(attr.handler_name, &declared);
        let message = match hint {
            Some(h) => format!(
                "template attr @{}=\"{}\" references handler '{}' but no such `event` is declared in this component. Did you mean '{}'?",
                attr.event_name, attr.handler_name, attr.handler_name, h,
            ),
            None if declared.is_empty() => format!(
                "template attr @{}=\"{}\" references handler '{}' but this component declares no `event ...` handlers",
                attr.event_name, attr.handler_name, attr.handler_name,
            ),
            None if declared.len() <= 5 => format!(
                "template attr @{}=\"{}\" references handler '{}' but no such `event` is declared in this component. Available: {}",
                attr.event_name,
                attr.handler_name,
                attr.handler_name,
                declared.join(", "),
            ),
            None => format!(
                "template attr @{}=\"{}\" references handler '{}' but no such `event` is declared in this component",
                attr.event_name, attr.handler_name, attr.handler_name,
            ),
        };
        errors.push(CheckError {
            message,
            loc: attr.loc,
            context: format!(
                "component '{}': template event attr '@{}'",
                component.name, attr.event_name,
            ),
        });
    }
}

/// Reference to a `@event="handler"` binding on a template element.
struct EventAttrRef<'a> {
    event_name: &'a str,
    handler_name: &'a str,
    loc: Loc,
}

fn collect_event_attrs<'a>(nodes: &'a [ExpandedTemplateNode], out: &mut Vec<EventAttrRef<'a>>) {
    for node in nodes {
        match node {
            ExpandedTemplateNode::Text(_)
            | ExpandedTemplateNode::Interpolation { .. }
            | ExpandedTemplateNode::Slot { .. }
            | ExpandedTemplateNode::ChildComponent { .. } => {}
            ExpandedTemplateNode::Element {
                attrs, children, ..
            } => {
                for attr in attrs {
                    if let ExpandedAttr::Event {
                        event_name,
                        handler_name,
                        loc,
                    } = attr
                    {
                        out.push(EventAttrRef {
                            event_name,
                            handler_name,
                            loc: *loc,
                        });
                    }
                }
                collect_event_attrs(children, out);
            }
            ExpandedTemplateNode::If {
                children,
                else_children,
                ..
            } => {
                collect_event_attrs(children, out);
                if let Some(else_kids) = else_children {
                    collect_event_attrs(else_kids, out);
                }
            }
            ExpandedTemplateNode::For { children, .. } => {
                collect_event_attrs(children, out);
            }
        }
    }
}

/// Return the declared handler closest to `target` when the
/// Levenshtein distance is ≤ 3, else `None`. The threshold catches
/// realistic typos (`stat` for `start`, `saev` for `save`) without
/// suggesting random matches on unrelated names.
fn suggestion_for<'a>(target: &str, declared: &[&'a str]) -> Option<&'a str> {
    let mut best: Option<(&'a str, usize)> = None;
    for candidate in declared {
        let d = levenshtein_distance(target, candidate);
        if d > 3 {
            continue;
        }
        match best {
            None => best = Some((*candidate, d)),
            Some((_, best_d)) if d < best_d => best = Some((*candidate, d)),
            _ => {}
        }
    }
    best.map(|(c, _)| c)
}

/// Iterative Levenshtein with two rolling rows — O(a·b) time,
/// O(min(a,b)) space. Small enough to inline here; not worth pulling
/// in a crate.
fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        curr[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// A type is *`Str`-friendly* if the SSR/client target can convert
/// it to a string suitable for embedding in HTML output.
///
/// **Allow-list** (renders cleanly via existing Display impls):
/// - Primitives: Int, Float, Str, Bool, Null, Bytes
/// - Range (renders as `0..10`)
/// - List/Map (Display renders as `[1, 2, 3]` / `{"a": 1}`)
/// - Date/DateTime/Uuid (ISO 8601 / canonical)
/// - Nominal(_) (all user `type`s receive an auto-Display in codegen)
/// - Nullable(inner) — recurses; `null` renders as the string `"null"`
/// - Any, PyAny — gradual escape
///
/// **Block-list** (must be unwrapped before rendering):
/// - Result<T,E> — `match`/`?` first
/// - Future<T> — `.await` first
/// - WsConn — opaque handle, no textual form
/// - DbConn, DbRow — opaque handles
/// - QueryBuilder, Aggregated — call a terminal (`.all`/`.first`/…)
/// - Secret<T> — deliberately opaque; `.expose()` explicitly
/// - Function — a callable, not a value to display
fn is_str_friendly(ty: &Type) -> bool {
    match ty {
        Type::Int
        | Type::Float
        | Type::Str
        | Type::Bool
        | Type::Null
        | Type::Bytes
        | Type::Range
        | Type::Date
        | Type::DateTime
        | Type::Uuid
        | Type::Nominal(_)
        | Type::Any
        | Type::PyAny => true,
        Type::List(_) | Type::Map(_, _) | Type::Tuple(_) => true,
        Type::Nullable(inner) => is_str_friendly(inner),
        Type::Result { .. }
        | Type::Future(_)
        | Type::WsConn { .. }
        | Type::DbConn
        | Type::DbRow
        | Type::QueryBuilder(_)
        | Type::Aggregated(_)
        | Type::Secret(_)
        | Type::Function { .. } => false,
    }
}

// ---------------------------------------------------------------------------
// Phase 11.5.d — child-component composition (`<Child prop="v" />`)
// ---------------------------------------------------------------------------

/// Validate every `<Child />` composition site in `template`:
///
/// - The referenced child must be declared in the same
///   `ExpandedViewFile` (typo hint via Levenshtein ≤ 3, else a
///   full list of available components).
/// - The child is NOT `component` itself (self-reference would be
///   infinite descent — rejected with a "cannot mount itself"
///   message).
/// - Every prop's `field_name` matches a declared state field of
///   the child (typo hint via the state field names of the
///   child).
/// - No two props with the same `field_name` (accidental
///   overwrite trap — reject with a targeted message).
/// - Each prop's `raw_value` coerces into the field's declared
///   `TypeExpr`. Only primitive scalars are supported in MVP:
///   `Int`, `Float`, `Str`, `Bool`, and `Nullable<T>` of those.
///   Compound types (`List`, `Map`, `Nominal`) are rejected with
///   a 11.6+ pointer.
///
/// On success, we OVERWRITE `raw_value` in-place with a Rust
/// literal that the emitter drops straight into
/// `*child.field.borrow_mut() = <raw_value>;`. Encoding the
/// coerced representation this way keeps the emitter dumb and
/// avoids threading an "already-coerced" side-channel through
/// the AST — the AST holds the coerced value directly.
///
/// Since it mutates the AST, this pass takes `&ExpandedViewFile`
/// via UnsafeCell would be silly — we clone the file lookup
/// state (a `HashMap<&str, &ExpandedComponent>` snapshot) and
/// perform the mutation via `check_child_components_mut` on a
/// separately-borrowed template that lives on the SAME
/// component. Since the checker is called with `&file` and the
/// template is `Option<ExpandedTemplate>` on the component,
/// this would require `&mut` access — which the current
/// public `check(&file)` signature does not permit. Rather
/// than change the public signature, we accumulate the coerced
/// values in a side table and let the emitter (a downstream
/// consumer) apply them via `coerce_child_prop_raw_value`
/// helper.
///
/// So the concrete plan: this pass VALIDATES only, producing
/// errors. The emitter re-derives the coerced representation
/// using the same helper. Kept in sync via unit tests.
fn check_child_components<'a>(
    file: &'a ExpandedViewFile,
    imported: &'a [ExpandedComponent],
    component: &ExpandedComponent,
    template: &ExpandedTemplate,
    errors: &mut Vec<CheckError>,
) {
    let component_map = build_component_map(file, imported);
    for node in &template.roots {
        walk_child_components(node, &component_map, &component.name, errors);
    }
}

/// Build a `component_name → component` lookup for the file, unioning the
/// local components with any cross-file imported ones (Phase 11.7). Local
/// components are inserted last so they WIN on a name collision — an
/// imported component of the same name is shadowed by the local one.
fn build_component_map<'a>(
    file: &'a ExpandedViewFile,
    imported: &'a [ExpandedComponent],
) -> std::collections::HashMap<&'a str, &'a ExpandedComponent> {
    let mut map: std::collections::HashMap<&'a str, &'a ExpandedComponent> =
        std::collections::HashMap::new();
    for c in imported {
        map.insert(c.name.as_str(), c);
    }
    for c in &file.components {
        map.insert(c.name.as_str(), c);
    }
    map
}

fn walk_child_components(
    node: &ExpandedTemplateNode,
    component_map: &std::collections::HashMap<&str, &ExpandedComponent>,
    parent_name: &str,
    errors: &mut Vec<CheckError>,
) {
    match node {
        ExpandedTemplateNode::ChildComponent {
            name,
            props,
            events,
            slot_content,
            loc,
            ..
        } => {
            // Phase 11.7.b R2b — the `key="{expr}"` attribute is
            // validated at emit time (required for a `<Child />` inside
            // a `{#for}`, ignored for static sites). The checker
            // validates the child exists, its props coerce, (11.7.c)
            // each `@event` binding names a real child event + parent
            // handler, and (11.7.d) slot content pairs with a `<slot />`.
            validate_child_site(
                name,
                props,
                events,
                slot_content,
                *loc,
                component_map,
                parent_name,
                errors,
            );
        }
        ExpandedTemplateNode::Element { children, .. } => {
            for child in children {
                walk_child_components(child, component_map, parent_name, errors);
            }
        }
        ExpandedTemplateNode::If {
            children,
            else_children,
            ..
        } => {
            for child in children {
                walk_child_components(child, component_map, parent_name, errors);
            }
            if let Some(else_kids) = else_children {
                for child in else_kids {
                    walk_child_components(child, component_map, parent_name, errors);
                }
            }
        }
        ExpandedTemplateNode::For { children, .. } => {
            for child in children {
                walk_child_components(child, component_map, parent_name, errors);
            }
        }
        ExpandedTemplateNode::Text(_)
        | ExpandedTemplateNode::Interpolation { .. }
        | ExpandedTemplateNode::Slot { .. } => {}
    }
}

/// True when a component's template contains a `<slot />` (Phase
/// 11.7.d). Used to validate that slot content has somewhere to go.
fn component_has_slot(component: &ExpandedComponent) -> bool {
    fn node_has_slot(node: &ExpandedTemplateNode) -> bool {
        match node {
            ExpandedTemplateNode::Slot { .. } => true,
            ExpandedTemplateNode::Element { children, .. } => children.iter().any(node_has_slot),
            ExpandedTemplateNode::If {
                children,
                else_children,
                ..
            } => {
                children.iter().any(node_has_slot)
                    || else_children
                        .as_ref()
                        .is_some_and(|els| els.iter().any(node_has_slot))
            }
            ExpandedTemplateNode::For { children, .. } => children.iter().any(node_has_slot),
            _ => false,
        }
    }
    component
        .template
        .as_ref()
        .is_some_and(|t| t.roots.iter().any(node_has_slot))
}

#[allow(clippy::too_many_arguments)]
fn validate_child_site(
    child_name: &str,
    props: &[ChildComponentProp],
    events: &[super::expand::ChildEventBinding],
    slot_content: &[ExpandedTemplateNode],
    loc: Loc,
    component_map: &std::collections::HashMap<&str, &ExpandedComponent>,
    parent_name: &str,
    errors: &mut Vec<CheckError>,
) {
    if child_name == parent_name {
        errors.push(CheckError {
            message: format!(
                "component `{child_name}` cannot mount itself — that would \
                 recurse forever at render time. Break the cycle by \
                 extracting the shared UI into a separate component."
            ),
            loc,
            context: format!("component '{parent_name}': template `<{child_name} />`"),
        });
        return;
    }

    let child = match component_map.get(child_name) {
        Some(c) => c,
        None => {
            let names: Vec<&str> = component_map.keys().copied().collect();
            let hint = suggestion_for(child_name, &names);
            let msg = match hint {
                Some(near) => format!(
                    "unknown component `<{child_name} />` — did you mean `<{near} />`? \
                     (declared in this file: {})",
                    format_component_list(&names)
                ),
                None => format!(
                    "unknown component `<{child_name} />`. Declare `component {child_name} \
                     {{ ... }}` in this file, or check the spelling. Available: {}.",
                    format_component_list(&names)
                ),
            };
            errors.push(CheckError {
                message: msg,
                loc,
                context: format!("component '{parent_name}': template `<{child_name} />`"),
            });
            return;
        }
    };

    // Phase 11.7.d — if the parent provides slot content
    // (`<Child>content</Child>`), the child must have a `<slot />` to put
    // it in; otherwise the content would be silently dropped.
    if !slot_content.is_empty() && !component_has_slot(child) {
        errors.push(CheckError {
            message: format!(
                "`<{child_name}>...</{child_name}>` provides slot content, but \
                 component `{child_name}` has no `<slot />` in its template — the \
                 content would be dropped. Add `<slot />` to `{child_name}`, or use \
                 `<{child_name} />` (self-closing)."
            ),
            loc,
            context: format!("component '{parent_name}': template `<{child_name} />`"),
        });
    }

    // Phase 11.7.c — validate each `@event="handler"` binding: the child
    // must declare `event <event_name>`, and the parent must declare
    // `event <handler_name>` (the handler that runs when it bubbles).
    for binding in events {
        if !child.events.iter().any(|e| e.name == binding.event_name) {
            errors.push(CheckError {
                message: format!(
                    "`<{child_name} @{}=\"...\" />` binds an event the child does not \
                     declare. Add `event {}() {{ ... }}` to component `{child_name}`, or \
                     fix the event name.",
                    binding.event_name, binding.event_name
                ),
                loc: binding.loc,
                context: format!("component '{parent_name}': template `<{child_name} />`"),
            });
        }
        if let Some(parent) = component_map.get(parent_name) {
            if !parent.events.iter().any(|e| e.name == binding.handler_name) {
                errors.push(CheckError {
                    message: format!(
                        "`<{child_name} @{}=\"{}\" />` refers to `{}`, which is not an \
                         `event` of the parent component `{parent_name}`.",
                        binding.event_name, binding.handler_name, binding.handler_name
                    ),
                    loc: binding.loc,
                    context: format!("component '{parent_name}': template `<{child_name} />`"),
                });
            }
        }
    }

    // Guard against duplicate props (accidental double-assign).
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for prop in props {
        if !seen.insert(prop.field_name.as_str()) {
            errors.push(CheckError {
                message: format!(
                    "duplicate prop `{}` on `<{child_name} />`. Each prop must appear \
                     at most once.",
                    prop.field_name
                ),
                loc: prop.loc,
                context: format!("component '{parent_name}': template `<{child_name} />`"),
            });
        }
    }

    // Each prop must match a declared state field, and its value
    // must coerce.
    for prop in props {
        let field = child.state.iter().find(|f| f.name == prop.field_name);
        let field = match field {
            Some(f) => f,
            None => {
                let field_names: Vec<&str> = child.state.iter().map(|f| f.name.as_str()).collect();
                let hint = suggestion_for(&prop.field_name, &field_names);
                let msg = match hint {
                    Some(near) => format!(
                        "unknown prop `{}` on `<{child_name} />` — did you mean `{near}`? \
                         (`{child_name}` declares: {})",
                        prop.field_name,
                        format_field_list(&field_names)
                    ),
                    None => format!(
                        "unknown prop `{}` on `<{child_name} />`. `{child_name}` \
                         declares no such state field. Available: {}.",
                        prop.field_name,
                        format_field_list(&field_names)
                    ),
                };
                errors.push(CheckError {
                    message: msg,
                    loc: prop.loc,
                    context: format!("component '{parent_name}': template `<{child_name} />`"),
                });
                continue;
            }
        };

        // K-3 remainder + S.3: interpolated props (`prop="{expr}"`)
        // bypass the coercion helper — the value is a Fitz expression
        // the SSR emitter inlines verbatim. Post S.3 (2026-07-17) we
        // do a light type-check for the safest shape: bare `Ident`
        // referring to a parent state field. If the parent's field
        // type is not compatible with the child's declared field
        // type, surface it at check time. Richer expr shapes (BinOp,
        // Call, Field access, etc.) still skip — full type inference
        // would need to route the classic checker's TypeEnv through
        // this pass, which is out of scope for a small refinement.
        // False negatives on those shapes still show up at classic-
        // checker time on the emitted module.
        if prop.is_interpolated() {
            if let Some(expr) = &prop.expr {
                if let Some(msg) = light_check_interpolated_prop(
                    expr,
                    &field.type_expr,
                    &field.name,
                    child_name,
                    component_map.get(parent_name).copied(),
                ) {
                    errors.push(CheckError {
                        message: msg,
                        loc: prop.loc,
                        context: format!("component '{parent_name}': template `<{child_name} />`"),
                    });
                }
            }
            continue;
        }

        if let Err(msg) = coerce_child_prop_raw_value(&prop.raw_value, &field.type_expr) {
            errors.push(CheckError {
                message: format!(
                    "prop `{}=\"{}\"` on `<{child_name} />`: {}",
                    prop.field_name, prop.raw_value, msg
                ),
                loc: prop.loc,
                context: format!("component '{parent_name}': template `<{child_name} />`"),
            });
        }
    }
}

fn format_component_list(names: &[&str]) -> String {
    if names.is_empty() {
        return "(none — this file declares no components?)".to_string();
    }
    let mut sorted: Vec<&&str> = names.iter().collect();
    sorted.sort();
    sorted
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_field_list(names: &[&str]) -> String {
    if names.is_empty() {
        return "(no state fields declared)".to_string();
    }
    names
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Coerce a raw static-attribute value (a string as captured by
/// the HTML sub-parser) into a Rust literal suitable for
/// `*child.field.borrow_mut() = <literal>;`. Returns `Ok(literal)`
/// on success, `Err(msg)` on failure.
///
/// Supported primitive targets:
/// - `Str` → the value wrapped as `"..."` with proper escaping.
/// - `Int` → parsed as `i64`, emitted as `123i64`.
/// - `Float` → parsed as `f64`, emitted as `1.5f64`.
/// - `Bool` → strictly `"true"` or `"false"`, emitted as `true`/`false`.
/// - `Nullable<T>` → literal `"null"` emits `None`; otherwise
///   recurse into `T` and wrap the result in `Some(...)`.
/// - `List<T>` → comma-separated raw values; empty string yields
///   `vec![]`, otherwise each item is recursively coerced and the
///   result is `vec![item1, item2, ...]`. Whitespace around commas
///   is trimmed. **No escaping of commas today** (K-3 MVP).
///
/// `Map<K, V>`, nominal, function, and tuple types still reject
/// with a targeted pointer to the deferred sub-phase.
///
/// This helper is `pub(crate)` so `codegen_wasm.rs::emit_child_
/// component` uses the SAME coercion when emitting, ensuring the
/// checker and the emitter agree bit-for-bit.
/// S.3 (2026-07-17) — light type-check for interpolated `<Child
/// prop="{expr}" />` props. Catches the common typo pattern of a
/// bare `Ident` referring to a parent state field whose type is
/// incompatible with the child's declared field type. Richer expr
/// shapes (`{n + 1}`, `{obj.field}`, method calls, imports) skip —
/// full type inference isn't in scope for this small refinement,
/// and the classic checker running over the emitted module still
/// catches those mismatches downstream. Returns `Some(msg)` on
/// mismatch, `None` otherwise.
fn light_check_interpolated_prop(
    expr: &crate::ast::Expr,
    child_field_type: &crate::ast::TypeExpr,
    child_field_name: &str,
    child_name: &str,
    parent: Option<&ExpandedComponent>,
) -> Option<String> {
    let name = match expr {
        crate::ast::Expr::Ident(n, _) => n,
        _ => return None, // Only bare Ident — see fn doc.
    };
    let parent = parent?;
    // Look up the ident on the parent's state fields. If not a
    // state field, it's likely an imported name or an outer local
    // (`{#for x in xs}`) — those we can't check here. Skip.
    let parent_field = parent.state.iter().find(|f| &f.name == name)?;
    if type_expr_compatible(&parent_field.type_expr, child_field_type) {
        return None;
    }
    Some(format!(
        "interpolated prop `{child_field_name}=\"{{{name}}}\"` on `<{child_name} />` — \
         parent state field `{name}: {}` is not compatible with the child's declared \
         field type `{}: {}`. Either declare the parent's `{name}` with a matching type \
         or wrap the value (e.g. `\"{{{name}.to_string()}}\"` if the target expects `Str`).",
        parent_field.type_expr.display_name(),
        child_field_name,
        child_field_type.display_name(),
    ))
}

/// Structural compatibility between two view-side `TypeExpr`s.
/// Rules:
/// - Same shape → compatible (recursive Named / Generic / Nullable).
/// - `T` is compatible with `T?` (assignment lifts to Some).
/// - `Null` shape from parent isn't representable here (parent state
///   defaults are concrete types), so we don't handle it.
///
/// Kept local to the view checker — the classic checker's
/// `types::is_compatible` operates on resolved `Type`s, not on the
/// syntactic `TypeExpr` we have at this point in the pipeline.
fn type_expr_compatible(parent_ty: &crate::ast::TypeExpr, child_ty: &crate::ast::TypeExpr) -> bool {
    use crate::ast::TypeExpr as T;
    // T is compatible with T? (child promoted to Nullable).
    if let T::Nullable(inner) = child_ty {
        if !matches!(parent_ty, T::Nullable(_)) {
            return type_expr_compatible(parent_ty, inner);
        }
    }
    match (parent_ty, child_ty) {
        (T::Named(a), T::Named(b)) => a == b,
        (T::Nullable(a), T::Nullable(b)) => type_expr_compatible(a, b),
        (T::Generic { name: na, args: aa }, T::Generic { name: nb, args: ab }) => {
            na == nb
                && aa.len() == ab.len()
                && aa
                    .iter()
                    .zip(ab.iter())
                    .all(|(x, y)| type_expr_compatible(x, y))
        }
        _ => false,
    }
}

pub(crate) fn coerce_child_prop_raw_value(
    raw: &str,
    type_expr: &crate::ast::TypeExpr,
) -> Result<String, String> {
    use crate::ast::TypeExpr as T;
    match type_expr {
        T::Named(name) => match name.as_str() {
            "Str" => Ok(rust_str_literal(raw)),
            "Int" => raw.parse::<i64>().map(|n| format!("{n}i64")).map_err(|_| {
                format!(
                    "expected an integer literal for `Int` field, got `{raw}`. \
                     Use a bare integer like `count=\"42\"`."
                )
            }),
            "Float" => raw.parse::<f64>().map(|n| format!("{n}f64")).map_err(|_| {
                format!(
                    "expected a float literal for `Float` field, got `{raw}`. \
                     Use a decimal like `rate=\"1.5\"`."
                )
            }),
            "Bool" => match raw {
                "true" => Ok("true".to_string()),
                "false" => Ok("false".to_string()),
                _ => Err(format!(
                    "expected `\"true\"` or `\"false\"` for `Bool` field, got `{raw}`."
                )),
            },
            other => Err(format!(
                "prop coercion to `{other}` — static props for nominal / user-defined \
                 types are not supported in Phase 11.5.d. Only `Str`, `Int`, `Float`, \
                 `Bool` (and their `Nullable<T>` wrappers) coerce today; compound \
                 types land alongside Phase 11.6+."
            )),
        },
        T::Nullable(inner) => {
            if raw == "null" {
                Ok("None".to_string())
            } else {
                coerce_child_prop_raw_value(raw, inner).map(|lit| format!("Some({lit})"))
            }
        }
        T::Generic { name, args } => {
            if name == "List" && args.len() == 1 {
                // K-3: List<primitive> via comma-separated raw values.
                // Empty string → empty vec. Whitespace around commas is trimmed.
                // Each item is recursively coerced to the inner type. No escaping
                // of commas today (MVP scope — matches Vue's `:tags="'a,b,c'"`
                // pattern when tags contain no commas).
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return Ok("vec![]".to_string());
                }
                let inner = &args[0];
                let mut lits: Vec<String> = Vec::new();
                for item in trimmed.split(',') {
                    let piece = item.trim();
                    let lit = coerce_child_prop_raw_value(piece, inner).map_err(|e| {
                        format!("List<{}> item `{}`: {}", inner.head_name(), piece, e)
                    })?;
                    lits.push(lit);
                }
                Ok(format!("vec![{}]", lits.join(", ")))
            } else if name == "Map" && args.len() == 2 {
                // S.2: Map<Str, Str> static props via `k=v,k=v` convention.
                // Only supported for Map<Str, Str> because the raw HTML attr
                // is a string — non-Str keys/values would need per-piece
                // parsing beyond simple split (Int/Float/Bool from strings
                // hit ambiguity). Users needing richer maps should use
                // interpolation `<Child meta="{someMap}" />` (K-3 remainder).
                //
                // Empty string → `vec![]` (empty Vec<(K, V)>). Whitespace
                // around commas AND around `=` is trimmed. No comma or `=`
                // escaping today (MVP — same trade-off as List<primitive>).
                let is_str = |t: &crate::ast::TypeExpr| {
                    matches!(
                        t,
                        crate::ast::TypeExpr::Named(n) if n == "Str"
                    )
                };
                if !(is_str(&args[0]) && is_str(&args[1])) {
                    return Err(format!(
                        "prop coercion to `Map<{}, {}>` — static props for `Map` are \
                         only supported for `Map<Str, Str>` today (the raw attribute \
                         value is a string, so `k=v` pieces can't disambiguate \
                         Int/Float/Bool). Use interpolation `<Child meta=\"{{someMap}}\" />` \
                         (K-3 remainder) for richer Map shapes.",
                        args[0].head_name(),
                        args[1].head_name()
                    ));
                }
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return Ok("vec![]".to_string());
                }
                let mut lits: Vec<String> = Vec::new();
                for pair in trimmed.split(',') {
                    let (k, v) = pair.split_once('=').ok_or_else(|| {
                        format!(
                            "Map<Str, Str> pair `{}` is not in `key=value` form. Use \
                             comma-separated `k=v,k2=v2` pairs.",
                            pair.trim()
                        )
                    })?;
                    let key_str = rust_str_literal(k.trim());
                    let val_str = rust_str_literal(v.trim());
                    lits.push(format!("({key_str}, {val_str})"));
                }
                Ok(format!("vec![{}]", lits.join(", ")))
            } else {
                Err(format!(
                    "prop coercion to `{name}<...>` — static props for this compound \
                     type are not supported today. Only `Str`, `Int`, `Float`, `Bool`, \
                     their `Nullable<T>` wrappers, `List<primitive>` (comma-separated), \
                     and `Map<Str, Str>` (comma-separated `k=v,k=v`) coerce; richer \
                     compound types land alongside Phase 11.7+."
                ))
            }
        }
        T::Function { .. } => Err(
            "prop coercion to a function type — passing callbacks as props is \
             not supported yet. Model the callback as an event bubbled up via \
             `<Child @some_event=\"handler\" />` (11.6+ work). No coercion today."
                .to_string(),
        ),
        T::Tuple(_) => Err(
            "prop coercion to a tuple type — not supported. Extract the tuple \
             into a nominal type (`type MyPair { ... }`) if you need to pass \
             both halves, once nominal-type composition lands (11.6+)."
                .to_string(),
        ),
    }
}

/// Escape `s` into a Rust `"..."` string literal. Handles the
/// small set of characters that would corrupt the emitted source
/// (`"`, `\`, newlines, tabs, and other control chars). Overlaps
/// with `codegen_wasm::rust_string_literal` — kept as a local
/// twin to avoid the module dependency in the checker (checker
/// should not import the emitter).
fn rust_str_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{{{:x}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out.push_str(".to_string()");
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr, Span, TypeExpr};
    use crate::view::ast::Loc;
    use crate::view::expand::{expand, ExpandedComponent, ExpandedStateField};
    use crate::view::parse as view_parse;

    fn check_str(src: &str) -> Vec<CheckError> {
        let raw = view_parse(src).expect("view parses");
        let expanded = expand(&raw).expect("expands cleanly");
        check(&expanded)
    }

    /// Builds an `ExpandedViewFile` with a single component whose only
    /// contents are the given state fields. Bypasses the view lexer +
    /// expander to prove the checker handles generic / nullable /
    /// compound type shapes independently of the POC parser's current
    /// character set. The POC lexer does NOT tokenize `<`, `>`, `?`
    /// today (an intentional gap — the raw block detector treats `<`
    /// as `<template>` / `<style scoped>` opener), so those shapes
    /// can't come through source yet. This helper lets the checker
    /// tests exercise them anyway. The debt is documented in
    /// `docs/fase-11-plan.md` §7.
    fn synth_file(component_name: &str, fields: Vec<ExpandedStateField>) -> ExpandedViewFile {
        ExpandedViewFile {
            imports: Vec::new(),
            components: vec![ExpandedComponent {
                name: component_name.into(),
                loc: Loc::new(1, 1),
                state: fields,
                events: Vec::new(),
                template: None,
                style: None,
            }],
        }
    }

    fn synth_state_field(name: &str, type_expr: TypeExpr, default: Expr) -> ExpandedStateField {
        ExpandedStateField {
            name: name.into(),
            type_expr,
            default,
            loc: Loc::new(1, 1),
        }
    }

    // ---- Mini-commit 1: state field defaults ------------------------------

    #[test]
    fn state_field_str_default_compat_no_errors() {
        // `Str` default is `"Untitled"` — a plain Str literal.
        let src = r#"component Card {
  state {
    title: Str = "Untitled"
  }
}"#;
        assert!(check_str(src).is_empty(), "no errors expected");
    }

    #[test]
    fn state_field_bool_default_compat_no_errors() {
        let src = r#"component Card {
  state {
    is_editing: Bool = false
  }
}"#;
        assert!(check_str(src).is_empty());
    }

    #[test]
    fn state_field_int_to_float_coerces_no_errors() {
        // Classic rule: `Int` compatible with `Float`. The default
        // `0` is `Int` but the declared type is `Float` — must
        // coerce silently.
        let src = r#"component Card {
  state {
    ratio: Float = 0
  }
}"#;
        assert!(check_str(src).is_empty(), "Int→Float should coerce");
    }

    #[test]
    fn state_field_type_mismatch_reports_error_with_context() {
        // `Str` declared, `Int` default. The classic checker
        // emits its "declared as X received a value Y" message.
        let src = r#"component Card {
  state {
    title: Str = 42
  }
}"#;
        let errs = check_str(src);
        assert_eq!(errs.len(), 1, "one mismatch expected: {:?}", errs);
        let e = &errs[0];
        assert!(
            e.context.contains("component 'Card'") && e.context.contains("state field 'title'"),
            "context = {:?}",
            e.context
        );
        assert!(
            e.message.contains("Str") && e.message.contains("Int"),
            "message should name both types, got {:?}",
            e.message
        );
    }

    #[test]
    fn state_field_unknown_nominal_reports_error() {
        // `FooBar` is not a nominal declared anywhere. The classic
        // resolver emits "type `FooBar` not defined".
        let src = r#"component Card {
  state {
    thing: FooBar = 42
  }
}"#;
        let errs = check_str(src);
        assert!(!errs.is_empty(), "unknown type should error");
        let msg = &errs[0].message;
        assert!(
            msg.contains("FooBar"),
            "message should name the offending type, got {:?}",
            msg
        );
    }

    #[test]
    fn state_field_nullable_accepts_null_default_directly_constructed() {
        // Direct construction — the view POC lexer does not tokenize
        // `?` yet, so `subtitle: Str? = null` can't come through
        // source today. Bypasses parse+expand to prove the checker
        // handles `Null → T?`. When the lexer gains `?`, a
        // source-level version of this test lands as part of that
        // mini-commit.
        let file = synth_file(
            "Card",
            vec![synth_state_field(
                "subtitle",
                TypeExpr::Nullable(Box::new(TypeExpr::Named("Str".into()))),
                Expr::Null(Span::ZERO),
            )],
        );
        assert!(check(&file).is_empty(), "null should fit Str?");
    }

    #[test]
    fn state_field_nullable_accepts_concrete_value() {
        // `Str?` also accepts a plain Str — the classic rule
        // `is_compatible(T, T?) = true`.
        let file = synth_file(
            "Card",
            vec![synth_state_field(
                "subtitle",
                TypeExpr::Nullable(Box::new(TypeExpr::Named("Str".into()))),
                Expr::Str("hello".into(), Span::ZERO),
            )],
        );
        assert!(check(&file).is_empty(), "Str should fit Str?");
    }

    #[test]
    fn state_field_list_default_matches_declared_generic() {
        // Direct construction — view POC lexer does not tokenize
        // `<`, `>` yet outside `<template>`/`<style scoped>`, so
        // `List<Str>` can't come through source. Debt tracked in
        // `docs/fase-11-plan.md` §7.
        let file = synth_file(
            "X",
            vec![synth_state_field(
                "tags",
                TypeExpr::Generic {
                    name: "List".into(),
                    args: vec![TypeExpr::Named("Str".into())],
                },
                Expr::List(
                    vec![
                        Expr::Str("a".into(), Span::ZERO),
                        Expr::Str("b".into(), Span::ZERO),
                    ],
                    Span::ZERO,
                ),
            )],
        );
        assert!(check(&file).is_empty(), "List<Str> default should fit");
    }

    #[test]
    fn state_field_list_of_wrong_element_type_reports_error() {
        // `List<Int>` declared, `List<Str>` default → not compatible.
        let file = synth_file(
            "X",
            vec![synth_state_field(
                "xs",
                TypeExpr::Generic {
                    name: "List".into(),
                    args: vec![TypeExpr::Named("Int".into())],
                },
                Expr::List(vec![Expr::Str("nope".into(), Span::ZERO)], Span::ZERO),
            )],
        );
        let errs = check(&file);
        assert_eq!(errs.len(), 1, "one mismatch expected: {:?}", errs);
        assert!(
            errs[0].context.contains("state field 'xs'"),
            "context = {:?}",
            errs[0].context
        );
    }

    #[test]
    fn state_field_map_default_matches_declared_generic() {
        // Same story as List: direct construction because the view
        // POC lexer does not tokenize `<`, `>` yet.
        let file = synth_file(
            "X",
            vec![synth_state_field(
                "meta",
                TypeExpr::Generic {
                    name: "Map".into(),
                    args: vec![TypeExpr::Named("Str".into()), TypeExpr::Named("Int".into())],
                },
                Expr::Map(
                    vec![
                        (Expr::Str("a".into(), Span::ZERO), Expr::Int(1, Span::ZERO)),
                        (Expr::Str("b".into(), Span::ZERO), Expr::Int(2, Span::ZERO)),
                    ],
                    Span::ZERO,
                ),
            )],
        );
        assert!(check(&file).is_empty(), "Map<Str, Int> default should fit");
    }

    #[test]
    fn multiple_state_fields_only_the_bad_one_errors() {
        // Two fields: `title: Str = "ok"` (OK) and
        // `count: Int = "bad"` (mismatch). Only ONE error, and it
        // points at `count`.
        let src = r#"component X {
  state {
    title: Str = "ok"
    count: Int = "bad"
  }
}"#;
        let errs = check_str(src);
        assert_eq!(errs.len(), 1, "only the bad field errors: {:?}", errs);
        assert!(errs[0].context.contains("state field 'count'"));
        assert!(!errs[0].context.contains("state field 'title'"));
    }

    #[test]
    fn multiple_components_each_state_checked_independently() {
        // Two components, one with a good field, one with a bad
        // field. Only the bad one errors, and the context names
        // the correct component.
        let src = r#"component A {
  state { title: Str = "ok" }
}

component B {
  state { count: Int = "bad" }
}"#;
        let errs = check_str(src);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].context.contains("component 'B'"));
        assert!(errs[0].context.contains("state field 'count'"));
    }

    #[test]
    fn error_loc_matches_state_field_blob_loc() {
        // The `Loc` we surface has to be the state field's blob
        // location, not `(0, 0)`. Concretely: the bad field lives
        // on line 3 of this source (the `count: Int = "bad"` line),
        // so the error's line should be 3.
        let src = "component X {\n  state {\n    count: Int = \"bad\"\n  }\n}\n";
        let errs = check_str(src);
        assert_eq!(errs.len(), 1);
        // Line 3 in 1-based coord — the state field's `Loc` sits at
        // its declaration line.
        assert_eq!(errs[0].loc.line, 3, "loc = {:?}", errs[0].loc);
    }

    #[test]
    fn empty_component_produces_no_errors() {
        let src = "component Empty {}";
        assert!(check_str(src).is_empty());
    }

    #[test]
    fn component_without_state_produces_no_errors() {
        let src = r#"component X {
  <template><div>hello</div></template>
}"#;
        assert!(check_str(src).is_empty());
    }

    #[test]
    fn card_component_from_expand_module_type_checks_cleanly() {
        // The canonical Card fixture used in `view::expand::tests`
        // must type-check cleanly under mini-commit 2 too: state
        // defaults are valid, the two handlers reassign fields with
        // matching types, and the template interpolation references
        // an existing state field (`title` — a Str, which IS
        // Str-friendly).
        let src = r#"component Card {
  state {
    title: Str = "Untitled"
    is_editing: Bool = false
  }

  event start() {
    is_editing = true
  }

  event save(new_title: Str) {
    title = new_title
    is_editing = false
  }

  <template>
    <div class="card">
      <div class="title">{title}</div>
      <button @click="start">Edit</button>
    </div>
  </template>

  <style scoped>
    .card { border: 1px solid #ccc; padding: 1rem; }
  </style>
}
"#;
        assert!(
            check_str(src).is_empty(),
            "Card should type-check cleanly end-to-end (mini-commit 2)"
        );
    }

    // ---- Mini-commit 2: event handler bodies ------------------------------

    #[test]
    fn handler_reassigning_state_with_matching_type_ok() {
        // `is_editing = true` reassigns Bool → Bool: valid.
        let src = r#"component X {
  state { is_editing: Bool = false }
  event start() { is_editing = true }
}"#;
        assert!(
            check_str(src).is_empty(),
            "matching-type reassignment should be valid"
        );
    }

    #[test]
    fn handler_reassigning_state_with_wrong_type_reports_error() {
        // `is_editing: Bool = false` declared, handler body
        // reassigns `is_editing = 42`. Classic checker's post-5a
        // rule catches this: the binding is `annotated: true`, so
        // subsequent assignments must match its declared type.
        let src = r#"component X {
  state { is_editing: Bool = false }
  event weird() { is_editing = 42 }
}"#;
        let errs = check_str(src);
        assert_eq!(errs.len(), 1, "wrong-type reassignment errors: {:?}", errs);
        let e = &errs[0];
        assert!(
            e.context.contains("event handler 'weird'"),
            "context = {:?}",
            e.context
        );
        // Loc must be the handler's, not the state field's.
        assert_ne!(e.loc, Loc::new(2, 3), "should not be the state field's loc");
    }

    #[test]
    fn handler_uses_param_as_local_variable() {
        // The handler's param `new_title: Str` must be visible in
        // its body. `title = new_title` reassigns state field
        // `title: Str` with a Str param — valid.
        let src = r#"component X {
  state { title: Str = "" }
  event save(new_title: Str) { title = new_title }
}"#;
        assert!(
            check_str(src).is_empty(),
            "param `new_title` must be in scope inside body"
        );
    }

    #[test]
    fn handler_params_do_not_leak_across_handlers() {
        // `save`'s `new_title` param should NOT be visible inside
        // `start`. If it were, a spurious "undefined" error would NOT
        // trigger — so instead we test the opposite: `start` body
        // referencing `new_title` errors because it's not in scope.
        let src = r#"component X {
  state { title: Str = "" }
  event save(new_title: Str) { title = new_title }
  event start() { title = new_title }
}"#;
        let errs = check_str(src);
        assert!(
            !errs.is_empty(),
            "`start` referencing `new_title` should error"
        );
        assert!(
            errs.iter()
                .any(|e| e.context.contains("event handler 'start'")
                    && e.message.contains("new_title")),
            "errors = {:#?}",
            errs
        );
    }

    #[test]
    fn handler_body_can_reference_state_from_other_handler_scope() {
        // Between handlers, state fields stay visible (they're at
        // the outer scope). `save` reads `title` and reassigns
        // `is_editing`, both fine because both are declared state.
        let src = r#"component X {
  state {
    title: Str = ""
    is_editing: Bool = false
  }
  event save(new_title: Str) {
    title = new_title
    is_editing = false
  }
}"#;
        assert!(check_str(src).is_empty());
    }

    #[test]
    fn handler_body_calling_undefined_ident_reports_error() {
        // `undefined_var` is not a state field nor a param.
        let src = r#"component X {
  state { title: Str = "" }
  event save() { title = undefined_var }
}"#;
        let errs = check_str(src);
        assert!(
            !errs.is_empty(),
            "undefined ident inside handler must error"
        );
        assert!(errs
            .iter()
            .any(|e| e.context.contains("event handler 'save'")));
    }

    #[test]
    fn handler_with_state_error_is_not_double_checked_cascade_avoidance() {
        // State field default has an error. Handler references the
        // same field. Only the state error should surface — the
        // handler check gets skipped (cascade avoidance).
        let src = r#"component X {
  state { count: Int = "bad" }
  event tick() { count = 5 }
}"#;
        let errs = check_str(src);
        assert_eq!(
            errs.len(),
            1,
            "only the state error should surface, not cascaded: {:?}",
            errs
        );
        assert!(errs[0].context.contains("state field 'count'"));
    }

    #[test]
    fn multiple_handlers_only_the_bad_one_errors() {
        let src = r#"component X {
  state {
    title: Str = ""
    is_editing: Bool = false
  }
  event ok_one() { is_editing = true }
  event bad_one() { is_editing = 42 }
}"#;
        let errs = check_str(src);
        assert_eq!(errs.len(), 1, "only the bad handler errors: {:?}", errs);
        assert!(errs[0].context.contains("event handler 'bad_one'"));
        assert!(!errs[0].context.contains("event handler 'ok_one'"));
    }

    // ---- Mini-commit 2: template interpolations --------------------------

    #[test]
    fn interpolation_reads_state_field_ok() {
        // Interpolating a Str state field — Str is Str-friendly.
        let src = r#"component X {
  state { title: Str = "hi" }
  <template><div>{title}</div></template>
}"#;
        assert!(check_str(src).is_empty());
    }

    #[test]
    fn interpolation_of_undefined_ident_reports_error() {
        let src = r#"component X {
  state { title: Str = "hi" }
  <template><div>{missing}</div></template>
}"#;
        let errs = check_str(src);
        assert!(!errs.is_empty());
        assert!(
            errs.iter()
                .any(|e| e.context.contains("template interpolation")
                    && e.message.contains("missing")),
            "errors = {:#?}",
            errs
        );
    }

    #[test]
    fn interpolation_in_attr_value_is_checked() {
        // Attribute value with an interpolation should check as well.
        let src = r#"component X {
  state { title: Str = "hi" }
  <template><input value="{title}" /></template>
}"#;
        assert!(check_str(src).is_empty());
    }

    #[test]
    fn interpolation_in_attr_value_context_names_attr() {
        // Wrong ident in attr interpolation — context label should
        // include the attr name so users find the bug in a busy
        // template.
        let src = r#"component X {
  state { title: Str = "hi" }
  <template><input value="{missing}" /></template>
}"#;
        let errs = check_str(src);
        assert!(!errs.is_empty());
        assert!(
            errs.iter()
                .any(|e| e.context.contains("attr 'value'") && e.context.contains("interpolation")),
            "errors = {:#?}",
            errs
        );
    }

    #[test]
    fn interpolation_of_str_friendly_types_are_ok() {
        // The view POC parser tokenizes state defaults with the same
        // lexer it uses for the shell — Float literals with a `.`
        // are not accepted at that layer yet (deferred with the
        // rest of the state-field literal shape refinement noted in
        // §7 of the plan doc). We exercise Int + Bool through
        // source; Float coverage lives in the direct-construction
        // path further below.
        let src = r#"component X {
  state {
    count: Int = 0
    ok: Bool = true
  }
  <template>
    <div>{count}</div>
    <div>{ok}</div>
  </template>
}"#;
        assert!(
            check_str(src).is_empty(),
            "Int and Bool are both Str-friendly"
        );
    }

    #[test]
    fn interpolation_of_float_field_ok_direct_construction() {
        // Float coverage via direct construction — see the note in
        // `interpolation_of_str_friendly_types_are_ok` about the
        // view POC lexer not accepting `.` in state defaults.
        let file = ExpandedViewFile {
            imports: Vec::new(),
            components: vec![ExpandedComponent {
                name: "X".into(),
                loc: Loc::new(1, 1),
                state: vec![synth_state_field(
                    "ratio",
                    TypeExpr::Named("Float".into()),
                    Expr::Float(0.5, Span::ZERO),
                )],
                events: Vec::new(),
                template: Some(crate::view::expand::ExpandedTemplate {
                    roots: vec![crate::view::expand::ExpandedTemplateNode::Interpolation {
                        expr: Expr::Ident("ratio".into(), Span::new(1, 1)),
                        loc: Loc::new(2, 1),
                    }],
                    loc: Loc::new(2, 1),
                }),
                style: None,
            }],
        };
        assert!(check(&file).is_empty(), "Float is Str-friendly");
    }

    #[test]
    fn interpolation_of_function_value_reports_str_friendly_error() {
        // Reference the handler itself as a value — its type is
        // `Function`, which is NOT Str-friendly. The handler is
        // top-level after synth so its Ident resolves; the type is
        // a Function.
        let src = r#"component X {
  state { title: Str = "" }
  event go() { title = "next" }
  <template><div>{go}</div></template>
}"#;
        let errs = check_str(src);
        assert!(
            errs.iter().any(|e| e.message.contains("Str-friendly")
                && e.context.contains("template interpolation")),
            "expected Str-friendly rejection, got {:#?}",
            errs
        );
    }

    #[test]
    fn interpolation_with_state_error_is_not_double_checked_cascade_avoidance() {
        // Same rule as handlers: state error → skip interpolation
        // checks.
        let src = r#"component X {
  state { count: Int = "bad" }
  <template><div>{count}</div></template>
}"#;
        let errs = check_str(src);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].context.contains("state field 'count'"));
    }

    #[test]
    fn interpolation_expr_arithmetic_ok() {
        // `{count + 1}` — Int + Int → Int, still Str-friendly.
        let src = r#"component X {
  state { count: Int = 0 }
  <template><div>{count + 1}</div></template>
}"#;
        assert!(check_str(src).is_empty());
    }

    #[test]
    fn interpolation_multiple_interpolations_each_checked() {
        // Two interpolations: one references defined `title`, one
        // references undefined `missing`. Only the second errors.
        let src = r#"component X {
  state { title: Str = "" }
  <template>
    <div>{title}</div>
    <div>{missing}</div>
  </template>
}"#;
        let errs = check_str(src);
        assert_eq!(errs.len(), 1, "one error expected: {:?}", errs);
        assert!(errs[0].message.contains("missing"));
    }

    // ---- Mini-commit 2: is_str_friendly unit tests ----------------------

    #[test]
    fn is_str_friendly_accepts_primitives() {
        assert!(is_str_friendly(&Type::Int));
        assert!(is_str_friendly(&Type::Float));
        assert!(is_str_friendly(&Type::Str));
        assert!(is_str_friendly(&Type::Bool));
        assert!(is_str_friendly(&Type::Null));
        assert!(is_str_friendly(&Type::Bytes));
    }

    #[test]
    fn is_str_friendly_accepts_gradual_and_compound() {
        assert!(is_str_friendly(&Type::Any));
        assert!(is_str_friendly(&Type::PyAny));
        assert!(is_str_friendly(&Type::List(Box::new(Type::Int))));
        assert!(is_str_friendly(&Type::Map(
            Box::new(Type::Str),
            Box::new(Type::Int)
        )));
        assert!(is_str_friendly(&Type::Nullable(Box::new(Type::Str))));
    }

    #[test]
    fn is_str_friendly_rejects_result_and_future() {
        assert!(!is_str_friendly(&Type::Result {
            ok: Box::new(Type::Int),
            err: Box::new(Type::Str)
        }));
        assert!(!is_str_friendly(&Type::Future(Box::new(Type::Int))));
    }

    #[test]
    fn is_str_friendly_rejects_secret_and_opaque_handles() {
        assert!(!is_str_friendly(&Type::Secret(Box::new(Type::Str))));
        assert!(!is_str_friendly(&Type::DbConn));
        assert!(!is_str_friendly(&Type::DbRow));
        assert!(!is_str_friendly(&Type::WsConn {
            recv: Box::new(Type::Str),
            send: Box::new(Type::Str),
        }));
        assert!(!is_str_friendly(&Type::Function {
            params: vec![],
            ret: Box::new(Type::Null),
        }));
    }

    #[test]
    fn is_str_friendly_nullable_of_unfriendly_is_unfriendly() {
        // `Result<T,E>?` is still not Str-friendly — the inner
        // Result carries the rejection.
        assert!(!is_str_friendly(&Type::Nullable(Box::new(Type::Result {
            ok: Box::new(Type::Int),
            err: Box::new(Type::Str)
        }))));
    }

    // ---- Mini-commit 3: @event="handler" cross-refs ---------------------

    #[test]
    fn event_attr_referencing_declared_handler_ok() {
        // `@click="start"` and the component declares `event start()`
        // — cross-ref must pass silently.
        let src = r#"component X {
  state { is_editing: Bool = false }
  event start() { is_editing = true }
  <template><button @click="start">Edit</button></template>
}"#;
        assert!(check_str(src).is_empty(), "declared handler must resolve");
    }

    #[test]
    fn event_attr_referencing_undeclared_handler_reports_error() {
        // `@click="missing"` but no `event missing() {...}` is
        // declared. Must error with a context naming the event and
        // the missing handler.
        let src = r#"component X {
  state { title: Str = "" }
  event save() { title = "next" }
  <template><button @click="missing">Go</button></template>
}"#;
        let errs = check_str(src);
        assert!(!errs.is_empty(), "undeclared handler must error");
        let e = errs
            .iter()
            .find(|e| e.context.contains("template event attr '@click'"))
            .expect("event attr error");
        assert!(
            e.message.contains("missing"),
            "message should name the missing handler, got {:?}",
            e.message
        );
    }

    #[test]
    fn event_attr_typo_suggests_nearest_declared_handler() {
        // `@click="stat"` — one letter off from declared `start`.
        // Levenshtein 1 — the hint must recommend `start`.
        let src = r#"component X {
  state { is_editing: Bool = false }
  event start() { is_editing = true }
  <template><button @click="stat">Edit</button></template>
}"#;
        let errs = check_str(src);
        assert_eq!(errs.len(), 1, "one cross-ref error expected: {:?}", errs);
        assert!(
            errs[0].message.contains("Did you mean 'start'"),
            "message must contain suggestion, got {:?}",
            errs[0].message
        );
    }

    #[test]
    fn event_attr_unknown_handler_lists_available_when_small_set() {
        // No near-miss (Levenshtein > 3) but the declared set is
        // small (≤ 5), so the error lists every available handler.
        let src = r#"component X {
  state { title: Str = "" }
  event save() { title = "s" }
  event reset() { title = "" }
  <template><button @click="totally_different_name">X</button></template>
}"#;
        let errs = check_str(src);
        assert_eq!(errs.len(), 1);
        let msg = &errs[0].message;
        assert!(
            msg.contains("Available:"),
            "should list available when no near-miss + small set, got {:?}",
            msg,
        );
        assert!(msg.contains("save"));
        assert!(msg.contains("reset"));
    }

    #[test]
    fn event_attr_unknown_handler_no_declared_events_says_so() {
        // Component declares zero handlers — the error should say so
        // instead of listing an empty set.
        let src = r#"component X {
  state { title: Str = "" }
  <template><button @click="save">Save</button></template>
}"#;
        let errs = check_str(src);
        assert_eq!(errs.len(), 1);
        assert!(
            errs[0].message.contains("declares no `event ...` handlers"),
            "message = {:?}",
            errs[0].message
        );
    }

    #[test]
    fn event_attr_cross_ref_runs_even_when_state_has_errors() {
        // State field has a type mismatch AND the template references
        // an undeclared handler. Both errors must surface — the
        // event-attr check is deliberately independent of state
        // validity.
        let src = r#"component X {
  state { count: Int = "bad" }
  event tick() { count = 5 }
  <template><button @click="undeclared_handler">Go</button></template>
}"#;
        let errs = check_str(src);
        // Expect: state field error + event attr cross-ref error.
        // (Handler body + interpolations skipped by cascade
        // avoidance, but event attrs still surface.)
        assert!(
            errs.iter()
                .any(|e| e.context.contains("state field 'count'")),
            "state error must surface: {:?}",
            errs
        );
        assert!(
            errs.iter()
                .any(|e| e.context.contains("template event attr '@click'")
                    && e.message.contains("undeclared_handler")),
            "event attr error must surface even with state error: {:?}",
            errs
        );
    }

    #[test]
    fn event_attr_multiple_bad_refs_each_reported() {
        // Two elements with two different bad `@event` refs. Each
        // must produce its own CheckError with the right event name
        // in the context.
        let src = r#"component X {
  state { title: Str = "" }
  event save() { title = "s" }
  <template>
    <button @click="save">OK</button>
    <button @click="nope1">Bad 1</button>
    <input @input="nope2" />
  </template>
}"#;
        let errs = check_str(src);
        assert_eq!(errs.len(), 2, "two cross-ref errors expected: {:?}", errs);
        assert!(errs
            .iter()
            .any(|e| e.context.contains("'@click'") && e.message.contains("nope1")));
        assert!(errs
            .iter()
            .any(|e| e.context.contains("'@input'") && e.message.contains("nope2")));
    }

    // ---- Mini-commit 3: helper unit tests -------------------------------

    #[test]
    fn levenshtein_distance_basic_cases() {
        assert_eq!(levenshtein_distance("", ""), 0);
        assert_eq!(levenshtein_distance("", "abc"), 3);
        assert_eq!(levenshtein_distance("abc", ""), 3);
        assert_eq!(levenshtein_distance("same", "same"), 0);
        assert_eq!(levenshtein_distance("stat", "start"), 1);
        assert_eq!(levenshtein_distance("kitten", "sitting"), 3);
        assert_eq!(levenshtein_distance("saev", "save"), 2);
    }

    #[test]
    fn suggestion_for_returns_closest_within_threshold() {
        let declared = vec!["start", "save", "reset"];
        // Distance 1 — clear winner.
        assert_eq!(suggestion_for("stat", &declared), Some("start"));
        // Distance 1 — winner.
        assert_eq!(suggestion_for("sav", &declared), Some("save"));
    }

    #[test]
    fn suggestion_for_none_when_too_far() {
        let declared = vec!["start", "save"];
        // Distance too far for any candidate.
        assert_eq!(suggestion_for("completely_unrelated_name", &declared), None);
    }

    #[test]
    fn suggestion_for_none_when_declared_empty() {
        let declared: Vec<&str> = Vec::new();
        assert_eq!(suggestion_for("anything", &declared), None);
    }

    // ---- view-lexer §7 follow-up: source-level tests for shapes
    // ---- that previously required direct AST construction. Now that
    // ---- the view lexer emits `Lt`, `Gt`, `Question`, `LBracket`,
    // ---- `RBracket`, users can write `List<Str>`, `Map<K, V>`, and
    // ---- `Str?` directly in `.fitzv` state annotations + defaults.
    // ---- Direct-construction variants above stay as coverage of the
    // ---- checker's internal paths, but the source-level equivalents
    // ---- prove the parse→expand→check pipeline handles the
    // ---- destrabado shape end-to-end.

    #[test]
    fn state_field_nullable_str_default_null_source_level() {
        // Source-level equivalent of
        // `state_field_nullable_accepts_null_default_directly_constructed`.
        let src = r#"component X {
  state { subtitle: Str? = null }
}"#;
        assert!(
            check_str(src).is_empty(),
            "Str? = null must round-trip through source"
        );
    }

    #[test]
    fn state_field_nullable_str_default_concrete_source_level() {
        // Source-level equivalent of
        // `state_field_nullable_accepts_concrete_value`.
        let src = r#"component X {
  state { subtitle: Str? = "hello" }
}"#;
        assert!(
            check_str(src).is_empty(),
            "Str? = \"hello\" must round-trip through source"
        );
    }

    #[test]
    fn state_field_list_of_str_source_level() {
        // Source-level equivalent of
        // `state_field_list_default_matches_declared_generic`.
        let src = r#"component X {
  state { tags: List<Str> = [] }
}"#;
        assert!(
            check_str(src).is_empty(),
            "List<Str> = [] must round-trip through source"
        );
    }

    #[test]
    fn state_field_list_of_wrong_element_type_source_level() {
        // Source-level equivalent of
        // `state_field_list_of_wrong_element_type_reports_error`.
        let src = r#"component X {
  state { xs: List<Int> = ["nope"] }
}"#;
        let errs = check_str(src);
        assert_eq!(errs.len(), 1, "one mismatch expected: {:?}", errs);
        assert!(errs[0].context.contains("state field 'xs'"));
    }

    #[test]
    fn state_field_map_str_int_source_level() {
        // Source-level equivalent of
        // `state_field_map_default_matches_declared_generic`.
        let src = r#"component X {
  state { meta: Map<Str, Int> = {} }
}"#;
        assert!(
            check_str(src).is_empty(),
            "Map<Str, Int> = {{}} must round-trip through source"
        );
    }

    #[test]
    fn state_field_nested_generic_source_level() {
        // `List<Map<Str, Int>>` — two levels of `Lt`/`Gt` — must
        // round-trip. Exercises the shell-parser rebuild + classic
        // parser handoff for nested generics.
        let src = r#"component X {
  state { rows: List<Map<Str, Int>> = [] }
}"#;
        assert!(
            check_str(src).is_empty(),
            "nested generics must round-trip through source"
        );
    }

    #[test]
    fn state_field_list_nullable_source_level() {
        // `List<Str>?` — Nullable wrapping a generic — must
        // round-trip. Exercises the `Question` after a `Gt`.
        let src = r#"component X {
  state { xs: List<Str>? = null }
}"#;
        assert!(
            check_str(src).is_empty(),
            "List<Str>? = null must round-trip through source"
        );
    }

    // ---- 11.2.c mini-commit 1: `{#if cond}...{/if}` checker --------

    #[test]
    fn if_cond_bool_state_field_ok() {
        // `{#if is_ready}` where `is_ready: Bool` is declared. Cond
        // resolves in the state env and types as Bool.
        let src = r#"component X {
  state { is_ready: Bool = false }
  <template>{#if is_ready}<div>hi</div>{/if}</template>
}"#;
        assert!(check_str(src).is_empty(), "Bool cond must pass silently");
    }

    #[test]
    fn if_cond_binop_int_gt_int_is_bool_ok() {
        // `{#if count > 0}` — BinOp on Int fields yields Bool.
        let src = r#"component X {
  state { count: Int = 0 }
  <template>{#if count > 0}<span/>{/if}</template>
}"#;
        assert!(check_str(src).is_empty(), "count > 0 is Bool");
    }

    #[test]
    fn if_cond_non_bool_type_reports_error() {
        // `{#if title}` where title: Str — cond is not Bool. Must
        // error with the `{#if}` cond context label.
        let src = r#"component X {
  state { title: Str = "" }
  <template>{#if title}<span/>{/if}</template>
}"#;
        let errs = check_str(src);
        assert_eq!(errs.len(), 1, "one cond-type error expected: {:?}", errs);
        let e = &errs[0];
        assert!(
            e.context.contains("`{#if}` condition"),
            "context = {:?}",
            e.context
        );
        assert!(
            e.message.contains("must evaluate to Bool") && e.message.contains("Str"),
            "message = {:?}",
            e.message
        );
    }

    #[test]
    fn if_cond_undefined_ident_reports_error() {
        // `{#if missing}` — no such state field / var. Classic
        // checker's "unknown variable" propagates with the cond
        // context label.
        let src = r#"component X {
  state { flag: Bool = false }
  <template>{#if missing}<span/>{/if}</template>
}"#;
        let errs = check_str(src);
        assert!(!errs.is_empty(), "undefined ident must error");
        assert!(
            errs.iter()
                .any(|e| e.context.contains("`{#if}` condition") && e.message.contains("missing")),
            "errors = {:#?}",
            errs
        );
    }

    #[test]
    fn if_cond_interpolation_inside_body_is_checked() {
        // `{title}` inside `{#if}` body — the interpolation walker
        // must recurse into If children. Broken interpolation surfaces
        // as an interpolation error, not silently.
        let src = r#"component X {
  state { flag: Bool = false }
  <template>{#if flag}<div>{missing_var}</div>{/if}</template>
}"#;
        let errs = check_str(src);
        assert!(
            errs.iter()
                .any(|e| e.context.contains("template interpolation")
                    && e.message.contains("missing_var")),
            "interpolation inside If must be checked: {:#?}",
            errs
        );
    }

    #[test]
    fn if_cond_event_attr_inside_body_is_checked() {
        // `@click="undeclared"` inside `{#if}` body — the
        // event-attr walker must recurse into If children.
        let src = r#"component X {
  state { flag: Bool = false }
  event save() { flag = true }
  <template>{#if flag}<button @click="undeclared">X</button>{/if}</template>
}"#;
        let errs = check_str(src);
        assert!(
            errs.iter()
                .any(|e| e.context.contains("template event attr '@click'")
                    && e.message.contains("undeclared")),
            "event attr inside If must be checked: {:#?}",
            errs
        );
    }

    #[test]
    fn if_cond_nullable_bool_is_accepted() {
        // `Bool?` cond is accepted (see `is_bool_compatible` doc).
        // Direct construction — the view lexer tokenizes `Bool?` at
        // source level since view-lexer §7 but for defaults `false`
        // is easier to write than a full Bool? source-level path.
        let file = ExpandedViewFile {
            imports: Vec::new(),
            components: vec![ExpandedComponent {
                name: "X".into(),
                loc: Loc::new(1, 1),
                state: vec![synth_state_field(
                    "maybe",
                    TypeExpr::Nullable(Box::new(TypeExpr::Named("Bool".into()))),
                    Expr::Null(Span::ZERO),
                )],
                events: Vec::new(),
                template: Some(crate::view::expand::ExpandedTemplate {
                    roots: vec![crate::view::expand::ExpandedTemplateNode::If {
                        cond: Expr::Ident("maybe".into(), Span::new(1, 1)),
                        children: Vec::new(),
                        else_children: None,
                        loc: Loc::new(2, 1),
                    }],
                    loc: Loc::new(2, 1),
                }),
                style: None,
            }],
        };
        assert!(check(&file).is_empty(), "Bool? is accepted as cond");
    }

    #[test]
    fn if_cond_int_type_is_not_bool_compatible() {
        // Int cond is NOT accepted (no JS-style truthiness). User
        // must write `count > 0` explicitly.
        let src = r#"component X {
  state { count: Int = 0 }
  <template>{#if count}<span/>{/if}</template>
}"#;
        let errs = check_str(src);
        assert!(!errs.is_empty(), "Int cond must error");
        assert!(
            errs.iter()
                .any(|e| e.message.contains("Int") && e.message.contains("must evaluate to Bool")),
            "errors = {:#?}",
            errs
        );
    }

    #[test]
    fn if_cond_nested_if_conds_each_checked() {
        // Two nested `{#if}`. The outer cond is Bool (ok); the inner
        // cond is Str (bad). Only the inner errors.
        let src = r#"component X {
  state {
    flag: Bool = false
    title: Str = ""
  }
  <template>{#if flag}{#if title}<span/>{/if}{/if}</template>
}"#;
        let errs = check_str(src);
        assert_eq!(errs.len(), 1, "one cond error expected: {:?}", errs);
        assert!(
            errs[0].context.contains("`{#if}` condition") && errs[0].message.contains("Str"),
            "errors = {:#?}",
            errs
        );
    }

    // ---- 11.2.c mini-commit 1: is_bool_compatible unit tests -------

    #[test]
    fn is_bool_compatible_accepts_bool_and_gradual_and_nullable_bool() {
        assert!(is_bool_compatible(&Type::Bool));
        assert!(is_bool_compatible(&Type::Any));
        assert!(is_bool_compatible(&Type::PyAny));
        assert!(is_bool_compatible(&Type::Nullable(Box::new(Type::Bool))));
        assert!(is_bool_compatible(&Type::Nullable(Box::new(Type::Any))));
    }

    #[test]
    fn is_bool_compatible_rejects_other_types() {
        assert!(!is_bool_compatible(&Type::Int));
        assert!(!is_bool_compatible(&Type::Str));
        assert!(!is_bool_compatible(&Type::Null));
        assert!(!is_bool_compatible(&Type::List(Box::new(Type::Int))));
        assert!(!is_bool_compatible(&Type::Nullable(Box::new(Type::Str))));
    }

    // ---- 11.2.c mini-commit 2: `{#for x in xs}...{/for}` -----------

    #[test]
    fn for_block_iter_list_of_str_binds_x_str_friendly() {
        // `xs: List<Str>` — inside the for, `x` is Str, which is
        // Str-friendly. Cero errores.
        let src = r#"component X {
  state {
    xs: List<Str> = []
  }
  <template>{#for x in xs}<li>{x}</li>{/for}</template>
}"#;
        assert!(check_str(src).is_empty(), "no errors expected");
    }

    #[test]
    fn for_block_iter_list_of_int_binds_x_int_str_friendly() {
        // Same shape with `List<Int>`; `{x}` renders Int via auto-
        // Display. Cero errores.
        let src = r#"component X {
  state {
    nums: List<Int> = []
  }
  <template>{#for n in nums}<li>{n}</li>{/for}</template>
}"#;
        assert!(check_str(src).is_empty(), "no errors expected");
    }

    #[test]
    fn for_block_iter_range_binds_x_int() {
        // `for x in 0..10` — the classic checker types x as Int for
        // ranges. `{x}` renders Int. Cero errores.
        let src = r#"component X {
  <template>{#for i in 0..10}<li>{i}</li>{/for}</template>
}"#;
        assert!(check_str(src).is_empty(), "no errors expected");
    }

    #[test]
    fn for_block_iter_non_iterable_reports_error() {
        // `title: Str` is not iterable. Classic For checker emits
        // "the `for` iterable must be List, Range or Map, received
        // `Str`". Check we shift the error to the `{#for}` block's
        // loc and label its context clearly.
        let src = r#"component X {
  state {
    title: Str = ""
  }
  <template>{#for c in title}<li/>{/for}</template>
}"#;
        let errs = check_str(src);
        assert!(!errs.is_empty(), "at least one iter error expected");
        // Look for the iter-context error specifically.
        let iter_err = errs
            .iter()
            .find(|e| e.context.contains("`{#for") && e.context.contains("iter"))
            .unwrap_or_else(|| panic!("no iter context error found: {:#?}", errs));
        assert!(
            iter_err.message.contains("List") && iter_err.message.contains("Str"),
            "iter error should mention iter type: {:?}",
            iter_err.message
        );
    }

    #[test]
    fn for_block_var_visible_inside_body_interp() {
        // Same shape as the basic test but using `x` in an attr
        // value interp: `<li class="{x}">…`. `x` must be visible
        // to attr interps too (they route through the same
        // collector).
        let src = r#"component X {
  state {
    tags: List<Str> = []
  }
  <template>{#for tag in tags}<span class="{tag}">hi</span>{/for}</template>
}"#;
        assert!(
            check_str(src).is_empty(),
            "no errors expected: {:#?}",
            check_str(src)
        );
    }

    #[test]
    fn for_block_var_not_visible_outside_body() {
        // `{x}` AFTER `{/for}` — the binding is out of scope. Classic
        // checker emits "variable `x` no definida" (or similar). We
        // shift it to the interp's loc.
        let src = r#"component X {
  state {
    xs: List<Str> = []
  }
  <template>{#for x in xs}<li/>{/for}<span>{x}</span></template>
}"#;
        let errs = check_str(src);
        assert!(
            errs.iter()
                .any(|e| e.context.contains("template interpolation")
                    && (e.message.contains("no definida")
                        || e.message.contains("not defined")
                        || e.message.contains("unknown variable")
                        || e.message.contains("undeclared"))),
            "expected 'unknown variable' error for out-of-scope x: {:#?}",
            errs
        );
    }

    #[test]
    fn for_block_if_cond_sees_for_binding() {
        // `{#for x in nums}{#if x > 0}<span>{x}</span>{/if}{/for}` —
        // the cond `x > 0` references x from the enclosing for.
        // Cero errores (x tipa Int, x > 0 tipa Bool).
        let src = r#"component X {
  state {
    nums: List<Int> = []
  }
  <template>{#for x in nums}{#if x > 0}<span>{x}</span>{/if}{/for}</template>
}"#;
        let errs = check_str(src);
        assert!(errs.is_empty(), "no errors expected: {:#?}", errs);
    }

    #[test]
    fn for_block_nested_for_inner_iter_sees_outer_var() {
        // The inner for's iter `p.title` refs the outer binding `p`
        // — wait that's wrong. Should be `u.posts` where `u` is
        // outer. Let me redo: `{#for u in users}{#for tag in u.tags}
        // <li>{tag}</li>{/for}{/for}`. The inner iter `u.tags`
        // references the outer binding `u`.
        //
        // Requires a nominal with a `tags: List<Str>` field for `u`.
        // The source declares `type User { tags: List<Str> = [] }`
        // and state has `users: List<User> = []`.
        let src = r#"component X {
  state {
    users: List<User> = []
  }
  <template>{#for u in users}{#for tag in u.tags}<li>{tag}</li>{/for}{/for}</template>
}"#;
        // The type `User` is not declared in the component itself —
        // components today don't have inline `type` decls, and the
        // classic checker rejects with `type "User" not defined`. We
        // expect ONE such error citing the state field. This test
        // documents that behavior + proves the nested walk doesn't
        // add cascading errors.
        let errs = check_str(src);
        // Restrict expectation: state field error should surface;
        // handler/interp checks are cascade-avoided so no downstream
        // noise from `u.tags`.
        assert!(
            errs.iter()
                .any(|e| e.context.contains("state field") && e.message.contains("User")),
            "expected state field type error for undeclared `User`: {:#?}",
            errs
        );
    }

    #[test]
    fn for_block_nested_for_iter_missing_field_on_nominal_reports_error() {
        // Use a built-in like `Range` behavior via a shape we CAN
        // construct without declaring a nominal: nested for over a
        // List<List<Int>>. Outer u: List<Int>; inner uses u — but
        // `u` is List<Int>, and iterating over it should work. Any
        // undefined `.some_method()` on List<Int> pulls a clear
        // error via the classic method resolver.
        //
        // Actually simpler: outer `x in xs` with xs: List<Int>;
        // inner `y in x.no_such_method(...)`. Classic checker
        // will report Int has no `.no_such_method`.
        let src = r#"component X {
  state {
    xs: List<Int> = []
  }
  <template>{#for x in xs}{#for y in x.no_such_method()}<li/>{/for}{/for}</template>
}"#;
        let errs = check_str(src);
        assert!(
            errs.iter()
                .any(|e| e.context.contains("`{#for") && e.context.contains("iter")),
            "expected iter-context error for bad method on Int: {:#?}",
            errs
        );
    }

    #[test]
    fn for_block_map_iter_binds_tuple_str_friendly() {
        // `for (k, v) in m` in classic is Tuple[K, V]. In the
        // template shape `{#for kv in m}` we bind a single ident to
        // that Tuple. Tuple is Str-friendly (via auto-Display), so
        // `{kv}` doesn't error out.
        let src = r#"component X {
  state {
    m: Map<Str, Int> = {}
  }
  <template>{#for kv in m}<li>{kv}</li>{/for}</template>
}"#;
        assert!(
            check_str(src).is_empty(),
            "Map iter binding to Tuple should not error: {:#?}",
            check_str(src)
        );
    }

    #[test]
    fn for_block_iter_with_method_chain_typechecks() {
        // `xs.map(fn(x) => x * 2)` — the iter is a method chain
        // producing a `List<Int>` from a `List<Int>`. Bindings type
        // as Int. Cero errores.
        let src = r#"component X {
  state {
    xs: List<Int> = []
  }
  <template>{#for y in xs.map(fn(x) => x * 2)}<li>{y}</li>{/for}</template>
}"#;
        assert!(
            check_str(src).is_empty(),
            "no errors expected: {:#?}",
            check_str(src)
        );
    }

    // ---- 11.2.c mini-commit 3: `{#else}` + `<slot />` checker tests --

    #[test]
    fn if_else_interpolation_inside_else_branch_is_checked() {
        // Interpolation `{missing}` inside the else branch must be
        // reported — proves `collect_interpolations` walks into
        // `else_children` and the classic checker sees it.
        let src = r#"component X {
  state {
    ready: Bool = false
    label: Str = "hi"
  }
  <template>{#if ready}<span>{label}</span>{#else}<span>{missing}</span>{/if}</template>
}"#;
        let errs = check_str(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unknown variable") && e.message.contains("missing")),
            "expected `unknown variable missing` error inside else branch: {:#?}",
            errs
        );
    }

    #[test]
    fn if_else_event_attr_inside_else_branch_is_checked() {
        // A bad `@click` inside the else branch must be reported —
        // proves `collect_event_attrs` walks into `else_children`.
        let src = r#"component X {
  event go() {}
  <template>{#if true}<button @click="go">yes</button>{#else}<button @click="does_not_exist">no</button>{/if}</template>
}"#;
        let errs = check_str(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("does_not_exist")
                    && e.message.contains("no such `event`")),
            "expected event-attr error for `does_not_exist` in else branch: {:#?}",
            errs
        );
    }

    #[test]
    fn if_else_nested_if_cond_inside_else_is_checked() {
        // A nested `{#if}` inside the else branch — its cond must
        // be reported when non-Bool (proves `collect_if_conds`
        // walks into `else_children`).
        let src = r#"component X {
  state {
    a: Bool = false
    n: Int = 3
  }
  <template>{#if a}<div/>{#else}{#if n}<div/>{/if}{/if}</template>
}"#;
        let errs = check_str(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("`{#if}`") && e.message.contains("Bool")),
            "expected `{{#if}}` cond error for Int-typed cond inside else branch: {:#?}",
            errs
        );
    }

    #[test]
    fn if_else_for_iter_inside_else_branch_is_checked() {
        // A bad iter `{#for x in <Str>}` inside the else branch —
        // must be reported (proves `collect_for_iters` walks into
        // `else_children`).
        let src = r#"component X {
  state {
    ready: Bool = false
    label: Str = "hi"
  }
  <template>{#if ready}<div/>{#else}{#for x in label}<span/>{/for}{/if}</template>
}"#;
        let errs = check_str(src);
        assert!(
            errs.iter()
                .any(|e| e.context.contains("`{#for") && e.context.contains("iter")),
            "expected for-iter error for iterating Str in else branch: {:#?}",
            errs
        );
    }

    #[test]
    fn if_without_else_still_checks_the_then_branch() {
        // Regression: plain `{#if}...{/if}` (no else) still checks
        // interpolations in the then branch.
        let src = r#"component X {
  state { ready: Bool = false }
  <template>{#if ready}<span>{missing}</span>{/if}</template>
}"#;
        let errs = check_str(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unknown variable") && e.message.contains("missing")),
            "expected `unknown variable missing` error in then branch: {:#?}",
            errs
        );
    }

    #[test]
    fn slot_is_ignored_by_collectors_no_spurious_errors() {
        // A component with a `<slot />` and NO other errors must
        // return zero errors — the slot is an opaque marker, no
        // expressions to check, no cross-refs.
        let src = r#"component X {
  <template><slot /></template>
}"#;
        assert!(
            check_str(src).is_empty(),
            "expected no errors for a bare <slot />: {:#?}",
            check_str(src)
        );
    }

    #[test]
    fn slot_with_name_ignored_by_collectors_no_spurious_errors() {
        // Same with a named slot.
        let src = r#"component X {
  <template><slot name="header" /></template>
}"#;
        assert!(
            check_str(src).is_empty(),
            "expected no errors for <slot name=\"header\" />: {:#?}",
            check_str(src)
        );
    }

    #[test]
    fn slot_alongside_other_template_content_does_not_break_walks() {
        // Slot mixed with interpolations, events, and an if/else —
        // the slot is a leaf in every collector but the other
        // content still gets checked. This catches accidental
        // early-return in a collector arm.
        let src = r#"component X {
  state {
    title: Str = "hi"
    ready: Bool = false
  }
  event go() {}
  <template>
    <div>
      <slot name="header" />
      {title}
      <button @click="go">go</button>
      {#if ready}<span>on</span>{#else}<slot />{/if}
    </div>
  </template>
}"#;
        assert!(
            check_str(src).is_empty(),
            "expected no errors for a well-formed component with slots: {:#?}",
            check_str(src)
        );
    }

    // ---- Phase 11.5.d — child-component composition validation ----

    #[test]
    fn phase_11_5_d_check_valid_composition_ok() {
        let src = r#"component Parent {
  state {}
  <template><Card title="Hi" count="3" /></template>
}
component Card {
  state {
    title: Str = "x"
    count: Int = 0
  }
  <template><div>{title}</div></template>
}"#;
        let errs = check_str(src);
        assert!(errs.is_empty(), "expected no errors, got: {errs:#?}");
    }

    #[test]
    fn phase_11_5_d_check_unknown_component_errors_with_typo_hint() {
        let src = r#"component Parent {
  state {}
  <template><Carr /></template>
}
component Card {
  state { title: Str = "x" }
  <template><div>{title}</div></template>
}"#;
        let errs = check_str(src);
        assert_eq!(errs.len(), 1);
        let msg = &errs[0].message;
        assert!(msg.contains("unknown component"), "msg: {msg}");
        assert!(msg.contains("Card"), "msg: {msg}");
        assert!(
            msg.contains("did you mean"),
            "expected typo hint in msg: {msg}"
        );
    }

    // ---- Phase 11.7 — cross-file `<Child />` composition -----------

    /// Check a parent `.fitzv` source with the components of an imported
    /// child `.fitzv` supplied as cross-file surfaces. Mirrors what the
    /// WASM CLI does: load imported components, then
    /// `check_with_imported_components`.
    fn check_str_with_imported(parent_src: &str, child_srcs: &[&str]) -> Vec<CheckError> {
        let raw = view_parse(parent_src).expect("parent view parses");
        let parent = expand(&raw).expect("parent expands cleanly");
        let mut imported: Vec<ExpandedComponent> = Vec::new();
        for child in child_srcs {
            let raw = view_parse(child).expect("child view parses");
            let ex = expand(&raw).expect("child expands cleanly");
            imported.extend(ex.components);
        }
        check_with_imported_components(&parent, &imported)
    }

    #[test]
    fn cross_file_child_composition_accepts_imported_component() {
        // `<Card />` is declared in a SEPARATE file; supplied as an
        // imported surface it composes cleanly — no "unknown component".
        let parent = r#"from Card import Card
component App {
  state {}
  <template><div><Card title="hi" /></div></template>
}"#;
        let child = r#"component Card {
  state { title: Str = "" }
  <template><article>{title}</article></template>
}"#;
        let errs = check_str_with_imported(parent, &[child]);
        assert!(errs.is_empty(), "expected no errors, got: {errs:#?}");
    }

    #[test]
    fn cross_file_unknown_imported_component_still_errors() {
        // No imported surfaces supplied → `<Card />` is unknown.
        let parent = r#"from Card import Card
component App {
  state {}
  <template><div><Card title="hi" /></div></template>
}"#;
        let errs = check_str_with_imported(parent, &[]);
        assert!(
            errs.iter().any(|e| e.message.contains("unknown component")),
            "an unresolved cross-file child must error: {errs:#?}"
        );
    }

    #[test]
    fn cross_file_child_prop_typo_validated_against_imported_surface() {
        // The prop name is validated against the IMPORTED child's real
        // state fields — a typo is caught cross-file.
        let parent = r#"from Card import Card
component App {
  state {}
  <template><div><Card titel="hi" /></div></template>
}"#;
        let child = r#"component Card {
  state { title: Str = "" }
  <template><article>{title}</article></template>
}"#;
        let errs = check_str_with_imported(parent, &[child]);
        assert!(
            !errs.is_empty(),
            "an unknown prop on a cross-file child must error: {errs:#?}"
        );
    }

    #[test]
    fn cross_file_child_event_binding_validated_against_imported_surface() {
        // Binding an event the imported child doesn't declare errors.
        let parent = r#"from Card import Card
component App {
  state { n: Int = 0 }
  event bump() { n = n + 1 }
  <template><div><Card @nope="bump" /></div></template>
}"#;
        let child = r#"component Card {
  state {}
  event like() {}
  <template><button @click="like">x</button></template>
}"#;
        let errs = check_str_with_imported(parent, &[child]);
        assert!(
            errs.iter().any(|e| e.message.contains("does not declare")),
            "binding an event the imported child lacks must error: {errs:#?}"
        );
    }

    #[test]
    fn cross_file_local_component_shadows_imported_of_same_name() {
        // A local `Card` wins over an imported `Card`: composition
        // validates against the LOCAL surface (which has `local`, not
        // `title`), so `title="..."` is an unknown prop.
        let parent = r#"component App {
  state {}
  <template><div><Card title="hi" /></div></template>
}
component Card {
  state { local: Str = "" }
  <template><span>{local}</span></template>
}"#;
        let imported = r#"component Card {
  state { title: Str = "" }
  <template><article>{title}</article></template>
}"#;
        let errs = check_str_with_imported(parent, &[imported]);
        assert!(
            !errs.is_empty(),
            "local Card wins → `title` is unknown on it: {errs:#?}"
        );
    }

    #[test]
    fn phase_11_7_c_check_child_event_binding_ok() {
        let src = r#"component App {
  state { hits: Int = 0 }
  event on_hit() { hits = hits + 1 }
  <template><div><Kid @ping="on_hit" /></div></template>
}
component Kid {
  state { n: Int = 0 }
  event ping() {}
  <template><button @click="ping">{n}</button></template>
}"#;
        let errs = check_str(src);
        assert!(errs.is_empty(), "expected no errors, got: {errs:#?}");
    }

    #[test]
    fn phase_11_7_c_check_unknown_child_event_errors() {
        let src = r#"component App {
  state { hits: Int = 0 }
  event on_hit() { hits = hits + 1 }
  <template><div><Kid @nope="on_hit" /></div></template>
}
component Kid {
  state { n: Int = 0 }
  event ping() {}
  <template><button @click="ping">{n}</button></template>
}"#;
        let errs = check_str(src);
        assert!(
            errs.iter().any(|e| e.message.contains("does not declare")),
            "binding an event the child doesn't declare must error: {errs:#?}"
        );
    }

    #[test]
    fn phase_11_7_c_check_unknown_parent_handler_errors() {
        let src = r#"component App {
  state { hits: Int = 0 }
  event on_hit() { hits = hits + 1 }
  <template><div><Kid @ping="not_a_handler" /></div></template>
}
component Kid {
  state { n: Int = 0 }
  event ping() {}
  <template><button @click="ping">{n}</button></template>
}"#;
        let errs = check_str(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("not an `event` of the parent")),
            "binding to a non-existent parent handler must error: {errs:#?}"
        );
    }

    #[test]
    fn phase_11_7_d_check_slot_content_ok_when_child_has_slot() {
        let src = r#"component App {
  state {}
  <template><Panel><span>hi</span></Panel></template>
}
component Panel {
  state {}
  <template><section><slot /></section></template>
}"#;
        let errs = check_str(src);
        assert!(errs.is_empty(), "expected no errors, got: {errs:#?}");
    }

    #[test]
    fn phase_11_7_d_check_slot_content_without_child_slot_errors() {
        let src = r#"component App {
  state {}
  <template><Panel><span>hi</span></Panel></template>
}
component Panel {
  state {}
  <template><section>no slot here</section></template>
}"#;
        let errs = check_str(src);
        assert!(
            errs.iter().any(|e| e.message.contains("has no `<slot />`")),
            "slot content with no child <slot /> must error: {errs:#?}"
        );
    }

    #[test]
    fn phase_11_5_d_check_unknown_component_without_hint_lists_available() {
        // `Zzzzzz` is far enough from `Card` that no typo suggestion
        // fires (Levenshtein > 3). The message should list all
        // available names.
        let src = r#"component Parent {
  state {}
  <template><Zzzzzz /></template>
}
component Card {
  state { title: Str = "x" }
  <template><div>{title}</div></template>
}"#;
        let errs = check_str(src);
        assert_eq!(errs.len(), 1);
        let msg = &errs[0].message;
        assert!(msg.contains("Zzzzzz"), "msg: {msg}");
        assert!(
            msg.contains("Available"),
            "expected 'Available:' listing: {msg}"
        );
        assert!(msg.contains("Card"), "msg: {msg}");
        assert!(msg.contains("Parent"), "msg: {msg}");
    }

    #[test]
    fn phase_11_5_d_check_self_reference_rejects_with_dedicated_message() {
        let src = r#"component Loop {
  state {}
  <template><Loop /></template>
}"#;
        let errs = check_str(src);
        assert_eq!(errs.len(), 1);
        let msg = &errs[0].message;
        assert!(msg.contains("cannot mount itself"), "msg: {msg}");
    }

    #[test]
    fn phase_11_5_d_check_unknown_prop_errors_with_typo_hint() {
        let src = r#"component Parent {
  state {}
  <template><Card titel="Hi" /></template>
}
component Card {
  state {
    title: Str = "x"
    count: Int = 0
  }
  <template><div>{title}</div></template>
}"#;
        let errs = check_str(src);
        assert_eq!(errs.len(), 1);
        let msg = &errs[0].message;
        assert!(msg.contains("unknown prop"), "msg: {msg}");
        assert!(msg.contains("titel"), "msg: {msg}");
        assert!(msg.contains("did you mean"), "typo hint expected: {msg}");
        assert!(
            msg.contains("title"),
            "typo hint should include title: {msg}"
        );
    }

    #[test]
    fn phase_11_5_d_check_duplicate_prop_rejects() {
        let src = r#"component Parent {
  state {}
  <template><Card title="A" title="B" /></template>
}
component Card {
  state { title: Str = "x" }
  <template><div>{title}</div></template>
}"#;
        let errs = check_str(src);
        assert!(!errs.is_empty(), "expected an error");
        assert!(
            errs.iter().any(|e| e.message.contains("duplicate prop")),
            "errors: {errs:#?}"
        );
    }

    #[test]
    fn phase_11_5_d_check_int_prop_coerces_valid_integer() {
        let src = r#"component Parent {
  state {}
  <template><Card count="42" /></template>
}
component Card {
  state { count: Int = 0 }
  <template><span>{count}</span></template>
}"#;
        let errs = check_str(src);
        assert!(errs.is_empty(), "expected no errors, got: {errs:#?}");
    }

    #[test]
    fn phase_11_5_d_check_int_prop_rejects_non_numeric_value() {
        let src = r#"component Parent {
  state {}
  <template><Card count="abc" /></template>
}
component Card {
  state { count: Int = 0 }
  <template><span>{count}</span></template>
}"#;
        let errs = check_str(src);
        assert!(!errs.is_empty(), "expected an error");
        let msg = &errs[0].message;
        assert!(msg.contains("expected an integer literal"), "msg: {msg}");
    }

    #[test]
    fn phase_11_5_d_check_bool_prop_accepts_true_false_rejects_others() {
        let ok_src = r#"component Parent {
  state {}
  <template><Card active="true" /></template>
}
component Card {
  state { active: Bool = false }
  <template><span>{active}</span></template>
}"#;
        assert!(check_str(ok_src).is_empty());
        let bad_src = r#"component Parent {
  state {}
  <template><Card active="yes" /></template>
}
component Card {
  state { active: Bool = false }
  <template><span>{active}</span></template>
}"#;
        let errs = check_str(bad_src);
        assert!(!errs.is_empty());
        assert!(
            errs[0].message.contains("`\"true\"` or `\"false\"`"),
            "msg: {}",
            errs[0].message
        );
    }

    #[test]
    fn phase_11_5_d_check_nullable_int_accepts_null_and_integer() {
        let src = r#"component Parent {
  state {}
  <template><Card n="null" /></template>
}
component Card {
  state { n: Int? = null }
  <template><span>hi</span></template>
}"#;
        assert!(check_str(src).is_empty(), "null case failed");

        let src2 = r#"component Parent {
  state {}
  <template><Card n="7" /></template>
}
component Card {
  state { n: Int? = null }
  <template><span>hi</span></template>
}"#;
        assert!(check_str(src2).is_empty(), "int case failed");
    }

    #[test]
    fn k3_check_list_int_prop_end_to_end_accepts_comma_separated_values() {
        // Was `phase_11_5_d_check_list_prop_rejects_citing_11_6`
        // pre-K-3. K-3 lifted the block — `<Card items="1,2,3" />`
        // now coerces against `List<Int>` cleanly through the full
        // view parser + expander + checker pipeline. This is the
        // end-to-end complement to the direct-helper unit tests
        // above (which build TypeExpr by hand and never touch the
        // view parser).
        let src = r#"component Parent {
  state {}
  <template><Card items="1,2,3" /></template>
}
component Card {
  state { items: List<Int> = [] }
  <template><span>hi</span></template>
}"#;
        let errs = check_str(src);
        assert!(errs.is_empty(), "expected zero errors; got: {:?}", errs);
    }

    #[test]
    fn s2_check_map_str_str_prop_end_to_end_accepts_k_equals_v() {
        // Was `k3_check_map_prop_still_rejects_citing_11_6_or_later`
        // pre-S.2. S.2 (2026-07-17) lifted the Map<Str,Str> block —
        // `<Card meta="k=v" />` with `meta: Map<Str, Str>` now
        // coerces via the k=v,k=v convention.
        let src = r#"component Parent {
  state {}
  <template><Card meta="role=admin,scope=full" /></template>
}
component Card {
  state { meta: Map<Str, Str> = {} }
  <template><span>hi</span></template>
}"#;
        let errs = check_str(src);
        assert!(
            errs.is_empty(),
            "expected zero errors post-S.2; got: {:?}",
            errs
        );
    }

    #[test]
    fn s2_check_map_str_int_prop_still_rejects_end_to_end() {
        // Map<Str, Int> still deferred (raw HTML attr can't
        // disambiguate). Users should interpolate instead.
        let src = r#"component Parent {
  state {}
  <template><Card scores="a=1" /></template>
}
component Card {
  state { scores: Map<Str, Int> = {} }
  <template><span>hi</span></template>
}"#;
        let errs = check_str(src);
        assert!(!errs.is_empty(), "expected rejection for Map<Str, Int>");
        assert!(
            errs[0].message.contains("Map<Str, Int>"),
            "msg: {}",
            errs[0].message
        );
    }

    // ---------------------------------------------------------------------
    // S.3 — Light type-check for interpolated props (bare Ident vs parent
    //        state field)
    // ---------------------------------------------------------------------

    #[test]
    fn s3_check_interpolated_ident_matching_parent_state_type_is_ok() {
        // Bare Ident refers to a parent state field whose type
        // matches the child's declared field — should type-check
        // clean.
        let src = r#"component Parent {
  state { title: Str = "hi" }
  <template><Card label="{title}" /></template>
}
component Card {
  state { label: Str = "" }
  <template><span>{label}</span></template>
}"#;
        let errs = check_str(src);
        assert!(errs.is_empty(), "expected no errors; got: {:?}", errs);
    }

    #[test]
    fn s3_check_interpolated_ident_mismatched_parent_state_type_is_error() {
        // Bare Ident refers to a parent state field whose type does
        // NOT match the child's declared field. S.3 catches this at
        // check time (was silently accepted pre-S.3).
        let src = r#"component Parent {
  state { title: Str = "hi" }
  <template><Card num="{title}" /></template>
}
component Card {
  state { num: Int = 0 }
  <template><span>{num}</span></template>
}"#;
        let errs = check_str(src);
        assert!(
            !errs.is_empty(),
            "expected error for parent Str vs child Int mismatch"
        );
        let msg = &errs[0].message;
        assert!(msg.contains("title"), "msg must cite the ident: {msg}");
        assert!(msg.contains("Str"), "msg must cite parent type Str: {msg}");
        assert!(msg.contains("Int"), "msg must cite child type Int: {msg}");
    }

    #[test]
    fn s3_check_interpolated_ident_str_promotes_to_nullable_str_ok() {
        // Str is compatible with Str? (assignment promotes to
        // Some(value)). S.3's `type_expr_compatible` handles this.
        let src = r#"component Parent {
  state { title: Str = "hi" }
  <template><Card label="{title}" /></template>
}
component Card {
  state { label: Str? = null }
  <template><span>hi</span></template>
}"#;
        let errs = check_str(src);
        assert!(
            errs.is_empty(),
            "Str → Str? should be compatible; got: {:?}",
            errs
        );
    }

    #[test]
    fn s3_check_interpolated_non_ident_expr_still_skips_type_check() {
        // BinOp `{n + 1}` and other richer exprs skip the light
        // check — the classic checker running on the emitted module
        // catches deeper mismatches at emit time. Regression: don't
        // false-positive on shapes we can't reason about.
        let src = r#"component Parent {
  state { n: Int = 0 }
  <template><Card label="{n + 1}" /></template>
}
component Card {
  state { label: Str = "" }
  <template><span>hi</span></template>
}"#;
        let errs = check_str(src);
        // We don't check BinOp result vs field type here — so no
        // error even though `n + 1: Int` clashes with `label: Str`.
        // The emitted classic module surfaces it at classic-check
        // time. Regression test: S.3 must NOT trip on non-Ident.
        assert!(
            errs.is_empty(),
            "S.3 should skip non-Ident exprs; got: {:?}",
            errs
        );
    }

    #[test]
    fn s3_check_interpolated_unknown_ident_skips_type_check() {
        // Ident that doesn't match any parent state field skips
        // silently — could be an imported name (K-4) or a closure
        // param (`{#for x in xs}`). S.3 only checks the known-safe
        // case; unknown idents surface as errors on the K-4 path
        // instead (or from the classic checker downstream).
        let src = r#"from helpers import someHelper

component Parent {
  state { count: Int = 0 }
  <template><Card label="{someHelper}" /></template>
}
component Card {
  state { label: Str = "" }
  <template><span>hi</span></template>
}"#;
        let errs = check_str(src);
        // K-4's `imported_names` puts `someHelper` in scope for the
        // template; S.3 has no way to know its type, so it skips.
        // Emitted-module classic check may or may not catch the
        // mismatch depending on the helper's signature.
        assert!(
            errs.is_empty(),
            "S.3 should skip unknown idents; got: {:?}",
            errs
        );
    }

    #[test]
    fn phase_11_5_d_check_nominal_type_prop_rejects_with_hint() {
        let src = r#"component Parent {
  state {}
  <template><Card user="unused" /></template>
}
component Card {
  state { user: User? = null }
  <template><span>hi</span></template>
}"#;
        // The Nullable<User> unwraps to Named("User"), which is a
        // nominal type — the coercer rejects with the "nominal /
        // user-defined types" message. The `null` sub-branch would
        // have coerced fine, but "unused" recurses into the inner
        // and hits the nominal rejection.
        let errs = check_str(src);
        assert!(!errs.is_empty());
        let msg = &errs[0].message;
        assert!(msg.contains("nominal"), "msg: {msg}");
    }

    #[test]
    fn phase_11_5_d_check_coerce_helper_str_wraps_and_escapes() {
        use crate::ast::TypeExpr;
        // Str with quotes/backslashes gets escaped in the emitted
        // Rust literal.
        let s = coerce_child_prop_raw_value(r#"Hello "world"\n"#, &TypeExpr::Named("Str".into()))
            .unwrap();
        // Result should be a Rust `"..."` literal + `.to_string()`
        // suffix, with quotes/backslashes escaped.
        assert!(s.starts_with('"'), "got: {s}");
        assert!(s.ends_with(".to_string()"), "got: {s}");
        assert!(s.contains(r#"\""#), "quotes must be escaped: {s}");
    }

    #[test]
    fn phase_11_5_d_check_coerce_helper_int_produces_i64_suffix() {
        use crate::ast::TypeExpr;
        assert_eq!(
            coerce_child_prop_raw_value("42", &TypeExpr::Named("Int".into())).unwrap(),
            "42i64"
        );
        assert_eq!(
            coerce_child_prop_raw_value("-7", &TypeExpr::Named("Int".into())).unwrap(),
            "-7i64"
        );
    }

    #[test]
    fn phase_11_5_d_check_coerce_helper_bool_produces_bare_literal() {
        use crate::ast::TypeExpr;
        assert_eq!(
            coerce_child_prop_raw_value("true", &TypeExpr::Named("Bool".into())).unwrap(),
            "true"
        );
        assert_eq!(
            coerce_child_prop_raw_value("false", &TypeExpr::Named("Bool".into())).unwrap(),
            "false"
        );
    }

    #[test]
    fn phase_11_5_d_check_coerce_helper_nullable_null_produces_none() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Nullable(Box::new(TypeExpr::Named("Int".into())));
        assert_eq!(coerce_child_prop_raw_value("null", &ty).unwrap(), "None");
        assert_eq!(coerce_child_prop_raw_value("5", &ty).unwrap(), "Some(5i64)");
    }

    // -----------------------------------------------------------------------
    // K-3 — List<primitive> compound props via comma-separated values
    // -----------------------------------------------------------------------
    //
    // Extends the child-prop coercion so a `<Child tags="a,b,c" />`
    // where `tags: List<Str>` produces a Rust `vec![...]` literal
    // instead of the pre-K-3 error. Empty string yields `vec![]`.
    // Whitespace around commas is trimmed. Nested primitives (Int,
    // Float, Bool, Nullable<primitive>) recurse via the same helper
    // so the SSR + WASM emitters share the exact acceptance shape.

    #[test]
    fn k3_check_coerce_helper_list_str_produces_vec_of_string_literals() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::Named("Str".into())],
        };
        let lit = coerce_child_prop_raw_value("a,b,c", &ty).unwrap();
        // vec![ "a".to_string(), "b".to_string(), "c".to_string() ]
        assert!(lit.starts_with("vec!["), "got: {lit}");
        assert!(lit.contains(r#""a""#), "got: {lit}");
        assert!(lit.contains(r#""c""#), "got: {lit}");
        assert!(lit.contains(".to_string()"), "got: {lit}");
    }

    #[test]
    fn k3_check_coerce_helper_list_int_trims_whitespace_around_commas() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::Named("Int".into())],
        };
        let lit = coerce_child_prop_raw_value(" 1 , 2 , 3 ", &ty).unwrap();
        assert_eq!(lit, "vec![1i64, 2i64, 3i64]");
    }

    #[test]
    fn k3_check_coerce_helper_list_empty_string_produces_empty_vec() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::Named("Str".into())],
        };
        assert_eq!(coerce_child_prop_raw_value("", &ty).unwrap(), "vec![]");
        assert_eq!(coerce_child_prop_raw_value("   ", &ty).unwrap(), "vec![]");
    }

    #[test]
    fn k3_check_coerce_helper_list_nullable_int_recurses() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::Nullable(Box::new(TypeExpr::Named("Int".into())))],
        };
        let lit = coerce_child_prop_raw_value("1,null,3", &ty).unwrap();
        assert_eq!(lit, "vec![Some(1i64), None, Some(3i64)]");
    }

    #[test]
    fn k3_check_coerce_helper_list_bool_bad_item_reports_position() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::Named("Bool".into())],
        };
        let err =
            coerce_child_prop_raw_value("true,yes,false", &ty).expect_err("`yes` is not a Bool");
        // Item shows up in the error breadcrumb; head is the inner
        // type name (`Bool`).
        assert!(err.contains("List<Bool>"), "got: {err}");
        assert!(err.contains("`yes`"), "got: {err}");
    }

    #[test]
    fn k3_check_coerce_helper_map_str_int_rejected_only_str_str() {
        // Post S.2 (2026-07-17): `Map<Str, Str>` static props are
        // accepted via `k=v,k=v`, but `Map<Str, Int>` still rejects
        // because the raw HTML attr can't disambiguate Int from Str
        // for the value side without per-piece parsing.
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::Named("Str".into()), TypeExpr::Named("Int".into())],
        };
        let err =
            coerce_child_prop_raw_value("k=v", &ty).expect_err("Map<Str, Int> not supported yet");
        assert!(err.contains("Map<Str, Int>"), "got: {err}");
        assert!(
            err.contains("interpolation"),
            "err must cite workaround: {err}"
        );
    }

    // ---------------------------------------------------------------------
    // S.2 — Map<Str, Str> static props via `k=v,k=v` convention
    // ---------------------------------------------------------------------

    #[test]
    fn s2_check_coerce_helper_map_str_str_produces_vec_of_pairs() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::Named("Str".into()), TypeExpr::Named("Str".into())],
        };
        let lit = coerce_child_prop_raw_value("k1=v1,k2=v2", &ty).unwrap();
        // vec![("k1".to_string(), "v1".to_string()), ("k2".to_string(), "v2".to_string())]
        assert!(lit.starts_with("vec!["), "got: {lit}");
        assert!(lit.contains("(\"k1\""), "got: {lit}");
        assert!(lit.contains("\"v1\""), "got: {lit}");
        assert!(lit.contains("(\"k2\""), "got: {lit}");
        assert!(lit.contains(".to_string()"), "got: {lit}");
    }

    #[test]
    fn s2_check_coerce_helper_map_str_str_empty_produces_empty_vec() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::Named("Str".into()), TypeExpr::Named("Str".into())],
        };
        assert_eq!(coerce_child_prop_raw_value("", &ty).unwrap(), "vec![]");
        assert_eq!(coerce_child_prop_raw_value("   ", &ty).unwrap(), "vec![]");
    }

    #[test]
    fn s2_check_coerce_helper_map_str_str_trims_whitespace_around_pairs() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::Named("Str".into()), TypeExpr::Named("Str".into())],
        };
        // Whitespace around `,` AND around `=` both trimmed.
        let lit = coerce_child_prop_raw_value(" k1 = v1 , k2 = v2 ", &ty).unwrap();
        assert!(lit.contains("(\"k1\""), "got: {lit}");
        assert!(lit.contains("\"v2\""), "got: {lit}");
    }

    #[test]
    fn s2_check_coerce_helper_map_pair_without_equals_reports_error() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::Named("Str".into()), TypeExpr::Named("Str".into())],
        };
        let err = coerce_child_prop_raw_value("k1=v1,noequals", &ty)
            .expect_err("pair without `=` should fail");
        assert!(err.contains("noequals"), "err must cite bad pair: {err}");
        assert!(
            err.contains("key=value"),
            "err must cite expected shape: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // §9.cc V-4 — `payload` in view checker event-body scope
    // -----------------------------------------------------------------------
    //
    // The SSR emitter emits event handlers with signature
    // `fn X_event(state: X, payload: Map<Str, Str>) -> X` (see §9.z).
    // Before this fix, the view checker synthesised an empty-params
    // fn (`fn X_event() { <body> }`) so any `payload.has(...)` /
    // `payload["k"]` reference in the body errored with "unknown
    // variable `payload`". The fix synthesises a top-level `let
    // payload: Map<Str, Str> = {}` (only when checking an event
    // handler body) so the classic checker's lexical scope resolves
    // `payload` naturally. These tests lock the new behaviour.

    #[test]
    fn v4_payload_has_call_in_event_body_no_longer_unknown_variable() {
        // The canonical chat-migration pattern: `payload.has("k")`
        // inside a nested if. Pre-fix this errored with "unknown
        // variable payload"; post-fix it type-checks clean.
        let src = r#"component X {
  state { text: Str = "" }

  event send() {
    if (payload.has("text")) {
      text = payload["text"]
    }
  }
}"#;
        let errors = check_str(src);
        assert!(errors.is_empty(), "expected zero errors; got: {:?}", errors);
    }

    #[test]
    fn v4_payload_index_and_nested_guards_type_check() {
        // The exact 2-level nested guard shape from the chat
        // migration probe (also validated by §9.aa event-body
        // widening on the emitter side).
        let src = r#"component ChatRoom {
  state { last_author: Str = "" }

  event send_message() {
    if (payload.has("author")) {
      if (payload.has("text")) {
        let author = payload["author"]
        let text = payload["text"]
        last_author = author
      }
    }
  }
}"#;
        let errors = check_str(src);
        assert!(
            errors.is_empty(),
            "expected zero errors on nested payload guards; got: {:?}",
            errors
        );
    }

    #[test]
    fn v4_payload_typed_as_map_str_str_lookup_returns_str() {
        // Reassigning a Str state field from `payload["k"]` (which
        // types as `Str` per the Map<Str, Str> annotation) must be
        // accepted. Pre-fix the checker would flag "unknown payload"
        // before even reaching the type flow; post-fix the type flow
        // works cleanly.
        let src = r#"component X {
  state { name: Str = "unset" }

  event rename() {
    name = payload["name"]
  }
}"#;
        let errors = check_str(src);
        assert!(
            errors.is_empty(),
            "expected zero errors when assigning a `Str` state field \
             from `payload[\"key\"]` (Map<Str, Str> lookup returns \
             Str); got: {:?}",
            errors
        );
    }

    #[test]
    fn v4_payload_not_visible_outside_event_body_context() {
        // The V-4 fix only injects `payload` when
        // `include_body_for.is_some()`. Interpolations / if-conds
        // MUST NOT see it. This locks the scoping so a rogue
        // template author writing `{payload["k"]}` gets a clean
        // "unknown variable" error (as they should — `payload` is
        // meaningful only in event bodies).
        let src = r#"component X {
  state { count: Int = 0 }

  <template>
    <p>{payload}</p>
  </template>
}"#;
        let errors = check_str(src);
        assert!(
            !errors.is_empty(),
            "expected at least one error citing `payload` in template \
             interpolation scope; got zero errors"
        );
        assert!(
            errors.iter().any(|e| e.message.contains("payload")),
            "expected error message referencing `payload`; got: {:?}",
            errors
        );
    }

    // -----------------------------------------------------------------------
    // §9.dd V-3 + V-5 — cross-file nominals via `from X import Y` in `.fitzv`
    // -----------------------------------------------------------------------

    #[test]
    fn v3_state_field_with_imported_nominal_type_no_longer_unknown() {
        // Canonical chat-migration case: `state { messages:
        // List<Message> = [] }` with `Message` declared in a sibling
        // `.fitz` module (`message.fitz`) and imported via
        // `from message import Message` at the top of the `.fitzv`.
        // Pre-fix: "unknown type Message" from the view checker.
        // Post-fix: parses + checks clean.
        let src = r#"from message import Message

component ChatRoom {
  state { messages: List<Message> = [] }
}"#;
        let errors = check_str(src);
        assert!(
            errors.is_empty(),
            "expected zero errors with imported nominal in state; got: {:?}",
            errors
        );
    }

    #[test]
    fn v5_struct_literal_of_imported_nominal_in_event_body_no_longer_unknown() {
        // Canonical chat-migration case: `messages.push(Message {
        // author: author, text: text })` inside event body. Post
        // §9.cc V-6 the bare `.push()` is accepted; post §9.dd V-3+V-5
        // the `Message { ... }` struct literal now resolves via the
        // imported nominal stub (fields: None in TypeEnv → struct lit
        // shape validation is skipped silently, matching classic Fitz
        // behaviour for cross-file imported types with no declared
        // fields in the current module).
        let src = r#"from message import Message

component ChatRoom {
  state { messages: List<Message> = [] }
  event send() {
    if (payload.has("author")) {
      if (payload.has("text")) {
        let author = payload["author"]
        let text = payload["text"]
        messages.push(Message { author: author, text: text })
      }
    }
  }
}"#;
        let errors = check_str(src);
        assert!(
            errors.is_empty(),
            "expected zero errors with imported nominal used as struct \
             literal in event body; got: {:?}",
            errors
        );
    }

    #[test]
    fn v3_multi_name_from_import_all_names_visible() {
        // Verify that multi-name `from X import Y1, Y2, Y3` shape
        // registers ALL names as nominals in scope, not just the first.
        let src = r#"from users import User, Post, Comment

component X {
  state {
    u: User = null
    p: Post = null
    c: Comment = null
  }
}"#;
        let errors = check_str(src);
        // No errors expected — but the state defaults are `null`
        // against non-Nullable types, which classic checker rejects.
        // Filter those out (not our concern) and check no "unknown type"
        // errors surface for the imported names.
        let unknown_type_errors: Vec<_> = errors
            .iter()
            .filter(|e| e.message.contains("unknown type"))
            .collect();
        assert!(
            unknown_type_errors.is_empty(),
            "expected zero 'unknown type' errors on multi-name import; got: {:?}",
            unknown_type_errors
        );
    }

    #[test]
    fn v3_dotted_path_from_import_treated_same_as_single_segment() {
        let src = r#"from utils.shared import Widget

component X {
  state { w: List<Widget> = [] }
}"#;
        let errors = check_str(src);
        assert!(
            errors.is_empty(),
            "expected zero errors with dotted-path import; got: {:?}",
            errors
        );
    }

    #[test]
    fn v3_no_imports_backward_compat_regression() {
        // Regression: `.fitzv` files without any imports must still
        // check clean (counter/dashboard/MetricTile shape). The
        // §9.dd fix only ADDS a code path; doesn't remove anything.
        let src = r#"component Counter {
  state { count: Int = 0 }
  event increment() { count = count + 1 }
}"#;
        let errors = check_str(src);
        assert!(
            errors.is_empty(),
            "regression: no-imports shape must still check clean; got: {:?}",
            errors
        );
    }

    #[test]
    fn v4_regression_events_without_payload_still_type_check() {
        // Counter-shape event body (bare state re-assign, no payload
        // reference) MUST still type-check clean. The V-4 fix only
        // ADDS `payload` to scope; it doesn't remove or modify
        // anything else.
        let src = r#"component Counter {
  state { count: Int = 0 }

  event increment() { count = count + 1 }
  event decrement() { count = count - 1 }
  event reset() { count = 0 }
}"#;
        let errors = check_str(src);
        assert!(
            errors.is_empty(),
            "counter-shape regression: expected zero errors on \
             bare state re-assign events; got: {:?}",
            errors
        );
    }
}
