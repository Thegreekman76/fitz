// lint.rs — Phase 9.z.5 (`fitz lint`)
//
// Linter for patterns beyond types. The static checker
// (`types.rs`) catches hard errors (type mismatch, match
// exhaustiveness, fn arity). The linter catches patterns that DO
// compile but are code smells: unused vars, dead imports,
// match with a single catch-all arm, string concatenation
// instead of interpolation.
//
// MVP decisions:
//
// - **4 lints**: `unused_variable`, `unused_import`, `useless_match`,
//   `string_concat`. The original roadmap also mentions
//   `panic_in_test_only` and `redundant_clone`; the first does NOT apply
//   (Fitz has no `panic!` builtin of its own), and the second requires
//   move analysis that the compiler does not do yet.
//
// - **Default warning**: `fitz lint` always exits 0, unless the
//   user passes `--deny <lint>` with a lint that appears in the output.
//   Cargo-clippy style.
//
// - **Suppression** with the `// @allow(<name>)` comment on the
//   IMMEDIATELY PREVIOUS line of the offending stmt. Inspection of the raw
//   source (no trivia stream from the lexer): pragmatic and sufficient.
//
// - **Auto-fix** (`--fix`) only for `string_concat` (trivial
//   transformation to interpolation). The rest requires manual editing.
//
// - **Closed catalog**: the 4 lints live in this file. Future
//   plugins are not in the MVP scope.
//
// The linter works over the AST + raw source (for
// suppression). It does NOT use the checker: lints like `unused_variable`
// do not depend on type information. This keeps the
// `check` (types) / `lint` (patterns) separation.

use crate::ast::{AssignTarget, BinOpKind, Expr, MatchArm, Param, Pattern, Program, Stmt, StrPart};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A finding produced by some lint. The runner prints them
/// cargo-clippy style:
/// ```text
/// warning: variable `x` declarada pero no usada
///   --> src/main.fitz:3:5
///   = nota: si es intencional, prefijá con `_` (ej. `_x`) o suprimí
///          con `// @allow(unused_variable)` en la línea anterior.
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct LintFinding {
    /// Canonical name of the lint (matches `// @allow(<name>)`).
    /// E.g.: `"unused_variable"`, `"string_concat"`.
    pub name: &'static str,
    /// Main message ("warning: ..." style).
    pub message: String,
    /// 1-based, parallel to `FitzError`.
    pub line: usize,
    pub column: usize,
    /// Optional hint under the main message.
    pub hint: Option<String>,
    /// If the lint has auto-fix, the replacement of the fragment over the
    /// `(start_line, start_col)..(end_line, end_col)` range. Today only
    /// `string_concat` emits it.
    pub fix: Option<LintFix>,
}

/// Literal source patch for `--fix`. Replaces the
/// `(start_line, start_col)..(end_line, end_col)` range with `replacement`.
/// 1-based positions, like the lexer's.
#[derive(Debug, Clone, PartialEq)]
pub struct LintFix {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub replacement: String,
}

/// Linter entry point. Walks `program` collecting findings
/// and applies suppressions reading the raw `source`. Returns findings
/// in stable order (line, column).
pub fn lint_source(source: &str, program: &Program) -> Vec<LintFinding> {
    let mut findings: Vec<LintFinding> = Vec::new();

    // Pre-collect: all `Expr::Ident(name)` of the program.
    // Needed by `unused_variable` and `unused_import`.
    let uses = collect_ident_uses(program);

    lint_unused_variables(program, &uses, &mut findings);
    lint_unused_imports(program, &uses, &mut findings);
    lint_useless_match(program, &mut findings);
    lint_string_concat(source, program, &mut findings);

    apply_suppressions(source, &mut findings);

    // Stable order: by line+column so the output is
    // predictable when there are multiple findings.
    findings.sort_by_key(|f| (f.line, f.column, f.name));
    findings
}

// ---------------------------------------------------------------------------
// Use collection
// ---------------------------------------------------------------------------

/// Collects all referenced names (`Expr::Ident(name)`)
/// in the program. Single set (HashSet) — we do not care about how
/// many uses or where, only whether AT LEAST one exists. Recursive walk
/// over exprs inside stmts (including nested fn bodies).
fn collect_ident_uses(program: &Program) -> std::collections::HashSet<String> {
    let mut uses = std::collections::HashSet::new();
    for stmt in program {
        collect_uses_in_stmt(stmt, &mut uses);
    }
    uses
}

fn collect_uses_in_stmt(stmt: &Stmt, uses: &mut std::collections::HashSet<String>) {
    match stmt {
        Stmt::Destructure { value, .. } => {
            // The pattern declares names (not uses). The value IS a use.
            collect_uses_in_expr(value, uses);
        }
        Stmt::Assign { target, value, .. } => {
            // The target is NOT a use (it is definition/reassignment). The
            // value IS walked (it can contain idents).
            collect_uses_in_expr(value, uses);
            // Exception: if the target is `obj.field = ...`, then `obj`
            // is a use. `AssignTarget::Field { object }` exposes it.
            if let AssignTarget::Field { object, .. } = target {
                collect_uses_in_expr(object, uses);
            }
        }
        Stmt::Return(e, _) => collect_uses_in_expr(e, uses),
        Stmt::ReturnStatus { status, body, .. } => {
            collect_uses_in_expr(status, uses);
            if let Some(b) = body {
                collect_uses_in_expr(b, uses);
            }
        }
        Stmt::Expr(e, _) => collect_uses_in_expr(e, uses),
        Stmt::FnDef {
            body, decorators, ..
        } => {
            for s in body {
                collect_uses_in_stmt(s, uses);
            }
            // Decorators: can reference fns (e.g. `@middleware(logger)`).
            for d in decorators {
                for a in &d.args {
                    collect_uses_in_expr(a, uses);
                }
                for (_, expr) in &d.kwargs {
                    collect_uses_in_expr(expr, uses);
                }
            }
        }
        Stmt::TypeDef { fields, .. } => {
            // Defaults can reference consts in scope.
            for f in fields {
                if let Some(default) = &f.default {
                    collect_uses_in_expr(default, uses);
                }
            }
        }
        Stmt::While {
            condition, body, ..
        } => {
            collect_uses_in_expr(condition, uses);
            for s in body {
                collect_uses_in_stmt(s, uses);
            }
        }
        Stmt::Loop { body, .. } => {
            for s in body {
                collect_uses_in_stmt(s, uses);
            }
        }
        Stmt::For { iter, body, .. } => {
            collect_uses_in_expr(iter, uses);
            for s in body {
                collect_uses_in_stmt(s, uses);
            }
        }
        Stmt::Break(_, _, _)
        | Stmt::Continue(_, _)
        | Stmt::Import { .. }
        | Stmt::FromImport { .. }
        | Stmt::Error(_) => {}
    }
}

fn collect_uses_in_expr(expr: &Expr, uses: &mut std::collections::HashSet<String>) {
    match expr {
        Expr::Ident(name, _) => {
            uses.insert(name.clone());
        }
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Str(..)
        | Expr::Bool(..)
        | Expr::Null(_)
        | Expr::Bytes(..)
        | Expr::Error(_) => {}
        Expr::StrInterp(parts, _) => {
            for p in parts {
                if let StrPart::Expr(e, _) = p {
                    collect_uses_in_expr(e, uses);
                }
            }
        }
        Expr::BinOp { left, right, .. } => {
            collect_uses_in_expr(left, uses);
            collect_uses_in_expr(right, uses);
        }
        Expr::UnaryOp { operand, .. } => collect_uses_in_expr(operand, uses),
        Expr::Call { callee, args, .. } => {
            collect_uses_in_expr(callee, uses);
            for a in args {
                collect_uses_in_expr(a, uses);
            }
        }
        Expr::FnExpr { body, .. } => {
            for s in body {
                collect_uses_in_stmt(s, uses);
            }
        }
        Expr::Field { object, .. } => collect_uses_in_expr(object, uses),
        Expr::Index { object, index, .. } => {
            collect_uses_in_expr(object, uses);
            collect_uses_in_expr(index, uses);
        }
        Expr::Slice {
            object, start, end, ..
        } => {
            collect_uses_in_expr(object, uses);
            if let Some(s) = start {
                collect_uses_in_expr(s, uses);
            }
            if let Some(e) = end {
                collect_uses_in_expr(e, uses);
            }
        }
        Expr::Tuple(items, _) => {
            for i in items {
                collect_uses_in_expr(i, uses);
            }
        }
        Expr::TupleField { tuple, .. } => collect_uses_in_expr(tuple, uses),
        Expr::Loop { body, .. } => {
            for s in body {
                collect_uses_in_stmt(s, uses);
            }
        }
        Expr::List(items, _) => {
            for i in items {
                collect_uses_in_expr(i, uses);
            }
        }
        // C + Cmp+ mini-batches — list comprehension. We walk the iter
        // of the first clause + extras + filter + expr. The `var`s
        // are BOUND inside, they are not uses.
        Expr::ListComp {
            expr,
            iter,
            extra_clauses,
            filter,
            ..
        } => {
            collect_uses_in_expr(iter, uses);
            for (_, it) in extra_clauses {
                collect_uses_in_expr(it, uses);
            }
            if let Some(f) = filter {
                collect_uses_in_expr(f, uses);
            }
            collect_uses_in_expr(expr, uses);
        }
        // Cmp+ mini-batch — map comprehension.
        Expr::MapComp {
            key,
            value,
            iter,
            extra_clauses,
            filter,
            ..
        } => {
            collect_uses_in_expr(iter, uses);
            for (_, it) in extra_clauses {
                collect_uses_in_expr(it, uses);
            }
            if let Some(f) = filter {
                collect_uses_in_expr(f, uses);
            }
            collect_uses_in_expr(key, uses);
            collect_uses_in_expr(value, uses);
        }
        Expr::Map(pairs, _) => {
            for (k, v) in pairs {
                collect_uses_in_expr(k, uses);
                collect_uses_in_expr(v, uses);
            }
        }
        Expr::Range { start, end, .. } => {
            collect_uses_in_expr(start, uses);
            collect_uses_in_expr(end, uses);
        }
        Expr::If {
            condition,
            then,
            else_,
            ..
        } => {
            collect_uses_in_expr(condition, uses);
            for s in then {
                collect_uses_in_stmt(s, uses);
            }
            if let Some(else_body) = else_ {
                for s in else_body {
                    collect_uses_in_stmt(s, uses);
                }
            }
        }
        Expr::Match { value, arms, .. } => {
            collect_uses_in_expr(value, uses);
            for arm in arms {
                for s in &arm.body {
                    collect_uses_in_stmt(s, uses);
                }
            }
        }
        Expr::StructLit { fields, .. } => {
            for (_, e) in fields {
                collect_uses_in_expr(e, uses);
            }
        }
        Expr::Ok(inner, _) | Expr::Err(inner, _) | Expr::Try(inner, _) | Expr::Await(inner, _) => {
            collect_uses_in_expr(inner, uses);
        }
        // Fp.3 — NamedArg passthrough to the value.
        Expr::NamedArg { value, .. } => {
            collect_uses_in_expr(value, uses);
        }
    }
}

// ---------------------------------------------------------------------------
// Lint: unused_variable
// ---------------------------------------------------------------------------

/// `unused_variable`: detects `let x = ...` (or `x = ...` initial
/// declaration) whose name NEVER appears in `Expr::Ident` of the program.
///
/// MVP caveats:
/// - **Shadowing is not detected**: `let x = 5; let x = 10; x` correctly
///   reports that `x` is used (no flag); the shadowing of the first
///   stays invisible.
/// - **Reassignment counts as a use**: NO; the target of `Stmt::Assign`
///   is not counted as a use, only the value and the Idents in other exprs.
/// - **`_` prefix**: vars starting with `_` are ignored (the
///   "intentionally unused" convention, parallel to Rust).
/// - **Fn params**: NOT flagged in MVP (many HTTP handlers /
///   callbacks receive params that they do not need to use; flagging
///   them would be noise).
fn lint_unused_variables(
    program: &Program,
    uses: &std::collections::HashSet<String>,
    findings: &mut Vec<LintFinding>,
) {
    for stmt in program {
        check_unused_var_in_stmt(stmt, uses, findings);
    }
}

fn check_unused_var_in_stmt(
    stmt: &Stmt,
    uses: &std::collections::HashSet<String>,
    findings: &mut Vec<LintFinding>,
) {
    match stmt {
        Stmt::Assign {
            target: AssignTarget::Ident(name, _),
            span,
            ..
        } if !name.starts_with('_') && !uses.contains(name) => {
            findings.push(LintFinding {
                name: "unused_variable",
                message: format!("variable `{}` declarada pero no usada", name),
                line: span.line,
                column: span.column,
                hint: Some(format!(
                    "si es intencional, prefijá con `_` (ej. `_{}`) o suprimí con \
                     `// @allow(unused_variable)` en la línea anterior.",
                    name
                )),
                fix: None,
            });
        }
        Stmt::FnDef { body, .. } => {
            for s in body {
                check_unused_var_in_stmt(s, uses, findings);
            }
        }
        Stmt::While { body, .. } | Stmt::Loop { body, .. } | Stmt::For { body, .. } => {
            for s in body {
                check_unused_var_in_stmt(s, uses, findings);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Lint: unused_import
// ---------------------------------------------------------------------------

/// `unused_import`: detects `import X` and `from X import Y` whose
/// binding is NOT referenced. For `import foo as f`, the binding is
/// `f`. For `from foo import bar as b`, the bindings are the
/// `b`s (one per entry). For `from foo import bar` the binding is `bar`.
fn lint_unused_imports(
    program: &Program,
    uses: &std::collections::HashSet<String>,
    findings: &mut Vec<LintFinding>,
) {
    for stmt in program {
        match stmt {
            Stmt::Import { path, alias, span } => {
                // The default binding is the LAST segment of the path.
                let binding = match alias {
                    Some(a) => a.clone(),
                    None => path.last().cloned().unwrap_or_default(),
                };
                if !binding.is_empty() && !uses.contains(&binding) {
                    findings.push(LintFinding {
                        name: "unused_import",
                        message: format!("import `{}` declarado pero no usado", binding),
                        line: span.line,
                        column: span.column,
                        hint: Some(
                            "si es intencional, eliminá el `import` o suprimí con \
                             `// @allow(unused_import)` en la línea anterior."
                                .into(),
                        ),
                        fix: None,
                    });
                }
            }
            Stmt::FromImport { names, span, .. } => {
                for (orig, alias) in names {
                    let binding = alias.clone().unwrap_or_else(|| orig.clone());
                    if !uses.contains(&binding) {
                        findings.push(LintFinding {
                            name: "unused_import",
                            message: format!("import `{}` declarado pero no usado", binding),
                            line: span.line,
                            column: span.column,
                            hint: Some(
                                "si es intencional, eliminá el `from import` o suprimí \
                                 con `// @allow(unused_import)` en la línea anterior."
                                    .into(),
                            ),
                            fix: None,
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Lint: useless_match
// ---------------------------------------------------------------------------

/// `useless_match`: detects `match expr { _ => body }` (or
/// `match expr { ident => body }`) with ONE single catch-all arm. The
/// match contributes nothing; the user can replace it with a direct
/// `let` (`let _ = expr; body` or `let ident = expr; body`).
///
/// Does NOT flag matches with catch-all + other arms (those are useful).
/// Does NOT flag matches with 0 arms (the parser already rejects them).
fn lint_useless_match(program: &Program, findings: &mut Vec<LintFinding>) {
    for stmt in program {
        walk_exprs_in_stmt(stmt, &mut |expr| {
            if let Expr::Match { arms, span, .. } = expr {
                if arms.len() == 1 {
                    let is_catchall =
                        matches!(arms[0].pattern, Pattern::Wildcard | Pattern::Ident(_, _));
                    if is_catchall {
                        findings.push(LintFinding {
                            name: "useless_match",
                            message: "`match` con un solo arm catch-all es equivalente a un `let`"
                                .into(),
                            line: span.line,
                            column: span.column,
                            hint: Some(
                                "reemplazá `match expr { _ => body }` con `body` directo, \
                                 o `match expr { x => body }` con `let x = expr; body`."
                                    .into(),
                            ),
                            fix: None,
                        });
                    }
                }
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Lint: string_concat
// ---------------------------------------------------------------------------

/// `string_concat`: detects `"a" + "b"` and similar (any `+`
/// where BOTH operands are `Expr::Str` literals). Suggests
/// replacing with interpolation: `"ab"` or `"{x}{y}"`.
///
/// **Auto-fix**: for the "both literals" case, emits the
/// replacement concatenating the strings and producing a single
/// literal. More complex cases (`"hola, " + name`) do NOT emit a fix
/// because they require converting to interpolation, which needs
/// parsing the name and escaping — future sub-step.
///
/// We inherit the `start` spans of the left literal and the `end`
/// of the right literal from the raw source. To do it well we need
/// `end_span`, which the AST does not have today (residual S1 debt);
/// workaround: read the source and look up the range. For the MVP of
/// 9.z.5, **no auto-fix** (deferred) — we only emit the
/// warning with a textual suggestion.
fn lint_string_concat(_source: &str, program: &Program, findings: &mut Vec<LintFinding>) {
    for stmt in program {
        walk_exprs_in_stmt(stmt, &mut |expr| {
            if let Expr::BinOp {
                op: BinOpKind::Add,
                left,
                right,
                span,
            } = expr
            {
                if matches!(left.as_ref(), Expr::Str(_, _))
                    && matches!(right.as_ref(), Expr::Str(_, _))
                {
                    findings.push(LintFinding {
                        name: "string_concat",
                        message: "concatenación de strings literales — usá interpolación".into(),
                        line: span.line,
                        column: span.column,
                        hint: Some(
                            "reemplazá `\"a\" + \"b\"` con `\"ab\"` (o usá interpolación \
                             `\"{a}{b}\"` si los lados son variables)."
                                .into(),
                        ),
                        fix: None,
                    });
                }
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Walker helpers
// ---------------------------------------------------------------------------

/// Recursive walk over the expressions of a stmt, invoking `f`
/// for each Expr visited. Useful for lints that detect expression
/// patterns regardless of the containing stmt.
fn walk_exprs_in_stmt(stmt: &Stmt, f: &mut impl FnMut(&Expr)) {
    match stmt {
        Stmt::Assign { value, .. } => walk_expr(value, f),
        Stmt::Destructure { value, .. } => walk_expr(value, f),
        Stmt::Return(e, _) => walk_expr(e, f),
        Stmt::ReturnStatus { status, body, .. } => {
            walk_expr(status, f);
            if let Some(b) = body {
                walk_expr(b, f);
            }
        }
        Stmt::Expr(e, _) => walk_expr(e, f),
        Stmt::FnDef { body, .. } => {
            for s in body {
                walk_exprs_in_stmt(s, f);
            }
        }
        Stmt::TypeDef { fields, .. } => {
            for fld in fields {
                if let Some(d) = &fld.default {
                    walk_expr(d, f);
                }
            }
        }
        Stmt::While {
            condition, body, ..
        } => {
            walk_expr(condition, f);
            for s in body {
                walk_exprs_in_stmt(s, f);
            }
        }
        Stmt::Loop { body, .. } => {
            for s in body {
                walk_exprs_in_stmt(s, f);
            }
        }
        Stmt::For { iter, body, .. } => {
            walk_expr(iter, f);
            for s in body {
                walk_exprs_in_stmt(s, f);
            }
        }
        Stmt::Break(_, _, _)
        | Stmt::Continue(_, _)
        | Stmt::Import { .. }
        | Stmt::FromImport { .. }
        | Stmt::Error(_) => {}
    }
}

fn walk_expr(expr: &Expr, f: &mut impl FnMut(&Expr)) {
    f(expr);
    match expr {
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Str(..)
        | Expr::Bool(..)
        | Expr::Null(_)
        | Expr::Bytes(..)
        | Expr::Ident(..)
        | Expr::Error(_) => {}
        Expr::StrInterp(parts, _) => {
            for p in parts {
                if let StrPart::Expr(e, _) = p {
                    walk_expr(e, f);
                }
            }
        }
        Expr::BinOp { left, right, .. } => {
            walk_expr(left, f);
            walk_expr(right, f);
        }
        Expr::UnaryOp { operand, .. } => walk_expr(operand, f),
        Expr::Call { callee, args, .. } => {
            walk_expr(callee, f);
            for a in args {
                walk_expr(a, f);
            }
        }
        Expr::FnExpr { body, .. } => {
            for s in body {
                walk_exprs_in_stmt(s, f);
            }
        }
        Expr::Field { object, .. } => walk_expr(object, f),
        Expr::Index { object, index, .. } => {
            walk_expr(object, f);
            walk_expr(index, f);
        }
        Expr::Slice {
            object, start, end, ..
        } => {
            walk_expr(object, f);
            if let Some(s) = start {
                walk_expr(s, f);
            }
            if let Some(e) = end {
                walk_expr(e, f);
            }
        }
        Expr::Tuple(items, _) => {
            for i in items {
                walk_expr(i, f);
            }
        }
        Expr::TupleField { tuple, .. } => walk_expr(tuple, f),
        Expr::Loop { body, .. } => {
            for s in body {
                walk_exprs_in_stmt(s, f);
            }
        }
        Expr::List(items, _) => {
            for i in items {
                walk_expr(i, f);
            }
        }
        // C + Cmp+ mini-batches — list comprehension. We walk the
        // sub-Exprs of each clause + filter + expr.
        Expr::ListComp {
            expr,
            iter,
            extra_clauses,
            filter,
            ..
        } => {
            walk_expr(iter, f);
            for (_, it) in extra_clauses {
                walk_expr(it, f);
            }
            if let Some(flt) = filter {
                walk_expr(flt, f);
            }
            walk_expr(expr, f);
        }
        // Cmp+ mini-batch — map comprehension.
        Expr::MapComp {
            key,
            value,
            iter,
            extra_clauses,
            filter,
            ..
        } => {
            walk_expr(iter, f);
            for (_, it) in extra_clauses {
                walk_expr(it, f);
            }
            if let Some(flt) = filter {
                walk_expr(flt, f);
            }
            walk_expr(key, f);
            walk_expr(value, f);
        }
        Expr::Map(pairs, _) => {
            for (k, v) in pairs {
                walk_expr(k, f);
                walk_expr(v, f);
            }
        }
        Expr::Range { start, end, .. } => {
            walk_expr(start, f);
            walk_expr(end, f);
        }
        Expr::If {
            condition,
            then,
            else_,
            ..
        } => {
            walk_expr(condition, f);
            for s in then {
                walk_exprs_in_stmt(s, f);
            }
            if let Some(else_body) = else_ {
                for s in else_body {
                    walk_exprs_in_stmt(s, f);
                }
            }
        }
        Expr::Match { value, arms, .. } => {
            walk_expr(value, f);
            for arm in arms {
                // Sp.2 — body is Vec<Stmt>. We iterate only the most
                // common Exprs (Stmt::Expr / Return / ReturnStatus) without
                // recursion through walk_exprs_in_stmt (it triggers
                // monomorphization recursion limit with nested
                // closures).
                for s in &arm.body {
                    match s {
                        Stmt::Expr(e, _) | Stmt::Return(e, _) => walk_expr(e, f),
                        Stmt::ReturnStatus { status, body, .. } => {
                            walk_expr(status, f);
                            if let Some(b) = body {
                                walk_expr(b, f);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Expr::StructLit { fields, .. } => {
            for (_, e) in fields {
                walk_expr(e, f);
            }
        }
        Expr::Ok(inner, _) | Expr::Err(inner, _) | Expr::Try(inner, _) | Expr::Await(inner, _) => {
            walk_expr(inner, f)
        }
        // Fp.3 — NamedArg passthrough.
        Expr::NamedArg { value, .. } => walk_expr(value, f),
    }
}

// Stubs so the code compiles without unused warnings (Param and
// MatchArm are imported for walker completeness in future
// lints, although we do not touch them directly today).
#[allow(dead_code)]
fn _refs(_: &Param, _: &MatchArm) {}

// ---------------------------------------------------------------------------
// Suppression with `// @allow(<name>)`
// ---------------------------------------------------------------------------

/// Inspects the raw source: if the line immediately before
/// the finding contains `// @allow(<name>)`, we silence it.
///
/// MVP decision: lookahead only to the previous line (no multiple
/// lines, no inline). Simple, predictable.
fn apply_suppressions(source: &str, findings: &mut Vec<LintFinding>) {
    let lines: Vec<&str> = source.lines().collect();
    findings.retain(|f| {
        if f.line == 0 || f.line == 1 {
            return true;
        }
        let prev = lines.get(f.line - 2).copied().unwrap_or("");
        let needle = format!("@allow({})", f.name);
        !prev.contains(&needle)
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;

    fn lint(src: &str) -> Vec<LintFinding> {
        let tokens = tokenize(src).expect("tokenize");
        let program = parse(tokens).expect("parse");
        lint_source(src, &program)
    }

    fn names(findings: &[LintFinding]) -> Vec<&str> {
        findings.iter().map(|f| f.name).collect()
    }

    #[test]
    fn unused_variable_simple() {
        let src = "let x = 5\nprint(\"hola\")";
        let findings = lint(src);
        assert_eq!(names(&findings), vec!["unused_variable"]);
        assert!(findings[0].message.contains("`x`"));
    }

    #[test]
    fn unused_variable_se_usa_no_flaguea() {
        let src = "let x = 5\nprint(x)";
        let findings = lint(src);
        assert!(findings.is_empty(), "no debería flaguear: {:?}", findings);
    }

    #[test]
    fn unused_variable_prefijo_underscore_se_ignora() {
        let src = "let _temp = 5\nprint(\"hola\")";
        let findings = lint(src);
        assert!(
            findings.is_empty(),
            "vars con `_` no se flaguean: {:?}",
            findings
        );
    }

    #[test]
    fn unused_variable_supresion_con_allow_funciona() {
        let src = "// @allow(unused_variable)\nlet x = 5\nprint(\"hola\")";
        let findings = lint(src);
        assert!(
            findings.is_empty(),
            "@allow debería suprimir: {:?}",
            findings
        );
    }

    #[test]
    fn unused_variable_dentro_de_fn() {
        let src = "fn f() {\n    let local = 1\n    return 0\n}\nf()";
        let findings = lint(src);
        assert_eq!(names(&findings), vec!["unused_variable"]);
        assert!(findings[0].message.contains("`local`"));
    }

    #[test]
    fn unused_import_from_with_alias() {
        let src = "from math import sqrt as raiz\nprint(\"hola\")";
        let findings = lint(src);
        assert_eq!(names(&findings), vec!["unused_import"]);
        assert!(findings[0].message.contains("`raiz`"));
    }

    #[test]
    fn unused_import_usado_no_flaguea() {
        let src = "from math import sqrt\nprint(sqrt(16.0))";
        let findings = lint(src);
        assert!(findings.is_empty(), "{:?}", findings);
    }

    #[test]
    fn unused_import_import_modulo() {
        let src = "import math\nprint(\"hola\")";
        let findings = lint(src);
        assert_eq!(names(&findings), vec!["unused_import"]);
        assert!(findings[0].message.contains("`math`"));
    }

    #[test]
    fn useless_match_wildcard_se_flaguea() {
        let src = "let x = 5\nmatch x { _ => print(\"hola\") }";
        let findings = lint(src);
        let useless: Vec<&LintFinding> = findings
            .iter()
            .filter(|f| f.name == "useless_match")
            .collect();
        assert_eq!(useless.len(), 1);
    }

    #[test]
    fn useless_match_dos_arms_no_se_flaguea() {
        let src = "let x = 5\nmatch x {\n    0 => print(\"cero\"),\n    _ => print(\"otro\"),\n}";
        let findings = lint(src);
        let useless: Vec<&LintFinding> = findings
            .iter()
            .filter(|f| f.name == "useless_match")
            .collect();
        assert!(useless.is_empty());
    }

    #[test]
    fn string_concat_literales_se_flaguea() {
        let src = "let x = \"a\" + \"b\"\nprint(x)";
        let findings = lint(src);
        let sc: Vec<&LintFinding> = findings
            .iter()
            .filter(|f| f.name == "string_concat")
            .collect();
        assert_eq!(sc.len(), 1);
    }

    #[test]
    fn string_concat_con_var_no_se_flaguea() {
        // Only "both literals" triggers. Concat with var stays OK.
        let src = "let x = \"a\"\nlet y = x + \"b\"\nprint(y)";
        let findings = lint(src);
        let sc: Vec<&LintFinding> = findings
            .iter()
            .filter(|f| f.name == "string_concat")
            .collect();
        assert!(sc.is_empty());
    }

    #[test]
    fn programa_limpio_no_emite_findings() {
        let src =
            "fn greet(name: Str) -> Str {\n    return \"Hola, {name}\"\n}\nprint(greet(\"Fitz\"))";
        let findings = lint(src);
        assert!(findings.is_empty(), "{:?}", findings);
    }

    #[test]
    fn findings_se_ordenan_por_linea_columna() {
        let src = "let z = 1\nlet a = 2\nprint(\"hola\")";
        let findings = lint(src);
        assert_eq!(findings.len(), 2);
        // `z` is on line 1, `a` is on line 2.
        assert!(findings[0].line < findings[1].line);
    }

    #[test]
    fn supresion_solo_aplica_a_la_linea_inmediata_anterior() {
        // The comment is 2 lines above; the suppress should NOT
        // work (it only applies to the immediately previous line).
        let src = "// @allow(unused_variable)\n\nlet x = 5\nprint(\"hola\")";
        let findings = lint(src);
        assert_eq!(names(&findings), vec!["unused_variable"]);
    }
}
