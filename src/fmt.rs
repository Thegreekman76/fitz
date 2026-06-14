//! Fitz formatter (`fitz fmt`) — Phase 9.z.1.a.
//!
//! Hand-written pretty-printer over the AST. Zero config —
//! fixed conventions (4-space indent, double quotes, trailing
//! comma only multi-line, max 100 chars soft).
//!
//! Flow: `format_source(src)` → tokenize → parse → walk the AST →
//! string. The caller decides what to do with the string (write or compare).
//!
//! ## ⚠ CRITICAL LIMITATION OF 9.z.1.a — comments + blank lines get erased
//!
//! The lexer strips comments before reaching the AST, and the
//! formatter does not preserve user blank lines. Therefore, when
//! rewriting a file, **comments (`//`) and all of the author's
//! intentional blank lines get lost**.
//!
//! This is **missing table-stakes** for a production-grade
//! formatter (gofmt, prettier, black all preserve). It closes
//! in **9.z.1.b**:
//!
//! - lexer emits comments as tokens (side stream)
//! - parser builds a side-table `Vec<(SpanKey, Comment)>` adjacent
//!   to the AST
//! - formatter threads the comments back into the output according
//!   to original position
//!
//! While 9.z.1.b is not landed, `fitz fmt` (write mode) emits a
//! loud warning to the user about the loss. The `--check`
//! (read-only) mode does not need a warning — it does not break anything.
//!
//! ## Other debts (do not block the MVP)
//!
//! - **`is_let` lost in the AST**: the parser produces the same
//!   `Stmt::Assign` for `let x = 1` and `x = 1` (reassignment). The
//!   formatter inspects the source line via `Span` to detect
//!   `let` and preserve it. Refactoring the AST (adding
//!   `is_let: bool`) is minor debt; the current hack is isolated
//!   in `stmt_has_let_keyword`.
//! - **Unhandled nodes** fall back to `// <invalid>`. In the
//!   MVP we cover the AST nodes that appear in >90% of guide
//!   code; rare ones get completed iteratively.
//! - **Auto-wrap of lines > 100 chars**: NOT implemented. The
//!   formatter does not break long lines. Auto-wrap requires
//!   sensible break-point analysis — future debt if pressure appears.

use crate::ast::{
    AssignTarget, BinOpKind, Decorator, Expr, MatchArm, Param, Pattern, Span, Stmt, StrPart,
    TypeExpr, UnaryOpKind,
};
use crate::error::FitzError;
use crate::lexer::{tokenize_with_trivia, Comment, CommentKind, Trivia};
use crate::parser::parse;

/// Formatter style — zero config in 9.z.1, but centralized
/// here in case `fitz.toml [fmt]` is introduced in the future.
struct Style;
impl Style {
    const INDENT: &'static str = "    "; // 4 spaces
}

/// Formatter state during the AST walk.
struct FmtCtx<'a> {
    indent_level: usize,
    output: String,
    /// Original source — used to detect the `let` keyword in
    /// `Stmt::Assign` and to fall back for unhandled nodes via Span.
    source: &'a str,
    /// Phase 9.z.1.b — trivia captured by the lexer (comments +
    /// blank lines). Empty when called from `format_source_only_ast`
    /// (internal test path that does not need trivia threading).
    trivia: &'a Trivia,
    /// Cursor over `trivia.comments` to emit them in order of
    /// appearance without re-scanning.
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

/// Public entry point. Parses the source and produces its
/// formatted form. If the source has syntax errors, it fails — the
/// formatter does not try to fix code that the parser does not understand.
///
/// Phase 9.z.1.b: uses `tokenize_with_trivia` to capture
/// comments and blank lines from the source, and re-emits them at
/// their original positions in the output.
///
/// Idempotence: `format_source(format_source(x)) == format_source(x)`
/// for any valid code (tested in unit tests).
pub fn format_source(source: &str) -> Result<String, FitzError> {
    let (tokens, trivia) = tokenize_with_trivia(source)?;
    let program = parse(tokens)?;
    let mut ctx = FmtCtx::new(source, &trivia);
    format_program(&mut ctx, &program);
    Ok(ctx.output)
}

// ---- Program + statements ----

fn format_program(ctx: &mut FmtCtx, program: &[Stmt]) {
    fmt_stmt_list(ctx, program, /* in_block = */ false);
}

/// Renders a list of top-level stmts or stmts inside a block,
/// threading the comments + blank lines of `ctx.trivia` at their
/// original source positions.
///
/// Algorithm:
///
/// 1. For each stmt in order:
///    - (a) Emit "leading" trivia: comments with
///      `line < stmt.start_line` that have not yet been emitted.
///    - (b) If between the previous stmt (or the previous comment)
///      and this one there is a preserved blank line, emit blank.
///    - (c) Render the stmt.
///    - (d) Emit "trailing" trivia: comment on the same line as
///      the stmt's end (side-by-side, with 2 spaces).
/// 2. After the last stmt: emit any remaining comment/blank
///    (post-last).
fn fmt_stmt_list(ctx: &mut FmtCtx, stmts: &[Stmt], in_block: bool) {
    // `prev_end_line` tracks the upper bound of "already emitted" to
    // decide whether a blank line was preserved in the original.
    // At top-level it starts at 0; inside a block, at the block's
    // opening line (mid-stream).
    let mut prev_end_line: usize = 0;

    for stmt in stmts {
        let stmt_start = stmt.span().line;
        let stmt_end = end_line_of_stmt(stmt);

        // 1a. Leading comments (any comment with line < stmt_start
        // that has not yet been emitted). This includes file-header
        // comments before the first stmt.
        emit_leading_comments(ctx, prev_end_line, stmt_start);

        // 1b. Blank line in the gap between prev and stmt — two sources:
        //  (a) Preserved from the original: trivia.blank_lines contains
        //      a line in (after_what, stmt_start).
        //  (b) Top-level smart heuristic: between two stmts where at
        //      least one is fn/type, we insert blank (improves
        //      readability). BUT suppressed if we just emitted
        //      a leading comment for the current stmt — comments
        //      "stick to" the following stmt and we do not want to
        //      separate them.
        //
        // Skip entirely if this is the first stmt of the file/block.
        //
        // **Bug fix (post-9.z.5)**: when we are `in_block=true` and
        // this is the first stmt in the block (`prev_end_line == 0`), do NOT
        // check blanks. Without this guard, `last_emitted_comment_line`
        // may carry a value from the outer scope (e.g. a trailing
        // comment from the stmt before the block) and `has_blank_between`
        // may report blanks that are OUTSIDE the current block,
        // inserting a spurious blank inside.
        //
        // But at top-level with leading comments (file header),
        // `prev_end_line == 0` and we do want to preserve the blank between
        // the comments and the first stmt. The condition distinguishes:
        //   - In block: only if there was a previous stmt (prev_end_line > 0).
        //   - Top-level: if there was a previous stmt OR if there were
        //     leading comments (last_emitted_comment_line > 0) — the file
        //     header can be followed by a blank before the first stmt.
        let after_what = std::cmp::max(prev_end_line, last_emitted_comment_line(ctx));
        let block_allows_blank = if in_block {
            prev_end_line > 0
        } else {
            after_what > 0
        };
        let had_blank_in_source =
            block_allows_blank && has_blank_between(ctx.trivia, after_what, stmt_start);
        // If the last emitted comment belongs to "the gap" between prev_end_line
        // and stmt_start, suppress smart_blank (the comment already plays the role
        // of visual separator + we want the comment to stay attached to the stmt).
        let leading_comment_just_emitted = last_emitted_comment_line(ctx) > prev_end_line;
        let smart_blank = !in_block
            && prev_end_line > 0
            && !leading_comment_just_emitted
            && needs_blank_line_before_smart(prev_stmt_at(stmts, stmt_start), stmt);
        if had_blank_in_source || smart_blank {
            ctx.newline();
        }

        // 1c. Render the stmt.
        fmt_stmt(ctx, stmt);

        // 1d. Trailing comment on the same line as the stmt's end.
        let trailing_text = peek_comment_at_line(ctx, stmt_end).map(|c| c.text.clone());
        if let Some(text) = trailing_text {
            ctx.write("  // ");
            ctx.write(text.trim_start());
            ctx.comment_cursor += 1;
        }

        ctx.newline();
        prev_end_line = stmt_end;
    }

    // Post-last-stmt: only emit footer comments if we are at
    // top-level. Inside a block, the remaining comments stay in
    // the cursor — they get processed by the outer caller when it
    // emits leading comments for the next outer stmt.
    //
    // Minor debt: comments INSIDE a block, after the last stmt
    // but before the `}` (e.g. `fn f() { x = 1; // trailing\n }`),
    // end up leaving the block in the formatted output. Rare in
    // practice — documented.
    if !in_block {
        emit_trailing_comments(ctx, prev_end_line);
    }
}

/// Emits all comments with `line < upper_bound` that have not yet
/// been emitted. Puts them on their own lines with the current
/// indent. Trailing comments (those on a stmt's line) are handled
/// by the caller separately.
fn emit_leading_comments(ctx: &mut FmtCtx, prev_end_line: usize, upper_bound: usize) {
    while ctx.comment_cursor < ctx.trivia.comments.len() {
        let c = &ctx.trivia.comments[ctx.comment_cursor];
        if c.line >= upper_bound {
            break;
        }
        // Blank line before the comment if in the original there was one
        // between the last emitted item and this comment.
        let after_what = std::cmp::max(
            prev_end_line,
            last_emitted_comment_line_excluding_current(ctx),
        );
        if after_what > 0 && has_blank_between(ctx.trivia, after_what, c.line) {
            ctx.newline();
        }
        emit_single_comment(ctx, c);
        ctx.comment_cursor += 1;
    }
}

/// Emits remaining comments (any line >= prev_end_line +
/// that has not yet been emitted). Useful for footer comments
/// post-last-stmt.
fn emit_trailing_comments(ctx: &mut FmtCtx, prev_end_line: usize) {
    while ctx.comment_cursor < ctx.trivia.comments.len() {
        let c = &ctx.trivia.comments[ctx.comment_cursor];
        let after_what = std::cmp::max(
            prev_end_line,
            last_emitted_comment_line_excluding_current(ctx),
        );
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
            // Normalize `//foo` → `// foo` (space after `//`).
            ctx.write("//");
            let trimmed = c.text.trim_start();
            if !trimmed.is_empty() {
                ctx.write(" ");
                ctx.write(trimmed);
            }
        }
        CommentKind::Block => {
            // Keep `/* ... */` as-is; content raw.
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

/// Same as the previous one but useful when we are about to DECIDE
/// whether to emit a blank before a not-yet-emitted comment — we need
/// the "previous" one without counting the current.
fn last_emitted_comment_line_excluding_current(ctx: &FmtCtx) -> usize {
    last_emitted_comment_line(ctx)
}

fn has_blank_between(trivia: &Trivia, lower_exclusive: usize, upper_exclusive: usize) -> bool {
    trivia
        .blank_lines
        .iter()
        .any(|&bl| bl > lower_exclusive && bl < upper_exclusive)
}

/// Top-level heuristic: if the previous or current stmt is a
/// fn/type def, we want a blank between them even if the source
/// did not have one. Improves readability without contradicting
/// intent (the user did not put a blank but does not object to having one).
fn needs_blank_line_before_smart(prev: Option<&Stmt>, curr: &Stmt) -> bool {
    let Some(prev) = prev else {
        return false;
    };
    is_complex_top_level(prev) || is_complex_top_level(curr)
}

fn is_complex_top_level(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::FnDef { .. } | Stmt::TypeDef { .. })
}

/// Returns the stmt before the one in `stmts` whose
/// `start_line == cur_start_line`. Linear, but stmts are usually
/// few per block.
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

/// Recursively computes the highest line of any descendant of
/// the stmt. Needed to detect trailing comments (which live at
/// `stmt.end_line`, not at `stmt.start_line`).
fn end_line_of_stmt(stmt: &Stmt) -> usize {
    let start = stmt.span().line;
    let nested = match stmt {
        Stmt::Assign { value, .. } => Some(end_line_of_expr(value)),
        Stmt::Destructure { value, .. } => Some(end_line_of_expr(value)),
        Stmt::Return(e, _) => Some(end_line_of_expr(e)),
        Stmt::ReturnStatus { status, body, .. } => {
            let s = end_line_of_expr(status);
            body.as_ref()
                .map(end_line_of_expr)
                .map(|b| s.max(b))
                .or(Some(s))
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
        Stmt::Break(_, _, _)
        | Stmt::Continue(_, _)
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
        Expr::Field { object, .. } | Expr::Index { object, .. } => Some(end_line_of_expr(object)),
        Expr::TupleField { tuple, .. } => Some(end_line_of_expr(tuple)),
        Expr::Tuple(items, _) => items.iter().map(end_line_of_expr).max(),
        Expr::Loop { body, .. } => body.iter().map(end_line_of_stmt).max(),
        Expr::Slice {
            object, start, end, ..
        } => {
            let mut m = end_line_of_expr(object);
            if let Some(s) = start {
                m = m.max(end_line_of_expr(s));
            }
            if let Some(e) = end {
                m = m.max(end_line_of_expr(e));
            }
            Some(m)
        }
        Expr::List(items, _) => items.iter().map(end_line_of_expr).max(),
        Expr::ListComp {
            expr,
            iter,
            extra_clauses,
            filter,
            ..
        } => {
            let mut m = end_line_of_expr(expr).max(end_line_of_expr(iter));
            for (_, it) in extra_clauses {
                m = m.max(end_line_of_expr(it));
            }
            if let Some(f) = filter {
                m = m.max(end_line_of_expr(f));
            }
            Some(m)
        }
        Expr::MapComp {
            key,
            value,
            iter,
            extra_clauses,
            filter,
            ..
        } => {
            let mut m = end_line_of_expr(key)
                .max(end_line_of_expr(value))
                .max(end_line_of_expr(iter));
            for (_, it) in extra_clauses {
                m = m.max(end_line_of_expr(it));
            }
            if let Some(f) = filter {
                m = m.max(end_line_of_expr(f));
            }
            Some(m)
        }
        Expr::Map(entries, _) => entries
            .iter()
            .flat_map(|(k, v)| [end_line_of_expr(k), end_line_of_expr(v)])
            .max(),
        Expr::Range {
            start: s, end: e, ..
        } => Some(end_line_of_expr(s).max(end_line_of_expr(e))),
        Expr::If {
            condition,
            then,
            else_,
            ..
        } => {
            let mut m = end_line_of_expr(condition);
            if let Some(s) = then.iter().map(end_line_of_stmt).max() {
                m = m.max(s);
            }
            if let Some(e) = else_
                .as_ref()
                .and_then(|el| el.iter().map(end_line_of_stmt).max())
            {
                m = m.max(e);
            }
            Some(m)
        }
        Expr::Match { value, arms, .. } => {
            let mut m = end_line_of_expr(value);
            for a in arms {
                for s in &a.body {
                    m = m.max(end_line_of_stmt(s));
                }
            }
            Some(m)
        }
        Expr::StructLit { fields, .. } => fields.iter().map(|(_, e)| end_line_of_expr(e)).max(),
        Expr::Ok(inner, _) | Expr::Err(inner, _) | Expr::Try(inner, _) | Expr::Await(inner, _) => {
            Some(end_line_of_expr(inner))
        }
        // Fp.3 — NamedArg passthrough to the value.
        Expr::NamedArg { value, .. } => Some(end_line_of_expr(value)),
        Expr::StrInterp(parts, _) => parts
            .iter()
            .filter_map(|p| match p {
                StrPart::Expr(e, _) => Some(end_line_of_expr(e)),
                StrPart::Lit(_) => None,
            })
            .max(),
        Expr::Int(_, _)
        | Expr::Float(_, _)
        | Expr::Str(_, _)
        | Expr::Bool(_, _)
        | Expr::Null(_)
        | Expr::Bytes(_, _)
        | Expr::Ident(_, _)
        | Expr::Error(_) => None,
    };
    start.max(nested.unwrap_or(start))
}

fn fmt_stmt(ctx: &mut FmtCtx, stmt: &Stmt) {
    ctx.write_indent();
    match stmt {
        Stmt::Assign {
            target,
            type_,
            value,
            span,
        } => {
            fmt_assign(ctx, target, type_.as_ref(), value, *span);
        }
        Stmt::Destructure { pattern, value, .. } => {
            ctx.write("let ");
            fmt_pattern(ctx, pattern);
            ctx.write(" = ");
            fmt_expr(ctx, value);
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
            name,
            params,
            return_type,
            body,
            is_async,
            decorators,
            ..
        } => {
            fmt_fndef(
                ctx,
                name,
                params,
                return_type.as_ref(),
                body,
                *is_async,
                decorators,
            );
        }
        Stmt::TypeDef {
            name,
            fields,
            methods,
            ..
        } => {
            fmt_typedef(ctx, name, fields, methods);
        }
        Stmt::Break(value, label, _) => {
            ctx.write("break");
            if let Some(l) = label {
                ctx.write(" '");
                ctx.write(l);
            }
            if let Some(e) = value {
                ctx.write(" ");
                ctx.write(&expr_to_inline_string(e));
            }
        }
        Stmt::Continue(label, _) => {
            ctx.write("continue");
            if let Some(l) = label {
                ctx.write(" '");
                ctx.write(l);
            }
        }
        Stmt::While {
            condition, body, ..
        } => {
            ctx.write("while (");
            fmt_expr(ctx, condition);
            ctx.write(") ");
            fmt_block(ctx, body);
        }
        Stmt::Loop { body, .. } => {
            ctx.write("loop ");
            fmt_block(ctx, body);
        }
        Stmt::For {
            var, iter, body, ..
        } => {
            ctx.write("for ");
            // Md mini-batch: var is a Pattern (can be Ident, Wildcard,
            // Tuple). `fmt_pattern` already covers the 3 cases.
            fmt_pattern(ctx, var);
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
            // The strict parser does not produce this; defensive fallback.
            ctx.write("// <invalid stmt>");
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
    // The AST does not preserve whether the `let` keyword was there.
    // We recover it from the source — debt documented in the module header.
    let has_let = match target {
        AssignTarget::Ident(_, _) => stmt_has_let_keyword(ctx.source, span),
        AssignTarget::Field { .. } => false, // `obj.f = v` never carries let
        AssignTarget::Index { .. } => false, // `xs[i] = v` never carries let
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
        AssignTarget::Ident(n, _) => ctx.write(n),
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

/// Inspects the source line at `span` to see if the stmt
/// starts with `let`. Spans are 1-based. Contained hack to
/// supply info that the AST does not preserve (see module header).
fn stmt_has_let_keyword(source: &str, span: Span) -> bool {
    if !span.is_known() {
        // Synthetic stmts (arrow-fn body, tests building the
        // AST by hand) — we prefer NOT to emit `let` for consistency
        // with code that does not typically carry it.
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
    // Decorators one per line, in order.
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

fn fmt_typedef(
    ctx: &mut FmtCtx,
    name: &str,
    fields: &[crate::ast::Field],
    methods: &[crate::ast::MethodDef],
) {
    ctx.write("type ");
    ctx.write(name);
    ctx.write(" {");
    if fields.is_empty() && methods.is_empty() {
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
        // R.3 — custom methods. Blank line between consecutive items:
        // between fields and the first method, and between successive methods.
        for (i, m) in methods.iter().enumerate() {
            let needs_blank = i > 0 || !fields.is_empty();
            if needs_blank {
                ctx.newline();
            }
            fmt_method_def(ctx, m);
        }
    });
    ctx.write_indent();
    ctx.write("}");
}

fn fmt_method_def(ctx: &mut FmtCtx, m: &crate::ast::MethodDef) {
    ctx.write_indent();
    if m.is_async {
        ctx.write("async ");
    }
    ctx.write("fn ");
    ctx.write(&m.name);
    ctx.write("(");
    let param_strs: Vec<String> = m.params.iter().map(fmt_param_to_string).collect();
    ctx.write(&param_strs.join(", "));
    ctx.write(")");
    if let Some(rt) = &m.return_type {
        ctx.write(" -> ");
        ctx.write(&rt.display_name());
    }
    ctx.write(" ");
    fmt_block(ctx, &m.body);
    ctx.newline();
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
        // Fp.3 — NamedArg only valid in Call.args. Call's fmt already
        // handles the `name: value` case before reaching here. If a
        // loose NamedArg lands here, we emit the syntax as fallback.
        Expr::NamedArg { name, value, .. } => {
            ctx.write(name);
            ctx.write(": ");
            fmt_expr(ctx, value);
        }
        Expr::Int(n, _) => ctx.write(&n.to_string()),
        Expr::Float(f, _) => ctx.write(&format_float_literal(*f)),
        Expr::Str(s, _) => ctx.write(&format_str_literal(s)),
        Expr::Bool(b, _) => ctx.write(if *b { "true" } else { "false" }),
        Expr::Null(_) => ctx.write("null"),
        Expr::Bytes(bs, _) => {
            // Bytes mini-batch — `b"..."` format parallel to the Display
            // of Value::Bytes. ASCII printable + common escapes; the
            // rest goes as `\xHH`.
            ctx.write("b\"");
            for &b in bs.iter() {
                match b {
                    b'\\' => ctx.write("\\\\"),
                    b'"' => ctx.write("\\\""),
                    b'\n' => ctx.write("\\n"),
                    b'\r' => ctx.write("\\r"),
                    b'\t' => ctx.write("\\t"),
                    0x20..=0x7e => ctx.write(&(b as char).to_string()),
                    _ => ctx.write(&format!("\\x{:02x}", b)),
                }
            }
            ctx.write("\"");
        }
        Expr::Ident(name, _) => ctx.write(name),
        Expr::StrInterp(parts, _) => fmt_str_interp(ctx, parts),
        Expr::BinOp {
            op, left, right, ..
        } => fmt_binop(ctx, op, left, right),
        Expr::UnaryOp { op, operand, .. } => {
            match op {
                UnaryOpKind::Neg => ctx.write("-"),
                UnaryOpKind::Not => ctx.write("not "),
                UnaryOpKind::BitNot => ctx.write("~"),
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
        Expr::Slice {
            object,
            start,
            end,
            inclusive,
            ..
        } => {
            fmt_expr(ctx, object);
            ctx.write("[");
            if let Some(s) = start {
                ctx.write(&expr_to_inline_string(s));
            }
            ctx.write(if *inclusive { "..=" } else { ".." });
            if let Some(e) = end {
                ctx.write(&expr_to_inline_string(e));
            }
            ctx.write("]");
        }
        Expr::Tuple(items, _) => {
            ctx.write("(");
            let parts: Vec<String> = items.iter().map(expr_to_inline_string).collect();
            ctx.write(&parts.join(", "));
            if items.len() == 1 {
                ctx.write(",");
            }
            ctx.write(")");
        }
        Expr::TupleField { tuple, index, .. } => {
            fmt_expr(ctx, tuple);
            ctx.write(".");
            ctx.write(&index.to_string());
        }
        Expr::Loop { body, .. } => {
            ctx.write("loop ");
            fmt_block(ctx, body);
        }
        Expr::List(items, _) => {
            ctx.write("[");
            let parts: Vec<String> = items.iter().map(expr_to_inline_string).collect();
            ctx.write(&parts.join(", "));
            ctx.write("]");
        }
        // C + Cmp+ mini-batches — `[expr for var in iter ([for ...]*) (if filter)?]`.
        // One line with canonical spacing. Multi-line stays as
        // residual debt if demand appears.
        Expr::ListComp {
            expr,
            var,
            iter,
            extra_clauses,
            filter,
            ..
        } => {
            ctx.write("[");
            ctx.write(&expr_to_inline_string(expr));
            ctx.write(" for ");
            fmt_pattern(ctx, var);
            ctx.write(" in ");
            ctx.write(&expr_to_inline_string(iter));
            for (extra_var, extra_iter) in extra_clauses {
                ctx.write(" for ");
                fmt_pattern(ctx, extra_var);
                ctx.write(" in ");
                ctx.write(&expr_to_inline_string(extra_iter));
            }
            if let Some(f) = filter {
                ctx.write(" if ");
                ctx.write(&expr_to_inline_string(f));
            }
            ctx.write("]");
        }
        // Cmp+ mini-batch — `{key: value for var in iter (for ...)* (if cond)?}`.
        Expr::MapComp {
            key,
            value,
            var,
            iter,
            extra_clauses,
            filter,
            ..
        } => {
            ctx.write("{");
            ctx.write(&expr_to_inline_string(key));
            ctx.write(": ");
            ctx.write(&expr_to_inline_string(value));
            ctx.write(" for ");
            fmt_pattern(ctx, var);
            ctx.write(" in ");
            ctx.write(&expr_to_inline_string(iter));
            for (extra_var, extra_iter) in extra_clauses {
                ctx.write(" for ");
                fmt_pattern(ctx, extra_var);
                ctx.write(" in ");
                ctx.write(&expr_to_inline_string(extra_iter));
            }
            if let Some(f) = filter {
                ctx.write(" if ");
                ctx.write(&expr_to_inline_string(f));
            }
            ctx.write("}");
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
        Expr::If {
            condition,
            then,
            else_,
            ..
        } => {
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
        Expr::StructLit {
            type_name, fields, ..
        } => {
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
        Expr::Error(_) => ctx.write("/* <invalid expr> */"),
    }
}

/// To avoid regenerating the FmtCtx, expressions that appear
/// inside a single line (fn args, list items, etc.) are
/// formatted into a separate String and concatenated. No trivia
/// threading — comments inside inline expressions are future debt.
fn expr_to_inline_string(expr: &Expr) -> String {
    let empty = Trivia::default();
    let mut ctx = FmtCtx::new("", &empty);
    fmt_expr(&mut ctx, expr);
    ctx.output
}

fn fmt_expr_with_parens_if_needed(ctx: &mut FmtCtx, expr: &Expr) {
    // Minimal heuristic: UnaryOp over BinOp needs parens.
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
        BinOpKind::Xor => "xor",
        // Bits mini-batch — bitwise operators.
        BinOpKind::BitAnd => "&",
        BinOpKind::BitOr => "|",
        BinOpKind::BitXor => "^",
        BinOpKind::Shl => "<<",
        BinOpKind::Shr => ">>",
    }
}

fn fmt_str_interp(ctx: &mut FmtCtx, parts: &[StrPart]) {
    ctx.write("\"");
    for part in parts {
        match part {
            StrPart::Lit(s) => {
                // We only escape `"` and `\`; the rest passes raw to
                // preserve emojis, unicode, etc.
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
            StrPart::Expr(e, spec) => {
                ctx.write("{");
                let inline = expr_to_inline_string(e);
                ctx.write(&inline);
                // Fm mini-batch — re-emit the spec if present.
                // `FormatSpec::to_source()` reconstructs the canonical
                // `[fill]align[sign]#0width,prec_type` syntax.
                if let Some(s) = spec {
                    ctx.write(":");
                    ctx.write(&s.to_source());
                }
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
            // R.2.2 — optional `if <cond>` guard between pattern and `=>`.
            if let Some(guard) = &arm.guard {
                ctx.write(" if ");
                let inline = expr_to_inline_string(guard);
                ctx.write(&inline);
            }
            ctx.write(" => ");
            // Sp.2 — body is Vec<Stmt>. Typical case: 1 Stmt::Expr —
            // we emit inline as an expression. Block case (>1 stmt or
            // Stmt::Return/etc.): we emit as `{ ... }` with stmts
            // inside indented.
            if arm.body.len() == 1 {
                if let Stmt::Expr(e, _) = &arm.body[0] {
                    fmt_expr(ctx, e);
                } else {
                    // Stmt::Return/Break/Continue → emit as a bare stmt
                    // (no braces) to preserve the form.
                    fmt_stmt(ctx, &arm.body[0]);
                }
            } else {
                ctx.write("{");
                ctx.newline();
                ctx.with_indent(|ctx| {
                    for s in &arm.body {
                        ctx.write_indent();
                        fmt_stmt(ctx, s);
                        ctx.newline();
                    }
                });
                ctx.write_indent();
                ctx.write("}");
            }
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
        Pattern::Ident(name, _) => ctx.write(name),
        Pattern::Wildcard => ctx.write("_"),
        Pattern::OkBinding(n, _) => {
            ctx.write("Ok(");
            ctx.write(n);
            ctx.write(")");
        }
        Pattern::ErrBinding(n, _) => {
            ctx.write("Err(");
            ctx.write(n);
            ctx.write(")");
        }
        Pattern::OkWildcard => ctx.write("Ok(_)"),
        Pattern::ErrWildcard => ctx.write("Err(_)"),
        Pattern::Range {
            start,
            end,
            inclusive,
        } => {
            ctx.write(&start.to_string());
            ctx.write(if *inclusive { "..=" } else { ".." });
            ctx.write(&end.to_string());
        }
        Pattern::Or(subs) => {
            // R.2.1 — or-pattern. We emit `p1 | p2 | p3` with spaces
            // around the separator. Each sub-pattern is formatted
            // with the same fn (no nested or-patterns in the real
            // AST because the parser flattens them, but this handles
            // them well if they arrive).
            for (i, sub) in subs.iter().enumerate() {
                if i > 0 {
                    ctx.write(" | ");
                }
                fmt_pattern(ctx, sub);
            }
        }
        Pattern::Tuple(subs) => {
            ctx.write("(");
            for (i, sub) in subs.iter().enumerate() {
                if i > 0 {
                    ctx.write(", ");
                }
                fmt_pattern(ctx, sub);
            }
            if subs.len() == 1 {
                ctx.write(",");
            }
            ctx.write(")");
        }
    }
}

fn fmt_fnexpr(ctx: &mut FmtCtx, params: &[Param], body: &[Stmt]) {
    ctx.write("fn(");
    let param_strs: Vec<String> = params.iter().map(fmt_param_to_string).collect();
    ctx.write(&param_strs.join(", "));
    ctx.write(")");
    // If the body is a single return, use the arrow syntax.
    if let [Stmt::Return(expr, _)] = body {
        ctx.write(" => ");
        let inline = expr_to_inline_string(expr);
        ctx.write(&inline);
    } else {
        ctx.write(" ");
        fmt_block(ctx, body);
    }
}

// ---- Literal-format helpers ----

/// Float literal with at least one decimal (`1.0`, not `1`) to
/// visually distinguish from Int.
fn format_float_literal(f: f64) -> String {
    let s = format!("{f}");
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{s}.0")
    }
}

/// Str literal with minimal escaping (same as in StrInterp).
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

    /// Asserts that `format_source(input)` == `expected`.
    fn check(input: &str, expected: &str) {
        let actual = format_source(input).unwrap_or_else(|e| panic!("parse: {e}"));
        assert_eq!(
            actual, expected,
            "input:\n{input}\nactual:\n{actual}\nexpected:\n{expected}"
        );
    }

    /// Asserts that `format_source` is idempotent on `input`.
    fn check_idempotent(input: &str) {
        let once = format_source(input).unwrap_or_else(|e| panic!("parse(once): {e}"));
        let twice = format_source(&once).unwrap_or_else(|e| panic!("parse(twice): {e}"));
        assert_eq!(
            once, twice,
            "no idempotente:\noriginal:\n{input}\nonce:\n{once}\ntwice:\n{twice}"
        );
    }

    #[test]
    fn formats_simple_print() {
        check("print(\"hola\")\n", "print(\"hola\")\n");
    }

    #[test]
    fn formats_let_preserves_keyword() {
        check("let x = 1\n", "let x = 1\n");
    }

    #[test]
    fn formats_assign_without_let_preserves() {
        // If the source does not have `let`, the formatter does not emit it either.
        check("name = \"Patagonia\"\n", "name = \"Patagonia\"\n");
    }

    #[test]
    fn formats_let_with_type_annotation() {
        check("let x: Int = 5\n", "let x: Int = 5\n");
    }

    #[test]
    fn formats_simple_fn_def() {
        check(
            "fn double(n: Int) -> Int { return n * 2 }\n",
            "fn double(n: Int) -> Int {\n    return n * 2\n}\n",
        );
    }

    #[test]
    fn formats_fn_def_arrow_normalizes_to_block() {
        // The parser converts the arrow form (`=> expr`) to
        // `body: [Return(expr)]`. Since the AST does not preserve the
        // arrow form vs block, the formatter always emits a block.
        // Minor debt — AST refactor if it looks worthwhile.
        check(
            "fn add(a, b) => a + b\n",
            "fn add(a, b) {\n    return a + b\n}\n",
        );
    }

    #[test]
    fn formats_if_else_as_stmt() {
        check(
            "if (x > 0) { print(\"pos\") } else { print(\"non-pos\") }\n",
            "if (x > 0) {\n    print(\"pos\")\n} else {\n    print(\"non-pos\")\n}\n",
        );
    }

    #[test]
    fn formats_while_and_for() {
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
    fn formats_loop_with_break() {
        check("loop { break }\n", "loop {\n    break\n}\n");
    }

    #[test]
    fn formats_type_def_with_fields_and_defaults() {
        check(
            "type User { id: Int, name: Str = \"anon\", email: Str? }\n",
            "type User {\n    id: Int\n    name: Str = \"anon\"\n    email: Str?\n}\n",
        );
    }

    #[test]
    fn formats_struct_lit_inline() {
        check(
            "let u = User { id: 1, name: \"x\" }\n",
            "let u = User { id: 1, name: \"x\" }\n",
        );
    }

    #[test]
    fn formats_list_and_map_inline() {
        check("let xs = [1, 2, 3]\n", "let xs = [1, 2, 3]\n");
        check(
            "let m = {\"a\": 1, \"b\": 2}\n",
            "let m = {\"a\": 1, \"b\": 2}\n",
        );
    }

    #[test]
    fn formats_match_over_result() {
        check(
            "let r = match x { Ok(v) => v, Err(_) => 0, }\n",
            "let r = match x {\n    Ok(v) => v,\n    Err(_) => 0,\n}\n",
        );
    }

    #[test]
    fn formats_str_interp_with_var() {
        check("print(\"hola, {name}\")\n", "print(\"hola, {name}\")\n");
    }

    #[test]
    fn formats_stackable_decorator() {
        check(
            "@get(\"/users\") fn list() => 1\n",
            "@get(\"/users\")\nfn list() {\n    return 1\n}\n",
        );
    }

    #[test]
    fn formats_import_and_from_import_with_alias() {
        check("import utils\n", "import utils\n");
        check("import utils as u\n", "import utils as u\n");
        check(
            "from utils import a, b as bb\n",
            "from utils import a, b as bb\n",
        );
    }

    #[test]
    fn formats_async_and_await() {
        check(
            "async fn ping() -> Int { return sleep(0).await }\n",
            "async fn ping() -> Int {\n    return sleep(0).await\n}\n",
        );
    }

    #[test]
    fn formats_ok_err_try() {
        check(
            "fn f() -> Result<Int> { return Ok(g()?) }\n",
            "fn f() -> Result<Int> {\n    return Ok(g()?)\n}\n",
        );
    }

    #[test]
    fn formats_blank_line_between_fn_defs() {
        check(
            "fn a() => 1\nfn b() => 2\n",
            "fn a() {\n    return 1\n}\n\nfn b() {\n    return 2\n}\n",
        );
    }

    #[test]
    fn idempotent_over_complex_program() {
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

    // ---- Phase 9.z.1.b — comment + blank line preservation ----

    #[test]
    fn preserves_line_comment_before_stmt() {
        check("// header\nlet x = 1\n", "// header\nlet x = 1\n");
    }

    #[test]
    fn preserves_multiple_consecutive_comments() {
        check("// uno\n// dos\nlet x = 1\n", "// uno\n// dos\nlet x = 1\n");
    }

    #[test]
    fn preserves_blank_line_between_stmts() {
        check("let x = 1\n\nlet y = 2\n", "let x = 1\n\nlet y = 2\n");
    }

    #[test]
    fn preserves_trailing_comment_on_same_line() {
        check("let x = 1 // explicación\n", "let x = 1  // explicación\n");
    }

    #[test]
    fn normalizes_comment_without_space_after_slashes() {
        // `//foo` is normalized to `// foo`.
        check("//foo\nlet x = 1\n", "// foo\nlet x = 1\n");
    }

    #[test]
    fn preserves_comment_between_stmts_with_blank() {
        check(
            "let x = 1\n\n// separador\nlet y = 2\n",
            "let x = 1\n\n// separador\nlet y = 2\n",
        );
    }

    #[test]
    fn comments_inside_fn_body_are_preserved() {
        check(
            "fn f() {\n    // primera\n    let x = 1\n    // segunda\n    return x\n}\n",
            "fn f() {\n    // primera\n    let x = 1\n    // segunda\n    return x\n}\n",
        );
    }

    #[test]
    fn idempotent_with_comments_and_blanks() {
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
    fn preserves_smoke_of_02_hola_from_guide() {
        // Replicates the contents of examples/guide/02-hola.fitz.
        // The manual smoke confirmed that the round-trip preserves everything.
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
    fn multiple_consecutive_blanks_collapse_to_one() {
        // The user could have 3 blanks; the formatter collapses to 1.
        check("let x = 1\n\n\n\nlet y = 2\n", "let x = 1\n\nlet y = 2\n");
    }

    #[test]
    fn idempotent_over_match_and_result() {
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
