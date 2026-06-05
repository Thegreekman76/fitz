// lint.rs — Fase 9.z.5 (`fitz lint`)
//
// Linter de patrones más allá de tipos. El checker estático
// (`types.rs`) captura errores duros (type mismatch, exhaustividad
// de match, aridad de fn). El linter captura patrones que SÍ
// compilan pero son code smells: vars no usadas, imports muertos,
// match con un solo arm catch-all, concatenación de strings en
// lugar de interpolación.
//
// Decisiones del MVP:
//
// - **4 lints**: `unused_variable`, `unused_import`, `useless_match`,
//   `string_concat`. El roadmap original menciona también
//   `panic_in_test_only` y `redundant_clone`; el primero NO aplica
//   (Fitz no tiene `panic!` builtin propio), el segundo requiere
//   análisis de movimientos que el compilador todavía no hace.
//
// - **Default warning**: `fitz lint` siempre exit 0, salvo que el
//   user pase `--deny <lint>` con un lint que aparezca en el output.
//   Cargo-clippy style.
//
// - **Supresión** con comment `// @allow(<name>)` en la línea
//   INMEDIATAMENTE ANTERIOR al stmt offending. Inspección del source
//   raw (no trivia stream del lexer): pragmático y suficiente.
//
// - **Auto-fix** (`--fix`) solo para `string_concat` (transformación
//   trivial a interpolación). El resto exige edición a mano.
//
// - **Catálogo cerrado**: los 4 lints viven en este archivo. Plugins
//   futuros no son scope del MVP.
//
// El linter trabaja sobre el AST + el source crudo (para
// suppression). NO usa el checker: lints como `unused_variable` no
// dependen de información de tipos. Esto mantiene la separación
// `check` (tipos) / `lint` (patrones).

use crate::ast::{AssignTarget, BinOpKind, Expr, MatchArm, Param, Pattern, Program, Stmt, StrPart};

// ---------------------------------------------------------------------------
// Tipos públicos
// ---------------------------------------------------------------------------

/// Un finding producido por algún lint. El runner los imprime
/// estilo cargo-clippy:
/// ```text
/// warning: variable `x` declarada pero no usada
///   --> src/main.fitz:3:5
///   = nota: si es intencional, prefijá con `_` (ej. `_x`) o suprimí
///          con `// @allow(unused_variable)` en la línea anterior.
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct LintFinding {
    /// Nombre canónico del lint (matchea el `// @allow(<name>)`).
    /// Ej: `"unused_variable"`, `"string_concat"`.
    pub name: &'static str,
    /// Mensaje principal (estilo "warning: ...").
    pub message: String,
    /// 1-based, paralelo a `FitzError`.
    pub line: usize,
    pub column: usize,
    /// Hint opcional debajo del mensaje principal.
    pub hint: Option<String>,
    /// Si el lint tiene auto-fix, el reemplazo del fragmento sobre el
    /// rango `(start_line, start_col)..(end_line, end_col)`. Hoy solo
    /// `string_concat` lo emite.
    pub fix: Option<LintFix>,
}

/// Patch literal del source para `--fix`. Reemplaza el rango
/// `(start_line, start_col)..(end_line, end_col)` con `replacement`.
/// Posiciones 1-based como las del lexer.
#[derive(Debug, Clone, PartialEq)]
pub struct LintFix {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub replacement: String,
}

/// Entry point del linter. Walkea el `program` recogiendo findings
/// y aplica supresiones leyendo `source` raw. Devuelve los findings
/// en el orden estable (line, column).
pub fn lint_source(source: &str, program: &Program) -> Vec<LintFinding> {
    let mut findings: Vec<LintFinding> = Vec::new();

    // Pre-collect: todos los `Expr::Ident(name)` del programa.
    // Lo necesitan `unused_variable` y `unused_import`.
    let uses = collect_ident_uses(program);

    lint_unused_variables(program, &uses, &mut findings);
    lint_unused_imports(program, &uses, &mut findings);
    lint_useless_match(program, &mut findings);
    lint_string_concat(source, program, &mut findings);

    apply_suppressions(source, &mut findings);

    // Orden estable: por línea+columna para que el output sea
    // predecible cuando hay múltiples findings.
    findings.sort_by_key(|f| (f.line, f.column, f.name));
    findings
}

// ---------------------------------------------------------------------------
// Recolección de uses
// ---------------------------------------------------------------------------

/// Colecciona todos los nombres referenciados (`Expr::Ident(name)`)
/// en el programa. Set único (HashSet) — no nos importa la cantidad
/// de uses ni dónde, solo si AL MENOS uno existe. Walkea recursivo
/// sobre exprs adentro de stmts (incluyendo bodies de fn anidadas).
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
            // El pattern declara names (no uses). El value SÍ es use.
            collect_uses_in_expr(value, uses);
        }
        Stmt::Assign { target, value, .. } => {
            // El target NO es use (es definición/reasignación). El
            // value SÍ se walkea (puede contener idents).
            collect_uses_in_expr(value, uses);
            // Excepción: si el target es `obj.field = ...`, el `obj`
            // es un use. `AssignTarget::Field { object }` lo expone.
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
            // Decorators: pueden referenciar fns (ej. `@middleware(logger)`).
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
            // Los defaults pueden referenciar consts del scope.
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
        // Mini-tanda C + Cmp+ — list comprehension. Walkeamos iter
        // del primer clause + extras + filter + expr. Los `var`s se
        // BINDEAN adentro, no son usos.
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
        // Mini-tanda Cmp+ — map comprehension.
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
        // Fp.3 — NamedArg passthrough al value.
        Expr::NamedArg { value, .. } => {
            collect_uses_in_expr(value, uses);
        }
    }
}

// ---------------------------------------------------------------------------
// Lint: unused_variable
// ---------------------------------------------------------------------------

/// `unused_variable`: detecta `let x = ...` (o `x = ...` declaración
/// inicial) cuyo nombre NUNCA aparece en `Expr::Ident` del programa.
///
/// Caveats del MVP:
/// - **Shadowing no se detecta**: `let x = 5; let x = 10; x` reporta
///   correcto que `x` se usa (no flagueo); el shadowing del primero
///   queda invisible.
/// - **Reasignación cuenta como uso**: NO; el target de `Stmt::Assign`
///   no se cuenta como use, solo el value y los Idents en otras exprs.
/// - **Prefijo `_`**: vars que arrancan con `_` se ignoran (convención
///   "intencionalmente no usada", paralelo a Rust).
/// - **Params de fns**: NO se flaguean en MVP (muchos handlers HTTP /
///   callbacks reciben params que no necesitan usar; flaguearlos
///   sería ruido).
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

/// `unused_import`: detecta `import X` y `from X import Y` cuyo
/// binding NO se referencia. Para `import foo as f`, el binding es
/// `f`. Para `from foo import bar as b`, los bindings son los
/// `b` (uno por entry). `from foo import bar` el binding es `bar`.
fn lint_unused_imports(
    program: &Program,
    uses: &std::collections::HashSet<String>,
    findings: &mut Vec<LintFinding>,
) {
    for stmt in program {
        match stmt {
            Stmt::Import { path, alias, span } => {
                // El binding default es el ÚLTIMO segmento del path.
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

/// `useless_match`: detecta `match expr { _ => body }` (o
/// `match expr { ident => body }`) con UN solo arm catch-all. El
/// match no aporta nada; el user puede reemplazarlo con un `let`
/// directo (`let _ = expr; body` o `let ident = expr; body`).
///
/// NO flaguea matches con catch-all + otros arms (eso sí es útil).
/// NO flaguea matches con 0 arms (parser ya los rechaza).
fn lint_useless_match(program: &Program, findings: &mut Vec<LintFinding>) {
    for stmt in program {
        walk_exprs_in_stmt(stmt, &mut |expr| {
            if let Expr::Match { arms, span, .. } = expr {
                if arms.len() == 1 {
                    let is_catchall =
                        matches!(arms[0].pattern, Pattern::Wildcard | Pattern::Ident(_));
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

/// `string_concat`: detecta `"a" + "b"` y similares (cualquier `+`
/// donde AMBOS operandos son `Expr::Str` literales). Sugiere
/// reemplazar con interpolación: `"ab"` o `"{x}{y}"`.
///
/// **Auto-fix**: para el caso "ambos literales", emite el
/// reemplazo concatenando los strings y produciendo un único
/// literal. Casos más complejos (`"hola, " + name`) NO emiten fix
/// porque requieren convertir a interpolación, que necesita
/// parsing del nombre y escape — sub-paso futuro.
///
/// Heredamos los spans `start` del literal izquierdo y `end` del
/// literal derecho del source raw. Para hacerlo bien necesitamos
/// `end_span` que el AST hoy no tiene (deuda S1 residual);
/// workaround: leer el source y buscar el rango. Para el MVP de
/// 9.z.5, **sin auto-fix** (lo difiero) — solo emitimos la
/// advertencia con sugerencia textual.
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

/// Walk recursivo sobre las expresiones de un stmt, invocando `f`
/// por cada Expr visitada. Útil para lints que detectan patrones de
/// expresión sin importar el stmt contenedor.
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
        // Mini-tanda C + Cmp+ — list comprehension. Walkeamos los
        // sub-Exprs de cada clause + filter + expr.
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
        // Mini-tanda Cmp+ — map comprehension.
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
                // Sp.2 — body es Vec<Stmt>. Iteramos solo las Expr más
                // comunes (Stmt::Expr / Return / ReturnStatus) sin
                // recursión a través de walk_exprs_in_stmt (genera
                // monomorphization recursion limit con closures
                // anidados).
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

// Stubs para que el código compile sin warnings de unused (Param y
// MatchArm los importamos por completitud del walker en futuros
// lints, aunque hoy no los tocamos directo).
#[allow(dead_code)]
fn _refs(_: &Param, _: &MatchArm) {}

// ---------------------------------------------------------------------------
// Supresión con `// @allow(<name>)`
// ---------------------------------------------------------------------------

/// Inspecciona el source raw: si la línea inmediatamente anterior
/// al finding contiene `// @allow(<name>)`, lo silenciamos.
///
/// Decisión MVP: lookahead solo a la línea anterior (no múltiples
/// líneas, no inline). Simple, predecible.
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
        // Solo "ambos literales" dispara. Concat con var queda OK.
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
        // `z` está en línea 1, `a` está en línea 2.
        assert!(findings[0].line < findings[1].line);
    }

    #[test]
    fn supresion_solo_aplica_a_la_linea_inmediata_anterior() {
        // El comment está 2 líneas arriba; la suppress NO debería
        // funcionar (solo aplica a la inmediata anterior).
        let src = "// @allow(unused_variable)\n\nlet x = 5\nprint(\"hola\")";
        let findings = lint(src);
        assert_eq!(names(&findings), vec!["unused_variable"]);
    }
}
