// view/check.rs — Phase 11.2.b — type-check an `ExpandedViewFile`
// produced by `super::expand`.
//
// **Scope (mini-commit 2 of 11.2.b)**: state field defaults +
// event handler bodies + template interpolations.
//
// - State field defaults must be compatible with their declared
//   `TypeExpr` (this is the mini-commit 1 rule, preserved).
// - Event handler bodies are checked as `async fn`s with the
//   component's state fields visible as let-bindings in the outer
//   scope + the handler's own params inside the fn scope.
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
//   matched before rendering.
//
// Deferred to mini-commit 3:
// - `@event="handler"` attrs cross-check: the referenced handler
//   name must correspond to a declared `event ...` in the same
//   component.
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
    ExpandedAttr, ExpandedComponent, ExpandedEventHandler, ExpandedStateField, ExpandedTemplate,
    ExpandedTemplateNode, ExpandedViewFile,
};
use crate::ast::{AssignTarget, Expr, Span, Stmt};
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
    let mut errors = Vec::new();
    for component in &file.components {
        let before = errors.len();
        for field in &component.state {
            check_state_field(component.name.as_str(), field, &mut errors);
        }
        // Cascade avoidance — only check handlers + interpolations
        // when the state is clean, so consequential errors don't
        // pile up on top of the actual bug.
        if errors.len() == before {
            for handler in &component.events {
                check_event_handler(component, handler, &mut errors);
            }
            if let Some(template) = &component.template {
                check_template_interpolations(component, template, &mut errors);
            }
        }
    }
    errors
}

fn check_state_field(
    component_name: &str,
    field: &ExpandedStateField,
    errors: &mut Vec<CheckError>,
) {
    // Synthesise `<field.name>: <field.type_expr> = <field.default>`
    // as a single top-level Stmt::Assign. Every span is `Span::ZERO`
    // because the classic checker would use it to point at the source
    // — we intercept the emitted error and replace its position with
    // the state field's blob `Loc`.
    let program: Vec<Stmt> = vec![Stmt::Assign {
        target: AssignTarget::Ident(field.name.clone(), Span::ZERO),
        type_: Some(field.type_expr.clone()),
        value: field.default.clone(),
        span: Span::ZERO,
    }];
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
    errors: &mut Vec<CheckError>,
) {
    // Include ONLY the current handler's body; the other handlers
    // are pushed as fn signatures with empty bodies so handler-to-
    // handler calls resolve without accidentally surfacing their
    // body errors here (each handler's body is checked in its own
    // pass via `check_event_handler`).
    let program = build_env_program(component, Some(&handler.name), None);
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
    errors: &mut Vec<CheckError>,
) {
    let mut interpolations: Vec<InterpolationRef<'_>> = Vec::new();
    collect_interpolations(&template.roots, &mut interpolations);

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
            None,
            Some(Stmt::Assign {
                target: AssignTarget::Ident(check_var, Span::ZERO),
                type_: None,
                value: interp.expr.clone(),
                span: Span::ZERO,
            }),
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
fn build_env_program(
    component: &ExpandedComponent,
    include_body_for: Option<&str>,
    extra: Option<Stmt>,
) -> Vec<Stmt> {
    let mut program: Vec<Stmt> =
        Vec::with_capacity(component.state.len() + component.events.len() + 1);
    for field in &component.state {
        program.push(Stmt::Assign {
            target: AssignTarget::Ident(field.name.clone(), Span::ZERO),
            type_: Some(field.type_expr.clone()),
            value: field.default.clone(),
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
        program.push(s);
    }
    program
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
) {
    for node in nodes {
        match node {
            ExpandedTemplateNode::Text(_) => {}
            ExpandedTemplateNode::Interpolation { expr, loc } => {
                out.push(InterpolationRef {
                    expr,
                    loc: *loc,
                    attr_name: None,
                });
            }
            ExpandedTemplateNode::Element {
                attrs, children, ..
            } => {
                for attr in attrs {
                    if let ExpandedAttr::Interpolation { name, expr, loc } = attr {
                        out.push(InterpolationRef {
                            expr,
                            loc: *loc,
                            attr_name: Some(name.clone()),
                        });
                    }
                }
                collect_interpolations(children, out);
            }
        }
    }
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
}
