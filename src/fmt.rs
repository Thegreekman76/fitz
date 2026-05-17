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
use crate::lexer::{tokenize_with_trivia, Comment, CommentKind, Trivia};
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
    /// Fase 9.z.1.b — trivia capturada por el lexer (comments +
    /// blank lines). Empty cuando se llama desde `format_source_only_ast`
    /// (path interno de tests que no necesitan threading de trivia).
    trivia: &'a Trivia,
    /// Cursor sobre `trivia.comments` para emitirlos en orden de
    /// aparición sin re-escanear.
    comment_cursor: usize,
}

impl<'a> FmtCtx<'a> {
    fn new(source: &'a str, trivia: &'a Trivia) -> Self {
        Self {
            indent_level: 0,
            output: String::new(),
            source,
            trivia,
            comment_cursor: 0,
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
/// Fase 9.z.1.b: usa `tokenize_with_trivia` para capturar
/// comentarios y blank lines del source, y los re-emite en sus
/// posiciones originales en el output.
///
/// Idempotencia: `format_source(format_source(x)) == format_source(x)`
/// para cualquier código válido (testeado en unit tests).
pub fn format_source(source: &str) -> Result<String, FitzError> {
    let (tokens, trivia) = tokenize_with_trivia(source)?;
    let program = parse(tokens)?;
    let mut ctx = FmtCtx::new(source, &trivia);
    format_program(&mut ctx, &program);
    Ok(ctx.output)
}

// ---- Programa + statements ----

fn format_program(ctx: &mut FmtCtx, program: &[Stmt]) {
    fmt_stmt_list(ctx, program, /* in_block = */ false);
}

/// Renderiza una lista de stmts top-level o adentro de un bloque,
/// threading los comments + blank lines de `ctx.trivia` en sus
/// posiciones originales del source.
///
/// Algoritmo:
///
/// 1. Para cada stmt en orden:
///    - (a) Emit "leading" trivia: comments con
///      `line < stmt.start_line` que todavía no se emitieron.
///    - (b) Si entre el stmt anterior (o el comment anterior) y
///      este hay una blank line preservada, emit blank.
///    - (c) Render el stmt.
///    - (d) Emit "trailing" trivia: comment en la misma línea de
///      fin del stmt (al lado, con 2 espacios).
/// 2. Después del último stmt: emit cualquier comment/blank
///    remanente (post-último).
fn fmt_stmt_list(ctx: &mut FmtCtx, stmts: &[Stmt], in_block: bool) {
    // `prev_end_line` rastrea el límite superior de "ya emitido" para
    // decidir si una línea blank fue preservada en el original.
    // En top-level arranca en 0; adentro de un bloque, en la línea
    // de apertura del bloque (mid-stream).
    let mut prev_end_line: usize = 0;

    for stmt in stmts {
        let stmt_start = stmt.span().line;
        let stmt_end = end_line_of_stmt(stmt);

        // 1a. Leading comments (cualquier comment con line < stmt_start
        // que todavía no se emitió). Esto incluye comentarios del
        // "header" del archivo antes del primer stmt.
        emit_leading_comments(ctx, prev_end_line, stmt_start);

        // 1b. Blank line en el gap entre prev y stmt — dos sources:
        //  (a) Preservada del original: trivia.blank_lines contiene
        //      una línea en (after_what, stmt_start).
        //  (b) Smart heuristic top-level: entre dos stmts donde al
        //      menos uno es fn/type, insertamos blank (mejora
        //      legibilidad). PERO suprimida si acabamos de emitir
        //      un leading comment para el stmt actual — los
        //      comments "se atan" al stmt siguiente y no queremos
        //      separarlos.
        //
        // Skip totalmente si es el primer stmt del file/block.
        //
        // **Bug fix (post-9.z.5)**: cuando estamos `in_block=true` y
        // este es el primer stmt del bloque (`prev_end_line == 0`), NO
        // chequear blanks. Sin esta guarda, `last_emitted_comment_line`
        // puede traer un valor del scope outer (por ej. un trailing
        // comment del stmt anterior al bloque) y `has_blank_between`
        // reportar blanks que están FUERA del bloque actual,
        // insertando un blank spurio adentro.
        //
        // Pero en top-level con leading comments (header del file),
        // `prev_end_line == 0` y queremos preservar la blank entre
        // los comments y el primer stmt. La condición distingue:
        //   - In block: solo si hubo stmt previo (prev_end_line > 0).
        //   - Top-level: si hubo stmt previo O si hubo comments
        //     leading (last_emitted_comment_line > 0) — el header
        //     del file puede ir seguido de blank antes del primer stmt.
        let after_what = std::cmp::max(prev_end_line, last_emitted_comment_line(ctx));
        let block_allows_blank = if in_block {
            prev_end_line > 0
        } else {
            after_what > 0
        };
        let had_blank_in_source = block_allows_blank
            && has_blank_between(ctx.trivia, after_what, stmt_start);
        // Si el último comment emitido pertenece "al gap" entre prev_end_line
        // y stmt_start, suppress smart_blank (comment ya cumple esa función
        // de separación visual + queremos que el comment quede pegado al stmt).
        let leading_comment_just_emitted = last_emitted_comment_line(ctx) > prev_end_line;
        let smart_blank = !in_block
            && prev_end_line > 0
            && !leading_comment_just_emitted
            && needs_blank_line_before_smart(prev_stmt_at(stmts, stmt_start), stmt);
        if had_blank_in_source || smart_blank {
            ctx.newline();
        }

        // 1c. Render el stmt.
        fmt_stmt(ctx, stmt);

        // 1d. Trailing comment en la misma línea del fin del stmt.
        let trailing_text = peek_comment_at_line(ctx, stmt_end).map(|c| c.text.clone());
        if let Some(text) = trailing_text {
            ctx.write("  // ");
            ctx.write(text.trim_start());
            ctx.comment_cursor += 1;
        }

        ctx.newline();
        prev_end_line = stmt_end;
    }

    // Post-última-stmt: solo emit footer comments si estamos
    // top-level. Adentro de un bloque, los comments restantes
    // quedan en el cursor — los procesa el caller exterior cuando
    // emit comments leading del siguiente stmt outer.
    //
    // Deuda menor: comments INSIDE un block, después del último stmt
    // pero antes del `}` (ej. `fn f() { x = 1; // trailing\n }`),
    // terminan saliendo del block en el output formateado. Caso
    // raro en práctica — documentado.
    if !in_block {
        emit_trailing_comments(ctx, prev_end_line);
    }
}

/// Emite todos los comments con `line < upper_bound` que todavía no
/// se emitieron. Los pone como líneas propias con su indent actual.
/// Trailing comments (los de la línea de un stmt) los maneja el
/// caller por separado.
fn emit_leading_comments(ctx: &mut FmtCtx, prev_end_line: usize, upper_bound: usize) {
    while ctx.comment_cursor < ctx.trivia.comments.len() {
        let c = &ctx.trivia.comments[ctx.comment_cursor];
        if c.line >= upper_bound {
            break;
        }
        // Blank line antes del comment si en el original había una
        // entre el último item emitido y este comment.
        let after_what = std::cmp::max(prev_end_line, last_emitted_comment_line_excluding_current(ctx));
        if after_what > 0 && has_blank_between(ctx.trivia, after_what, c.line) {
            ctx.newline();
        }
        emit_single_comment(ctx, c);
        ctx.comment_cursor += 1;
    }
}

/// Emite los comments restantes (cualquier line >= prev_end_line +
/// que no haya sido emitido todavía). Útil para footer comments
/// post-último-stmt.
fn emit_trailing_comments(ctx: &mut FmtCtx, prev_end_line: usize) {
    while ctx.comment_cursor < ctx.trivia.comments.len() {
        let c = &ctx.trivia.comments[ctx.comment_cursor];
        let after_what = std::cmp::max(prev_end_line, last_emitted_comment_line_excluding_current(ctx));
        if after_what > 0 && has_blank_between(ctx.trivia, after_what, c.line) {
            ctx.newline();
        }
        emit_single_comment(ctx, c);
        ctx.comment_cursor += 1;
    }
}

fn emit_single_comment(ctx: &mut FmtCtx, c: &Comment) {
    ctx.write_indent();
    match c.kind {
        CommentKind::Line => {
            // Normalizar `//foo` → `// foo` (espacio post-`//`).
            ctx.write("//");
            let trimmed = c.text.trim_start();
            if !trimmed.is_empty() {
                ctx.write(" ");
                ctx.write(trimmed);
            }
        }
        CommentKind::Block => {
            // Mantenemos el `/* ... */` igual; el contenido raw.
            ctx.write("/*");
            ctx.write(&c.text);
            ctx.write("*/");
        }
    }
    ctx.newline();
}

fn peek_comment_at_line<'a>(ctx: &'a FmtCtx, line: usize) -> Option<&'a Comment> {
    let c = ctx.trivia.comments.get(ctx.comment_cursor)?;
    if c.line == line {
        Some(c)
    } else {
        None
    }
}

fn last_emitted_comment_line(ctx: &FmtCtx) -> usize {
    if ctx.comment_cursor == 0 {
        0
    } else {
        ctx.trivia.comments[ctx.comment_cursor - 1].line
    }
}

/// Igual que la anterior pero useful cuando estamos por DECIDIR si
/// emitir blank antes de un comment todavía no emitido — necesitamos
/// el "anterior" sin contar el actual.
fn last_emitted_comment_line_excluding_current(ctx: &FmtCtx) -> usize {
    last_emitted_comment_line(ctx)
}

fn has_blank_between(trivia: &Trivia, lower_exclusive: usize, upper_exclusive: usize) -> bool {
    trivia
        .blank_lines
        .iter()
        .any(|&bl| bl > lower_exclusive && bl < upper_exclusive)
}

/// Heurística top-level: si el stmt anterior o el actual es un
/// fn/type def, queremos blank entre ellos aunque el source no la
/// tuviera. Mejora legibilidad sin contradecir intención (el user
/// no puso blank pero no objeta que la haya).
fn needs_blank_line_before_smart(prev: Option<&Stmt>, curr: &Stmt) -> bool {
    let Some(prev) = prev else { return false; };
    is_complex_top_level(prev) || is_complex_top_level(curr)
}

fn is_complex_top_level(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::FnDef { .. } | Stmt::TypeDef { .. })
}

/// Devuelve el stmt anterior dentro de `stmts` al que tiene
/// `start_line == cur_start_line`. Linear pero stmts típicamente
/// son pocos por bloque.
fn prev_stmt_at(stmts: &[Stmt], cur_start_line: usize) -> Option<&Stmt> {
    let mut prev: Option<&Stmt> = None;
    for s in stmts {
        if s.span().line == cur_start_line {
            return prev;
        }
        prev = Some(s);
    }
    prev
}

/// Recursivamente computa la línea más alta de cualquier descendiente
/// del stmt. Necesario para detectar trailing comments (que viven en
/// `stmt.end_line`, no en `stmt.start_line`).
fn end_line_of_stmt(stmt: &Stmt) -> usize {
    let start = stmt.span().line;
    let nested = match stmt {
        Stmt::Assign { value, .. } => Some(end_line_of_expr(value)),
        Stmt::Return(e, _) => Some(end_line_of_expr(e)),
        Stmt::ReturnStatus { status, body, .. } => {
            let s = end_line_of_expr(status);
            body.as_ref().map(end_line_of_expr).map(|b| s.max(b)).or(Some(s))
        }
        Stmt::Expr(e, _) => Some(end_line_of_expr(e)),
        Stmt::FnDef { body, .. }
        | Stmt::While { body, .. }
        | Stmt::Loop { body, .. }
        | Stmt::For { body, .. } => body.iter().map(end_line_of_stmt).max(),
        Stmt::TypeDef { fields, .. } => fields
            .iter()
            .filter_map(|f| f.default.as_ref().map(end_line_of_expr))
            .max(),
        Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::Import { .. }
        | Stmt::FromImport { .. }
        | Stmt::Error(_) => None,
    };
    start.max(nested.unwrap_or(start))
}

fn end_line_of_expr(expr: &Expr) -> usize {
    let start = expr.span().line;
    let nested = match expr {
        Expr::BinOp { left, right, .. } => {
            Some(end_line_of_expr(left).max(end_line_of_expr(right)))
        }
        Expr::UnaryOp { operand, .. } => Some(end_line_of_expr(operand)),
        Expr::Call { callee, args, .. } => {
            let mut m = end_line_of_expr(callee);
            for a in args {
                m = m.max(end_line_of_expr(a));
            }
            Some(m)
        }
        Expr::FnExpr { body, .. } => body.iter().map(end_line_of_stmt).max(),
        Expr::Field { object, .. } | Expr::Index { object, .. } => {
            Some(end_line_of_expr(object))
        }
        Expr::List(items, _) => items.iter().map(end_line_of_expr).max(),
        Expr::Map(entries, _) => entries
            .iter()
            .flat_map(|(k, v)| [end_line_of_expr(k), end_line_of_expr(v)])
            .max(),
        Expr::Range { start: s, end: e, .. } => {
            Some(end_line_of_expr(s).max(end_line_of_expr(e)))
        }
        Expr::If { condition, then, else_, .. } => {
            let mut m = end_line_of_expr(condition);
            if let Some(s) = then.iter().map(end_line_of_stmt).max() {
                m = m.max(s);
            }
            if let Some(e) = else_.as_ref().and_then(|el| el.iter().map(end_line_of_stmt).max()) {
                m = m.max(e);
            }
            Some(m)
        }
        Expr::Match { value, arms, .. } => {
            let mut m = end_line_of_expr(value);
            for a in arms {
                m = m.max(end_line_of_expr(&a.body));
            }
            Some(m)
        }
        Expr::StructLit { fields, .. } => {
            fields.iter().map(|(_, e)| end_line_of_expr(e)).max()
        }
        Expr::Ok(inner, _) | Expr::Err(inner, _) | Expr::Try(inner, _) | Expr::Await(inner, _) => {
            Some(end_line_of_expr(inner))
        }
        Expr::StrInterp(parts, _) => parts
            .iter()
            .filter_map(|p| match p {
                StrPart::Expr(e) => Some(end_line_of_expr(e)),
                StrPart::Lit(_) => None,
            })
            .max(),
        Expr::Int(_, _) | Expr::Float(_, _) | Expr::Str(_, _) | Expr::Bool(_, _)
        | Expr::Null(_) | Expr::Ident(_, _) | Expr::Error(_) => None,
    };
    start.max(nested.unwrap_or(start))
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
        AssignTarget::Index { .. } => false, // `xs[i] = v` nunca lleva let
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
        AssignTarget::Index { object, index } => {
            fmt_expr(ctx, object);
            ctx.write("[");
            fmt_expr(ctx, index);
            ctx.write("]");
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
        fmt_stmt_list(ctx, body, /* in_block = */ true);
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
                UnaryOpKind::Not => ctx.write("not "),
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
/// formatean a String aparte y se concatenan. No threading de trivia
/// — comments adentro de expresiones inline son deuda futura.
fn expr_to_inline_string(expr: &Expr) -> String {
    let empty = Trivia::default();
    let mut ctx = FmtCtx::new("", &empty);
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
        BinOpKind::Mod => "%",
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
            // R.2.2 — guard opcional `if <cond>` entre pattern y `=>`.
            if let Some(guard) = &arm.guard {
                ctx.write(" if ");
                let inline = expr_to_inline_string(guard);
                ctx.write(&inline);
            }
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
        Pattern::Range { start, end, inclusive } => {
            ctx.write(&start.to_string());
            ctx.write(if *inclusive { "..=" } else { ".." });
            ctx.write(&end.to_string());
        }
        Pattern::Or(subs) => {
            // R.2.1 — or-pattern. Emitimos `p1 | p2 | p3` con espacios
            // alrededor del separador. Cada sub-pattern se formatea
            // con la misma fn (no hay or-patterns anidados en el AST
            // real porque el parser aplana, pero esto los maneja
            // bien si llegan).
            for (i, sub) in subs.iter().enumerate() {
                if i > 0 {
                    ctx.write(" | ");
                }
                fmt_pattern(ctx, sub);
            }
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

    // ---- Fase 9.z.1.b — comment + blank line preservation ----

    #[test]
    fn preserva_comment_de_linea_antes_de_stmt() {
        check(
            "// header\nlet x = 1\n",
            "// header\nlet x = 1\n",
        );
    }

    #[test]
    fn preserva_multiple_comments_seguidos() {
        check(
            "// uno\n// dos\nlet x = 1\n",
            "// uno\n// dos\nlet x = 1\n",
        );
    }

    #[test]
    fn preserva_blank_line_entre_stmts() {
        check(
            "let x = 1\n\nlet y = 2\n",
            "let x = 1\n\nlet y = 2\n",
        );
    }

    #[test]
    fn preserva_comment_trailing_en_misma_linea() {
        check(
            "let x = 1 // explicación\n",
            "let x = 1  // explicación\n",
        );
    }

    #[test]
    fn normaliza_comment_sin_espacio_post_slash() {
        // `//foo` se normaliza a `// foo`.
        check(
            "//foo\nlet x = 1\n",
            "// foo\nlet x = 1\n",
        );
    }

    #[test]
    fn preserva_comment_entre_stmts_con_blank() {
        check(
            "let x = 1\n\n// separador\nlet y = 2\n",
            "let x = 1\n\n// separador\nlet y = 2\n",
        );
    }

    #[test]
    fn comments_adentro_de_fn_body_se_preservan() {
        check(
            "fn f() {\n    // primera\n    let x = 1\n    // segunda\n    return x\n}\n",
            "fn f() {\n    // primera\n    let x = 1\n    // segunda\n    return x\n}\n",
        );
    }

    #[test]
    fn idempotente_con_comments_y_blanks() {
        check_idempotent(
            r#"// header del archivo
// segunda línea de header

let x = 10  // var importante

// separador
fn double(n: Int) -> Int {
    // doc del cuerpo
    return n * 2
}

// otro separador
fn main() {
    print(double(x))
}
"#,
        );
    }

    #[test]
    fn preserva_smoke_de_02_hola_de_la_guia() {
        // Replica el contenido de examples/guide/02-hola.fitz.
        // El smoke a mano confirmó que el round-trip preserva todo.
        let original = "// 02-hola.fitz — El primer programa de la guía.\n\
                       // Muestra: print, asignación sin tipo, interpolación de strings.\n\
                       \n\
                       print(\"Hola desde Fitz 🏔️\")\n\
                       \n\
                       name = \"Patagonia\"\n\
                       print(\"Hola, {name}!\")\n";
        check(original, original);
    }

    #[test]
    fn multiples_blanks_consecutivas_se_colapsan_a_una() {
        // El user podría tener 3 blanks; el formatter colapsa a 1.
        check(
            "let x = 1\n\n\n\nlet y = 2\n",
            "let x = 1\n\nlet y = 2\n",
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
