// parser.rs — Phase 2.3
//
// The parser turns the flat token list from the lexer into an AST.
// Implementation: recursive descent. Every grammar rule is a function;
// operator precedence is encoded in the call hierarchy (`equality`
// calls `comparison`, which calls `term`, etc.).
//
// Status: under construction. See docs/roadmap.md section 2.3 for the
// scope and explicit debt.

use crate::ast::{
    AssignTarget, BinOpKind, Decorator, Expr, Field, FormatSpec, MatchArm, MethodDef, Param,
    Pattern, Program, Span, Stmt, StrPart, TypeExpr, UnaryOpKind,
};
use crate::error::{ErrorKind, FitzError, FitzResult};
use crate::lexer::{tokenize, Token, TokenWithPos};

/// Parsing state. Module-private.
///
/// The parser consumes tokens left to right. `pos` points to the next
/// token to read. When we reach the end, `peek` returns `&Token::EOF`
/// (not `Option<&Token>`) — the lexer guarantees the last token is
/// always EOF, so we save `unwrap`s in every rule.
struct Parser {
    tokens: Vec<TokenWithPos>,
    pos: usize,
    /// When `true`, an `Ident` followed by `{` is NOT interpreted as a
    /// struct literal — the postfix is broken and the `{` is left for
    /// the caller (typically a control block: `if/while/for/match`).
    ///
    /// The flag is set when entering the condition of `if`/`while`,
    /// the iterable of `for`, and the scrutinee of `match`. It is
    /// cleared in delimited subexpressions (parentheses, call args,
    /// list/map/struct literal bodies, indexing), where there is no
    /// ambiguity with blocks.
    ///
    /// If, in blocked mode, a body that looks like a struct literal
    /// (`{ Ident : ...`) is seen, the parser bails with an explicit
    /// error suggesting wrapping it in parentheses.
    no_struct_literal: bool,

    /// Mini-batch I.2 — slicing. When `true`, `range_expr` does NOT
    /// consume the `..`/`..=` operator: it returns the start without
    /// promoting it to `Expr::Range`. The postfix `[` looks at this
    /// and builds the matching `Expr::Slice`.
    in_slice_context: bool,

    /// Phase 9.0.1 (F15): if `true`, the top-level stmt loops
    /// (`parse_program` + `parse_block`) catch errors from
    /// `parse_stmt`, accumulate them into `recovered_errors`,
    /// synchronize to the next stmt boundary (Newline/Semicolon/
    /// RBrace/EOF) and continue with a `Stmt::Error(span)` in place
    /// of the original stmt. Used by external tooling (LSP) that
    /// needs a partial AST over in-progress buffers. Strict `parse()`
    /// keeps it `false`; `parse_with_recovery()` turns it on.
    recovery_mode: bool,

    /// Errors accumulated during `parse_with_recovery`. In strict
    /// mode it stays empty. Cap: see `MAX_RECOVERED_ERRORS`.
    recovered_errors: Vec<FitzError>,
}

/// Hard cap on accumulated errors in `parse_with_recovery`. When
/// reached, the parser gives up: it discards the rest of the input
/// and returns what it has. Protects against runaway cascades on
/// large, very broken buffers. 100 covers the 90% case (~5-20 errors
/// in a real LSP buffer) with plenty of headroom.
const MAX_RECOVERED_ERRORS: usize = 100;

impl Parser {
    fn new(tokens: Vec<TokenWithPos>) -> Self {
        Self {
            tokens,
            pos: 0,
            no_struct_literal: false,
            in_slice_context: false,
            recovery_mode: false,
            recovered_errors: Vec::new(),
        }
    }

    // ---------- navigation ----------

    /// Current token without consuming.
    fn peek(&self) -> &Token {
        &self.tokens[self.pos].token
    }

    /// Token at `pos + n` without consuming. Handy for short
    /// lookahead. Returns `&Token::EOF` if we go past the end.
    fn peek_at(&self, n: usize) -> &Token {
        self.tokens
            .get(self.pos + n)
            .map(|t| &t.token)
            .unwrap_or(&Token::EOF)
    }

    /// `(line, column)` of the current token. Used to build errors.
    fn current_pos(&self) -> (usize, usize) {
        let t = &self.tokens[self.pos];
        (t.line, t.column)
    }

    /// `Span` of the current token. Shortcut for building `Expr` nodes
    /// with their position. Equivalent to
    /// `let (l, c) = self.current_pos(); Span::new(l, c)`.
    fn cur_span(&self) -> Span {
        let (line, column) = self.current_pos();
        Span::new(line, column)
    }

    /// `true` if we are sitting on the EOF token.
    fn is_at_end(&self) -> bool {
        matches!(self.peek(), Token::EOF)
    }

    /// Consume the current token and return it cloned. The cursor
    /// advances, unless we are already at EOF (we do not advance
    /// past the end, so `peek` is always valid).
    fn advance(&mut self) -> TokenWithPos {
        let tok = self.tokens[self.pos].clone();
        if !self.is_at_end() {
            self.pos += 1;
        }
        tok
    }

    // ---------- comparison / consumption ----------

    /// `true` if the current token matches `want`. Uses `Token`'s
    /// `PartialEq` implementation, which compares variant AND payload
    /// — works for payload-less tokens (`Plus`, `RParen`, ...). For
    /// `Ident(_)` or others with payload, use `matches!` directly on
    /// `peek()`.
    fn check(&self, want: &Token) -> bool {
        self.peek() == want
    }

    /// Consume the token if it matches `want`. Returns `true` on a
    /// match. Useful for optional tokens (e.g. trailing comma).
    fn eat(&mut self, want: &Token) -> bool {
        if self.check(want) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Consume the token if it matches `want`, or return an error
    /// with the given message and the current token's position.
    fn expect(&mut self, want: &Token, message: impl Into<String>) -> FitzResult<()> {
        if self.eat(want) {
            Ok(())
        } else {
            Err(self.error(ErrorKind::UnexpectedToken, message))
        }
    }

    /// If the current token is an `Ident`, consume it and return the
    /// name. Otherwise, return an error with the given message.
    fn expect_ident(&mut self, message: impl Into<String>) -> FitzResult<String> {
        // The `match` borrow ends when `name.clone()` runs, so
        // `self.advance()` (which requires `&mut self`) can run
        // afterwards without fighting the borrow checker.
        let name = match self.peek() {
            Token::Ident(name) => name.clone(),
            _ => return Err(self.error(ErrorKind::UnexpectedToken, message)),
        };
        self.advance();
        Ok(name)
    }

    /// V2 (2026-06-05) — variant of `expect_ident` that also returns
    /// the `Span` of the Ident token. Used while building
    /// `AssignTarget::Ident` so the checker can record the binding
    /// type under the LHS span and enable hover on the variable name.
    fn expect_ident_with_span(&mut self, message: impl Into<String>) -> FitzResult<(String, Span)> {
        let span = self.cur_span();
        let name = self.expect_ident(message)?;
        Ok((name, span))
    }

    /// Consume runs of `Newline`. Use before each element inside a
    /// list (args, fields, arms) and before each statement inside a
    /// block. Between tokens of an expression, newlines matter (they
    /// end the statement) — do NOT call there.
    fn skip_newlines(&mut self) {
        while matches!(self.peek(), Token::Newline) {
            self.advance();
        }
    }

    // ---------- error construction ----------

    /// Build a `FitzError` with the current token's position.
    /// Centralizing here gives us consistent errors across the parser.
    fn error(&self, kind: ErrorKind, message: impl Into<String>) -> FitzError {
        let (line, column) = self.current_pos();
        FitzError::new(kind, line, column, message)
    }

    // ---------- expressions: precedence ladder ----------
    //
    // From lowest to highest precedence:
    //   expression  → logic_or
    //   logic_or    → logic_and ( "or" logic_and )*
    //   logic_and   → equality  ( "and" equality )*
    //   equality    → comparison ( ("==" | "!=") comparison )*
    //   comparison  → range      ( ("<" | ">" | "<=" | ">=") range )*
    //   range       → term       ( ".." term )?     (not chainable)
    //   term        → factor     ( ("+" | "-") factor )*
    //   factor      → unary      ( ("*" | "/") unary )*
    //   unary       → "-" unary  |  postfix
    //   postfix     → primary    ( "." Ident  |  "(" args ")"  |  "[" expr "]" )*
    //   primary     → literal | Ident | "(" expression ")" | list | map
    //
    // Chainable binaries are left-associative: the `while` iterates,
    // nesting onto `left` each time. `range` is NOT chainable —
    // `1..2..3` is an error (caught by the caller if `peek_at` is
    // still `..`). `expression` is the entry point from any outer
    // rule that wants to parse a full expression.

    fn expression(&mut self) -> FitzResult<Expr> {
        self.logic_or()
    }

    /// Same as `expression()`, but with the `no_struct_literal` flag
    /// active: `Ident { ... }` will NOT be parsed as a struct literal
    /// inside this expression. Used in positions where the next `{`
    /// opens a control block (the condition of `if`/`while`, the
    /// iterable of `for`, the scrutinee of `match`).
    ///
    /// Delimited subexpressions inside the call (parens, args,
    /// indexing, literal bodies) restore the flag to `false` locally
    /// — so `if x == (User { id: 1 })` works without fighting the
    /// flag.
    fn expression_no_struct_lit(&mut self) -> FitzResult<Expr> {
        let prev = std::mem::replace(&mut self.no_struct_literal, true);
        let result = self.expression();
        self.no_struct_literal = prev;
        result
    }

    /// Heuristic to tell `Ident { ... }` as struct literal apart from
    /// `Ident` followed by a control block. Only used to emit an
    /// error with a hint when we are in `no_struct_literal` mode and
    /// the body unmistakably looks like a struct literal.
    ///
    /// Pre: `peek()` is `Token::LBrace`. Looks ahead skipping
    /// newlines and returns `true` if the body starts with
    /// `Ident :` — a struct-literal field pattern that, under normal
    /// circumstances, cannot start a block (`x: Int = 1` could, but
    /// it needs `Ident` after the `:`).
    fn looks_like_struct_lit_body(&self) -> bool {
        if !matches!(self.peek(), Token::LBrace) {
            return false;
        }
        // Skip newlines after the `{`.
        let mut i = 1;
        while matches!(self.peek_at(i), Token::Newline) {
            i += 1;
        }
        // Empty body `{ }` → treat as struct literal (an empty `{}`
        // in expression position inside a control block makes no
        // sense, so the hint is still useful).
        if matches!(self.peek_at(i), Token::RBrace) {
            return true;
        }
        // Must start with `Ident` followed by `:`. If after the `:`
        // there is `Ident =`, this is a typed block assignment, not
        // a struct literal — we let that case slip (we prefer a
        // clear error from the caller for that rare case).
        let p1 = self.peek_at(i);
        let p2 = self.peek_at(i + 1);
        if !matches!(p1, Token::Ident(_)) || !matches!(p2, Token::Colon) {
            return false;
        }
        // If after `Ident :` comes `Ident =`, it looks like a typed
        // assignment inside a block (`{ x: Int = 1 }`). In that case
        // it is not a struct literal and we don't add the hint.
        let after_colon = self.peek_at(i + 2);
        let after_after = self.peek_at(i + 3);
        if matches!(after_colon, Token::Ident(_)) && matches!(after_after, Token::Eq) {
            return false;
        }
        true
    }

    /// `a or b or c` — `or` and `xor` are left-associative and share
    /// precedence (lower than `and`, parallel to Python for `or`).
    /// This gives `a and b or c` = `(a and b) or c` and
    /// `a or b xor c` = `(a or b) xor c` (left-fold).
    ///
    /// Mini-batch Xor: `xor` was added at the same level so
    /// `a xor b xor c` chains naturally without parens.
    fn logic_or(&mut self) -> FitzResult<Expr> {
        let mut left = self.logic_and()?;
        loop {
            let op = match self.peek() {
                Token::Or => BinOpKind::Or,
                Token::Xor => BinOpKind::Xor,
                _ => break,
            };
            let span = self.cur_span();
            self.advance();
            let right = self.logic_and()?;
            left = Expr::BinOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    /// `a and b and c` — higher than `or`, lower than `==`. Result:
    /// `a == 1 and b == 2` parses as `(a == 1) and (b == 2)`.
    fn logic_and(&mut self) -> FitzResult<Expr> {
        let mut left = self.equality()?;
        while matches!(self.peek(), Token::And) {
            let span = self.cur_span();
            self.advance();
            let right = self.equality()?;
            left = Expr::BinOp {
                op: BinOpKind::And,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    fn equality(&mut self) -> FitzResult<Expr> {
        let mut left = self.comparison()?;
        while let Some((op, span)) = self.match_equality_op() {
            let right = self.comparison()?;
            left = Expr::BinOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    fn match_equality_op(&mut self) -> Option<(BinOpKind, Span)> {
        let op = match self.peek() {
            Token::EqEq => BinOpKind::Eq,
            Token::NotEq => BinOpKind::NotEq,
            _ => return None,
        };
        let span = self.cur_span();
        self.advance();
        Some((op, span))
    }

    fn comparison(&mut self) -> FitzResult<Expr> {
        let mut left = self.bitor_expr()?;
        while let Some((op, span)) = self.match_comparison_op() {
            let right = self.bitor_expr()?;
            left = Expr::BinOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    /// Mini-batch Bits — `|` bitwise OR. Lowest precedence among
    /// bitwise ops (parallel to Python/C): `|` < `^` < `&` < `<<`/`>>`.
    ///
    /// Watch out: `|` is also used as the or-pattern separator in
    /// match arms (R.2.1), but the match parser doesn't reach here
    /// — patterns are parsed via `parse_or_pattern`.
    fn bitor_expr(&mut self) -> FitzResult<Expr> {
        let mut left = self.bitxor_expr()?;
        while matches!(self.peek(), Token::Pipe) {
            let span = self.cur_span();
            self.advance();
            let right = self.bitxor_expr()?;
            left = Expr::BinOp {
                op: BinOpKind::BitOr,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    /// Mini-batch Bits — `^` bitwise XOR.
    fn bitxor_expr(&mut self) -> FitzResult<Expr> {
        let mut left = self.bitand_expr()?;
        while matches!(self.peek(), Token::Caret) {
            let span = self.cur_span();
            self.advance();
            let right = self.bitand_expr()?;
            left = Expr::BinOp {
                op: BinOpKind::BitXor,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    /// Mini-batch Bits — `&` bitwise AND.
    fn bitand_expr(&mut self) -> FitzResult<Expr> {
        let mut left = self.shift_expr()?;
        while matches!(self.peek(), Token::Amp) {
            let span = self.cur_span();
            self.advance();
            let right = self.shift_expr()?;
            left = Expr::BinOp {
                op: BinOpKind::BitAnd,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    /// Mini-batch Bits — `<<` and `>>`. Precedence between bitwise and range.
    fn shift_expr(&mut self) -> FitzResult<Expr> {
        let mut left = self.range_expr()?;
        loop {
            let op = match self.peek() {
                Token::Shl => BinOpKind::Shl,
                Token::Shr => BinOpKind::Shr,
                _ => break,
            };
            let span = self.cur_span();
            self.advance();
            let right = self.range_expr()?;
            left = Expr::BinOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    /// `start..end` (exclusive) or `start..=end` (inclusive, R.1.4).
    /// `span` points to the `..` or `..=`.
    fn range_expr(&mut self) -> FitzResult<Expr> {
        let start = self.term()?;
        // I.2 — in bracket context, do NOT consume `..`/`..=`: the
        // postfix `[` looks at this to build the `Expr::Slice`.
        if self.in_slice_context {
            return Ok(start);
        }
        let inclusive = match self.peek() {
            Token::DotDot => false,
            Token::DotDotEq => true,
            _ => return Ok(start),
        };
        let span = self.cur_span();
        self.advance(); // consume '..' or '..='
        let end = self.term()?;
        if matches!(self.peek(), Token::DotDot | Token::DotDotEq) {
            return Err(self.error(
                ErrorKind::InvalidSyntax,
                "ranges do not chain — use parentheses if you want a range of ranges",
            ));
        }
        Ok(Expr::Range {
            start: Box::new(start),
            end: Box::new(end),
            inclusive,
            span,
        })
    }

    fn match_comparison_op(&mut self) -> Option<(BinOpKind, Span)> {
        let op = match self.peek() {
            Token::Lt => BinOpKind::Lt,
            Token::LtEq => BinOpKind::LtEq,
            Token::Gt => BinOpKind::Gt,
            Token::GtEq => BinOpKind::GtEq,
            _ => return None,
        };
        let span = self.cur_span();
        self.advance();
        Some((op, span))
    }

    fn term(&mut self) -> FitzResult<Expr> {
        let mut left = self.factor()?;
        while let Some((op, span)) = self.match_term_op() {
            let right = self.factor()?;
            left = Expr::BinOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    fn match_term_op(&mut self) -> Option<(BinOpKind, Span)> {
        let op = match self.peek() {
            Token::Plus => BinOpKind::Add,
            Token::Minus => BinOpKind::Sub,
            _ => return None,
        };
        let span = self.cur_span();
        self.advance();
        Some((op, span))
    }

    fn factor(&mut self) -> FitzResult<Expr> {
        let mut left = self.unary()?;
        while let Some((op, span)) = self.match_factor_op() {
            let right = self.unary()?;
            left = Expr::BinOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    fn match_factor_op(&mut self) -> Option<(BinOpKind, Span)> {
        let op = match self.peek() {
            Token::Star => BinOpKind::Mul,
            Token::Slash => BinOpKind::Div,
            // R.1.2 — `%` has the same precedence as `*` and `/`.
            Token::Percent => BinOpKind::Mod,
            _ => return None,
        };
        let span = self.cur_span();
        self.advance();
        Some((op, span))
    }

    /// Unary prefix: `-x` (numeric negation) or `not x` (logical
    /// negation, R.1.1). `span` points to the operator. Both have
    /// the same precedence (higher than comparison, below postfix),
    /// so `not x == 1` would parse as `not (x == 1)` if that's what
    /// we wanted — but actual associativity is `(not x) == 1`. To
    /// avoid the ambiguity, **`not` has higher precedence than
    /// `==`/`!=`**: `not x == 1` parses as `(not x) == 1`. For the
    /// other order, use parens: `not (x == 1)`.
    fn unary(&mut self) -> FitzResult<Expr> {
        match self.peek() {
            Token::Minus => {
                let span = self.cur_span();
                self.advance();
                let operand = self.unary()?;
                Ok(Expr::UnaryOp {
                    op: UnaryOpKind::Neg,
                    operand: Box::new(operand),
                    span,
                })
            }
            Token::Not => {
                let span = self.cur_span();
                self.advance();
                let operand = self.unary()?;
                Ok(Expr::UnaryOp {
                    op: UnaryOpKind::Not,
                    operand: Box::new(operand),
                    span,
                })
            }
            // Mini-batch Bits — `~x` bitwise NOT (unary, Int only).
            // Same precedence as `-` and `not`.
            Token::Tilde => {
                let span = self.cur_span();
                self.advance();
                let operand = self.unary()?;
                Ok(Expr::UnaryOp {
                    op: UnaryOpKind::BitNot,
                    operand: Box::new(operand),
                    span,
                })
            }
            _ => self.postfix(),
        }
    }

    /// Operadores postfix: acceso a campo (`.field`), llamada (`(args)`),
    /// indexing (`[expr]`) y `?` postfix. Iteran en loop porque se pueden
    /// encadenar: `user.profile.email`, `xs[0][1]`, `m["clave"]`,
    /// `xs.map(f).filter(g)`.
    ///
    /// Since Phase 3.4 the callee of a call is any postfix expression
    /// — `Expr::Call.callee` is `Box<Expr>`. That unlocks method
    /// calls (`xs.map(...)`), on-the-fly anonymous fn invocation
    /// (`(fn(x) => x + 1)(2)`), and future higher-order patterns.
    fn postfix(&mut self) -> FitzResult<Expr> {
        let mut expr = self.primary()?;
        loop {
            // PreF8.2: multi-line method chain. If peek() is Newline,
            // look ahead skipping newlines: if the next significant
            // token is `.`, consume the newlines and let the next
            // loop iteration match the `Dot`. Only `.` continues —
            // `(`, `[`, `?` break as before so we don't change the
            // semantics of expression statements ambiguously separated
            // by newlines.
            if matches!(self.peek(), Token::Newline) {
                let mut i = 1;
                while matches!(self.peek_at(i), Token::Newline) {
                    i += 1;
                }
                if matches!(self.peek_at(i), Token::Dot) {
                    for _ in 0..i {
                        self.advance();
                    }
                    continue;
                }
                break;
            }
            match self.peek() {
                Token::Dot => {
                    let span = self.cur_span();
                    // Phase 6.1: `.await` postfix. Detected before
                    // consuming the `.` because `await` is already a
                    // lexer keyword (`Token::Await`), not an Ident —
                    // the normal `.field` path would fail with "field
                    // name expected" otherwise. Same spot in the
                    // chain as `.field` and `.method()`, so
                    // `expr.await?`, `expr.await.field`, `expr.await()`
                    // fit through the loop's natural continuation.
                    if matches!(self.peek_at(1), Token::Await) {
                        self.advance(); // consume '.'
                        self.advance(); // consume 'await'
                        expr = Expr::Await(Box::new(expr), span);
                        continue;
                    }
                    // Mini-batch T — `t.0`, `t.1`, etc. Tuple field
                    // access. The lexer emits `Int(n)` separately from
                    // the `.`, so we detect `Dot Int` via lookahead.
                    if let Token::Int(n) = self.peek_at(1).clone() {
                        if n < 0 {
                            return Err(self.error(
                                ErrorKind::InvalidSyntax,
                                "tuple index must be non-negative",
                            ));
                        }
                        self.advance(); // consume '.'
                        self.advance(); // consume the Int
                        expr = Expr::TupleField {
                            tuple: Box::new(expr),
                            index: n as usize,
                            span,
                        };
                        continue;
                    }
                    self.advance();
                    // v0.9.51 F15 sub-stmt recovery — if there is no
                    // ident after `.` (typical: EOF, Newline, another
                    // stmt), in recovery mode we keep `Expr::Field`
                    // with `field: ""` (placeholder) instead of
                    // dropping the whole stmt. Enables fine completion
                    // after `user.<typo or EOF>` — the LSP uses the
                    // `object` type to suggest fields/methods.
                    let field = match self.expect_ident("expected field name after '.'") {
                        Ok(f) => f,
                        Err(e) => {
                            if self.recovery_mode {
                                self.push_recovered(e);
                                String::new()
                            } else {
                                return Err(e);
                            }
                        }
                    };
                    expr = Expr::Field {
                        object: Box::new(expr),
                        field,
                        span,
                    };
                }
                Token::LParen => {
                    let span = self.cur_span();
                    self.advance(); // consume '('
                    let prev = std::mem::replace(&mut self.no_struct_literal, false);
                    let args_result = self.parse_call_args();
                    self.no_struct_literal = prev;
                    let args = args_result?;

                    let is_ok_or_err =
                        matches!(&expr, Expr::Ident(n, _) if n == "Ok" || n == "Err");
                    if is_ok_or_err {
                        let name = if let Expr::Ident(n, _) = &expr {
                            n.clone()
                        } else {
                            unreachable!()
                        };
                        if args.len() != 1 {
                            return Err(self.error(
                                ErrorKind::InvalidSyntax,
                                format!(
                                    "`{}` expects exactly 1 argument, got {}",
                                    name,
                                    args.len()
                                ),
                            ));
                        }
                        let inner = args.into_iter().next().unwrap();
                        // The Ok/Err span is inherited from the receiver Ident.
                        let ctor_span = expr.span();
                        expr = if name == "Ok" {
                            Expr::Ok(Box::new(inner), ctor_span)
                        } else {
                            Expr::Err(Box::new(inner), ctor_span)
                        };
                    } else {
                        expr = Expr::Call {
                            callee: Box::new(expr),
                            args,
                            span,
                        };
                    }
                }
                Token::Question => {
                    let span = self.cur_span();
                    self.advance();
                    expr = Expr::Try(Box::new(expr), span);
                }
                Token::LBracket => {
                    let span = self.cur_span();
                    self.advance(); // consume '['
                    let prev_no_struct = std::mem::replace(&mut self.no_struct_literal, false);
                    // I.2 — enter slice context so range_expr does NOT
                    // consume `..`/`..=`. We handle it manually here.
                    let prev_slice = std::mem::replace(&mut self.in_slice_context, true);

                    // Case A: `[..end]` or `[..=end]` or `[..]` —
                    // slice with no start.
                    let bracket_result: FitzResult<Expr> = match self.peek().clone() {
                        Token::DotDot | Token::DotDotEq => {
                            let inclusive = matches!(self.peek(), Token::DotDotEq);
                            self.advance(); // consume `..` or `..=`
                            let end = if matches!(self.peek(), Token::RBracket) {
                                None
                            } else {
                                Some(Box::new(self.expression()?))
                            };
                            Ok(Expr::Slice {
                                object: Box::new(expr.clone()),
                                start: None,
                                end,
                                inclusive,
                                span,
                            })
                        }
                        _ => {
                            // Case B: parse the first expr (with
                            // in_slice_context=true, won't consume `..`).
                            let first = self.expression()?;
                            // Case B.1: plain index.
                            if matches!(self.peek(), Token::RBracket) {
                                Ok(Expr::Index {
                                    object: Box::new(expr.clone()),
                                    index: Box::new(first),
                                    span,
                                })
                            } else if matches!(self.peek(), Token::DotDot | Token::DotDotEq) {
                                // Case B.2: slice with start. End
                                // is optional.
                                let inclusive = matches!(self.peek(), Token::DotDotEq);
                                self.advance(); // consume `..` or `..=`
                                let end = if matches!(self.peek(), Token::RBracket) {
                                    None
                                } else {
                                    Some(Box::new(self.expression()?))
                                };
                                Ok(Expr::Slice {
                                    object: Box::new(expr.clone()),
                                    start: Some(Box::new(first)),
                                    end,
                                    inclusive,
                                    span,
                                })
                            } else {
                                Err(self.error(
                                    ErrorKind::UnexpectedToken,
                                    "expected ']', '..' or '..=' in indexing content",
                                ))
                            }
                        }
                    };
                    self.no_struct_literal = prev_no_struct;
                    self.in_slice_context = prev_slice;
                    expr = bracket_result?;
                    self.expect(&Token::RBracket, "expected ']' to close indexing")?;
                }
                Token::LBrace => {
                    let ident_info = match &expr {
                        Expr::Ident(n, s) => Some((n.clone(), *s)),
                        _ => None,
                    };
                    let Some((name, ident_span)) = ident_info else {
                        break;
                    };

                    if self.no_struct_literal {
                        if self.looks_like_struct_lit_body() {
                            return Err(self.error(
                                ErrorKind::UnexpectedToken,
                                "struct literals are not allowed \
                                 directly in if/while/for/match \
                                 conditions — wrap it in \
                                 parentheses: `(User { id: 1 })`",
                            ));
                        }
                        break;
                    }

                    // Reuse the Ident span (the type name).
                    expr = self.parse_struct_lit_body(name, ident_span)?;
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    /// Struct literal body: `{ field: expr, field: expr, ... }`. The
    /// receiver (type name) is already consumed and is passed as
    /// `type_name`. Accepts:
    ///   - Empty: `{}`.
    ///   - Trailing comma.
    ///   - Newlines between fields (multiline literal).
    ///   - Comma or newline as field separator.
    ///
    /// Inside values, the `no_struct_literal` flag is restored to
    /// `false` (each value is delimited by `,` or `}`), so we allow
    /// nesting: `Order { user: User { id: 1, name: "x" } }`.
    fn parse_struct_lit_body(&mut self, type_name: String, span: Span) -> FitzResult<Expr> {
        self.expect(&Token::LBrace, "expected '{'")?;
        let prev = std::mem::replace(&mut self.no_struct_literal, false);
        let result = self.parse_struct_lit_fields(type_name, span);
        self.no_struct_literal = prev;
        result
    }

    fn parse_struct_lit_fields(&mut self, type_name: String, span: Span) -> FitzResult<Expr> {
        let mut fields: Vec<(String, Expr)> = Vec::new();
        self.skip_newlines();
        // Empty: `Empty {}`.
        if matches!(self.peek(), Token::RBrace) {
            self.advance();
            return Ok(Expr::StructLit {
                type_name,
                fields,
                span,
            });
        }
        loop {
            self.skip_newlines();
            if matches!(self.peek(), Token::RBrace) {
                self.advance();
                return Ok(Expr::StructLit {
                    type_name,
                    fields,
                    span,
                });
            }
            if self.is_at_end() {
                return Err(self.error(
                    ErrorKind::MissingClosingBrace,
                    "expected '}' to close the struct literal",
                ));
            }
            let field_name = self.expect_ident("expected field name in struct literal")?;
            self.expect(
                &Token::Colon,
                "expected ':' after field name in struct literal",
            )?;
            self.skip_newlines();
            let value = self.expression()?;
            fields.push((field_name, value));
            // Allowed separators: comma or newline. RBrace closes the
            // literal in the next loop iter. Anything else → error.
            match self.peek() {
                Token::Comma => {
                    self.advance();
                }
                Token::Newline | Token::RBrace => {
                    // skip_newlines in the next iter eats the newline.
                }
                _ => {
                    return Err(self.error(
                        ErrorKind::UnexpectedToken,
                        "expected ',', newline, or '}' between struct literal fields",
                    ));
                }
            }
        }
    }

    // ---------- statements ----------
    //
    // A program is a list of statements separated by `Newline` (or
    // `EOF` at the end). The braces of a block (`{ ... }`) also act
    // as an implicit terminator: a block can end without a newline
    // before the `}`. That logic lives in `consume_stmt_terminator`.
    //
    // `parse_stmt` dispatch:
    //   Let                    → `let` assignment
    //   Return                 → return
    //   Break / Continue       → simple statement
    //   Ident + (Eq|Colon)     → assignment without `let`  (peek_at(1) lookahead)
    //   anything else          → expression-statement

    /// Entry point to parse a full program (top-level). Consumes
    /// everything up to `EOF`.
    ///
    /// If `recovery_mode` is active (`parse_with_recovery` mode), an
    /// error from `parse_stmt` is not propagated: it is accumulated
    /// in `recovered_errors`, we synchronize to the next stmt
    /// boundary, and a `Stmt::Error(span)` is inserted in its place.
    /// The loop runs until EOF or until `MAX_RECOVERED_ERRORS` is
    /// reached.
    fn parse_program(&mut self) -> FitzResult<Program> {
        let mut stmts = Vec::new();
        loop {
            self.skip_newlines();
            if self.is_at_end() {
                break;
            }
            if self.recovery_mode && self.recovered_errors.len() >= MAX_RECOVERED_ERRORS {
                break;
            }
            let stmt_span = self.cur_span();
            match self.parse_stmt() {
                Ok(s) => stmts.push(s),
                Err(e) => {
                    if !self.recovery_mode {
                        return Err(e);
                    }
                    self.push_recovered(e);
                    self.synchronize();
                    stmts.push(Stmt::Error(stmt_span));
                    continue;
                }
            }
            if let Err(e) = self.consume_stmt_terminator() {
                if !self.recovery_mode {
                    return Err(e);
                }
                self.push_recovered(e);
                self.synchronize();
            }
        }
        Ok(stmts)
    }

    /// Push a recovered error, respecting the cap. If we already hit
    /// the maximum, the error is silently dropped — the caller will
    /// see in the final `Vec` that we are at the limit.
    fn push_recovered(&mut self, e: FitzError) {
        if self.recovered_errors.len() < MAX_RECOVERED_ERRORS {
            self.recovered_errors.push(e);
        }
    }

    /// Advance the cursor to a stmt-level sync point. Sync points are:
    ///  - `Newline` — Fitz's natural stmt terminator (consumed).
    ///  - `RBrace` — block close (NOT consumed; the caller handles
    ///    it to close the current block).
    ///  - `EOF` — end of file (NOT consumed).
    ///  - Keywords that typically start a stmt: `Let`, `Fn`, `Async`,
    ///    `Type`, `Return`, `Break`, `Continue`, `While`, `Loop`,
    ///    `For`, `If`, `Import`, `From`, `At` (decorator). If the
    ///    cursor sits on one, it is NOT consumed — we stop right
    ///    before so the next `parse_stmt` can grab it.
    ///
    /// Why stop at keywords: `primary()` consumes the current token
    /// before validating it. If an expression breaks on a `Newline`
    /// or another odd token, the cursor may have advanced past the
    /// newline up to the `Let` of the next stmt. Without the
    /// keyword rule, `synchronize` would eat the whole next stmt
    /// hunting for a newline.
    ///
    /// Fitz has no `;` as a separator — Newline is the only explicit
    /// terminator.
    fn synchronize(&mut self) {
        loop {
            match self.peek() {
                Token::Newline => {
                    self.advance();
                    return;
                }
                Token::RBrace | Token::EOF => return,
                // Keywords that typically start a stmt. Do not consume
                // — stop right before so the next `parse_stmt` can
                // process them from scratch.
                Token::Let
                | Token::Fn
                | Token::Async
                | Token::Type
                | Token::Return
                | Token::Break
                | Token::Continue
                | Token::While
                | Token::Loop
                | Token::For
                | Token::If
                | Token::Import
                | Token::From
                | Token::At => return,
                _ => {
                    self.advance();
                }
            }
        }
    }

    /// After a statement, consume its terminator. `Newline` is
    /// consumed; `EOF` and `RBrace` are left unconsumed (the caller
    /// decides what to do with them).
    fn consume_stmt_terminator(&mut self) -> FitzResult<()> {
        match self.peek() {
            Token::Newline => {
                self.advance();
                Ok(())
            }
            Token::EOF | Token::RBrace => Ok(()),
            _ => Err(self.error(
                ErrorKind::UnexpectedToken,
                "expected newline or end of block between statements",
            )),
        }
    }

    /// Parse ONE statement. The caller handles terminators and loops.
    /// Captures the span of the first token and passes it to each
    /// sub-parser through its `Stmt` constructors.
    fn parse_stmt(&mut self) -> FitzResult<Stmt> {
        let (line, column) = self.current_pos();
        let span = Span::new(line, column);
        match self.peek() {
            Token::Let => self.parse_assign_with_let(span),
            Token::Return => self.parse_return(span),
            Token::Fn | Token::Async => self.parse_fndef(span),
            Token::Type => self.parse_typedef(span),
            Token::At => self.parse_decorated_stmt(span),
            Token::Break => {
                self.advance();
                // Mini-batch L — syntax `break ['label] [<expr>]`.
                // Label first (if any), then optional value. Rust
                // uses the same order: `break 'outer 42`.
                let label = if let Token::Label(l) = self.peek().clone() {
                    self.advance();
                    Some(l)
                } else {
                    None
                };
                let value = match self.peek() {
                    Token::Newline | Token::RBrace | Token::EOF => None,
                    _ => Some(self.expression()?),
                };
                Ok(Stmt::Break(value, label, span))
            }
            Token::Continue => {
                self.advance();
                let label = if let Token::Label(l) = self.peek().clone() {
                    self.advance();
                    Some(l)
                } else {
                    None
                };
                Ok(Stmt::Continue(label, span))
            }
            Token::While => self.parse_while(span),
            Token::Loop => self.parse_loop(span),
            Token::For => self.parse_for(span),
            // Mini-batch L — `'label: <loop>` declares a label before
            // the loop. Supports loop/while/for. The parser consumes
            // the Label + Colon and delegates to parse_*_with_label.
            Token::Label(_) => {
                let label = if let Token::Label(l) = self.peek().clone() {
                    l
                } else {
                    unreachable!()
                };
                self.advance();
                self.expect(&Token::Colon, "expected ':' after label")?;
                match self.peek() {
                    Token::Loop => self.parse_loop_with_label(span, Some(label)),
                    Token::While => self.parse_while_with_label(span, Some(label)),
                    Token::For => self.parse_for_with_label(span, Some(label)),
                    _ => Err(self.error(
                        ErrorKind::UnexpectedToken,
                        "expected `loop`, `while`, or `for` after label",
                    )),
                }
            }
            Token::Import => self.parse_import(span),
            Token::From => self.parse_from_import(span),
            _ => self.parse_expr_or_assign_stmt(span),
        }
    }

    /// `import foo` or `import foo.bar.baz`. The path accumulates as
    /// `Ident ( '.' Ident )*`. PreF8.4: accepts `as <ident>` at the
    /// end to alias the namespace (`import foo as f` → binding `f`
    /// instead of the last segment).
    fn parse_import(&mut self, span: Span) -> FitzResult<Stmt> {
        self.expect(&Token::Import, "expected 'import'")?;
        let path = self.parse_module_path()?;
        let alias = if matches!(self.peek(), Token::As) {
            self.advance();
            Some(self.expect_ident("expected an identifier after 'as' in 'import ... as ...'")?)
        } else {
            None
        };
        Ok(Stmt::Import { path, alias, span })
    }

    /// `from foo import a, b, c` — the path may have dots (`from
    /// sub.foo import bar`). The names list must have at least one.
    /// Accepts trailing comma. PreF8.4: each name may carry
    /// `as <ident>` for aliasing (`from foo import bar as b, baz as z`).
    fn parse_from_import(&mut self, span: Span) -> FitzResult<Stmt> {
        self.expect(&Token::From, "expected 'from'")?;
        let path = self.parse_module_path()?;
        self.expect(
            &Token::Import,
            "expected 'import' after path in 'from ... import ...'",
        )?;

        // Mini-batch Mln — multi-line via parens. If a `(` follows
        // the `import`, we enter multi-line mode: newlines between
        // names are tolerated (we consume them), and we close with
        // `)`. Without parens, the single-line behavior stands.
        let multiline = matches!(self.peek(), Token::LParen);
        if multiline {
            self.advance(); // consume '('
            self.skip_newlines_inside_parens();
            let mut names: Vec<(String, Option<String>)> = Vec::new();
            names.push(self.parse_from_import_name(/*is_first=*/ true)?);
            self.skip_newlines_inside_parens();
            while matches!(self.peek(), Token::Comma) {
                self.advance();
                self.skip_newlines_inside_parens();
                // Trailing comma before `)` is OK.
                if matches!(self.peek(), Token::RParen) {
                    break;
                }
                names.push(self.parse_from_import_name(/*is_first=*/ false)?);
                self.skip_newlines_inside_parens();
            }
            self.expect(
                &Token::RParen,
                "expected ')' to close `from ... import (...)`",
            )?;
            return Ok(Stmt::FromImport { path, names, span });
        }

        let mut names: Vec<(String, Option<String>)> = Vec::new();
        names.push(self.parse_from_import_name(/*is_first=*/ true)?);
        while matches!(self.peek(), Token::Comma) {
            self.advance();
            // Trailing comma: `from foo import a,` — stop without error.
            if matches!(self.peek(), Token::Newline | Token::EOF | Token::RBrace) {
                break;
            }
            names.push(self.parse_from_import_name(/*is_first=*/ false)?);
        }
        Ok(Stmt::FromImport { path, names, span })
    }

    /// Mini-batch Mln — Helper for multi-line `from foo import (...)`.
    /// Consumes consecutive newlines until a content token is reached.
    /// No depth checks — the caller is already inside the paren.
    fn skip_newlines_inside_parens(&mut self) {
        while matches!(self.peek(), Token::Newline) {
            self.advance();
        }
    }

    /// Helper: parse a `from ... import` binding: `Ident [as Ident]`.
    /// `is_first` only changes the error message of the first ident.
    fn parse_from_import_name(&mut self, is_first: bool) -> FitzResult<(String, Option<String>)> {
        let name = self.expect_ident(if is_first {
            "expected at least one identifier after 'import'"
        } else {
            "expected identifier after ',' in 'from ... import'"
        })?;
        let alias = if matches!(self.peek(), Token::As) {
            self.advance();
            Some(self.expect_ident(
                "expected an identifier after 'as' in 'from ... import ... as ...'",
            )?)
        } else {
            None
        };
        Ok((name, alias))
    }

    /// Module path: `Ident ( '.' Ident )*`. Returns the segments.
    /// Always has at least one element. Serves both `import` and
    /// `from ... import`.
    fn parse_module_path(&mut self) -> FitzResult<Vec<String>> {
        let first = self.expect_ident("expected module name (identifier)")?;
        let mut segments = vec![first];
        while matches!(self.peek(), Token::Dot) {
            self.advance();
            let next = self.expect_ident("expected module name after '.'")?;
            segments.push(next);
        }
        Ok(segments)
    }

    fn parse_assign_with_let(&mut self, span: Span) -> FitzResult<Stmt> {
        self.expect(&Token::Let, "expected 'let'")?;
        // Mini-batch T — destructuring `let (a, b) = expr`. Detected
        // by peeking `(`. The pattern allows nesting: `let ((x, y),
        // z) = ...`. No type annotation for MVP simplicity (the
        // checker infers from the RHS).
        if matches!(self.peek(), Token::LParen) {
            let pattern = self.parse_pattern()?;
            self.expect(&Token::Eq, "expected '=' in destructuring declaration")?;
            let value = self.expression()?;
            return Ok(Stmt::Destructure {
                pattern,
                value,
                span,
            });
        }
        let (name, name_span) =
            self.expect_ident_with_span("expected variable name after 'let'")?;
        let type_ = self.parse_optional_type_annotation()?;
        self.expect(&Token::Eq, "expected '=' in declaration")?;
        let value = self.expression()?;
        Ok(Stmt::Assign {
            target: AssignTarget::Ident(name, name_span),
            type_,
            value,
            span,
        })
    }

    /// Parse a statement that starts with an expression. Three cases:
    ///   1. `expr` — expression statement (typically a call).
    ///   2. `Ident: Type = expr` — annotated declaration/reassign.
    ///   3. `lvalue = expr` — assignment. The lvalue can be an
    ///      `Ident` (variable) or `Expr::Field` (mutating an
    ///      instance field). Any other form (`f() = ...`,
    ///      `xs[0] = ...`) is an error.
    ///
    /// Unifies the formerly separate `parse_assign_no_let` and
    /// `parse_expr_stmt` paths: we parse the full expression first,
    /// then decide if it was an assignment based on the remaining
    /// token. That naturally resolves `user.name = "x"` and removes
    /// the hard lookahead that used to look only at `peek_at(1)`.
    fn parse_expr_or_assign_stmt(&mut self, span: Span) -> FitzResult<Stmt> {
        let lhs = self.expression()?;

        // Case 2: `Ident : Type = expr`. The annotation is only
        // accepted on a bare identifier.
        if matches!(self.peek(), Token::Colon) {
            let (name, name_span) = match lhs {
                Expr::Ident(n, ispan) => (n, ispan),
                _ => {
                    return Err(self.error(
                        ErrorKind::InvalidSyntax,
                        "type annotation is only allowed when declaring a variable",
                    ));
                }
            };
            self.advance(); // consume ':'
            let type_ = self.parse_type_expr()?;
            self.expect(&Token::Eq, "expected '=' in assignment")?;
            let value = self.expression()?;
            return Ok(Stmt::Assign {
                target: AssignTarget::Ident(name, name_span),
                type_: Some(type_),
                value,
                span,
            });
        }

        // Case 3: `lvalue = expr`.
        if self.eat(&Token::Eq) {
            let value = self.expression()?;
            let target = match lhs {
                Expr::Ident(n, ispan) => AssignTarget::Ident(n, ispan),
                Expr::Field { object, field, .. } => AssignTarget::Field { object, field },
                // R.1.3 — `xs[i] = v` and `m["k"] = v` (mini-phase R).
                // The parser already built `Expr::Index { object,
                // index }` as part of the postfix; we deconstruct it
                // here to build the `AssignTarget::Index`.
                Expr::Index { object, index, .. } => AssignTarget::Index { object, index },
                _ => {
                    return Err(self.error(
                        ErrorKind::InvalidSyntax,
                        "unsupported assignment target (only identifier, \
                         `expr.field`, or `expr[index]`)",
                    ));
                }
            };
            return Ok(Stmt::Assign {
                target,
                type_: None,
                value,
                span,
            });
        }

        // Case 3b — R.2.3: compound operators `+=`/`-=`/`*=`/`/=`.
        // Desugar to `target = target <op> rhs` in the parser. That
        // leaves the rest of the pipeline (checker, evaluator,
        // codegen) untouched — they work with regular `Stmt::Assign`.
        // The target is evaluated TWICE: once as Expr (RHS of the
        // BinOp) and once as AssignTarget (destination). The index
        // evaluator uses the "compute first, lock last" pattern
        // (R.1.3), so double evaluation of the index is also safe.
        let compound_op = match self.peek() {
            Token::PlusEq => Some(BinOpKind::Add),
            Token::MinusEq => Some(BinOpKind::Sub),
            Token::StarEq => Some(BinOpKind::Mul),
            Token::SlashEq => Some(BinOpKind::Div),
            // Mini-batch Cmp — compound bitwise ops.
            Token::AmpEq => Some(BinOpKind::BitAnd),
            Token::PipeEq => Some(BinOpKind::BitOr),
            Token::CaretEq => Some(BinOpKind::BitXor),
            Token::ShlEq => Some(BinOpKind::Shl),
            Token::ShrEq => Some(BinOpKind::Shr),
            _ => None,
        };
        if let Some(op) = compound_op {
            let op_span = self.cur_span();
            self.advance(); // consume the `+=`/etc. token
            let rhs = self.expression()?;
            let (target, target_as_expr) = match lhs {
                Expr::Ident(n, ispan) => {
                    (AssignTarget::Ident(n.clone(), ispan), Expr::Ident(n, ispan))
                }
                Expr::Field {
                    object,
                    field,
                    span: fspan,
                } => (
                    AssignTarget::Field {
                        object: object.clone(),
                        field: field.clone(),
                    },
                    Expr::Field {
                        object,
                        field,
                        span: fspan,
                    },
                ),
                Expr::Index {
                    object,
                    index,
                    span: ispan,
                } => (
                    AssignTarget::Index {
                        object: object.clone(),
                        index: index.clone(),
                    },
                    Expr::Index {
                        object,
                        index,
                        span: ispan,
                    },
                ),
                _ => {
                    return Err(self.error(
                        ErrorKind::InvalidSyntax,
                        "unsupported compound assignment target (only identifier, \
                         `expr.field`, or `expr[index]`)",
                    ));
                }
            };
            let value = Expr::BinOp {
                op,
                left: Box::new(target_as_expr),
                right: Box::new(rhs),
                span: op_span,
            };
            return Ok(Stmt::Assign {
                target,
                type_: None,
                value,
                span,
            });
        }

        // Case 1: expression statement.
        Ok(Stmt::Expr(lhs, span))
    }

    /// Optional type annotation: `: TypeExpr`. Returns `Some(t)` if
    /// it was present. Accepts `Int`, `Str`, `List<Int>`,
    /// `Map<Str, User>`, `Result<List<User>>`, `User?`,
    /// `Map<Str, Int>?`, etc.
    fn parse_optional_type_annotation(&mut self) -> FitzResult<Option<TypeExpr>> {
        if self.eat(&Token::Colon) {
            Ok(Some(self.parse_type_expr()?))
        } else {
            Ok(None)
        }
    }

    /// Parse a (required) `TypeExpr` in annotation position.
    ///
    /// Grammar:
    ///
    /// ```text
    /// type_expr := fn_type | atom ( '?' )?
    /// fn_type   := 'Fn' '(' ( type_expr ( ',' type_expr )* )? ')' '->' type_expr
    /// atom      := Ident generic_args?
    /// generic_args := '<' type_expr ( ',' type_expr )* '>'
    /// ```
    ///
    /// The `?` suffix attaches to the whole atom: `List<Int>?` →
    /// `Nullable(List<Int>)`. We accept `?` only once for now; `T??`
    /// could be modeled later (`Nullable(Nullable(T))`), but today
    /// `eat` only consumes one and a second `?` is left unconsumed
    /// without an explicit error. The static checker can normalize
    /// it when it lands.
    ///
    /// `Fn` is a syntactic contextual keyword of the function type.
    /// When `Fn` is followed by `(`, we parse it as
    /// `TypeExpr::Function`. If the next token is not `(`, `Fn` is
    /// treated as a regular nominal name — resolution will fail
    /// because it does not exist as a type in the env.
    ///
    /// Lexing note: the lexer always emits `>` as `Token::Gt` (there
    /// is no `>>` as a single token), so `Result<List<Int>>` closes
    /// by consuming two separate `Token::Gt` — one per generic level.
    fn parse_type_expr(&mut self) -> FitzResult<TypeExpr> {
        // Mini-batch T — tuple type `(T1, T2, ...)`. `()` is the
        // empty tuple, `(T,)` a single-element tuple (trailing
        // comma required), `(T)` is just parens (no tuple, delegates
        // to the inner type).
        if matches!(self.peek(), Token::LParen) {
            self.advance(); // consume `(`
            if matches!(self.peek(), Token::RParen) {
                self.advance();
                let mut t = TypeExpr::Tuple(Vec::new());
                if self.eat(&Token::Question) {
                    t = TypeExpr::Nullable(Box::new(t));
                }
                return Ok(t);
            }
            let first = self.parse_type_expr()?;
            if matches!(self.peek(), Token::Comma) {
                let mut items = vec![first];
                while matches!(self.peek(), Token::Comma) {
                    self.advance();
                    if matches!(self.peek(), Token::RParen) {
                        break;
                    }
                    items.push(self.parse_type_expr()?);
                }
                self.expect(&Token::RParen, "expected ')' to close the tuple type")?;
                let mut t = TypeExpr::Tuple(items);
                if self.eat(&Token::Question) {
                    t = TypeExpr::Nullable(Box::new(t));
                }
                return Ok(t);
            }
            // No comma → just grouping parens.
            self.expect(
                &Token::RParen,
                "expected ')' to close the parenthesized type",
            )?;
            let mut t = first;
            if self.eat(&Token::Question) {
                t = TypeExpr::Nullable(Box::new(t));
            }
            return Ok(t);
        }
        let name = self.expect_ident("expected a type name")?;
        // Contextual keyword: `Fn(...)` → function type.
        if name == "Fn" && matches!(self.peek(), Token::LParen) {
            return self.parse_fn_type();
        }
        let mut t = if matches!(self.peek(), Token::Lt) {
            self.advance(); // consume '<'
            self.skip_newlines();
            if matches!(self.peek(), Token::Gt) {
                return Err(self.error(
                    ErrorKind::UnexpectedToken,
                    format!(
                        "empty generic `{}<>`: expected at least one type argument",
                        name
                    ),
                ));
            }
            let mut args = Vec::new();
            loop {
                self.skip_newlines();
                args.push(self.parse_type_expr()?);
                self.skip_newlines();
                if matches!(self.peek(), Token::Comma) {
                    self.advance();
                    continue;
                }
                break;
            }
            self.skip_newlines();
            // Mini-batch Bits: the lexer now produces `Token::Shr`
            // for `>>`, which breaks `List<List<Int>>` and similar.
            // Here we split a `Shr` into two `Gt` consuming only one
            // — the second `>` stays as `Gt` for the outer caller.
            match self.peek() {
                Token::Gt => {
                    self.advance();
                }
                Token::Shr => {
                    // Mutate the current token to Gt in place and
                    // shift the column to point at the second `>`.
                    self.tokens[self.pos].token = Token::Gt;
                    self.tokens[self.pos].column += 1;
                }
                _ => {
                    return Err(self.error(
                        ErrorKind::UnexpectedToken,
                        format!("expected '>' to close `{}<...>`", name),
                    ));
                }
            }
            TypeExpr::Generic { name, args }
        } else {
            TypeExpr::Named(name)
        };
        if self.eat(&Token::Question) {
            t = TypeExpr::Nullable(Box::new(t));
        }
        Ok(t)
    }

    /// `Fn` already consumed; parses `(P1, P2, ...) -> R`.
    fn parse_fn_type(&mut self) -> FitzResult<TypeExpr> {
        self.expect(&Token::LParen, "expected '(' after `Fn`")?;
        self.skip_newlines();
        let mut params: Vec<TypeExpr> = Vec::new();
        if !matches!(self.peek(), Token::RParen) {
            loop {
                self.skip_newlines();
                params.push(self.parse_type_expr()?);
                self.skip_newlines();
                if matches!(self.peek(), Token::Comma) {
                    self.advance();
                    continue;
                }
                break;
            }
        }
        self.skip_newlines();
        self.expect(&Token::RParen, "expected ')' to close `Fn(...)`")?;
        self.expect(
            &Token::Arrow,
            "expected '->' with the return type after `Fn(...)`",
        )?;
        let ret = self.parse_type_expr()?;
        Ok(TypeExpr::Function {
            params,
            ret: Box::new(ret),
        })
    }

    fn parse_return(&mut self, span: Span) -> FitzResult<Stmt> {
        self.expect(&Token::Return, "expected 'return'")?;
        // `return` with no value returns implicit null. Detect the
        // valid statement terminators: end of line, block close, or
        // end of file.
        match self.peek() {
            Token::Newline | Token::RBrace | Token::EOF => {
                return Ok(Stmt::Return(Expr::Null(span), span));
            }
            _ => {}
        }

        // Mini-batch OAPI — specific lookahead to detect the
        // ReturnStatus pattern BEFORE calling `expression()` (which
        // greedily parses `Ident { ... }` as a struct literal). The
        // pattern is:
        //   `<Int> { ... }` or `<Ident> { ... }` where the `{...}`
        //   is a map literal (first key Str), not a struct lit
        //   (first key Ident).
        //
        // Robust disambiguation:
        //   - tok0: Int or Ident
        //   - tok1: LBrace
        //   - first non-newline token after LBrace: Str (map lit)
        //     or RBrace (empty map)
        //
        // If the first key is Ident → struct lit
        // (`return P { x: 1 }`), this is NOT a ReturnStatus.
        // If it is Str → map lit
        // (`return NOT_FOUND { "error": "..." }`), this IS a
        // ReturnStatus.
        let looks_like_return_status = {
            let t0 = self.peek_at(0);
            let t1 = self.peek_at(1);
            let head_ok = matches!(t0, Token::Int(_) | Token::Ident(_));
            let brace_next = matches!(t1, Token::LBrace);
            if head_ok && brace_next {
                // Skip newlines after the LBrace (bound: 16 to
                // avoid long walks over pathological files).
                const MAX_SKIP: usize = 16;
                let mut i = 2usize;
                let mut is_map_body = false;
                while i < 2 + MAX_SKIP {
                    match self.peek_at(i) {
                        Token::Newline => {
                            i += 1;
                        }
                        Token::Str(_) | Token::RBrace => {
                            is_map_body = true;
                            break;
                        }
                        _ => break,
                    }
                }
                is_map_body
            } else {
                false
            }
        };

        if looks_like_return_status {
            // Parse the status as an atom — Int or Ident only, with
            // NO postfix (no call, no field, no struct lit).
            let status_span = self.cur_span();
            let status = match self.peek().clone() {
                Token::Int(n) => {
                    self.advance();
                    Expr::Int(n, status_span)
                }
                Token::Ident(name) => {
                    self.advance();
                    Expr::Ident(name, status_span)
                }
                _ => unreachable!("lookahead guarantees Int or Ident"),
            };
            let body = self.expression()?;
            return Ok(Stmt::ReturnStatus {
                status,
                body: Some(body),
                span,
            });
        }

        let value = self.expression()?;
        Ok(Stmt::Return(value, span))
    }

    // ---------- function definition ----------
    //
    // Four shapes (combinable with optional `async`):
    //   fn name(params) { body }
    //   fn name(params) -> Type { body }
    //   fn name(params) => expr
    //   fn name(params) -> Type => expr
    //
    // The arrow shape desugars to `body: vec![Stmt::Return(expr,
    // Span::ZERO)]` (decision documented in ast.rs).

    fn parse_fndef(&mut self, span: Span) -> FitzResult<Stmt> {
        let is_async = self.eat(&Token::Async);
        self.expect(&Token::Fn, "expected 'fn'")?;
        let name = self.expect_ident("expected function name after 'fn'")?;
        self.expect(&Token::LParen, "expected '(' after function name")?;
        let params = self.parse_params()?;
        let return_type = self.parse_optional_return_type()?;

        // Body: block `{ ... }` or arrow `=> expr`.
        let body = match self.peek() {
            Token::FatArrow => {
                self.advance();
                let (arrow_line, arrow_col) = self.current_pos();
                let expr = self.expression()?;
                vec![Stmt::Return(expr, Span::new(arrow_line, arrow_col))]
            }
            Token::LBrace => self.parse_block()?,
            _ => {
                return Err(self.error(
                    ErrorKind::UnexpectedToken,
                    "expected '{' or '=>' for the function body",
                ));
            }
        };

        Ok(Stmt::FnDef {
            name,
            params,
            return_type,
            body,
            is_async,
            // The bare-fn parser does not know about decorators.
            // When entering via `parse_decorated_fndef`, that path
            // rebuilds the FnDef attaching the accumulated decorators.
            decorators: vec![],
            span,
        })
    }

    /// Anonymous function in expression position: `fn(x) => x * 2`
    /// or `fn(x) { return x * 2 }`. Differences vs `parse_fndef`:
    /// no name, and `async` was not allowed (had nowhere to apply
    /// until Phase 4). Body and return type are parsed the same way.
    fn parse_fn_expr(&mut self) -> FitzResult<Expr> {
        let span = self.cur_span();
        // Mini-batch Async-cl — `async fn(...)` is an async closure.
        // The body may use `.await` and the fn returns a `Future<T>`.
        let is_async = if matches!(self.peek(), Token::Async) {
            self.advance();
            true
        } else {
            false
        };
        self.expect(&Token::Fn, "expected 'fn'")?;
        self.expect(
            &Token::LParen,
            "expected '(' after 'fn' in anonymous function",
        )?;
        let params = self.parse_params()?;
        let _return_type = self.parse_optional_return_type()?;

        let body = match self.peek() {
            Token::FatArrow => {
                self.advance();
                let (arrow_line, arrow_col) = self.current_pos();
                let expr = self.expression()?;
                vec![Stmt::Return(expr, Span::new(arrow_line, arrow_col))]
            }
            Token::LBrace => self.parse_block()?,
            _ => {
                return Err(self.error(
                    ErrorKind::UnexpectedToken,
                    "expected '{' or '=>' for the anonymous function body",
                ));
            }
        };

        Ok(Expr::FnExpr {
            params,
            body,
            is_async,
            span,
        })
    }

    /// Parameter list, with the '(' already consumed. Finishes by
    /// consuming the ')'. Each parameter is `name`, `name: Type`,
    /// `name = default`, `name: Type = default` (mini-batch Fp —
    /// default params), or `...name: Type` (mini-batch Fp.2 —
    /// varargs). Accepts trailing comma and newlines inside the
    /// parens.
    ///
    /// **Python rule for defaults**: once a param has a default, all
    /// following params must have a default too.
    ///
    /// **Varargs rule (Fp.2)**: only the LAST param may be varargs.
    /// A varargs param CANNOT have a default. The body binding types
    /// as `List<T>` (or `List<Any>` if not annotated).
    fn parse_params(&mut self) -> FitzResult<Vec<Param>> {
        let mut params = Vec::new();
        let mut saw_default = false;
        let mut saw_varargs = false;
        self.skip_newlines();
        if matches!(self.peek(), Token::RParen) {
            self.advance();
            return Ok(params);
        }
        loop {
            self.skip_newlines();
            // Fp.2 — `...name` signals varargs. Detected as `..` + `.`
            // (Token::DotDot followed by Token::Dot). Fitz has `..`
            // for Range and `..=` for inclusive Range; three
            // consecutive `.` don't collide with anything (the lexer
            // matches greedily).
            let varargs =
                if matches!(self.peek(), Token::DotDot) && matches!(self.peek_at(1), Token::Dot) {
                    if saw_varargs {
                        return Err(self.error(
                            ErrorKind::UnexpectedToken,
                            "only one variadic parameter is allowed, and it must be last",
                        ));
                    }
                    self.advance(); // consume `..`
                    self.advance(); // consume `.`
                    saw_varargs = true;
                    true
                } else {
                    if saw_varargs {
                        return Err(self.error(
                            ErrorKind::UnexpectedToken,
                            "no more parameters allowed after a variadic parameter",
                        ));
                    }
                    false
                };
            let (name, name_span) = self.expect_ident_with_span("expected parameter name")?;
            let type_ = self.parse_optional_type_annotation()?;
            // Fp — default value `= <expr>`. Varargs do not allow a default.
            let default = if matches!(self.peek(), Token::Eq) {
                if varargs {
                    return Err(self.error(
                        ErrorKind::UnexpectedToken,
                        format!("variadic parameter `{}` cannot have a default", name),
                    ));
                }
                self.advance(); // consume `=`
                let expr = self.expression()?;
                saw_default = true;
                Some(expr)
            } else {
                // Default + varargs are mutually exclusive: a varargs
                // absorbs 0+ args, so it does NOT trigger the "all
                // following need defaults" rule (there are no
                // following params and it absorbs the role of
                // "additional optional args").
                if saw_default && !varargs {
                    return Err(self.error(
                        ErrorKind::UnexpectedToken,
                        format!(
                            "parameter `{}` has no default but a previous one does — \
                             in Fitz, once a param has a default, all following ones must too",
                            name
                        ),
                    ));
                }
                None
            };
            // T3 — duplicate name check. `fn f(a: Int, a: Int)` is a
            // typical copy-paste bug; the evaluator would emit a
            // confusing error ("variable redefined") when binding
            // the second `a`. Better to catch it in the parser with
            // a clear message citing the name.
            if params.iter().any(|p| p.name == name) {
                return Err(self.error(
                    ErrorKind::UnexpectedToken,
                    format!("parameter `{}` is duplicated in the parameter list", name),
                ));
            }
            params.push(Param {
                name,
                type_,
                default,
                varargs,
                name_span,
            });
            self.skip_newlines();
            if matches!(self.peek(), Token::Comma) {
                self.advance();
                self.skip_newlines();
                if matches!(self.peek(), Token::RParen) {
                    self.advance();
                    return Ok(params);
                }
            } else {
                break;
            }
        }
        self.expect(&Token::RParen, "expected ')' to close the parameter list")?;
        Ok(params)
    }

    /// Optional `-> TypeExpr`. Shares the type grammar with
    /// `parse_optional_type_annotation` — accepts generics and nullables.
    fn parse_optional_return_type(&mut self) -> FitzResult<Option<TypeExpr>> {
        if self.eat(&Token::Arrow) {
            Ok(Some(self.parse_type_expr()?))
        } else {
            Ok(None)
        }
    }

    /// Block `{ stmt; stmt; ... }`. Consumes the opening and closing
    /// braces. Accepts blank lines between statements and empty blocks.
    ///
    /// Recovery (9.0.1, F15): if `recovery_mode` is active, errors
    /// from `parse_stmt` inside the block are caught in parallel to
    /// the top-level loop — `Stmt::Error(span)` instead of the
    /// failing stmt, `synchronize()` up to `Newline`/`RBrace`/`EOF`,
    /// and we keep going. If the opening `{` never appeared or the
    /// closing `}` is missing, the error IS propagated: fixing a
    /// broken block structure during recovery is very costly; we
    /// prefer to abort the whole block and let the parent loop
    /// realign at the next sync point.
    fn parse_block(&mut self) -> FitzResult<Vec<Stmt>> {
        self.expect(&Token::LBrace, "expected '{'")?;
        let mut stmts = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), Token::RBrace) {
                self.advance();
                return Ok(stmts);
            }
            if self.is_at_end() {
                return Err(self.error(
                    ErrorKind::MissingClosingBrace,
                    "expected '}' to close the block",
                ));
            }
            if self.recovery_mode && self.recovered_errors.len() >= MAX_RECOVERED_ERRORS {
                // Jump to the block close (if any) so we don't leave
                // a dangling `{` in the parent AST.
                while !matches!(self.peek(), Token::RBrace | Token::EOF) {
                    self.advance();
                }
                if matches!(self.peek(), Token::RBrace) {
                    self.advance();
                }
                return Ok(stmts);
            }
            let stmt_span = self.cur_span();
            match self.parse_stmt() {
                Ok(s) => stmts.push(s),
                Err(e) => {
                    if !self.recovery_mode {
                        return Err(e);
                    }
                    self.push_recovered(e);
                    self.synchronize();
                    stmts.push(Stmt::Error(stmt_span));
                    continue;
                }
            }
            if let Err(e) = self.consume_stmt_terminator() {
                if !self.recovery_mode {
                    return Err(e);
                }
                self.push_recovered(e);
                self.synchronize();
            }
        }
    }

    // ---------- loops ----------

    /// `while cond { body }`. Conditional iteration. The condition is
    /// evaluated before each iteration; on `false`, the loop ends.
    fn parse_while(&mut self, span: Span) -> FitzResult<Stmt> {
        self.parse_while_with_label(span, None)
    }

    fn parse_while_with_label(&mut self, span: Span, label: Option<String>) -> FitzResult<Stmt> {
        self.expect(&Token::While, "expected 'while'")?;
        // The condition does not allow a struct literal at the top
        // level — the next `{` opens the while body. Inside parens
        // it's fine.
        let condition = self.expression_no_struct_lit()?;
        let body = self.parse_block()?;
        Ok(Stmt::While {
            condition,
            body,
            label,
            span,
        })
    }

    /// `loop { body }` — infinite loop. Only exits via `break` or `return`.
    fn parse_loop(&mut self, span: Span) -> FitzResult<Stmt> {
        self.parse_loop_with_label(span, None)
    }

    fn parse_loop_with_label(&mut self, span: Span, label: Option<String>) -> FitzResult<Stmt> {
        self.expect(&Token::Loop, "expected 'loop'")?;
        let body = self.parse_block()?;
        Ok(Stmt::Loop { body, label, span })
    }

    /// Mini-batch L — `loop { body }` as an expression. Identical
    /// version of `parse_loop` but returns `Expr::Loop`. Used when
    /// `loop` appears as the RHS of let, a call arg, etc. Optional
    /// `label` for `'name: loop { ... }`.
    fn parse_loop_expr(&mut self, label: Option<String>) -> FitzResult<Expr> {
        let span = self.cur_span();
        self.expect(&Token::Loop, "expected 'loop'")?;
        let body = self.parse_block()?;
        Ok(Expr::Loop { body, label, span })
    }

    /// `for var in iter { body }`. Iteration over lists and ranges
    /// (maps not yet, until we have the `Pair` type). `var` is
    /// defined in each iteration within the body's scope.
    fn parse_for(&mut self, span: Span) -> FitzResult<Stmt> {
        self.parse_for_with_label(span, None)
    }

    fn parse_for_with_label(&mut self, span: Span, label: Option<String>) -> FitzResult<Stmt> {
        self.expect(&Token::For, "expected 'for'")?;
        // Mini-batch Md: the `for` var is now a Pattern. Reuses
        // `parse_pattern` (same one used by match arms), which
        // covers Ident, Wildcard, Tuple — the 3 valid cases in
        // `for`. Other patterns (literals, Ok/Err, Range) are
        // rejected by the checker.
        let var = self.parse_pattern()?;
        self.expect(&Token::In, "expected 'in' after 'for' variable")?;
        // The iterable does not allow a struct literal at the top
        // level — the next `{` opens the `for` body. Inside parens
        // or lists it's fine: `for u in [User { id: 1 }]`.
        let iter = self.expression_no_struct_lit()?;
        let body = self.parse_block()?;
        Ok(Stmt::For {
            var,
            iter,
            body,
            label,
            span,
        })
    }

    // ---------- if / match / type ----------

    /// `if cond { ... }` or `if cond { ... } else { ... }` or
    /// `if cond { ... } else if ... { ... } else { ... }`. The
    /// `else if` chain desugars to an `else` containing a single
    /// statement: the next `if` wrapped in `Stmt::Expr`.
    fn parse_if_expr(&mut self) -> FitzResult<Expr> {
        let span = self.cur_span();
        self.expect(&Token::If, "expected 'if'")?;
        let condition = self.expression_no_struct_lit()?;
        let then = self.parse_block()?;
        let else_ = if self.eat(&Token::Else) {
            if matches!(self.peek(), Token::If) {
                let (nested_line, nested_col) = self.current_pos();
                let nested = self.parse_if_expr()?;
                Some(vec![Stmt::Expr(nested, Span::new(nested_line, nested_col))])
            } else {
                Some(self.parse_block()?)
            }
        } else {
            None
        };
        Ok(Expr::If {
            condition: Box::new(condition),
            then,
            else_,
            span,
        })
    }

    /// `match value { pat => expr, pat => expr, ... }`. Arms are
    /// separated by comma or newline (both accepted). Pattern
    /// limitations, per the AST: only `Ident`, `_` (wildcard),
    /// `Ok(x)`, `Err(e)`. Literals and ranges in patterns are
    /// explicit debt.
    fn parse_match_expr(&mut self) -> FitzResult<Expr> {
        let span = self.cur_span();
        self.expect(&Token::Match, "expected 'match'")?;
        // The scrutinee does not allow a struct literal at the top
        // level — the next `{` opens the arms block. Inside parens
        // it's fine.
        let value = self.expression_no_struct_lit()?;
        self.expect(&Token::LBrace, "expected '{' after match expression")?;
        let mut arms: Vec<MatchArm> = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), Token::RBrace) {
                self.advance();
                break;
            }
            if self.is_at_end() {
                return Err(self.error(
                    ErrorKind::MissingClosingBrace,
                    "expected '}' to close match",
                ));
            }
            let pattern = self.parse_or_pattern()?;
            // R.2.2 — optional guard `if <cond>` between pattern and `=>`.
            let guard = if matches!(self.peek(), Token::If) {
                self.advance(); // consume `if`
                Some(self.expression()?)
            } else {
                None
            };
            self.expect(&Token::FatArrow, "expected '=>' after pattern")?;
            // Sp.2 — the arm body may be:
            //   1. `return <expr>` / `break <expr>` / `continue` → Stmt directly.
            //   2. `{ <stmts> }` → block of stmts (parse_block).
            //   3. `<expr>` → a single Stmt::Expr entry (legacy).
            let body: Vec<Stmt> = match self.peek() {
                Token::Return | Token::Break | Token::Continue => {
                    let stmt = self.parse_stmt()?;
                    vec![stmt]
                }
                Token::LBrace => self.parse_block()?,
                _ => {
                    let (line, col) = self.current_pos();
                    let expr = self.expression()?;
                    vec![Stmt::Expr(expr, Span::new(line, col))]
                }
            };
            arms.push(MatchArm {
                pattern,
                guard,
                body,
            });
            // Separator between arms: comma or newline. RBrace and
            // EOF are let through — the next loop iter handles them:
            // RBrace ends the match, EOF falls into MissingClosingBrace.
            match self.peek() {
                Token::Comma => {
                    self.advance();
                }
                Token::Newline | Token::RBrace | Token::EOF => {}
                _ => {
                    return Err(self.error(
                        ErrorKind::UnexpectedToken,
                        "expected ',' or newline between match arms",
                    ));
                }
            }
        }
        Ok(Expr::Match {
            value: Box::new(value),
            arms,
            span,
        })
    }

    /// Supported patterns:
    ///   _           → Wildcard
    ///   name        → Ident(name)          (capture, matches anything)
    ///   42 / -3     → Int (with `-` for negatives)
    ///   3.14        → Float
    ///   "text"      → Str
    ///   true/false  → Bool
    ///   null        → Null
    ///   0..10       → Range (Int only; bounds may be negative)
    ///   Ok(name)    → OkBinding(name)      (runtime-blocked until Phase 3)
    ///   Err(name)   → ErrBinding(name)     (runtime-blocked until Phase 3)
    /// Parse one or more patterns separated by `|` (or-pattern,
    /// R.2.1). If only one, returns the plain pattern without
    /// wrapping in `Or`. With 2+, returns `Pattern::Or(...)`.
    ///
    /// MVP restrictions (parallel to Rust):
    ///  - **No bindings** inside or-patterns. `Ident(x)`,
    ///    `OkBinding(name)` and `ErrBinding(name)` are rejected
    ///    with a clear error citing the caveat. Suggested workaround
    ///    for the user: use `Wildcard` / `OkWildcard` /
    ///    `ErrWildcard`, or split the arm.
    fn parse_or_pattern(&mut self) -> FitzResult<Pattern> {
        let first = self.parse_pattern()?;
        if !matches!(self.peek(), Token::Pipe) {
            return Ok(first);
        }
        let mut subs = vec![first];
        while matches!(self.peek(), Token::Pipe) {
            self.advance(); // consume `|`
            let next = self.parse_pattern()?;
            subs.push(next);
        }
        // Validate MVP restrictions: no bindings in sub-patterns.
        // See the `Pattern::Or` doc comment.
        for sub in &subs {
            if matches!(
                sub,
                Pattern::Ident(_, _) | Pattern::OkBinding(_, _) | Pattern::ErrBinding(_, _)
            ) {
                return Err(self.error(
                    ErrorKind::InvalidSyntax,
                    "or-patterns do not allow bindings (use '_' or split the arm)",
                ));
            }
        }
        Ok(Pattern::Or(subs))
    }

    fn parse_pattern(&mut self) -> FitzResult<Pattern> {
        // Mini-batch T — `(p1, p2, ...)` tuple pattern. Parser
        // decision: if it starts with `(`, assume tuple pattern
        // (there is no other use of `(` in pattern position).
        // `()` → empty tuple. `(p)` without comma → in match it
        // makes no sense (a pattern in parens is equivalent to
        // `p`), but we accept it for consistency.
        if matches!(self.peek(), Token::LParen) {
            self.advance(); // consume `(`
            if matches!(self.peek(), Token::RParen) {
                self.advance();
                return Ok(Pattern::Tuple(Vec::new()));
            }
            let first = self.parse_or_pattern()?;
            if matches!(self.peek(), Token::Comma) {
                let mut subs = vec![first];
                while matches!(self.peek(), Token::Comma) {
                    self.advance();
                    if matches!(self.peek(), Token::RParen) {
                        break;
                    }
                    subs.push(self.parse_or_pattern()?);
                }
                self.expect(&Token::RParen, "expected ')' to close the tuple pattern")?;
                return Ok(Pattern::Tuple(subs));
            }
            self.expect(&Token::RParen, "expected ')' to close the pattern")?;
            return Ok(first);
        }
        // Literals. Clone the peek before advancing so we don't fight
        // the borrow checker. Ints go through `try_int_or_range` to
        // check whether `..` follows and promote to Range.
        match self.peek().clone() {
            Token::Int(n) => {
                self.advance();
                return self.try_int_or_range(n);
            }
            Token::Float(x) => {
                self.advance();
                return Ok(Pattern::Float(x));
            }
            Token::Str(s) => {
                self.advance();
                return Ok(Pattern::Str(s));
            }
            Token::True => {
                self.advance();
                return Ok(Pattern::Bool(true));
            }
            Token::False => {
                self.advance();
                return Ok(Pattern::Bool(false));
            }
            Token::Null => {
                self.advance();
                return Ok(Pattern::Null);
            }
            Token::Minus => {
                // Support for negative literals: `-42`, `-3.14`. If
                // there is no number after the `-`, error (we do not
                // accept `-x` as a pattern).
                self.advance();
                match self.peek().clone() {
                    Token::Int(n) => {
                        self.advance();
                        return self.try_int_or_range(-n);
                    }
                    Token::Float(x) => {
                        self.advance();
                        return Ok(Pattern::Float(-x));
                    }
                    _ => {
                        return Err(self.error(
                            ErrorKind::InvalidSyntax,
                            "expected number after '-' in pattern",
                        ));
                    }
                }
            }
            _ => {}
        }

        // Special cases: Ok(...) and Err(...).
        if let Token::Ident(name) = self.peek() {
            if name == "Ok" || name == "Err" {
                let is_ok = name == "Ok";
                self.advance();
                self.expect(&Token::LParen, "expected '(' after Ok/Err in pattern")?;
                let (binding, binding_span) =
                    self.expect_ident_with_span("expected identifier for Ok/Err binding")?;
                self.expect(&Token::RParen, "expected ')' at end of Ok/Err pattern")?;
                // `_` inside is a wildcard (does not bind): closes
                // the old 3.3 debt where `_` was bound as a var.
                return Ok(match (is_ok, binding.as_str()) {
                    (true, "_") => Pattern::OkWildcard,
                    (false, "_") => Pattern::ErrWildcard,
                    (true, _) => Pattern::OkBinding(binding, binding_span),
                    (false, _) => Pattern::ErrBinding(binding, binding_span),
                });
            }
        }
        // General case: identifier or wildcard.
        let (name, name_span) = self.expect_ident_with_span("expected pattern")?;
        if name == "_" {
            Ok(Pattern::Wildcard)
        } else {
            Ok(Pattern::Ident(name, name_span))
        }
    }

    /// After consuming an Int (possibly negative), peek `..` or
    /// `..=`: if present, parse the second endpoint and return
    /// `Pattern::Range`; otherwise return `Pattern::Int(start)`
    /// directly. The right endpoint also accepts `-Int`. R.1.4
    /// added `..=` support (inclusive range).
    fn try_int_or_range(&mut self, start: i64) -> FitzResult<Pattern> {
        let inclusive = match self.peek() {
            Token::DotDot => false,
            Token::DotDotEq => true,
            _ => return Ok(Pattern::Int(start)),
        };
        self.advance(); // consume '..' or '..='
        let end = match self.peek().clone() {
            Token::Int(n) => {
                self.advance();
                n
            }
            Token::Minus => {
                self.advance();
                match self.peek().clone() {
                    Token::Int(n) => {
                        self.advance();
                        -n
                    }
                    _ => {
                        return Err(self.error(
                            ErrorKind::InvalidSyntax,
                            "expected Int after '-' in range pattern",
                        ));
                    }
                }
            }
            _ => {
                return Err(self.error(
                    ErrorKind::InvalidSyntax,
                    "range pattern requires Int at both ends (Float and other types not supported)",
                ));
            }
        };
        Ok(Pattern::Range {
            start,
            end,
            inclusive,
        })
    }

    /// `type Name { field: TypeExpr [= default], ..., fn method(...) {...} }`.
    /// Separator between items: comma or newline (both accepted).
    /// Items can be **fields** (`name: TypeExpr [= default]`) or
    /// **methods** (`[async] fn name(params) [-> Ret] { body }` —
    /// R.3, mini-phase R). Trivial lookahead: `fn` or `async` →
    /// method; any other Ident → field.
    /// The field type uses the same grammar as the rest of the
    /// annotations (`parse_type_expr`): allows generics and the
    /// `?` nullable suffix. Nullability lives inside `TypeExpr` as
    /// `TypeExpr::Nullable(...)`.
    fn parse_typedef(&mut self, span: Span) -> FitzResult<Stmt> {
        self.expect(&Token::Type, "expected 'type'")?;
        let name = self.expect_ident("expected type name")?;
        self.expect(&Token::LBrace, "expected '{' after type name")?;
        let mut fields: Vec<Field> = Vec::new();
        let mut methods: Vec<MethodDef> = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), Token::RBrace) {
                self.advance();
                return Ok(Stmt::TypeDef {
                    name,
                    decorators: Vec::new(),
                    fields,
                    methods,
                    span,
                });
            }
            if self.is_at_end() {
                return Err(self.error(
                    ErrorKind::MissingClosingBrace,
                    "expected '}' to close 'type'",
                ));
            }
            // Phase 10.3.a — per-field decorators (`@primary`,
            // `@column(name="...")`, `@unique`, `@index`). Accumulate
            // before reading the target field/method. Stacking is
            // supported (`@primary @unique id: Int`).
            let mut field_decorators: Vec<Decorator> = Vec::new();
            while matches!(self.peek(), Token::At) {
                field_decorators.push(self.parse_one_decorator()?);
                self.skip_newlines();
            }
            // R.3 — instance method: `[async] fn name(...) [-> T] { ... }`.
            // Mini-batch St — static method: `static [async] fn name(...)`.
            if matches!(self.peek(), Token::Async | Token::Fn | Token::Static) {
                if !field_decorators.is_empty() {
                    return Err(self.error(
                        ErrorKind::UnexpectedToken,
                        "decorators on methods of a `type` are not supported (only on fields)",
                    ));
                }
                let method_span = self.cur_span();
                let method = self.parse_method_def(method_span)?;
                methods.push(method);
            } else {
                let field_name = self.expect_ident("expected field name or `fn`")?;
                self.expect(&Token::Colon, "expected ':' after field name")?;
                let type_ = self.parse_type_expr()?;
                let default = if self.eat(&Token::Eq) {
                    Some(self.expression()?)
                } else {
                    None
                };
                fields.push(Field {
                    name: field_name,
                    type_,
                    default,
                    decorators: field_decorators,
                });
            }
            // Optional separator: comma. Newline is consumed in the
            // next iteration by skip_newlines.
            if matches!(self.peek(), Token::Comma) {
                self.advance();
            }
        }
    }

    /// Parse a custom method inside a `type` block (R.3). Syntax
    /// identical to `parse_fndef`, but does NOT allow decorators
    /// (methods don't accept `@get`/`@server`/etc.) and emits
    /// `MethodDef` instead of `Stmt::FnDef`.
    fn parse_method_def(&mut self, span: Span) -> FitzResult<MethodDef> {
        // Mini-batch St — `static [async] fn ...` declares a static
        // method (no `self` receiver). `static` must come before
        // `async`/`fn`.
        let is_static = self.eat(&Token::Static);
        let is_async = self.eat(&Token::Async);
        self.expect(&Token::Fn, "expected 'fn'")?;
        let name = self.expect_ident("expected method name after 'fn'")?;
        self.expect(&Token::LParen, "expected '(' after method name")?;
        let params = self.parse_params()?;
        let return_type = self.parse_optional_return_type()?;
        let body = match self.peek() {
            Token::FatArrow => {
                self.advance();
                let (arrow_line, arrow_col) = self.current_pos();
                let expr = self.expression()?;
                vec![Stmt::Return(expr, Span::new(arrow_line, arrow_col))]
            }
            Token::LBrace => self.parse_block()?,
            _ => {
                return Err(self.error(
                    ErrorKind::UnexpectedToken,
                    "expected '{' or '=>' for the method body",
                ));
            }
        };
        Ok(MethodDef {
            name,
            params,
            return_type,
            body,
            is_async,
            is_static,
            span,
        })
    }

    // ---------- decorators ----------
    //
    // Shape:
    //   @name(arg1, arg2, ...)
    //   [@other_deco(...)]*
    //   [async] fn handler(...) [-> Type] { ... }
    //
    // We accumulate decorators into `Decorator { name, args, kwargs }`
    // and attach them to the resulting `Stmt::FnDef`. Semantics
    // (what each decorator actually does) is the evaluator's job:
    // the parser only guarantees they structurally come before a
    // fn, that the syntax is `@Ident(args, key=value)`, and that
    // kwargs come after positionals. Args and values are arbitrary
    // expressions — the specific decorator decides what types it
    // accepts at runtime.
    //
    // Up to 4.1 the evaluator bails with an explicit error as soon
    // as it sees non-empty decorators; 4.2 wires `@get`/`@post`/
    // `@put`/`@delete` against the HTTP runtime.

    /// Parse one or more stacked decorators (`@x @y @z`) followed by
    /// `fn`, `async fn` or `type`. Renamed from
    /// `parse_decorated_fndef` in Phase 10.3.a — it used to allow
    /// decorators only on fns; now also on `type` for
    /// `@table("users") type User { ... }` of the declarative ORM.
    fn parse_decorated_stmt(&mut self, span: Span) -> FitzResult<Stmt> {
        let mut decorators: Vec<Decorator> = Vec::new();
        // At least one: the caller entered here seeing `@`.
        loop {
            decorators.push(self.parse_one_decorator()?);
            // Allow newline between stacked decorators.
            self.skip_newlines();
            if !matches!(self.peek(), Token::At) {
                break;
            }
        }

        match self.peek() {
            Token::Fn | Token::Async => {
                let fndef = self.parse_fndef(span)?;
                match fndef {
                    Stmt::FnDef {
                        name,
                        params,
                        return_type,
                        body,
                        is_async,
                        decorators: _,
                        span,
                    } => Ok(Stmt::FnDef {
                        name,
                        params,
                        return_type,
                        body,
                        is_async,
                        decorators,
                        span,
                    }),
                    other => Ok(other),
                }
            }
            Token::Type => {
                let typedef = self.parse_typedef(span)?;
                match typedef {
                    Stmt::TypeDef {
                        name,
                        fields,
                        methods,
                        decorators: _,
                        span,
                    } => Ok(Stmt::TypeDef {
                        name,
                        decorators,
                        fields,
                        methods,
                        span,
                    }),
                    other => Ok(other),
                }
            }
            _ => Err(self.error(
                ErrorKind::UnexpectedToken,
                "after a decorator there must come `fn`, `async fn`, or `type`",
            )),
        }
    }

    /// Parse a single decorator (`@ Ident ( args )?`), with the `@`
    /// still unconsumed. Returns the ready `Decorator`; the caller
    /// decides whether to keep accumulating.
    ///
    /// Parens are **optional** since 9.z.2.a (needed for `@test fn
    /// ...` which takes no args). Decorators without parens are
    /// equivalent to `@name()` (args = kwargs = empty). Backwards-
    /// compatible: `@server()` and `@get("/x")` keep working the
    /// same.
    fn parse_one_decorator(&mut self) -> FitzResult<Decorator> {
        self.expect(&Token::At, "expected '@'")?;
        let name = self.expect_ident("expected decorator name after '@'")?;
        // If `(` follows, parse args; otherwise it's a decorator
        // without args. The next significant token picks the branch
        // (without skipping newlines — the `(` must be on the same
        // line as the name to avoid ambiguity with the next stmt).
        let (args, kwargs) = if matches!(self.peek(), Token::LParen) {
            self.advance();
            self.parse_decorator_args()?
        } else {
            (Vec::new(), Vec::new())
        };
        Ok(Decorator { name, args, kwargs })
    }

    /// Parse a decorator's arguments after the `(` is consumed.
    /// Splits positionals from kwargs. Rule:
    ///
    /// - While the next arg is a bare expression, it goes into
    ///   `args` (positional).
    /// - Kwarg detection: `Ident '='` (with `Token::Eq`, NOT
    ///   `Token::EqEq` — `a == b` is still a valid expression as a
    ///   positional arg).
    /// - Once the first kwarg is seen, **all** following args must
    ///   be kwargs; a later positional is an error.
    /// - Duplicate kwargs are an error.
    ///
    /// Ends by consuming the `)`. Accepts empty list, trailing comma
    /// and newlines between elements.
    #[allow(clippy::type_complexity)]
    fn parse_decorator_args(&mut self) -> FitzResult<(Vec<Expr>, Vec<(String, Expr)>)> {
        let mut args: Vec<Expr> = Vec::new();
        let mut kwargs: Vec<(String, Expr)> = Vec::new();
        self.skip_newlines();
        // Empty case: @deco()
        if matches!(self.peek(), Token::RParen) {
            self.advance();
            return Ok((args, kwargs));
        }
        loop {
            self.skip_newlines();
            // Kwarg detection: Ident followed by `=` (Token::Eq).
            // `==` (Token::EqEq) does NOT fire: it is a BinOp in a
            // positional expression.
            let is_kwarg =
                matches!(self.peek(), Token::Ident(_)) && matches!(self.peek_at(1), Token::Eq);
            if is_kwarg {
                let key_tok = self.advance();
                let key = match key_tok.token {
                    Token::Ident(s) => s,
                    _ => unreachable!("checked by is_kwarg"),
                };
                // Consume the `=`.
                self.advance();
                self.skip_newlines();
                let value = self.expression()?;
                // Duplicate.
                if kwargs.iter().any(|(k, _)| k == &key) {
                    return Err(self.error(
                        ErrorKind::InvalidSyntax,
                        format!(
                            "named argument '{}=' was already given in the same decorator",
                            key
                        ),
                    ));
                }
                kwargs.push((key, value));
            } else {
                // Positional. If kwargs already appeared, error.
                if !kwargs.is_empty() {
                    return Err(self.error(
                        ErrorKind::InvalidSyntax,
                        "positional arguments cannot come after \
                         named arguments (key=value)"
                            .to_string(),
                    ));
                }
                args.push(self.expression()?);
            }
            self.skip_newlines();
            if matches!(self.peek(), Token::Comma) {
                self.advance();
                self.skip_newlines();
                // Trailing comma: @deco(1, 2,)
                if matches!(self.peek(), Token::RParen) {
                    self.advance();
                    return Ok((args, kwargs));
                }
            } else {
                break;
            }
        }
        self.expect(&Token::RParen, "expected ')' to close decorator arguments")?;
        Ok((args, kwargs))
    }

    /// M5 helper (post-audit 2026-05-27) — parses a comma-separated
    /// sequence of items, terminated by `terminator`. Handles
    /// trailing comma (`[1, 2, 3,]`) and newlines between items.
    /// The caller passes the per-item parser as a closure; the
    /// helper handles the scaffolding (skip_newlines, comma,
    /// trailing comma, expecting the terminator).
    ///
    /// `terminator` must be a unit variant (`RParen`/`RBracket`/
    /// `RBrace`) — we compare them via exhaustive `matches!` in
    /// the internal dispatch.
    ///
    /// **Does NOT apply to** `parse_struct_lit_fields` (separator
    /// can be newline in addition to comma) nor to
    /// `parse_list_literal_items` (needs comprehension detection
    /// after the first item). Those keep their own loops for
    /// reasons documented in their own doc-comments.
    fn parse_comma_separated<T, F>(
        &mut self,
        terminator: &Token,
        close_msg: &str,
        mut parse_item: F,
    ) -> FitzResult<Vec<T>>
    where
        F: FnMut(&mut Self) -> FitzResult<T>,
    {
        let mut items: Vec<T> = Vec::new();
        self.skip_newlines();
        if std::mem::discriminant(self.peek()) == std::mem::discriminant(terminator) {
            self.advance();
            return Ok(items);
        }
        loop {
            self.skip_newlines();
            items.push(parse_item(self)?);
            self.skip_newlines();
            if matches!(self.peek(), Token::Comma) {
                self.advance();
                self.skip_newlines();
                if std::mem::discriminant(self.peek()) == std::mem::discriminant(terminator) {
                    self.advance();
                    return Ok(items);
                }
            } else {
                break;
            }
        }
        self.expect(terminator, close_msg)?;
        Ok(items)
    }

    /// Parse a call's arguments, with the '(' already consumed.
    /// Ends by consuming the ')'. Accepts empty list, trailing
    /// comma, and newlines between elements (handy for multiline
    /// calls).
    fn parse_call_args(&mut self) -> FitzResult<Vec<Expr>> {
        // M5 (post-audit) — the "comma + trailing comma + newlines
        // + RParen" scaffold is provided by `parse_comma_separated`.
        // The per-item logic (Fp.3: named arg vs positional + the
        // `saw_named` flag for ordering) lives in the closure.
        let mut saw_named = false;
        self.parse_comma_separated(&Token::RParen, "expected ')' to close the call", |p| {
            // Fp.3 — `name: value` with Ident + Colon lookahead.
            // Same pattern as decorator kwargs (eval already
            // does it for `@server(port=3000)`). The parser
            // does NOT check here whether the name corresponds
            // to a real param — that's done by the checker and
            // the evaluator/codegen when dispatching the call.
            let (start_line, start_col) = p.current_pos();
            let is_named =
                matches!(p.peek(), Token::Ident(_)) && matches!(p.peek_at(1), Token::Colon);
            if is_named {
                let name = p.expect_ident("expected argument name").unwrap();
                p.advance(); // consume `:`
                let value = p.expression()?;
                saw_named = true;
                Ok(Expr::NamedArg {
                    name,
                    value: Box::new(value),
                    span: Span::new(start_line, start_col),
                })
            } else {
                if saw_named {
                    return Err(p.error(
                        ErrorKind::UnexpectedToken,
                        "cannot mix positional args after named args — \
                             named args go at the end",
                    ));
                }
                p.expression()
            }
        })
    }

    /// "Leaf" expression: literal, identifier, parens, `if`,
    /// `match`, list literal `[...]` or map literal `{...}`. The
    /// downward recursion in the ladder bottoms out here.
    ///
    /// Note about `{`: in expression position it ALWAYS starts a
    /// map literal. Blocks (`fn ... { body }`, `if cond { ... }`,
    /// etc.) consume their `{` from `parse_block`/`parse_match_expr`
    /// — the flow never falls through here for those constructs.
    fn primary(&mut self) -> FitzResult<Expr> {
        // `if` and `match` are expressions — handle them before
        // consuming the token so their own parsers do it.
        match self.peek() {
            Token::If => return self.parse_if_expr(),
            Token::Match => return self.parse_match_expr(),
            // Mini-batch L — `loop { body }` as an expression. In
            // statement position, `parse_stmt` already intercepts
            // Token::Loop earlier; this branch only fires in the
            // RHS of let, args, etc. Returns `Expr::Loop { body }`.
            Token::Loop => return self.parse_loop_expr(None),
            // Mini-batch L.2 — `'label: loop { ... }` as expression.
            // Detected as Label + Colon lookahead + Loop.
            Token::Label(_)
                if matches!(self.peek_at(1), Token::Colon)
                    && matches!(self.peek_at(2), Token::Loop) =>
            {
                let label = if let Token::Label(l) = self.peek().clone() {
                    l
                } else {
                    unreachable!()
                };
                self.advance(); // consume label
                self.advance(); // consume `:`
                return self.parse_loop_expr(Some(label));
            }
            Token::LBracket => return self.parse_list_literal(),
            Token::LBrace => return self.parse_map_literal(),
            // `fn(...)` or `fn(...) => expr` — anonymous function in
            // expression position. `fn name(...)` is NOT valid here:
            // a named function is a `Stmt::FnDef`, a statement, not
            // an expression.
            Token::Fn if matches!(self.peek_at(1), Token::LParen) => {
                return self.parse_fn_expr();
            }
            // Mini-batch Async-cl — `async fn(...)` async closure in
            // expression position. Reuses `parse_fn_expr` (which
            // detects the `async` prefix and sets `is_async`).
            Token::Async
                if matches!(self.peek_at(1), Token::Fn)
                    && matches!(self.peek_at(2), Token::LParen) =>
            {
                return self.parse_fn_expr();
            }
            _ => {}
        }
        let tok = self.advance();
        let tok_span = Span::new(tok.line, tok.column);
        match tok.token {
            Token::Int(n) => Ok(Expr::Int(n, tok_span)),
            Token::Float(n) => Ok(Expr::Float(n, tok_span)),
            Token::Str(s) => build_string_expr(&s, tok.line, tok.column),
            Token::Bytes(bs) => Ok(Expr::Bytes(bs, tok_span)),
            Token::True => Ok(Expr::Bool(true, tok_span)),
            Token::False => Ok(Expr::Bool(false, tok_span)),
            Token::Null => Ok(Expr::Null(tok_span)),
            Token::Ident(name) => Ok(Expr::Ident(name, tok_span)),
            Token::LParen => {
                // Mini-batch T — distinguish:
                //   `()`        → empty tuple.
                //   `(e,)`      → 1-element tuple (trailing comma).
                //   `(e1, ...)` → tuple.
                //   `(e)`       → grouping parens only.
                //
                // Inside parens there is no ambiguity with blocks:
                // clear `no_struct_literal` to allow struct literals
                // (enables `(User { id: 1 }) == other`).
                let prev = std::mem::replace(&mut self.no_struct_literal, false);
                // Case: empty tuple `()`.
                if matches!(self.peek(), Token::RParen) {
                    self.advance();
                    self.no_struct_literal = prev;
                    return Ok(Expr::Tuple(Vec::new(), tok_span));
                }
                let first_result = self.expression();
                self.no_struct_literal = prev;
                let first = first_result?;
                // If the next is comma → tuple.
                if matches!(self.peek(), Token::Comma) {
                    let mut items = vec![first];
                    while matches!(self.peek(), Token::Comma) {
                        self.advance(); // consume `,`
                                        // Trailing comma allowed: `(e,)` or `(e1, e2,)`.
                        if matches!(self.peek(), Token::RParen) {
                            break;
                        }
                        let prev2 = std::mem::replace(&mut self.no_struct_literal, false);
                        let r = self.expression();
                        self.no_struct_literal = prev2;
                        items.push(r?);
                    }
                    self.expect(&Token::RParen, "expected ')' to close the tuple")?;
                    return Ok(Expr::Tuple(items, tok_span));
                }
                // No comma → just grouping parens.
                self.expect(&Token::RParen, "expected ')' to close parenthesis")?;
                Ok(first)
            }
            other => Err(FitzError::new(
                ErrorKind::UnexpectedToken,
                tok.line,
                tok.column,
                format!("Expected an expression, found '{:?}'", other),
            )),
        }
    }

    /// `[expr, expr, ...]` — list literal. Accepts empty `[]`,
    /// trailing comma and newlines between elements (handy for
    /// multiline lists).
    fn parse_list_literal(&mut self) -> FitzResult<Expr> {
        let span = self.cur_span();
        self.expect(&Token::LBracket, "expected '['")?;
        let prev = std::mem::replace(&mut self.no_struct_literal, false);
        let result = self.parse_list_literal_items(span);
        self.no_struct_literal = prev;
        result
    }

    fn parse_list_literal_items(&mut self, span: Span) -> FitzResult<Expr> {
        let mut items: Vec<Expr> = Vec::new();
        self.skip_newlines();
        if matches!(self.peek(), Token::RBracket) {
            self.advance();
            return Ok(Expr::List(items, span));
        }
        loop {
            self.skip_newlines();
            let first = self.expression()?;
            self.skip_newlines();
            // Mini-batch C: after parsing the first expr, if `for`
            // follows, this is a list comprehension. Only when
            // `items` is empty (we can't mix `[1, 2 for x in xs]`).
            if items.is_empty() && matches!(self.peek(), Token::For) {
                return self.parse_list_comprehension_tail(span, first);
            }
            items.push(first);
            self.skip_newlines();
            if matches!(self.peek(), Token::Comma) {
                self.advance();
                self.skip_newlines();
                if matches!(self.peek(), Token::RBracket) {
                    self.advance();
                    return Ok(Expr::List(items, span));
                }
            } else {
                break;
            }
        }
        self.expect(&Token::RBracket, "expected ']' to close the list")?;
        Ok(Expr::List(items, span))
    }

    /// Mini-batch C + Cmp+ — parse the tail of a list comprehension
    /// after the initial expr: `for <var> in <iter> [for ...]* [if cond]?]`.
    /// The leading `[` and the `expr` were already consumed by the
    /// caller. Mini-batch Cmp+ extends this for multiple `for`
    /// clauses (cartesian product); the optional trailing `if`
    /// runs inside the innermost loop.
    fn parse_list_comprehension_tail(&mut self, span: Span, expr: Expr) -> FitzResult<Expr> {
        let (var, iter, extra_clauses, filter) =
            self.parse_comprehension_clauses(&Token::RBracket, "list comprehension")?;
        self.expect(
            &Token::RBracket,
            "expected ']' to close the list comprehension",
        )?;
        Ok(Expr::ListComp {
            expr: Box::new(expr),
            var,
            iter: Box::new(iter),
            extra_clauses,
            filter,
            span,
        })
    }

    /// Mini-batch Cmp+ — parse `for <pat> in <iter>` clauses (1 or
    /// more) and an optional trailing `if <cond>`. Shares logic
    /// between list comprehension (`[expr for ...]`) and map
    /// comprehension (`{k: v for ...}`). Returns
    /// `(var, iter, extra_clauses, filter)`: the first `for` comes
    /// out separately for compatibility with the current AST shape;
    /// clauses 2+ go into `extra_clauses`. Does not consume the
    /// closing delimiter (`]` or `}`); the caller expects it.
    #[allow(clippy::type_complexity)]
    fn parse_comprehension_clauses(
        &mut self,
        _terminator: &Token,
        context: &str,
    ) -> FitzResult<(
        crate::ast::Pattern,
        Expr,
        Vec<(crate::ast::Pattern, Expr)>,
        Option<Box<Expr>>,
    )> {
        self.expect(&Token::For, format!("expected 'for' in {}", context))?;
        let var = self.parse_pattern()?;
        self.expect(
            &Token::In,
            format!("expected 'in' after variable in {}", context),
        )?;
        self.skip_newlines();
        let iter = self.expression()?;
        self.skip_newlines();

        let mut extra_clauses: Vec<(crate::ast::Pattern, Expr)> = Vec::new();
        // Multiple `for` clauses: `[expr for a in xs for b in ys]`.
        while matches!(self.peek(), Token::For) {
            self.advance(); // consume `for`
            let extra_var = self.parse_pattern()?;
            self.expect(
                &Token::In,
                format!("expected 'in' after extra variable in {}", context),
            )?;
            self.skip_newlines();
            let extra_iter = self.expression()?;
            self.skip_newlines();
            extra_clauses.push((extra_var, extra_iter));
        }

        let filter = if matches!(self.peek(), Token::If) {
            self.advance(); // consume `if`
            self.skip_newlines();
            Some(Box::new(self.expression()?))
        } else {
            None
        };
        self.skip_newlines();
        Ok((var, iter, extra_clauses, filter))
    }

    /// `{"k": v, ...}` — map literal. Accepts empty `{}`, trailing
    /// comma and newlines between pairs. The key is an expression,
    /// not a bare identifier: to use a variable's value as the key,
    /// string literals are natural (`{"name": x}`), but
    /// `{key_expr: value}` is valid if `key_expr` evaluates to
    /// something hashable at runtime.
    fn parse_map_literal(&mut self) -> FitzResult<Expr> {
        let span = self.cur_span();
        self.expect(&Token::LBrace, "expected '{'")?;
        let prev = std::mem::replace(&mut self.no_struct_literal, false);
        let result = self.parse_map_literal_pairs(span);
        self.no_struct_literal = prev;
        result
    }

    fn parse_map_literal_pairs(&mut self, span: Span) -> FitzResult<Expr> {
        let mut pairs: Vec<(Expr, Expr)> = Vec::new();
        self.skip_newlines();
        if matches!(self.peek(), Token::RBrace) {
            self.advance();
            return Ok(Expr::Map(pairs, span));
        }
        // First pair: `key: value`.
        self.skip_newlines();
        let key = self.expression()?;
        self.expect(&Token::Colon, "expected ':' between key and value in map")?;
        self.skip_newlines();
        let value = self.expression()?;
        self.skip_newlines();

        // Mini-batch Cmp+ — after the first pair, if `for` follows,
        // this is a map comprehension `{k: v for ...}`. If `,` or
        // `}` follows, it's a normal map literal and we keep parsing
        // pairs.
        if matches!(self.peek(), Token::For) {
            let (var, iter, extra_clauses, filter) =
                self.parse_comprehension_clauses(&Token::RBrace, "map comprehension")?;
            self.expect(
                &Token::RBrace,
                "expected '}' to close the map comprehension",
            )?;
            return Ok(Expr::MapComp {
                key: Box::new(key),
                value: Box::new(value),
                var,
                iter: Box::new(iter),
                extra_clauses,
                filter,
                span,
            });
        }

        pairs.push((key, value));
        // M5 (post-audit) — the remaining pairs are parsed by the
        // `parse_comma_separated` helper. The first pair is already
        // in `pairs`; to reuse the helper we simulate an alternative
        // terminator via a dedicated branch: if after the first pair
        // a `}` already follows, we bail without calling it. If `,`
        // comes, we consume it and let the helper iterate from the
        // next pair.
        self.skip_newlines();
        if matches!(self.peek(), Token::RBrace) {
            self.advance();
            return Ok(Expr::Map(pairs, span));
        }
        // Next char should be `,`. The helper expects it outside
        // the first item, so consume it manually here.
        self.expect(
            &Token::Comma,
            "expected ',' or '}' after the first map pair",
        )?;
        let rest =
            self.parse_comma_separated(&Token::RBrace, "expected '}' to close the map", |p| {
                let k = p.expression()?;
                p.expect(&Token::Colon, "expected ':' between key and value in map")?;
                p.skip_newlines();
                let v = p.expression()?;
                Ok((k, v))
            })?;
        pairs.extend(rest);
        Ok(Expr::Map(pairs, span))
    }
}

/// Public parser entry point. Converts tokens into a `Program`.
pub fn parse(tokens: Vec<TokenWithPos>) -> FitzResult<Program> {
    let mut parser = Parser::new(tokens);
    parser.parse_program()
}

/// Recovering variant of the parser. Designed for external tooling
/// (LSP, formatter, future analysis tools) that needs a partial AST
/// over in-progress buffers or buffers with transient typos. **Not**
/// used by the strict CLI (`fitz run`, `fitz build`, `fitz check`):
/// those still call `parse()` and abort on the first error.
/// Phase 9.0.1 (F15).
///
/// Rules:
///  - Captures stmt-level errors and accumulates them in the
///    returned `Vec<FitzError>`. The returned AST is always
///    structurally valid (a `Vec<Stmt>` possibly with
///    `Stmt::Error(span)` in broken places).
///  - Sync points: `Newline`, `RBrace` (not consumed), `EOF`.
///  - Hard cap: `MAX_RECOVERED_ERRORS` (100). When reached, the
///    parser drops the rest of the input and returns what it has.
///  - Errors INSIDE a stmt (unclosed paren, incomplete expression,
///    etc.) discard the whole stmt — the cursor advances to the
///    next sync point. Sub-stmt recovery is explicit debt for later.
///
/// Guarantees that it **never** returns `Err`: any error ends up
/// accumulated in the parallel list. The caller decides what to do.
///
/// `#[allow(dead_code)]`: in Phase 9.0.1 this API is exercised only
/// by tests. Real consumers (LSP, formatter, future tools) land in
/// later sub-steps of Phase 9. The allow is removed when the first
/// caller outside of tests appears.
#[allow(dead_code)]
pub fn parse_with_recovery(tokens: Vec<TokenWithPos>) -> (Program, Vec<FitzError>) {
    let mut parser = Parser::new(tokens);
    parser.recovery_mode = true;
    // In recovery mode, `parse_program` does not return `Err`
    // (errors go to `recovered_errors`); but the return type stays
    // `FitzResult` to avoid code duplication. `unwrap_or_else` is
    // a defense in case any strict-residual path slips through —
    // in that case, the error is accumulated as part of the list.
    let stmts = parser.parse_program().unwrap_or_else(|e| {
        parser.recovered_errors.push(e);
        Vec::new()
    });
    (stmts, parser.recovered_errors)
}

/// Takes the raw contents of a `Token::Str` and builds the matching
/// expression: `Expr::Str` if it's just text, or `Expr::StrInterp`
/// if it has `{...}` interpolations.
///
/// Processing rules:
///  - `\{` and `\}` are unescaped to literal `{` and `}` (the lexer
///    preserves them with the backslash so we can tell them apart
///    here).
///  - Unescaped `{ ... }` opens interpolation. The content between
///    braces is re-tokenized and parsed as an expression.
///  - A lone `}` (without a preceding `{`) is an error — the user
///    must escape it as `\}`.
///
/// Residual limitation:
///  - Strings inside interpolation are not supported: the `}`
///    scanner is naïve and gets confused by `}` inside nested
///    `"..."`.
///  - If the string contains escapes (`\n`, `\t`, etc.), the column
///    reported in interpolation errors is off by one char for every
///    escape before the error. Without access to the original
///    source we can't reconstruct the exact mapping.
fn build_string_expr(raw: &str, line: usize, column: usize) -> FitzResult<Expr> {
    let chars: Vec<char> = raw.chars().collect();
    let mut parts: Vec<StrPart> = Vec::new();
    let mut current_lit = String::new();
    let mut i = 0;
    let str_span = Span::new(line, column);

    // Column of the first char of the string content in the source:
    // the `column` we receive points at the opening quote `"`.
    let content_col = column + 1;

    while i < chars.len() {
        let c = chars[i];

        // Escape for literal '{' or '}': '\{' or '\}'.
        if c == '\\' && i + 1 < chars.len() && (chars[i + 1] == '{' || chars[i + 1] == '}') {
            current_lit.push(chars[i + 1]);
            i += 2;
            continue;
        }

        // Start of interpolation.
        if c == '{' {
            // Column of the `{` in the original source (approximate
            // — see the escape-related residual limitation).
            let interp_col = content_col + i;

            if !current_lit.is_empty() {
                parts.push(StrPart::Lit(std::mem::take(&mut current_lit)));
            }
            i += 1;
            let expr_start = i;
            // Look for closing '}'. Naïve: doesn't understand nested
            // strings — documented as debt.
            while i < chars.len() && chars[i] != '}' {
                i += 1;
            }
            if i >= chars.len() {
                return Err(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    line,
                    interp_col,
                    "String interpolation missing closing '}'",
                ));
            }
            let interp_src: String = chars[expr_start..i].iter().collect();

            // The subexpression starts one char after `{` in the source.
            let sub_col_base = interp_col + 1;

            // Mini-batch Fm — split `expr` from `:spec` at the first
            // depth-0 `:` (not inside parens/brackets/braces). This
            // lets `{m["k"]:.2f}` tell apart the spec's `:` from a
            // nested map literal's.
            let (expr_src, spec_src) = split_expr_and_format_spec(&interp_src);

            // Re-tokenize. Any sub-lexer error carries the position
            // relative to the start of expr_src — translate it to the
            // real source so the user sees the right line/column.
            let sub_tokens = tokenize(&expr_src).map_err(|mut e| {
                e.line = line;
                e.column = sub_col_base + e.column.saturating_sub(1);
                e
            })?;
            let mut sub_parser = Parser::new(sub_tokens);
            let mut expr = sub_parser.expression().map_err(|mut e| {
                e.line = line;
                e.column = sub_col_base + e.column.saturating_sub(1);
                e
            })?;
            // V1 (2026-06-05) — adjust sub-Expr spans to the original
            // source. Without this step, the resulting Expr's spans
            // carry `line=1, col=N` from the sub-tokenizer (which
            // starts at the beginning of the isolated `expr_src`).
            // That broke hover and diagnostics inside string
            // interpolation:
            //   - Hover over `{altitud_m}` returned `Str` (the type
            //     of the whole StrInterp) because the inner Ident's
            //     span was at col=1 and lost the "max col ≤ cursor"
            //     heuristic.
            //   - A checker error on `{a + b}` with `Int + Str` was
            //     reported on line 1:1 instead of the real line.
            // See `docs/deudas-post-5b.md` → V1.
            shift_expr_spans(&mut expr, line, sub_col_base);
            // Nothing should remain after the expression (beyond the
            // EOF the lexer appends).
            if !sub_parser.is_at_end() {
                return Err(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    line,
                    sub_col_base,
                    format!("Extra tokens inside interpolation: '{}'", expr_src),
                ));
            }
            // Mini-batch Fm — if there was a `:spec`, parse it into FormatSpec.
            let format_spec = if let Some(spec) = spec_src {
                Some(parse_format_spec(&spec).map_err(|msg| {
                    FitzError::new(
                        ErrorKind::InvalidSyntax,
                        line,
                        sub_col_base + expr_src.len() + 1,
                        format!("Invalid format spec `{}`: {}", spec, msg),
                    )
                })?)
            } else {
                None
            };
            parts.push(StrPart::Expr(expr, format_spec));
            i += 1; // skip '}'
            continue;
        }

        // '}' without a preceding '{' — the user probably meant to escape it.
        if c == '}' {
            return Err(FitzError::new(
                ErrorKind::InvalidSyntax,
                line,
                content_col + i,
                "stray '}' in string — escape it as '\\}' to include it literally",
            ));
        }

        current_lit.push(c);
        i += 1;
    }

    if !current_lit.is_empty() {
        parts.push(StrPart::Lit(current_lit));
    }

    // If all parts are literals (or there are no parts), return a
    // plain `Expr::Str` — nothing to interpolate. If there's at
    // least one `StrPart::Expr`, it becomes `Expr::StrInterp`.
    let has_interp = parts.iter().any(|p| matches!(p, StrPart::Expr(_, _)));
    if has_interp {
        Ok(Expr::StrInterp(parts, str_span))
    } else {
        let combined: String = parts
            .into_iter()
            .map(|p| match p {
                StrPart::Lit(s) => s,
                StrPart::Expr(_, _) => unreachable!(),
            })
            .collect();
        Ok(Expr::Str(combined, str_span))
    }
}

/// V1 (2026-06-05) — recursive walker that rewrites the `Span`s of
/// the `Expr` produced by the string-interpolation sub-parser so
/// they point at the original source instead of the isolated
/// sub-text.
///
/// For each non-ZERO `Span` of the Expr (and all its sub-Exprs):
///
/// ```text
/// new_span.line   = line               (line of the original source)
/// new_span.column = sub_col_base + (old.column - 1)
/// ```
///
/// `Span::ZERO` (the `0:0` sentinel for synthetic nodes) is
/// preserved as-is — not shifted.
///
/// **Scope**: walks only `Expr` and sub-`Expr` (including Call args,
/// If branches, Match arms, etc.). Stmts inside
/// `FnExpr.body`/`Loop.body`/etc., Match Patterns, and TypeExprs are
/// NOT walked — minor residual debt. In practice, 99% of
/// interpolations are `{ident}`, `{a + b}`, `{f(x)}`, `{x.field}`
/// which don't involve Stmts/Patterns/TypeExprs.
fn shift_expr_spans(expr: &mut Expr, line: usize, sub_col_base: usize) {
    // Rewrite the current node's span (if not ZERO).
    {
        let s = expr.span_mut();
        if s.column > 0 {
            s.line = line;
            s.column = sub_col_base + s.column.saturating_sub(1);
        }
    }
    // Recurse on sub-Exprs depending on the variant.
    match expr {
        // Literals with no sub-Exprs.
        Expr::Int(_, _)
        | Expr::Float(_, _)
        | Expr::Str(_, _)
        | Expr::Bool(_, _)
        | Expr::Null(_)
        | Expr::Bytes(_, _)
        | Expr::Ident(_, _)
        | Expr::Error(_) => {}
        // Nested StrInterp (rare but possible): walk each Expr of
        // the parts. Each was parsed by a sub-sub-parser with its
        // own base; by the time we get here they already had their
        // own shift. We make this a no-op to avoid double-shifting.
        Expr::StrInterp(_, _) => {}
        Expr::BinOp { left, right, .. } => {
            shift_expr_spans(left, line, sub_col_base);
            shift_expr_spans(right, line, sub_col_base);
        }
        Expr::UnaryOp { operand, .. } => {
            shift_expr_spans(operand, line, sub_col_base);
        }
        Expr::Call { callee, args, .. } => {
            shift_expr_spans(callee, line, sub_col_base);
            for a in args {
                shift_expr_spans(a, line, sub_col_base);
            }
        }
        Expr::NamedArg { value, .. } => {
            shift_expr_spans(value, line, sub_col_base);
        }
        Expr::FnExpr { body, .. } => {
            // body: Vec<Stmt> — residual debt (we don't walk Stmts).
            // In practice, an inline FnExpr inside a StrInterp is
            // extremely rare. If demand appears, we'll add a Stmt
            // walker in another sub-step.
            let _ = body;
        }
        Expr::Field { object, .. } => {
            shift_expr_spans(object, line, sub_col_base);
        }
        Expr::Index { object, index, .. } => {
            shift_expr_spans(object, line, sub_col_base);
            shift_expr_spans(index, line, sub_col_base);
        }
        Expr::Slice {
            object, start, end, ..
        } => {
            shift_expr_spans(object, line, sub_col_base);
            if let Some(s) = start.as_mut() {
                shift_expr_spans(s, line, sub_col_base);
            }
            if let Some(e) = end.as_mut() {
                shift_expr_spans(e, line, sub_col_base);
            }
        }
        Expr::Tuple(items, _) => {
            for it in items {
                shift_expr_spans(it, line, sub_col_base);
            }
        }
        Expr::TupleField { tuple, .. } => {
            shift_expr_spans(tuple, line, sub_col_base);
        }
        Expr::Loop { .. } => {
            // body: Vec<Stmt> — same residual debt as FnExpr.
        }
        Expr::List(items, _) => {
            for it in items {
                shift_expr_spans(it, line, sub_col_base);
            }
        }
        Expr::ListComp { .. } | Expr::MapComp { .. } => {
            // Composite — minor residual debt (very rare in interp).
        }
        Expr::Map(pairs, _) => {
            for (k, v) in pairs {
                shift_expr_spans(k, line, sub_col_base);
                shift_expr_spans(v, line, sub_col_base);
            }
        }
        Expr::Range { start, end, .. } => {
            shift_expr_spans(start, line, sub_col_base);
            shift_expr_spans(end, line, sub_col_base);
        }
        Expr::If {
            condition,
            then,
            else_,
            ..
        } => {
            shift_expr_spans(condition, line, sub_col_base);
            // then/else are Vec<Stmt> — residual debt.
            let _ = then;
            let _ = else_;
        }
        Expr::Match { value, .. } => {
            shift_expr_spans(value, line, sub_col_base);
            // arms are Vec<MatchArm> with Pattern + body — residual debt.
        }
        Expr::StructLit { fields, .. } => {
            for (_, value) in fields {
                shift_expr_spans(value, line, sub_col_base);
            }
        }
        Expr::Ok(inner, _) | Expr::Err(inner, _) | Expr::Try(inner, _) | Expr::Await(inner, _) => {
            shift_expr_spans(inner, line, sub_col_base);
        }
    }
}

/// Mini-batch Fm — split `{expr:spec}` into `(expr_src, Some(spec))`,
/// or `(expr_src, None)` if there is no spec. The split takes the
/// first `:` that is NOT inside balanced parens/brackets/braces.
fn split_expr_and_format_spec(s: &str) -> (String, Option<String>) {
    let mut depth: i32 = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ':' if depth == 0 => {
                return (s[..i].to_string(), Some(s[i + 1..].to_string()));
            }
            _ => {}
        }
    }
    (s.to_string(), None)
}

/// Mini-batch Fm — parse a Python-style format spec.
/// Grammar: `[[fill]align][sign][#][0][width][grouping][.precision][type]`.
fn parse_format_spec(s: &str) -> Result<FormatSpec, String> {
    use crate::ast::{FormatKind, FormatSign};
    let mut spec = FormatSpec::default();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;

    // fill + align: 2 chars where the second one is align.
    if chars.len() >= 2 {
        if let Some(a) = align_from_char(chars[1]) {
            spec.fill = Some(chars[0]);
            spec.align = Some(a);
            i = 2;
        }
    }
    // align only.
    if spec.align.is_none() && i < chars.len() {
        if let Some(a) = align_from_char(chars[i]) {
            spec.align = Some(a);
            i += 1;
        }
    }
    // sign.
    if i < chars.len() {
        match chars[i] {
            '+' => {
                spec.sign = Some(FormatSign::Plus);
                i += 1;
            }
            '-' => {
                spec.sign = Some(FormatSign::Minus);
                i += 1;
            }
            ' ' => {
                spec.sign = Some(FormatSign::Space);
                i += 1;
            }
            _ => {}
        }
    }
    if i < chars.len() && chars[i] == '#' {
        spec.alternate = true;
        i += 1;
    }
    if i < chars.len() && chars[i] == '0' {
        spec.zero_pad = true;
        i += 1;
    }
    let width_start = i;
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    if i > width_start {
        let width_str: String = chars[width_start..i].iter().collect();
        spec.width = Some(
            width_str
                .parse::<usize>()
                .map_err(|_| format!("invalid width: `{}`", width_str))?,
        );
    }
    if i < chars.len() && (chars[i] == ',' || chars[i] == '_') {
        spec.grouping = Some(chars[i]);
        i += 1;
    }
    if i < chars.len() && chars[i] == '.' {
        i += 1;
        let prec_start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        if i == prec_start {
            return Err("precision after `.` requires at least one digit".into());
        }
        let prec_str: String = chars[prec_start..i].iter().collect();
        spec.precision = Some(
            prec_str
                .parse::<usize>()
                .map_err(|_| format!("invalid precision: `{}`", prec_str))?,
        );
    }
    if i < chars.len() {
        let k = match chars[i] {
            'b' => FormatKind::Binary,
            'c' => FormatKind::Char,
            'd' => FormatKind::Decimal,
            'e' => FormatKind::ExponentLower,
            'E' => FormatKind::ExponentUpper,
            'f' => FormatKind::FixedLower,
            'F' => FormatKind::FixedUpper,
            'g' => FormatKind::GeneralLower,
            'G' => FormatKind::GeneralUpper,
            'o' => FormatKind::Octal,
            's' => FormatKind::String,
            'x' => FormatKind::HexLower,
            'X' => FormatKind::HexUpper,
            '%' => FormatKind::Percent,
            other => return Err(format!("type char desconocido: `{}`", other)),
        };
        spec.kind = Some(k);
        i += 1;
    }
    if i != chars.len() {
        return Err(format!(
            "caracteres sobrantes tras el type char: `{}`",
            &s[i..]
        ));
    }
    Ok(spec)
}

fn align_from_char(c: char) -> Option<crate::ast::FormatAlign> {
    use crate::ast::FormatAlign;
    match c {
        '<' => Some(FormatAlign::Left),
        '>' => Some(FormatAlign::Right),
        '^' => Some(FormatAlign::Center),
        '=' => Some(FormatAlign::Pad),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests — Parser helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::approx_constant)] // 3.14 in tests is a generic Float, not PI.
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    /// Helper: tokenize the source and create a Parser ready for tests.
    fn parser(src: &str) -> Parser {
        let tokens = tokenize(src).expect("source must tokenize without error");
        Parser::new(tokens)
    }

    /// Phase 10.3.a helper: parse the whole source into a `Program`,
    /// assuming it's valid. Useful for decorator tests that need to
    /// see the whole AST.
    fn parse_ok(src: &str) -> Program {
        let tokens = tokenize(src).expect("source must tokenize without error");
        parse(tokens).expect("source must parse without error")
    }

    #[test]
    fn peek_returns_current_token_without_advancing() {
        let p = parser("42 + 1");
        assert_eq!(*p.peek(), Token::Int(42));
        // Second call: same token, did not consume.
        assert_eq!(*p.peek(), Token::Int(42));
    }

    #[test]
    fn peek_at_supports_lookahead() {
        let p = parser("x = 42");
        assert_eq!(*p.peek_at(0), Token::Ident("x".into()));
        assert_eq!(*p.peek_at(1), Token::Eq);
        assert_eq!(*p.peek_at(2), Token::Int(42));
    }

    #[test]
    fn peek_past_end_returns_eof() {
        let p = parser("");
        assert_eq!(*p.peek(), Token::EOF);
        assert_eq!(*p.peek_at(5), Token::EOF);
    }

    #[test]
    fn advance_moves_cursor_forward() {
        let mut p = parser("42 + 1");
        let first = p.advance();
        assert_eq!(first.token, Token::Int(42));
        assert_eq!(*p.peek(), Token::Plus);
    }

    #[test]
    fn advance_at_eof_is_idempotent() {
        let mut p = parser("");
        assert!(p.is_at_end());
        // Even if we call advance several times, we stay at EOF.
        p.advance();
        p.advance();
        assert!(p.is_at_end());
        assert_eq!(*p.peek(), Token::EOF);
    }

    #[test]
    fn check_compares_variant_and_payload() {
        let p = parser("42");
        assert!(p.check(&Token::Int(42)));
        assert!(!p.check(&Token::Int(99)));
    }

    #[test]
    fn eat_consumes_only_on_match() {
        let mut p = parser("+ -");
        assert!(p.eat(&Token::Plus));
        // No match: does not consume.
        assert!(!p.eat(&Token::Plus));
        assert!(p.eat(&Token::Minus));
        assert!(p.is_at_end());
    }

    #[test]
    fn expect_returns_ok_on_match() {
        let mut p = parser("(");
        assert!(p.expect(&Token::LParen, "expected '('").is_ok());
        assert!(p.is_at_end());
    }

    #[test]
    fn expect_returns_err_with_token_position_on_mismatch() {
        let mut p = parser("42");
        let err = p.expect(&Token::LParen, "expected '('").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
        assert_eq!(err.line, 1);
        assert_eq!(err.column, 1);
        assert!(err.message.contains("expected '('"));
    }

    #[test]
    fn expect_ident_extracts_name() {
        let mut p = parser("user");
        let name = p.expect_ident("expected identifier").unwrap();
        assert_eq!(name, "user");
        assert!(p.is_at_end());
    }

    #[test]
    fn expect_ident_fails_on_keyword() {
        // 'fn' is a keyword, not an Ident — should fail.
        let mut p = parser("fn");
        let err = p.expect_ident("expected identifier").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn skip_newlines_consumes_runs() {
        let mut p = parser("\n\n\n42");
        p.skip_newlines();
        assert_eq!(*p.peek(), Token::Int(42));
    }

    #[test]
    fn skip_newlines_is_noop_when_no_newline() {
        let mut p = parser("42");
        p.skip_newlines();
        assert_eq!(*p.peek(), Token::Int(42));
    }

    #[test]
    fn current_pos_tracks_token_position() {
        let mut p = parser("let\n  x");
        // Before consuming: Let at (1, 1)
        assert_eq!(p.current_pos(), (1, 1));
        p.advance(); // consume Let
                     // Next token: Newline at (1, 4)
        assert_eq!(p.current_pos(), (1, 4));
        p.advance(); // consume Newline
                     // Next token: Ident("x") at (2, 3)
        assert_eq!(p.current_pos(), (2, 3));
    }

    #[test]
    fn parse_empty_source_returns_empty_program() {
        let tokens = tokenize("").unwrap();
        let program = parse(tokens).unwrap();
        assert!(program.is_empty());
    }

    // -----------------------------------------------------------------------
    // Tests — Span on Expr (S1.2 sub-step 1)
    //
    // The parser propagates `Span { line, column }` to every `Expr`
    // node. These tests pin the call sites of the 5 most checker-
    // visible rules (literal, BinOp, Call, Field, Index) and make
    // any refactor that loses spans show up in the suite. They
    // compare positions explicitly (not via `assert_eq!` on `Expr`
    // — `Span::PartialEq` is always trivial, so the only way to
    // validate the position is to look at `.span().line` /
    // `.span().column` directly).
    // -----------------------------------------------------------------------

    #[test]
    fn span_literal_points_to_first_token() {
        // In `  42`, the `42` starts at column 3 (1-indexed). The
        // `Expr::Int` node's span reuses the literal token's
        // position.
        let e = parse_expr("  42").unwrap();
        let s = e.span();
        assert_eq!(s.line, 1);
        assert_eq!(s.column, 3);
        // Sanity: also for Str and Ident.
        let e = parse_expr("\"hola\"").unwrap();
        assert_eq!(e.span().column, 1);
        let e = parse_expr("user").unwrap();
        assert_eq!(e.span().column, 1);
    }

    #[test]
    fn span_binop_points_to_operator_not_left() {
        // In `1 + 2`, the `+` is at column 3. The `Expr::BinOp` span
        // must point at the operator (rustc/clang convention). The
        // `left` (`Expr::Int(1)`) carries its own span at column 1.
        let e = parse_expr("1 + 2").unwrap();
        let outer = e.span();
        assert_eq!(outer.line, 1);
        assert_eq!(outer.column, 3);
        if let Expr::BinOp { left, .. } = &e {
            // The `left` sub-node keeps its own span.
            assert_eq!(left.span().column, 1);
        } else {
            panic!("expected BinOp, got {:?}", e);
        }
    }

    #[test]
    fn span_call_points_to_opening_paren() {
        // In `f(1, 2)`, the `(` is at column 2. The `Expr::Call` span
        // must point at the `(`, not at the callee (which keeps its
        // own span at column 1).
        let e = parse_expr("f(1, 2)").unwrap();
        assert_eq!(e.span().column, 2);
        if let Expr::Call { callee, .. } = &e {
            assert_eq!(callee.span().column, 1);
        } else {
            panic!("expected Call, got {:?}", e);
        }
    }

    #[test]
    fn span_field_points_to_dot() {
        // In `user.name`, the `.` is at column 5. The `Expr::Field`
        // span points at the `.`; the receiver keeps its span at
        // column 1.
        let e = parse_expr("user.name").unwrap();
        assert_eq!(e.span().column, 5);
        if let Expr::Field { object, .. } = &e {
            assert_eq!(object.span().column, 1);
        } else {
            panic!("expected Field, got {:?}", e);
        }
    }

    #[test]
    fn span_index_points_to_bracket() {
        // In `xs[0]`, the `[` is at column 3. The `Expr::Index` span
        // points at the `[`; the receiver keeps its span at column 1
        // and the index at column 4.
        let e = parse_expr("xs[0]").unwrap();
        assert_eq!(e.span().column, 3);
        if let Expr::Index { object, index, .. } = &e {
            assert_eq!(object.span().column, 1);
            assert_eq!(index.span().column, 4);
        } else {
            panic!("expected Index, got {:?}", e);
        }
    }

    // -----------------------------------------------------------------------
    // Tests — `.await` postfix (Phase 6.1)
    //
    // The parser builds `Expr::Await(inner, span)` when it sees
    // `.await` after any postfix expression. The `await` keyword is
    // already tokenized as `Token::Await` from before Phase 6
    // (dormant token). The checker/evaluator/codegen reject the node
    // with an explicit error until 6.2/6.4/6.6; the barrier tests
    // live in `types.rs`, `evaluator.rs` and `codegen.rs`.
    // -----------------------------------------------------------------------

    #[test]
    fn await_postfix_wraps_ident_receiver() {
        let e = parse_expr("x.await").unwrap();
        match e {
            Expr::Await(inner, _) => {
                assert_eq!(*inner, Expr::Ident("x".into(), Span::ZERO));
            }
            other => panic!("expected Await, got {:?}", other),
        }
    }

    #[test]
    fn await_postfix_wraps_call() {
        // `f(x).await` → Await(Call(...))
        let e = parse_expr("f(x).await").unwrap();
        match e {
            Expr::Await(inner, _) => {
                assert!(
                    matches!(*inner, Expr::Call { .. }),
                    "expected Await(Call), inner was {:?}",
                    inner
                );
            }
            other => panic!("expected Await, got {:?}", other),
        }
    }

    #[test]
    fn await_chains_with_method_chain() {
        // `xs.map(f).await` → Await(Call(callee=Field(xs, "map"), args=[f]))
        let e = parse_expr("xs.map(f).await").unwrap();
        match e {
            Expr::Await(inner, _) => match *inner {
                Expr::Call { callee, .. } => {
                    assert!(matches!(*callee, Expr::Field { .. }));
                }
                other => panic!("expected Call inside Await, was {:?}", other),
            },
            other => panic!("expected Await, got {:?}", other),
        }
    }

    #[test]
    fn await_followed_by_try_is_try_of_await() {
        // `expr.await?` → Try(Await(expr))
        // The postfix loop processes `.await` first, then `?`.
        let e = parse_expr("x.await?").unwrap();
        match e {
            Expr::Try(inner, _) => {
                assert!(
                    matches!(*inner, Expr::Await(..)),
                    "expected Try(Await(..)), was {:?}",
                    inner
                );
            }
            other => panic!("expected Try, got {:?}", other),
        }
    }

    #[test]
    fn await_followed_by_field_is_field_of_await() {
        // `expr.await.name` → Field(Await(expr), "name")
        let e = parse_expr("x.await.name").unwrap();
        match e {
            Expr::Field { object, field, .. } => {
                assert_eq!(field, "name");
                assert!(matches!(*object, Expr::Await(..)));
            }
            other => panic!("expected Field, got {:?}", other),
        }
    }

    #[test]
    fn double_await_nests_awaits() {
        // `x.await.await` → Await(Await(x))
        let e = parse_expr("x.await.await").unwrap();
        match e {
            Expr::Await(outer_inner, _) => match *outer_inner {
                Expr::Await(inner, _) => {
                    assert_eq!(*inner, Expr::Ident("x".into(), Span::ZERO));
                }
                other => panic!("expected nested Await, was {:?}", other),
            },
            other => panic!("expected outer Await, got {:?}", other),
        }
    }

    #[test]
    fn await_span_points_to_dot() {
        // In `user.await`, the `.` is at column 5. The `Expr::Await`
        // node's span points at the `.` (parallel to `Field`).
        let e = parse_expr("user.await").unwrap();
        assert_eq!(e.span().line, 1);
        assert_eq!(e.span().column, 5);
        if let Expr::Await(inner, _) = &e {
            // The receiver keeps its own span at column 1.
            assert_eq!(inner.span().column, 1);
        } else {
            panic!("expected Await, got {:?}", e);
        }
    }

    #[test]
    fn future_as_type_annotation_parses_as_generic() {
        // `Future<T>` reuses `TypeExpr::Generic` just like `List<T>`
        // — no new AST variant needed. This test pins the 6.1
        // decision: if a dedicated `TypeExpr::Future` is added in
        // the future, this test changes explicitly.
        let tokens = tokenize("fn f() -> Future<Int> => 0").expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let stmt = program.into_iter().next().expect("al menos 1 stmt");
        match stmt {
            Stmt::FnDef {
                return_type: Some(TypeExpr::Generic { name, args }),
                ..
            } => {
                assert_eq!(name, "Future");
                assert_eq!(args.len(), 1);
                assert!(matches!(&args[0], TypeExpr::Named(n) if n == "Int"));
            }
            other => panic!("expected FnDef with return Future<Int>, was {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Tests — expressions (step 2: precedence ladder)
    // -----------------------------------------------------------------------

    /// Helper: parse a single expression from the source.
    fn parse_expr(src: &str) -> FitzResult<Expr> {
        let mut p = parser(src);
        p.expression()
    }

    #[test]
    fn primary_literals() {
        assert_eq!(parse_expr("42").unwrap(), Expr::Int(42, Span::ZERO));
        assert_eq!(parse_expr("3.14").unwrap(), Expr::Float(3.14, Span::ZERO));
        assert_eq!(
            parse_expr(r#""hola""#).unwrap(),
            Expr::Str("hola".into(), Span::ZERO)
        );
        assert_eq!(parse_expr("true").unwrap(), Expr::Bool(true, Span::ZERO));
        assert_eq!(parse_expr("false").unwrap(), Expr::Bool(false, Span::ZERO));
        assert_eq!(parse_expr("null").unwrap(), Expr::Null(Span::ZERO));
    }

    #[test]
    fn primary_identifier() {
        assert_eq!(
            parse_expr("user").unwrap(),
            Expr::Ident("user".into(), Span::ZERO)
        );
    }

    #[test]
    fn primary_parens_pass_through_without_node() {
        // (42) parses as Int(42) — parens add no node to the AST,
        // they only control precedence.
        assert_eq!(parse_expr("(42)").unwrap(), Expr::Int(42, Span::ZERO));
    }

    #[test]
    fn primary_unclosed_paren_errors() {
        let err = parse_expr("(42").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn primary_errors_on_unexpected_token() {
        // A lone ')' does not start any valid expression.
        let err = parse_expr(")").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn binary_addition_basic() {
        assert_eq!(
            parse_expr("1 + 2").unwrap(),
            Expr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(Expr::Int(1, Span::ZERO)),
                right: Box::new(Expr::Int(2, Span::ZERO)),
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn binary_subtraction_is_left_associative() {
        // 1 - 2 - 3 → (1 - 2) - 3
        assert_eq!(
            parse_expr("1 - 2 - 3").unwrap(),
            Expr::BinOp {
                op: BinOpKind::Sub,
                left: Box::new(Expr::BinOp {
                    op: BinOpKind::Sub,
                    left: Box::new(Expr::Int(1, Span::ZERO)),
                    right: Box::new(Expr::Int(2, Span::ZERO)),
                    span: Span::ZERO,
                }),
                right: Box::new(Expr::Int(3, Span::ZERO)),
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn mul_has_higher_precedence_than_add() {
        // 1 + 2 * 3 → 1 + (2 * 3)
        assert_eq!(
            parse_expr("1 + 2 * 3").unwrap(),
            Expr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(Expr::Int(1, Span::ZERO)),
                right: Box::new(Expr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(Expr::Int(2, Span::ZERO)),
                    right: Box::new(Expr::Int(3, Span::ZERO)),
                    span: Span::ZERO,
                }),
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn parens_override_precedence() {
        // (1 + 2) * 3
        assert_eq!(
            parse_expr("(1 + 2) * 3").unwrap(),
            Expr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Int(1, Span::ZERO)),
                    right: Box::new(Expr::Int(2, Span::ZERO)),
                    span: Span::ZERO,
                }),
                right: Box::new(Expr::Int(3, Span::ZERO)),
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn comparison_has_lower_precedence_than_arithmetic() {
        // 1 + 2 < 5 → (1 + 2) < 5
        assert_eq!(
            parse_expr("1 + 2 < 5").unwrap(),
            Expr::BinOp {
                op: BinOpKind::Lt,
                left: Box::new(Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Int(1, Span::ZERO)),
                    right: Box::new(Expr::Int(2, Span::ZERO)),
                    span: Span::ZERO,
                }),
                right: Box::new(Expr::Int(5, Span::ZERO)),
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn equality_has_lower_precedence_than_comparison() {
        // 1 < 2 == true → (1 < 2) == true
        assert_eq!(
            parse_expr("1 < 2 == true").unwrap(),
            Expr::BinOp {
                op: BinOpKind::Eq,
                left: Box::new(Expr::BinOp {
                    op: BinOpKind::Lt,
                    left: Box::new(Expr::Int(1, Span::ZERO)),
                    right: Box::new(Expr::Int(2, Span::ZERO)),
                    span: Span::ZERO,
                }),
                right: Box::new(Expr::Bool(true, Span::ZERO)),
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn unary_neg_wraps_operand() {
        assert_eq!(
            parse_expr("-5").unwrap(),
            Expr::UnaryOp {
                op: UnaryOpKind::Neg,
                operand: Box::new(Expr::Int(5, Span::ZERO)),
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn unary_neg_has_higher_precedence_than_mul() {
        // -5 * 3 → (-5) * 3
        assert_eq!(
            parse_expr("-5 * 3").unwrap(),
            Expr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(Expr::UnaryOp {
                    op: UnaryOpKind::Neg,
                    operand: Box::new(Expr::Int(5, Span::ZERO)),
                    span: Span::ZERO,
                }),
                right: Box::new(Expr::Int(3, Span::ZERO)),
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn double_unary_neg_nests() {
        // --x → -(-x)
        assert_eq!(
            parse_expr("--x").unwrap(),
            Expr::UnaryOp {
                op: UnaryOpKind::Neg,
                operand: Box::new(Expr::UnaryOp {
                    op: UnaryOpKind::Neg,
                    operand: Box::new(Expr::Ident("x".into(), Span::ZERO)),
                    span: Span::ZERO,
                }),
                span: Span::ZERO,
            }
        );
    }

    // ---------------- R.1.1 — `not` (mini-phase R) ----------------

    #[test]
    fn unary_not_parses_over_bool_literal() {
        assert_eq!(
            parse_expr("not true").unwrap(),
            Expr::UnaryOp {
                op: UnaryOpKind::Not,
                operand: Box::new(Expr::Bool(true, Span::ZERO)),
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn unary_not_parses_over_ident() {
        assert_eq!(
            parse_expr("not active").unwrap(),
            Expr::UnaryOp {
                op: UnaryOpKind::Not,
                operand: Box::new(Expr::Ident("active".into(), Span::ZERO)),
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn double_unary_not_nests() {
        // not not x → not(not x)
        assert_eq!(
            parse_expr("not not x").unwrap(),
            Expr::UnaryOp {
                op: UnaryOpKind::Not,
                operand: Box::new(Expr::UnaryOp {
                    op: UnaryOpKind::Not,
                    operand: Box::new(Expr::Ident("x".into(), Span::ZERO)),
                    span: Span::ZERO,
                }),
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn unary_not_has_higher_precedence_than_eq() {
        // `not x == y` → `(not x) == y` (left-to-right associativity
        // of unary, higher precedence than ==).
        let expr = parse_expr("not x == y").unwrap();
        // The root node is BinOp Eq with left = UnaryOp Not.
        match expr {
            Expr::BinOp { op, left, .. } => {
                assert_eq!(op, BinOpKind::Eq);
                match *left {
                    Expr::UnaryOp {
                        op: UnaryOpKind::Not,
                        ..
                    } => {}
                    other => panic!("expected UnaryOp Not, was {:?}", other),
                }
            }
            other => panic!("expected BinOp Eq, was {:?}", other),
        }
    }

    #[test]
    fn unary_not_in_if_condition() {
        // `if not active { ... }` parsea OK.
        let stmt = parse_one_stmt("if (not active) { print(\"x\") }");
        match stmt {
            Stmt::Assign { .. } | Stmt::Expr(_, _) => {
                // Stmt::If se modela como Stmt::Expr(Expr::If, _).
            }
            other => panic!("expected Stmt::Expr(If), was {:?}", other),
        }
    }

    // ---------------- R.1.2 — `%` operator (mini-phase R) ----------------

    #[test]
    fn op_modulo_parses_with_same_precedence_as_mul() {
        // 10 + 3 % 2 → 10 + (3 % 2)
        let expr = parse_expr("10 + 3 % 2").unwrap();
        match expr {
            Expr::BinOp {
                op: BinOpKind::Add,
                right,
                ..
            } => match *right {
                Expr::BinOp {
                    op: BinOpKind::Mod, ..
                } => {}
                other => panic!("expected BinOp Mod in right, was {:?}", other),
            },
            other => panic!("expected BinOp Add root, was {:?}", other),
        }
    }

    #[test]
    fn op_modulo_left_associative_with_mul() {
        // 10 % 3 * 2 → (10 % 3) * 2 (left-to-right between same
        // precedence levels).
        let expr = parse_expr("10 % 3 * 2").unwrap();
        match expr {
            Expr::BinOp {
                op: BinOpKind::Mul,
                left,
                ..
            } => match *left {
                Expr::BinOp {
                    op: BinOpKind::Mod, ..
                } => {}
                other => panic!("expected BinOp Mod in left, was {:?}", other),
            },
            other => panic!("expected BinOp Mul root, was {:?}", other),
        }
    }

    #[test]
    fn op_modulo_simple() {
        let expr = parse_expr("7 % 3").unwrap();
        assert!(matches!(
            expr,
            Expr::BinOp {
                op: BinOpKind::Mod,
                ..
            }
        ));
    }

    // ---------------- R.1.3 — index assignment (mini-phase R) ----------------

    #[test]
    fn assign_index_list_parses() {
        let stmt = parse_one_stmt("xs[0] = 99");
        match stmt {
            Stmt::Assign {
                target: AssignTarget::Index { object, index },
                value,
                ..
            } => {
                assert!(matches!(*object, Expr::Ident(ref n, _) if n == "xs"));
                assert!(matches!(*index, Expr::Int(0, _)));
                assert!(matches!(value, Expr::Int(99, _)));
            }
            other => panic!("expected Stmt::Assign Index, was {:?}", other),
        }
    }

    #[test]
    fn assign_index_map_str_key_parses() {
        let stmt = parse_one_stmt("m[\"a\"] = 10");
        match stmt {
            Stmt::Assign {
                target: AssignTarget::Index { object, index },
                value,
                ..
            } => {
                assert!(matches!(*object, Expr::Ident(ref n, _) if n == "m"));
                assert!(matches!(*index, Expr::Str(ref s, _) if s == "a"));
                assert!(matches!(value, Expr::Int(10, _)));
            }
            other => panic!("expected Stmt::Assign Index, was {:?}", other),
        }
    }

    #[test]
    fn assign_index_with_complex_expression_as_index() {
        // xs[i + 1] = ...
        let stmt = parse_one_stmt("xs[i + 1] = 99");
        match stmt {
            Stmt::Assign {
                target: AssignTarget::Index { index, .. },
                ..
            } => {
                assert!(matches!(
                    *index,
                    Expr::BinOp {
                        op: BinOpKind::Add,
                        ..
                    }
                ));
            }
            other => panic!("expected Stmt::Assign Index, was {:?}", other),
        }
    }

    // ---------------- R.1.4 — inclusive ranges `..=` (mini-phase R) ----------------

    #[test]
    fn range_inclusive_expr_parses() {
        let expr = parse_expr("0..=10").unwrap();
        match expr {
            Expr::Range {
                start,
                end,
                inclusive,
                ..
            } => {
                assert!(matches!(*start, Expr::Int(0, _)));
                assert!(matches!(*end, Expr::Int(10, _)));
                assert!(inclusive, "..= must parse as inclusive");
            }
            other => panic!("expected Expr::Range, was {:?}", other),
        }
    }

    #[test]
    fn range_exclusive_still_works() {
        let expr = parse_expr("0..10").unwrap();
        match expr {
            Expr::Range { inclusive, .. } => {
                assert!(!inclusive, ".. (without =) must parse as exclusive");
            }
            other => panic!("expected Expr::Range, was {:?}", other),
        }
    }

    #[test]
    fn range_inclusive_pattern_in_match() {
        // 0..=59 as a match pattern.
        let stmt = parse_one_stmt("let r = match n { 0..=59 => \"F\", _ => \"otro\" }");
        match stmt {
            Stmt::Assign {
                value: Expr::Match { arms, .. },
                ..
            } => match &arms[0].pattern {
                Pattern::Range {
                    start: 0,
                    end: 59,
                    inclusive: true,
                } => {}
                other => panic!("expected inclusive Range 0..=59, was {:?}", other),
            },
            other => panic!("expected Stmt::Assign with Match, was {:?}", other),
        }
    }

    #[test]
    fn unary_neg_applies_to_parenthesized_expression() {
        // -(1 + 2)
        assert_eq!(
            parse_expr("-(1 + 2)").unwrap(),
            Expr::UnaryOp {
                op: UnaryOpKind::Neg,
                operand: Box::new(Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Int(1, Span::ZERO)),
                    right: Box::new(Expr::Int(2, Span::ZERO)),
                    span: Span::ZERO,
                }),
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn not_equal_operator() {
        assert_eq!(
            parse_expr("x != y").unwrap(),
            Expr::BinOp {
                op: BinOpKind::NotEq,
                left: Box::new(Expr::Ident("x".into(), Span::ZERO)),
                right: Box::new(Expr::Ident("y".into(), Span::ZERO)),
                span: Span::ZERO,
            }
        );
    }

    // -----------------------------------------------------------------------
    // Tests — postfix (step 3: field access and call)
    // -----------------------------------------------------------------------

    #[test]
    fn field_access_simple() {
        assert_eq!(
            parse_expr("user.name").unwrap(),
            Expr::Field {
                object: Box::new(Expr::Ident("user".into(), Span::ZERO)),
                field: "name".into(),
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn field_access_is_left_associative_when_chained() {
        // user.profile.email → Field(Field(user, profile), email)
        assert_eq!(
            parse_expr("user.profile.email").unwrap(),
            Expr::Field {
                object: Box::new(Expr::Field {
                    object: Box::new(Expr::Ident("user".into(), Span::ZERO)),
                    field: "profile".into(),
                    span: Span::ZERO,
                }),
                field: "email".into(),
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn field_access_missing_name_errors() {
        let err = parse_expr("user.").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn call_no_args() {
        assert_eq!(
            parse_expr("hello()").unwrap(),
            Expr::Call {
                callee: Box::new(Expr::Ident("hello".into(), Span::ZERO)),
                args: vec![],
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn call_single_arg() {
        assert_eq!(
            parse_expr("print(42)").unwrap(),
            Expr::Call {
                callee: Box::new(Expr::Ident("print".into(), Span::ZERO)),
                args: vec![Expr::Int(42, Span::ZERO)],
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn call_multiple_args() {
        assert_eq!(
            parse_expr("sum(1, 2, 3)").unwrap(),
            Expr::Call {
                callee: Box::new(Expr::Ident("sum".into(), Span::ZERO)),
                args: vec![
                    Expr::Int(1, Span::ZERO),
                    Expr::Int(2, Span::ZERO),
                    Expr::Int(3, Span::ZERO)
                ],
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn call_with_trailing_comma() {
        // Trailing comma allowed — handy for clean diffs.
        assert_eq!(
            parse_expr("sum(1, 2,)").unwrap(),
            Expr::Call {
                callee: Box::new(Expr::Ident("sum".into(), Span::ZERO)),
                args: vec![Expr::Int(1, Span::ZERO), Expr::Int(2, Span::ZERO)],
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn call_with_newlines_inside_parens() {
        // Inside '(' ... ')' newlines are ignored.
        let src = "sum(\n  1,\n  2,\n  3\n)";
        assert_eq!(
            parse_expr(src).unwrap(),
            Expr::Call {
                callee: Box::new(Expr::Ident("sum".into(), Span::ZERO)),
                args: vec![
                    Expr::Int(1, Span::ZERO),
                    Expr::Int(2, Span::ZERO),
                    Expr::Int(3, Span::ZERO)
                ],
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn call_with_complex_arg_expression() {
        // print(1 + 2 * 3)
        assert_eq!(
            parse_expr("print(1 + 2 * 3)").unwrap(),
            Expr::Call {
                callee: Box::new(Expr::Ident("print".into(), Span::ZERO)),
                args: vec![Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Int(1, Span::ZERO)),
                    right: Box::new(Expr::BinOp {
                        op: BinOpKind::Mul,
                        left: Box::new(Expr::Int(2, Span::ZERO)),
                        right: Box::new(Expr::Int(3, Span::ZERO)),
                        span: Span::ZERO,
                    }),
                    span: Span::ZERO,
                }],
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn nested_call() {
        // print(double(x))
        assert_eq!(
            parse_expr("print(double(x))").unwrap(),
            Expr::Call {
                callee: Box::new(Expr::Ident("print".into(), Span::ZERO)),
                args: vec![Expr::Call {
                    callee: Box::new(Expr::Ident("double".into(), Span::ZERO)),
                    args: vec![Expr::Ident("x".into(), Span::ZERO)],
                    span: Span::ZERO,
                }],
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn call_unclosed_paren_errors() {
        let err = parse_expr("f(1, 2").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn method_call_parses_as_call_with_field_callee() {
        // `foo.bar()` now parses: `Call { callee: Field { foo, bar }, args: [] }`.
        // Previously the parser errored (explicit debt from 2.3). The
        // method dispatch itself is checked by the evaluator.
        let expr = parse_expr("foo.bar()").unwrap();
        assert_eq!(
            expr,
            Expr::Call {
                callee: Box::new(Expr::Field {
                    object: Box::new(Expr::Ident("foo".into(), Span::ZERO)),
                    field: "bar".into(),
                    span: Span::ZERO,
                }),
                args: vec![],
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn call_combines_with_arithmetic_precedence() {
        // 1 + f(2) * 3 → 1 + (f(2) * 3)
        assert_eq!(
            parse_expr("1 + f(2) * 3").unwrap(),
            Expr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(Expr::Int(1, Span::ZERO)),
                right: Box::new(Expr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(Expr::Call {
                        callee: Box::new(Expr::Ident("f".into(), Span::ZERO)),
                        args: vec![Expr::Int(2, Span::ZERO)],
                        span: Span::ZERO,
                    }),
                    right: Box::new(Expr::Int(3, Span::ZERO)),
                    span: Span::ZERO,
                }),
                span: Span::ZERO,
            }
        );
    }

    #[test]
    fn unary_neg_binds_tighter_than_postfix() {
        // -foo.bar → -(foo.bar)  (postfix has higher precedence than unary)
        assert_eq!(
            parse_expr("-foo.bar").unwrap(),
            Expr::UnaryOp {
                op: UnaryOpKind::Neg,
                operand: Box::new(Expr::Field {
                    object: Box::new(Expr::Ident("foo".into(), Span::ZERO)),
                    field: "bar".into(),
                    span: Span::ZERO,
                }),
                span: Span::ZERO,
            }
        );
    }

    // -----------------------------------------------------------------------
    // PreF8.2: multi-line method chain
    // -----------------------------------------------------------------------
    //
    // The postfix loop tolerates Newline before `.` and continues the
    // expression. The resulting AST is identical to the one-liner
    // equivalent.

    #[test]
    fn method_chain_multiline_parses_same_as_oneliner() {
        let one = parse_expr("xs.filter(f).map(g)").unwrap();
        let many = parse_expr("xs\n    .filter(f)\n    .map(g)").unwrap();
        assert_eq!(one, many);
    }

    #[test]
    fn method_chain_of_3_lines_nests_correctly() {
        // xs\n.a()\n.b()\n.c() → Call(Field(Call(Field(Call(Field(xs, a)), b)), c))
        let e = parse_expr("xs\n  .a()\n  .b()\n  .c()").unwrap();
        let Expr::Call { callee, .. } = e else {
            panic!("expected outer Call")
        };
        let Expr::Field { object, field, .. } = *callee else {
            panic!("outer callee should have been Field")
        };
        assert_eq!(field, "c");
        let Expr::Call { callee, .. } = *object else {
            panic!("level 2 should have been Call")
        };
        let Expr::Field { object, field, .. } = *callee else {
            panic!("level 2 callee should have been Field")
        };
        assert_eq!(field, "b");
        let Expr::Call { callee, .. } = *object else {
            panic!("level 3 should have been Call")
        };
        let Expr::Field {
            object: receptor,
            field,
            ..
        } = *callee
        else {
            panic!("level 3 callee should have been Field")
        };
        assert_eq!(field, "a");
        assert_eq!(*receptor, Expr::Ident("xs".into(), Span::ZERO));
    }

    #[test]
    fn field_access_multiline_parses_same_as_oneliner() {
        // Without parens: just chained field access.
        let one = parse_expr("user.profile.email").unwrap();
        let many = parse_expr("user\n  .profile\n  .email").unwrap();
        assert_eq!(one, many);
    }

    #[test]
    fn await_multiline_chains_to_receiver() {
        // `fut\n  .await` → Await(fut)
        let one = parse_expr("fut.await").unwrap();
        let many = parse_expr("fut\n  .await").unwrap();
        assert_eq!(one, many);
    }

    #[test]
    fn method_chain_multiline_with_blank_newlines_works() {
        // More than one newline between links: all of them get consumed.
        let one = parse_expr("xs.a().b()").unwrap();
        let many = parse_expr("xs\n\n\n    .a()\n\n    .b()").unwrap();
        assert_eq!(one, many);
    }

    #[test]
    fn method_chain_multiline_does_not_consume_newline_if_no_dot_follows() {
        // `let x = foo` followed by `bar()` on the next line: two
        // statements, NOT a `foo()` call that "continues". The
        // lookahead only fires when what follows is `.`.
        let program = parse_program_str("let x = foo\nbar()").unwrap();
        assert_eq!(program.len(), 2, "se esperaban 2 stmts separados");
    }

    #[test]
    fn method_chain_multiline_works_in_let_rhs() {
        // Canonical use case: chain as the RHS of a `let`.
        let program =
            parse_program_str("let nombres = users\n  .filter(activo)\n  .map(nombre)").unwrap();
        assert_eq!(program.len(), 1);
        let Stmt::Assign { value, .. } = &program[0] else {
            panic!("expected Assign")
        };
        // The value must be a Call with a Field callee.
        let Expr::Call { callee, .. } = value else {
            panic!("expected Call in RHS")
        };
        let Expr::Field { field, .. } = callee.as_ref() else {
            panic!("callee should have been Field")
        };
        assert_eq!(field, "map");
    }

    #[test]
    fn dot_at_statement_start_without_receiver_is_still_error() {
        // Should not become a continuation of anything: a lone
        // `.foo()` starting a line is still an error (Dot is not primary).
        let result = parse_program_str(".foo()");
        assert!(result.is_err(), "expected parse error");
    }

    // -----------------------------------------------------------------------
    // Tests — statements (step 4: assign / return / expr-stmt / program)
    // -----------------------------------------------------------------------

    /// Helper: parse a program and return the `Program` (list of stmts).
    fn parse_program_str(src: &str) -> FitzResult<Program> {
        parse(tokenize(src).unwrap())
    }

    /// Helper: parse a program that is expected to have exactly one
    /// statement, and return that statement.
    fn parse_one_stmt(src: &str) -> Stmt {
        let program = parse_program_str(src).expect("parseo OK");
        assert_eq!(program.len(), 1, "expected a single statement");
        program.into_iter().next().unwrap()
    }

    #[test]
    fn empty_program_parses_to_empty() {
        assert!(parse_program_str("").unwrap().is_empty());
    }

    #[test]
    fn program_with_only_newlines_parses_to_empty() {
        assert!(parse_program_str("\n\n\n").unwrap().is_empty());
    }

    #[test]
    fn assign_with_let_no_type() {
        assert_eq!(
            parse_one_stmt("let x = 42"),
            Stmt::Assign {
                target: AssignTarget::Ident("x".into(), Span::default()),
                type_: None,
                value: Expr::Int(42, Span::ZERO),
                span: Span::ZERO
            }
        );
    }

    #[test]
    fn assign_with_let_and_type() {
        assert_eq!(
            parse_one_stmt("let x: Int = 42"),
            Stmt::Assign {
                target: AssignTarget::Ident("x".into(), Span::default()),
                type_: Some(TypeExpr::named("Int")),
                value: Expr::Int(42, Span::ZERO),
                span: Span::ZERO
            }
        );
    }

    #[test]
    fn assign_without_let_no_type() {
        assert_eq!(
            parse_one_stmt("x = 42"),
            Stmt::Assign {
                target: AssignTarget::Ident("x".into(), Span::default()),
                type_: None,
                value: Expr::Int(42, Span::ZERO),
                span: Span::ZERO
            }
        );
    }

    #[test]
    fn assign_without_let_with_type() {
        assert_eq!(
            parse_one_stmt("name: Str = \"Fitz\""),
            Stmt::Assign {
                target: AssignTarget::Ident("name".into(), Span::default()),
                type_: Some(TypeExpr::named("Str")),
                value: Expr::Str("Fitz".into(), Span::ZERO),
                span: Span::ZERO
            }
        );
    }

    #[test]
    fn assign_with_complex_expression() {
        // x = 10 + 5
        assert_eq!(
            parse_one_stmt("x = 10 + 5"),
            Stmt::Assign {
                target: AssignTarget::Ident("x".into(), Span::default()),
                type_: None,
                value: Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Int(10, Span::ZERO)),
                    right: Box::new(Expr::Int(5, Span::ZERO)),
                    span: Span::ZERO,
                },
                span: Span::ZERO
            }
        );
    }

    #[test]
    fn return_with_expression() {
        assert_eq!(
            parse_one_stmt("return 42"),
            Stmt::Return(Expr::Int(42, Span::ZERO), Span::ZERO),
        );
    }

    #[test]
    fn return_with_complex_expression() {
        assert_eq!(
            parse_one_stmt("return x + 1"),
            Stmt::Return(
                Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Ident("x".into(), Span::ZERO)),
                    right: Box::new(Expr::Int(1, Span::ZERO)),
                    span: Span::ZERO,
                },
                Span::ZERO
            ),
        );
    }

    #[test]
    fn return_status_with_body_map() {
        // `return <Int> { ... }` fires `Stmt::ReturnStatus`. The
        // body is parsed as any `Expr` — here a map literal with
        // explicit string keys.
        match parse_one_stmt("return 401 {\"message\": \"no autorizado\"}") {
            Stmt::ReturnStatus { status, body, .. } => {
                assert!(matches!(status, Expr::Int(401, _)), "status: {:?}", status);
                let Some(b) = body else {
                    panic!("body esperado")
                };
                assert!(matches!(b, Expr::Map(..)), "body should be Map: {:?}", b);
            }
            other => panic!("expected ReturnStatus, was: {:?}", other),
        }
    }

    #[test]
    fn return_int_without_body_is_still_normal_return() {
        // Without `{...}` after the Int, it's still a plain Int
        // Return — does NOT fire ReturnStatus. This preserves the
        // existing syntax (`return 42` in a fn that returns Int).
        assert_eq!(
            parse_one_stmt("return 204"),
            Stmt::Return(Expr::Int(204, Span::ZERO), Span::ZERO),
        );
    }

    #[test]
    fn return_status_only_with_int_literal() {
        // Only Int literals fire `ReturnStatus`. A more complex expr
        // (`return x { ... }`) does NOT — it stays as a Return of
        // the full expr (which would fail later anyway).
        match parse_one_stmt("return get_status() ") {
            Stmt::Return(Expr::Call { .. }, _) => {}
            other => panic!("expected Return(Call), was: {:?}", other),
        }
    }

    #[test]
    fn return_without_expression_returns_null() {
        // Bare `return` (with newline at the end). The parser models
        // it as `Stmt::Return(Expr::Null(_), Span::ZERO)`.
        assert_eq!(
            parse_one_stmt("return"),
            Stmt::Return(Expr::Null(Span::ZERO), Span::ZERO)
        );
    }

    #[test]
    fn return_without_expression_inside_fn_body() {
        // fn early_exit() { return }
        let src = "fn early_exit() { return }";
        let tokens = crate::lexer::tokenize(src).unwrap();
        let program = parse(tokens).unwrap();
        match &program[0] {
            Stmt::FnDef { body, .. } => {
                assert_eq!(
                    body,
                    &vec![Stmt::Return(Expr::Null(Span::ZERO), Span::ZERO)]
                );
            }
            _ => panic!("expected FnDef"),
        }
    }

    #[test]
    fn expression_statement_with_call() {
        assert_eq!(
            parse_one_stmt("print(x)"),
            Stmt::Expr(
                Expr::Call {
                    callee: Box::new(Expr::Ident("print".into(), Span::ZERO)),
                    args: vec![Expr::Ident("x".into(), Span::ZERO)],
                    span: Span::ZERO,
                },
                Span::ZERO
            ),
        );
    }

    // ---- B.1: Span propagation -----------------------------------

    #[test]
    fn stmt_carries_span_of_first_line() {
        // Simple stmt at line 1, col 1 → span must be (1, 1).
        let stmt = parse_one_stmt("let x = 42");
        let span = stmt.span();
        assert_eq!(span.line, 1, "expected line 1, was {}", span.line);
        assert_eq!(span.column, 1, "expected col 1, was {}", span.column);
    }

    #[test]
    fn stmt_carries_span_of_later_line() {
        // Stmts on lines 2 and 3 — each with its own span.
        let src = "\n  let x = 1\nreturn x";
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        assert_eq!(program.len(), 2);
        let s0 = program[0].span();
        let s1 = program[1].span();
        assert_eq!(
            (s0.line, s0.column),
            (2, 3),
            "expected (2,3) for `let`, was ({},{})",
            s0.line,
            s0.column
        );
        assert_eq!(
            (s1.line, s1.column),
            (3, 1),
            "expected (3,1) for `return`, was ({},{})",
            s1.line,
            s1.column
        );
    }

    #[test]
    fn span_of_fn_def_points_to_fn() {
        let src = "  fn foo() => 1";
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let span = program[0].span();
        assert_eq!((span.line, span.column), (1, 3));
    }

    #[test]
    fn span_of_decorated_fn_points_to_decorator() {
        // A decorated `Stmt::FnDef` span points at `@`, not at `fn`.
        let src = "@get(\"/\") fn handler() => 0";
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let span = program[0].span();
        assert_eq!(span.column, 1);
    }

    // ---- end of B.1 span tests ------------------------------------

    #[test]
    fn break_statement() {
        assert!(matches!(parse_one_stmt("break"), Stmt::Break(_, _, _)));
    }

    #[test]
    fn continue_statement() {
        assert!(matches!(parse_one_stmt("continue"), Stmt::Continue(_, _)));
    }

    #[test]
    fn while_basic_parses() {
        let stmt = parse_one_stmt("while x < 10 { x = x + 1 }");
        match stmt {
            Stmt::While {
                condition, body, ..
            } => {
                assert!(matches!(
                    condition,
                    Expr::BinOp {
                        op: BinOpKind::Lt,
                        ..
                    }
                ));
                assert_eq!(body.len(), 1);
                assert!(matches!(body[0], Stmt::Assign { .. }));
            }
            other => panic!("expected Stmt::While, got {:?}", other),
        }
    }

    #[test]
    fn while_with_break_inside() {
        let stmt = parse_one_stmt("while true { break }");
        match stmt {
            Stmt::While { body, .. } => {
                assert!(matches!(body[..], [Stmt::Break(_, _, _)]));
            }
            _ => panic!("expected while"),
        }
    }

    #[test]
    fn loop_basic_parses() {
        let stmt = parse_one_stmt("loop { x = 1 }");
        match stmt {
            Stmt::Loop { body, .. } => {
                assert_eq!(body.len(), 1);
                assert!(matches!(body[0], Stmt::Assign { .. }));
            }
            _ => panic!("expected Stmt::Loop"),
        }
    }

    #[test]
    fn and_basic_parses() {
        assert_eq!(
            parse_one_stmt("x and y"),
            Stmt::Expr(
                Expr::BinOp {
                    op: BinOpKind::And,
                    left: Box::new(Expr::Ident("x".into(), Span::ZERO)),
                    right: Box::new(Expr::Ident("y".into(), Span::ZERO)),
                    span: Span::ZERO,
                },
                Span::ZERO
            ),
        );
    }

    #[test]
    fn or_basic_parses() {
        assert_eq!(
            parse_one_stmt("x or y"),
            Stmt::Expr(
                Expr::BinOp {
                    op: BinOpKind::Or,
                    left: Box::new(Expr::Ident("x".into(), Span::ZERO)),
                    right: Box::new(Expr::Ident("y".into(), Span::ZERO)),
                    span: Span::ZERO,
                },
                Span::ZERO
            ),
        );
    }

    // ---- Mini-batch Xor ----

    #[test]
    fn xor_basic_parses() {
        assert_eq!(
            parse_one_stmt("x xor y"),
            Stmt::Expr(
                Expr::BinOp {
                    op: BinOpKind::Xor,
                    left: Box::new(Expr::Ident("x".into(), Span::ZERO)),
                    right: Box::new(Expr::Ident("y".into(), Span::ZERO)),
                    span: Span::ZERO,
                },
                Span::ZERO
            ),
        );
    }

    #[test]
    fn xor_same_precedence_as_or_left_assoc() {
        // `a xor b xor c` → `(a xor b) xor c`
        let stmt = parse_one_stmt("a xor b xor c");
        let expected = Stmt::Expr(
            Expr::BinOp {
                op: BinOpKind::Xor,
                left: Box::new(Expr::BinOp {
                    op: BinOpKind::Xor,
                    left: Box::new(Expr::Ident("a".into(), Span::ZERO)),
                    right: Box::new(Expr::Ident("b".into(), Span::ZERO)),
                    span: Span::ZERO,
                }),
                right: Box::new(Expr::Ident("c".into(), Span::ZERO)),
                span: Span::ZERO,
            },
            Span::ZERO,
        );
        assert_eq!(stmt, expected);
    }

    #[test]
    fn xor_and_or_chain_freely_same_precedence() {
        // `a or b xor c` → `(a or b) xor c` (mismo nivel, left-assoc).
        let stmt = parse_one_stmt("a or b xor c");
        let expected = Stmt::Expr(
            Expr::BinOp {
                op: BinOpKind::Xor,
                left: Box::new(Expr::BinOp {
                    op: BinOpKind::Or,
                    left: Box::new(Expr::Ident("a".into(), Span::ZERO)),
                    right: Box::new(Expr::Ident("b".into(), Span::ZERO)),
                    span: Span::ZERO,
                }),
                right: Box::new(Expr::Ident("c".into(), Span::ZERO)),
                span: Span::ZERO,
            },
            Span::ZERO,
        );
        assert_eq!(stmt, expected);
    }

    #[test]
    fn and_has_higher_precedence_than_xor() {
        // `a and b xor c` → `(a and b) xor c`
        let stmt = parse_one_stmt("a and b xor c");
        let expected = Stmt::Expr(
            Expr::BinOp {
                op: BinOpKind::Xor,
                left: Box::new(Expr::BinOp {
                    op: BinOpKind::And,
                    left: Box::new(Expr::Ident("a".into(), Span::ZERO)),
                    right: Box::new(Expr::Ident("b".into(), Span::ZERO)),
                    span: Span::ZERO,
                }),
                right: Box::new(Expr::Ident("c".into(), Span::ZERO)),
                span: Span::ZERO,
            },
            Span::ZERO,
        );
        assert_eq!(stmt, expected);
    }

    #[test]
    fn and_has_higher_precedence_than_or() {
        // `a and b or c` → `(a and b) or c`
        let stmt = parse_one_stmt("a and b or c");
        let expected = Stmt::Expr(
            Expr::BinOp {
                op: BinOpKind::Or,
                left: Box::new(Expr::BinOp {
                    op: BinOpKind::And,
                    left: Box::new(Expr::Ident("a".into(), Span::ZERO)),
                    right: Box::new(Expr::Ident("b".into(), Span::ZERO)),
                    span: Span::ZERO,
                }),
                right: Box::new(Expr::Ident("c".into(), Span::ZERO)),
                span: Span::ZERO,
            },
            Span::ZERO,
        );
        assert_eq!(stmt, expected);
    }

    #[test]
    fn comparison_has_higher_precedence_than_and() {
        // `a > 0 and a < 10` → `(a > 0) and (a < 10)`
        let stmt = parse_one_stmt("a > 0 and a < 10");
        let expected = Stmt::Expr(
            Expr::BinOp {
                op: BinOpKind::And,
                left: Box::new(Expr::BinOp {
                    op: BinOpKind::Gt,
                    left: Box::new(Expr::Ident("a".into(), Span::ZERO)),
                    right: Box::new(Expr::Int(0, Span::ZERO)),
                    span: Span::ZERO,
                }),
                right: Box::new(Expr::BinOp {
                    op: BinOpKind::Lt,
                    left: Box::new(Expr::Ident("a".into(), Span::ZERO)),
                    right: Box::new(Expr::Int(10, Span::ZERO)),
                    span: Span::ZERO,
                }),
                span: Span::ZERO,
            },
            Span::ZERO,
        );
        assert_eq!(stmt, expected);
    }

    #[test]
    fn equality_in_expr_stmt_is_not_assignment() {
        // `x == y` must be an expr-stmt with BinOp(Eq), NOT Assign.
        // This validates that the lookahead distinguishes Eq from EqEq.
        assert_eq!(
            parse_one_stmt("x == y"),
            Stmt::Expr(
                Expr::BinOp {
                    op: BinOpKind::Eq,
                    left: Box::new(Expr::Ident("x".into(), Span::ZERO)),
                    right: Box::new(Expr::Ident("y".into(), Span::ZERO)),
                    span: Span::ZERO,
                },
                Span::ZERO
            ),
        );
    }

    #[test]
    fn multiple_statements_separated_by_newlines() {
        let src = "x = 1\ny = 2\nprint(x)";
        let program = parse_program_str(src).unwrap();
        assert_eq!(program.len(), 3);
        assert_eq!(
            program[0],
            Stmt::Assign {
                target: AssignTarget::Ident("x".into(), Span::default()),
                type_: None,
                value: Expr::Int(1, Span::ZERO),
                span: Span::ZERO
            }
        );
        assert_eq!(
            program[2],
            Stmt::Expr(
                Expr::Call {
                    callee: Box::new(Expr::Ident("print".into(), Span::ZERO)),
                    args: vec![Expr::Ident("x".into(), Span::ZERO)],
                    span: Span::ZERO,
                },
                Span::ZERO
            )
        );
    }

    #[test]
    fn blank_lines_between_statements_are_tolerated() {
        let src = "x = 1\n\n\ny = 2";
        let program = parse_program_str(src).unwrap();
        assert_eq!(program.len(), 2);
    }

    #[test]
    fn trailing_newline_is_fine() {
        let src = "x = 1\n";
        let program = parse_program_str(src).unwrap();
        assert_eq!(program.len(), 1);
    }

    #[test]
    fn assign_without_value_errors() {
        // let x =  (no expression after '=')
        let err = parse_program_str("let x =").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn let_without_name_errors() {
        let err = parse_program_str("let = 5").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn two_statements_same_line_without_separator_errors() {
        // No separator between `x = 1` and `print(x)` on the same line.
        let err = parse_program_str("x = 1 print(x)").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    // -----------------------------------------------------------------------
    // Tests — fndef (step 5)
    // -----------------------------------------------------------------------

    #[test]
    fn fndef_arrow_no_types() {
        // fn double(n) => n * 2
        assert_eq!(
            parse_one_stmt("fn double(n) => n * 2"),
            Stmt::FnDef {
                name: "double".into(),
                params: vec![Param {
                    name: "n".into(),
                    type_: None,
                    default: None,
                    varargs: false,
                    name_span: Span::default(),
                }],
                return_type: None,
                body: vec![Stmt::Return(
                    Expr::BinOp {
                        op: BinOpKind::Mul,
                        left: Box::new(Expr::Ident("n".into(), Span::ZERO)),
                        right: Box::new(Expr::Int(2, Span::ZERO)),
                        span: Span::ZERO,
                    },
                    Span::ZERO
                )],
                is_async: false,
                decorators: vec![],
                span: Span::ZERO
            },
        );
    }

    #[test]
    fn fndef_arrow_with_types() {
        // fn double(n: Int) -> Int => n * 2
        assert_eq!(
            parse_one_stmt("fn double(n: Int) -> Int => n * 2"),
            Stmt::FnDef {
                name: "double".into(),
                params: vec![Param {
                    name: "n".into(),
                    type_: Some(TypeExpr::named("Int")),
                    default: None,
                    varargs: false,
                    name_span: Span::default(),
                }],
                return_type: Some(TypeExpr::named("Int")),
                body: vec![Stmt::Return(
                    Expr::BinOp {
                        op: BinOpKind::Mul,
                        left: Box::new(Expr::Ident("n".into(), Span::ZERO)),
                        right: Box::new(Expr::Int(2, Span::ZERO)),
                        span: Span::ZERO,
                    },
                    Span::ZERO
                )],
                is_async: false,
                decorators: vec![],
                span: Span::ZERO
            },
        );
    }

    #[test]
    fn fndef_block_no_types() {
        // fn greet(name) { print(name) }
        assert_eq!(
            parse_one_stmt("fn greet(name) { print(name) }"),
            Stmt::FnDef {
                name: "greet".into(),
                params: vec![Param {
                    name: "name".into(),
                    type_: None,
                    default: None,
                    varargs: false,
                    name_span: Span::default(),
                }],
                return_type: None,
                body: vec![Stmt::Expr(
                    Expr::Call {
                        callee: Box::new(Expr::Ident("print".into(), Span::ZERO)),
                        args: vec![Expr::Ident("name".into(), Span::ZERO)],
                        span: Span::ZERO,
                    },
                    Span::ZERO
                )],
                is_async: false,
                decorators: vec![],
                span: Span::ZERO
            },
        );
    }

    #[test]
    fn fndef_block_multiline_body() {
        let src = "fn calc(n) {\n  let x = n * 2\n  return x\n}";
        assert_eq!(
            parse_one_stmt(src),
            Stmt::FnDef {
                name: "calc".into(),
                params: vec![Param {
                    name: "n".into(),
                    type_: None,
                    default: None,
                    varargs: false,
                    name_span: Span::default(),
                }],
                return_type: None,
                body: vec![
                    Stmt::Assign {
                        target: AssignTarget::Ident("x".into(), Span::default()),
                        type_: None,
                        value: Expr::BinOp {
                            op: BinOpKind::Mul,
                            left: Box::new(Expr::Ident("n".into(), Span::ZERO)),
                            right: Box::new(Expr::Int(2, Span::ZERO)),
                            span: Span::ZERO,
                        },
                        span: Span::ZERO
                    },
                    Stmt::Return(Expr::Ident("x".into(), Span::ZERO), Span::ZERO),
                ],
                is_async: false,
                decorators: vec![],
                span: Span::ZERO,
            },
        );
    }

    #[test]
    fn fndef_block_with_full_types_and_multiple_params() {
        // fn add(a: Int, b: Int) -> Int { return a + b }
        let stmt = parse_one_stmt("fn add(a: Int, b: Int) -> Int { return a + b }");
        match stmt {
            Stmt::FnDef {
                name,
                params,
                return_type,
                body,
                is_async,
                decorators,
                ..
            } => {
                assert_eq!(name, "add");
                assert_eq!(params.len(), 2);
                assert_eq!(params[0].name, "a");
                assert_eq!(params[0].type_, Some(TypeExpr::named("Int")));
                assert_eq!(params[1].name, "b");
                assert_eq!(params[1].type_, Some(TypeExpr::named("Int")));
                assert_eq!(return_type, Some(TypeExpr::named("Int")));
                assert_eq!(body.len(), 1);
                assert!(!is_async);
                assert!(decorators.is_empty());
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn fndef_no_params() {
        let stmt = parse_one_stmt("fn main() { return 0 }");
        match stmt {
            Stmt::FnDef { params, .. } => assert!(params.is_empty()),
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn fndef_empty_block_body() {
        let stmt = parse_one_stmt("fn noop() { }");
        match stmt {
            Stmt::FnDef { body, .. } => assert!(body.is_empty()),
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn fndef_async_block() {
        // async fn fetch(id: Int) -> User { return user }
        let stmt = parse_one_stmt("async fn fetch(id: Int) -> User { return user }");
        match stmt {
            Stmt::FnDef {
                name,
                is_async,
                return_type,
                ..
            } => {
                assert_eq!(name, "fetch");
                assert!(is_async);
                assert_eq!(return_type, Some(TypeExpr::named("User")));
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn fndef_async_arrow() {
        let stmt = parse_one_stmt("async fn double(n) => n * 2");
        match stmt {
            Stmt::FnDef { is_async, .. } => assert!(is_async),
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn fndef_params_with_newlines_and_trailing_comma() {
        // fn sum(
        //   a,
        //   b,
        // ) => a + b
        let src = "fn sum(\n  a,\n  b,\n) => a + b";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::FnDef { params, .. } => {
                assert_eq!(params.len(), 2);
                assert_eq!(params[0].name, "a");
                assert_eq!(params[1].name, "b");
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn fndef_missing_name_errors() {
        let err = parse_program_str("fn () { }").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn fndef_unclosed_block_errors() {
        // 'fn f() {' without a closing '}' at the end.
        let err = parse_program_str("fn f() {\n  x = 1\n").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::MissingClosingBrace));
    }

    #[test]
    fn fndef_missing_body_marker_errors() {
        // After ')' or '-> Type' there must be '{' or '=>'.
        let err = parse_program_str("fn f() return 1").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    // -----------------------------------------------------------------------
    // Tests — StrInterp (step 6)
    // -----------------------------------------------------------------------

    #[test]
    fn string_without_interpolation_is_plain_str() {
        assert_eq!(
            parse_expr(r#""hola""#).unwrap(),
            Expr::Str("hola".into(), Span::ZERO)
        );
    }

    #[test]
    fn empty_string_is_plain_str() {
        assert_eq!(
            parse_expr(r#""""#).unwrap(),
            Expr::Str("".into(), Span::ZERO)
        );
    }

    #[test]
    fn string_with_simple_ident_interpolation() {
        // "Hola, {name}!" → StrInterp([Lit, Expr, Lit])
        assert_eq!(
            parse_expr(r#""Hola, {name}!""#).unwrap(),
            Expr::StrInterp(
                vec![
                    StrPart::Lit("Hola, ".into()),
                    StrPart::Expr(Expr::Ident("name".into(), Span::ZERO), None),
                    StrPart::Lit("!".into()),
                ],
                Span::ZERO
            ),
        );
    }

    #[test]
    fn string_starting_with_interpolation() {
        // "{x} es el valor" → StrInterp([Expr, Lit])
        assert_eq!(
            parse_expr(r#""{x} es el valor""#).unwrap(),
            Expr::StrInterp(
                vec![
                    StrPart::Expr(Expr::Ident("x".into(), Span::ZERO), None),
                    StrPart::Lit(" es el valor".into()),
                ],
                Span::ZERO
            ),
        );
    }

    #[test]
    fn string_ending_with_interpolation() {
        // "valor: {x}" → StrInterp([Lit, Expr])
        assert_eq!(
            parse_expr(r#""valor: {x}""#).unwrap(),
            Expr::StrInterp(
                vec![
                    StrPart::Lit("valor: ".into()),
                    StrPart::Expr(Expr::Ident("x".into(), Span::ZERO), None),
                ],
                Span::ZERO
            ),
        );
    }

    #[test]
    fn string_with_only_interpolation_no_literal_parts() {
        // "{x}" — no literals around.
        assert_eq!(
            parse_expr(r#""{x}""#).unwrap(),
            Expr::StrInterp(
                vec![StrPart::Expr(Expr::Ident("x".into(), Span::ZERO), None)],
                Span::ZERO
            ),
        );
    }

    #[test]
    fn string_with_multiple_interpolations() {
        // "Hola {name}, tenés {n} mensajes"
        assert_eq!(
            parse_expr(r#""Hola {name}, tenés {n} mensajes""#).unwrap(),
            Expr::StrInterp(
                vec![
                    StrPart::Lit("Hola ".into()),
                    StrPart::Expr(Expr::Ident("name".into(), Span::ZERO), None),
                    StrPart::Lit(", tenés ".into()),
                    StrPart::Expr(Expr::Ident("n".into(), Span::ZERO), None),
                    StrPart::Lit(" mensajes".into()),
                ],
                Span::ZERO
            ),
        );
    }

    #[test]
    fn string_with_arithmetic_interpolation() {
        // "respuesta: {40 + 2}"
        assert_eq!(
            parse_expr(r#""respuesta: {40 + 2}""#).unwrap(),
            Expr::StrInterp(
                vec![
                    StrPart::Lit("respuesta: ".into()),
                    StrPart::Expr(
                        Expr::BinOp {
                            op: BinOpKind::Add,
                            left: Box::new(Expr::Int(40, Span::ZERO)),
                            right: Box::new(Expr::Int(2, Span::ZERO)),
                            span: Span::ZERO,
                        },
                        None
                    ),
                ],
                Span::ZERO
            ),
        );
    }

    #[test]
    fn escaped_braces_become_literal_in_plain_string() {
        // "\{name\}" → literal "{name}" with no interpolation.
        assert_eq!(
            parse_expr(r#""\{nombre\}""#).unwrap(),
            Expr::Str("{nombre}".into(), Span::ZERO),
        );
    }

    #[test]
    fn escaped_and_unescaped_braces_in_same_string() {
        // "\{ {x} \}" → literal "{ ", interpolation of x, literal " }"
        assert_eq!(
            parse_expr(r#""\{ {x} \}""#).unwrap(),
            Expr::StrInterp(
                vec![
                    StrPart::Lit("{ ".into()),
                    StrPart::Expr(Expr::Ident("x".into(), Span::ZERO), None),
                    StrPart::Lit(" }".into()),
                ],
                Span::ZERO
            ),
        );
    }

    #[test]
    fn unclosed_interpolation_errors() {
        // "hola {name"  — missing '}'
        let err = parse_expr(r#""hola {name""#).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
    }

    #[test]
    fn lone_close_brace_errors() {
        // "hola }"  — lone '}' without preceding '{'
        let err = parse_expr(r#""hola }""#).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
    }

    #[test]
    fn unclosed_interpolation_reports_column_of_open_brace() {
        // `"a{x"` — the `{` is at column 3 (after the quote at col 1
        // and the 'a' at col 2). The error must point there, not at
        // column 1 of the string.
        let tokens = crate::lexer::tokenize(r#""a{x""#).unwrap();
        let err = parse(tokens).unwrap_err();
        assert_eq!(err.column, 3);
    }

    #[test]
    fn error_in_interp_subexpression_points_inside_string() {
        // `"foo{1 +}"` — the `+}` (invalid subexpression) must be
        // reported with a column inside the interpolation block, not
        // at column 1.
        let tokens = crate::lexer::tokenize(r#""foo{1 +}""#).unwrap();
        let err = parse(tokens).unwrap_err();
        // The string starts at col 1, the content at col 2, the `{` at col 5.
        // The subexpression starts at col 6. Any column > 1 confirms
        // that the translation is active.
        assert!(
            err.column > 1,
            "expected column > 1, got {} (msg: {})",
            err.column,
            err.message,
        );
    }

    #[test]
    fn invalid_subexpression_propagates_error() {
        // "{1 +}"  — invalid subexpression
        let err = parse_expr(r#""{1 +}""#).unwrap_err();
        // The error may be UnexpectedToken (from the subexpression).
        assert!(matches!(
            err.kind,
            ErrorKind::UnexpectedToken | ErrorKind::InvalidSyntax
        ));
    }

    // -----------------------------------------------------------------------
    // Tests — if / match / type (step 7)
    // -----------------------------------------------------------------------

    #[test]
    fn if_without_else() {
        // if x < 5 { print(x) }
        let stmt = parse_one_stmt("if x < 5 { print(x) }");
        match stmt {
            Stmt::Expr(
                Expr::If {
                    condition,
                    then,
                    else_,
                    ..
                },
                _,
            ) => {
                assert_eq!(
                    *condition,
                    Expr::BinOp {
                        op: BinOpKind::Lt,
                        left: Box::new(Expr::Ident("x".into(), Span::ZERO)),
                        right: Box::new(Expr::Int(5, Span::ZERO)),
                        span: Span::ZERO,
                    }
                );
                assert_eq!(then.len(), 1);
                assert!(else_.is_none());
            }
            other => panic!("expected Stmt::Expr(If, Span::ZERO), got {:?}", other),
        }
    }

    #[test]
    fn if_with_else() {
        let stmt = parse_one_stmt("if x { 1 } else { 2 }");
        match stmt {
            Stmt::Expr(Expr::If { else_: Some(e), .. }, _) => {
                assert_eq!(e.len(), 1);
            }
            other => panic!("expected If with else, got {:?}", other),
        }
    }

    #[test]
    fn if_else_if_else_chains_as_nested_else() {
        // if a { 1 } else if b { 2 } else { 3 }
        // → If(a, [1], else: [Expr(If(b, [2], else: [3]))])
        let stmt = parse_one_stmt("if a { 1 } else if b { 2 } else { 3 }");
        match stmt {
            Stmt::Expr(
                Expr::If {
                    else_: Some(outer_else),
                    ..
                },
                _,
            ) => {
                // The outer else holds a single stmt: a nested Expr::If.
                assert_eq!(outer_else.len(), 1);
                match &outer_else[0] {
                    Stmt::Expr(
                        Expr::If {
                            else_: Some(inner_else),
                            ..
                        },
                        _,
                    ) => {
                        assert_eq!(inner_else.len(), 1);
                    }
                    other => panic!("expected nested if, got {:?}", other),
                }
            }
            other => panic!("expected If, got {:?}", other),
        }
    }

    #[test]
    fn if_as_expression_in_assignment() {
        // status = if active { "on" } else { "off" }
        let stmt = parse_one_stmt(r#"status = if active { "on" } else { "off" }"#);
        match stmt {
            Stmt::Assign {
                target: AssignTarget::Ident(name, _),
                value: Expr::If { .. },
                ..
            } => {
                assert_eq!(name, "status");
            }
            other => panic!("expected Assign with If as value, got {:?}", other),
        }
    }

    #[test]
    fn if_with_multiline_block() {
        let src = "if x {\n  let y = 1\n  print(y)\n}";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::If { then, .. }, _) => {
                assert_eq!(then.len(), 2);
            }
            other => panic!("expected If, got {:?}", other),
        }
    }

    #[test]
    fn match_with_wildcard_and_ident_patterns() {
        // match x { foo => 1, _ => 0 }
        let stmt = parse_one_stmt("match x { foo => 1, _ => 0 }");
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(arms.len(), 2);
                assert_eq!(
                    arms[0].pattern,
                    Pattern::Ident("foo".into(), Span::default())
                );
                assert_eq!(arms[1].pattern, Pattern::Wildcard);
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn match_with_ok_and_err_bindings() {
        // match result { Ok(u) => u, Err(e) => 0 }
        let stmt = parse_one_stmt("match result { Ok(u) => u, Err(e) => 0 }");
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(arms.len(), 2);
                assert_eq!(
                    arms[0].pattern,
                    Pattern::OkBinding("u".into(), Span::default())
                );
                assert_eq!(
                    arms[1].pattern,
                    Pattern::ErrBinding("e".into(), Span::default())
                );
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn match_with_ok_and_err_wildcards() {
        // `Ok(_)` and `Err(_)` parse as dedicated wildcards without
        // polluting the scope with a var named `_`.
        let stmt = parse_one_stmt("match result { Ok(_) => 1, Err(_) => 0 }");
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(arms.len(), 2);
                assert_eq!(arms[0].pattern, Pattern::OkWildcard);
                assert_eq!(arms[1].pattern, Pattern::ErrWildcard);
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn match_with_newline_separated_arms() {
        let src = "match x {\n  foo => 1\n  bar => 2\n  _ => 0\n}";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => assert_eq!(arms.len(), 3),
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn match_unclosed_errors() {
        let err = parse_program_str("match x { foo => 1").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::MissingClosingBrace));
    }

    #[test]
    fn typedef_empty() {
        let stmt = parse_one_stmt("type Empty { }");
        match stmt {
            Stmt::TypeDef { name, fields, .. } => {
                assert_eq!(name, "Empty");
                assert!(fields.is_empty());
            }
            other => panic!("expected TypeDef, got {:?}", other),
        }
    }

    #[test]
    fn typedef_with_simple_fields() {
        let src = "type User {\n  id: Int\n  name: Str\n}";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::TypeDef { name, fields, .. } => {
                assert_eq!(name, "User");
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].name, "id");
                assert_eq!(fields[0].type_, TypeExpr::named("Int"));
                assert!(!fields[0].type_.is_nullable());
                assert!(fields[0].default.is_none());
            }
            other => panic!("expected TypeDef, got {:?}", other),
        }
    }

    #[test]
    fn typedef_with_nullable_and_default() {
        // type User { id: Int, email: Str? = null, active: Bool = true }
        let src = "type User { id: Int, email: Str? = null, active: Bool = true }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::TypeDef { fields, .. } => {
                assert_eq!(fields.len(), 3);
                // email is nullable with default null
                assert_eq!(fields[1].name, "email");
                assert!(fields[1].type_.is_nullable());
                assert_eq!(fields[1].default, Some(Expr::Null(Span::ZERO)));
                // active is not nullable but has default true
                assert_eq!(fields[2].name, "active");
                assert!(!fields[2].type_.is_nullable());
                assert_eq!(fields[2].default, Some(Expr::Bool(true, Span::ZERO)));
            }
            other => panic!("expected TypeDef, got {:?}", other),
        }
    }

    #[test]
    fn typedef_unclosed_errors() {
        let err = parse_program_str("type User { id: Int").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::MissingClosingBrace));
    }

    // -----------------------------------------------------------------------
    // Tests — decorators on FnDef (Phase 4, step 4.1)
    // -----------------------------------------------------------------------
    //
    // The parser doesn't know what each decorator does (that's the
    // evaluator's job). Here we validate pure structure: name, args,
    // and that they attach to the FnDef in the right order.

    #[test]
    fn decorator_get_attaches_decorator_to_fndef() {
        // @get("/")
        // fn index() => "hola"
        let src = "@get(\"/\")\nfn index() => \"hola\"";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::FnDef {
                name,
                is_async,
                decorators,
                ..
            } => {
                assert_eq!(name, "index");
                assert!(!is_async);
                assert_eq!(decorators.len(), 1);
                assert_eq!(decorators[0].name, "get");
                assert_eq!(decorators[0].args, vec![Expr::Str("/".into(), Span::ZERO)]);
            }
            other => panic!("expected FnDef with decorators, got {:?}", other),
        }
    }

    #[test]
    fn decorator_post_with_async_handler() {
        let src =
            "@post(\"/users\")\nasync fn create_user(body: UserInput) -> User {\n  return body\n}";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::FnDef {
                name,
                is_async,
                return_type,
                params,
                decorators,
                ..
            } => {
                assert_eq!(name, "create_user");
                assert!(is_async);
                assert_eq!(return_type, Some(TypeExpr::named("User")));
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].name, "body");
                assert_eq!(decorators.len(), 1);
                assert_eq!(decorators[0].name, "post");
                assert_eq!(
                    decorators[0].args,
                    vec![Expr::Str("/users".into(), Span::ZERO)]
                );
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn decorator_put_and_delete_recognized_by_name() {
        // Note about `/users/{id}`: the parser interprets it as
        // `StrInterp` because `{id}` is Fitz's string interpolation
        // syntax. For the HTTP runtime this is good news, not a bug:
        // in 4.2, the `StrPart::Expr(Ident(...))` in the path are
        // recognized directly as path params, without needing a mini
        // parser dedicated inside the decorator.
        let put = parse_one_stmt("@put(\"/users/{id}\")\nasync fn upd(id: Int) -> User => user");
        let del = parse_one_stmt("@delete(\"/users\")\nasync fn del(id: Int) => 0");
        match put {
            Stmt::FnDef { decorators, .. } => {
                assert_eq!(decorators.len(), 1);
                assert_eq!(decorators[0].name, "put");
                // The path has `{id}` → arrives as StrInterp.
                assert_eq!(decorators[0].args.len(), 1);
                assert!(matches!(decorators[0].args[0], Expr::StrInterp(_, _)));
                if let Expr::StrInterp(parts, _) = &decorators[0].args[0] {
                    assert_eq!(parts[0], StrPart::Lit("/users/".into()));
                    assert_eq!(
                        parts[1],
                        StrPart::Expr(Expr::Ident("id".into(), Span::ZERO), None)
                    );
                }
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
        match del {
            Stmt::FnDef { decorators, .. } => {
                assert_eq!(decorators[0].name, "delete");
                // No path params: arrives as a bare Str.
                assert_eq!(
                    decorators[0].args,
                    vec![Expr::Str("/users".into(), Span::ZERO)]
                );
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn decorator_without_args_allows_empty_parens() {
        // `@server()` — empty parens are valid for symmetry with
        // function calls.
        let stmt = parse_one_stmt("@server()\nfn config() => 0");
        match stmt {
            Stmt::FnDef { decorators, .. } => {
                assert_eq!(decorators.len(), 1);
                assert_eq!(decorators[0].name, "server");
                assert!(decorators[0].args.is_empty());
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn decorator_allows_multiple_args_and_expressions() {
        // `@server(8080, "0.0.0.0")` — positional args with mixed types
        // mixed. The evaluator will validate semantics; the parser only
        // stores them.
        let stmt = parse_one_stmt("@server(8080, \"0.0.0.0\")\nfn cfg() => 0");
        match stmt {
            Stmt::FnDef { decorators, .. } => {
                assert_eq!(
                    decorators[0].args,
                    vec![
                        Expr::Int(8080, Span::ZERO),
                        Expr::Str("0.0.0.0".into(), Span::ZERO),
                    ]
                );
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn stacked_decorators_accumulate_in_order() {
        // @get("/admin") + @auth("admin") stacked on the same fn.
        // Each on its own line.
        let src = "@get(\"/admin\")\n@auth(\"admin\")\nfn dash() => \"ok\"";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::FnDef { decorators, .. } => {
                assert_eq!(decorators.len(), 2);
                assert_eq!(decorators[0].name, "get");
                assert_eq!(decorators[1].name, "auth");
                assert_eq!(
                    decorators[1].args,
                    vec![Expr::Str("admin".into(), Span::ZERO)]
                );
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn decorator_without_parens_parses_with_empty_args() {
        // Phase 9.z.2.a — optional parens in decorators (needed for
        // `@test fn ...`). `@get fn h() => 0` parses with
        // `args = kwargs = empty`. Semantic validation that `@get`
        // needs a path is done by the evaluator, not the parser.
        let stmt = parse_one_stmt("@get\nfn h() => 0");
        match stmt {
            Stmt::FnDef { decorators, .. } => {
                assert_eq!(decorators.len(), 1);
                assert_eq!(decorators[0].name, "get");
                assert!(decorators[0].args.is_empty());
                assert!(decorators[0].kwargs.is_empty());
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn test_decorator_without_parens_parses() {
        // Canonical 9.z.2.a case: `@test fn name() { ... }` without
        // parens after `@test`. The spec's idiomatic form.
        let stmt = parse_one_stmt("@test\nfn suma_funciona() { let x = 1 }");
        match stmt {
            Stmt::FnDef { decorators, .. } => {
                assert_eq!(decorators[0].name, "test");
                assert!(decorators[0].args.is_empty());
                assert!(decorators[0].kwargs.is_empty());
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn decorator_without_handler_errors() {
        // @get("/x") and nothing after: the parser bails because there is no fn.
        let err = parse_program_str("@get(\"/x\")").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn decorator_followed_by_non_fn_errors() {
        // @get("/x") let x = 1  → error claro
        let err = parse_program_str("@get(\"/x\")\nlet x = 1").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn unknown_decorator_is_not_parser_error() {
        // Any `@name(args)` that is syntactically valid parses.
        // Whether `@patch` is implemented is the evaluator's call.
        let stmt = parse_one_stmt("@patch(\"/x\")\nfn h() => 0");
        match stmt {
            Stmt::FnDef { decorators, .. } => {
                assert_eq!(decorators[0].name, "patch");
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Tests — Phase 7, sub-step 7.0 (kwargs in decorators)
    // -----------------------------------------------------------------------

    #[test]
    fn decorator_without_kwargs_leaves_empty_vector() {
        // Regression: `@get("/x")` keeps `kwargs = []`.
        let stmt = parse_one_stmt("@get(\"/x\")\nfn h() => 0");
        match stmt {
            Stmt::FnDef { decorators, .. } => {
                assert_eq!(decorators[0].name, "get");
                assert_eq!(decorators[0].args.len(), 1);
                assert!(decorators[0].kwargs.is_empty());
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn decorator_kwarg_only_separates_key_and_value() {
        // `@server(docs=false)` — a single kwarg, no positionals.
        let stmt = parse_one_stmt("@server(docs=false)\nfn cfg() => 0");
        match stmt {
            Stmt::FnDef { decorators, .. } => {
                assert!(decorators[0].args.is_empty());
                assert_eq!(decorators[0].kwargs.len(), 1);
                assert_eq!(decorators[0].kwargs[0].0, "docs");
                assert_eq!(decorators[0].kwargs[0].1, Expr::Bool(false, Span::ZERO));
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn decorator_mixes_positional_and_kwargs_in_that_order() {
        // `@server(3000, host="0.0.0.0", docs=false)` —
        // 1 positional + 2 kwargs.
        let src = "@server(3000, host=\"0.0.0.0\", docs=false)\nfn cfg() => 0";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::FnDef { decorators, .. } => {
                let d = &decorators[0];
                assert_eq!(d.name, "server");
                assert_eq!(d.args, vec![Expr::Int(3000, Span::ZERO)]);
                assert_eq!(d.kwargs.len(), 2);
                assert_eq!(d.kwargs[0].0, "host");
                assert_eq!(d.kwargs[0].1, Expr::Str("0.0.0.0".into(), Span::ZERO));
                assert_eq!(d.kwargs[1].0, "docs");
                assert_eq!(d.kwargs[1].1, Expr::Bool(false, Span::ZERO));
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn decorator_positional_after_kwarg_is_error() {
        // `@get(a=1, "/x")` — kwarg first, then positional: rejected.
        let err = parse_program_str("@get(a=1, \"/x\")\nfn h() => 0").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
        assert!(
            err.message.contains("positional"),
            "expected message about positional/kwarg order, was: {}",
            err.message
        );
    }

    #[test]
    fn decorator_duplicate_kwarg_is_error() {
        // `@server(host="a", host="b")` — mismo kwarg dos veces.
        let err = parse_program_str("@server(host=\"a\", host=\"b\")\nfn cfg() => 0").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
        assert!(
            err.message.contains("host"),
            "expected message to cite the duplicated key, was: {}",
            err.message
        );
    }

    #[test]
    fn decorator_eqeq_in_arg_is_not_confused_with_kwarg() {
        // `@deco(a == b)` — one positional arg `BinOp(Eq)`, NOT a kwarg
        // with key `a` and value `b`. The lexer makes the difference:
        // `==` is `Token::EqEq`, while `=` is `Token::Eq`.
        let stmt = parse_one_stmt("@deco(a == b)\nfn h() => 0");
        match stmt {
            Stmt::FnDef { decorators, .. } => {
                let d = &decorators[0];
                assert!(d.kwargs.is_empty());
                assert_eq!(d.args.len(), 1);
                assert!(matches!(
                    d.args[0],
                    Expr::BinOp {
                        op: BinOpKind::Eq,
                        ..
                    }
                ));
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Final integration test of the Phase 2 criterion — NOW with interpolation
    // -----------------------------------------------------------------------

    /// Full Phase 2 success criterion at the parser level. The
    /// resulting AST matches exactly the one built by hand in
    /// `ast::tests::can_represent_phase2_success_program`.
    #[test]
    fn parses_phase2_success_program_end_to_end() {
        let src = "name = \"Fitz\"\nx = 10 + 5\nprint(\"Hola, {name}!\")\nfn double(n) => n * 2\nprint(double(x))";
        let program = parse_program_str(src).unwrap();
        assert_eq!(program.len(), 5);

        // 1. name = "Fitz"
        assert_eq!(
            program[0],
            Stmt::Assign {
                target: AssignTarget::Ident("name".into(), Span::default()),
                type_: None,
                value: Expr::Str("Fitz".into(), Span::ZERO),
                span: Span::ZERO
            }
        );

        // 2. x = 10 + 5
        assert_eq!(
            program[1],
            Stmt::Assign {
                target: AssignTarget::Ident("x".into(), Span::default()),
                type_: None,
                value: Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Int(10, Span::ZERO)),
                    right: Box::new(Expr::Int(5, Span::ZERO)),
                    span: Span::ZERO,
                },
                span: Span::ZERO
            }
        );

        // 3. print("Hola, {name}!")
        assert_eq!(
            program[2],
            Stmt::Expr(
                Expr::Call {
                    callee: Box::new(Expr::Ident("print".into(), Span::ZERO)),
                    args: vec![Expr::StrInterp(
                        vec![
                            StrPart::Lit("Hola, ".into()),
                            StrPart::Expr(Expr::Ident("name".into(), Span::ZERO), None),
                            StrPart::Lit("!".into()),
                        ],
                        Span::ZERO
                    )],
                    span: Span::ZERO,
                },
                Span::ZERO
            )
        );

        // 4. fn double(n) => n * 2
        assert_eq!(
            program[3],
            Stmt::FnDef {
                name: "double".into(),
                params: vec![Param {
                    name: "n".into(),
                    type_: None,
                    default: None,
                    varargs: false,
                    name_span: Span::default(),
                }],
                return_type: None,
                body: vec![Stmt::Return(
                    Expr::BinOp {
                        op: BinOpKind::Mul,
                        left: Box::new(Expr::Ident("n".into(), Span::ZERO)),
                        right: Box::new(Expr::Int(2, Span::ZERO)),
                        span: Span::ZERO,
                    },
                    Span::ZERO
                )],
                is_async: false,
                decorators: vec![],
                span: Span::ZERO
            }
        );

        // 5. print(double(x))
        assert_eq!(
            program[4],
            Stmt::Expr(
                Expr::Call {
                    callee: Box::new(Expr::Ident("print".into(), Span::ZERO)),
                    args: vec![Expr::Call {
                        callee: Box::new(Expr::Ident("double".into(), Span::ZERO)),
                        args: vec![Expr::Ident("x".into(), Span::ZERO)],
                        span: Span::ZERO,
                    }],
                    span: Span::ZERO,
                },
                Span::ZERO
            ),
        );
    }

    // -----------------------------------------------------------------------
    // Tests — Lists, maps, ranges, indexing (Phase 3, step 1)
    // -----------------------------------------------------------------------

    #[test]
    fn list_literal_empty() {
        assert_eq!(parse_expr("[]").unwrap(), Expr::List(vec![], Span::ZERO));
    }

    #[test]
    fn list_literal_single_element() {
        assert_eq!(
            parse_expr("[42]").unwrap(),
            Expr::List(vec![Expr::Int(42, Span::ZERO)], Span::ZERO)
        );
    }

    #[test]
    fn list_literal_multiple_elements() {
        assert_eq!(
            parse_expr("[1, 2, 3]").unwrap(),
            Expr::List(
                vec![
                    Expr::Int(1, Span::ZERO),
                    Expr::Int(2, Span::ZERO),
                    Expr::Int(3, Span::ZERO)
                ],
                Span::ZERO
            ),
        );
    }

    #[test]
    fn list_literal_trailing_comma() {
        assert_eq!(
            parse_expr("[1, 2,]").unwrap(),
            Expr::List(
                vec![Expr::Int(1, Span::ZERO), Expr::Int(2, Span::ZERO)],
                Span::ZERO
            ),
        );
    }

    #[test]
    fn list_literal_with_newlines_inside() {
        // Multi-line lists — newlines between elements are ignored.
        let src = "[\n  1,\n  2,\n  3,\n]";
        assert_eq!(
            parse_expr(src).unwrap(),
            Expr::List(
                vec![
                    Expr::Int(1, Span::ZERO),
                    Expr::Int(2, Span::ZERO),
                    Expr::Int(3, Span::ZERO)
                ],
                Span::ZERO
            ),
        );
    }

    #[test]
    fn list_literal_with_expressions() {
        // [a, b + 1, "hola"]
        assert_eq!(
            parse_expr(r#"[a, b + 1, "hola"]"#).unwrap(),
            Expr::List(
                vec![
                    Expr::Ident("a".into(), Span::ZERO),
                    Expr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(Expr::Ident("b".into(), Span::ZERO)),
                        right: Box::new(Expr::Int(1, Span::ZERO)),
                        span: Span::ZERO,
                    },
                    Expr::Str("hola".into(), Span::ZERO),
                ],
                Span::ZERO
            ),
        );
    }

    #[test]
    fn list_literal_nested() {
        // [[1, 2], [3, 4]]
        assert_eq!(
            parse_expr("[[1, 2], [3, 4]]").unwrap(),
            Expr::List(
                vec![
                    Expr::List(
                        vec![Expr::Int(1, Span::ZERO), Expr::Int(2, Span::ZERO)],
                        Span::ZERO
                    ),
                    Expr::List(
                        vec![Expr::Int(3, Span::ZERO), Expr::Int(4, Span::ZERO)],
                        Span::ZERO
                    ),
                ],
                Span::ZERO
            ),
        );
    }

    #[test]
    fn list_unclosed_errors() {
        let err = parse_expr("[1, 2").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn map_literal_empty() {
        assert_eq!(parse_expr("{}").unwrap(), Expr::Map(vec![], Span::ZERO));
    }

    #[test]
    fn map_literal_single_pair() {
        assert_eq!(
            parse_expr(r#"{"a": 1}"#).unwrap(),
            Expr::Map(
                vec![(Expr::Str("a".into(), Span::ZERO), Expr::Int(1, Span::ZERO))],
                Span::ZERO
            ),
        );
    }

    #[test]
    fn map_literal_multiple_pairs_preserves_order() {
        assert_eq!(
            parse_expr(r#"{"a": 1, "b": 2}"#).unwrap(),
            Expr::Map(
                vec![
                    (Expr::Str("a".into(), Span::ZERO), Expr::Int(1, Span::ZERO)),
                    (Expr::Str("b".into(), Span::ZERO), Expr::Int(2, Span::ZERO)),
                ],
                Span::ZERO
            ),
        );
    }

    #[test]
    fn map_literal_trailing_comma() {
        assert_eq!(
            parse_expr(r#"{"a": 1,}"#).unwrap(),
            Expr::Map(
                vec![(Expr::Str("a".into(), Span::ZERO), Expr::Int(1, Span::ZERO))],
                Span::ZERO
            ),
        );
    }

    #[test]
    fn map_literal_with_newlines() {
        let src = "{\n  \"a\": 1,\n  \"b\": 2,\n}";
        assert_eq!(
            parse_expr(src).unwrap(),
            Expr::Map(
                vec![
                    (Expr::Str("a".into(), Span::ZERO), Expr::Int(1, Span::ZERO)),
                    (Expr::Str("b".into(), Span::ZERO), Expr::Int(2, Span::ZERO)),
                ],
                Span::ZERO
            ),
        );
    }

    #[test]
    fn map_literal_missing_colon_errors() {
        let err = parse_expr(r#"{"a", "b"}"#).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
        assert!(err.message.contains(":"));
    }

    #[test]
    fn map_literal_nested_in_list() {
        // [{"k": 1}, {"k": 2}]
        assert_eq!(
            parse_expr(r#"[{"k": 1}, {"k": 2}]"#).unwrap(),
            Expr::List(
                vec![
                    Expr::Map(
                        vec![(Expr::Str("k".into(), Span::ZERO), Expr::Int(1, Span::ZERO))],
                        Span::ZERO
                    ),
                    Expr::Map(
                        vec![(Expr::Str("k".into(), Span::ZERO), Expr::Int(2, Span::ZERO))],
                        Span::ZERO
                    ),
                ],
                Span::ZERO
            ),
        );
    }

    #[test]
    fn range_simple_int_literals() {
        // 0..10
        assert_eq!(
            parse_expr("0..10").unwrap(),
            Expr::Range {
                start: Box::new(Expr::Int(0, Span::ZERO)),
                end: Box::new(Expr::Int(10, Span::ZERO)),
                inclusive: false,
                span: Span::ZERO,
            },
        );
    }

    #[test]
    fn range_with_expressions_as_ends() {
        // a..b+1 → a..(b+1) (range has lower precedence than '+')
        assert_eq!(
            parse_expr("a..b+1").unwrap(),
            Expr::Range {
                start: Box::new(Expr::Ident("a".into(), Span::ZERO)),
                end: Box::new(Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Ident("b".into(), Span::ZERO)),
                    right: Box::new(Expr::Int(1, Span::ZERO)),
                    span: Span::ZERO,
                }),
                inclusive: false,
                span: Span::ZERO,
            },
        );
    }

    #[test]
    fn range_precedence_below_comparison() {
        // 0..n < 10 → (0..n) < 10
        // (range has higher precedence than '<')
        assert_eq!(
            parse_expr("0..n < 10").unwrap(),
            Expr::BinOp {
                op: BinOpKind::Lt,
                left: Box::new(Expr::Range {
                    start: Box::new(Expr::Int(0, Span::ZERO)),
                    end: Box::new(Expr::Ident("n".into(), Span::ZERO)),
                    inclusive: false,
                    span: Span::ZERO,
                }),
                right: Box::new(Expr::Int(10, Span::ZERO)),
                span: Span::ZERO,
            },
        );
    }

    #[test]
    fn range_precedence_above_arithmetic() {
        // 1+2..3+4 → (1+2)..(3+4)
        assert_eq!(
            parse_expr("1+2..3+4").unwrap(),
            Expr::Range {
                start: Box::new(Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Int(1, Span::ZERO)),
                    right: Box::new(Expr::Int(2, Span::ZERO)),
                    span: Span::ZERO,
                }),
                end: Box::new(Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Int(3, Span::ZERO)),
                    right: Box::new(Expr::Int(4, Span::ZERO)),
                    span: Span::ZERO,
                }),
                inclusive: false,
                span: Span::ZERO,
            },
        );
    }

    #[test]
    fn range_chain_errors() {
        // 1..2..3 — not chainable
        let err = parse_expr("1..2..3").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
    }

    #[test]
    fn range_with_negative_int() {
        // -3..3 — unary minus applies to the first endpoint
        assert_eq!(
            parse_expr("-3..3").unwrap(),
            Expr::Range {
                start: Box::new(Expr::UnaryOp {
                    op: UnaryOpKind::Neg,
                    operand: Box::new(Expr::Int(3, Span::ZERO)),
                    span: Span::ZERO,
                }),
                end: Box::new(Expr::Int(3, Span::ZERO)),
                inclusive: false,
                span: Span::ZERO,
            },
        );
    }

    #[test]
    fn index_simple() {
        // xs[0]
        assert_eq!(
            parse_expr("xs[0]").unwrap(),
            Expr::Index {
                object: Box::new(Expr::Ident("xs".into(), Span::ZERO)),
                index: Box::new(Expr::Int(0, Span::ZERO)),
                span: Span::ZERO,
            },
        );
    }

    #[test]
    fn index_chained() {
        // m["a"][1]
        assert_eq!(
            parse_expr(r#"m["a"][1]"#).unwrap(),
            Expr::Index {
                object: Box::new(Expr::Index {
                    object: Box::new(Expr::Ident("m".into(), Span::ZERO)),
                    index: Box::new(Expr::Str("a".into(), Span::ZERO)),
                    span: Span::ZERO,
                }),
                index: Box::new(Expr::Int(1, Span::ZERO)),
                span: Span::ZERO,
            },
        );
    }

    #[test]
    fn index_on_list_literal() {
        // [1, 2, 3][1] — indexing directo sobre literal
        assert_eq!(
            parse_expr("[1, 2, 3][1]").unwrap(),
            Expr::Index {
                object: Box::new(Expr::List(
                    vec![
                        Expr::Int(1, Span::ZERO),
                        Expr::Int(2, Span::ZERO),
                        Expr::Int(3, Span::ZERO),
                    ],
                    Span::ZERO
                )),
                index: Box::new(Expr::Int(1, Span::ZERO)),
                span: Span::ZERO,
            },
        );
    }

    #[test]
    fn index_with_expression_as_index() {
        // xs[i + 1]
        assert_eq!(
            parse_expr("xs[i + 1]").unwrap(),
            Expr::Index {
                object: Box::new(Expr::Ident("xs".into(), Span::ZERO)),
                index: Box::new(Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Ident("i".into(), Span::ZERO)),
                    right: Box::new(Expr::Int(1, Span::ZERO)),
                    span: Span::ZERO,
                }),
                span: Span::ZERO,
            },
        );
    }

    #[test]
    fn index_unclosed_errors() {
        let err = parse_expr("xs[0").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn list_assignment_works() {
        // let xs = [1, 2, 3]
        let stmt = parse_one_stmt("let xs = [1, 2, 3]");
        assert_eq!(
            stmt,
            Stmt::Assign {
                target: AssignTarget::Ident("xs".into(), Span::default()),
                type_: None,
                value: Expr::List(
                    vec![
                        Expr::Int(1, Span::ZERO),
                        Expr::Int(2, Span::ZERO),
                        Expr::Int(3, Span::ZERO)
                    ],
                    Span::ZERO
                ),
                span: Span::ZERO
            },
        );
    }

    #[test]
    fn map_assignment_works() {
        // let m = {"a": 1, "b": 2}
        let stmt = parse_one_stmt(r#"let m = {"a": 1, "b": 2}"#);
        assert_eq!(
            stmt,
            Stmt::Assign {
                target: AssignTarget::Ident("m".into(), Span::default()),
                type_: None,
                value: Expr::Map(
                    vec![
                        (Expr::Str("a".into(), Span::ZERO), Expr::Int(1, Span::ZERO)),
                        (Expr::Str("b".into(), Span::ZERO), Expr::Int(2, Span::ZERO)),
                    ],
                    Span::ZERO
                ),
                span: Span::ZERO
            },
        );
    }

    // -----------------------------------------------------------------------
    // Tests — for loop (Phase 3, step 1)
    // -----------------------------------------------------------------------

    #[test]
    fn for_loop_over_list() {
        // for x in xs { print(x) }
        let stmt = parse_one_stmt("for x in xs { print(x) }");
        assert_eq!(
            stmt,
            Stmt::For {
                var: Pattern::Ident("x".into(), Span::default()),
                iter: Expr::Ident("xs".into(), Span::ZERO),
                body: vec![Stmt::Expr(
                    Expr::Call {
                        callee: Box::new(Expr::Ident("print".into(), Span::ZERO)),
                        args: vec![Expr::Ident("x".into(), Span::ZERO)],
                        span: Span::ZERO,
                    },
                    Span::ZERO
                )],
                label: None,
                span: Span::ZERO
            },
        );
    }

    #[test]
    fn for_loop_over_range() {
        // for i in 0..10 { print(i) }
        let stmt = parse_one_stmt("for i in 0..10 { print(i) }");
        match stmt {
            Stmt::For {
                var, iter, body, ..
            } => {
                assert_eq!(var, Pattern::Ident("i".into(), Span::default()));
                assert_eq!(
                    iter,
                    Expr::Range {
                        start: Box::new(Expr::Int(0, Span::ZERO)),
                        end: Box::new(Expr::Int(10, Span::ZERO)),
                        inclusive: false,
                        span: Span::ZERO,
                    },
                );
                assert_eq!(body.len(), 1);
            }
            other => panic!("expected For, got {:?}", other),
        }
    }

    #[test]
    fn for_loop_over_list_literal() {
        // for x in [1, 2, 3] { print(x) }
        let stmt = parse_one_stmt("for x in [1, 2, 3] { print(x) }");
        match stmt {
            Stmt::For { iter, .. } => {
                assert_eq!(
                    iter,
                    Expr::List(
                        vec![
                            Expr::Int(1, Span::ZERO),
                            Expr::Int(2, Span::ZERO),
                            Expr::Int(3, Span::ZERO)
                        ],
                        Span::ZERO
                    )
                );
            }
            other => panic!("expected For, got {:?}", other),
        }
    }

    #[test]
    fn for_loop_with_break_and_continue() {
        let src = "for i in 0..10 { if i == 5 { break } else { continue } }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::For { body, .. } => {
                // The body has a single statement: an if/else with break/continue.
                assert_eq!(body.len(), 1);
            }
            other => panic!("expected For, got {:?}", other),
        }
    }

    #[test]
    fn for_loop_missing_in_errors() {
        // for x 0..10 { ... } — falta `in`
        let err = parse_program_str("for x 0..10 {}").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
        assert!(err.message.contains("in"));
    }

    #[test]
    fn for_loop_missing_var_errors() {
        // for in xs { ... } — falta variable
        let err = parse_program_str("for in xs {}").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    // -----------------------------------------------------------------------
    // Tests — range patterns in match (Phase 3, step 1)
    // -----------------------------------------------------------------------

    #[test]
    fn pattern_range_simple() {
        // match n { 0..10 => "chico", _ => "grande" }
        let src = "match n { 0..10 => \"chico\", _ => \"grande\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(arms.len(), 2);
                assert_eq!(
                    arms[0].pattern,
                    Pattern::Range {
                        start: 0,
                        end: 10,
                        inclusive: false
                    }
                );
                assert_eq!(arms[1].pattern, Pattern::Wildcard);
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn pattern_range_with_negatives() {
        // match n { -10..0 => "negativo", 0..10 => "chico", _ => "grande" }
        let src = "match n { -10..0 => \"negativo\", 0..10 => \"chico\", _ => \"grande\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(arms.len(), 3);
                assert_eq!(
                    arms[0].pattern,
                    Pattern::Range {
                        start: -10,
                        end: 0,
                        inclusive: false
                    }
                );
                assert_eq!(
                    arms[1].pattern,
                    Pattern::Range {
                        start: 0,
                        end: 10,
                        inclusive: false
                    }
                );
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn pattern_range_both_negative() {
        // match n { -5..-1 => "neg" }
        let src = "match n { -5..-1 => \"neg\", _ => \"otro\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(
                    arms[0].pattern,
                    Pattern::Range {
                        start: -5,
                        end: -1,
                        inclusive: false
                    }
                );
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn pattern_int_without_dotdot_is_still_int() {
        // Sanity check: the change for Pattern::Range must not break Pattern::Int.
        let src = "match n { 42 => \"sí\", _ => \"no\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(arms[0].pattern, Pattern::Int(42));
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn pattern_range_with_float_is_error() {
        // 0..1.5 — float as an endpoint is not supported in patterns
        let src = "match n { 0..1.5 => \"x\", _ => \"y\" }";
        let err = parse_program_str(src).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
    }

    // -----------------------------------------------------------------------
    // Tests — Or-patterns (R.2.1, mini-phase R)
    // -----------------------------------------------------------------------

    #[test]
    fn or_pattern_two_literals() {
        // match n { 1 | 2 => "ok", _ => "x" }
        let src = "match n { 1 | 2 => \"ok\", _ => \"x\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(
                    arms[0].pattern,
                    Pattern::Or(vec![Pattern::Int(1), Pattern::Int(2)])
                );
                assert_eq!(arms[1].pattern, Pattern::Wildcard);
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn or_pattern_three_strings() {
        let src = "match d { \"a\" | \"b\" | \"c\" => 1, _ => 0 }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(
                    arms[0].pattern,
                    Pattern::Or(vec![
                        Pattern::Str("a".into()),
                        Pattern::Str("b".into()),
                        Pattern::Str("c".into()),
                    ])
                );
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn or_pattern_single_pat_without_pipe_does_not_wrap() {
        // Sanity: a simple pattern without `|` is not wrapped in Or.
        let src = "match n { 1 => \"x\", _ => \"y\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(arms[0].pattern, Pattern::Int(1));
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn or_pattern_mixes_range_and_literal() {
        // match n { 0 | 5..=10 => "ok", _ => "no" }
        let src = "match n { 0 | 5..=10 => \"ok\", _ => \"no\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(
                    arms[0].pattern,
                    Pattern::Or(vec![
                        Pattern::Int(0),
                        Pattern::Range {
                            start: 5,
                            end: 10,
                            inclusive: true
                        },
                    ])
                );
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn or_pattern_with_ok_err_wildcard() {
        // match r { Ok(_) | Err(_) => "siempre" }
        let src = "match r { Ok(_) | Err(_) => \"siempre\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(
                    arms[0].pattern,
                    Pattern::Or(vec![Pattern::OkWildcard, Pattern::ErrWildcard])
                );
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn or_pattern_with_binding_ident_is_error() {
        // match n { 1 | x => "x" } — `x` is an Ident binding, vetoed.
        let src = "match n { 1 | x => \"x\" }";
        let err = parse_program_str(src).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
        assert!(err.message.contains("or-patterns do not allow bindings"));
    }

    #[test]
    fn or_pattern_with_ok_binding_is_error() {
        // match r { Ok(x) | Err(_) => "x" } — `Ok(x)` binding, vetado.
        let src = "match r { Ok(x) | Err(_) => \"x\" }";
        let err = parse_program_str(src).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
    }

    // -----------------------------------------------------------------------
    // Tests — Guards in match (R.2.2)
    // -----------------------------------------------------------------------

    #[test]
    fn guard_simple_over_ident_pattern() {
        let src = "match n { x if x > 10 => \"grande\", _ => \"chico\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(arms[0].pattern, Pattern::Ident("x".into(), Span::default()));
                assert!(arms[0].guard.is_some());
                assert!(arms[1].guard.is_none());
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn guard_over_ok_binding() {
        let src = "match r { Ok(v) if v > 0 => \"pos\", _ => \"x\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(
                    arms[0].pattern,
                    Pattern::OkBinding("v".into(), Span::default())
                );
                assert!(arms[0].guard.is_some());
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn guard_combined_with_range_pattern() {
        let src = "match n { 0..=10 if n > 5 => \"alto\", _ => \"x\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert!(matches!(
                    arms[0].pattern,
                    Pattern::Range {
                        start: 0,
                        end: 10,
                        inclusive: true
                    }
                ));
                assert!(arms[0].guard.is_some());
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn guard_combined_with_or_pattern() {
        let src = "match n { 1 | 2 | 3 if n > 1 => \"x\", _ => \"y\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert!(matches!(arms[0].pattern, Pattern::Or(_)));
                assert!(arms[0].guard.is_some());
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn guard_is_complex_expression() {
        // The guard can be any boolean expression.
        let src = "match n { x if x > 0 and x < 100 => \"ok\", _ => \"x\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert!(arms[0].guard.is_some());
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Tests — Compound operators +=/-=/*=//= (R.2.3)
    // -----------------------------------------------------------------------

    #[test]
    fn compound_plus_eq_over_ident() {
        // `x += 5` must desugar to `x = x + 5`.
        let src = "x += 5";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                target: AssignTarget::Ident(name, _),
                value,
                ..
            } => {
                assert_eq!(name, "x");
                match value {
                    Expr::BinOp {
                        op, left, right, ..
                    } => {
                        assert_eq!(op, BinOpKind::Add);
                        assert!(matches!(*left, Expr::Ident(ref n, _) if n == "x"));
                        assert!(matches!(*right, Expr::Int(5, _)));
                    }
                    other => panic!("expected BinOp, was {:?}", other),
                }
            }
            other => panic!("expected Stmt::Assign, was {:?}", other),
        }
    }

    #[test]
    fn compound_minus_eq_over_ident() {
        let src = "x -= 3";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                value: Expr::BinOp { op, .. },
                ..
            } => {
                assert_eq!(op, BinOpKind::Sub);
            }
            other => panic!("expected Stmt::Assign with BinOp Sub, was {:?}", other),
        }
    }

    #[test]
    fn compound_star_eq_over_ident() {
        let src = "x *= 7";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                value: Expr::BinOp { op, .. },
                ..
            } => {
                assert_eq!(op, BinOpKind::Mul);
            }
            other => panic!("expected Stmt::Assign with BinOp Mul, was {:?}", other),
        }
    }

    #[test]
    fn compound_slash_eq_over_ident() {
        let src = "x /= 2";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                value: Expr::BinOp { op, .. },
                ..
            } => {
                assert_eq!(op, BinOpKind::Div);
            }
            other => panic!("expected Stmt::Assign with BinOp Div, was {:?}", other),
        }
    }

    #[test]
    fn compound_plus_eq_over_field() {
        // `c.count += 1` desugar a `c.count = c.count + 1`.
        let src = "c.count += 1";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                target: AssignTarget::Field { field, .. },
                value,
                ..
            } => {
                assert_eq!(field, "count");
                assert!(matches!(
                    value,
                    Expr::BinOp {
                        op: BinOpKind::Add,
                        ..
                    }
                ));
            }
            other => panic!("expected Stmt::Assign Field, was {:?}", other),
        }
    }

    #[test]
    fn compound_plus_eq_over_index() {
        // `xs[0] += 10` desugar a `xs[0] = xs[0] + 10`.
        let src = "xs[0] += 10";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                target: AssignTarget::Index { .. },
                value,
                ..
            } => {
                assert!(matches!(
                    value,
                    Expr::BinOp {
                        op: BinOpKind::Add,
                        ..
                    }
                ));
            }
            other => panic!("expected Stmt::Assign Index, was {:?}", other),
        }
    }

    #[test]
    fn compound_rhs_complete_expression() {
        // The RHS must parse as a full expression, not just a literal.
        let src = "x += a + b * 2";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                value:
                    Expr::BinOp {
                        op: BinOpKind::Add,
                        right,
                        ..
                    },
                ..
            } => {
                // right is `a + b * 2` too
                assert!(matches!(*right, Expr::BinOp { .. }));
            }
            other => panic!("expected Stmt::Assign with compound RHS, was {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Tests — Custom methods on `type` (R.3, mini-phase R)
    // -----------------------------------------------------------------------

    #[test]
    fn type_def_with_only_fields_still_works() {
        let src = "type User { id: Int, name: Str }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::TypeDef {
                name,
                fields,
                methods,
                ..
            } => {
                assert_eq!(name, "User");
                assert_eq!(fields.len(), 2);
                assert!(methods.is_empty());
            }
            other => panic!("expected TypeDef, was {:?}", other),
        }
    }

    #[test]
    fn type_def_with_one_simple_method() {
        let src = "type User {\n\
                       name: Str\n\
                       fn greet() -> Str { return \"hola\" }\n\
                   }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::TypeDef {
                fields, methods, ..
            } => {
                assert_eq!(fields.len(), 1);
                assert_eq!(methods.len(), 1);
                assert_eq!(methods[0].name, "greet");
                assert!(methods[0].params.is_empty());
                assert!(methods[0].return_type.is_some());
                assert!(!methods[0].is_async);
            }
            other => panic!("expected TypeDef, was {:?}", other),
        }
    }

    #[test]
    fn type_def_with_method_with_params() {
        let src = "type User {\n\
                       age: Int\n\
                       fn older_than(target: Int) -> Bool { return age > target }\n\
                   }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::TypeDef { methods, .. } => {
                assert_eq!(methods.len(), 1);
                assert_eq!(methods[0].params.len(), 1);
                assert_eq!(methods[0].params[0].name, "target");
            }
            other => panic!("expected TypeDef, was {:?}", other),
        }
    }

    #[test]
    fn type_def_with_async_method() {
        let src = "type User {\n\
                       id: Int\n\
                       async fn fetch() -> Str { return \"...\" }\n\
                   }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::TypeDef { methods, .. } => {
                assert!(methods[0].is_async);
            }
            other => panic!("expected TypeDef, was {:?}", other),
        }
    }

    #[test]
    fn type_def_with_arrow_method() {
        // `fn greet() => "x"` desugars to a body with Return.
        let src = "type User {\n\
                       fn name_str() -> Str => \"ada\"\n\
                   }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::TypeDef { methods, .. } => {
                assert_eq!(methods[0].body.len(), 1);
                assert!(matches!(methods[0].body[0], Stmt::Return(_, _)));
            }
            other => panic!("expected TypeDef, was {:?}", other),
        }
    }

    #[test]
    fn type_def_mixes_fields_and_methods() {
        let src = "type Counter {\n\
                       count: Int\n\
                       fn inc() -> Int { return count + 1 }\n\
                       step: Int = 1\n\
                       fn double() -> Int { return count * 2 }\n\
                   }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::TypeDef {
                fields, methods, ..
            } => {
                assert_eq!(fields.len(), 2, "fields: {:?}", fields);
                assert_eq!(methods.len(), 2);
            }
            other => panic!("expected TypeDef, was {:?}", other),
        }
    }

    #[test]
    fn type_def_with_method_without_body_is_error() {
        // `fn name()` without a body is an error (we don't allow abstract methods).
        let src = "type X { fn f() }";
        let err = parse_program_str(src).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    // -----------------------------------------------------------------------
    // Tests — Struct literals (Phase 3, step 2)
    //
    // The parser recognizes `Name { field: expr, ... }` as
    // `Expr::StructLit` inside an Ident's postfix. The ambiguity with
    // blocks is resolved via the `no_struct_literal` flag: in
    // if/while/for/match conditions, struct literals require parens.
    // -----------------------------------------------------------------------

    #[test]
    fn struct_lit_simple_in_assignment() {
        let src = "let u = User { id: 1, name: \"x\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                value: Expr::StructLit {
                    type_name, fields, ..
                },
                ..
            } => {
                assert_eq!(type_name, "User");
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].0, "id");
                assert_eq!(fields[0].1, Expr::Int(1, Span::ZERO));
                assert_eq!(fields[1].0, "name");
                assert_eq!(fields[1].1, Expr::Str("x".into(), Span::ZERO));
            }
            other => panic!("expected Assign(StructLit), got {:?}", other),
        }
    }

    #[test]
    fn struct_lit_empty() {
        let src = "let u = Empty {}";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                value: Expr::StructLit {
                    type_name, fields, ..
                },
                ..
            } => {
                assert_eq!(type_name, "Empty");
                assert!(fields.is_empty());
            }
            other => panic!("expected empty StructLit, got {:?}", other),
        }
    }

    #[test]
    fn struct_lit_with_trailing_comma() {
        let src = "let u = User { id: 1, name: \"x\", }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                value: Expr::StructLit { fields, .. },
                ..
            } => {
                assert_eq!(fields.len(), 2);
            }
            other => panic!("expected StructLit, got {:?}", other),
        }
    }

    #[test]
    fn struct_lit_multiline_with_newlines_between_fields() {
        // No comma between lines — newline as separator.
        let src = "let u = User {\n    id: 1\n    name: \"x\"\n}";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                value: Expr::StructLit { fields, .. },
                ..
            } => {
                assert_eq!(fields.len(), 2);
            }
            other => panic!("expected multiline StructLit, got {:?}", other),
        }
    }

    #[test]
    fn struct_lit_nested() {
        let src = "let o = Order { user: User { id: 1, name: \"x\" } }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                value: Expr::StructLit {
                    type_name, fields, ..
                },
                ..
            } => {
                assert_eq!(type_name, "Order");
                assert_eq!(fields.len(), 1);
                assert_eq!(fields[0].0, "user");
                match &fields[0].1 {
                    Expr::StructLit {
                        type_name: inner_name,
                        fields: inner_fields,
                        ..
                    } => {
                        assert_eq!(inner_name, "User");
                        assert_eq!(inner_fields.len(), 2);
                    }
                    other => panic!("expected nested StructLit, got {:?}", other),
                }
            }
            other => panic!("expected Assign(StructLit), got {:?}", other),
        }
    }

    #[test]
    fn struct_lit_with_complex_expression_as_value() {
        // The field value can be any expression.
        let src = "let p = Point { x: 1 + 2, y: f(3) }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                value: Expr::StructLit { fields, .. },
                ..
            } => {
                assert!(matches!(fields[0].1, Expr::BinOp { .. }));
                assert!(matches!(fields[1].1, Expr::Call { .. }));
            }
            other => panic!("expected StructLit, got {:?}", other),
        }
    }

    #[test]
    fn struct_lit_as_function_argument() {
        // Inside parens there is no ambiguity — the struct literal
        // is allowed without wrapping.
        let src = "print(User { id: 1, name: \"x\" })";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Call { args, .. }, _) => {
                assert_eq!(args.len(), 1);
                assert!(matches!(args[0], Expr::StructLit { .. }));
            }
            other => panic!("expected Call with StructLit arg, got {:?}", other),
        }
    }

    #[test]
    fn struct_lit_inside_list() {
        // Inside `[...]` each item is delimited by `,` or `]` —
        // no ambiguity with blocks.
        let src = "let xs = [User { id: 1, name: \"a\" }, User { id: 2, name: \"b\" }]";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                value: Expr::List(items, _),
                ..
            } => {
                assert_eq!(items.len(), 2);
                assert!(matches!(items[0], Expr::StructLit { .. }));
                assert!(matches!(items[1], Expr::StructLit { .. }));
            }
            other => panic!("expected List with StructLits, got {:?}", other),
        }
    }

    #[test]
    fn struct_lit_in_return() {
        let src = "fn make() => User { id: 1, name: \"x\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::FnDef { body, .. } => match &body[0] {
                Stmt::Return(Expr::StructLit { .. }, _) => {}
                other => panic!("expected Return(StructLit), got {:?}", other),
            },
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn struct_lit_as_index_and_index_receiver() {
        // The struct literal can appear inside an indexing `[...]`.
        let src = "let v = m[Key { id: 1 }]";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                value: Expr::Index { index, .. },
                ..
            } => {
                assert!(matches!(*index, Expr::StructLit { .. }));
            }
            other => panic!("expected Index with StructLit, got {:?}", other),
        }
    }

    #[test]
    fn while_with_struct_literal_without_parens_gives_error_with_hint() {
        // `while User { id: 1 } { body }` — the parser sees the `{`
        // after `User` and, since we are in a condition, detects
        // that it looks like a struct literal and emits an error
        // with a hint to use parens.
        let src = "while User { id: 1 } { print(x) }";
        let err = parse_program_str(src).unwrap_err();
        let msg = err.message.to_lowercase();
        assert!(
            msg.contains("parentheses") || msg.contains("parenthesis"),
            "the error should mention parentheses, was: {}",
            err.message
        );
    }

    #[test]
    fn if_with_struct_literal_without_parens_gives_error_with_hint() {
        let src = "if User { id: 1 } == other { print(x) }";
        let err = parse_program_str(src).unwrap_err();
        let msg = err.message.to_lowercase();
        assert!(
            msg.contains("parentheses") || msg.contains("parenthesis"),
            "the error should mention parentheses, was: {}",
            err.message
        );
    }

    #[test]
    fn for_with_struct_literal_without_parens_gives_error_with_hint() {
        let src = "for u in User { id: 1 } { print(u) }";
        let err = parse_program_str(src).unwrap_err();
        let msg = err.message.to_lowercase();
        assert!(
            msg.contains("parentheses") || msg.contains("parenthesis"),
            "the error should mention parentheses, was: {}",
            err.message
        );
    }

    #[test]
    fn match_with_struct_literal_without_parens_gives_error_with_hint() {
        let src = "match User { id: 1 } { _ => \"x\" }";
        let err = parse_program_str(src).unwrap_err();
        let msg = err.message.to_lowercase();
        assert!(
            msg.contains("parentheses") || msg.contains("parenthesis"),
            "the error should mention parentheses, was: {}",
            err.message
        );
    }

    #[test]
    fn if_with_struct_literal_wrapped_in_parens_parses() {
        // With parens yes: the condition sees a full struct literal.
        let src = "if (User { id: 1 }) == other { print(x) }";
        let stmts = parse_program_str(src).expect("should parse with parentheses");
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Stmt::Expr(Expr::If { condition, .. }, _) => match condition.as_ref() {
                Expr::BinOp { left, .. } => {
                    assert!(matches!(**left, Expr::StructLit { .. }));
                }
                other => panic!("expected BinOp as condition, got {:?}", other),
            },
            other => panic!("expected If, got {:?}", other),
        }
    }

    #[test]
    fn while_with_ident_and_block_without_struct_pattern_still_works() {
        // `while x { print(x) }` — the block body does not have a
        // struct-literal shape, so the flag lets the `{` through for
        // `parse_block` to grab.
        let src = "while x { print(x) }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::While {
                condition, body, ..
            } => {
                assert_eq!(condition, Expr::Ident("x".into(), Span::ZERO));
                assert_eq!(body.len(), 1);
            }
            other => panic!("expected While, got {:?}", other),
        }
    }

    #[test]
    fn for_over_list_of_struct_literals_parses() {
        // Inside `[...]` struct literals are allowed
        // even when the `for` is in no_struct_literal mode.
        let src = "for u in [User { id: 1, name: \"a\" }] { print(u) }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::For {
                var, iter, body, ..
            } => {
                assert_eq!(var, Pattern::Ident("u".into(), Span::default()));
                assert!(matches!(iter, Expr::List(_, _)));
                assert_eq!(body.len(), 1);
            }
            other => panic!("expected For, got {:?}", other),
        }
    }

    #[test]
    fn if_with_typed_assignment_in_block_not_confused_with_struct_literal() {
        // `if x { y: Int = 1 }` — the block has a typed assignment,
        // which shares the initial shape with a struct literal
        // (`Ident :`).
        // The parser must distinguish and let the block through without error.
        let src = "if x { y: Int = 1 }";
        let stmts = parse_program_str(src).expect("should parse");
        assert_eq!(stmts.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Tests — Result + Ok/Err + ? (Phase 3, step 3)
    // -----------------------------------------------------------------------

    #[test]
    fn ok_ctor_parses_to_expr_ok() {
        let e = parse_expr("Ok(42)").unwrap();
        assert_eq!(e, Expr::Ok(Box::new(Expr::Int(42, Span::ZERO)), Span::ZERO));
    }

    #[test]
    fn err_ctor_parses_to_expr_err() {
        let e = parse_expr(r#"Err("boom")"#).unwrap();
        assert_eq!(
            e,
            Expr::Err(Box::new(Expr::Str("boom".into(), Span::ZERO)), Span::ZERO)
        );
    }

    #[test]
    fn ok_with_complex_expression_inside() {
        // Ok(1 + 2 * 3) → Ok(Add(1, Mul(2, 3)))
        let e = parse_expr("Ok(1 + 2 * 3)").unwrap();
        let inner = Expr::BinOp {
            op: BinOpKind::Add,
            left: Box::new(Expr::Int(1, Span::ZERO)),
            right: Box::new(Expr::BinOp {
                op: BinOpKind::Mul,
                left: Box::new(Expr::Int(2, Span::ZERO)),
                right: Box::new(Expr::Int(3, Span::ZERO)),
                span: Span::ZERO,
            }),
            span: Span::ZERO,
        };
        assert_eq!(e, Expr::Ok(Box::new(inner), Span::ZERO));
    }

    #[test]
    fn ok_without_arguments_is_arity_error() {
        let err = parse_expr("Ok()").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
        assert!(err.message.contains("`Ok`") && err.message.contains("1 argument"));
    }

    #[test]
    fn err_with_two_arguments_is_arity_error() {
        let err = parse_expr("Err(1, 2)").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
        assert!(err.message.contains("`Err`"));
    }

    #[test]
    fn try_postfix_wraps_expression() {
        // f(x)? → Try(Call(f, [x]))
        let e = parse_expr("f(x)?").unwrap();
        assert_eq!(
            e,
            Expr::Try(
                Box::new(Expr::Call {
                    callee: Box::new(Expr::Ident("f".into(), Span::ZERO)),
                    args: vec![Expr::Ident("x".into(), Span::ZERO)],
                    span: Span::ZERO,
                }),
                Span::ZERO
            ),
        );
    }

    #[test]
    fn try_over_identifier() {
        // x? → Try(Ident("x"))
        let e = parse_expr("x?").unwrap();
        assert_eq!(
            e,
            Expr::Try(Box::new(Expr::Ident("x".into(), Span::ZERO)), Span::ZERO)
        );
    }

    #[test]
    fn try_chains_with_field_access() {
        // get(id)?.name → Field { object: Try(Call(get, [id])), field: "name" }
        let e = parse_expr("get(id)?.name").unwrap();
        let inner_call = Expr::Call {
            callee: Box::new(Expr::Ident("get".into(), Span::ZERO)),
            args: vec![Expr::Ident("id".into(), Span::ZERO)],
            span: Span::ZERO,
        };
        assert_eq!(
            e,
            Expr::Field {
                object: Box::new(Expr::Try(Box::new(inner_call), Span::ZERO)),
                field: "name".into(),
                span: Span::ZERO,
            },
        );
    }

    #[test]
    fn try_nested_with_ok_and_err() {
        // Ok(get(id)?) → Ok(Try(Call(get, [id])))
        let e = parse_expr("Ok(get(id)?)").unwrap();
        let inner = Expr::Try(
            Box::new(Expr::Call {
                callee: Box::new(Expr::Ident("get".into(), Span::ZERO)),
                args: vec![Expr::Ident("id".into(), Span::ZERO)],
                span: Span::ZERO,
            }),
            Span::ZERO,
        );
        assert_eq!(e, Expr::Ok(Box::new(inner), Span::ZERO));
    }

    #[test]
    fn match_with_ok_and_err_patterns_parses() {
        // Sanity: the pattern parser already supported Ok/Err; verify
        // that the whole set (match + Ok/Err in value) composes well.
        let stmt = parse_one_stmt(
            "match Ok(1) {\n\
             \tOk(v) => v\n\
             \tErr(e) => -1\n\
             }",
        );
        if let Stmt::Expr(Expr::Match { value, arms, .. }, _) = stmt {
            assert_eq!(
                *value,
                Expr::Ok(Box::new(Expr::Int(1, Span::ZERO)), Span::ZERO)
            );
            assert_eq!(arms.len(), 2);
            assert_eq!(
                arms[0].pattern,
                Pattern::OkBinding("v".into(), Span::default())
            );
            assert_eq!(
                arms[1].pattern,
                Pattern::ErrBinding("e".into(), Span::default())
            );
        } else {
            panic!("expected a match");
        }
    }

    // -----------------------------------------------------------------------
    // Tests — Modules / import (Phase 3, step 5)
    // -----------------------------------------------------------------------

    #[test]
    fn import_simple_parses() {
        // `import utils` → Stmt::Import with alias None.
        assert_eq!(
            parse_one_stmt("import utils"),
            Stmt::Import {
                path: vec!["utils".into()],
                alias: None,
                span: Span::ZERO
            },
        );
    }

    #[test]
    fn dotted_import_accumulates_segments() {
        assert_eq!(
            parse_one_stmt("import sub.foo.bar"),
            Stmt::Import {
                path: vec!["sub".into(), "foo".into(), "bar".into()],
                alias: None,
                span: Span::ZERO
            },
        );
    }

    #[test]
    fn import_without_name_is_error() {
        let err = parse_program_str("import").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn import_path_ending_in_dot_is_error() {
        // `import foo.` — missing the following segment.
        let err = parse_program_str("import foo.").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn from_import_one_name() {
        // `from utils import slugify`
        assert_eq!(
            parse_one_stmt("from utils import slugify"),
            Stmt::FromImport {
                path: vec!["utils".into()],
                names: vec![("slugify".into(), None)],
                span: Span::ZERO
            },
        );
    }

    #[test]
    fn from_import_multiple_names_comma_separated() {
        // `from utils import a, b, c`
        assert_eq!(
            parse_one_stmt("from utils import a, b, c"),
            Stmt::FromImport {
                path: vec!["utils".into()],
                names: vec![("a".into(), None), ("b".into(), None), ("c".into(), None),],
                span: Span::ZERO
            },
        );
    }

    #[test]
    fn from_import_with_dotted_path() {
        // `from sub.foo import bar`
        assert_eq!(
            parse_one_stmt("from sub.foo import bar"),
            Stmt::FromImport {
                path: vec!["sub".into(), "foo".into()],
                names: vec![("bar".into(), None)],
                span: Span::ZERO
            },
        );
    }

    #[test]
    fn from_import_accepts_trailing_comma() {
        // `from utils import a, b,` — coma final permitida.
        assert_eq!(
            parse_one_stmt("from utils import a, b,"),
            Stmt::FromImport {
                path: vec!["utils".into()],
                names: vec![("a".into(), None), ("b".into(), None)],
                span: Span::ZERO
            },
        );
    }

    // ---- Mini-batch Mln — multi-line from foo import ( ... ) ----

    #[test]
    fn mln_from_import_parens_single_line() {
        // `from utils import (a, b, c)` — parens without newlines.
        assert_eq!(
            parse_one_stmt("from utils import (a, b, c)"),
            Stmt::FromImport {
                path: vec!["utils".into()],
                names: vec![("a".into(), None), ("b".into(), None), ("c".into(), None),],
                span: Span::ZERO,
            },
        );
    }

    #[test]
    fn mln_from_import_parens_multi_line_canonical() {
        // Pythonic idiom: `(`/`)` wrapping a list of
        // names separated by commas and newlines.
        let src = "from utils import (\n\
                       a,\n\
                       b,\n\
                       c,\n\
                   )";
        assert_eq!(
            parse_one_stmt(src),
            Stmt::FromImport {
                path: vec!["utils".into()],
                names: vec![("a".into(), None), ("b".into(), None), ("c".into(), None),],
                span: Span::ZERO,
            },
        );
    }

    #[test]
    fn mln_from_import_parens_with_mixed_aliases() {
        // Aliases inside the multi-line parens work the same as in
        // single-line.
        let src = "from utils import (\n\
                       greet,\n\
                       shout as scream,\n\
                       PREFIX as P,\n\
                       User as Persona,\n\
                   )";
        assert_eq!(
            parse_one_stmt(src),
            Stmt::FromImport {
                path: vec!["utils".into()],
                names: vec![
                    ("greet".into(), None),
                    ("shout".into(), Some("scream".into())),
                    ("PREFIX".into(), Some("P".into())),
                    ("User".into(), Some("Persona".into())),
                ],
                span: Span::ZERO,
            },
        );
    }

    #[test]
    fn mln_from_import_parens_without_trailing_comma() {
        // The last name before `)` does not require a comma.
        let src = "from utils import (\n\
                       a,\n\
                       b\n\
                   )";
        assert_eq!(
            parse_one_stmt(src),
            Stmt::FromImport {
                path: vec!["utils".into()],
                names: vec![("a".into(), None), ("b".into(), None)],
                span: Span::ZERO,
            },
        );
    }

    #[test]
    fn mln_from_import_parens_unclosed_is_error() {
        let err = parse_program_str("from utils import (a, b\n").unwrap_err();
        assert!(
            err.message.contains("')'") || err.message.contains("import"),
            "expected message about `)` or import, was: {}",
            err.message
        );
    }

    #[test]
    fn from_without_import_is_error() {
        // `from utils slugify` — missing the `import` keyword.
        let err = parse_program_str("from utils slugify").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn from_import_without_names_is_error() {
        // `from utils import` — at least one name is required.
        let err = parse_program_str("from utils import").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    // PreF8.4: alias tests.

    #[test]
    fn import_with_alias_parses_the_alias() {
        // `import utils as u`
        assert_eq!(
            parse_one_stmt("import utils as u"),
            Stmt::Import {
                path: vec!["utils".into()],
                alias: Some("u".into()),
                span: Span::ZERO,
            },
        );
    }

    #[test]
    fn dotted_import_with_alias() {
        // `import sub.foo as f` — the alias applies to the full binding.
        assert_eq!(
            parse_one_stmt("import sub.foo as f"),
            Stmt::Import {
                path: vec!["sub".into(), "foo".into()],
                alias: Some("f".into()),
                span: Span::ZERO,
            },
        );
    }

    #[test]
    fn from_import_with_simple_alias() {
        // `from utils import slugify as s`
        assert_eq!(
            parse_one_stmt("from utils import slugify as s"),
            Stmt::FromImport {
                path: vec!["utils".into()],
                names: vec![("slugify".into(), Some("s".into()))],
                span: Span::ZERO,
            },
        );
    }

    #[test]
    fn from_import_alias_mixed_with_and_without() {
        // `from foo import a as x, b, c as z`
        assert_eq!(
            parse_one_stmt("from foo import a as x, b, c as z"),
            Stmt::FromImport {
                path: vec!["foo".into()],
                names: vec![
                    ("a".into(), Some("x".into())),
                    ("b".into(), None),
                    ("c".into(), Some("z".into())),
                ],
                span: Span::ZERO,
            },
        );
    }

    #[test]
    fn import_as_without_ident_is_error() {
        // `import foo as` — missing ident after `as`.
        let err = parse_program_str("import foo as").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn from_import_as_without_ident_is_error() {
        // `from foo import bar as` — missing ident after `as`.
        let err = parse_program_str("from foo import bar as").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    // -----------------------------------------------------------------------
    // Tests — TypeExpr in annotations (Phase 5, step 5.1)
    //
    // We cover the three places the parser asks for a type:
    //   - `let x: T = ...` (Stmt::Assign.type_)
    //   - `fn f(p: T) -> T` (Param.type_ and FnDef.return_type)
    //   - `type X { f: T }` (Field.type_)
    //
    // The scope of step 5.1 is syntactic structure: the parser
    // builds the right TypeExpr. Semantic validation (that the name
    // exists, that the generic arity is correct, etc.) is left to
    // 5.2 — the type checker.
    // -----------------------------------------------------------------------

    /// Helper: extract the `TypeExpr` from a simple `let x: T = 0`.
    fn parse_assign_type(src: &str) -> TypeExpr {
        match parse_one_stmt(src) {
            Stmt::Assign { type_: Some(t), .. } => t,
            other => panic!("expected Stmt::Assign with type, got {:?}", other),
        }
    }

    #[test]
    fn type_expr_simple_parses_as_named() {
        let t = parse_assign_type("let x: Int = 0");
        assert_eq!(t, TypeExpr::named("Int"));
    }

    #[test]
    fn type_expr_generic_one_argument() {
        // List<Int>
        let t = parse_assign_type("let xs: List<Int> = []");
        assert_eq!(
            t,
            TypeExpr::Generic {
                name: "List".into(),
                args: vec![TypeExpr::named("Int")],
            },
        );
    }

    #[test]
    fn type_expr_generic_two_arguments() {
        // Map<Str, User>
        let t = parse_assign_type("let m: Map<Str, User> = {}");
        assert_eq!(
            t,
            TypeExpr::Generic {
                name: "Map".into(),
                args: vec![TypeExpr::named("Str"), TypeExpr::named("User")],
            },
        );
    }

    #[test]
    fn type_expr_generic_nested() {
        // Result<List<User>>  — two consecutive `>` to close.
        let t = parse_assign_type("let r: Result<List<User>> = Ok([])");
        assert_eq!(
            t,
            TypeExpr::Generic {
                name: "Result".into(),
                args: vec![TypeExpr::Generic {
                    name: "List".into(),
                    args: vec![TypeExpr::named("User")],
                }],
            },
        );
    }

    #[test]
    fn type_expr_nullable_over_named() {
        // User?
        let t = parse_assign_type("let u: User? = null");
        assert_eq!(t, TypeExpr::Nullable(Box::new(TypeExpr::named("User"))),);
    }

    #[test]
    fn type_expr_nullable_over_generic() {
        // List<Int>?  — the `?` applies to the whole atom, not the last arg.
        let t = parse_assign_type("let xs: List<Int>? = null");
        assert_eq!(
            t,
            TypeExpr::Nullable(Box::new(TypeExpr::Generic {
                name: "List".into(),
                args: vec![TypeExpr::named("Int")],
            })),
        );
    }

    #[test]
    fn type_expr_nullable_inside_generic() {
        // List<Int?>  — the `?` is inside, not outside.
        let t = parse_assign_type("let xs: List<Int?> = []");
        assert_eq!(
            t,
            TypeExpr::Generic {
                name: "List".into(),
                args: vec![TypeExpr::Nullable(Box::new(TypeExpr::named("Int")))],
            },
        );
    }

    #[test]
    fn type_expr_in_param_and_return_of_fndef() {
        // fn pick(xs: List<Int>) -> Result<Int> { return Ok(0) }
        let stmt = parse_one_stmt("fn pick(xs: List<Int>) -> Result<Int> { return Ok(0) }");
        match stmt {
            Stmt::FnDef {
                params,
                return_type,
                ..
            } => {
                assert_eq!(params.len(), 1);
                assert_eq!(
                    params[0].type_,
                    Some(TypeExpr::Generic {
                        name: "List".into(),
                        args: vec![TypeExpr::named("Int")],
                    }),
                );
                assert_eq!(
                    return_type,
                    Some(TypeExpr::Generic {
                        name: "Result".into(),
                        args: vec![TypeExpr::named("Int")],
                    }),
                );
            }
            other => panic!("expected FnDef, got {:?}", other),
        }
    }

    #[test]
    fn type_expr_in_typedef_field_with_nullable() {
        // type User { id: Int, tags: List<Str>?, email: Str? }
        let stmt = parse_one_stmt("type User { id: Int, tags: List<Str>?, email: Str? }");
        match stmt {
            Stmt::TypeDef { fields, .. } => {
                assert_eq!(fields.len(), 3);
                assert_eq!(fields[0].type_, TypeExpr::named("Int"));
                assert_eq!(
                    fields[1].type_,
                    TypeExpr::Nullable(Box::new(TypeExpr::Generic {
                        name: "List".into(),
                        args: vec![TypeExpr::named("Str")],
                    })),
                );
                assert!(fields[1].type_.is_nullable());
                assert_eq!(
                    fields[2].type_,
                    TypeExpr::Nullable(Box::new(TypeExpr::named("Str"))),
                );
            }
            other => panic!("expected TypeDef, got {:?}", other),
        }
    }

    #[test]
    fn type_expr_generic_empty_is_error() {
        // `List<>` should not parse: at least one argument is required.
        let err = parse_program_str("let xs: List<> = []").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn type_expr_function_simple() {
        // Fn(Int) -> Int
        let t = parse_assign_type("let f: Fn(Int) -> Int = null");
        assert_eq!(
            t,
            TypeExpr::Function {
                params: vec![TypeExpr::named("Int")],
                ret: Box::new(TypeExpr::named("Int")),
            },
        );
    }

    #[test]
    fn type_expr_function_without_params() {
        // Fn() -> Str
        let t = parse_assign_type("let f: Fn() -> Str = null");
        assert_eq!(
            t,
            TypeExpr::Function {
                params: vec![],
                ret: Box::new(TypeExpr::named("Str")),
            },
        );
    }

    #[test]
    fn type_expr_function_multiple_params() {
        // Fn(Int, Str, Bool) -> User
        let t = parse_assign_type("let f: Fn(Int, Str, Bool) -> User = null");
        assert_eq!(
            t,
            TypeExpr::Function {
                params: vec![
                    TypeExpr::named("Int"),
                    TypeExpr::named("Str"),
                    TypeExpr::named("Bool"),
                ],
                ret: Box::new(TypeExpr::named("User")),
            },
        );
    }

    #[test]
    fn type_expr_function_nested_as_param() {
        // Fn(Fn(Int) -> Int, Int) -> Int — higher-order anotado.
        let t = parse_assign_type("let h: Fn(Fn(Int) -> Int, Int) -> Int = null");
        assert_eq!(
            t,
            TypeExpr::Function {
                params: vec![
                    TypeExpr::Function {
                        params: vec![TypeExpr::named("Int")],
                        ret: Box::new(TypeExpr::named("Int")),
                    },
                    TypeExpr::named("Int"),
                ],
                ret: Box::new(TypeExpr::named("Int")),
            },
        );
    }

    #[test]
    fn type_expr_function_without_arrow_is_error() {
        // `Fn(Int)` without `-> R` → explicit parser error.
        let err = parse_program_str("let f: Fn(Int) = null").unwrap_err();
        assert!(err.message.contains("'->"));
    }

    #[test]
    fn type_expr_generic_unclosed_is_error() {
        // The final `>` is missing.
        let err = parse_program_str("let xs: List<Int = []").unwrap_err();
        // The parser fails when it tries to consume `>` and finds `=`.
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn type_expr_annotation_without_name_is_error() {
        // `:` with no type after.
        let err = parse_program_str("let x: = 1").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn type_expr_display_round_trip_over_complex_case() {
        // The display must reproduce the form written in the source.
        let t = parse_assign_type("let m: Map<Str, Result<List<User>?>> = {}");
        assert_eq!(t.display_name(), "Map<Str, Result<List<User>?>>");
    }

    // ---------------------------------------------------------------------
    // Phase 9.0.1 — parse_with_recovery
    // ---------------------------------------------------------------------

    /// Helper that tokenizes and runs `parse_with_recovery`. Returns
    /// `(stmts, errors)`.
    fn parse_recovering(src: &str) -> (Program, Vec<FitzError>) {
        let tokens = tokenize(src).expect("source must tokenize without error");
        parse_with_recovery(tokens)
    }

    #[test]
    fn recovery_valid_program_does_not_accumulate_errors() {
        // Smoke: the recovering API produces the same AST as strict
        // over error-free code, with an empty `Vec<FitzError>`.
        let src = "let x = 1\nlet y = 2\nprint(x + y)";
        let (stmts_rec, errors) = parse_recovering(src);
        assert!(errors.is_empty(), "no se esperaban errores: {:?}", errors);
        let stmts_strict = parse(tokenize(src).unwrap()).unwrap();
        assert_eq!(stmts_rec, stmts_strict);
    }

    #[test]
    fn recovery_broken_stmt_at_top_level_inserts_error_and_continues() {
        // The `1 +` leaves a pending binop — fails. The parser
        // synchronizes to the next Newline and continues with
        // `let y = 2`, which must parse OK.
        let src = "let x = 1 +\nlet y = 2";
        let (stmts, errors) = parse_recovering(src);
        assert_eq!(errors.len(), 1, "exactamente un error: {:?}", errors);
        assert_eq!(stmts.len(), 2);
        assert!(matches!(stmts[0], Stmt::Error(_)));
        assert!(matches!(
            stmts[1],
            Stmt::Assign {
                target: AssignTarget::Ident(ref n, _), ..
            } if n == "y"
        ));
    }

    #[test]
    fn recovery_two_consecutive_broken_stmts_emit_two_errors() {
        // Two broken lines: the parser must accumulate two errors,
        // not get lost.
        let src = "let a = 1 +\nlet b = *\nlet c = 3";
        let (stmts, errors) = parse_recovering(src);
        assert_eq!(errors.len(), 2);
        assert_eq!(stmts.len(), 3);
        assert!(matches!(stmts[0], Stmt::Error(_)));
        assert!(matches!(stmts[1], Stmt::Error(_)));
        assert!(matches!(
            stmts[2],
            Stmt::Assign {
                target: AssignTarget::Ident(ref n, _), ..
            } if n == "c"
        ));
    }

    #[test]
    fn recovery_broken_stmt_inside_block_inserts_error_and_continues() {
        // The `if` body has a broken stmt followed by a valid one.
        let src = "if (x) {\n  let a = 1 +\n  let b = 2\n}";
        let (stmts, errors) = parse_recovering(src);
        assert_eq!(errors.len(), 1);
        assert_eq!(stmts.len(), 1);
        // The `if` is Stmt::Expr(Expr::If { ... }, _). Inspect its body.
        match &stmts[0] {
            Stmt::Expr(Expr::If { then, .. }, _) => {
                assert_eq!(then.len(), 2);
                assert!(matches!(then[0], Stmt::Error(_)));
                assert!(matches!(
                    then[1],
                    Stmt::Assign {
                        target: AssignTarget::Ident(ref n, _), ..
                    } if n == "b"
                ));
            }
            other => panic!("expected Stmt::Expr(Expr::If), got {:?}", other),
        }
    }

    #[test]
    fn recovery_error_span_points_to_token_where_stmt_started() {
        // The broken stmt starts at line 1, col 1 (the `let`). The
        // `Stmt::Error` span reflects this so the LSP underlines it
        // from the start of the stmt, not from the odd character.
        let src = "let x = +\nlet y = 2";
        let (stmts, _errors) = parse_recovering(src);
        match &stmts[0] {
            Stmt::Error(span) => {
                assert_eq!(span.line, 1);
                assert_eq!(span.column, 1);
            }
            other => panic!("expected Stmt::Error, got {:?}", other),
        }
    }

    #[test]
    fn recovery_error_carries_line_and_column_of_problematic_token() {
        // The reported error must point at the token where the
        // problem was detected (the lone `+`), not at the start of
        // the stmt — useful for the LSP underlining the squiggly.
        let src = "let x = +\nlet y = 2";
        let (_stmts, errors) = parse_recovering(src);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, 1);
        // The `+` is at column 9.
        assert_eq!(errors[0].column, 9);
    }

    #[test]
    fn recovery_unexpected_eof_accumulates_as_error() {
        // `let x =` leaves a pending expression at the end of file.
        // The parser must accumulate the error and return what it
        // could build.
        let src = "let x =";
        let (stmts, errors) = parse_recovering(src);
        assert_eq!(errors.len(), 1);
        // The broken stmt comes through as Error.
        assert!(matches!(stmts.last(), Some(Stmt::Error(_))));
    }

    #[test]
    fn recovery_error_cap_cuts_accumulation() {
        // Generate a program with more than MAX_RECOVERED_ERRORS
        // broken lines. Verify the cap is respected.
        let n = MAX_RECOVERED_ERRORS + 50;
        let lines: Vec<String> = (0..n).map(|_| "let a = +".to_string()).collect();
        let src = lines.join("\n");
        let (_stmts, errors) = parse_recovering(&src);
        assert_eq!(errors.len(), MAX_RECOVERED_ERRORS);
    }

    #[test]
    fn recovery_fn_with_broken_body_preserves_structure() {
        // The `fn foo` body has a broken stmt. The key point: the
        // FnDef remains a FnDef (with a body containing Stmt::Error),
        // it isn't discarded entirely. The stmt after the fn close
        // also parses OK.
        let src = "fn foo() {\n  let a = +\n}\nlet b = 1";
        let (stmts, errors) = parse_recovering(src);
        assert_eq!(errors.len(), 1);
        assert_eq!(stmts.len(), 2);
        match &stmts[0] {
            Stmt::FnDef { name, body, .. } => {
                assert_eq!(name, "foo");
                assert_eq!(body.len(), 1);
                assert!(matches!(body[0], Stmt::Error(_)));
            }
            other => panic!("expected Stmt::FnDef, got {:?}", other),
        }
        assert!(matches!(
            stmts[1],
            Stmt::Assign {
                target: AssignTarget::Ident(ref n, _), ..
            } if n == "b"
        ));
    }

    #[test]
    fn recovery_parse_strict_still_aborts_at_first_error() {
        // Key guarantee: strict `parse()` does NOT change behavior.
        // It still returns `Err` on the first error. The strict CLI
        // (`fitz run`/`build`/`check`) keeps working the same.
        let src = "let x = +\nlet y = 2";
        let err = parse(tokenize(src).unwrap()).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    // ---------------------------------------------------------------------
    // Mini-batch C — list comprehensions.
    // ---------------------------------------------------------------------

    /// Extract the value of a top-level `let x = <expr>` and return
    /// it. Useful for tests that build small programs and want to
    /// inspect the first parsed `Expr`.
    fn parse_first_let_value(src: &str) -> Expr {
        let stmts = parse(tokenize(src).expect("tokenize")).expect("parse");
        match stmts.into_iter().next().expect("al menos un stmt") {
            Stmt::Assign { value, .. } => value,
            other => panic!("expected Stmt::Assign, got {:?}", other),
        }
    }

    #[test]
    fn comprehension_parses_basic_case() {
        let v = parse_first_let_value("let ys = [x for x in xs]");
        match v {
            Expr::ListComp {
                expr,
                var,
                iter,
                filter,
                ..
            } => {
                assert!(matches!(*expr, Expr::Ident(ref n, _) if n == "x"));
                assert!(matches!(var, Pattern::Ident(ref n, _) if n == "x"));
                assert!(matches!(*iter, Expr::Ident(ref n, _) if n == "xs"));
                assert!(filter.is_none());
            }
            other => panic!("expected ListComp, got {:?}", other),
        }
    }

    // Mini-batch Up — tuple destructuring in list comprehension.
    #[test]
    fn up_comprehension_accepts_tuple_destructuring() {
        let v = parse_first_let_value("let ys = [a + b for (a, b) in pairs]");
        match v {
            Expr::ListComp { var, .. } => {
                if let Pattern::Tuple(subs) = var {
                    assert_eq!(subs.len(), 2);
                    assert!(matches!(subs[0], Pattern::Ident(ref n, _) if n == "a"));
                    assert!(matches!(subs[1], Pattern::Ident(ref n, _) if n == "b"));
                } else {
                    panic!("expected Pattern::Tuple, saw {:?}", var);
                }
            }
            other => panic!("expected ListComp, got {:?}", other),
        }
    }

    #[test]
    fn comprehension_parses_with_inline_filter() {
        let v = parse_first_let_value("let ys = [x for x in xs if x > 0]");
        match v {
            Expr::ListComp { filter, .. } => {
                assert!(filter.is_some(), "inline filter must be present");
            }
            other => panic!("expected ListComp, got {:?}", other),
        }
    }

    #[test]
    fn comprehension_parses_over_range() {
        let v = parse_first_let_value("let ys = [x * 2 for x in 0..10]");
        match v {
            Expr::ListComp { iter, .. } => {
                assert!(matches!(*iter, Expr::Range { .. }));
            }
            other => panic!("expected ListComp, got {:?}", other),
        }
    }

    #[test]
    fn single_element_list_not_confused_with_comprehension() {
        // `[42]` is a single-element list, NOT a comprehension.
        // The parser only detects a comprehension if, after the first expr,
        // `for` follows (not `,` or `]`).
        let v = parse_first_let_value("let xs = [42]");
        match v {
            Expr::List(items, _) => assert_eq!(items.len(), 1),
            other => panic!("expected List, got {:?}", other),
        }
    }

    // ---------------------------------------------------------------
    // Mini-batch Fm — format specs in interpolation.
    // ---------------------------------------------------------------

    fn extract_first_strinterp_spec(src: &str) -> Option<crate::ast::FormatSpec> {
        match parse_first_let_value(src) {
            Expr::StrInterp(parts, _) => parts.into_iter().find_map(|p| match p {
                StrPart::Expr(_, spec) => spec,
                _ => None,
            }),
            other => panic!("expected StrInterp, got {:?}", other),
        }
    }

    #[test]
    fn format_spec_precision_float_parses() {
        let spec = extract_first_strinterp_spec(r#"let r = "{x:.2f}""#).unwrap();
        assert_eq!(spec.precision, Some(2));
        assert!(matches!(
            spec.kind,
            Some(crate::ast::FormatKind::FixedLower)
        ));
    }

    #[test]
    fn format_spec_width_int_zero_pad() {
        let spec = extract_first_strinterp_spec(r#"let r = "{n:05d}""#).unwrap();
        assert_eq!(spec.width, Some(5));
        assert!(spec.zero_pad);
        assert!(matches!(spec.kind, Some(crate::ast::FormatKind::Decimal)));
    }

    #[test]
    fn format_spec_align_right_with_width() {
        let spec = extract_first_strinterp_spec(r#"let r = "{x:>10}""#).unwrap();
        assert!(matches!(spec.align, Some(crate::ast::FormatAlign::Right)));
        assert_eq!(spec.width, Some(10));
    }

    #[test]
    fn format_spec_fill_align_custom() {
        let spec = extract_first_strinterp_spec(r#"let r = "{x:*>5}""#).unwrap();
        assert_eq!(spec.fill, Some('*'));
        assert!(matches!(spec.align, Some(crate::ast::FormatAlign::Right)));
        assert_eq!(spec.width, Some(5));
    }

    #[test]
    fn format_spec_grouping_and_precision_together() {
        // `,.2f` — thousands separator + 2 decimal places.
        let spec = extract_first_strinterp_spec(r#"let r = "{x:,.2f}""#).unwrap();
        assert_eq!(spec.grouping, Some(','));
        assert_eq!(spec.precision, Some(2));
    }

    #[test]
    fn format_spec_hex_alternate() {
        let spec = extract_first_strinterp_spec(r#"let r = "{n:#x}""#).unwrap();
        assert!(spec.alternate);
        assert!(matches!(spec.kind, Some(crate::ast::FormatKind::HexLower)));
    }

    #[test]
    fn interpolation_without_spec_still_works_compat() {
        // Classic case without `:` — the second field of StrPart::Expr is None.
        let value = parse_first_let_value(r#"let r = "hola {name}""#);
        match value {
            Expr::StrInterp(parts, _) => {
                let has_none = parts.iter().any(|p| matches!(p, StrPart::Expr(_, None)));
                assert!(has_none, "expected StrPart::Expr(_, None) without spec");
            }
            other => panic!("expected StrInterp, got {:?}", other),
        }
    }

    // ---------------------------------------------------------------
    // Mini-batch Md — for with Pattern in `var`.
    // ---------------------------------------------------------------

    #[test]
    fn for_with_tuple_pattern_parses() {
        // `for (k, v) in m { ... }` with a Pattern::Tuple of 2 idents.
        let stmt = parse_one_stmt("for (k, v) in m { print(k) }");
        match stmt {
            Stmt::For { var, .. } => match var {
                Pattern::Tuple(subs) => {
                    assert_eq!(subs.len(), 2);
                    assert!(matches!(subs[0], Pattern::Ident(ref n, _) if n == "k"));
                    assert!(matches!(subs[1], Pattern::Ident(ref n, _) if n == "v"));
                }
                other => panic!("expected Pattern::Tuple, gave {:?}", other),
            },
            other => panic!("expected Stmt::For, gave {:?}", other),
        }
    }

    #[test]
    fn for_with_wildcard_pattern_parses() {
        // `for _ in 0..10 { ... }` with Pattern::Wildcard.
        let stmt = parse_one_stmt("for _ in 0..10 { print(\"x\") }");
        match stmt {
            Stmt::For { var, .. } => {
                assert!(matches!(var, Pattern::Wildcard));
            }
            other => panic!("expected Stmt::For, gave {:?}", other),
        }
    }

    #[test]
    fn for_with_simple_ident_still_works() {
        // Regression: `for x in xs` with Pattern::Ident.
        let stmt = parse_one_stmt("for x in xs { print(x) }");
        match stmt {
            Stmt::For { var, .. } => {
                assert_eq!(var, Pattern::Ident("x".into(), Span::default()));
            }
            other => panic!("expected Stmt::For, gave {:?}", other),
        }
    }

    // ---- Mini-batch Fp — default params ----

    #[test]
    fn fp_param_with_default_int_parses() {
        let stmt = parse_one_stmt("fn f(x: Int = 5) -> Int { return x }");
        match stmt {
            Stmt::FnDef { params, .. } => {
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].name, "x");
                assert!(params[0].default.is_some(), "expected default");
                if let Some(Expr::Int(5, _)) = params[0].default {
                } else {
                    panic!("expected default Int(5), gave {:?}", params[0].default);
                }
            }
            other => panic!("expected FnDef, gave {:?}", other),
        }
    }

    #[test]
    fn fp_param_default_str_parses() {
        let stmt = parse_one_stmt("fn greet(name: Str = \"amigo\") -> Str { return name }");
        match stmt {
            Stmt::FnDef { params, .. } => {
                assert_eq!(params.len(), 1);
                match &params[0].default {
                    Some(Expr::Str(s, _)) => assert_eq!(s, "amigo"),
                    other => panic!("expected Str default, gave {:?}", other),
                }
            }
            other => panic!("expected FnDef, gave {:?}", other),
        }
    }

    #[test]
    fn fp_mixes_required_and_default_parses() {
        let stmt = parse_one_stmt("fn f(a: Int, b: Int = 10) -> Int { return a + b }");
        match stmt {
            Stmt::FnDef { params, .. } => {
                assert_eq!(params.len(), 2);
                assert!(params[0].default.is_none(), "a must NOT have default");
                assert!(params[1].default.is_some(), "b MUST have default");
            }
            other => panic!("expected FnDef, gave {:?}", other),
        }
    }

    #[test]
    fn fp_required_after_default_is_error() {
        // Python rule: once a param has a default, all following
        // params must have defaults too. `fn f(a = 1, b)` must be rejected.
        let result = parse_program_str("fn f(a: Int = 1, b: Int) -> Int { return a + b }");
        assert!(result.is_err(), "expected error, gave {:?}", result);
        let msg = result.unwrap_err().message;
        assert!(
            msg.contains("default") && msg.contains("b"),
            "expected message to contain 'default' and 'b', was: {}",
            msg
        );
    }

    #[test]
    fn fp_param_default_without_type_parses() {
        // `fn f(x = 5)` without a type annotation. Gradual: default
        // yes, but the param type stays as Any.
        let stmt = parse_one_stmt("fn f(x = 5) { return x }");
        match stmt {
            Stmt::FnDef { params, .. } => {
                assert!(params[0].type_.is_none());
                assert!(params[0].default.is_some());
            }
            other => panic!("expected FnDef, gave {:?}", other),
        }
    }

    // ---- Mini-batch Fp.2 — varargs ----

    #[test]
    fn fp2_param_varargs_parses() {
        let stmt = parse_one_stmt("fn sum(...xs: Int) -> Int { return 0 }");
        match stmt {
            Stmt::FnDef { params, .. } => {
                assert_eq!(params.len(), 1);
                assert!(params[0].varargs, "expected varargs=true");
                assert_eq!(params[0].name, "xs");
            }
            other => panic!("expected FnDef, gave {:?}", other),
        }
    }

    #[test]
    fn fp2_varargs_only_last_is_error() {
        let result = parse_program_str("fn f(...xs: Int, ...ys: Int) -> Int { return 0 }");
        assert!(result.is_err(), "expected error for duplicate varargs");
    }

    #[test]
    fn fp2_param_after_varargs_is_error() {
        let result = parse_program_str("fn f(...xs: Int, y: Int) -> Int { return 0 }");
        assert!(result.is_err(), "expected error for param after varargs");
    }

    #[test]
    fn fp2_varargs_with_default_is_error() {
        let result = parse_program_str("fn f(...xs: Int = 5) -> Int { return 0 }");
        assert!(result.is_err(), "expected error for varargs with default");
    }

    #[test]
    fn fp2_mixes_required_and_varargs_parses() {
        let stmt = parse_one_stmt("fn f(a: Str, ...xs: Int) -> Int { return 0 }");
        match stmt {
            Stmt::FnDef { params, .. } => {
                assert_eq!(params.len(), 2);
                assert!(!params[0].varargs);
                assert!(params[1].varargs);
            }
            other => panic!("expected FnDef, gave {:?}", other),
        }
    }

    // ---- Mini-batch Fp.3 — named args ----

    #[test]
    fn fp3_call_with_named_arg_emits_named_arg() {
        let src = "let r = f(name: 1)";
        let program = parse_program_str(src).expect("parse OK");
        match &program[0] {
            Stmt::Assign { value, .. } => {
                if let Expr::Call { args, .. } = value {
                    assert_eq!(args.len(), 1);
                    if let Expr::NamedArg { name, .. } = &args[0] {
                        assert_eq!(name, "name");
                    } else {
                        panic!("expected NamedArg, gave {:?}", args[0]);
                    }
                } else {
                    panic!("expected Call, gave {:?}", value);
                }
            }
            other => panic!("expected Assign, gave {:?}", other),
        }
    }

    #[test]
    fn fp3_positional_after_named_is_error() {
        let result = parse_program_str("let r = f(name: 1, 2)");
        assert!(result.is_err(), "expected error positional-after-named");
    }

    // ---- Mini-batch Sp.2 — return in match arm ----

    #[test]
    fn sp2_match_arm_with_return_parses_as_stmt_return() {
        let src = "fn f(n: Int) -> Str {\n  match n {\n    0 => return \"zero\"\n    _ => \"other\"\n  }\n  return \"end\"\n}";
        let stmt = parse_one_stmt(src);
        if let Stmt::FnDef { body, .. } = stmt {
            // Find the Expr::Match inside.
            if let Stmt::Expr(Expr::Match { arms, .. }, _) = &body[0] {
                // Arm 0: pattern Int(0) → Stmt::Return.
                assert_eq!(arms[0].body.len(), 1);
                assert!(matches!(arms[0].body[0], Stmt::Return(..)));
                // Arm 1: pattern Wildcard → Stmt::Expr("other").
                assert!(matches!(arms[1].body[0], Stmt::Expr(..)));
            } else {
                panic!("expected Stmt::Expr(Match), gave {:?}", body[0]);
            }
        } else {
            panic!("expected FnDef");
        }
    }

    #[test]
    fn sp2_match_arm_body_is_vec_stmt_of_1_for_simple_expr() {
        // Common case: arm body with 1 stmt expr.
        let src = "let r = match 1 { 0 => \"a\"\n_ => \"b\" }";
        let program = parse_program_str(src).expect("parse OK");
        if let Stmt::Assign {
            value: Expr::Match { arms, .. },
            ..
        } = &program[0]
        {
            for arm in arms {
                assert_eq!(arm.body.len(), 1);
                assert!(matches!(arm.body[0], Stmt::Expr(..)));
            }
        } else {
            panic!("expected Match");
        }
    }

    // ===== Phase 10.3.a — decorators on `type` and fields =====

    #[test]
    fn type_accepts_decorator_table_with_string() {
        let stmts = parse_ok("@table(\"users\") type User { id: Int, name: Str }");
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Stmt::TypeDef {
                name, decorators, ..
            } => {
                assert_eq!(name, "User");
                assert_eq!(decorators.len(), 1);
                assert_eq!(decorators[0].name, "table");
                assert_eq!(decorators[0].args.len(), 1);
                match &decorators[0].args[0] {
                    Expr::Str(s, _) => assert_eq!(s, "users"),
                    other => panic!("expected Expr::Str, was {:?}", other),
                }
            }
            other => panic!("expected TypeDef, was {:?}", other),
        }
    }

    #[test]
    fn type_accepts_decorator_table_without_args() {
        let stmts = parse_ok("@table type Post { id: Int }");
        match &stmts[0] {
            Stmt::TypeDef { decorators, .. } => {
                assert_eq!(decorators.len(), 1);
                assert_eq!(decorators[0].name, "table");
                assert!(decorators[0].args.is_empty());
            }
            other => panic!("expected TypeDef, was {:?}", other),
        }
    }

    #[test]
    fn type_accepts_stacked_decorators() {
        let stmts = parse_ok("@table(\"posts\") @soft_delete type Post { id: Int }");
        match &stmts[0] {
            Stmt::TypeDef { decorators, .. } => {
                assert_eq!(decorators.len(), 2);
                assert_eq!(decorators[0].name, "table");
                assert_eq!(decorators[1].name, "soft_delete");
            }
            other => panic!("expected TypeDef, was {:?}", other),
        }
    }

    #[test]
    fn field_accepts_decorator_primary() {
        let stmts =
            parse_ok("@table(\"users\") type User {\n  @primary\n  id: Int\n  name: Str\n}");
        match &stmts[0] {
            Stmt::TypeDef { fields, .. } => {
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].name, "id");
                assert_eq!(fields[0].decorators.len(), 1);
                assert_eq!(fields[0].decorators[0].name, "primary");
                assert_eq!(fields[1].name, "name");
                assert!(fields[1].decorators.is_empty());
            }
            other => panic!("expected TypeDef, was {:?}", other),
        }
    }

    #[test]
    fn field_accepts_decorator_column_with_kwargs() {
        // NOTE: kwarg `sql_type` (not `type`) — keyword collision.
        let stmts = parse_ok(
            "@table(\"users\") type User {\n  @column(name=\"user_id\", sql_type=\"bigint\")\n  id: Int\n}",
        );
        match &stmts[0] {
            Stmt::TypeDef { fields, .. } => {
                assert_eq!(fields[0].decorators.len(), 1);
                let d = &fields[0].decorators[0];
                assert_eq!(d.name, "column");
                assert_eq!(d.kwargs.len(), 2);
                let names: Vec<&str> = d.kwargs.iter().map(|(k, _)| k.as_str()).collect();
                assert!(names.contains(&"name"));
                assert!(names.contains(&"sql_type"));
            }
            other => panic!("expected TypeDef, was {:?}", other),
        }
    }

    #[test]
    fn field_accepts_stacked_decorators() {
        let stmts = parse_ok("@table type T {\n  @primary @unique @index\n  id: Int\n}");
        match &stmts[0] {
            Stmt::TypeDef { fields, .. } => {
                assert_eq!(fields[0].decorators.len(), 3);
                assert_eq!(fields[0].decorators[0].name, "primary");
                assert_eq!(fields[0].decorators[1].name, "unique");
                assert_eq!(fields[0].decorators[2].name, "index");
            }
            other => panic!("expected TypeDef, was {:?}", other),
        }
    }

    // ===== T3 — parser error paths (residual audit debt) =====

    #[test]
    fn fn_def_with_duplicate_params_is_error() {
        let tokens = tokenize("fn f(a: Int, a: Int) => a").expect("must tokenize");
        let err = parse(tokens).expect_err("must reject duplicated params");
        let msg = err.to_string();
        assert!(
            msg.contains("duplicated") && msg.contains('a'),
            "expected message that cites duplicated and param name, was: {}",
            msg
        );
    }

    #[test]
    fn fn_def_with_duplicate_params_without_type_is_error() {
        // Must also be caught when there is no type annotation.
        let tokens = tokenize("fn g(x, y, x) => 0").expect("must tokenize");
        let err = parse(tokens).expect_err("must reject duplicated params");
        let msg = err.to_string();
        assert!(
            msg.contains("duplicated"),
            "expected message about duplicated, was: {}",
            msg
        );
    }

    #[test]
    fn decorator_over_let_is_error() {
        // The parser only accepts decorators on `fn`, `async fn` or `type`.
        // `@x(1)\nlet y = 2` must fail with a clear message.
        let tokens = tokenize("@get(\"/x\")\nlet y = 2").expect("must tokenize");
        let err = parse(tokens).expect_err("must reject decorator over let");
        let msg = err.to_string();
        assert!(
            msg.contains("decorador") || msg.contains("`fn`") || msg.contains("`type`"),
            "expected message about decorator target, was: {}",
            msg
        );
    }

    #[test]
    fn decorator_over_bare_expression_is_error() {
        let tokens = tokenize("@table(\"x\")\n42").expect("must tokenize");
        let err = parse(tokens).expect_err("must reject decorator over expression");
        let msg = err.to_string();
        assert!(
            msg.contains("decorador") || msg.contains("`fn`") || msg.contains("`type`"),
            "expected message about decorator target, was: {}",
            msg
        );
    }

    #[test]
    fn string_with_invalid_escape_is_lexer_error() {
        // The lexer rejects unknown escapes like `\q`. Reported at
        // tokenize, not in parse, but it still counts as an error path of
        // pipeline lex-parse.
        let res = tokenize("let x = \"\\q\"");
        assert!(res.is_err(), "expected invalid escape error");
        let msg = res.unwrap_err().to_string();
        assert!(
            msg.contains("escape") || msg.contains("\\q"),
            "expected message about invalid escape, was: {}",
            msg
        );
    }

    #[test]
    fn parens_unclosed_in_expression_is_error() {
        let tokens = tokenize("let x = (1 + 2").expect("must tokenize");
        let err = parse(tokens).expect_err("must reject unclosed parenthesis");
        let msg = err.to_string();
        assert!(
            !msg.is_empty(),
            "expected non-empty message for unclosed parenthesis"
        );
    }

    #[test]
    fn brace_unclosed_in_block_is_error() {
        let tokens = tokenize("fn f() {\n  let x = 1").expect("must tokenize");
        let err = parse(tokens).expect_err("must reject unclosed brace");
        let msg = err.to_string();
        assert!(
            !msg.is_empty(),
            "expected non-empty message for unclosed brace"
        );
    }

    #[test]
    fn bracket_unclosed_in_list_literal_is_error() {
        let tokens = tokenize("let xs = [1, 2, 3").expect("must tokenize");
        let err = parse(tokens).expect_err("must reject unclosed bracket");
        let msg = err.to_string();
        assert!(
            !msg.is_empty(),
            "expected non-empty message for unclosed bracket"
        );
    }

    #[test]
    fn nested_brackets_mismatched_is_error() {
        // `[1, [2, 3]` — one bracket inside and one outside, the
        // outer one is missing its close.
        let tokens = tokenize("let xs = [1, [2, 3]").expect("must tokenize");
        let err = parse(tokens).expect_err("must reject mismatched nesting");
        let msg = err.to_string();
        assert!(
            !msg.is_empty(),
            "expected non-empty message for mismatched nesting"
        );
    }

    // ---- V1 (2026-06-05) — correct spans inside StrInterp ----

    /// Extract the first `Expr::StrInterp` from the `Program` and
    /// return its parts. Helper for the V1 tests.
    fn first_strinterp_parts(src: &str) -> Vec<StrPart> {
        let tokens = tokenize(src).expect("must tokenize");
        let program = parse(tokens).expect("must parse");
        for stmt in program {
            match stmt {
                Stmt::Expr(Expr::Call { args, .. }, _) => {
                    for a in args {
                        if let Expr::StrInterp(parts, _) = a {
                            return parts;
                        }
                    }
                }
                Stmt::Assign {
                    value: Expr::StrInterp(parts, _),
                    ..
                } => return parts,
                _ => {}
            }
        }
        panic!("did not find StrInterp in the program");
    }

    #[test]
    fn v1_ident_inside_strinterp_has_real_span_not_col_1() {
        // `print("hola {nombre}!")` — the Ident("nombre") span must
        // be (1, 14), NOT (1, 1) (which was the pre-V1 bug).
        let parts = first_strinterp_parts("print(\"hola {nombre}!\")\n");
        let ident = parts
            .iter()
            .find_map(|p| match p {
                StrPart::Expr(e, _) => Some(e),
                _ => None,
            })
            .expect("must have a StrPart::Expr");
        let span = ident.span();
        assert_eq!(
            span.line, 1,
            "span.line of inner Ident should be 1, was {}",
            span.line
        );
        // The `{` is at col 13 (after `print("hola `), the `n` of
        // `nombre` starts at col 14.
        assert_eq!(
            span.column, 14,
            "span.column of inner Ident should be 14, was {}",
            span.column
        );
    }

    #[test]
    fn v1_binop_inside_strinterp_recurses_to_operands() {
        // `print("v: {a + b}")` — the BinOp span and its operands
        // (Ident a, Ident b) must point at the real source.
        let parts = first_strinterp_parts("print(\"v: {a + b}\")\n");
        let expr = parts
            .iter()
            .find_map(|p| match p {
                StrPart::Expr(e, _) => Some(e),
                _ => None,
            })
            .expect("must have a StrPart::Expr");
        match expr {
            Expr::BinOp { left, right, .. } => {
                // `a` is at col 12, `b` at col 16.
                assert_eq!(left.span().line, 1);
                assert_eq!(left.span().column, 12, "col de `a`");
                assert_eq!(right.span().line, 1);
                assert_eq!(right.span().column, 16, "col de `b`");
            }
            other => panic!("expected BinOp, got {:?}", other),
        }
    }

    #[test]
    fn v1_call_inside_strinterp_walks_callee_and_args() {
        // `print("v: {f(x)}")` — Call.callee and Call.args get walked.
        let parts = first_strinterp_parts("print(\"v: {f(x)}\")\n");
        let expr = parts
            .iter()
            .find_map(|p| match p {
                StrPart::Expr(e, _) => Some(e),
                _ => None,
            })
            .expect("must have a StrPart::Expr");
        match expr {
            Expr::Call { callee, args, .. } => {
                // `f` is at col 12, `x` at col 14.
                assert_eq!(callee.span().column, 12);
                assert_eq!(args.first().expect("un arg").span().column, 14);
            }
            other => panic!("expected Call, got {:?}", other),
        }
    }

    #[test]
    fn v1_field_access_inside_strinterp_walks_object() {
        // `print("name: {u.name}")` — Field.object walkea.
        let parts = first_strinterp_parts("print(\"name: {u.name}\")\n");
        let expr = parts
            .iter()
            .find_map(|p| match p {
                StrPart::Expr(e, _) => Some(e),
                _ => None,
            })
            .expect("must have a StrPart::Expr");
        match expr {
            Expr::Field { object, .. } => {
                // `u` is at col 15.
                assert_eq!(object.span().column, 15);
            }
            other => panic!("expected Field, got {:?}", other),
        }
    }
}
