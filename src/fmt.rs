//! Formatter de Fitz (`fitz fmt`) — Fase 9.z.1.a.
//!
//! Pretty-printer escrito a mano sobre el AST. Cero config —
//! convenciones fijas (4 espacios indent, comillas dobles, trailing
//! comma solo multi-línea, max 100 chars soft).
//!
//! Flujo: `format_source(src)` → tokenize → parse → walk del AST →
//! string. El caller decide qué hacer con el string (write o compare).
//!
//! ## ⚠ LIMITACIÓN CRÍTICA DE 9.z.1.a — comments + blank lines se borran
//!
//! El lexer strippea comentarios antes de llegar al AST, y el
//! formatter no preserva blank lines del usuario. Por lo tanto, al
//! reescribir un archivo, **se pierden comentarios (`//`) y todas
//! las líneas en blanco intencionales del autor**.
//!
//! Esto es **table-stakes faltante** para un formatter de
//! producción (gofmt, prettier, black todos preservan). Se cierra
//! en **9.z.1.b**:
//!
//! - lexer emite comments como tokens (side stream)
//! - parser arma una side-table `Vec<(SpanKey, Comment)>` adyacente
//!   al AST
//! - formatter threadea los comments de vuelta al output según
//!   posición original
//!
//! Mientras 9.z.1.b no aterriza, `fitz fmt` (modo write) emite un
//! warning loud al usuario advirtiendo la pérdida. El modo
//! `--check` (read-only) no necesita warning — no rompe nada.
//!
//! ## Otras deudas (no bloquean MVP)
//!
//! - **`is_let` perdido en el AST**: el parser produce el mismo
//!   `Stmt::Assign` para `let x = 1` y `x = 1` (re-asignación). El
//!   formatter inspecciona la línea del source via `Span` para
//!   detectar `let` y preservarlo. Refactor del AST (agregar
//!   `is_let: bool`) es deuda menor; el hack actual está aislado
//!   en `stmt_has_let_keyword`.
//! - **Nodos no manejados** caen al fallback `// <inválido>`. En el
//!   MVP cubrimos los nodos del AST que aparecen en >90% del código
//!   de la guía; los raros se completan iterativamente.
//! - **Auto-wrap de líneas > 100 chars**: NO implementado. El
//!   formatter no rompe líneas largas. Auto-wrap requiere análisis
//!   de break-points sensato — deuda futura si aparece presión.

use crate::ast::{
    AssignTarget, BinOpKind, Decorator, Expr, MatchArm, Param, Pattern, Span, Stmt, StrPart,
    TypeExpr, UnaryOpKind,
};
use crate::error::FitzError;
use crate::lexer::tokenize;
use crate::parser::parse;

/// Estilo del formatter — cero config en 9.z.1, pero centralizado
/// acá por si en el futuro se introduce `fitz.toml [fmt]`.
struct Style;
impl Style {
    const INDENT: &'static str = "    "; // 4 espacios
}

/// Estado del formatter durante el walk del AST.
struct FmtCtx<'a> {
    indent_level: usize,
    output: String,
    /// Source original — usado para detectar `let` keyword en
    /// `Stmt::Assign` y para fallback de nodos no manejados via Span.
    source: &'a str,
}

impl<'a> FmtCtx<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            indent_level: 0,
            output: String::new(),
            source,
        }
    }

    fn write(&mut self, s: &str) {
        self.output.push_str(s);
    }

    fn newline(&mut self) {
        self.output.push('\n');
    }

    fn write_indent(&mut self) {
        for _ in 0..self.indent_level {
            self.output.push_str(Style::INDENT);
        }
    }

    fn with_indent<F: FnOnce(&mut Self)>(&mut self, f: F) {
        self.indent_level += 1;
        f(self);
        self.indent_level -= 1;
    }
}

/// Entry point público. Parsea el source y produce su forma
/// formateada. Si el source tiene errores de sintaxis, falla — el
/// formatter no intenta arreglar código que el parser no entiende.
///
/// Idempotencia: `format_source(format_source(x)) == format_source(x)`
/// para cualquier código válido (testeado en unit tests).
pub fn format_source(source: &str) -> Result<String, FitzError> {
    let tokens = tokenize(source)?;
    let program = parse(tokens)?;
    let mut ctx = FmtCtx::new(source);
    format_program(&mut ctx, &program);
    Ok(ctx.output)
}

// ---- Programa + statements ----

fn format_program(ctx: &mut FmtCtx, program: &[Stmt]) {
    for (i, stmt) in program.iter().enumerate() {
        if i > 0 && needs_blank_line_before(&program[i - 1], stmt) {
            ctx.newline();
        }
        // Preservar comentarios y blank lines via source-inspection
        // queda como deuda — el formatter actual los borra. Refactor
        // mayor (lexer que retiene comentarios + AST con comment
        // attachments) llega cuando aparezca presión real.
        fmt_stmt(ctx, stmt);
        ctx.newline();
    }
}

/// Inserta blank line entre stmts top-level cuando uno de los dos es
/// "complejo" (fn/type/decorator), para legibilidad. Stmts simples
/// consecutivos no obtienen blank line.
fn needs_blank_line_before(prev: &Stmt, curr: &Stmt) -> bool {
    is_complex_top_level(prev) || is_complex_top_level(curr)
}

fn is_complex_top_level(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::FnDef { .. } | Stmt::TypeDef { .. }
    )
}

fn fmt_stmt(ctx: &mut FmtCtx, stmt: &Stmt) {
    ctx.write_indent();
    match stmt {
        Stmt::Assign { target, type_, value, span } => {
            fmt_assign(ctx, target, type_.as_ref(), value, *span);
        }
        Stmt::Return(expr, _) => {
            ctx.write("return ");
            fmt_expr(ctx, expr);
        }
        Stmt::ReturnStatus { status, body, .. } => {
            ctx.write("return ");
            fmt_expr(ctx, status);
            if let Some(b) = body {
                ctx.write(" ");
                fmt_expr(ctx, b);
            }
        }
        Stmt::Expr(expr, _) => {
            fmt_expr(ctx, expr);
        }
        Stmt::FnDef {
            name, params, return_type, body, is_async, decorators, ..
        } => {
            fmt_fndef(ctx, name, params, return_type.as_ref(), body, *is_async, decorators);
        }
        Stmt::TypeDef { name, fields, .. } => {
            fmt_typedef(ctx, name, fields);
        }
        Stmt::Break(_) => ctx.write("break"),
        Stmt::Continue(_) => ctx.write("continue"),
        Stmt::While { condition, body, .. } => {
            ctx.write("while (");
            fmt_expr(ctx, condition);
            ctx.write(") ");
            fmt_block(ctx, body);
        }
        Stmt::Loop { body, .. } => {
            ctx.write("loop ");
            fmt_block(ctx, body);
        }
        Stmt::For { var, iter, body, .. } => {
            ctx.write("for ");
            ctx.write(var);
            ctx.write(" in ");
            fmt_expr(ctx, iter);
            ctx.write(" ");
            fmt_block(ctx, body);
        }
        Stmt::Import { path, alias, .. } => {
            ctx.write("import ");
            ctx.write(&path.join("."));
            if let Some(a) = alias {
                ctx.write(" as ");
                ctx.write(a);
            }
        }
        Stmt::FromImport { path, names, .. } => {
            ctx.write("from ");
            ctx.write(&path.join("."));
            ctx.write(" import ");
            let parts: Vec<String> = names
                .iter()
                .map(|(n, a)| match a {
                    Some(alias) => format!("{n} as {alias}"),
                    None => n.clone(),
                })
                .collect();
            ctx.write(&parts.join(", "));
        }
        Stmt::Error(_) => {
            // El strict parser no produce esto; defensive fallback.
            ctx.write("// <stmt inválido>");
        }
    }
}

fn fmt_assign(
    ctx: &mut FmtCtx,
    target: &AssignTarget,
    type_: Option<&TypeExpr>,
    value: &Expr,
    span: Span,
) {
    // El AST no preserva si había `let` keyword. Recuperamos del
    // source — deuda documentada en el header del módulo.
    let has_let = match target {
        AssignTarget::Ident(_) => stmt_has_let_keyword(ctx.source, span),
        AssignTarget::Field { .. } => false, // `obj.f = v` nunca lleva let
    };

    if has_let {
        ctx.write("let ");
    }
    fmt_assign_target(ctx, target);
    if let Some(t) = type_ {
        ctx.write(": ");
        ctx.write(&t.display_name());
    }
    ctx.write(" = ");
    fmt_expr(ctx, value);
}

fn fmt_assign_target(ctx: &mut FmtCtx, target: &AssignTarget) {
    match target {
        AssignTarget::Ident(n) => ctx.write(n),
        AssignTarget::Field { object, field } => {
            fmt_expr(ctx, object);
            ctx.write(".");
            ctx.write(field);
        }
    }
}

/// Inspecciona la línea del source en `span` para ver si el stmt
/// arranca con `let`. Spans son 1-based. Hack contenido para
/// suplir info que el AST no preserva (ver header del módulo).
fn stmt_has_let_keyword(source: &str, span: Span) -> bool {
    if !span.is_known() {
        // Stmts sintéticos (fn body de flecha, tests construyendo
        // AST a mano) — preferimos NO emitir `let` por consistencia
        // con código que típicamente no lo lleva.
        return false;
    }
    let line_idx = span.line.saturating_sub(1);
    let Some(line) = source.lines().nth(line_idx) else {
        return false;
    };
    let col_idx = span.column.saturating_sub(1);
    if col_idx >= line.len() {
        return false;
    }
    let from_col = &line[col_idx..];
    from_col.trim_start().starts_with("let ")
}

fn fmt_block(ctx: &mut FmtCtx, body: &[Stmt]) {
    if body.is_empty() {
        ctx.write("{}");
        return;
    }
    ctx.write("{");
    ctx.newline();
    ctx.with_indent(|ctx| {
        for stmt in body {
            fmt_stmt(ctx, stmt);
            ctx.newline();
        }
    });
    ctx.write_indent();
    ctx.write("}");
}

fn fmt_fndef(
    ctx: &mut FmtCtx,
    name: &str,
    params: &[Param],
    return_type: Option<&TypeExpr>,
    body: &[Stmt],
    is_async: bool,
    decorators: &[Decorator],
) {
    // Decorators uno por línea, en orden.
    for deco in decorators {
        fmt_decorator(ctx, deco);
        ctx.newline();
        ctx.write_indent();
    }

    if is_async {
        ctx.write("async ");
    }
    ctx.write("fn ");
    ctx.write(name);
    ctx.write("(");
    let param_strs: Vec<String> = params.iter().map(fmt_param_to_string).collect();
    ctx.write(&param_strs.join(", "));
    ctx.write(")");
    if let Some(rt) = return_type {
        ctx.write(" -> ");
        ctx.write(&rt.display_name());
    }
    ctx.write(" ");
    fmt_block(ctx, body);
}

fn fmt_param_to_string(p: &Param) -> String {
    match &p.type_ {
        Some(t) => format!("{}: {}", p.name, t.display_name()),
        None => p.name.clone(),
    }
}

fn fmt_typedef(ctx: &mut FmtCtx, name: &str, fields: &[crate::ast::Field]) {
    ctx.write("type ");
    ctx.write(name);
    ctx.write(" {");
    if fields.is_empty() {
        ctx.write("}");
        return;
    }
    ctx.newline();
    ctx.with_indent(|ctx| {
        for f in fields {
            ctx.write_indent();
            ctx.write(&f.name);
            ctx.write(": ");
            ctx.write(&f.type_.display_name());
            if let Some(default) = &f.default {
                ctx.write(" = ");
                fmt_expr(ctx, default);
            }
            ctx.newline();
        }
    });
    ctx.write_indent();
    ctx.write("}");
}

fn fmt_decorator(ctx: &mut FmtCtx, deco: &Decorator) {
    ctx.write("@");
    ctx.write(&deco.name);
    if deco.args.is_empty() && deco.kwargs.is_empty() {
        return;
    }
    ctx.write("(");
    let mut parts: Vec<String> = deco.args.iter().map(expr_to_inline_string).collect();
    for (k, v) in &deco.kwargs {
        parts.push(format!("{}={}", k, expr_to_inline_string(v)));
    }
    ctx.write(&parts.join(", "));
    ctx.write(")");
}

// ---- Expressions ----

fn fmt_expr(ctx: &mut FmtCtx, expr: &Expr) {
    match expr {
        Expr::Int(n, _) => ctx.write(&n.to_string()),
        Expr::Float(f, _) => ctx.write(&format_float_literal(*f)),
        Expr::Str(s, _) => ctx.write(&format_str_literal(s)),
        Expr::Bool(b, _) => ctx.write(if *b { "true" } else { "false" }),
        Expr::Null(_) => ctx.write("null"),
        Expr::Ident(name, _) => ctx.write(name),
        Expr::StrInterp(parts, _) => fmt_str_interp(ctx, parts),
        Expr::BinOp { op, left, right, .. } => fmt_binop(ctx, op, left, right),
        Expr::UnaryOp { op, operand, .. } => {
            match op {
                UnaryOpKind::Neg => ctx.write("-"),
            }
            fmt_expr_with_parens_if_needed(ctx, operand);
        }
        Expr::Call { callee, args, .. } => {
            fmt_expr(ctx, callee);
            ctx.write("(");
            let arg_strs: Vec<String> = args.iter().map(expr_to_inline_string).collect();
            ctx.write(&arg_strs.join(", "));
            ctx.write(")");
        }
        Expr::Field { object, field, .. } => {
            fmt_expr(ctx, object);
            ctx.write(".");
            ctx.write(field);
        }
        Expr::Index { object, index, .. } => {
            fmt_expr(ctx, object);
            ctx.write("[");
            fmt_expr(ctx, index);
            ctx.write("]");
        }
        Expr::List(items, _) => {
            ctx.write("[");
            let parts: Vec<String> = items.iter().map(expr_to_inline_string).collect();
            ctx.write(&parts.join(", "));
            ctx.write("]");
        }
        Expr::Map(entries, _) => {
            ctx.write("{");
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}: {}", expr_to_inline_string(k), expr_to_inline_string(v)))
                .collect();
            ctx.write(&parts.join(", "));
            ctx.write("}");
        }
        Expr::Range { start, end, .. } => {
            fmt_expr(ctx, start);
            ctx.write("..");
            fmt_expr(ctx, end);
        }
        Expr::If { condition, then, else_, .. } => {
            ctx.write("if (");
            fmt_expr(ctx, condition);
            ctx.write(") ");
            fmt_block(ctx, then);
            if let Some(e) = else_ {
                ctx.write(" else ");
                fmt_block(ctx, e);
            }
        }
        Expr::Match { value, arms, .. } => fmt_match(ctx, value, arms),
        Expr::StructLit { type_name, fields, .. } => {
            ctx.write(type_name);
            ctx.write(" { ");
            let parts: Vec<String> = fields
                .iter()
                .map(|(n, v)| format!("{}: {}", n, expr_to_inline_string(v)))
                .collect();
            ctx.write(&parts.join(", "));
            ctx.write(" }");
        }
        Expr::Ok(inner, _) => {
            ctx.write("Ok(");
            fmt_expr(ctx, inner);
            ctx.write(")");
        }
        Expr::Err(inner, _) => {
            ctx.write("Err(");
            fmt_expr(ctx, inner);
            ctx.write(")");
        }
        Expr::Try(inner, _) => {
            fmt_expr(ctx, inner);
            ctx.write("?");
        }
        Expr::Await(inner, _) => {
            fmt_expr(ctx, inner);
            ctx.write(".await");
        }
        Expr::FnExpr { params, body, .. } => fmt_fnexpr(ctx, params, body),
        Expr::Error(_) => ctx.write("/* <expr inválida> */"),
    }
}

/// Para evitar regenerar el FmtCtx, las expresiones que aparecen
/// adentro de una sola línea (args de fn, items de lista, etc.) se
/// formatean a String aparte y se concatenan.
fn expr_to_inline_string(expr: &Expr) -> String {
    let mut ctx = FmtCtx::new("");
    fmt_expr(&mut ctx, expr);
    ctx.output
}

fn fmt_expr_with_parens_if_needed(ctx: &mut FmtCtx, expr: &Expr) {
    // Heurística mínima: UnaryOp sobre BinOp necesita parens.
    let needs = matches!(expr, Expr::BinOp { .. });
    if needs {
        ctx.write("(");
        fmt_expr(ctx, expr);
        ctx.write(")");
    } else {
        fmt_expr(ctx, expr);
    }
}

fn fmt_binop(ctx: &mut FmtCtx, op: &BinOpKind, left: &Expr, right: &Expr) {
    fmt_expr(ctx, left);
    ctx.write(" ");
    ctx.write(binop_str(op));
    ctx.write(" ");
    fmt_expr(ctx, right);
}

fn binop_str(op: &BinOpKind) -> &'static str {
    match op {
        BinOpKind::Add => "+",
        BinOpKind::Sub => "-",
        BinOpKind::Mul => "*",
        BinOpKind::Div => "/",
        BinOpKind::Eq => "==",
        BinOpKind::NotEq => "!=",
        BinOpKind::Lt => "<",
        BinOpKind::LtEq => "<=",
        BinOpKind::Gt => ">",
        BinOpKind::GtEq => ">=",
        BinOpKind::And => "and",
        BinOpKind::Or => "or",
    }
}

fn fmt_str_interp(ctx: &mut FmtCtx, parts: &[StrPart]) {
    ctx.write("\"");
    for part in parts {
        match part {
            StrPart::Lit(s) => {
                // Escapamos solo `"` y `\`; el resto pasa raw para
                // preservar emojis, unicode, etc.
                for c in s.chars() {
                    match c {
                        '"' => ctx.write("\\\""),
                        '\\' => ctx.write("\\\\"),
                        '\n' => ctx.write("\\n"),
                        '\t' => ctx.write("\\t"),
                        _ => ctx.output.push(c),
                    }
                }
            }
            StrPart::Expr(e) => {
                ctx.write("{");
                let inline = expr_to_inline_string(e);
                ctx.write(&inline);
                ctx.write("}");
            }
        }
    }
    ctx.write("\"");
}

fn fmt_match(ctx: &mut FmtCtx, value: &Expr, arms: &[MatchArm]) {
    ctx.write("match ");
    fmt_expr(ctx, value);
    ctx.write(" {");
    if arms.is_empty() {
        ctx.write("}");
        return;
    }
    ctx.newline();
    ctx.with_indent(|ctx| {
        for arm in arms {
            ctx.write_indent();
            fmt_pattern(ctx, &arm.pattern);
            ctx.write(" => ");
            fmt_expr(ctx, &arm.body);
            ctx.write(",");
            ctx.newline();
        }
    });
    ctx.write_indent();
    ctx.write("}");
}

fn fmt_pattern(ctx: &mut FmtCtx, pat: &Pattern) {
    match pat {
        Pattern::Int(n) => ctx.write(&n.to_string()),
        Pattern::Float(f) => ctx.write(&format_float_literal(*f)),
        Pattern::Str(s) => ctx.write(&format_str_literal(s)),
        Pattern::Bool(b) => ctx.write(if *b { "true" } else { "false" }),
        Pattern::Null => ctx.write("null"),
        Pattern::Ident(name) => ctx.write(name),
        Pattern::Wildcard => ctx.write("_"),
        Pattern::OkBinding(n) => {
            ctx.write("Ok(");
            ctx.write(n);
            ctx.write(")");
        }
        Pattern::ErrBinding(n) => {
            ctx.write("Err(");
            ctx.write(n);
            ctx.write(")");
        }
        Pattern::OkWildcard => ctx.write("Ok(_)"),
        Pattern::ErrWildcard => ctx.write("Err(_)"),
        Pattern::Range { start, end } => {
            ctx.write(&start.to_string());
            ctx.write("..");
            ctx.write(&end.to_string());
        }
    }
}

fn fmt_fnexpr(ctx: &mut FmtCtx, params: &[Param], body: &[Stmt]) {
    ctx.write("fn(");
    let param_strs: Vec<String> = params.iter().map(fmt_param_to_string).collect();
    ctx.write(&param_strs.join(", "));
    ctx.write(")");
    // Si el body es un único return, usar la sintaxis flecha.
    if let [Stmt::Return(expr, _)] = body {
        ctx.write(" => ");
        let inline = expr_to_inline_string(expr);
        ctx.write(&inline);
    } else {
        ctx.write(" ");
        fmt_block(ctx, body);
    }
}

// ---- Helpers de formato de literales ----

/// Float literal con al menos un decimal (`1.0` no `1`) para
/// distinguir visualmente de Int.
fn format_float_literal(f: f64) -> String {
    let s = format!("{f}");
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{s}.0")
    }
}

/// Str literal con escaping mínimo (igual que en StrInterp).
fn format_str_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserta que `format_source(input)` == `expected`.
    fn check(input: &str, expected: &str) {
        let actual = format_source(input).unwrap_or_else(|e| panic!("parse: {e}"));
        assert_eq!(actual, expected, "input:\n{input}\nactual:\n{actual}\nexpected:\n{expected}");
    }

    /// Asserta que `format_source` es idempotente sobre `input`.
    fn check_idempotent(input: &str) {
        let once = format_source(input).unwrap_or_else(|e| panic!("parse(once): {e}"));
        let twice = format_source(&once).unwrap_or_else(|e| panic!("parse(twice): {e}"));
        assert_eq!(
            once, twice,
            "no idempotente:\noriginal:\n{input}\nonce:\n{once}\ntwice:\n{twice}"
        );
    }

    #[test]
    fn formatea_print_simple() {
        check("print(\"hola\")\n", "print(\"hola\")\n");
    }

    #[test]
    fn formatea_let_preserva_keyword() {
        check("let x = 1\n", "let x = 1\n");
    }

    #[test]
    fn formatea_assign_sin_let_preserva() {
        // Si el source no tiene `let`, el formatter tampoco lo emite.
        check("name = \"Patagonia\"\n", "name = \"Patagonia\"\n");
    }

    #[test]
    fn formatea_let_con_anotacion_de_tipo() {
        check("let x: Int = 5\n", "let x: Int = 5\n");
    }

    #[test]
    fn formatea_fn_def_simple() {
        check(
            "fn double(n: Int) -> Int { return n * 2 }\n",
            "fn double(n: Int) -> Int {\n    return n * 2\n}\n",
        );
    }

    #[test]
    fn formatea_fn_def_arrow_se_normaliza_a_bloque() {
        // La forma flecha (`=> expr`) el parser la convierte a
        // `body: [Return(expr)]`. Como el AST no preserva la forma
        // flecha vs bloque, el formatter siempre emite bloque.
        // Deuda menor — refactor del AST si pinta como ergonomic.
        check(
            "fn add(a, b) => a + b\n",
            "fn add(a, b) {\n    return a + b\n}\n",
        );
    }

    #[test]
    fn formatea_if_else_como_stmt() {
        check(
            "if (x > 0) { print(\"pos\") } else { print(\"non-pos\") }\n",
            "if (x > 0) {\n    print(\"pos\")\n} else {\n    print(\"non-pos\")\n}\n",
        );
    }

    #[test]
    fn formatea_while_y_for() {
        check(
            "while (i < 10) { i = i + 1 }\n",
            "while (i < 10) {\n    i = i + 1\n}\n",
        );
        check(
            "for x in 0..10 { print(x) }\n",
            "for x in 0..10 {\n    print(x)\n}\n",
        );
    }

    #[test]
    fn formatea_loop_con_break() {
        check(
            "loop { break }\n",
            "loop {\n    break\n}\n",
        );
    }

    #[test]
    fn formatea_type_def_con_fields_y_defaults() {
        check(
            "type User { id: Int, name: Str = \"anon\", email: Str? }\n",
            "type User {\n    id: Int\n    name: Str = \"anon\"\n    email: Str?\n}\n",
        );
    }

    #[test]
    fn formatea_struct_lit_inline() {
        check(
            "let u = User { id: 1, name: \"x\" }\n",
            "let u = User { id: 1, name: \"x\" }\n",
        );
    }

    #[test]
    fn formatea_lista_y_mapa_inline() {
        check(
            "let xs = [1, 2, 3]\n",
            "let xs = [1, 2, 3]\n",
        );
        check(
            "let m = {\"a\": 1, \"b\": 2}\n",
            "let m = {\"a\": 1, \"b\": 2}\n",
        );
    }

    #[test]
    fn formatea_match_sobre_result() {
        check(
            "let r = match x { Ok(v) => v, Err(_) => 0, }\n",
            "let r = match x {\n    Ok(v) => v,\n    Err(_) => 0,\n}\n",
        );
    }

    #[test]
    fn formatea_str_interp_con_var() {
        check(
            "print(\"hola, {name}\")\n",
            "print(\"hola, {name}\")\n",
        );
    }

    #[test]
    fn formatea_decorator_apilable() {
        check(
            "@get(\"/users\") fn list() => 1\n",
            "@get(\"/users\")\nfn list() {\n    return 1\n}\n",
        );
    }

    #[test]
    fn formatea_import_y_from_import_con_alias() {
        check("import utils\n", "import utils\n");
        check("import utils as u\n", "import utils as u\n");
        check(
            "from utils import a, b as bb\n",
            "from utils import a, b as bb\n",
        );
    }

    #[test]
    fn formatea_async_y_await() {
        check(
            "async fn ping() -> Int { return sleep(0).await }\n",
            "async fn ping() -> Int {\n    return sleep(0).await\n}\n",
        );
    }

    #[test]
    fn formatea_ok_err_try() {
        check(
            "fn f() -> Result<Int> { return Ok(g()?) }\n",
            "fn f() -> Result<Int> {\n    return Ok(g()?)\n}\n",
        );
    }

    #[test]
    fn formatea_blank_line_entre_fn_defs() {
        check(
            "fn a() => 1\nfn b() => 2\n",
            "fn a() {\n    return 1\n}\n\nfn b() {\n    return 2\n}\n",
        );
    }

    #[test]
    fn idempotente_sobre_programa_complejo() {
        check_idempotent(
            r#"
let x = 10
let name: Str = "fitz"

fn greet(n: Str) -> Str {
    return "hola, {n}"
}

@get("/users/{id}")
fn get_user(id: Int) -> User {
    return User { id: id, name: "x" }
}

type User {
    id: Int
    name: Str
    email: Str?
}

fn main() {
    for i in 0..5 {
        print(greet("user-{i}"))
    }
}
"#,
        );
    }

    #[test]
    fn idempotente_sobre_match_y_result() {
        check_idempotent(
            r#"
fn divide(a: Int, b: Int) -> Result<Int> {
    if (b == 0) {
        return Err("div by zero")
    } else {
        return Ok(a / b)
    }
}

fn run() {
    match divide(10, 2) {
        Ok(v) => print("v={v}"),
        Err(msg) => print("err: {msg}"),
    }
}
"#,
        );
    }
}
