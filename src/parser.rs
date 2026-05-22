// parser.rs — Fase 2.3
//
// El parser convierte la lista plana de tokens del lexer en un AST.
// Implementación: recursive descent. Cada regla gramatical es una
// función; la precedencia de operadores está codificada en la jerarquía
// de llamadas (`equality` llama a `comparison`, que llama a `term`,
// etc.).
//
// Estado: en construcción. Ver docs/roadmap.md sección 2.3 para alcance
// y deuda explícita.

use crate::ast::{
    AssignTarget, BinOpKind, Decorator, Expr, Field, FormatSpec, MatchArm, MethodDef, Param,
    Pattern, Program, Span, Stmt, StrPart, TypeExpr, UnaryOpKind,
};
use crate::error::{ErrorKind, FitzError, FitzResult};
use crate::lexer::{tokenize, Token, TokenWithPos};

/// Estado del parseo. Privado al módulo.
///
/// El parser consume tokens de izquierda a derecha. `pos` apunta al
/// próximo token a leer. Cuando llegamos al final, `peek` devuelve
/// `&Token::EOF` (no `Option<&Token>`) — el lexer garantiza que el
/// último token siempre es EOF, así nos ahorramos `unwrap`s en cada
/// regla.
struct Parser {
    tokens: Vec<TokenWithPos>,
    pos: usize,
    /// Cuando es `true`, un `Ident` seguido de `{` NO se interpreta como
    /// struct literal — se rompe el postfix y el `{` queda para el
    /// caller (típicamente un bloque controlado: `if/while/for/match`).
    ///
    /// El flag se setea al entrar a la condición de `if`/`while`, al
    /// iterable de `for`, y al scrutinee de `match`. Se limpia en
    /// subexpresiones delimitadas (paréntesis, args de llamada,
    /// cuerpos de listas/mapas/struct literals, indexing), donde no
    /// hay ambigüedad con bloques.
    ///
    /// Si en modo bloqueado se ve un cuerpo que tiene pinta de struct
    /// literal (`{ Ident : ...`), el parser corta con un error
    /// explícito sugiriendo envolver en paréntesis.
    no_struct_literal: bool,

    /// Mini-tanda I.2 — slicing. Cuando es `true`, `range_expr` NO
    /// consume el operador `..`/`..=`: devuelve el start sin
    /// promoverlo a `Expr::Range`. El postfix `[` lo mira y arma el
    /// `Expr::Slice` correspondiente.
    in_slice_context: bool,

    /// Fase 9.0.1 (F15): si es `true`, los loops top-level de stmts
    /// (`parse_program` + `parse_block`) capturan errores de
    /// `parse_stmt`, los acumulan en `recovered_errors`, sincronizan
    /// hasta el próximo stmt-boundary (Newline/Semicolon/RBrace/EOF) y
    /// continúan con un `Stmt::Error(span)` en lugar del stmt original.
    /// Sirve para tooling externo (LSP) que necesita un AST parcial
    /// sobre buffers en construcción. `parse()` strict lo deja en
    /// `false`; `parse_with_recovery()` lo prende.
    recovery_mode: bool,

    /// Errores acumulados durante `parse_with_recovery`. En modo strict
    /// queda siempre vacío. Cota: ver `MAX_RECOVERED_ERRORS`.
    recovered_errors: Vec<FitzError>,
}

/// Cota dura de errores acumulados en `parse_with_recovery`. Cuando se
/// alcanza, el parser se rinde: descarta el resto del input y devuelve
/// lo que tiene. Protege contra cascadas runaway en buffers grandes
/// muy rotos. 100 cubre el caso 90% (~5-20 errores en un buffer LSP
/// real) con margen amplio.
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

    // ---------- navegación ----------

    /// Token actual sin consumir.
    fn peek(&self) -> &Token {
        &self.tokens[self.pos].token
    }

    /// Token en `pos + n` sin consumir. Útil para lookahead corto.
    /// Devuelve `&Token::EOF` si nos pasamos del final.
    fn peek_at(&self, n: usize) -> &Token {
        self.tokens
            .get(self.pos + n)
            .map(|t| &t.token)
            .unwrap_or(&Token::EOF)
    }

    /// `(line, column)` del token actual. Útil para construir errores.
    fn current_pos(&self) -> (usize, usize) {
        let t = &self.tokens[self.pos];
        (t.line, t.column)
    }

    /// `Span` del token actual. Atajo para construir nodos `Expr` con
    /// su posición. Equivale a
    /// `let (l, c) = self.current_pos(); Span::new(l, c)`.
    fn cur_span(&self) -> Span {
        let (line, column) = self.current_pos();
        Span::new(line, column)
    }

    /// `true` si estamos parados en el token EOF.
    fn is_at_end(&self) -> bool {
        matches!(self.peek(), Token::EOF)
    }

    /// Consume el token actual y lo devuelve clonado. El cursor avanza,
    /// salvo que ya estemos en EOF (no avanzamos pasado el final, así
    /// `peek` siempre es válido).
    fn advance(&mut self) -> TokenWithPos {
        let tok = self.tokens[self.pos].clone();
        if !self.is_at_end() {
            self.pos += 1;
        }
        tok
    }

    // ---------- comparación / consumo ----------

    /// `true` si el token actual coincide con `want`. Usa la
    /// implementación de `PartialEq` de `Token`, que compara variante
    /// Y payload — sirve para tokens sin payload (`Plus`, `RParen`,
    /// ...). Para `Ident(_)` u otros con payload, usar `matches!`
    /// directamente sobre `peek()`.
    fn check(&self, want: &Token) -> bool {
        self.peek() == want
    }

    /// Consume el token si coincide con `want`. Devuelve `true` si
    /// hubo match. Útil para tokens opcionales (ej. coma trailing).
    fn eat(&mut self, want: &Token) -> bool {
        if self.check(want) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Consume el token si coincide con `want`, o devuelve un error
    /// con el mensaje dado y la posición del token actual.
    fn expect(&mut self, want: &Token, message: impl Into<String>) -> FitzResult<()> {
        if self.eat(want) {
            Ok(())
        } else {
            Err(self.error(ErrorKind::UnexpectedToken, message))
        }
    }

    /// Si el token actual es un `Ident`, lo consume y devuelve el
    /// nombre. Si no, devuelve un error con el mensaje dado.
    fn expect_ident(&mut self, message: impl Into<String>) -> FitzResult<String> {
        // El `match` borrow termina al ejecutarse `name.clone()`, así
        // que `self.advance()` (que requiere `&mut self`) puede
        // ejecutarse después sin pelear con el borrow checker.
        let name = match self.peek() {
            Token::Ident(name) => name.clone(),
            _ => return Err(self.error(ErrorKind::UnexpectedToken, message)),
        };
        self.advance();
        Ok(name)
    }

    /// Consume runs de `Newline`. Usar antes de cada elemento dentro
    /// de listas (args, fields, arms) y antes de cada sentencia dentro
    /// de un bloque. Entre tokens de una expresión, los newlines
    /// importan (terminan la sentencia) — ahí NO se llama.
    fn skip_newlines(&mut self) {
        while matches!(self.peek(), Token::Newline) {
            self.advance();
        }
    }

    // ---------- construcción de errores ----------

    /// Construye un `FitzError` con la posición del token actual.
    /// Centralizar acá nos da consistencia en todos los errores.
    fn error(&self, kind: ErrorKind, message: impl Into<String>) -> FitzError {
        let (line, column) = self.current_pos();
        FitzError::new(kind, line, column, message)
    }

    // ---------- expresiones: escalera de precedencia ----------
    //
    // De menor a mayor precedencia:
    //   expression  → logic_or
    //   logic_or    → logic_and ( "or" logic_and )*
    //   logic_and   → equality  ( "and" equality )*
    //   equality    → comparison ( ("==" | "!=") comparison )*
    //   comparison  → range      ( ("<" | ">" | "<=" | ">=") range )*
    //   range       → term       ( ".." term )?     (no chainable)
    //   term        → factor     ( ("+" | "-") factor )*
    //   factor      → unary      ( ("*" | "/") unary )*
    //   unary       → "-" unary  |  postfix
    //   postfix     → primary    ( "." Ident  |  "(" args ")"  |  "[" expr "]" )*
    //   primary     → literal | Ident | "(" expression ")" | list | map
    //
    // Los binarios chainables son izquierda-asociativos: el `while` itera
    // y va anidando al `left` cada vez. `range` NO es chainable — `1..2..3`
    // es error (lo agarra el caller si peek_at sigue siendo `..`). `expression`
    // es el punto de entrada desde cualquier regla externa que quiera parsear
    // una expresión completa.

    fn expression(&mut self) -> FitzResult<Expr> {
        self.logic_or()
    }

    /// Igual que `expression()`, pero con el flag `no_struct_literal`
    /// activo: `Ident { ... }` NO se intentará parsear como struct
    /// literal dentro de esta expresión. Se usa en posiciones donde
    /// el `{` que sigue es la apertura de un bloque controlado
    /// (condición de `if`/`while`, iterable de `for`, scrutinee de
    /// `match`).
    ///
    /// Subexpresiones delimitadas dentro de la llamada (paréntesis,
    /// args, indexing, cuerpos de literales) restauran el flag a
    /// `false` localmente — así se permite `if x == (User { id: 1 })`
    /// sin pelearse con el flag.
    fn expression_no_struct_lit(&mut self) -> FitzResult<Expr> {
        let prev = std::mem::replace(&mut self.no_struct_literal, true);
        let result = self.expression();
        self.no_struct_literal = prev;
        result
    }

    /// Heurística para distinguir `Ident { ... }` como struct literal
    /// vs. como Ident seguido de un bloque controlado. Solo se usa
    /// para emitir un error con hint cuando estamos en modo
    /// `no_struct_literal` y el cuerpo tiene pinta inequívoca de
    /// struct literal.
    ///
    /// Pre: `peek()` es `Token::LBrace`. Mira hacia adelante saltando
    /// newlines y retorna `true` si el cuerpo arranca con `Ident :` —
    /// patrón de campo de struct literal que no podría ser, en
    /// condiciones normales, el principio de un bloque (`x: Int = 1`
    /// sí podría, pero hace falta `Ident` después del `:`).
    fn looks_like_struct_lit_body(&self) -> bool {
        if !matches!(self.peek(), Token::LBrace) {
            return false;
        }
        // Saltar newlines después del `{`.
        let mut i = 1;
        while matches!(self.peek_at(i), Token::Newline) {
            i += 1;
        }
        // Cuerpo vacío `{ }` → tratamos como struct literal (en un
        // bloque controlado, `{}` vacío en posición de expresión no
        // tiene sentido, así que el hint sigue siendo útil).
        if matches!(self.peek_at(i), Token::RBrace) {
            return true;
        }
        // Tiene que arrancar con `Ident` seguido de `:`. Si después
        // del `:` hay `Ident =` esto es una asignación tipada de
        // bloque, no un struct literal — esa distinción la dejamos
        // pasar (preferimos un error claro en el caller para ese caso
        // raro).
        let p1 = self.peek_at(i);
        let p2 = self.peek_at(i + 1);
        if !matches!(p1, Token::Ident(_)) || !matches!(p2, Token::Colon) {
            return false;
        }
        // Si tras `Ident :` viene `Ident =`, parece asignación tipada
        // dentro de un bloque (`{ x: Int = 1 }`). En ese caso no es
        // un struct literal y no metemos el hint.
        let after_colon = self.peek_at(i + 2);
        let after_after = self.peek_at(i + 3);
        if matches!(after_colon, Token::Ident(_)) && matches!(after_after, Token::Eq) {
            return false;
        }
        true
    }

    /// `a or b or c` — `or` y `xor` son izquierda-asociativos y
    /// comparten precedencia (más baja que `and`, paralelo a Python
    /// para `or`). Esto da `a and b or c` = `(a and b) or c` y
    /// `a or b xor c` = `(a or b) xor c` (left-fold).
    ///
    /// Mini-tanda Xor: `xor` se sumó al mismo nivel para que
    /// `a xor b xor c` chain natural sin paréntesis.
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

    /// `a and b and c` — más alto que `or`, más bajo que `==`. Resultado:
    /// `a == 1 and b == 2` se parsea como `(a == 1) and (b == 2)`.
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

    /// Mini-tanda Bits — `|` OR bit-a-bit. Precedencia más baja entre
    /// los bitwise (paralelo a Python/C): `|` < `^` < `&` < `<<`/`>>`.
    ///
    /// Cuidado: `|` también se usa como separador de or-patterns en
    /// match arms (R.2.1), pero el parser de match no llega acá — los
    /// patterns se parsean con `parse_or_pattern`.
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

    /// Mini-tanda Bits — `^` XOR bit-a-bit.
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

    /// Mini-tanda Bits — `&` AND bit-a-bit.
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

    /// Mini-tanda Bits — `<<` y `>>`. Precedencia entre bitwise y rango.
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

    /// `start..end` (exclusivo) o `start..=end` (inclusivo, R.1.4).
    /// `span` apunta al `..` o `..=`.
    fn range_expr(&mut self) -> FitzResult<Expr> {
        let start = self.term()?;
        // I.2 — en bracket context, NO consumimos `..`/`..=`: el
        // postfix `[` lo mira para armar el `Expr::Slice`.
        if self.in_slice_context {
            return Ok(start);
        }
        let inclusive = match self.peek() {
            Token::DotDot => false,
            Token::DotDotEq => true,
            _ => return Ok(start),
        };
        let span = self.cur_span();
        self.advance(); // consume '..' o '..='
        let end = self.term()?;
        if matches!(self.peek(), Token::DotDot | Token::DotDotEq) {
            return Err(self.error(
                ErrorKind::InvalidSyntax,
                "los rangos no se encadenan — usá paréntesis si querés un rango de rangos",
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
            // R.1.2 — `%` tiene la misma precedencia que `*` y `/`.
            Token::Percent => BinOpKind::Mod,
            _ => return None,
        };
        let span = self.cur_span();
        self.advance();
        Some((op, span))
    }

    /// Unary prefijo: `-x` (negación numérica) o `not x` (negación
    /// lógica, R.1.1). `span` apunta al operador. Ambos tienen la
    /// misma precedencia (más alta que comparación, debajo de
    /// postfix), así que `not x == 1` parsea como `not (x == 1)`
    /// si quisiéramos eso — pero la asociatividad real es
    /// `(not x) == 1`. Para evitar la ambigüedad, **`not` tiene
    /// precedencia más alta que `==`/`!=`**: `not x == 1` parsea
    /// como `(not x) == 1`. Para el otro orden, usar paréntesis:
    /// `not (x == 1)`.
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
            // Mini-tanda Bits — `~x` NOT bit-a-bit (unario, solo Int).
            // Misma precedencia que `-` y `not`.
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
    /// Desde Fase 3.4 el callee de una llamada es cualquier expresión
    /// postfix — `Expr::Call.callee` es `Box<Expr>`. Eso destraba method
    /// calls (`xs.map(...)`), invocación de fn anónima al vuelo
    /// (`(fn(x) => x + 1)(2)`), y futuros patrones de orden superior.
    fn postfix(&mut self) -> FitzResult<Expr> {
        let mut expr = self.primary()?;
        loop {
            // PreF8.2: method chain multi-línea. Si peek() es Newline,
            // miramos adelante saltando newlines: si el próximo token
            // significativo es `.`, consumimos los newlines y dejamos
            // que la iteración siguiente del loop matchee el `Dot`.
            // Solo `.` continúa — `(`, `[`, `?` rompen como hoy para
            // no cambiar la semántica de expression statements
            // separados ambiguamente por newlines.
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
                    // Fase 6.1: `.await` postfix. Detectamos antes de
                    // consumir el `.` porque `await` ya es keyword del
                    // lexer (`Token::Await`), no un Ident — el camino
                    // normal de `.field` falla con "se esperaba nombre
                    // de campo" sin esto. Mismo lugar en la cadena que
                    // `.field` y `.method()`, así que `expr.await?`,
                    // `expr.await.field`, `expr.await()` encajan por
                    // continuación natural del loop.
                    if matches!(self.peek_at(1), Token::Await) {
                        self.advance(); // consume '.'
                        self.advance(); // consume 'await'
                        expr = Expr::Await(Box::new(expr), span);
                        continue;
                    }
                    // Mini-tanda T — `t.0`, `t.1`, etc. Tuple field
                    // access. El lexer emite `Int(n)` separado del `.`,
                    // así que detectamos `Dot Int` por lookahead.
                    if let Token::Int(n) = self.peek_at(1).clone() {
                        if n < 0 {
                            return Err(self.error(
                                ErrorKind::InvalidSyntax,
                                "índice de tupla debe ser no-negativo",
                            ));
                        }
                        self.advance(); // consume '.'
                        self.advance(); // consume el Int
                        expr = Expr::TupleField {
                            tuple: Box::new(expr),
                            index: n as usize,
                            span,
                        };
                        continue;
                    }
                    self.advance();
                    let field = self.expect_ident("se esperaba nombre de campo después de '.'")?;
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
                                    "`{}` espera exactamente 1 argumento, recibió {}",
                                    name,
                                    args.len()
                                ),
                            ));
                        }
                        let inner = args.into_iter().next().unwrap();
                        // El span del Ok/Err se hereda del Ident receptor.
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
                    // I.2 — entrar a slice context para que range_expr
                    // NO consuma `..`/`..=`. Lo manejamos manual.
                    let prev_slice = std::mem::replace(&mut self.in_slice_context, true);

                    // Caso A: `[..end]` o `[..=end]` o `[..]` — slice
                    // sin start.
                    let bracket_result: FitzResult<Expr> = match self.peek().clone() {
                        Token::DotDot | Token::DotDotEq => {
                            let inclusive = matches!(self.peek(), Token::DotDotEq);
                            self.advance(); // consume `..` o `..=`
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
                            // Caso B: parsear primer expr (con
                            // in_slice_context=true, no consume `..`).
                            let first = self.expression()?;
                            // Caso B.1: index simple.
                            if matches!(self.peek(), Token::RBracket) {
                                Ok(Expr::Index {
                                    object: Box::new(expr.clone()),
                                    index: Box::new(first),
                                    span,
                                })
                            } else if matches!(self.peek(), Token::DotDot | Token::DotDotEq) {
                                // Caso B.2: slice con start. End
                                // opcional.
                                let inclusive = matches!(self.peek(), Token::DotDotEq);
                                self.advance(); // consume `..` o `..=`
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
                                    "se esperaba ']', '..' o '..=' en el contenido del indexing",
                                ))
                            }
                        }
                    };
                    self.no_struct_literal = prev_no_struct;
                    self.in_slice_context = prev_slice;
                    expr = bracket_result?;
                    self.expect(&Token::RBracket, "se esperaba ']' para cerrar el indexing")?;
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
                                "los struct literals no se permiten \
                                 directamente en condiciones de \
                                 if/while/for/match — envolvélo en \
                                 paréntesis: `(User { id: 1 })`",
                            ));
                        }
                        break;
                    }

                    // Reusamos el span del Ident (nombre del tipo).
                    expr = self.parse_struct_lit_body(name, ident_span)?;
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    /// Cuerpo de un struct literal: `{ campo: expr, campo: expr, ... }`.
    /// El receptor (nombre del tipo) ya está consumido y se pasa como
    /// `type_name`. Acepta:
    ///   - Vacío: `{}`.
    ///   - Trailing comma.
    ///   - Newlines entre campos (literal multilínea).
    ///   - Coma o newline como separador entre campos.
    ///
    /// Dentro de los valores el flag `no_struct_literal` se restaura a
    /// `false` (cada valor está delimitado por `,` o `}`), así
    /// permitimos nidos: `Order { user: User { id: 1, name: "x" } }`.
    fn parse_struct_lit_body(&mut self, type_name: String, span: Span) -> FitzResult<Expr> {
        self.expect(&Token::LBrace, "se esperaba '{'")?;
        let prev = std::mem::replace(&mut self.no_struct_literal, false);
        let result = self.parse_struct_lit_fields(type_name, span);
        self.no_struct_literal = prev;
        result
    }

    fn parse_struct_lit_fields(&mut self, type_name: String, span: Span) -> FitzResult<Expr> {
        let mut fields: Vec<(String, Expr)> = Vec::new();
        self.skip_newlines();
        // Vacío: `Empty {}`.
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
                    "se esperaba '}' para cerrar el struct literal",
                ));
            }
            let field_name = self.expect_ident("se esperaba nombre de campo en struct literal")?;
            self.expect(
                &Token::Colon,
                "se esperaba ':' después del nombre del campo en struct literal",
            )?;
            self.skip_newlines();
            let value = self.expression()?;
            fields.push((field_name, value));
            // Separadores aceptados: coma o newline. RBrace cierra el
            // literal en la próxima iter del loop. Otra cosa → error.
            match self.peek() {
                Token::Comma => {
                    self.advance();
                }
                Token::Newline | Token::RBrace => {
                    // skip_newlines en la próxima iter consume el newline.
                }
                _ => {
                    return Err(self.error(
                        ErrorKind::UnexpectedToken,
                        "se esperaba ',', salto de línea o '}' entre campos del struct literal",
                    ));
                }
            }
        }
    }

    // ---------- sentencias ----------
    //
    // Un programa es una lista de sentencias separadas por `Newline`
    // (o `EOF` al final). Las llaves de un bloque (`{ ... }`) también
    // hacen de terminador implícito: un bloque puede terminar sin
    // newline antes del `}`. Esa lógica vive en `consume_stmt_terminator`.
    //
    // Dispatch de `parse_stmt`:
    //   Let                    → asignación con `let`
    //   Return                 → return
    //   Break / Continue       → sentencia simple
    //   Ident + (Eq|Colon)     → asignación sin `let`  (lookahead a peek_at(1))
    //   cualquier otra cosa    → expression-statement

    /// Punto de entrada para parsear un programa completo (top-level).
    /// Consume todo hasta `EOF`.
    ///
    /// Si `recovery_mode` está activo (modo `parse_with_recovery`), un
    /// error de `parse_stmt` no se propaga: se acumula en
    /// `recovered_errors`, se sincroniza hasta el próximo stmt-boundary
    /// y se inserta un `Stmt::Error(span)` en el lugar. El loop sigue
    /// hasta EOF o hasta alcanzar `MAX_RECOVERED_ERRORS`.
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

    /// Push de un error recuperado, con respeto a la cota. Si ya
    /// llegamos al máximo, el error se descarta silenciosamente — el
    /// caller verá en el `Vec` final que estamos en el límite.
    fn push_recovered(&mut self, e: FitzError) {
        if self.recovered_errors.len() < MAX_RECOVERED_ERRORS {
            self.recovered_errors.push(e);
        }
    }

    /// Avanza el cursor hasta un sync point stmt-level. Los sync points
    /// son:
    ///  - `Newline` — terminador natural de stmt en Fitz (se consume).
    ///  - `RBrace` — cierre de bloque (NO se consume; el caller lo
    ///    maneja para cerrar el bloque actual).
    ///  - `EOF` — fin del archivo (NO se consume).
    ///  - Keywords que típicamente arrancan un stmt: `Let`, `Fn`,
    ///    `Async`, `Type`, `Return`, `Break`, `Continue`, `While`,
    ///    `Loop`, `For`, `If`, `Import`, `From`, `At` (decorador). Si
    ///    el cursor está parado en uno, NO se consume — paramos justo
    ///    antes para que el próximo `parse_stmt` lo agarre.
    ///
    /// Por qué parar en keywords: `primary()` consume el token actual
    /// antes de validarlo. Si una expresión se rompe encontrando un
    /// `Newline` u otro token raro, el cursor puede haber avanzado más
    /// allá del newline hasta el `Let` del próximo stmt. Sin la regla
    /// de keywords, `synchronize` se comería el próximo stmt entero
    /// buscando un newline.
    ///
    /// Fitz no tiene `;` como separador — Newline es el único
    /// terminador explícito.
    fn synchronize(&mut self) {
        loop {
            match self.peek() {
                Token::Newline => {
                    self.advance();
                    return;
                }
                Token::RBrace | Token::EOF => return,
                // Keywords que típicamente arrancan un stmt. No
                // consumimos — paramos justo antes para que el próximo
                // `parse_stmt` los procese desde cero.
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

    /// Después de una sentencia, consumimos su terminador. `Newline`
    /// se consume; `EOF` y `RBrace` se dejan sin consumir (el caller
    /// decide qué hacer con ellos).
    fn consume_stmt_terminator(&mut self) -> FitzResult<()> {
        match self.peek() {
            Token::Newline => {
                self.advance();
                Ok(())
            }
            Token::EOF | Token::RBrace => Ok(()),
            _ => Err(self.error(
                ErrorKind::UnexpectedToken,
                "se esperaba salto de línea o fin de bloque entre sentencias",
            )),
        }
    }

    /// Parsea UNA sentencia. El caller maneja terminadores y loops.
    /// Captura el span del primer token y lo pasa a cada sub-parser
    /// vía sus constructores de `Stmt`.
    fn parse_stmt(&mut self) -> FitzResult<Stmt> {
        let (line, column) = self.current_pos();
        let span = Span::new(line, column);
        match self.peek() {
            Token::Let => self.parse_assign_with_let(span),
            Token::Return => self.parse_return(span),
            Token::Fn | Token::Async => self.parse_fndef(span),
            Token::Type => self.parse_typedef(span),
            Token::At => self.parse_decorated_fndef(span),
            Token::Break => {
                self.advance();
                // Mini-tanda L — sintaxis `break ['label] [<expr>]`.
                // Label primero (si está), después value opcional.
                // Rust usa el mismo orden: `break 'outer 42`.
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
            // Mini-tanda L — `'label: <loop>` declara label antes del
            // loop. Soporta loop/while/for. El parser consume el
            // Label + Colon y delega al parse_*_with_label.
            Token::Label(_) => {
                let label = if let Token::Label(l) = self.peek().clone() {
                    l
                } else {
                    unreachable!()
                };
                self.advance();
                self.expect(&Token::Colon, "se esperaba ':' después del label")?;
                match self.peek() {
                    Token::Loop => self.parse_loop_with_label(span, Some(label)),
                    Token::While => self.parse_while_with_label(span, Some(label)),
                    Token::For => self.parse_for_with_label(span, Some(label)),
                    _ => Err(self.error(
                        ErrorKind::UnexpectedToken,
                        "se esperaba `loop`, `while` o `for` después del label",
                    )),
                }
            }
            Token::Import => self.parse_import(span),
            Token::From => self.parse_from_import(span),
            _ => self.parse_expr_or_assign_stmt(span),
        }
    }

    /// `import foo` o `import foo.bar.baz`. El path se acumula como
    /// `Ident ( '.' Ident )*`. PreF8.4: acepta `as <ident>` al final
    /// para alias del namespace (`import foo as f` → binding `f` en
    /// lugar del último segmento).
    fn parse_import(&mut self, span: Span) -> FitzResult<Stmt> {
        self.expect(&Token::Import, "se esperaba 'import'")?;
        let path = self.parse_module_path()?;
        let alias = if matches!(self.peek(), Token::As) {
            self.advance();
            Some(self.expect_ident(
                "se esperaba un identificador después de 'as' en 'import ... as ...'",
            )?)
        } else {
            None
        };
        Ok(Stmt::Import { path, alias, span })
    }

    /// `from foo import a, b, c` — el path puede tener puntos (`from
    /// sub.foo import bar`). La lista de nombres tiene que tener al
    /// menos uno. Acepta trailing comma. PreF8.4: cada nombre puede
    /// llevar `as <ident>` para alias (`from foo import bar as b,
    /// baz as z`).
    fn parse_from_import(&mut self, span: Span) -> FitzResult<Stmt> {
        self.expect(&Token::From, "se esperaba 'from'")?;
        let path = self.parse_module_path()?;
        self.expect(
            &Token::Import,
            "se esperaba 'import' después del path en 'from ... import ...'",
        )?;

        // Mini-tanda Mln — multi-línea con paréntesis. Si después del
        // `import` viene un `(`, entramos a modo multi-línea: newlines
        // entre nombres se toleran (los consumimos), y cerramos con
        // `)`. Sin paréntesis sigue el comportamiento single-line.
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
                // Trailing comma antes del `)` es OK.
                if matches!(self.peek(), Token::RParen) {
                    break;
                }
                names.push(self.parse_from_import_name(/*is_first=*/ false)?);
                self.skip_newlines_inside_parens();
            }
            self.expect(
                &Token::RParen,
                "se esperaba ')' para cerrar `from ... import (...)`",
            )?;
            return Ok(Stmt::FromImport { path, names, span });
        }

        let mut names: Vec<(String, Option<String>)> = Vec::new();
        names.push(self.parse_from_import_name(/*is_first=*/ true)?);
        while matches!(self.peek(), Token::Comma) {
            self.advance();
            // Trailing comma: `from foo import a,` — paramos sin error.
            if matches!(self.peek(), Token::Newline | Token::EOF | Token::RBrace) {
                break;
            }
            names.push(self.parse_from_import_name(/*is_first=*/ false)?);
        }
        Ok(Stmt::FromImport { path, names, span })
    }

    /// Mini-tanda Mln — Helper para `from foo import (...)` multi-línea.
    /// Consume newlines consecutivos hasta encontrar un token de
    /// contenido. Sin checks de profundidad — el caller ya está adentro
    /// del paréntesis.
    fn skip_newlines_inside_parens(&mut self) {
        while matches!(self.peek(), Token::Newline) {
            self.advance();
        }
    }

    /// Helper: parsea un binding de `from ... import`: `Ident [as Ident]`.
    /// `is_first` solo cambia el mensaje de error del primer ident.
    fn parse_from_import_name(&mut self, is_first: bool) -> FitzResult<(String, Option<String>)> {
        let name = self.expect_ident(if is_first {
            "se esperaba al menos un identificador después de 'import'"
        } else {
            "se esperaba identificador después de ',' en 'from ... import'"
        })?;
        let alias = if matches!(self.peek(), Token::As) {
            self.advance();
            Some(self.expect_ident(
                "se esperaba un identificador después de 'as' en 'from ... import ... as ...'",
            )?)
        } else {
            None
        };
        Ok((name, alias))
    }

    /// Path de módulo: `Ident ( '.' Ident )*`. Devuelve los segmentos.
    /// Siempre tiene al menos un elemento. Sirve para `import` y para
    /// `from ... import`.
    fn parse_module_path(&mut self) -> FitzResult<Vec<String>> {
        let first = self.expect_ident("se esperaba nombre de módulo (identificador)")?;
        let mut segments = vec![first];
        while matches!(self.peek(), Token::Dot) {
            self.advance();
            let next = self.expect_ident("se esperaba nombre de módulo después de '.'")?;
            segments.push(next);
        }
        Ok(segments)
    }

    fn parse_assign_with_let(&mut self, span: Span) -> FitzResult<Stmt> {
        self.expect(&Token::Let, "se esperaba 'let'")?;
        // Mini-tanda T — destructuring `let (a, b) = expr`. Detectamos
        // por peek `(`. El pattern admite nesting: `let ((x, y), z) =
        // ...`. Sin type annotation por simplicidad MVP (el checker
        // infiere desde el RHS).
        if matches!(self.peek(), Token::LParen) {
            let pattern = self.parse_pattern()?;
            self.expect(
                &Token::Eq,
                "se esperaba '=' en la declaración con destructuring",
            )?;
            let value = self.expression()?;
            return Ok(Stmt::Destructure {
                pattern,
                value,
                span,
            });
        }
        let name = self.expect_ident("se esperaba nombre de variable después de 'let'")?;
        let type_ = self.parse_optional_type_annotation()?;
        self.expect(&Token::Eq, "se esperaba '=' en la declaración")?;
        let value = self.expression()?;
        Ok(Stmt::Assign {
            target: AssignTarget::Ident(name),
            type_,
            value,
            span,
        })
    }

    /// Parsea una sentencia que arranca con una expresión. Tres casos:
    ///   1. `expr` — sentencia-expresión (típicamente una llamada).
    ///   2. `Ident: Tipo = expr` — declaración/reasignación con anotación.
    ///   3. `lvalue = expr` — asignación. El lvalue puede ser `Ident`
    ///      (variable) o `Expr::Field` (mutación de campo de instancia).
    ///      Cualquier otra forma (`f() = ...`, `xs[0] = ...`) es error.
    ///
    /// Unifica el camino antes separado entre `parse_assign_no_let` y
    /// `parse_expr_stmt`: parseamos la expresión completa primero, y
    /// recién después decidimos si era asignación, según el token que
    /// haya quedado. Eso resuelve naturalmente `user.name = "x"` y
    /// elimina el lookahead duro que antes solo miraba `peek_at(1)`.
    fn parse_expr_or_assign_stmt(&mut self, span: Span) -> FitzResult<Stmt> {
        let lhs = self.expression()?;

        // Caso 2: `Ident : Tipo = expr`. La anotación solo se acepta
        // sobre un identificador pelado.
        if matches!(self.peek(), Token::Colon) {
            let name = match lhs {
                Expr::Ident(n, _) => n,
                _ => {
                    return Err(self.error(
                        ErrorKind::InvalidSyntax,
                        "anotación de tipo solo se admite al declarar una variable",
                    ));
                }
            };
            self.advance(); // consume ':'
            let type_ = self.parse_type_expr()?;
            self.expect(&Token::Eq, "se esperaba '=' en la asignación")?;
            let value = self.expression()?;
            return Ok(Stmt::Assign {
                target: AssignTarget::Ident(name),
                type_: Some(type_),
                value,
                span,
            });
        }

        // Caso 3: `lvalue = expr`.
        if self.eat(&Token::Eq) {
            let value = self.expression()?;
            let target = match lhs {
                Expr::Ident(n, _) => AssignTarget::Ident(n),
                Expr::Field { object, field, .. } => AssignTarget::Field { object, field },
                // R.1.3 — `xs[i] = v` y `m["k"] = v` (mini-fase R).
                // El parser ya construyó `Expr::Index { object, index }`
                // como parte del postfix; lo "destruimos" acá para
                // armar el `AssignTarget::Index`.
                Expr::Index { object, index, .. } => AssignTarget::Index { object, index },
                _ => {
                    return Err(self.error(
                        ErrorKind::InvalidSyntax,
                        "destino de asignación no soportado (solo identificador, \
                         `expr.campo` o `expr[indice]`)",
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

        // Caso 3b — R.2.3: operadores compuestos `+=`/`-=`/`*=`/`/=`.
        // Desugar a `target = target <op> rhs` en el parser. Esto deja
        // el resto del pipeline (checker, evaluator, codegen) sin tocar
        // — trabajan con `Stmt::Assign` regulares. El target se evalúa
        // DOS veces: una como Expr (RHS del BinOp) y otra como
        // AssignTarget (destino). El evaluator de índice usa el
        // patrón "compute first, lock last" (R.1.3) así que la doble
        // evaluación del index también va segura.
        let compound_op = match self.peek() {
            Token::PlusEq => Some(BinOpKind::Add),
            Token::MinusEq => Some(BinOpKind::Sub),
            Token::StarEq => Some(BinOpKind::Mul),
            Token::SlashEq => Some(BinOpKind::Div),
            // Mini-tanda Cmp — ops bit-a-bit compuestos.
            Token::AmpEq => Some(BinOpKind::BitAnd),
            Token::PipeEq => Some(BinOpKind::BitOr),
            Token::CaretEq => Some(BinOpKind::BitXor),
            Token::ShlEq => Some(BinOpKind::Shl),
            Token::ShrEq => Some(BinOpKind::Shr),
            _ => None,
        };
        if let Some(op) = compound_op {
            let op_span = self.cur_span();
            self.advance(); // consume el token `+=`/etc.
            let rhs = self.expression()?;
            let (target, target_as_expr) = match lhs {
                Expr::Ident(n, ispan) => (AssignTarget::Ident(n.clone()), Expr::Ident(n, ispan)),
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
                        "destino de asignación compuesta no soportado (solo identificador, \
                         `expr.campo` o `expr[indice]`)",
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

        // Caso 1: sentencia-expresión.
        Ok(Stmt::Expr(lhs, span))
    }

    /// Anotación de tipo opcional: `: TypeExpr`. Devuelve `Some(t)` si
    /// la había. Acepta `Int`, `Str`, `List<Int>`, `Map<Str, User>`,
    /// `Result<List<User>>`, `User?`, `Map<Str, Int>?`, etc.
    fn parse_optional_type_annotation(&mut self) -> FitzResult<Option<TypeExpr>> {
        if self.eat(&Token::Colon) {
            Ok(Some(self.parse_type_expr()?))
        } else {
            Ok(None)
        }
    }

    /// Parsea una `TypeExpr` (obligatoria) en posición de anotación.
    ///
    /// Gramática:
    ///
    /// ```text
    /// type_expr := fn_type | atom ( '?' )?
    /// fn_type   := 'Fn' '(' ( type_expr ( ',' type_expr )* )? ')' '->' type_expr
    /// atom      := Ident generic_args?
    /// generic_args := '<' type_expr ( ',' type_expr )* '>'
    /// ```
    ///
    /// Sufijo `?` se asocia al átomo entero: `List<Int>?` → `Nullable(List<Int>)`.
    /// Aceptamos `?` una sola vez por ahora; `T??` se podría modelar más
    /// adelante (`Nullable(Nullable(T))`), pero hoy `eat` solo consume uno
    /// y un segundo `?` se quedaría sin consumir, sin error explícito.
    /// El checker estático puede normalizarlo cuando llegue.
    ///
    /// `Fn` es keyword contextual sintáctica del tipo función. Cuando
    /// se ve `Fn` seguido de `(`, parseamos como `TypeExpr::Function`.
    /// Si el siguiente token no es `(`, `Fn` se trata como nombre
    /// nominal normal — fallará en resolución por no existir como
    /// tipo en el env.
    ///
    /// Nota sobre lexing: el lexer emite `>` siempre como `Token::Gt`
    /// (no hay `>>` como un solo token), así que `Result<List<Int>>` se
    /// cierra consumiendo dos `Token::Gt` separados — uno por nivel de
    /// genérico.
    fn parse_type_expr(&mut self) -> FitzResult<TypeExpr> {
        // Mini-tanda T — tipo tupla `(T1, T2, ...)`. `()` es la
        // tupla vacía, `(T,)` una tupla de un elemento (trailing
        // comma obligatoria), `(T)` solo paréntesis (sin tupla,
        // delega al tipo interno).
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
                self.expect(&Token::RParen, "se esperaba ')' para cerrar el tipo tupla")?;
                let mut t = TypeExpr::Tuple(items);
                if self.eat(&Token::Question) {
                    t = TypeExpr::Nullable(Box::new(t));
                }
                return Ok(t);
            }
            // Sin coma → solo paréntesis de agrupación.
            self.expect(
                &Token::RParen,
                "se esperaba ')' para cerrar el tipo entre paréntesis",
            )?;
            let mut t = first;
            if self.eat(&Token::Question) {
                t = TypeExpr::Nullable(Box::new(t));
            }
            return Ok(t);
        }
        let name = self.expect_ident("se esperaba un nombre de tipo")?;
        // Keyword contextual: `Fn(...)` → tipo función.
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
                        "genérico `{}<>` vacío: se esperaba al menos un argumento de tipo",
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
            // Mini-tanda Bits: el lexer ahora produce `Token::Shr` para
            // `>>`, lo que rompe `List<List<Int>>` y similares. Acá
            // splitteamos un `Shr` en dos `Gt` consumiendo solo uno —
            // el segundo `>` queda como `Gt` para el caller de
            // afuera.
            match self.peek() {
                Token::Gt => {
                    self.advance();
                }
                Token::Shr => {
                    // Mutamos el token actual a Gt en su lugar y
                    // shifteamos la columna para apuntar al segundo `>`.
                    self.tokens[self.pos].token = Token::Gt;
                    self.tokens[self.pos].column += 1;
                }
                _ => {
                    return Err(self.error(
                        ErrorKind::UnexpectedToken,
                        format!("se esperaba '>' para cerrar `{}<...>`", name),
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

    /// `Fn` ya consumido; parsea `(P1, P2, ...) -> R`.
    fn parse_fn_type(&mut self) -> FitzResult<TypeExpr> {
        self.expect(&Token::LParen, "se esperaba '(' después de `Fn`")?;
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
        self.expect(&Token::RParen, "se esperaba ')' para cerrar `Fn(...)`")?;
        self.expect(
            &Token::Arrow,
            "se esperaba '->' con el tipo de retorno después de `Fn(...)`",
        )?;
        let ret = self.parse_type_expr()?;
        Ok(TypeExpr::Function {
            params,
            ret: Box::new(ret),
        })
    }

    fn parse_return(&mut self, span: Span) -> FitzResult<Stmt> {
        self.expect(&Token::Return, "se esperaba 'return'")?;
        // `return` sin valor devuelve null implícito. Detectamos los
        // terminadores válidos para una sentencia: fin de línea, cierre
        // de bloque o fin de archivo.
        match self.peek() {
            Token::Newline | Token::RBrace | Token::EOF => {
                return Ok(Stmt::Return(Expr::Null(span), span));
            }
            _ => {}
        }

        // Mini-tanda OAPI — lookahead específico para detectar el
        // patrón ReturnStatus ANTES de invocar `expression()` (que
        // greedea `Ident { ... }` como struct literal). El patrón es:
        //   `<Int> { ... }` o `<Ident> { ... }` donde el `{...}` es un
        //   map literal (primera clave Str), no un struct lit (primera
        //   clave Ident).
        //
        // Disambiguación robusta:
        //   - tok0: Int o Ident
        //   - tok1: LBrace
        //   - primer token no-newline después de LBrace: Str (map lit)
        //     o RBrace (map vacío)
        //
        // Si la primera key es Ident → struct lit (`return P { x: 1 }`),
        // NO es ReturnStatus.
        // Si es Str → map lit (`return NOT_FOUND { "error": "..." }`),
        // SÍ es ReturnStatus.
        let looks_like_return_status = {
            let t0 = self.peek_at(0);
            let t1 = self.peek_at(1);
            let head_ok = matches!(t0, Token::Int(_) | Token::Ident(_));
            let brace_next = matches!(t1, Token::LBrace);
            if head_ok && brace_next {
                // Skip newlines después del LBrace (bound: 16 para
                // evitar walks largos sobre archivos patológicos).
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
            // Parsear el status como un atom — Int o Ident solo,
            // SIN postfix (sin call, sin field, sin struct lit).
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
                _ => unreachable!("lookahead garantiza Int o Ident"),
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

    // ---------- definición de función ----------
    //
    // Cuatro formas (combinables con `async` opcional):
    //   fn name(params) { body }
    //   fn name(params) -> Type { body }
    //   fn name(params) => expr
    //   fn name(params) -> Type => expr
    //
    // La forma de flecha se desugar a `body: vec![Stmt::Return(expr, Span::ZERO)]`
    // (decisión documentada en ast.rs).

    fn parse_fndef(&mut self, span: Span) -> FitzResult<Stmt> {
        let is_async = self.eat(&Token::Async);
        self.expect(&Token::Fn, "se esperaba 'fn'")?;
        let name = self.expect_ident("se esperaba nombre de función después de 'fn'")?;
        self.expect(
            &Token::LParen,
            "se esperaba '(' después del nombre de función",
        )?;
        let params = self.parse_params()?;
        let return_type = self.parse_optional_return_type()?;

        // Cuerpo: bloque `{ ... }` o flecha `=> expr`.
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
                    "se esperaba '{' o '=>' para el cuerpo de la función",
                ));
            }
        };

        Ok(Stmt::FnDef {
            name,
            params,
            return_type,
            body,
            is_async,
            // El parser de fn "pelada" no conoce decorators. Cuando se
            // entra por `parse_decorated_fndef`, ese path reconstruye el
            // FnDef pegándole los decorators acumulados.
            decorators: vec![],
            span,
        })
    }

    /// Función anónima en posición de expresión: `fn(x) => x * 2` o
    /// `fn(x) { return x * 2 }`. Diferencias con `parse_fndef`: no hay
    /// nombre y no se admite `async` (no tendría dónde aplicarse hasta
    /// Fase 4). El cuerpo y el tipo de retorno se parsean igual.
    fn parse_fn_expr(&mut self) -> FitzResult<Expr> {
        let span = self.cur_span();
        // Mini-tanda Async-cl — `async fn(...)` es un closure async.
        // El body puede usar `.await` y la fn devuelve un `Future<T>`.
        let is_async = if matches!(self.peek(), Token::Async) {
            self.advance();
            true
        } else {
            false
        };
        self.expect(&Token::Fn, "se esperaba 'fn'")?;
        self.expect(
            &Token::LParen,
            "se esperaba '(' después de 'fn' en función anónima",
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
                    "se esperaba '{' o '=>' para el cuerpo de la función anónima",
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

    /// Lista de parámetros, ya con '(' consumido. Termina consumiendo
    /// el ')'. Cada parámetro es `name`, `name: Type`, `name = default`,
    /// `name: Type = default` (mini-tanda Fp — default params), o
    /// `...name: Type` (mini-tanda Fp.2 — varargs). Acepta trailing
    /// comma y newlines dentro de los paréntesis.
    ///
    /// **Regla Python para defaults**: una vez que un param tiene default,
    /// todos los siguientes también deben tener default.
    ///
    /// **Regla varargs (Fp.2)**: solo el ÚLTIMO param puede ser varargs.
    /// Un varargs NO puede tener default. El binding adentro del body
    /// tipa como `List<T>` (o `List<Any>` si no se anotó).
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
            // Fp.2 — `...name` indica varargs. Detectamos `..` + `.`
            // (Token::DotDot seguido de Token::Dot). Fitz tiene `..` para
            // Range y `..=` para Range inclusivo; tres `.` consecutivos
            // no colisionan con nada (el lexer empareja greedy).
            let varargs =
                if matches!(self.peek(), Token::DotDot) && matches!(self.peek_at(1), Token::Dot) {
                    if saw_varargs {
                        return Err(self.error(
                            ErrorKind::UnexpectedToken,
                            "solo puede haber un parámetro variádico, y debe ser el último",
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
                            "después de un parámetro variádico no puede haber más parámetros",
                        ));
                    }
                    false
                };
            let name = self.expect_ident("se esperaba nombre de parámetro")?;
            let type_ = self.parse_optional_type_annotation()?;
            // Fp — default value `= <expr>`. Varargs no admite default.
            let default = if matches!(self.peek(), Token::Eq) {
                if varargs {
                    return Err(self.error(
                        ErrorKind::UnexpectedToken,
                        format!("el parámetro variádico `{}` no puede tener default", name),
                    ));
                }
                self.advance(); // consume `=`
                let expr = self.expression()?;
                saw_default = true;
                Some(expr)
            } else {
                // Default + varargs son mutex: un varargs absorbe 0+ args,
                // así que NO triggerea la regla de "todos los siguientes
                // necesitan default" (no hay siguientes y absorbe el rol
                // de "args opcionales adicionales").
                if saw_default && !varargs {
                    return Err(self.error(
                        ErrorKind::UnexpectedToken,
                        format!(
                            "el parámetro `{}` no tiene default pero uno anterior sí — \
                             en Fitz, una vez que un param tiene default, todos los siguientes también",
                            name
                        ),
                    ));
                }
                None
            };
            params.push(Param {
                name,
                type_,
                default,
                varargs,
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
        self.expect(
            &Token::RParen,
            "se esperaba ')' para cerrar la lista de parámetros",
        )?;
        Ok(params)
    }

    /// `-> TypeExpr` opcional. Comparte la gramática de tipos con
    /// `parse_optional_type_annotation` — acepta genéricos y nullables.
    fn parse_optional_return_type(&mut self) -> FitzResult<Option<TypeExpr>> {
        if self.eat(&Token::Arrow) {
            Ok(Some(self.parse_type_expr()?))
        } else {
            Ok(None)
        }
    }

    /// Bloque `{ stmt; stmt; ... }`. Consume llaves de apertura y cierre.
    /// Acepta líneas en blanco entre sentencias y bloques vacíos.
    ///
    /// Recovery (9.0.1, F15): si `recovery_mode` está activo, errores
    /// de `parse_stmt` adentro del bloque se capturan paralelamente al
    /// loop top-level — `Stmt::Error(span)` en lugar del stmt fallido,
    /// `synchronize()` hasta `Newline`/`RBrace`/`EOF`, y se sigue. Si
    /// el `{` de apertura nunca apareció o el `}` de cierre falta, el
    /// error sí se propaga: arreglar la estructura de un bloque es muy
    /// costoso adentro de recovery; preferimos abortar el bloque entero
    /// y dejar que el loop padre se reacomode en el próximo sync point.
    fn parse_block(&mut self) -> FitzResult<Vec<Stmt>> {
        self.expect(&Token::LBrace, "se esperaba '{'")?;
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
                    "se esperaba '}' para cerrar el bloque",
                ));
            }
            if self.recovery_mode && self.recovered_errors.len() >= MAX_RECOVERED_ERRORS {
                // Saltamos al cierre del bloque (si hay) para no dejar
                // un `{` colgado en el AST padre.
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

    /// `while cond { body }`. Iteración condicional. La condición se evalúa
    /// antes de cada iteración; si es `false`, termina el loop.
    fn parse_while(&mut self, span: Span) -> FitzResult<Stmt> {
        self.parse_while_with_label(span, None)
    }

    fn parse_while_with_label(&mut self, span: Span, label: Option<String>) -> FitzResult<Stmt> {
        self.expect(&Token::While, "se esperaba 'while'")?;
        // La condición no permite struct literal a primer nivel — el `{`
        // siguiente arranca el cuerpo del while. Adentro de paréntesis sí.
        let condition = self.expression_no_struct_lit()?;
        let body = self.parse_block()?;
        Ok(Stmt::While {
            condition,
            body,
            label,
            span,
        })
    }

    /// `loop { body }` — loop infinito. Solo se sale con `break` o `return`.
    fn parse_loop(&mut self, span: Span) -> FitzResult<Stmt> {
        self.parse_loop_with_label(span, None)
    }

    fn parse_loop_with_label(&mut self, span: Span, label: Option<String>) -> FitzResult<Stmt> {
        self.expect(&Token::Loop, "se esperaba 'loop'")?;
        let body = self.parse_block()?;
        Ok(Stmt::Loop { body, label, span })
    }

    /// Mini-tanda L — `loop { body }` como expresión. Versión
    /// idéntica a `parse_loop` pero devuelve `Expr::Loop`.
    /// Usada cuando `loop` aparece como RHS de let, arg de
    /// call, etc. `label` opcional para `'name: loop { ... }`.
    fn parse_loop_expr(&mut self, label: Option<String>) -> FitzResult<Expr> {
        let span = self.cur_span();
        self.expect(&Token::Loop, "se esperaba 'loop'")?;
        let body = self.parse_block()?;
        Ok(Expr::Loop { body, label, span })
    }

    /// `for var in iter { body }`. Iteración sobre listas y rangos
    /// (mapas todavía no, hasta que tengamos el tipo `Pair`).
    /// `var` se define en cada iteración en el scope del body.
    fn parse_for(&mut self, span: Span) -> FitzResult<Stmt> {
        self.parse_for_with_label(span, None)
    }

    fn parse_for_with_label(&mut self, span: Span, label: Option<String>) -> FitzResult<Stmt> {
        self.expect(&Token::For, "se esperaba 'for'")?;
        // Mini-tanda Md: el var del for ahora es un Pattern. Reusa
        // `parse_pattern` (el mismo de los arms de match), que cubre
        // Ident, Wildcard, Tuple — los 3 casos válidos en for. Otros
        // patterns (literales, Ok/Err, Range) el checker los rechaza.
        let var = self.parse_pattern()?;
        self.expect(
            &Token::In,
            "se esperaba 'in' después de la variable de 'for'",
        )?;
        // El iterable no permite struct literal a primer nivel — el `{`
        // siguiente arranca el cuerpo del for. Adentro de paréntesis o
        // listas sí: `for u in [User { id: 1 }]`.
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

    /// `if cond { ... }` o `if cond { ... } else { ... }` o
    /// `if cond { ... } else if ... { ... } else { ... }`.
    /// La cadena `else if` se desugar a un `else` que contiene una
    /// sola sentencia: el `if` siguiente envuelto en `Stmt::Expr`.
    fn parse_if_expr(&mut self) -> FitzResult<Expr> {
        let span = self.cur_span();
        self.expect(&Token::If, "se esperaba 'if'")?;
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

    /// `match value { pat => expr, pat => expr, ... }`.
    /// Brazos separados por coma o newline (ambos aceptados).
    /// Limitaciones de los patrones, según el AST: solo `Ident`,
    /// `_` (wildcard), `Ok(x)`, `Err(e)`. Literales y rangos en
    /// patrones son deuda explícita.
    fn parse_match_expr(&mut self) -> FitzResult<Expr> {
        let span = self.cur_span();
        self.expect(&Token::Match, "se esperaba 'match'")?;
        // El scrutinee no permite struct literal a primer nivel — el `{`
        // siguiente arranca el bloque de arms. Adentro de paréntesis sí.
        let value = self.expression_no_struct_lit()?;
        self.expect(
            &Token::LBrace,
            "se esperaba '{' después de la expresión de match",
        )?;
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
                    "se esperaba '}' para cerrar match",
                ));
            }
            let pattern = self.parse_or_pattern()?;
            // R.2.2 — guard opcional `if <cond>` entre pattern y `=>`.
            let guard = if matches!(self.peek(), Token::If) {
                self.advance(); // consume `if`
                Some(self.expression()?)
            } else {
                None
            };
            self.expect(&Token::FatArrow, "se esperaba '=>' después del patrón")?;
            // Sp.2 — el cuerpo del arm puede ser:
            //   1. `return <expr>` / `break <expr>` / `continue` → Stmt directo.
            //   2. `{ <stmts> }` → bloque de stmts (parse_block).
            //   3. `<expr>` → un Stmt::Expr de una sola entrada (legacy).
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
            // Separador entre brazos: coma o newline. RBrace y EOF se
            // dejan pasar — el siguiente iter del loop los maneja:
            // RBrace termina el match, EOF cae a MissingClosingBrace.
            match self.peek() {
                Token::Comma => {
                    self.advance();
                }
                Token::Newline | Token::RBrace | Token::EOF => {}
                _ => {
                    return Err(self.error(
                        ErrorKind::UnexpectedToken,
                        "se esperaba ',' o salto de línea entre brazos de match",
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

    /// Patrones soportados:
    ///   _           → Wildcard
    ///   nombre      → Ident(nombre)        (captura, matchea todo)
    ///   42 / -3     → Int (con `-` para negativos)
    ///   3.14        → Float
    ///   "texto"     → Str
    ///   true/false  → Bool
    ///   null        → Null
    ///   0..10       → Range (solo Int; extremos pueden ser negativos)
    ///   Ok(name)    → OkBinding(name)      (bloqueado runtime hasta Fase 3)
    ///   Err(name)   → ErrBinding(name)     (bloqueado runtime hasta Fase 3)
    /// Parsea uno o más patterns separados por `|` (or-pattern,
    /// R.2.1). Si solo hay uno, devuelve el pattern simple sin
    /// envolver en `Or`. Si hay 2+, devuelve `Pattern::Or(...)`.
    ///
    /// Restricciones del MVP (paralelas a Rust):
    ///  - **Sin bindings** adentro de or-patterns. `Ident(x)`,
    ///    `OkBinding(name)` y `ErrBinding(name)` se rechazan con
    ///    error claro citando el caveat. Workaround sugerido al
    ///    usuario: usar `Wildcard` / `OkWildcard` / `ErrWildcard`,
    ///    o desdoblar el arm.
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
        // Validar restricciones del MVP: sin bindings en
        // sub-patterns. Mirá el doc comment de `Pattern::Or`.
        for sub in &subs {
            if matches!(
                sub,
                Pattern::Ident(_) | Pattern::OkBinding(_) | Pattern::ErrBinding(_)
            ) {
                return Err(self.error(
                    ErrorKind::InvalidSyntax,
                    "or-patterns no admiten bindings (usá '_' o desdoblá el arm)",
                ));
            }
        }
        Ok(Pattern::Or(subs))
    }

    fn parse_pattern(&mut self) -> FitzResult<Pattern> {
        // Mini-tanda T — `(p1, p2, ...)` tuple pattern. Decisión
        // del parser: si arranca con `(`, asumimos tuple pattern
        // (no hay otro uso de `(` en posición de pattern). `()` →
        // tupla vacía. `(p)` sin coma → en match no tiene sentido
        // (un pattern entre paréntesis equivalente a `p`), pero
        // lo admitimos por consistencia.
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
                self.expect(
                    &Token::RParen,
                    "se esperaba ')' para cerrar el tuple pattern",
                )?;
                return Ok(Pattern::Tuple(subs));
            }
            self.expect(&Token::RParen, "se esperaba ')' para cerrar el pattern")?;
            return Ok(first);
        }
        // Literales. Clonamos el peek antes de avanzar para no chocar con
        // el borrow checker. Los Int caen en `try_int_or_range` para
        // chequear si después viene `..` y promovemos a Range.
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
                // Soporte para literales negativos: `-42`, `-3.14`. Si tras
                // el `-` no viene un número, es error (no aceptamos `-x`
                // como patrón).
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
                            "se esperaba número después de '-' en patrón",
                        ));
                    }
                }
            }
            _ => {}
        }

        // Casos especiales: Ok(...) y Err(...).
        if let Token::Ident(name) = self.peek() {
            if name == "Ok" || name == "Err" {
                let is_ok = name == "Ok";
                self.advance();
                self.expect(
                    &Token::LParen,
                    "se esperaba '(' después de Ok/Err en patrón",
                )?;
                let binding =
                    self.expect_ident("se esperaba identificador para el binding de Ok/Err")?;
                self.expect(&Token::RParen, "se esperaba ')' al final del patrón Ok/Err")?;
                // `_` adentro es wildcard (no bindea): cierra deuda
                // vieja de 3.3 donde `_` se bindeaba como var.
                return Ok(match (is_ok, binding.as_str()) {
                    (true, "_") => Pattern::OkWildcard,
                    (false, "_") => Pattern::ErrWildcard,
                    (true, _) => Pattern::OkBinding(binding),
                    (false, _) => Pattern::ErrBinding(binding),
                });
            }
        }
        // Caso general: identificador o wildcard.
        let name = self.expect_ident("se esperaba patrón")?;
        if name == "_" {
            Ok(Pattern::Wildcard)
        } else {
            Ok(Pattern::Ident(name))
        }
    }

    /// Después de consumir un Int (posiblemente negativo), peek `..`
    /// o `..=`: si está, parsea el segundo extremo y devuelve
    /// `Pattern::Range`; si no, devuelve `Pattern::Int(start)` sin
    /// más. El extremo derecho admite `-Int` también. R.1.4 sumó
    /// soporte de `..=` (rango inclusivo).
    fn try_int_or_range(&mut self, start: i64) -> FitzResult<Pattern> {
        let inclusive = match self.peek() {
            Token::DotDot => false,
            Token::DotDotEq => true,
            _ => return Ok(Pattern::Int(start)),
        };
        self.advance(); // consume '..' o '..='
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
                            "se esperaba Int después de '-' en patrón de rango",
                        ));
                    }
                }
            }
            _ => {
                return Err(self.error(
                    ErrorKind::InvalidSyntax,
                    "patrón de rango requiere Int en ambos extremos (Float y otros tipos no soportados)",
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
    /// Separador entre items: coma o newline (ambos aceptados).
    /// Items pueden ser **fields** (`name: TypeExpr [= default]`) o
    /// **métodos** (`[async] fn nombre(params) [-> Ret] { body }` —
    /// R.3, mini-fase R). Lookahead trivial: `fn` o `async` →
    /// método; cualquier otro Ident → field.
    /// El tipo del campo usa la misma gramática que el resto de las
    /// anotaciones (`parse_type_expr`): admite genéricos y el sufijo
    /// `?` para nullable. La nullabilidad queda dentro de `TypeExpr`
    /// como `TypeExpr::Nullable(...)`.
    fn parse_typedef(&mut self, span: Span) -> FitzResult<Stmt> {
        self.expect(&Token::Type, "se esperaba 'type'")?;
        let name = self.expect_ident("se esperaba nombre del tipo")?;
        self.expect(
            &Token::LBrace,
            "se esperaba '{' después del nombre del tipo",
        )?;
        let mut fields: Vec<Field> = Vec::new();
        let mut methods: Vec<MethodDef> = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), Token::RBrace) {
                self.advance();
                return Ok(Stmt::TypeDef {
                    name,
                    fields,
                    methods,
                    span,
                });
            }
            if self.is_at_end() {
                return Err(self.error(
                    ErrorKind::MissingClosingBrace,
                    "se esperaba '}' para cerrar 'type'",
                ));
            }
            // R.3 — método de instancia: `[async] fn nombre(...) [-> T] { ... }`.
            // Mini-tanda St — método estático: `static [async] fn nombre(...)`.
            if matches!(self.peek(), Token::Async | Token::Fn | Token::Static) {
                let method_span = self.cur_span();
                let method = self.parse_method_def(method_span)?;
                methods.push(method);
            } else {
                let field_name = self.expect_ident("se esperaba nombre de campo o `fn`")?;
                self.expect(
                    &Token::Colon,
                    "se esperaba ':' después del nombre del campo",
                )?;
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
                });
            }
            // Separador opcional: coma. Newline se consume en la
            // próxima iteración por skip_newlines.
            if matches!(self.peek(), Token::Comma) {
                self.advance();
            }
        }
    }

    /// Parsea un método custom adentro del bloque `type` (R.3).
    /// Sintaxis idéntica a `parse_fndef`, pero NO admite decoradores
    /// (los métodos no aceptan `@get`/`@server`/etc.) y emite
    /// `MethodDef` en lugar de `Stmt::FnDef`.
    fn parse_method_def(&mut self, span: Span) -> FitzResult<MethodDef> {
        // Mini-tanda St — `static [async] fn ...` declara un método
        // estático (sin receiver `self`). `static` debe preceder a
        // `async`/`fn`.
        let is_static = self.eat(&Token::Static);
        let is_async = self.eat(&Token::Async);
        self.expect(&Token::Fn, "se esperaba 'fn'")?;
        let name = self.expect_ident("se esperaba nombre del método después de 'fn'")?;
        self.expect(
            &Token::LParen,
            "se esperaba '(' después del nombre del método",
        )?;
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
                    "se esperaba '{' o '=>' para el cuerpo del método",
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

    // ---------- decoradores ----------
    //
    // Forma:
    //   @nombre(arg1, arg2, ...)
    //   [@otro_deco(...)]*
    //   [async] fn handler(...) [-> Type] { ... }
    //
    // Acumulamos los decoradores en `Decorator { name, args, kwargs }`
    // y los pegamos al `Stmt::FnDef` resultante. La semántica (qué
    // hace cada decorator) es responsabilidad del evaluador: el
    // parser solo garantiza que estructuralmente vienen antes de una
    // fn, que la sintaxis es `@Ident(args, key=value)`, y que los
    // kwargs van después de los positionals. Args y values son
    // expresiones cualquiera — el decorator específico decide qué
    // tipos acepta en runtime.
    //
    // Hasta 4.1 el evaluador corta con error explícito en cuanto ve
    // decorators no vacíos; 4.2 cablea `@get`/`@post`/`@put`/`@delete`
    // contra el runtime HTTP.

    fn parse_decorated_fndef(&mut self, span: Span) -> FitzResult<Stmt> {
        let mut decorators: Vec<Decorator> = Vec::new();
        // Al menos uno: el llamador entró acá viendo `@`.
        loop {
            decorators.push(self.parse_one_decorator()?);
            // Permitimos newline entre decorators apilados.
            self.skip_newlines();
            if !matches!(self.peek(), Token::At) {
                break;
            }
        }

        // El handler debe ser una FnDef (con `async` opcional). Si el
        // usuario pone otra cosa, error claro y temprano.
        if !matches!(self.peek(), Token::Fn | Token::Async) {
            return Err(self.error(
                ErrorKind::UnexpectedToken,
                "después de un decorador debe venir una definición de función",
            ));
        }
        let fndef = self.parse_fndef(span)?;
        // `parse_fndef` siempre devuelve un `Stmt::FnDef`; le pegamos los
        // decoradores acumulados.
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
            // Inalcanzable: parse_fndef es total.
            other => Ok(other),
        }
    }

    /// Parsea un único decorador (`@ Ident ( args )?`), con el `@`
    /// aún sin consumir. Devuelve el `Decorator` listo; el caller
    /// decide si seguir acumulando.
    ///
    /// Los paréntesis son **opcionales** desde 9.z.2.a (necesario
    /// para `@test fn ...` que no toma args). Decoradores sin paréntesis
    /// son equivalentes a `@nombre()` (args = kwargs = vacíos). Cambio
    /// retro-compatible: `@server()` y `@get("/x")` siguen funcionando
    /// idéntico.
    fn parse_one_decorator(&mut self) -> FitzResult<Decorator> {
        self.expect(&Token::At, "se esperaba '@'")?;
        let name = self.expect_ident("se esperaba nombre de decorador después de '@'")?;
        // Si viene `(`, parseamos args; si no, decorator sin args.
        // El próximo significativo determina la rama (saltando newlines
        // no — el `(` debe venir en la misma línea que el nombre, para
        // evitar ambigüedades con el próximo stmt).
        let (args, kwargs) = if matches!(self.peek(), Token::LParen) {
            self.advance();
            self.parse_decorator_args()?
        } else {
            (Vec::new(), Vec::new())
        };
        Ok(Decorator { name, args, kwargs })
    }

    /// Parsea los argumentos de un decorator después del `(` consumido.
    /// Separa positionals de kwargs. La regla:
    ///
    /// - Mientras el siguiente arg sea una expresión suelta, va a
    ///   `args` (positional).
    /// - Detección de kwarg: `Ident '='` (con `Token::Eq`, NO
    ///   `Token::EqEq` — `a == b` sigue siendo una expresión válida
    ///   como arg posicional).
    /// - Una vez visto el primer kwarg, **todos** los args siguientes
    ///   deben ser kwargs; un positional posterior es error.
    /// - Kwargs duplicados son error.
    ///
    /// Termina consumiendo el `)`. Acepta lista vacía, coma trailing
    /// y newlines entre elementos.
    #[allow(clippy::type_complexity)]
    fn parse_decorator_args(&mut self) -> FitzResult<(Vec<Expr>, Vec<(String, Expr)>)> {
        let mut args: Vec<Expr> = Vec::new();
        let mut kwargs: Vec<(String, Expr)> = Vec::new();
        self.skip_newlines();
        // Caso vacío: @deco()
        if matches!(self.peek(), Token::RParen) {
            self.advance();
            return Ok((args, kwargs));
        }
        loop {
            self.skip_newlines();
            // Detección de kwarg: Ident seguido de `=` (Token::Eq).
            // `==` (Token::EqEq) NO dispara: es un BinOp en una expresión
            // posicional.
            let is_kwarg =
                matches!(self.peek(), Token::Ident(_)) && matches!(self.peek_at(1), Token::Eq);
            if is_kwarg {
                let key_tok = self.advance();
                let key = match key_tok.token {
                    Token::Ident(s) => s,
                    _ => unreachable!("verificado por is_kwarg"),
                };
                // Consumir el `=`.
                self.advance();
                self.skip_newlines();
                let value = self.expression()?;
                // Duplicado.
                if kwargs.iter().any(|(k, _)| k == &key) {
                    return Err(self.error(
                        ErrorKind::InvalidSyntax,
                        format!(
                            "argumento por nombre '{}=' ya fue dado en el mismo decorador",
                            key
                        ),
                    ));
                }
                kwargs.push((key, value));
            } else {
                // Positional. Si ya hubo kwargs, es error.
                if !kwargs.is_empty() {
                    return Err(self.error(
                        ErrorKind::InvalidSyntax,
                        "los argumentos posicionales no pueden ir después de \
                         argumentos por nombre (key=value)"
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
        self.expect(
            &Token::RParen,
            "se esperaba ')' para cerrar los argumentos del decorador",
        )?;
        Ok((args, kwargs))
    }

    /// Parsea los argumentos de una llamada, ya con '(' consumido.
    /// Termina consumiendo el ')'. Acepta lista vacía, coma trailing,
    /// y newlines entre elementos (útil para llamadas multilínea).
    fn parse_call_args(&mut self) -> FitzResult<Vec<Expr>> {
        let mut args = Vec::new();
        let mut saw_named = false;
        self.skip_newlines();
        // Caso vacío: f()
        if matches!(self.peek(), Token::RParen) {
            self.advance();
            return Ok(args);
        }
        loop {
            self.skip_newlines();
            // Fp.3 — `name: value` con lookahead Ident + Colon. Mismo
            // patrón que kwargs de decoradores (eval ya lo hace para
            // `@server(port=3000)`). El parser no chequea aquí si el
            // name corresponde a un param real — eso lo hace el checker
            // y el evaluator/codegen al despachar la call.
            let (start_line, start_col) = self.current_pos();
            let is_named =
                matches!(self.peek(), Token::Ident(_)) && matches!(self.peek_at(1), Token::Colon);
            let arg = if is_named {
                let name = self
                    .expect_ident("se esperaba nombre de argumento")
                    .unwrap();
                self.advance(); // consume `:`
                let value = self.expression()?;
                saw_named = true;
                Expr::NamedArg {
                    name,
                    value: Box::new(value),
                    span: Span::new(start_line, start_col),
                }
            } else {
                if saw_named {
                    return Err(self.error(
                        ErrorKind::UnexpectedToken,
                        "no se pueden mezclar args posicionales después de args nombrados — \
                         los nombrados van al final",
                    ));
                }
                self.expression()?
            };
            args.push(arg);
            self.skip_newlines();
            if matches!(self.peek(), Token::Comma) {
                self.advance();
                self.skip_newlines();
                // Trailing comma: f(1, 2,)
                if matches!(self.peek(), Token::RParen) {
                    self.advance();
                    return Ok(args);
                }
            } else {
                break;
            }
        }
        self.expect(&Token::RParen, "se esperaba ')' para cerrar la llamada")?;
        Ok(args)
    }

    /// Expresión "hoja": literal, identificador, paréntesis, `if`,
    /// `match`, list literal `[...]` o map literal `{...}`. Acá
    /// termina la recursión hacia abajo en la escalera.
    ///
    /// Nota sobre `{`: en posición de expresión SIEMPRE arranca un
    /// map literal. Los bloques (`fn ... { body }`, `if cond { ... }`,
    /// etc.) consumen su `{` desde `parse_block`/`parse_match_expr`,
    /// no desde acá — el flujo nunca cae en este caso para esos
    /// constructos.
    fn primary(&mut self) -> FitzResult<Expr> {
        // `if` y `match` son expresiones — las manejamos antes de
        // consumir el token para que sus parsers lo hagan.
        match self.peek() {
            Token::If => return self.parse_if_expr(),
            Token::Match => return self.parse_match_expr(),
            // Mini-tanda L — `loop { body }` como expresión. En
            // statement position, `parse_stmt` ya intercepta
            // Token::Loop antes; este branch solo aplica en RHS de
            // let, args, etc. Devuelve `Expr::Loop { body }`.
            Token::Loop => return self.parse_loop_expr(None),
            // Mini-tanda L.2 — `'label: loop { ... }` como expresión.
            // Detectamos Label + lookahead Colon + Loop.
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
            // `fn(...)` o `fn(...) => expr` — función anónima en posición
            // de expresión. `fn name(...)` no es válido acá: una function
            // con nombre es `Stmt::FnDef`, sentencia, no expresión.
            Token::Fn if matches!(self.peek_at(1), Token::LParen) => {
                return self.parse_fn_expr();
            }
            // Mini-tanda Async-cl — `async fn(...)` closure async en
            // posición de expresión. Reusa `parse_fn_expr` (que detecta
            // el `async` prefijo y setea `is_async`).
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
                // Mini-tanda T — distinguimos:
                //   `()`        → tupla vacía.
                //   `(e,)`      → tupla de 1 elemento (trailing comma).
                //   `(e1, ...)` → tupla.
                //   `(e)`       → solo paréntesis de agrupación.
                //
                // Adentro de paréntesis no hay ambigüedad con bloques:
                // limpiamos `no_struct_literal` para permitir struct
                // literals (habilita `(User { id: 1 }) == other`).
                let prev = std::mem::replace(&mut self.no_struct_literal, false);
                // Caso: tupla vacía `()`.
                if matches!(self.peek(), Token::RParen) {
                    self.advance();
                    self.no_struct_literal = prev;
                    return Ok(Expr::Tuple(Vec::new(), tok_span));
                }
                let first_result = self.expression();
                self.no_struct_literal = prev;
                let first = first_result?;
                // Si la próxima es coma → tupla.
                if matches!(self.peek(), Token::Comma) {
                    let mut items = vec![first];
                    while matches!(self.peek(), Token::Comma) {
                        self.advance(); // consume `,`
                                        // Trailing comma admitida: `(e,)` o `(e1, e2,)`.
                        if matches!(self.peek(), Token::RParen) {
                            break;
                        }
                        let prev2 = std::mem::replace(&mut self.no_struct_literal, false);
                        let r = self.expression();
                        self.no_struct_literal = prev2;
                        items.push(r?);
                    }
                    self.expect(&Token::RParen, "se esperaba ')' para cerrar la tupla")?;
                    return Ok(Expr::Tuple(items, tok_span));
                }
                // Sin coma → solo paréntesis de agrupación.
                self.expect(&Token::RParen, "se esperaba ')' para cerrar el paréntesis")?;
                Ok(first)
            }
            other => Err(FitzError::new(
                ErrorKind::UnexpectedToken,
                tok.line,
                tok.column,
                format!("Se esperaba una expresión, se encontró '{:?}'", other),
            )),
        }
    }

    /// `[expr, expr, ...]` — lista literal. Acepta vacía `[]`,
    /// trailing comma y newlines entre elementos (útil para listas
    /// multilínea).
    fn parse_list_literal(&mut self) -> FitzResult<Expr> {
        let span = self.cur_span();
        self.expect(&Token::LBracket, "se esperaba '['")?;
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
            // Mini-tanda C: tras parsear el primer expr, si viene `for`,
            // es una list comprehension. Solo cuando items está vacío
            // (no podemos mezclar `[1, 2 for x in xs]`).
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
        self.expect(&Token::RBracket, "se esperaba ']' para cerrar la lista")?;
        Ok(Expr::List(items, span))
    }

    /// Mini-tanda C + Cmp+ — parsea la cola de una list comprehension
    /// después del expr inicial: `for <var> in <iter> [for ...]* [if cond]?]`.
    /// El primer `[` y el `expr` ya fueron consumidos por el caller.
    /// Mini-tanda Cmp+ extiende esto para múltiples `for` clauses
    /// (cartesian product); el `if` opcional al final se evalúa adentro
    /// del loop más interno.
    fn parse_list_comprehension_tail(&mut self, span: Span, expr: Expr) -> FitzResult<Expr> {
        let (var, iter, extra_clauses, filter) =
            self.parse_comprehension_clauses(&Token::RBracket, "list comprehension")?;
        self.expect(
            &Token::RBracket,
            "se esperaba ']' para cerrar la list comprehension",
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

    /// Mini-tanda Cmp+ — parsea las clauses `for <pat> in <iter>` (1 o
    /// más) y un `if <cond>` opcional al final. Comparte la lógica entre
    /// list comprehension (`[expr for ...]`) y map comprehension
    /// (`{k: v for ...}`). Devuelve `(var, iter, extra_clauses, filter)`:
    /// el primer for sale separado por compatibilidad con el shape AST
    /// actual; las clauses 2+ van en `extra_clauses`. No consume el
    /// delimitador de cierre (`]` o `}`); el caller lo expecta.
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
        self.expect(&Token::For, format!("se esperaba 'for' en {}", context))?;
        let var = self.parse_pattern()?;
        self.expect(
            &Token::In,
            format!("se esperaba 'in' después de la variable en {}", context),
        )?;
        self.skip_newlines();
        let iter = self.expression()?;
        self.skip_newlines();

        let mut extra_clauses: Vec<(crate::ast::Pattern, Expr)> = Vec::new();
        // Múltiples `for` clauses: `[expr for a in xs for b in ys]`.
        while matches!(self.peek(), Token::For) {
            self.advance(); // consume `for`
            let extra_var = self.parse_pattern()?;
            self.expect(
                &Token::In,
                format!(
                    "se esperaba 'in' después de la variable extra en {}",
                    context
                ),
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

    /// `{"k": v, ...}` — mapa literal. Acepta vacío `{}`, trailing
    /// comma y newlines entre pares. La clave es una expresión, no un
    /// identificador suelto: para usar el valor de una variable como
    /// clave, los strings literales son lo natural (`{"name": x}`),
    /// pero `{key_expr: value}` es válido si `key_expr` evalúa a algo
    /// hasheable en runtime.
    fn parse_map_literal(&mut self) -> FitzResult<Expr> {
        let span = self.cur_span();
        self.expect(&Token::LBrace, "se esperaba '{'")?;
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
        // Primer par: `key: value`.
        self.skip_newlines();
        let key = self.expression()?;
        self.expect(&Token::Colon, "se esperaba ':' entre clave y valor en mapa")?;
        self.skip_newlines();
        let value = self.expression()?;
        self.skip_newlines();

        // Mini-tanda Cmp+ — después del primer par, si viene `for`,
        // es una map comprehension `{k: v for ...}`. Si viene `,` o
        // `}`, es un map literal normal y seguimos parseando pares.
        if matches!(self.peek(), Token::For) {
            let (var, iter, extra_clauses, filter) =
                self.parse_comprehension_clauses(&Token::RBrace, "map comprehension")?;
            self.expect(
                &Token::RBrace,
                "se esperaba '}' para cerrar la map comprehension",
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
        // Resto de pares del map literal normal.
        loop {
            if matches!(self.peek(), Token::Comma) {
                self.advance();
                self.skip_newlines();
                if matches!(self.peek(), Token::RBrace) {
                    self.advance();
                    return Ok(Expr::Map(pairs, span));
                }
            } else {
                break;
            }
            self.skip_newlines();
            let key = self.expression()?;
            self.expect(&Token::Colon, "se esperaba ':' entre clave y valor en mapa")?;
            self.skip_newlines();
            let value = self.expression()?;
            pairs.push((key, value));
            self.skip_newlines();
        }
        self.expect(&Token::RBrace, "se esperaba '}' para cerrar el mapa")?;
        Ok(Expr::Map(pairs, span))
    }
}

/// Entrada pública del parser. Convierte tokens en un `Program`.
pub fn parse(tokens: Vec<TokenWithPos>) -> FitzResult<Program> {
    let mut parser = Parser::new(tokens);
    parser.parse_program()
}

/// Variante recovering del parser. Pensada para tooling externo (LSP,
/// formatter, futuras herramientas de análisis) que necesita un AST
/// parcial sobre buffers en construcción o con errores tipográficos
/// transitorios. **No** la usa la CLI strict (`fitz run`, `fitz build`,
/// `fitz check`): esos siguen llamando a `parse()` y abortan al primer
/// error. Fase 9.0.1 (F15).
///
/// Reglas:
///  - Captura errores stmt-level y los acumula en el `Vec<FitzError>`
///    devuelto. El AST devuelto siempre es estructuralmente válido (un
///    `Vec<Stmt>` posiblemente con `Stmt::Error(span)` en lugares
///    rotos).
///  - Sync points: `Newline`, `RBrace` (no consumido), `EOF`.
///  - Cota dura: `MAX_RECOVERED_ERRORS` (100). Al alcanzarla, el parser
///    abandona el resto del input y devuelve lo que tiene.
///  - Errores DENTRO de un stmt (paréntesis sin cerrar, expresión
///    incompleta, etc.) descartan el stmt entero — el cursor avanza
///    hasta el próximo sync point. Recovery sub-stmt queda como deuda
///    explícita para más adelante.
///
/// Garantiza que **nunca** retorna `Err`: cualquier error queda
/// acumulado en la lista paralela. El caller decide qué hacer.
///
/// `#[allow(dead_code)]`: en Fase 9.0.1 esta API solo se ejercita
/// desde tests. Los consumidores reales (LSP, formatter, futuras
/// herramientas) aterrizan en sub-pasos siguientes de Fase 9. El
/// allow se quita cuando aparezca el primer caller fuera de tests.
#[allow(dead_code)]
pub fn parse_with_recovery(tokens: Vec<TokenWithPos>) -> (Program, Vec<FitzError>) {
    let mut parser = Parser::new(tokens);
    parser.recovery_mode = true;
    // En recovery, `parse_program` no devuelve `Err` (los errores van a
    // `recovered_errors`); pero el tipo de retorno sigue siendo
    // `FitzResult` para no duplicar código. `unwrap_or_else` es defensa
    // por si alguna ruta strict-residual se cuela — en ese caso, el
    // error se acumula como parte de la lista.
    let stmts = parser.parse_program().unwrap_or_else(|e| {
        parser.recovered_errors.push(e);
        Vec::new()
    });
    (stmts, parser.recovered_errors)
}

/// Toma el contenido crudo de un `Token::Str` y construye la
/// expresión correspondiente: `Expr::Str` si es solo texto, o
/// `Expr::StrInterp` si tiene `{...}` interpolados.
///
/// Reglas de procesamiento:
///  - `\{` y `\}` se desescapan a `{` y `}` literales (el lexer los
///    preserva con la barra para que podamos distinguirlos acá).
///  - `{ ... }` no escapado abre interpolación. El contenido entre
///    llaves se re-tokeniza y se parsea como expresión.
///  - `}` suelto (sin `{` previo) es error — el usuario debe escapar
///    como `\}`.
///
/// Limitación residual:
///  - Strings dentro de la interpolación no se soportan: el buscador
///    de `}` es ingenuo y se confunde con `}` dentro de `"..."` anidados.
///  - Si el string contiene escapes (`\n`, `\t`, etc.), la columna
///    reportada en errores de interpolación está corrida un char por cada
///    escape anterior al error. Sin acceso al source original no podemos
///    reconstruir el mapping exacto.
fn build_string_expr(raw: &str, line: usize, column: usize) -> FitzResult<Expr> {
    let chars: Vec<char> = raw.chars().collect();
    let mut parts: Vec<StrPart> = Vec::new();
    let mut current_lit = String::new();
    let mut i = 0;
    let str_span = Span::new(line, column);

    // Columna del primer char del contenido del string en el source:
    // el `column` que recibimos apunta a la comilla de apertura `"`.
    let content_col = column + 1;

    while i < chars.len() {
        let c = chars[i];

        // Escape de '{' o '}' literal: '\{' o '\}'.
        if c == '\\' && i + 1 < chars.len() && (chars[i + 1] == '{' || chars[i + 1] == '}') {
            current_lit.push(chars[i + 1]);
            i += 2;
            continue;
        }

        // Inicio de interpolación.
        if c == '{' {
            // Columna del `{` en el source original (aproximada — ver
            // limitación residual sobre escapes).
            let interp_col = content_col + i;

            if !current_lit.is_empty() {
                parts.push(StrPart::Lit(std::mem::take(&mut current_lit)));
            }
            i += 1;
            let expr_start = i;
            // Buscar '}' que cierre. Ingenuo: no entiende strings
            // anidados — documentado como deuda.
            while i < chars.len() && chars[i] != '}' {
                i += 1;
            }
            if i >= chars.len() {
                return Err(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    line,
                    interp_col,
                    "Interpolación de string sin '}' de cierre",
                ));
            }
            let interp_src: String = chars[expr_start..i].iter().collect();

            // La subexpresión empieza un char después del `{` en el source.
            let sub_col_base = interp_col + 1;

            // Mini-tanda Fm — separar `expr` de `:spec` por el primer `:`
            // a depth 0 (no adentro de paréntesis/brackets/braces). Esto
            // permite que `{m["k"]:.2f}` distinga el `:` del spec del
            // de un map literal anidado.
            let (expr_src, spec_src) = split_expr_and_format_spec(&interp_src);

            // Re-tokenizamos. Cualquier error del sub-lexer lleva la
            // posición relativa al inicio de expr_src — la trasladamos al
            // source real para que el usuario vea la línea/columna correcta.
            let sub_tokens = tokenize(&expr_src).map_err(|mut e| {
                e.line = line;
                e.column = sub_col_base + e.column.saturating_sub(1);
                e
            })?;
            let mut sub_parser = Parser::new(sub_tokens);
            let expr = sub_parser.expression().map_err(|mut e| {
                e.line = line;
                e.column = sub_col_base + e.column.saturating_sub(1);
                e
            })?;
            // No debe quedar nada después de la expresión (más allá
            // del EOF que pone el lexer).
            if !sub_parser.is_at_end() {
                return Err(FitzError::new(
                    ErrorKind::InvalidSyntax,
                    line,
                    sub_col_base,
                    format!("Tokens extra dentro de interpolación: '{}'", expr_src),
                ));
            }
            // Mini-tanda Fm — si había `:spec`, parsearlo a FormatSpec.
            let format_spec = if let Some(spec) = spec_src {
                Some(parse_format_spec(&spec).map_err(|msg| {
                    FitzError::new(
                        ErrorKind::InvalidSyntax,
                        line,
                        sub_col_base + expr_src.len() + 1,
                        format!("Format spec inválido `{}`: {}", spec, msg),
                    )
                })?)
            } else {
                None
            };
            parts.push(StrPart::Expr(expr, format_spec));
            i += 1; // saltar '}'
            continue;
        }

        // '}' sin '{' previo — el usuario probablemente quiso escaparlo.
        if c == '}' {
            return Err(FitzError::new(
                ErrorKind::InvalidSyntax,
                line,
                content_col + i,
                "'}' suelto en string — escapá como '\\}' para incluirlo literal",
            ));
        }

        current_lit.push(c);
        i += 1;
    }

    if !current_lit.is_empty() {
        parts.push(StrPart::Lit(current_lit));
    }

    // Si todas las partes son literales (o no hay partes), devolvemos
    // un `Expr::Str` simple — nada que interpolar. Si hay al menos
    // una `StrPart::Expr`, va a `Expr::StrInterp`.
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

/// Mini-tanda Fm — separa `{expr:spec}` en `(expr_src, Some(spec))`,
/// o `(expr_src, None)` si no hay spec. El split toma el primer `:`
/// que NO está adentro de paréntesis/brackets/braces balanceados.
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

/// Mini-tanda Fm — parsea un format spec estilo Python.
/// Gramática: `[[fill]align][sign][#][0][width][grouping][.precision][type]`.
fn parse_format_spec(s: &str) -> Result<FormatSpec, String> {
    use crate::ast::{FormatKind, FormatSign};
    let mut spec = FormatSpec::default();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;

    // fill + align: 2 chars donde el segundo es align.
    if chars.len() >= 2 {
        if let Some(a) = align_from_char(chars[1]) {
            spec.fill = Some(chars[0]);
            spec.align = Some(a);
            i = 2;
        }
    }
    // align solo.
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
                .map_err(|_| format!("width inválido: `{}`", width_str))?,
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
            return Err("precision tras `.` requiere al menos un dígito".into());
        }
        let prec_str: String = chars[prec_start..i].iter().collect();
        spec.precision = Some(
            prec_str
                .parse::<usize>()
                .map_err(|_| format!("precision inválida: `{}`", prec_str))?,
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
// Tests — helpers del Parser
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::approx_constant)] // 3.14 en tests es un Float genérico, no PI.
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    /// Helper: tokeniza el source y crea un Parser listo para tests.
    fn parser(src: &str) -> Parser {
        let tokens = tokenize(src).expect("la fuente debe tokenizar sin error");
        Parser::new(tokens)
    }

    #[test]
    fn peek_returns_current_token_without_advancing() {
        let p = parser("42 + 1");
        assert_eq!(*p.peek(), Token::Int(42));
        // Segunda llamada: mismo token, no consumió.
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
        // Aunque llamemos advance varias veces, seguimos en EOF.
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
        // No coincide: no consume.
        assert!(!p.eat(&Token::Plus));
        assert!(p.eat(&Token::Minus));
        assert!(p.is_at_end());
    }

    #[test]
    fn expect_returns_ok_on_match() {
        let mut p = parser("(");
        assert!(p.expect(&Token::LParen, "se esperaba '('").is_ok());
        assert!(p.is_at_end());
    }

    #[test]
    fn expect_returns_err_with_token_position_on_mismatch() {
        let mut p = parser("42");
        let err = p.expect(&Token::LParen, "se esperaba '('").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
        assert_eq!(err.line, 1);
        assert_eq!(err.column, 1);
        assert!(err.message.contains("se esperaba '('"));
    }

    #[test]
    fn expect_ident_extracts_name() {
        let mut p = parser("user");
        let name = p.expect_ident("se esperaba identificador").unwrap();
        assert_eq!(name, "user");
        assert!(p.is_at_end());
    }

    #[test]
    fn expect_ident_fails_on_keyword() {
        // 'fn' es keyword, no Ident — debe fallar.
        let mut p = parser("fn");
        let err = p.expect_ident("se esperaba identificador").unwrap_err();
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
        // Antes de consumir: Let en (1, 1)
        assert_eq!(p.current_pos(), (1, 1));
        p.advance(); // consume Let
                     // Próximo token: Newline en (1, 4)
        assert_eq!(p.current_pos(), (1, 4));
        p.advance(); // consume Newline
                     // Próximo token: Ident("x") en (2, 3)
        assert_eq!(p.current_pos(), (2, 3));
    }

    #[test]
    fn parse_empty_source_returns_empty_program() {
        let tokens = tokenize("").unwrap();
        let program = parse(tokens).unwrap();
        assert!(program.is_empty());
    }

    // -----------------------------------------------------------------------
    // Tests — Span en Expr (S1.2 sub-paso 1)
    //
    // El parser propaga `Span { line, column }` a cada nodo `Expr`. Estos
    // tests fijan los call sites de las 5 reglas más visibles para el
    // checker (literal, BinOp, Call, Field, Index) y dejan que cualquier
    // refactor que pierda spans se note en la suite. Comparan posiciones
    // explícitamente (no por `assert_eq!` sobre `Expr` — `Span::PartialEq`
    // siempre es trivial, así que la única forma de validar la posición es
    // mirar `.span().line` / `.span().column` directamente).
    // -----------------------------------------------------------------------

    #[test]
    fn span_literal_apunta_al_primer_token() {
        // En `  42`, el `42` arranca en columna 3 (1-indexed). El span
        // del nodo `Expr::Int` reusa la posición del token literal.
        let e = parse_expr("  42").unwrap();
        let s = e.span();
        assert_eq!(s.line, 1);
        assert_eq!(s.column, 3);
        // Sanidad: también para Str e Ident.
        let e = parse_expr("\"hola\"").unwrap();
        assert_eq!(e.span().column, 1);
        let e = parse_expr("user").unwrap();
        assert_eq!(e.span().column, 1);
    }

    #[test]
    fn span_binop_apunta_al_operador_no_al_left() {
        // En `1 + 2`, el `+` está en columna 3. El span de `Expr::BinOp`
        // debe apuntar al operador (criterio rustc/clang). El left
        // (`Expr::Int(1)`) tiene su propio span en columna 1.
        let e = parse_expr("1 + 2").unwrap();
        let outer = e.span();
        assert_eq!(outer.line, 1);
        assert_eq!(outer.column, 3);
        if let Expr::BinOp { left, .. } = &e {
            // El sub-nodo `left` mantiene su span propio.
            assert_eq!(left.span().column, 1);
        } else {
            panic!("se esperaba BinOp, se obtuvo {:?}", e);
        }
    }

    #[test]
    fn span_call_apunta_al_paren_de_apertura() {
        // En `f(1, 2)`, el `(` está en columna 2. El span de `Expr::Call`
        // debe apuntar al `(`, no al callee (que tiene su propio span
        // en columna 1).
        let e = parse_expr("f(1, 2)").unwrap();
        assert_eq!(e.span().column, 2);
        if let Expr::Call { callee, .. } = &e {
            assert_eq!(callee.span().column, 1);
        } else {
            panic!("se esperaba Call, se obtuvo {:?}", e);
        }
    }

    #[test]
    fn span_field_apunta_al_punto() {
        // En `user.name`, el `.` está en columna 5. El span de
        // `Expr::Field` apunta al `.`; el receptor mantiene su span en
        // columna 1.
        let e = parse_expr("user.name").unwrap();
        assert_eq!(e.span().column, 5);
        if let Expr::Field { object, .. } = &e {
            assert_eq!(object.span().column, 1);
        } else {
            panic!("se esperaba Field, se obtuvo {:?}", e);
        }
    }

    #[test]
    fn span_index_apunta_al_corchete() {
        // En `xs[0]`, el `[` está en columna 3. El span de `Expr::Index`
        // apunta al `[`; el receptor mantiene su span en columna 1, y
        // el índice en columna 4.
        let e = parse_expr("xs[0]").unwrap();
        assert_eq!(e.span().column, 3);
        if let Expr::Index { object, index, .. } = &e {
            assert_eq!(object.span().column, 1);
            assert_eq!(index.span().column, 4);
        } else {
            panic!("se esperaba Index, se obtuvo {:?}", e);
        }
    }

    // -----------------------------------------------------------------------
    // Tests — `.await` postfix (Fase 6.1)
    //
    // El parser construye `Expr::Await(inner, span)` cuando ve `.await`
    // después de cualquier expresión postfix. La keyword `await` ya está
    // tokenizada como `Token::Await` desde antes de Fase 6 (token dormido).
    // El checker/evaluator/codegen rechazan el nodo con error explícito
    // hasta 6.2/6.4/6.6; los tests de barrera viven en `types.rs`,
    // `evaluator.rs` y `codegen.rs`.
    // -----------------------------------------------------------------------

    #[test]
    fn await_postfix_envuelve_ident_receptor() {
        let e = parse_expr("x.await").unwrap();
        match e {
            Expr::Await(inner, _) => {
                assert_eq!(*inner, Expr::Ident("x".into(), Span::ZERO));
            }
            other => panic!("se esperaba Await, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn await_postfix_envuelve_call() {
        // `f(x).await` → Await(Call(...))
        let e = parse_expr("f(x).await").unwrap();
        match e {
            Expr::Await(inner, _) => {
                assert!(
                    matches!(*inner, Expr::Call { .. }),
                    "se esperaba Await(Call), inner fue {:?}",
                    inner
                );
            }
            other => panic!("se esperaba Await, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn await_se_encadena_con_method_chain() {
        // `xs.map(f).await` → Await(Call(callee=Field(xs, "map"), args=[f]))
        let e = parse_expr("xs.map(f).await").unwrap();
        match e {
            Expr::Await(inner, _) => match *inner {
                Expr::Call { callee, .. } => {
                    assert!(matches!(*callee, Expr::Field { .. }));
                }
                other => panic!("se esperaba Call adentro de Await, fue {:?}", other),
            },
            other => panic!("se esperaba Await, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn await_seguido_de_try_es_try_de_await() {
        // `expr.await?` → Try(Await(expr))
        // El postfix loop procesa `.await` primero, después `?`.
        let e = parse_expr("x.await?").unwrap();
        match e {
            Expr::Try(inner, _) => {
                assert!(
                    matches!(*inner, Expr::Await(..)),
                    "se esperaba Try(Await(..)), fue {:?}",
                    inner
                );
            }
            other => panic!("se esperaba Try, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn await_seguido_de_field_es_field_de_await() {
        // `expr.await.name` → Field(Await(expr), "name")
        let e = parse_expr("x.await.name").unwrap();
        match e {
            Expr::Field { object, field, .. } => {
                assert_eq!(field, "name");
                assert!(matches!(*object, Expr::Await(..)));
            }
            other => panic!("se esperaba Field, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn await_doble_anida_los_await() {
        // `x.await.await` → Await(Await(x))
        let e = parse_expr("x.await.await").unwrap();
        match e {
            Expr::Await(outer_inner, _) => match *outer_inner {
                Expr::Await(inner, _) => {
                    assert_eq!(*inner, Expr::Ident("x".into(), Span::ZERO));
                }
                other => panic!("se esperaba Await anidado, fue {:?}", other),
            },
            other => panic!("se esperaba Await externo, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn span_del_await_apunta_al_punto() {
        // En `user.await`, el `.` está en columna 5. El span del nodo
        // `Expr::Await` apunta al `.` (paralelo a `Field`).
        let e = parse_expr("user.await").unwrap();
        assert_eq!(e.span().line, 1);
        assert_eq!(e.span().column, 5);
        if let Expr::Await(inner, _) = &e {
            // El receptor mantiene su span propio en columna 1.
            assert_eq!(inner.span().column, 1);
        } else {
            panic!("se esperaba Await, se obtuvo {:?}", e);
        }
    }

    #[test]
    fn future_como_anotacion_de_tipo_parsea_como_generic() {
        // `Future<T>` reusa `TypeExpr::Generic` igual que `List<T>` —
        // no necesita variante nueva en el AST. Test ancla la decisión
        // de 6.1: si en el futuro alguien suma `TypeExpr::Future`
        // dedicada, este test cambia explícitamente.
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
            other => panic!("se esperaba FnDef con return Future<Int>, fue {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Tests — expresiones (paso 2: escalera de precedencia)
    // -----------------------------------------------------------------------

    /// Helper: parsea una sola expresión desde el código fuente.
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
        // (42) parsea como Int(42) — los paréntesis no agregan nodo
        // al AST, solo controlan precedencia.
        assert_eq!(parse_expr("(42)").unwrap(), Expr::Int(42, Span::ZERO));
    }

    #[test]
    fn primary_unclosed_paren_errors() {
        let err = parse_expr("(42").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn primary_errors_on_unexpected_token() {
        // ')' aislado no inicia ninguna expresión válida.
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

    // ---------------- R.1.1 — `not` (mini-fase R) ----------------

    #[test]
    fn unary_not_parsea_sobre_bool_literal() {
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
    fn unary_not_parsea_sobre_ident() {
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
    fn unary_not_tiene_precedencia_mayor_que_eq() {
        // `not x == y` → `(not x) == y` (asociatividad left-to-right
        // del unary, mayor precedencia que ==).
        let expr = parse_expr("not x == y").unwrap();
        // El nodo raíz es BinOp Eq con left = UnaryOp Not.
        match expr {
            Expr::BinOp { op, left, .. } => {
                assert_eq!(op, BinOpKind::Eq);
                match *left {
                    Expr::UnaryOp {
                        op: UnaryOpKind::Not,
                        ..
                    } => {}
                    other => panic!("esperaba UnaryOp Not, fue {:?}", other),
                }
            }
            other => panic!("esperaba BinOp Eq, fue {:?}", other),
        }
    }

    #[test]
    fn unary_not_en_condicion_de_if() {
        // `if not active { ... }` parsea OK.
        let stmt = parse_one_stmt("if (not active) { print(\"x\") }");
        match stmt {
            Stmt::Assign { .. } | Stmt::Expr(_, _) => {
                // Stmt::If se modela como Stmt::Expr(Expr::If, _).
            }
            other => panic!("esperaba Stmt::Expr(If), fue {:?}", other),
        }
    }

    // ---------------- R.1.2 — operador `%` (mini-fase R) ----------------

    #[test]
    fn op_modulo_parsea_con_misma_precedencia_que_mul() {
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
                other => panic!("esperaba BinOp Mod en right, fue {:?}", other),
            },
            other => panic!("esperaba BinOp Add raíz, fue {:?}", other),
        }
    }

    #[test]
    fn op_modulo_left_associative_con_mul() {
        // 10 % 3 * 2 → (10 % 3) * 2 (left-to-right entre mismos
        // niveles de precedencia).
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
                other => panic!("esperaba BinOp Mod en left, fue {:?}", other),
            },
            other => panic!("esperaba BinOp Mul raíz, fue {:?}", other),
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

    // ---------------- R.1.3 — asignación a índice (mini-fase R) ----------------

    #[test]
    fn assign_index_list_parsea() {
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
            other => panic!("esperaba Stmt::Assign Index, fue {:?}", other),
        }
    }

    #[test]
    fn assign_index_map_str_key_parsea() {
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
            other => panic!("esperaba Stmt::Assign Index, fue {:?}", other),
        }
    }

    #[test]
    fn assign_index_con_expresion_compleja_como_index() {
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
            other => panic!("esperaba Stmt::Assign Index, fue {:?}", other),
        }
    }

    // ---------------- R.1.4 — rangos inclusivos `..=` (mini-fase R) ----------------

    #[test]
    fn range_inclusive_expr_parsea() {
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
                assert!(inclusive, "..= debe parsear como inclusive");
            }
            other => panic!("esperaba Expr::Range, fue {:?}", other),
        }
    }

    #[test]
    fn range_exclusive_sigue_andando() {
        let expr = parse_expr("0..10").unwrap();
        match expr {
            Expr::Range { inclusive, .. } => {
                assert!(!inclusive, ".. (sin =) debe parsear como exclusive");
            }
            other => panic!("esperaba Expr::Range, fue {:?}", other),
        }
    }

    #[test]
    fn range_inclusive_pattern_en_match() {
        // 0..=59 en pattern de match.
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
                other => panic!("esperaba Range inclusive 0..=59, fue {:?}", other),
            },
            other => panic!("esperaba Stmt::Assign con Match, fue {:?}", other),
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
    // Tests — postfix (paso 3: field access y call)
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
        // Coma trailing válida — útil para diffs limpios.
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
        // Dentro de '(' ... ')' los newlines se ignoran.
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
        // `foo.bar()` ahora parsea: `Call { callee: Field { foo, bar }, args: [] }`.
        // Antes el parser tiraba error (deuda explícita de 2.3). El dispatch
        // de método como tal lo verifica el evaluador.
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
        // -foo.bar → -(foo.bar)  (postfix tiene mayor precedencia que unary)
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
    // PreF8.2: method chain multi-línea
    // -----------------------------------------------------------------------
    //
    // El postfix loop tolera Newline antes de `.` y continúa la expresión.
    // El AST resultante es idéntico al de la versión one-liner equivalente.

    #[test]
    fn method_chain_multilinea_parsea_igual_que_oneliner() {
        let one = parse_expr("xs.filter(f).map(g)").unwrap();
        let many = parse_expr("xs\n    .filter(f)\n    .map(g)").unwrap();
        assert_eq!(one, many);
    }

    #[test]
    fn method_chain_de_3_lineas_anida_correctamente() {
        // xs\n.a()\n.b()\n.c() → Call(Field(Call(Field(Call(Field(xs, a)), b)), c))
        let e = parse_expr("xs\n  .a()\n  .b()\n  .c()").unwrap();
        let Expr::Call { callee, .. } = e else {
            panic!("se esperaba Call externo")
        };
        let Expr::Field { object, field, .. } = *callee else {
            panic!("callee externo debía ser Field")
        };
        assert_eq!(field, "c");
        let Expr::Call { callee, .. } = *object else {
            panic!("nivel 2 debía ser Call")
        };
        let Expr::Field { object, field, .. } = *callee else {
            panic!("callee nivel 2 debía ser Field")
        };
        assert_eq!(field, "b");
        let Expr::Call { callee, .. } = *object else {
            panic!("nivel 3 debía ser Call")
        };
        let Expr::Field {
            object: receptor,
            field,
            ..
        } = *callee
        else {
            panic!("callee nivel 3 debía ser Field")
        };
        assert_eq!(field, "a");
        assert_eq!(*receptor, Expr::Ident("xs".into(), Span::ZERO));
    }

    #[test]
    fn field_access_multilinea_parsea_igual_que_oneliner() {
        // Sin paréntesis: solo field access encadenado.
        let one = parse_expr("user.profile.email").unwrap();
        let many = parse_expr("user\n  .profile\n  .email").unwrap();
        assert_eq!(one, many);
    }

    #[test]
    fn await_multilinea_se_encadena_al_receptor() {
        // `fut\n  .await` → Await(fut)
        let one = parse_expr("fut.await").unwrap();
        let many = parse_expr("fut\n  .await").unwrap();
        assert_eq!(one, many);
    }

    #[test]
    fn method_chain_multilinea_con_newlines_en_blanco_funciona() {
        // Más de un newline entre eslabones: siguen consumiéndose todos.
        let one = parse_expr("xs.a().b()").unwrap();
        let many = parse_expr("xs\n\n\n    .a()\n\n    .b()").unwrap();
        assert_eq!(one, many);
    }

    #[test]
    fn method_chain_multilinea_no_consume_newline_si_no_sigue_dot() {
        // `let x = foo` seguido de `bar()` en línea siguiente: dos
        // statements, NO una llamada `foo()` que se "continúa". El
        // lookahead solo dispara cuando lo que sigue es `.`.
        let program = parse_program_str("let x = foo\nbar()").unwrap();
        assert_eq!(program.len(), 2, "se esperaban 2 stmts separados");
    }

    #[test]
    fn method_chain_multilinea_funciona_en_rhs_de_let() {
        // Caso de uso canónico: chain como RHS de un `let`.
        let program =
            parse_program_str("let nombres = users\n  .filter(activo)\n  .map(nombre)").unwrap();
        assert_eq!(program.len(), 1);
        let Stmt::Assign { value, .. } = &program[0] else {
            panic!("se esperaba Assign")
        };
        // El value debe ser una Call con callee Field.
        let Expr::Call { callee, .. } = value else {
            panic!("se esperaba Call en RHS")
        };
        let Expr::Field { field, .. } = callee.as_ref() else {
            panic!("callee debía ser Field")
        };
        assert_eq!(field, "map");
    }

    #[test]
    fn dot_a_inicio_de_statement_sin_receptor_sigue_siendo_error() {
        // No debería convertirse en continuación de nada: `.foo()` solo
        // arrancando una línea sigue siendo error (Dot no es primary).
        let result = parse_program_str(".foo()");
        assert!(result.is_err(), "se esperaba error de parseo");
    }

    // -----------------------------------------------------------------------
    // Tests — sentencias (paso 4: assign / return / expr-stmt / programa)
    // -----------------------------------------------------------------------

    /// Helper: parsea un programa y devuelve el `Program` (lista de stmts).
    fn parse_program_str(src: &str) -> FitzResult<Program> {
        parse(tokenize(src).unwrap())
    }

    /// Helper: parsea un programa que se espera tenga exactamente una
    /// sentencia, y devuelve esa sentencia.
    fn parse_one_stmt(src: &str) -> Stmt {
        let program = parse_program_str(src).expect("parseo OK");
        assert_eq!(program.len(), 1, "se esperaba una sola sentencia");
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
                target: AssignTarget::Ident("x".into()),
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
                target: AssignTarget::Ident("x".into()),
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
                target: AssignTarget::Ident("x".into()),
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
                target: AssignTarget::Ident("name".into()),
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
                target: AssignTarget::Ident("x".into()),
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
    fn return_status_con_body_map() {
        // `return <Int> { ... }` dispara `Stmt::ReturnStatus`. El body
        // se parsea como cualquier `Expr` — acá un map literal con key
        // string explícita.
        match parse_one_stmt("return 401 {\"message\": \"no autorizado\"}") {
            Stmt::ReturnStatus { status, body, .. } => {
                assert!(matches!(status, Expr::Int(401, _)), "status: {:?}", status);
                let Some(b) = body else {
                    panic!("body esperado")
                };
                assert!(matches!(b, Expr::Map(..)), "body debería ser Map: {:?}", b);
            }
            other => panic!("se esperaba ReturnStatus, fue: {:?}", other),
        }
    }

    #[test]
    fn return_int_sin_body_sigue_como_return_normal() {
        // Sin `{...}` después del Int, sigue siendo Return de Int — no
        // dispara ReturnStatus. Esto preserva la sintaxis existente
        // (`return 42` en una fn que devuelve Int).
        assert_eq!(
            parse_one_stmt("return 204"),
            Stmt::Return(Expr::Int(204, Span::ZERO), Span::ZERO),
        );
    }

    #[test]
    fn return_status_solo_con_int_literal() {
        // Solo Int literales disparan `ReturnStatus`. Una expr más
        // compleja (`return x { ... }`) NO — sigue siendo Return de la
        // expr completa (que igual fallaría más adelante).
        match parse_one_stmt("return get_status() ") {
            Stmt::Return(Expr::Call { .. }, _) => {}
            other => panic!("se esperaba Return(Call), fue: {:?}", other),
        }
    }

    #[test]
    fn return_sin_expresion_devuelve_null() {
        // `return` solo (con newline al final). El parser lo modela como
        // `Stmt::Return(Expr::Null(_), Span::ZERO)`.
        assert_eq!(
            parse_one_stmt("return"),
            Stmt::Return(Expr::Null(Span::ZERO), Span::ZERO)
        );
    }

    #[test]
    fn return_sin_expresion_dentro_de_fn_body() {
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
            _ => panic!("se esperaba FnDef"),
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
    fn stmt_lleva_span_de_la_primera_linea() {
        // Stmt simple en línea 1, col 1 → span debería ser (1, 1).
        let stmt = parse_one_stmt("let x = 42");
        let span = stmt.span();
        assert_eq!(span.line, 1, "esperaba línea 1, fue {}", span.line);
        assert_eq!(span.column, 1, "esperaba col 1, fue {}", span.column);
    }

    #[test]
    fn stmt_lleva_span_de_linea_posterior() {
        // Stmts en líneas 2 y 3 — cada uno con su span.
        let src = "\n  let x = 1\nreturn x";
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        assert_eq!(program.len(), 2);
        let s0 = program[0].span();
        let s1 = program[1].span();
        assert_eq!(
            (s0.line, s0.column),
            (2, 3),
            "esperaba (2,3) para `let`, fue ({},{})",
            s0.line,
            s0.column
        );
        assert_eq!(
            (s1.line, s1.column),
            (3, 1),
            "esperaba (3,1) para `return`, fue ({},{})",
            s1.line,
            s1.column
        );
    }

    #[test]
    fn span_de_fn_def_apunta_al_fn() {
        let src = "  fn foo() => 1";
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let span = program[0].span();
        assert_eq!((span.line, span.column), (1, 3));
    }

    #[test]
    fn span_de_fn_decorada_apunta_al_decorator() {
        // El span de `Stmt::FnDef` decorada apunta al `@`, no al `fn`.
        let src = "@get(\"/\") fn handler() => 0";
        let tokens = tokenize(src).expect("lex OK");
        let program = parse(tokens).expect("parse OK");
        let span = program[0].span();
        assert_eq!(span.column, 1);
    }

    // ---- fin tests B.1 span ---------------------------------------

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
            other => panic!("se esperaba Stmt::While, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn while_with_break_inside() {
        let stmt = parse_one_stmt("while true { break }");
        match stmt {
            Stmt::While { body, .. } => {
                assert!(matches!(body[..], [Stmt::Break(_, _, _)]));
            }
            _ => panic!("se esperaba while"),
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
            _ => panic!("se esperaba Stmt::Loop"),
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

    // ---- Mini-tanda Xor ----

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
    fn xor_misma_precedencia_que_or_left_assoc() {
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
    fn xor_y_or_chain_libremente_misma_precedencia() {
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
    fn and_tiene_mayor_precedencia_que_xor() {
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
    fn and_tiene_mayor_precedencia_que_or() {
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
    fn comparacion_tiene_mayor_precedencia_que_and() {
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
        // `x == y` debe ser expr-stmt con BinOp(Eq), NO Assign.
        // Esto valida que el lookahead distingue Eq de EqEq.
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
                target: AssignTarget::Ident("x".into()),
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
        // let x =  (sin expresión después de '=')
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
        // No hay separador entre `x = 1` y `print(x)` en la misma línea.
        let err = parse_program_str("x = 1 print(x)").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    // -----------------------------------------------------------------------
    // Tests — fndef (paso 5)
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
                    varargs: false
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
                    varargs: false
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
                    varargs: false
                }],
                return_type: None,
                body: vec![
                    Stmt::Assign {
                        target: AssignTarget::Ident("x".into()),
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
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn fndef_no_params() {
        let stmt = parse_one_stmt("fn main() { return 0 }");
        match stmt {
            Stmt::FnDef { params, .. } => assert!(params.is_empty()),
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn fndef_empty_block_body() {
        let stmt = parse_one_stmt("fn noop() { }");
        match stmt {
            Stmt::FnDef { body, .. } => assert!(body.is_empty()),
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
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
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn fndef_async_arrow() {
        let stmt = parse_one_stmt("async fn double(n) => n * 2");
        match stmt {
            Stmt::FnDef { is_async, .. } => assert!(is_async),
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
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
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn fndef_missing_name_errors() {
        let err = parse_program_str("fn () { }").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn fndef_unclosed_block_errors() {
        // 'fn f() {' sin '}' al final.
        let err = parse_program_str("fn f() {\n  x = 1\n").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::MissingClosingBrace));
    }

    #[test]
    fn fndef_missing_body_marker_errors() {
        // Después de ')' o '-> Type' debe venir '{' o '=>'.
        let err = parse_program_str("fn f() return 1").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    // -----------------------------------------------------------------------
    // Tests — StrInterp (paso 6)
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
        // "{x}" — sin literales alrededor.
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
        // "\{nombre\}" → literal "{nombre}" sin interpolación.
        assert_eq!(
            parse_expr(r#""\{nombre\}""#).unwrap(),
            Expr::Str("{nombre}".into(), Span::ZERO),
        );
    }

    #[test]
    fn escaped_and_unescaped_braces_in_same_string() {
        // "\{ {x} \}" → literal "{ ", interpolación de x, literal " }"
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
        // "hola {name"  — falta '}'
        let err = parse_expr(r#""hola {name""#).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
    }

    #[test]
    fn lone_close_brace_errors() {
        // "hola }"  — '}' suelto sin '{' previo
        let err = parse_expr(r#""hola }""#).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
    }

    #[test]
    fn unclosed_interpolation_reporta_columna_del_brace_abierto() {
        // `"a{x"` — el `{` está en columna 3 (después de la comilla en col 1
        // y del 'a' en col 2). El error tiene que apuntar ahí, no a la
        // columna 1 del string.
        let tokens = crate::lexer::tokenize(r#""a{x""#).unwrap();
        let err = parse(tokens).unwrap_err();
        assert_eq!(err.column, 3);
    }

    #[test]
    fn error_en_subexpresion_de_interp_apunta_dentro_del_string() {
        // `"foo{1 +}"` — el `+}` (subexpresión inválida) debe reportarse
        // con columna apuntando dentro del bloque de interpolación,
        // no a la columna 1.
        let tokens = crate::lexer::tokenize(r#""foo{1 +}""#).unwrap();
        let err = parse(tokens).unwrap_err();
        // El string empieza en col 1, el contenido en col 2, el `{` en col 5.
        // La subexpresión empieza en col 6. Cualquier columna > 1 confirma
        // que la traducción está activa.
        assert!(
            err.column > 1,
            "se esperaba columna > 1, se obtuvo {} (msg: {})",
            err.column,
            err.message,
        );
    }

    #[test]
    fn invalid_subexpression_propagates_error() {
        // "{1 +}"  — subexpresión inválida
        let err = parse_expr(r#""{1 +}""#).unwrap_err();
        // El error puede ser UnexpectedToken (de la subexpresión).
        assert!(matches!(
            err.kind,
            ErrorKind::UnexpectedToken | ErrorKind::InvalidSyntax
        ));
    }

    // -----------------------------------------------------------------------
    // Tests — if / match / type (paso 7)
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
            other => panic!(
                "se esperaba Stmt::Expr(If, Span::ZERO), se obtuvo {:?}",
                other
            ),
        }
    }

    #[test]
    fn if_with_else() {
        let stmt = parse_one_stmt("if x { 1 } else { 2 }");
        match stmt {
            Stmt::Expr(Expr::If { else_: Some(e), .. }, _) => {
                assert_eq!(e.len(), 1);
            }
            other => panic!("se esperaba If con else, se obtuvo {:?}", other),
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
                // El else exterior contiene una sola stmt: un Expr::If anidado.
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
                    other => panic!("se esperaba if anidado, se obtuvo {:?}", other),
                }
            }
            other => panic!("se esperaba If, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn if_as_expression_in_assignment() {
        // status = if active { "on" } else { "off" }
        let stmt = parse_one_stmt(r#"status = if active { "on" } else { "off" }"#);
        match stmt {
            Stmt::Assign {
                target: AssignTarget::Ident(name),
                value: Expr::If { .. },
                ..
            } => {
                assert_eq!(name, "status");
            }
            other => panic!(
                "se esperaba Assign con If como valor, se obtuvo {:?}",
                other
            ),
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
            other => panic!("se esperaba If, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn match_with_wildcard_and_ident_patterns() {
        // match x { foo => 1, _ => 0 }
        let stmt = parse_one_stmt("match x { foo => 1, _ => 0 }");
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(arms.len(), 2);
                assert_eq!(arms[0].pattern, Pattern::Ident("foo".into()));
                assert_eq!(arms[1].pattern, Pattern::Wildcard);
            }
            other => panic!("se esperaba Match, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn match_with_ok_and_err_bindings() {
        // match result { Ok(u) => u, Err(e) => 0 }
        let stmt = parse_one_stmt("match result { Ok(u) => u, Err(e) => 0 }");
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(arms.len(), 2);
                assert_eq!(arms[0].pattern, Pattern::OkBinding("u".into()));
                assert_eq!(arms[1].pattern, Pattern::ErrBinding("e".into()));
            }
            other => panic!("se esperaba Match, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn match_with_ok_and_err_wildcards() {
        // `Ok(_)` y `Err(_)` parsean como wildcards dedicados, sin
        // ensuciar el scope con una var llamada `_`.
        let stmt = parse_one_stmt("match result { Ok(_) => 1, Err(_) => 0 }");
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(arms.len(), 2);
                assert_eq!(arms[0].pattern, Pattern::OkWildcard);
                assert_eq!(arms[1].pattern, Pattern::ErrWildcard);
            }
            other => panic!("se esperaba Match, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn match_with_newline_separated_arms() {
        let src = "match x {\n  foo => 1\n  bar => 2\n  _ => 0\n}";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => assert_eq!(arms.len(), 3),
            other => panic!("se esperaba Match, se obtuvo {:?}", other),
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
            other => panic!("se esperaba TypeDef, se obtuvo {:?}", other),
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
            other => panic!("se esperaba TypeDef, se obtuvo {:?}", other),
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
                // email es nullable con default null
                assert_eq!(fields[1].name, "email");
                assert!(fields[1].type_.is_nullable());
                assert_eq!(fields[1].default, Some(Expr::Null(Span::ZERO)));
                // active no es nullable pero tiene default true
                assert_eq!(fields[2].name, "active");
                assert!(!fields[2].type_.is_nullable());
                assert_eq!(fields[2].default, Some(Expr::Bool(true, Span::ZERO)));
            }
            other => panic!("se esperaba TypeDef, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn typedef_unclosed_errors() {
        let err = parse_program_str("type User { id: Int").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::MissingClosingBrace));
    }

    // -----------------------------------------------------------------------
    // Tests — decoradores sobre FnDef (Fase 4, paso 4.1)
    // -----------------------------------------------------------------------
    //
    // El parser no entiende qué hace cada decorator (eso lo decide el
    // evaluador). Acá validamos pura estructura: nombre, args, y que se
    // peguen al FnDef en el orden correcto.

    #[test]
    fn decorator_get_pega_decorator_al_fndef() {
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
            other => panic!("se esperaba FnDef con decorators, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn decorator_post_con_async_handler() {
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
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn decorator_put_y_delete_reconocidos_por_nombre() {
        // Nota sobre `/users/{id}`: el parser lo interpreta como
        // `StrInterp` porque `{id}` es la sintaxis de interpolación de
        // strings de Fitz. Para el runtime HTTP esto es una buena
        // noticia, no un bug: en 4.2, los `StrPart::Expr(Ident(...))`
        // del path se reconocen directamente como path params, sin
        // necesidad de un mini parser dedicado dentro del decorator.
        let put = parse_one_stmt("@put(\"/users/{id}\")\nasync fn upd(id: Int) -> User => user");
        let del = parse_one_stmt("@delete(\"/users\")\nasync fn del(id: Int) => 0");
        match put {
            Stmt::FnDef { decorators, .. } => {
                assert_eq!(decorators.len(), 1);
                assert_eq!(decorators[0].name, "put");
                // El path tiene `{id}` → llega como StrInterp.
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
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
        match del {
            Stmt::FnDef { decorators, .. } => {
                assert_eq!(decorators[0].name, "delete");
                // Sin path params: llega como Str pelado.
                assert_eq!(
                    decorators[0].args,
                    vec![Expr::Str("/users".into(), Span::ZERO)]
                );
            }
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn decorator_sin_args_admite_parens_vacios() {
        // `@server()` — paréntesis vacíos válidos por simetría con
        // llamadas a función.
        let stmt = parse_one_stmt("@server()\nfn config() => 0");
        match stmt {
            Stmt::FnDef { decorators, .. } => {
                assert_eq!(decorators.len(), 1);
                assert_eq!(decorators[0].name, "server");
                assert!(decorators[0].args.is_empty());
            }
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn decorator_admite_multiples_args_y_expresiones() {
        // `@server(8080, "0.0.0.0")` — args positionals con tipos
        // mezclados. El evaluador validará semántica; el parser solo
        // los guarda.
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
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn decorators_apilados_se_acumulan_en_orden() {
        // @get("/admin") + @auth("admin") apilados sobre la misma fn.
        // Cada uno con su propia línea.
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
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn decorator_sin_parens_parsea_con_args_vacios() {
        // Fase 9.z.2.a — paréntesis opcionales en decorators
        // (necesario para `@test fn ...`). `@get fn h() => 0` parsea
        // con `args = kwargs = vacíos`. La validación semántica de
        // que `@get` necesita un path la hace el evaluator, no el
        // parser.
        let stmt = parse_one_stmt("@get\nfn h() => 0");
        match stmt {
            Stmt::FnDef { decorators, .. } => {
                assert_eq!(decorators.len(), 1);
                assert_eq!(decorators[0].name, "get");
                assert!(decorators[0].args.is_empty());
                assert!(decorators[0].kwargs.is_empty());
            }
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn test_decorator_sin_parens_parsea() {
        // Caso canónico de 9.z.2.a: `@test fn nombre() { ... }` sin
        // paréntesis después de `@test`. Forma idiomática del spec.
        let stmt = parse_one_stmt("@test\nfn suma_funciona() { let x = 1 }");
        match stmt {
            Stmt::FnDef { decorators, .. } => {
                assert_eq!(decorators[0].name, "test");
                assert!(decorators[0].args.is_empty());
                assert!(decorators[0].kwargs.is_empty());
            }
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn decorator_sin_handler_errores() {
        // @get("/x") y nada después: el parser corta porque no hay fn.
        let err = parse_program_str("@get(\"/x\")").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn decorator_seguido_de_no_fn_errores() {
        // @get("/x") let x = 1  → error claro
        let err = parse_program_str("@get(\"/x\")\nlet x = 1").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn decorator_desconocido_no_es_error_de_parser() {
        // Cualquier `@nombre(args)` válido sintácticamente parsea.
        // Que `@patch` no esté implementado lo decide el evaluator.
        let stmt = parse_one_stmt("@patch(\"/x\")\nfn h() => 0");
        match stmt {
            Stmt::FnDef { decorators, .. } => {
                assert_eq!(decorators[0].name, "patch");
            }
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Tests — Fase 7, sub-paso 7.0 (kwargs en decoradores)
    // -----------------------------------------------------------------------

    #[test]
    fn decorator_sin_kwargs_deja_vector_vacio() {
        // Regresión: `@get("/x")` mantiene `kwargs = []`.
        let stmt = parse_one_stmt("@get(\"/x\")\nfn h() => 0");
        match stmt {
            Stmt::FnDef { decorators, .. } => {
                assert_eq!(decorators[0].name, "get");
                assert_eq!(decorators[0].args.len(), 1);
                assert!(decorators[0].kwargs.is_empty());
            }
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn decorator_kwarg_solo_separa_clave_y_valor() {
        // `@server(docs=false)` — un único kwarg, ningún positional.
        let stmt = parse_one_stmt("@server(docs=false)\nfn cfg() => 0");
        match stmt {
            Stmt::FnDef { decorators, .. } => {
                assert!(decorators[0].args.is_empty());
                assert_eq!(decorators[0].kwargs.len(), 1);
                assert_eq!(decorators[0].kwargs[0].0, "docs");
                assert_eq!(decorators[0].kwargs[0].1, Expr::Bool(false, Span::ZERO));
            }
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn decorator_mezcla_positional_y_kwargs_en_ese_orden() {
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
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn decorator_positional_despues_de_kwarg_es_error() {
        // `@get(a=1, "/x")` — kwarg primero, positional después: rechaza.
        let err = parse_program_str("@get(a=1, \"/x\")\nfn h() => 0").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
        assert!(
            err.message.contains("posicionales"),
            "esperaba mensaje sobre orden positional/kwarg, fue: {}",
            err.message
        );
    }

    #[test]
    fn decorator_kwarg_duplicado_es_error() {
        // `@server(host="a", host="b")` — mismo kwarg dos veces.
        let err = parse_program_str("@server(host=\"a\", host=\"b\")\nfn cfg() => 0").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
        assert!(
            err.message.contains("host"),
            "esperaba que el mensaje cite la clave duplicada, fue: {}",
            err.message
        );
    }

    #[test]
    fn decorator_eqeq_en_arg_no_se_confunde_con_kwarg() {
        // `@deco(a == b)` — un arg posicional `BinOp(Eq)`, NO un kwarg
        // con clave `a` y valor `b`. La diferencia la hace el lexer:
        // `==` es `Token::EqEq`, mientras que `=` es `Token::Eq`.
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
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Test integrador final del criterio de Fase 2 — AHORA con interpolación
    // -----------------------------------------------------------------------

    /// Criterio de éxito de Fase 2 completo a nivel parser. El AST
    /// resultante coincide exactamente con el que se construye a mano
    /// en `ast::tests::can_represent_phase2_success_program`.
    #[test]
    fn parses_phase2_success_program_end_to_end() {
        let src = "name = \"Fitz\"\nx = 10 + 5\nprint(\"Hola, {name}!\")\nfn double(n) => n * 2\nprint(double(x))";
        let program = parse_program_str(src).unwrap();
        assert_eq!(program.len(), 5);

        // 1. name = "Fitz"
        assert_eq!(
            program[0],
            Stmt::Assign {
                target: AssignTarget::Ident("name".into()),
                type_: None,
                value: Expr::Str("Fitz".into(), Span::ZERO),
                span: Span::ZERO
            }
        );

        // 2. x = 10 + 5
        assert_eq!(
            program[1],
            Stmt::Assign {
                target: AssignTarget::Ident("x".into()),
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
                    varargs: false
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
    // Tests — Listas, mapas, rangos, indexing (Fase 3, paso 1)
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
        // Listas multilínea — los newlines entre elementos se ignoran.
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
        // a..b+1 → a..(b+1) (range tiene menor precedencia que '+')
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
        // (range tiene mayor precedencia que '<')
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
        // 1..2..3 — no chaineable
        let err = parse_expr("1..2..3").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
    }

    #[test]
    fn range_with_negative_int() {
        // -3..3 — unary minus se aplica al primer extremo
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
                target: AssignTarget::Ident("xs".into()),
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
                target: AssignTarget::Ident("m".into()),
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
    // Tests — for loop (Fase 3, paso 1)
    // -----------------------------------------------------------------------

    #[test]
    fn for_loop_over_list() {
        // for x in xs { print(x) }
        let stmt = parse_one_stmt("for x in xs { print(x) }");
        assert_eq!(
            stmt,
            Stmt::For {
                var: Pattern::Ident("x".into()),
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
                assert_eq!(var, Pattern::Ident("i".into()));
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
            other => panic!("se esperaba For, se obtuvo {:?}", other),
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
            other => panic!("se esperaba For, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn for_loop_with_break_and_continue() {
        let src = "for i in 0..10 { if i == 5 { break } else { continue } }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::For { body, .. } => {
                // El body tiene una sola sentencia: un if/else con break/continue.
                assert_eq!(body.len(), 1);
            }
            other => panic!("se esperaba For, se obtuvo {:?}", other),
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
    // Tests — patrones de rango en match (Fase 3, paso 1)
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
            other => panic!("se esperaba Match, se obtuvo {:?}", other),
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
            other => panic!("se esperaba Match, se obtuvo {:?}", other),
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
            other => panic!("se esperaba Match, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn pattern_int_sin_dotdot_sigue_siendo_int() {
        // Sanity check: que el cambio para Pattern::Range no rompa Pattern::Int.
        let src = "match n { 42 => \"sí\", _ => \"no\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(arms[0].pattern, Pattern::Int(42));
            }
            other => panic!("se esperaba Match, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn pattern_range_con_float_es_error() {
        // 0..1.5 — el float como extremo no se soporta en patrones
        let src = "match n { 0..1.5 => \"x\", _ => \"y\" }";
        let err = parse_program_str(src).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
    }

    // -----------------------------------------------------------------------
    // Tests — Or-patterns (R.2.1, mini-fase R)
    // -----------------------------------------------------------------------

    #[test]
    fn or_pattern_dos_literales() {
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
            other => panic!("se esperaba Match, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn or_pattern_tres_strings() {
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
            other => panic!("se esperaba Match, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn or_pattern_un_solo_pat_sin_pipe_no_envuelve() {
        // Sanity: pattern simple sin `|` no se envuelve en Or.
        let src = "match n { 1 => \"x\", _ => \"y\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(arms[0].pattern, Pattern::Int(1));
            }
            other => panic!("se esperaba Match, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn or_pattern_mezcla_range_y_literal() {
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
            other => panic!("se esperaba Match, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn or_pattern_con_ok_err_wildcard() {
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
            other => panic!("se esperaba Match, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn or_pattern_con_binding_ident_es_error() {
        // match n { 1 | x => "x" } — `x` es Ident binding, vetado.
        let src = "match n { 1 | x => \"x\" }";
        let err = parse_program_str(src).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
        assert!(err.message.contains("or-patterns no admiten bindings"));
    }

    #[test]
    fn or_pattern_con_ok_binding_es_error() {
        // match r { Ok(x) | Err(_) => "x" } — `Ok(x)` binding, vetado.
        let src = "match r { Ok(x) | Err(_) => \"x\" }";
        let err = parse_program_str(src).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
    }

    // -----------------------------------------------------------------------
    // Tests — Guards en match (R.2.2)
    // -----------------------------------------------------------------------

    #[test]
    fn guard_simple_sobre_ident_pattern() {
        let src = "match n { x if x > 10 => \"grande\", _ => \"chico\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(arms[0].pattern, Pattern::Ident("x".into()));
                assert!(arms[0].guard.is_some());
                assert!(arms[1].guard.is_none());
            }
            other => panic!("se esperaba Match, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn guard_sobre_ok_binding() {
        let src = "match r { Ok(v) if v > 0 => \"pos\", _ => \"x\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert_eq!(arms[0].pattern, Pattern::OkBinding("v".into()));
                assert!(arms[0].guard.is_some());
            }
            other => panic!("se esperaba Match, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn guard_combinado_con_range_pattern() {
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
            other => panic!("se esperaba Match, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn guard_combinado_con_or_pattern() {
        let src = "match n { 1 | 2 | 3 if n > 1 => \"x\", _ => \"y\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert!(matches!(arms[0].pattern, Pattern::Or(_)));
                assert!(arms[0].guard.is_some());
            }
            other => panic!("se esperaba Match, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn guard_es_expresion_compleja() {
        // El guard puede ser una expresión booleana arbitraria.
        let src = "match n { x if x > 0 and x < 100 => \"ok\", _ => \"x\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }, _) => {
                assert!(arms[0].guard.is_some());
            }
            other => panic!("se esperaba Match, se obtuvo {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Tests — Operadores compuestos +=/-=/*=//= (R.2.3)
    // -----------------------------------------------------------------------

    #[test]
    fn compound_plus_eq_sobre_ident() {
        // `x += 5` debe desugar a `x = x + 5`.
        let src = "x += 5";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                target: AssignTarget::Ident(name),
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
                    other => panic!("se esperaba BinOp, fue {:?}", other),
                }
            }
            other => panic!("se esperaba Stmt::Assign, fue {:?}", other),
        }
    }

    #[test]
    fn compound_minus_eq_sobre_ident() {
        let src = "x -= 3";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                value: Expr::BinOp { op, .. },
                ..
            } => {
                assert_eq!(op, BinOpKind::Sub);
            }
            other => panic!("se esperaba Stmt::Assign con BinOp Sub, fue {:?}", other),
        }
    }

    #[test]
    fn compound_star_eq_sobre_ident() {
        let src = "x *= 7";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                value: Expr::BinOp { op, .. },
                ..
            } => {
                assert_eq!(op, BinOpKind::Mul);
            }
            other => panic!("se esperaba Stmt::Assign con BinOp Mul, fue {:?}", other),
        }
    }

    #[test]
    fn compound_slash_eq_sobre_ident() {
        let src = "x /= 2";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                value: Expr::BinOp { op, .. },
                ..
            } => {
                assert_eq!(op, BinOpKind::Div);
            }
            other => panic!("se esperaba Stmt::Assign con BinOp Div, fue {:?}", other),
        }
    }

    #[test]
    fn compound_plus_eq_sobre_field() {
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
            other => panic!("se esperaba Stmt::Assign Field, fue {:?}", other),
        }
    }

    #[test]
    fn compound_plus_eq_sobre_index() {
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
            other => panic!("se esperaba Stmt::Assign Index, fue {:?}", other),
        }
    }

    #[test]
    fn compound_rhs_expresion_completa() {
        // El RHS debe parsear como expresión completa, no solo literal.
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
                // right es `a + b * 2` también
                assert!(matches!(*right, Expr::BinOp { .. }));
            }
            other => panic!(
                "se esperaba Stmt::Assign con RHS compuesto, fue {:?}",
                other
            ),
        }
    }

    // -----------------------------------------------------------------------
    // Tests — Métodos custom sobre `type` (R.3, mini-fase R)
    // -----------------------------------------------------------------------

    #[test]
    fn type_def_con_solo_fields_sigue_funcionando() {
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
            other => panic!("se esperaba TypeDef, fue {:?}", other),
        }
    }

    #[test]
    fn type_def_con_un_metodo_simple() {
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
            other => panic!("se esperaba TypeDef, fue {:?}", other),
        }
    }

    #[test]
    fn type_def_con_metodo_con_params() {
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
            other => panic!("se esperaba TypeDef, fue {:?}", other),
        }
    }

    #[test]
    fn type_def_con_metodo_async() {
        let src = "type User {\n\
                       id: Int\n\
                       async fn fetch() -> Str { return \"...\" }\n\
                   }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::TypeDef { methods, .. } => {
                assert!(methods[0].is_async);
            }
            other => panic!("se esperaba TypeDef, fue {:?}", other),
        }
    }

    #[test]
    fn type_def_con_metodo_flecha() {
        // `fn greet() => "x"` se desugarea a body con Return.
        let src = "type User {\n\
                       fn name_str() -> Str => \"ada\"\n\
                   }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::TypeDef { methods, .. } => {
                assert_eq!(methods[0].body.len(), 1);
                assert!(matches!(methods[0].body[0], Stmt::Return(_, _)));
            }
            other => panic!("se esperaba TypeDef, fue {:?}", other),
        }
    }

    #[test]
    fn type_def_mezcla_fields_y_metodos() {
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
            other => panic!("se esperaba TypeDef, fue {:?}", other),
        }
    }

    #[test]
    fn type_def_con_metodo_sin_cuerpo_es_error() {
        // `fn nombre()` sin body es error (no admitimos abstract methods).
        let src = "type X { fn f() }";
        let err = parse_program_str(src).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    // -----------------------------------------------------------------------
    // Tests — Struct literals (Fase 3, paso 2)
    //
    // El parser reconoce `Nombre { campo: expr, ... }` como `Expr::StructLit`
    // adentro de un postfix de Ident. La ambigüedad con bloques se resuelve
    // con el flag `no_struct_literal`: en condiciones de if/while/for/match
    // los struct literals exigen paréntesis.
    // -----------------------------------------------------------------------

    #[test]
    fn struct_lit_simple_en_asignacion() {
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
            other => panic!("se esperaba Assign(StructLit), se obtuvo {:?}", other),
        }
    }

    #[test]
    fn struct_lit_vacio() {
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
            other => panic!("se esperaba StructLit vacío, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn struct_lit_con_trailing_comma() {
        let src = "let u = User { id: 1, name: \"x\", }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                value: Expr::StructLit { fields, .. },
                ..
            } => {
                assert_eq!(fields.len(), 2);
            }
            other => panic!("se esperaba StructLit, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn struct_lit_multilinea_con_newlines_entre_campos() {
        // Sin coma entre líneas — newline como separador.
        let src = "let u = User {\n    id: 1\n    name: \"x\"\n}";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                value: Expr::StructLit { fields, .. },
                ..
            } => {
                assert_eq!(fields.len(), 2);
            }
            other => panic!("se esperaba StructLit multilínea, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn struct_lit_anidado() {
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
                    other => panic!("se esperaba StructLit anidado, se obtuvo {:?}", other),
                }
            }
            other => panic!("se esperaba Assign(StructLit), se obtuvo {:?}", other),
        }
    }

    #[test]
    fn struct_lit_con_expresion_compleja_como_valor() {
        // El valor del campo puede ser cualquier expresión.
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
            other => panic!("se esperaba StructLit, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn struct_lit_como_argumento_de_funcion() {
        // Adentro de paréntesis no hay ambigüedad — el struct literal
        // se permite sin envolver.
        let src = "print(User { id: 1, name: \"x\" })";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Call { args, .. }, _) => {
                assert_eq!(args.len(), 1);
                assert!(matches!(args[0], Expr::StructLit { .. }));
            }
            other => panic!("se esperaba Call con StructLit arg, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn struct_lit_dentro_de_lista() {
        // Adentro de `[...]` cada item está delimitado por `,` o `]` —
        // sin ambigüedad con bloques.
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
            other => panic!("se esperaba List con StructLits, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn struct_lit_en_return() {
        let src = "fn make() => User { id: 1, name: \"x\" }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::FnDef { body, .. } => match &body[0] {
                Stmt::Return(Expr::StructLit { .. }, _) => {}
                other => panic!("se esperaba Return(StructLit), se obtuvo {:?}", other),
            },
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn struct_lit_como_indice_y_receptor_de_index() {
        // El struct literal puede aparecer adentro de `[...]` de indexing.
        let src = "let v = m[Key { id: 1 }]";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Assign {
                value: Expr::Index { index, .. },
                ..
            } => {
                assert!(matches!(*index, Expr::StructLit { .. }));
            }
            other => panic!("se esperaba Index con StructLit, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn while_con_struct_literal_sin_parens_da_error_con_hint() {
        // `while User { id: 1 } { body }` — el parser ve el `{` después
        // de `User` y, como estamos en condición, detecta que parece
        // struct literal y emite un error con hint para usar paréntesis.
        let src = "while User { id: 1 } { print(x) }";
        let err = parse_program_str(src).unwrap_err();
        let msg = err.message.to_lowercase();
        assert!(
            msg.contains("paréntesis") || msg.contains("parentesis"),
            "el error debería mencionar paréntesis, fue: {}",
            err.message
        );
    }

    #[test]
    fn if_con_struct_literal_sin_parens_da_error_con_hint() {
        let src = "if User { id: 1 } == other { print(x) }";
        let err = parse_program_str(src).unwrap_err();
        let msg = err.message.to_lowercase();
        assert!(
            msg.contains("paréntesis") || msg.contains("parentesis"),
            "el error debería mencionar paréntesis, fue: {}",
            err.message
        );
    }

    #[test]
    fn for_con_struct_literal_sin_parens_da_error_con_hint() {
        let src = "for u in User { id: 1 } { print(u) }";
        let err = parse_program_str(src).unwrap_err();
        let msg = err.message.to_lowercase();
        assert!(
            msg.contains("paréntesis") || msg.contains("parentesis"),
            "el error debería mencionar paréntesis, fue: {}",
            err.message
        );
    }

    #[test]
    fn match_con_struct_literal_sin_parens_da_error_con_hint() {
        let src = "match User { id: 1 } { _ => \"x\" }";
        let err = parse_program_str(src).unwrap_err();
        let msg = err.message.to_lowercase();
        assert!(
            msg.contains("paréntesis") || msg.contains("parentesis"),
            "el error debería mencionar paréntesis, fue: {}",
            err.message
        );
    }

    #[test]
    fn if_con_struct_literal_envuelto_en_parens_parsea() {
        // Con paréntesis sí: la condición ve un struct literal entero.
        let src = "if (User { id: 1 }) == other { print(x) }";
        let stmts = parse_program_str(src).expect("debería parsear con paréntesis");
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Stmt::Expr(Expr::If { condition, .. }, _) => match condition.as_ref() {
                Expr::BinOp { left, .. } => {
                    assert!(matches!(**left, Expr::StructLit { .. }));
                }
                other => panic!("se esperaba BinOp como condición, se obtuvo {:?}", other),
            },
            other => panic!("se esperaba If, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn while_con_ident_y_bloque_sin_struct_pattern_sigue_andando() {
        // `while x { print(x) }` — el cuerpo del bloque no tiene shape
        // de struct literal, así que el flag deja pasar el `{` para
        // que `parse_block` lo agarre.
        let src = "while x { print(x) }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::While {
                condition, body, ..
            } => {
                assert_eq!(condition, Expr::Ident("x".into(), Span::ZERO));
                assert_eq!(body.len(), 1);
            }
            other => panic!("se esperaba While, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn for_sobre_lista_de_struct_literals_parsea() {
        // Adentro de `[...]` los struct literals están permitidos
        // incluso cuando el `for` está en modo no_struct_literal.
        let src = "for u in [User { id: 1, name: \"a\" }] { print(u) }";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::For {
                var, iter, body, ..
            } => {
                assert_eq!(var, Pattern::Ident("u".into()));
                assert!(matches!(iter, Expr::List(_, _)));
                assert_eq!(body.len(), 1);
            }
            other => panic!("se esperaba For, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn if_con_typed_assignment_en_bloque_no_se_confunde_con_struct_literal() {
        // `if x { y: Int = 1 }` — el bloque tiene una asignación tipada,
        // que comparte shape inicial con un struct literal (`Ident :`).
        // El parser debe distinguir y dejar pasar el bloque sin error.
        let src = "if x { y: Int = 1 }";
        let stmts = parse_program_str(src).expect("debería parsear");
        assert_eq!(stmts.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Tests — Result + Ok/Err + ? (Fase 3, paso 3)
    // -----------------------------------------------------------------------

    #[test]
    fn ok_ctor_se_parsea_a_expr_ok() {
        let e = parse_expr("Ok(42)").unwrap();
        assert_eq!(e, Expr::Ok(Box::new(Expr::Int(42, Span::ZERO)), Span::ZERO));
    }

    #[test]
    fn err_ctor_se_parsea_a_expr_err() {
        let e = parse_expr(r#"Err("boom")"#).unwrap();
        assert_eq!(
            e,
            Expr::Err(Box::new(Expr::Str("boom".into(), Span::ZERO)), Span::ZERO)
        );
    }

    #[test]
    fn ok_con_expresion_compleja_adentro() {
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
    fn ok_sin_argumentos_es_error_de_aridad() {
        let err = parse_expr("Ok()").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
        assert!(err.message.contains("`Ok`") && err.message.contains("1 argumento"));
    }

    #[test]
    fn err_con_dos_argumentos_es_error_de_aridad() {
        let err = parse_expr("Err(1, 2)").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
        assert!(err.message.contains("`Err`"));
    }

    #[test]
    fn try_postfix_envuelve_expresion() {
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
    fn try_sobre_identificador() {
        // x? → Try(Ident("x"))
        let e = parse_expr("x?").unwrap();
        assert_eq!(
            e,
            Expr::Try(Box::new(Expr::Ident("x".into(), Span::ZERO)), Span::ZERO)
        );
    }

    #[test]
    fn try_se_encadena_con_field_access() {
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
    fn try_anidado_con_ok_y_err() {
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
    fn match_con_patrones_ok_y_err_parsea() {
        // Sanity: el parser de patrones ya soportaba Ok/Err; verificamos
        // que el conjunto entero (match + Ok/Err en valor) compone bien.
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
            assert_eq!(arms[0].pattern, Pattern::OkBinding("v".into()));
            assert_eq!(arms[1].pattern, Pattern::ErrBinding("e".into()));
        } else {
            panic!("se esperaba un match");
        }
    }

    // -----------------------------------------------------------------------
    // Tests — Módulos / import (Fase 3, paso 5)
    // -----------------------------------------------------------------------

    #[test]
    fn import_simple_se_parsea() {
        // `import utils` → Stmt::Import con alias None.
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
    fn import_punteado_acumula_segmentos() {
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
    fn import_sin_nombre_es_error() {
        let err = parse_program_str("import").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn import_path_terminado_en_punto_es_error() {
        // `import foo.` — falta el segmento siguiente.
        let err = parse_program_str("import foo.").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn from_import_un_nombre() {
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
    fn from_import_varios_nombres_separados_por_coma() {
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
    fn from_import_con_path_punteado() {
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
    fn from_import_acepta_trailing_comma() {
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

    // ---- Mini-tanda Mln — from foo import ( ... ) multi-línea ----

    #[test]
    fn mln_from_import_parens_single_line() {
        // `from utils import (a, b, c)` — paréntesis sin newlines.
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
    fn mln_from_import_parens_multi_linea_canonico() {
        // Forma idiomática Python: `(`/`)` rodeando una lista de
        // nombres separados por comas y newlines.
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
    fn mln_from_import_parens_con_aliases_mixtos() {
        // Aliases dentro de los paréntesis multi-línea funcionan
        // igual que en single-line.
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
    fn mln_from_import_parens_sin_trailing_comma() {
        // El último nombre antes del `)` no requiere coma.
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
    fn mln_from_import_parens_sin_cerrar_es_error() {
        let err = parse_program_str("from utils import (a, b\n").unwrap_err();
        assert!(
            err.message.contains("')'") || err.message.contains("import"),
            "esperaba mensaje sobre `)` o import, fue: {}",
            err.message
        );
    }

    #[test]
    fn from_sin_import_es_error() {
        // `from utils slugify` — falta la keyword `import`.
        let err = parse_program_str("from utils slugify").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn from_import_sin_nombres_es_error() {
        // `from utils import` — al menos un nombre obligatorio.
        let err = parse_program_str("from utils import").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    // PreF8.4: tests de aliases.

    #[test]
    fn import_con_alias_parsea_el_alias() {
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
    fn import_punteado_con_alias() {
        // `import sub.foo as f` — alias se aplica al binding completo.
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
    fn from_import_con_alias_simple() {
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
    fn from_import_alias_mixto_con_y_sin() {
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
    fn import_as_sin_ident_es_error() {
        // `import foo as` — falta el ident después de `as`.
        let err = parse_program_str("import foo as").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn from_import_as_sin_ident_es_error() {
        // `from foo import bar as` — falta el ident después de `as`.
        let err = parse_program_str("from foo import bar as").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    // -----------------------------------------------------------------------
    // Tests — TypeExpr en anotaciones (Fase 5, paso 5.1)
    //
    // Cubrimos los tres lugares donde el parser pide un tipo:
    //   - `let x: T = ...` (Stmt::Assign.type_)
    //   - `fn f(p: T) -> T` (Param.type_ y FnDef.return_type)
    //   - `type X { f: T }` (Field.type_)
    //
    // El alcance del paso 5.1 es estructura sintáctica: el parser
    // construye la TypeExpr correcta. Validación semántica (que el
    // nombre exista, que la aridad del genérico sea correcta, etc.)
    // queda para 5.2 — el type checker.
    // -----------------------------------------------------------------------

    /// Helper: extrae la `TypeExpr` de un `let x: T = 0` simple.
    fn parse_assign_type(src: &str) -> TypeExpr {
        match parse_one_stmt(src) {
            Stmt::Assign { type_: Some(t), .. } => t,
            other => panic!("se esperaba Stmt::Assign con tipo, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn type_expr_simple_se_parsea_como_named() {
        let t = parse_assign_type("let x: Int = 0");
        assert_eq!(t, TypeExpr::named("Int"));
    }

    #[test]
    fn type_expr_generico_un_argumento() {
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
    fn type_expr_generico_dos_argumentos() {
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
    fn type_expr_generico_anidado() {
        // Result<List<User>>  — dos `>` consecutivos al cerrar.
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
    fn type_expr_nullable_sobre_named() {
        // User?
        let t = parse_assign_type("let u: User? = null");
        assert_eq!(t, TypeExpr::Nullable(Box::new(TypeExpr::named("User"))),);
    }

    #[test]
    fn type_expr_nullable_sobre_generico() {
        // List<Int>?  — el `?` aplica al átomo entero, no al último arg.
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
    fn type_expr_nullable_adentro_de_generico() {
        // List<Int?>  — el `?` está adentro, no afuera.
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
    fn type_expr_en_param_y_return_de_fndef() {
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
            other => panic!("se esperaba FnDef, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn type_expr_en_field_de_typedef_con_nullable() {
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
            other => panic!("se esperaba TypeDef, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn type_expr_generico_vacio_es_error() {
        // `List<>` no debería parsear: se exige al menos un argumento.
        let err = parse_program_str("let xs: List<> = []").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn type_expr_funcion_simple() {
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
    fn type_expr_funcion_sin_params() {
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
    fn type_expr_funcion_multiples_params() {
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
    fn type_expr_funcion_anidada_como_param() {
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
    fn type_expr_funcion_sin_arrow_es_error() {
        // `Fn(Int)` sin `-> R` → error explícito del parser.
        let err = parse_program_str("let f: Fn(Int) = null").unwrap_err();
        assert!(err.message.contains("'->"));
    }

    #[test]
    fn type_expr_generico_sin_cerrar_es_error() {
        // Falta el `>` final.
        let err = parse_program_str("let xs: List<Int = []").unwrap_err();
        // El parser falla cuando intenta consumir `>` y se encuentra con `=`.
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn type_expr_anotacion_sin_nombre_es_error() {
        // `:` sin tipo después.
        let err = parse_program_str("let x: = 1").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn type_expr_display_round_trip_sobre_un_caso_complejo() {
        // El display debe reproducir la forma escrita en el fuente.
        let t = parse_assign_type("let m: Map<Str, Result<List<User>?>> = {}");
        assert_eq!(t.display_name(), "Map<Str, Result<List<User>?>>");
    }

    // ---------------------------------------------------------------------
    // Fase 9.0.1 — parse_with_recovery
    // ---------------------------------------------------------------------

    /// Helper que tokeniza y corre `parse_with_recovery`. Devuelve
    /// `(stmts, errors)`.
    fn parse_recovering(src: &str) -> (Program, Vec<FitzError>) {
        let tokens = tokenize(src).expect("la fuente debe tokenizar sin error");
        parse_with_recovery(tokens)
    }

    #[test]
    fn recovery_programa_valido_no_acumula_errores() {
        // Smoke: la API recovering produce el mismo AST que strict
        // sobre código sin errores, con `Vec<FitzError>` vacío.
        let src = "let x = 1\nlet y = 2\nprint(x + y)";
        let (stmts_rec, errors) = parse_recovering(src);
        assert!(errors.is_empty(), "no se esperaban errores: {:?}", errors);
        let stmts_strict = parse(tokenize(src).unwrap()).unwrap();
        assert_eq!(stmts_rec, stmts_strict);
    }

    #[test]
    fn recovery_stmt_roto_a_top_level_inserta_error_y_continua() {
        // El `1 +` deja un binop pendiente — falla. El parser sincroniza
        // hasta el próximo Newline y continúa con `let y = 2`, que debe
        // parsear OK.
        let src = "let x = 1 +\nlet y = 2";
        let (stmts, errors) = parse_recovering(src);
        assert_eq!(errors.len(), 1, "exactamente un error: {:?}", errors);
        assert_eq!(stmts.len(), 2);
        assert!(matches!(stmts[0], Stmt::Error(_)));
        assert!(matches!(
            stmts[1],
            Stmt::Assign {
                target: AssignTarget::Ident(ref n), ..
            } if n == "y"
        ));
    }

    #[test]
    fn recovery_dos_stmts_rotos_consecutivos_emiten_dos_errores() {
        // Dos líneas rotas: el parser debe acumular dos errores, no
        // perderse.
        let src = "let a = 1 +\nlet b = *\nlet c = 3";
        let (stmts, errors) = parse_recovering(src);
        assert_eq!(errors.len(), 2);
        assert_eq!(stmts.len(), 3);
        assert!(matches!(stmts[0], Stmt::Error(_)));
        assert!(matches!(stmts[1], Stmt::Error(_)));
        assert!(matches!(
            stmts[2],
            Stmt::Assign {
                target: AssignTarget::Ident(ref n), ..
            } if n == "c"
        ));
    }

    #[test]
    fn recovery_stmt_roto_dentro_de_bloque_inserta_error_y_sigue() {
        // El body del `if` tiene un stmt roto seguido de uno válido.
        let src = "if (x) {\n  let a = 1 +\n  let b = 2\n}";
        let (stmts, errors) = parse_recovering(src);
        assert_eq!(errors.len(), 1);
        assert_eq!(stmts.len(), 1);
        // El `if` es Stmt::Expr(Expr::If { ... }, _). Inspeccionamos su body.
        match &stmts[0] {
            Stmt::Expr(Expr::If { then, .. }, _) => {
                assert_eq!(then.len(), 2);
                assert!(matches!(then[0], Stmt::Error(_)));
                assert!(matches!(
                    then[1],
                    Stmt::Assign {
                        target: AssignTarget::Ident(ref n), ..
                    } if n == "b"
                ));
            }
            other => panic!("se esperaba Stmt::Expr(Expr::If), recibió {:?}", other),
        }
    }

    #[test]
    fn recovery_error_span_apunta_al_token_donde_empezo_el_stmt() {
        // El stmt roto arranca en la línea 1, col 1 (el `let`). El span
        // del `Stmt::Error` lo refleja para que el LSP lo subraye desde
        // el inicio del stmt y no desde el caracter raro.
        let src = "let x = +\nlet y = 2";
        let (stmts, _errors) = parse_recovering(src);
        match &stmts[0] {
            Stmt::Error(span) => {
                assert_eq!(span.line, 1);
                assert_eq!(span.column, 1);
            }
            other => panic!("se esperaba Stmt::Error, recibió {:?}", other),
        }
    }

    #[test]
    fn recovery_error_lleva_linea_y_columna_del_token_problematico() {
        // El error reportado debe apuntar al token donde se detectó el
        // problema (el `+` suelto), no al inicio del stmt — útil para
        // el LSP que subraya el squiggly.
        let src = "let x = +\nlet y = 2";
        let (_stmts, errors) = parse_recovering(src);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line, 1);
        // El `+` está en la columna 9.
        assert_eq!(errors[0].column, 9);
    }

    #[test]
    fn recovery_eof_inesperado_se_acumula_como_error() {
        // `let x =` deja una expresión pendiente al final del archivo.
        // El parser debe acumular el error y devolver lo que pudo
        // construir.
        let src = "let x =";
        let (stmts, errors) = parse_recovering(src);
        assert_eq!(errors.len(), 1);
        // El stmt roto va como Error.
        assert!(matches!(stmts.last(), Some(Stmt::Error(_))));
    }

    #[test]
    fn recovery_cota_de_errores_corta_la_acumulacion() {
        // Generamos un programa con más de MAX_RECOVERED_ERRORS líneas
        // rotas. Verificamos que la cota se respeta.
        let n = MAX_RECOVERED_ERRORS + 50;
        let lines: Vec<String> = (0..n).map(|_| "let a = +".to_string()).collect();
        let src = lines.join("\n");
        let (_stmts, errors) = parse_recovering(&src);
        assert_eq!(errors.len(), MAX_RECOVERED_ERRORS);
    }

    #[test]
    fn recovery_fn_con_body_roto_preserva_estructura() {
        // El body de `fn foo` tiene un stmt roto. Lo importante: el
        // FnDef sigue siendo FnDef (con body que contiene Stmt::Error),
        // no se descarta entero. El stmt que sigue al cierre del fn
        // también parsea OK.
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
            other => panic!("se esperaba Stmt::FnDef, recibió {:?}", other),
        }
        assert!(matches!(
            stmts[1],
            Stmt::Assign {
                target: AssignTarget::Ident(ref n), ..
            } if n == "b"
        ));
    }

    #[test]
    fn recovery_parse_strict_sigue_abortando_al_primer_error() {
        // Garantía clave: `parse()` strict NO cambia su comportamiento.
        // Sigue devolviendo `Err` al primer error. La CLI strict
        // (`fitz run`/`build`/`check`) sigue funcionando igual.
        let src = "let x = +\nlet y = 2";
        let err = parse(tokenize(src).unwrap()).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    // ---------------------------------------------------------------------
    // Mini-tanda C — list comprehensions.
    // ---------------------------------------------------------------------

    /// Extrae el valor de un `let x = <expr>` top-level y lo devuelve.
    /// Útil para tests que arman programas chicos y quieren inspeccionar
    /// el primer `Expr` parseado.
    fn parse_first_let_value(src: &str) -> Expr {
        let stmts = parse(tokenize(src).expect("tokenize")).expect("parse");
        match stmts.into_iter().next().expect("al menos un stmt") {
            Stmt::Assign { value, .. } => value,
            other => panic!("se esperaba Stmt::Assign, recibió {:?}", other),
        }
    }

    #[test]
    fn comprehension_parsea_caso_basico() {
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
                assert!(matches!(var, Pattern::Ident(ref n) if n == "x"));
                assert!(matches!(*iter, Expr::Ident(ref n, _) if n == "xs"));
                assert!(filter.is_none());
            }
            other => panic!("se esperaba ListComp, recibió {:?}", other),
        }
    }

    // Mini-tanda Up — tuple destructuring en list comprehension.
    #[test]
    fn up_comprehension_acepta_tuple_destructuring() {
        let v = parse_first_let_value("let ys = [a + b for (a, b) in pairs]");
        match v {
            Expr::ListComp { var, .. } => {
                if let Pattern::Tuple(subs) = var {
                    assert_eq!(subs.len(), 2);
                    assert!(matches!(subs[0], Pattern::Ident(ref n) if n == "a"));
                    assert!(matches!(subs[1], Pattern::Ident(ref n) if n == "b"));
                } else {
                    panic!("esperaba Pattern::Tuple, vio {:?}", var);
                }
            }
            other => panic!("se esperaba ListComp, recibió {:?}", other),
        }
    }

    #[test]
    fn comprehension_parsea_con_filter_inline() {
        let v = parse_first_let_value("let ys = [x for x in xs if x > 0]");
        match v {
            Expr::ListComp { filter, .. } => {
                assert!(filter.is_some(), "filter inline debe estar presente");
            }
            other => panic!("se esperaba ListComp, recibió {:?}", other),
        }
    }

    #[test]
    fn comprehension_parsea_sobre_range() {
        let v = parse_first_let_value("let ys = [x * 2 for x in 0..10]");
        match v {
            Expr::ListComp { iter, .. } => {
                assert!(matches!(*iter, Expr::Range { .. }));
            }
            other => panic!("se esperaba ListComp, recibió {:?}", other),
        }
    }

    #[test]
    fn lista_de_un_elemento_no_se_confunde_con_comprehension() {
        // `[42]` es una lista de un elemento, NO una comprehension.
        // El parser solo detecta comprehension si tras el primer expr
        // viene `for` (no `,` ni `]`).
        let v = parse_first_let_value("let xs = [42]");
        match v {
            Expr::List(items, _) => assert_eq!(items.len(), 1),
            other => panic!("se esperaba List, recibió {:?}", other),
        }
    }

    // ---------------------------------------------------------------
    // Mini-tanda Fm — format specs en interpolación.
    // ---------------------------------------------------------------

    fn extract_first_strinterp_spec(src: &str) -> Option<crate::ast::FormatSpec> {
        match parse_first_let_value(src) {
            Expr::StrInterp(parts, _) => parts.into_iter().find_map(|p| match p {
                StrPart::Expr(_, spec) => spec,
                _ => None,
            }),
            other => panic!("se esperaba StrInterp, recibió {:?}", other),
        }
    }

    #[test]
    fn format_spec_precision_float_se_parsea() {
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
    fn format_spec_align_right_con_width() {
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
    fn format_spec_grouping_y_precision_juntos() {
        // `,.2f` — coma para miles + 2 decimales.
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
    fn interpolation_sin_spec_sigue_funcionando_compat() {
        // Caso clásico sin `:` — el segundo campo de StrPart::Expr es None.
        let value = parse_first_let_value(r#"let r = "hola {name}""#);
        match value {
            Expr::StrInterp(parts, _) => {
                let has_none = parts.iter().any(|p| matches!(p, StrPart::Expr(_, None)));
                assert!(has_none, "esperaba StrPart::Expr(_, None) sin spec");
            }
            other => panic!("se esperaba StrInterp, recibió {:?}", other),
        }
    }

    // ---------------------------------------------------------------
    // Mini-tanda Md — for con Pattern en `var`.
    // ---------------------------------------------------------------

    #[test]
    fn for_con_tuple_pattern_parsea() {
        // `for (k, v) in m { ... }` con Pattern::Tuple de 2 idents.
        let stmt = parse_one_stmt("for (k, v) in m { print(k) }");
        match stmt {
            Stmt::For { var, .. } => match var {
                Pattern::Tuple(subs) => {
                    assert_eq!(subs.len(), 2);
                    assert!(matches!(subs[0], Pattern::Ident(ref n) if n == "k"));
                    assert!(matches!(subs[1], Pattern::Ident(ref n) if n == "v"));
                }
                other => panic!("esperaba Pattern::Tuple, dio {:?}", other),
            },
            other => panic!("esperaba Stmt::For, dio {:?}", other),
        }
    }

    #[test]
    fn for_con_wildcard_pattern_parsea() {
        // `for _ in 0..10 { ... }` con Pattern::Wildcard.
        let stmt = parse_one_stmt("for _ in 0..10 { print(\"x\") }");
        match stmt {
            Stmt::For { var, .. } => {
                assert!(matches!(var, Pattern::Wildcard));
            }
            other => panic!("esperaba Stmt::For, dio {:?}", other),
        }
    }

    #[test]
    fn for_con_ident_simple_sigue_funcionando() {
        // Regresión: `for x in xs` con Pattern::Ident.
        let stmt = parse_one_stmt("for x in xs { print(x) }");
        match stmt {
            Stmt::For { var, .. } => {
                assert_eq!(var, Pattern::Ident("x".into()));
            }
            other => panic!("esperaba Stmt::For, dio {:?}", other),
        }
    }

    // ---- Mini-tanda Fp — default params ----

    #[test]
    fn fp_param_con_default_int_se_parsea() {
        let stmt = parse_one_stmt("fn f(x: Int = 5) -> Int { return x }");
        match stmt {
            Stmt::FnDef { params, .. } => {
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].name, "x");
                assert!(params[0].default.is_some(), "esperaba default");
                if let Some(Expr::Int(5, _)) = params[0].default {
                } else {
                    panic!("esperaba default Int(5), dio {:?}", params[0].default);
                }
            }
            other => panic!("esperaba FnDef, dio {:?}", other),
        }
    }

    #[test]
    fn fp_param_default_str_se_parsea() {
        let stmt = parse_one_stmt("fn greet(name: Str = \"amigo\") -> Str { return name }");
        match stmt {
            Stmt::FnDef { params, .. } => {
                assert_eq!(params.len(), 1);
                match &params[0].default {
                    Some(Expr::Str(s, _)) => assert_eq!(s, "amigo"),
                    other => panic!("esperaba Str default, dio {:?}", other),
                }
            }
            other => panic!("esperaba FnDef, dio {:?}", other),
        }
    }

    #[test]
    fn fp_mezcla_required_y_default_se_parsea() {
        let stmt = parse_one_stmt("fn f(a: Int, b: Int = 10) -> Int { return a + b }");
        match stmt {
            Stmt::FnDef { params, .. } => {
                assert_eq!(params.len(), 2);
                assert!(params[0].default.is_none(), "a NO debe tener default");
                assert!(params[1].default.is_some(), "b SÍ debe tener default");
            }
            other => panic!("esperaba FnDef, dio {:?}", other),
        }
    }

    #[test]
    fn fp_required_despues_de_default_es_error() {
        // Regla Python: una vez que un param tiene default, todos los
        // siguientes también. `fn f(a = 1, b)` debe rechazarse.
        let result = parse_program_str("fn f(a: Int = 1, b: Int) -> Int { return a + b }");
        assert!(result.is_err(), "esperaba error, dio {:?}", result);
        let msg = result.unwrap_err().message;
        assert!(
            msg.contains("default") && msg.contains("b"),
            "mensaje esperado contener 'default' y 'b', fue: {}",
            msg
        );
    }

    #[test]
    fn fp_param_default_sin_tipo_se_parsea() {
        // `fn f(x = 5)` sin anotación de tipo. Gradual: default
        // sí, pero el tipo del param queda en Any.
        let stmt = parse_one_stmt("fn f(x = 5) { return x }");
        match stmt {
            Stmt::FnDef { params, .. } => {
                assert!(params[0].type_.is_none());
                assert!(params[0].default.is_some());
            }
            other => panic!("esperaba FnDef, dio {:?}", other),
        }
    }

    // ---- Mini-tanda Fp.2 — varargs ----

    #[test]
    fn fp2_param_varargs_se_parsea() {
        let stmt = parse_one_stmt("fn sum(...xs: Int) -> Int { return 0 }");
        match stmt {
            Stmt::FnDef { params, .. } => {
                assert_eq!(params.len(), 1);
                assert!(params[0].varargs, "esperaba varargs=true");
                assert_eq!(params[0].name, "xs");
            }
            other => panic!("esperaba FnDef, dio {:?}", other),
        }
    }

    #[test]
    fn fp2_varargs_solo_ultimo_es_error() {
        let result = parse_program_str("fn f(...xs: Int, ...ys: Int) -> Int { return 0 }");
        assert!(result.is_err(), "esperaba error por varargs duplicado");
    }

    #[test]
    fn fp2_param_despues_de_varargs_es_error() {
        let result = parse_program_str("fn f(...xs: Int, y: Int) -> Int { return 0 }");
        assert!(
            result.is_err(),
            "esperaba error por param después de varargs"
        );
    }

    #[test]
    fn fp2_varargs_con_default_es_error() {
        let result = parse_program_str("fn f(...xs: Int = 5) -> Int { return 0 }");
        assert!(result.is_err(), "esperaba error por varargs con default");
    }

    #[test]
    fn fp2_mezcla_required_y_varargs_se_parsea() {
        let stmt = parse_one_stmt("fn f(a: Str, ...xs: Int) -> Int { return 0 }");
        match stmt {
            Stmt::FnDef { params, .. } => {
                assert_eq!(params.len(), 2);
                assert!(!params[0].varargs);
                assert!(params[1].varargs);
            }
            other => panic!("esperaba FnDef, dio {:?}", other),
        }
    }

    // ---- Mini-tanda Fp.3 — named args ----

    #[test]
    fn fp3_call_con_named_arg_emite_named_arg() {
        let src = "let r = f(name: 1)";
        let program = parse_program_str(src).expect("parse OK");
        match &program[0] {
            Stmt::Assign { value, .. } => {
                if let Expr::Call { args, .. } = value {
                    assert_eq!(args.len(), 1);
                    if let Expr::NamedArg { name, .. } = &args[0] {
                        assert_eq!(name, "name");
                    } else {
                        panic!("esperaba NamedArg, dio {:?}", args[0]);
                    }
                } else {
                    panic!("esperaba Call, dio {:?}", value);
                }
            }
            other => panic!("esperaba Assign, dio {:?}", other),
        }
    }

    #[test]
    fn fp3_positional_despues_de_named_es_error() {
        let result = parse_program_str("let r = f(name: 1, 2)");
        assert!(result.is_err(), "esperaba error positional-tras-named");
    }

    // ---- Mini-tanda Sp.2 — return en match arm ----

    #[test]
    fn sp2_match_arm_con_return_se_parsea_como_stmt_return() {
        let src = "fn f(n: Int) -> Str {\n  match n {\n    0 => return \"zero\"\n    _ => \"other\"\n  }\n  return \"end\"\n}";
        let stmt = parse_one_stmt(src);
        if let Stmt::FnDef { body, .. } = stmt {
            // Buscar el Expr::Match adentro.
            if let Stmt::Expr(Expr::Match { arms, .. }, _) = &body[0] {
                // Arm 0: pattern Int(0) → Stmt::Return.
                assert_eq!(arms[0].body.len(), 1);
                assert!(matches!(arms[0].body[0], Stmt::Return(..)));
                // Arm 1: pattern Wildcard → Stmt::Expr("other").
                assert!(matches!(arms[1].body[0], Stmt::Expr(..)));
            } else {
                panic!("esperaba Stmt::Expr(Match), dio {:?}", body[0]);
            }
        } else {
            panic!("esperaba FnDef");
        }
    }

    #[test]
    fn sp2_match_arm_body_es_vec_stmt_de_1_para_expr_simple() {
        // El caso common: arm body de 1 stmt expr.
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
            panic!("esperaba Match");
        }
    }
}
