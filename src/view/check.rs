// view/check.rs — Phase 11.2.b — type-check an `ExpandedViewFile`
// produced by `super::expand`.
//
// **Scope of this commit (mini-commit 1 of 11.2.b)**: state field
// defaults only. For every `ExpandedStateField`:
// - The declared `TypeExpr` must resolve against the fresh `TypeEnv`
//   seeded by `check_program` (primitives + built-in generics).
// - The parsed default `Expr` must produce a type compatible with the
//   declared type. Compatibility rules are the same as classic Fitz:
//   `Int → Float`, `Null → T?`, gradual `Any`, etc.
//
// Deferred to subsequent mini-commits:
// - Event handler bodies (with state fields visible as let-bindings +
//   handler params as their own scope) — mini-commit 2.
// - Template `{expr}` interpolations (against state env) — mini-commit 2.
// - `@event="handler"` cross-checks (handler must name a declared event) —
//   mini-commit 3.
//
// **Strategy**: for each state field we synthesise the smallest
// classic-Fitz program that expresses the constraint we want to
// check — `field_name: T = <default>` as a `Stmt::Assign` — and run
// `crate::types::check_program`. The full type checker handles type
// resolution (nominal lookups, arity, generics), compatibility with
// coercions, and reports errors with the message the user is used to
// from the classic pipeline. We then remap each `FitzError` back to a
// `CheckError` whose `Loc` points inside the `.fitzv` file at the
// state field's blob location. Position precision inside a field is
// deferred alongside expand's own precision debt (see
// `docs/fase-11-plan.md` §7).
//
// This synth-and-check strategy is intentional: it keeps the view
// checker tiny and forces the classic checker to remain the single
// source of truth for what "compatible with T" means. When 11.2.c
// adds `{#if}` / `{#for}` control flow to the template AST, the same
// pattern will extend — wrap each blob in the smallest containing
// program, delegate, remap errors.

use super::ast::Loc;
use super::expand::{ExpandedStateField, ExpandedViewFile};
use crate::ast::{AssignTarget, Span, Stmt};
use crate::types::check_program;
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
/// we surface them all.
///
/// The order of returned errors is: components in file order; within
/// a component, state fields in declaration order.
pub fn check(file: &ExpandedViewFile) -> Vec<CheckError> {
    let mut errors = Vec::new();
    for component in &file.components {
        for field in &component.state {
            check_state_field(component.name.as_str(), field, &mut errors);
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
        // must type-check without errors: `title: Str = "Untitled"`
        // and `is_editing: Bool = false` are both valid.
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
            "Card should type-check cleanly (state defaults are the only thing checked in mini-commit 1)"
        );
    }
}
