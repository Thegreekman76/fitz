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
    BinOpKind, Expr, Field, HttpMethod, MatchArm, Param, Pattern, Program, Stmt, StrPart,
    UnaryOpKind,
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
}

impl Parser {
    fn new(tokens: Vec<TokenWithPos>) -> Self {
        Self { tokens, pos: 0 }
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
    //   expression  → equality
    //   equality    → comparison ( ("==" | "!=") comparison )*
    //   comparison  → term       ( ("<" | ">" | "<=" | ">=") term )*
    //   term        → factor     ( ("+" | "-") factor )*
    //   factor      → unary      ( ("*" | "/") unary )*
    //   unary       → "-" unary  |  postfix
    //   postfix     → primary    ( "." Ident  |  "(" args ")" )*
    //   primary     → literal | Ident | "(" expression ")"
    //
    // Todos los binarios son izquierda-asociativos: el `while` itera
    // y va anidando al `left` cada vez. `expression` es el punto de
    // entrada desde cualquier regla externa que quiera parsear una
    // expresión completa.

    fn expression(&mut self) -> FitzResult<Expr> {
        self.logic_or()
    }

    /// `a or b or c` — `or` es izquierda-asociativo y tiene menor
    /// precedencia que `and`. Esto da `a and b or c` = `(a and b) or c`.
    fn logic_or(&mut self) -> FitzResult<Expr> {
        let mut left = self.logic_and()?;
        while matches!(self.peek(), Token::Or) {
            self.advance();
            let right = self.logic_and()?;
            left = Expr::BinOp {
                op: BinOpKind::Or,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    /// `a and b and c` — más alto que `or`, más bajo que `==`. Resultado:
    /// `a == 1 and b == 2` se parsea como `(a == 1) and (b == 2)`.
    fn logic_and(&mut self) -> FitzResult<Expr> {
        let mut left = self.equality()?;
        while matches!(self.peek(), Token::And) {
            self.advance();
            let right = self.equality()?;
            left = Expr::BinOp {
                op: BinOpKind::And,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn equality(&mut self) -> FitzResult<Expr> {
        let mut left = self.comparison()?;
        while let Some(op) = self.match_equality_op() {
            let right = self.comparison()?;
            left = Expr::BinOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn match_equality_op(&mut self) -> Option<BinOpKind> {
        let op = match self.peek() {
            Token::EqEq => BinOpKind::Eq,
            Token::NotEq => BinOpKind::NotEq,
            _ => return None,
        };
        self.advance();
        Some(op)
    }

    fn comparison(&mut self) -> FitzResult<Expr> {
        let mut left = self.term()?;
        while let Some(op) = self.match_comparison_op() {
            let right = self.term()?;
            left = Expr::BinOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn match_comparison_op(&mut self) -> Option<BinOpKind> {
        let op = match self.peek() {
            Token::Lt => BinOpKind::Lt,
            Token::LtEq => BinOpKind::LtEq,
            Token::Gt => BinOpKind::Gt,
            Token::GtEq => BinOpKind::GtEq,
            _ => return None,
        };
        self.advance();
        Some(op)
    }

    fn term(&mut self) -> FitzResult<Expr> {
        let mut left = self.factor()?;
        while let Some(op) = self.match_term_op() {
            let right = self.factor()?;
            left = Expr::BinOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn match_term_op(&mut self) -> Option<BinOpKind> {
        let op = match self.peek() {
            Token::Plus => BinOpKind::Add,
            Token::Minus => BinOpKind::Sub,
            _ => return None,
        };
        self.advance();
        Some(op)
    }

    fn factor(&mut self) -> FitzResult<Expr> {
        let mut left = self.unary()?;
        while let Some(op) = self.match_factor_op() {
            let right = self.unary()?;
            left = Expr::BinOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn match_factor_op(&mut self) -> Option<BinOpKind> {
        let op = match self.peek() {
            Token::Star => BinOpKind::Mul,
            Token::Slash => BinOpKind::Div,
            _ => return None,
        };
        self.advance();
        Some(op)
    }

    /// Unary prefijo. Por ahora solo `-x`. `--x` se parsea recursivo
    /// como `Neg(Neg(x))`.
    fn unary(&mut self) -> FitzResult<Expr> {
        if matches!(self.peek(), Token::Minus) {
            self.advance();
            let operand = self.unary()?;
            Ok(Expr::UnaryOp {
                op: UnaryOpKind::Neg,
                operand: Box::new(operand),
            })
        } else {
            self.postfix()
        }
    }

    /// Operadores postfix: acceso a campo (`.field`) y llamada (`(args)`).
    /// Iteran en loop porque se pueden encadenar: `user.profile.email`,
    /// `f(x).y` (esto último: ¡error! `f(x)` no es Ident, así que
    /// `.y` chocaría con el chequeo de método).
    ///
    /// Restricción documentada en roadmap 2.3: `Expr::Call` solo admite
    /// `name: String`. Por eso solo permitimos llamadas cuando el
    /// receptor inmediato es un `Ident`. Llamadas a métodos
    /// (`foo.bar()`) dan error explícito hasta que `Call` mute.
    fn postfix(&mut self) -> FitzResult<Expr> {
        let mut expr = self.primary()?;
        loop {
            match self.peek() {
                Token::Dot => {
                    self.advance();
                    let field = self.expect_ident(
                        "se esperaba nombre de campo después de '.'",
                    )?;
                    expr = Expr::Field {
                        object: Box::new(expr),
                        field,
                    };
                }
                Token::LParen => {
                    // Solo Ident soportado como receptor de llamada.
                    let name = match expr {
                        Expr::Ident(n) => n,
                        _ => {
                            return Err(self.error(
                                ErrorKind::UnexpectedToken,
                                "method calls (expr.method()) aún no soportados — \
                                 ver docs/roadmap.md sección 2.3",
                            ));
                        }
                    };
                    self.advance(); // consume '('
                    let args = self.parse_call_args()?;
                    expr = Expr::Call { name, args };
                }
                _ => break,
            }
        }
        Ok(expr)
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
    fn parse_program(&mut self) -> FitzResult<Program> {
        let mut stmts = Vec::new();
        loop {
            self.skip_newlines();
            if self.is_at_end() {
                break;
            }
            stmts.push(self.parse_stmt()?);
            self.consume_stmt_terminator()?;
        }
        Ok(stmts)
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
    fn parse_stmt(&mut self) -> FitzResult<Stmt> {
        match self.peek() {
            Token::Let => self.parse_assign_with_let(),
            Token::Return => self.parse_return(),
            Token::Fn | Token::Async => self.parse_fndef(),
            Token::Type => self.parse_typedef(),
            Token::At => self.parse_http_endpoint(),
            Token::Break => {
                self.advance();
                Ok(Stmt::Break)
            }
            Token::Continue => {
                self.advance();
                Ok(Stmt::Continue)
            }
            Token::While => self.parse_while(),
            Token::Loop => self.parse_loop(),
            Token::For => Err(self.error(
                ErrorKind::InvalidSyntax,
                "`for` requiere rangos o listas para iterar, que llegan en Fase 3",
            )),
            Token::Ident(_) => {
                // Lookahead: `x =` o `x :` arrancan asignación.
                // OJO: `x ==` es comparación (EqEq, no Eq), va por
                // expr-stmt. La comparación con `Eq` exacto en
                // `peek_at(1)` evita ese falso positivo.
                if matches!(self.peek_at(1), Token::Eq | Token::Colon) {
                    self.parse_assign_no_let()
                } else {
                    self.parse_expr_stmt()
                }
            }
            _ => self.parse_expr_stmt(),
        }
    }

    fn parse_assign_with_let(&mut self) -> FitzResult<Stmt> {
        self.expect(&Token::Let, "se esperaba 'let'")?;
        let name = self.expect_ident(
            "se esperaba nombre de variable después de 'let'",
        )?;
        let type_ = self.parse_optional_type_annotation()?;
        self.expect(&Token::Eq, "se esperaba '=' en la declaración")?;
        let value = self.expression()?;
        Ok(Stmt::Assign { name, type_, value })
    }

    fn parse_assign_no_let(&mut self) -> FitzResult<Stmt> {
        let name = self.expect_ident("se esperaba identificador")?;
        let type_ = self.parse_optional_type_annotation()?;
        self.expect(&Token::Eq, "se esperaba '=' en la asignación")?;
        let value = self.expression()?;
        Ok(Stmt::Assign { name, type_, value })
    }

    /// Anotación de tipo opcional: `: Ident`. Devuelve `Some(nombre)`
    /// si la había. Por ahora solo soporta nombres simples (no
    /// `List<Int>` ni `Str?`). Esto está implícito en que
    /// `Stmt::Assign.type_` es `Option<String>`.
    fn parse_optional_type_annotation(&mut self) -> FitzResult<Option<String>> {
        if self.eat(&Token::Colon) {
            let type_name = self.expect_ident(
                "se esperaba nombre de tipo después de ':'",
            )?;
            Ok(Some(type_name))
        } else {
            Ok(None)
        }
    }

    fn parse_return(&mut self) -> FitzResult<Stmt> {
        self.expect(&Token::Return, "se esperaba 'return'")?;
        // `return` sin valor devuelve null implícito. Detectamos los
        // terminadores válidos para una sentencia: fin de línea, cierre
        // de bloque o fin de archivo.
        let value = match self.peek() {
            Token::Newline | Token::RBrace | Token::EOF => Expr::Null,
            _ => self.expression()?,
        };
        Ok(Stmt::Return(value))
    }

    fn parse_expr_stmt(&mut self) -> FitzResult<Stmt> {
        let expr = self.expression()?;
        Ok(Stmt::Expr(expr))
    }

    // ---------- definición de función ----------
    //
    // Cuatro formas (combinables con `async` opcional):
    //   fn name(params) { body }
    //   fn name(params) -> Type { body }
    //   fn name(params) => expr
    //   fn name(params) -> Type => expr
    //
    // La forma de flecha se desugar a `body: vec![Stmt::Return(expr)]`
    // (decisión documentada en ast.rs).

    fn parse_fndef(&mut self) -> FitzResult<Stmt> {
        let is_async = self.eat(&Token::Async);
        self.expect(&Token::Fn, "se esperaba 'fn'")?;
        let name = self.expect_ident(
            "se esperaba nombre de función después de 'fn'",
        )?;
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
                let expr = self.expression()?;
                vec![Stmt::Return(expr)]
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
        })
    }

    /// Lista de parámetros, ya con '(' consumido. Termina consumiendo
    /// el ')'. Cada parámetro es `name` o `name: Type`. Acepta
    /// trailing comma y newlines dentro de los paréntesis.
    fn parse_params(&mut self) -> FitzResult<Vec<Param>> {
        let mut params = Vec::new();
        self.skip_newlines();
        if matches!(self.peek(), Token::RParen) {
            self.advance();
            return Ok(params);
        }
        loop {
            self.skip_newlines();
            let name = self.expect_ident("se esperaba nombre de parámetro")?;
            let type_ = self.parse_optional_type_annotation()?;
            params.push(Param { name, type_ });
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

    /// `-> Type` opcional. Solo soporta nombres de tipo simples por
    /// ahora (mismo límite que `parse_optional_type_annotation`).
    fn parse_optional_return_type(&mut self) -> FitzResult<Option<String>> {
        if self.eat(&Token::Arrow) {
            let type_name = self.expect_ident(
                "se esperaba nombre de tipo de retorno después de '->'",
            )?;
            Ok(Some(type_name))
        } else {
            Ok(None)
        }
    }

    /// Bloque `{ stmt; stmt; ... }`. Consume llaves de apertura y cierre.
    /// Acepta líneas en blanco entre sentencias y bloques vacíos.
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
            stmts.push(self.parse_stmt()?);
            self.consume_stmt_terminator()?;
        }
    }

    // ---------- loops ----------

    /// `while cond { body }`. Iteración condicional. La condición se evalúa
    /// antes de cada iteración; si es `false`, termina el loop.
    fn parse_while(&mut self) -> FitzResult<Stmt> {
        self.expect(&Token::While, "se esperaba 'while'")?;
        let condition = self.expression()?;
        let body = self.parse_block()?;
        Ok(Stmt::While { condition, body })
    }

    /// `loop { body }` — loop infinito. Solo se sale con `break` o `return`.
    fn parse_loop(&mut self) -> FitzResult<Stmt> {
        self.expect(&Token::Loop, "se esperaba 'loop'")?;
        let body = self.parse_block()?;
        Ok(Stmt::Loop { body })
    }

    // ---------- if / match / type ----------

    /// `if cond { ... }` o `if cond { ... } else { ... }` o
    /// `if cond { ... } else if ... { ... } else { ... }`.
    /// La cadena `else if` se desugar a un `else` que contiene una
    /// sola sentencia: el `if` siguiente envuelto en `Stmt::Expr`.
    fn parse_if_expr(&mut self) -> FitzResult<Expr> {
        self.expect(&Token::If, "se esperaba 'if'")?;
        let condition = self.expression()?;
        let then = self.parse_block()?;
        let else_ = if self.eat(&Token::Else) {
            if matches!(self.peek(), Token::If) {
                // `else if` — anidamos el siguiente if como un bloque
                // de una sola sentencia-expresión.
                let nested = self.parse_if_expr()?;
                Some(vec![Stmt::Expr(nested)])
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
        })
    }

    /// `match value { pat => expr, pat => expr, ... }`.
    /// Brazos separados por coma o newline (ambos aceptados).
    /// Limitaciones de los patrones, según el AST: solo `Ident`,
    /// `_` (wildcard), `Ok(x)`, `Err(e)`. Literales y rangos en
    /// patrones son deuda explícita.
    fn parse_match_expr(&mut self) -> FitzResult<Expr> {
        self.expect(&Token::Match, "se esperaba 'match'")?;
        let value = self.expression()?;
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
            let pattern = self.parse_pattern()?;
            self.expect(
                &Token::FatArrow,
                "se esperaba '=>' después del patrón",
            )?;
            let body = self.expression()?;
            arms.push(MatchArm { pattern, body });
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
    ///   Ok(name)    → OkBinding(name)      (bloqueado runtime hasta Fase 3)
    ///   Err(name)   → ErrBinding(name)     (bloqueado runtime hasta Fase 3)
    fn parse_pattern(&mut self) -> FitzResult<Pattern> {
        // Literales. Clonamos el peek antes de avanzar para no chocar con
        // el borrow checker.
        match self.peek().clone() {
            Token::Int(n) => {
                self.advance();
                return Ok(Pattern::Int(n));
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
                        return Ok(Pattern::Int(-n));
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
                let binding = self.expect_ident(
                    "se esperaba identificador para el binding de Ok/Err",
                )?;
                self.expect(
                    &Token::RParen,
                    "se esperaba ')' al final del patrón Ok/Err",
                )?;
                return Ok(if is_ok {
                    Pattern::OkBinding(binding)
                } else {
                    Pattern::ErrBinding(binding)
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

    /// `type Name { field: Type [?] [= default], ... }`.
    /// Separador entre campos: coma o newline (ambos aceptados).
    /// El tipo del campo es un nombre simple (mismo límite que en
    /// `Stmt::Assign.type_`).
    fn parse_typedef(&mut self) -> FitzResult<Stmt> {
        self.expect(&Token::Type, "se esperaba 'type'")?;
        let name = self.expect_ident("se esperaba nombre del tipo")?;
        self.expect(
            &Token::LBrace,
            "se esperaba '{' después del nombre del tipo",
        )?;
        let mut fields: Vec<Field> = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek(), Token::RBrace) {
                self.advance();
                return Ok(Stmt::TypeDef { name, fields });
            }
            if self.is_at_end() {
                return Err(self.error(
                    ErrorKind::MissingClosingBrace,
                    "se esperaba '}' para cerrar 'type'",
                ));
            }
            let field_name = self.expect_ident("se esperaba nombre de campo")?;
            self.expect(
                &Token::Colon,
                "se esperaba ':' después del nombre del campo",
            )?;
            let type_name = self.expect_ident("se esperaba tipo del campo")?;
            let nullable = self.eat(&Token::Question);
            let default = if self.eat(&Token::Eq) {
                Some(self.expression()?)
            } else {
                None
            };
            fields.push(Field {
                name: field_name,
                type_: type_name,
                nullable,
                default,
            });
            // Separador opcional: coma. Newline se consume en la
            // próxima iteración por skip_newlines.
            if matches!(self.peek(), Token::Comma) {
                self.advance();
            }
        }
    }

    // ---------- decoradores HTTP ----------
    //
    // Forma:
    //   @get("/path")
    //   [async] fn handler(...) [-> Type] { ... }
    //
    // Producimos `Stmt::HttpEndpoint { method, path, handler }` donde
    // `handler` envuelve un `Stmt::FnDef`. El AST nota que esto cambia
    // en Fase 4 a un esquema genérico de decoradores; hasta entonces
    // solo soportamos los 4 verbos básicos.

    fn parse_http_endpoint(&mut self) -> FitzResult<Stmt> {
        self.expect(&Token::At, "se esperaba '@'")?;
        // El nombre del decorador llega como Ident (get/post/put/delete).
        let (deco_line, deco_col) = self.current_pos();
        let method_name = self.expect_ident(
            "se esperaba nombre de decorador HTTP después de '@'",
        )?;
        let method = match method_name.as_str() {
            "get" => HttpMethod::Get,
            "post" => HttpMethod::Post,
            "put" => HttpMethod::Put,
            "delete" => HttpMethod::Delete,
            other => {
                return Err(FitzError::new(
                    ErrorKind::UnexpectedToken,
                    deco_line,
                    deco_col,
                    format!(
                        "decorador HTTP desconocido: '@{}' — esperaba get/post/put/delete",
                        other
                    ),
                ));
            }
        };

        self.expect(
            &Token::LParen,
            "se esperaba '(' después del decorador HTTP",
        )?;
        // La ruta tiene que ser un string literal (no una expresión).
        let path_tok = self.advance();
        let path = match path_tok.token {
            Token::Str(s) => s,
            other => {
                return Err(FitzError::new(
                    ErrorKind::UnexpectedToken,
                    path_tok.line,
                    path_tok.column,
                    format!(
                        "se esperaba una ruta string como '/users', se encontró '{:?}'",
                        other
                    ),
                ));
            }
        };
        self.expect(
            &Token::RParen,
            "se esperaba ')' al cerrar el decorador HTTP",
        )?;

        // Permitimos un newline entre el decorador y la fn def, que
        // es la forma canónica de escribirlos.
        self.skip_newlines();

        // El handler debe ser una FnDef (con `async` opcional). Si el
        // usuario pone otra cosa, parse_fndef da error claro.
        if !matches!(self.peek(), Token::Fn | Token::Async) {
            return Err(self.error(
                ErrorKind::UnexpectedToken,
                "después de un decorador HTTP debe venir una definición de función",
            ));
        }
        let handler = self.parse_fndef()?;

        Ok(Stmt::HttpEndpoint {
            method,
            path,
            handler: Box::new(handler),
        })
    }

    /// Parsea los argumentos de una llamada, ya con '(' consumido.
    /// Termina consumiendo el ')'. Acepta lista vacía, coma trailing,
    /// y newlines entre elementos (útil para llamadas multilínea).
    fn parse_call_args(&mut self) -> FitzResult<Vec<Expr>> {
        let mut args = Vec::new();
        self.skip_newlines();
        // Caso vacío: f()
        if matches!(self.peek(), Token::RParen) {
            self.advance();
            return Ok(args);
        }
        loop {
            self.skip_newlines();
            args.push(self.expression()?);
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
        self.expect(
            &Token::RParen,
            "se esperaba ')' para cerrar la llamada",
        )?;
        Ok(args)
    }

    /// Expresión "hoja": literal, identificador, paréntesis, `if`
    /// o `match`. Acá termina la recursión hacia abajo en la escalera.
    fn primary(&mut self) -> FitzResult<Expr> {
        // `if` y `match` son expresiones — las manejamos antes de
        // consumir el token para que sus parsers lo hagan.
        match self.peek() {
            Token::If => return self.parse_if_expr(),
            Token::Match => return self.parse_match_expr(),
            _ => {}
        }
        let tok = self.advance();
        match tok.token {
            Token::Int(n) => Ok(Expr::Int(n)),
            Token::Float(n) => Ok(Expr::Float(n)),
            // Procesamos el contenido crudo del string para detectar
            // interpolaciones `{...}` y desescapar `\{` / `\}`. Si no
            // hay interpolaciones, devuelve `Expr::Str`; si hay,
            // devuelve `Expr::StrInterp`.
            Token::Str(s) => build_string_expr(&s, tok.line, tok.column),
            Token::True => Ok(Expr::Bool(true)),
            Token::False => Ok(Expr::Bool(false)),
            Token::Null => Ok(Expr::Null),
            Token::Ident(name) => Ok(Expr::Ident(name)),
            Token::LParen => {
                let expr = self.expression()?;
                self.expect(
                    &Token::RParen,
                    "se esperaba ')' para cerrar el paréntesis",
                )?;
                Ok(expr)
            }
            other => Err(FitzError::new(
                ErrorKind::UnexpectedToken,
                tok.line,
                tok.column,
                format!("Se esperaba una expresión, se encontró '{:?}'", other),
            )),
        }
    }
}

/// Entrada pública del parser. Convierte tokens en un `Program`.
pub fn parse(tokens: Vec<TokenWithPos>) -> FitzResult<Program> {
    let mut parser = Parser::new(tokens);
    parser.parse_program()
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
            let expr_src: String = chars[expr_start..i].iter().collect();

            // La subexpresión empieza un char después del `{` en el source.
            let sub_col_base = interp_col + 1;

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
            parts.push(StrPart::Expr(expr));
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
    let has_interp = parts.iter().any(|p| matches!(p, StrPart::Expr(_)));
    if has_interp {
        Ok(Expr::StrInterp(parts))
    } else {
        let combined: String = parts
            .into_iter()
            .map(|p| match p {
                StrPart::Lit(s) => s,
                StrPart::Expr(_) => unreachable!(),
            })
            .collect();
        Ok(Expr::Str(combined))
    }
}

// ---------------------------------------------------------------------------
// Tests — helpers del Parser
// ---------------------------------------------------------------------------

#[cfg(test)]
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
    // Tests — expresiones (paso 2: escalera de precedencia)
    // -----------------------------------------------------------------------

    /// Helper: parsea una sola expresión desde el código fuente.
    fn parse_expr(src: &str) -> FitzResult<Expr> {
        let mut p = parser(src);
        p.expression()
    }

    #[test]
    fn primary_literals() {
        assert_eq!(parse_expr("42").unwrap(), Expr::Int(42));
        assert_eq!(parse_expr("3.14").unwrap(), Expr::Float(3.14));
        assert_eq!(
            parse_expr(r#""hola""#).unwrap(),
            Expr::Str("hola".into())
        );
        assert_eq!(parse_expr("true").unwrap(), Expr::Bool(true));
        assert_eq!(parse_expr("false").unwrap(), Expr::Bool(false));
        assert_eq!(parse_expr("null").unwrap(), Expr::Null);
    }

    #[test]
    fn primary_identifier() {
        assert_eq!(parse_expr("user").unwrap(), Expr::Ident("user".into()));
    }

    #[test]
    fn primary_parens_pass_through_without_node() {
        // (42) parsea como Int(42) — los paréntesis no agregan nodo
        // al AST, solo controlan precedencia.
        assert_eq!(parse_expr("(42)").unwrap(), Expr::Int(42));
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
                left: Box::new(Expr::Int(1)),
                right: Box::new(Expr::Int(2)),
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
                    left: Box::new(Expr::Int(1)),
                    right: Box::new(Expr::Int(2)),
                }),
                right: Box::new(Expr::Int(3)),
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
                left: Box::new(Expr::Int(1)),
                right: Box::new(Expr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(Expr::Int(2)),
                    right: Box::new(Expr::Int(3)),
                }),
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
                    left: Box::new(Expr::Int(1)),
                    right: Box::new(Expr::Int(2)),
                }),
                right: Box::new(Expr::Int(3)),
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
                    left: Box::new(Expr::Int(1)),
                    right: Box::new(Expr::Int(2)),
                }),
                right: Box::new(Expr::Int(5)),
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
                    left: Box::new(Expr::Int(1)),
                    right: Box::new(Expr::Int(2)),
                }),
                right: Box::new(Expr::Bool(true)),
            }
        );
    }

    #[test]
    fn unary_neg_wraps_operand() {
        assert_eq!(
            parse_expr("-5").unwrap(),
            Expr::UnaryOp {
                op: UnaryOpKind::Neg,
                operand: Box::new(Expr::Int(5)),
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
                    operand: Box::new(Expr::Int(5)),
                }),
                right: Box::new(Expr::Int(3)),
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
                    operand: Box::new(Expr::Ident("x".into())),
                }),
            }
        );
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
                    left: Box::new(Expr::Int(1)),
                    right: Box::new(Expr::Int(2)),
                }),
            }
        );
    }

    #[test]
    fn not_equal_operator() {
        assert_eq!(
            parse_expr("x != y").unwrap(),
            Expr::BinOp {
                op: BinOpKind::NotEq,
                left: Box::new(Expr::Ident("x".into())),
                right: Box::new(Expr::Ident("y".into())),
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
                object: Box::new(Expr::Ident("user".into())),
                field: "name".into(),
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
                    object: Box::new(Expr::Ident("user".into())),
                    field: "profile".into(),
                }),
                field: "email".into(),
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
                name: "hello".into(),
                args: vec![],
            }
        );
    }

    #[test]
    fn call_single_arg() {
        assert_eq!(
            parse_expr("print(42)").unwrap(),
            Expr::Call {
                name: "print".into(),
                args: vec![Expr::Int(42)],
            }
        );
    }

    #[test]
    fn call_multiple_args() {
        assert_eq!(
            parse_expr("sum(1, 2, 3)").unwrap(),
            Expr::Call {
                name: "sum".into(),
                args: vec![Expr::Int(1), Expr::Int(2), Expr::Int(3)],
            }
        );
    }

    #[test]
    fn call_with_trailing_comma() {
        // Coma trailing válida — útil para diffs limpios.
        assert_eq!(
            parse_expr("sum(1, 2,)").unwrap(),
            Expr::Call {
                name: "sum".into(),
                args: vec![Expr::Int(1), Expr::Int(2)],
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
                name: "sum".into(),
                args: vec![Expr::Int(1), Expr::Int(2), Expr::Int(3)],
            }
        );
    }

    #[test]
    fn call_with_complex_arg_expression() {
        // print(1 + 2 * 3)
        assert_eq!(
            parse_expr("print(1 + 2 * 3)").unwrap(),
            Expr::Call {
                name: "print".into(),
                args: vec![Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Int(1)),
                    right: Box::new(Expr::BinOp {
                        op: BinOpKind::Mul,
                        left: Box::new(Expr::Int(2)),
                        right: Box::new(Expr::Int(3)),
                    }),
                }],
            }
        );
    }

    #[test]
    fn nested_call() {
        // print(double(x))
        assert_eq!(
            parse_expr("print(double(x))").unwrap(),
            Expr::Call {
                name: "print".into(),
                args: vec![Expr::Call {
                    name: "double".into(),
                    args: vec![Expr::Ident("x".into())],
                }],
            }
        );
    }

    #[test]
    fn call_unclosed_paren_errors() {
        let err = parse_expr("f(1, 2").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn method_call_errors_explicitly() {
        // foo.bar() — deuda explícita: Call solo admite name simple.
        let err = parse_expr("foo.bar()").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
        assert!(err.message.contains("method"));
    }

    #[test]
    fn call_combines_with_arithmetic_precedence() {
        // 1 + f(2) * 3 → 1 + (f(2) * 3)
        assert_eq!(
            parse_expr("1 + f(2) * 3").unwrap(),
            Expr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(Expr::Int(1)),
                right: Box::new(Expr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(Expr::Call {
                        name: "f".into(),
                        args: vec![Expr::Int(2)],
                    }),
                    right: Box::new(Expr::Int(3)),
                }),
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
                    object: Box::new(Expr::Ident("foo".into())),
                    field: "bar".into(),
                }),
            }
        );
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
                name: "x".into(),
                type_: None,
                value: Expr::Int(42),
            }
        );
    }

    #[test]
    fn assign_with_let_and_type() {
        assert_eq!(
            parse_one_stmt("let x: Int = 42"),
            Stmt::Assign {
                name: "x".into(),
                type_: Some("Int".into()),
                value: Expr::Int(42),
            }
        );
    }

    #[test]
    fn assign_without_let_no_type() {
        assert_eq!(
            parse_one_stmt("x = 42"),
            Stmt::Assign {
                name: "x".into(),
                type_: None,
                value: Expr::Int(42),
            }
        );
    }

    #[test]
    fn assign_without_let_with_type() {
        assert_eq!(
            parse_one_stmt("name: Str = \"Fitz\""),
            Stmt::Assign {
                name: "name".into(),
                type_: Some("Str".into()),
                value: Expr::Str("Fitz".into()),
            }
        );
    }

    #[test]
    fn assign_with_complex_expression() {
        // x = 10 + 5
        assert_eq!(
            parse_one_stmt("x = 10 + 5"),
            Stmt::Assign {
                name: "x".into(),
                type_: None,
                value: Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Int(10)),
                    right: Box::new(Expr::Int(5)),
                },
            }
        );
    }

    #[test]
    fn return_with_expression() {
        assert_eq!(
            parse_one_stmt("return 42"),
            Stmt::Return(Expr::Int(42)),
        );
    }

    #[test]
    fn return_with_complex_expression() {
        assert_eq!(
            parse_one_stmt("return x + 1"),
            Stmt::Return(Expr::BinOp {
                op: BinOpKind::Add,
                left: Box::new(Expr::Ident("x".into())),
                right: Box::new(Expr::Int(1)),
            }),
        );
    }

    #[test]
    fn return_sin_expresion_devuelve_null() {
        // `return` solo (con newline al final). El parser lo modela como
        // `Stmt::Return(Expr::Null)`.
        assert_eq!(parse_one_stmt("return"), Stmt::Return(Expr::Null));
    }

    #[test]
    fn return_sin_expresion_dentro_de_fn_body() {
        // fn early_exit() { return }
        let src = "fn early_exit() { return }";
        let tokens = crate::lexer::tokenize(src).unwrap();
        let program = parse(tokens).unwrap();
        match &program[0] {
            Stmt::FnDef { body, .. } => {
                assert_eq!(body, &vec![Stmt::Return(Expr::Null)]);
            }
            _ => panic!("se esperaba FnDef"),
        }
    }

    #[test]
    fn expression_statement_with_call() {
        assert_eq!(
            parse_one_stmt("print(x)"),
            Stmt::Expr(Expr::Call {
                name: "print".into(),
                args: vec![Expr::Ident("x".into())],
            }),
        );
    }

    #[test]
    fn break_statement() {
        assert_eq!(parse_one_stmt("break"), Stmt::Break);
    }

    #[test]
    fn continue_statement() {
        assert_eq!(parse_one_stmt("continue"), Stmt::Continue);
    }

    #[test]
    fn while_basic_parses() {
        let stmt = parse_one_stmt("while x < 10 { x = x + 1 }");
        match stmt {
            Stmt::While { condition, body } => {
                assert!(matches!(condition, Expr::BinOp { op: BinOpKind::Lt, .. }));
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
                assert_eq!(body, vec![Stmt::Break]);
            }
            _ => panic!("se esperaba while"),
        }
    }

    #[test]
    fn loop_basic_parses() {
        let stmt = parse_one_stmt("loop { x = 1 }");
        match stmt {
            Stmt::Loop { body } => {
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
            Stmt::Expr(Expr::BinOp {
                op: BinOpKind::And,
                left: Box::new(Expr::Ident("x".into())),
                right: Box::new(Expr::Ident("y".into())),
            }),
        );
    }

    #[test]
    fn or_basic_parses() {
        assert_eq!(
            parse_one_stmt("x or y"),
            Stmt::Expr(Expr::BinOp {
                op: BinOpKind::Or,
                left: Box::new(Expr::Ident("x".into())),
                right: Box::new(Expr::Ident("y".into())),
            }),
        );
    }

    #[test]
    fn and_tiene_mayor_precedencia_que_or() {
        // `a and b or c` → `(a and b) or c`
        let stmt = parse_one_stmt("a and b or c");
        let expected = Stmt::Expr(Expr::BinOp {
            op: BinOpKind::Or,
            left: Box::new(Expr::BinOp {
                op: BinOpKind::And,
                left: Box::new(Expr::Ident("a".into())),
                right: Box::new(Expr::Ident("b".into())),
            }),
            right: Box::new(Expr::Ident("c".into())),
        });
        assert_eq!(stmt, expected);
    }

    #[test]
    fn comparacion_tiene_mayor_precedencia_que_and() {
        // `a > 0 and a < 10` → `(a > 0) and (a < 10)`
        let stmt = parse_one_stmt("a > 0 and a < 10");
        let expected = Stmt::Expr(Expr::BinOp {
            op: BinOpKind::And,
            left: Box::new(Expr::BinOp {
                op: BinOpKind::Gt,
                left: Box::new(Expr::Ident("a".into())),
                right: Box::new(Expr::Int(0)),
            }),
            right: Box::new(Expr::BinOp {
                op: BinOpKind::Lt,
                left: Box::new(Expr::Ident("a".into())),
                right: Box::new(Expr::Int(10)),
            }),
        });
        assert_eq!(stmt, expected);
    }

    #[test]
    fn for_emite_error_explicito_de_deuda() {
        // `for` no está implementado aún (espera Fase 3).
        let src = "for x in lista { print(x) }";
        let tokens = crate::lexer::tokenize(src).unwrap();
        let err = parse(tokens).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::InvalidSyntax));
        assert!(err.message.contains("Fase 3"));
    }

    #[test]
    fn equality_in_expr_stmt_is_not_assignment() {
        // `x == y` debe ser expr-stmt con BinOp(Eq), NO Assign.
        // Esto valida que el lookahead distingue Eq de EqEq.
        assert_eq!(
            parse_one_stmt("x == y"),
            Stmt::Expr(Expr::BinOp {
                op: BinOpKind::Eq,
                left: Box::new(Expr::Ident("x".into())),
                right: Box::new(Expr::Ident("y".into())),
            }),
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
                name: "x".into(),
                type_: None,
                value: Expr::Int(1),
            }
        );
        assert_eq!(
            program[2],
            Stmt::Expr(Expr::Call {
                name: "print".into(),
                args: vec![Expr::Ident("x".into())],
            })
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
                params: vec![Param { name: "n".into(), type_: None }],
                return_type: None,
                body: vec![Stmt::Return(Expr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(Expr::Ident("n".into())),
                    right: Box::new(Expr::Int(2)),
                })],
                is_async: false,
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
                    type_: Some("Int".into()),
                }],
                return_type: Some("Int".into()),
                body: vec![Stmt::Return(Expr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(Expr::Ident("n".into())),
                    right: Box::new(Expr::Int(2)),
                })],
                is_async: false,
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
                params: vec![Param { name: "name".into(), type_: None }],
                return_type: None,
                body: vec![Stmt::Expr(Expr::Call {
                    name: "print".into(),
                    args: vec![Expr::Ident("name".into())],
                })],
                is_async: false,
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
                params: vec![Param { name: "n".into(), type_: None }],
                return_type: None,
                body: vec![
                    Stmt::Assign {
                        name: "x".into(),
                        type_: None,
                        value: Expr::BinOp {
                            op: BinOpKind::Mul,
                            left: Box::new(Expr::Ident("n".into())),
                            right: Box::new(Expr::Int(2)),
                        },
                    },
                    Stmt::Return(Expr::Ident("x".into())),
                ],
                is_async: false,
            },
        );
    }

    #[test]
    fn fndef_block_with_full_types_and_multiple_params() {
        // fn add(a: Int, b: Int) -> Int { return a + b }
        let stmt = parse_one_stmt("fn add(a: Int, b: Int) -> Int { return a + b }");
        match stmt {
            Stmt::FnDef { name, params, return_type, body, is_async } => {
                assert_eq!(name, "add");
                assert_eq!(params.len(), 2);
                assert_eq!(params[0].name, "a");
                assert_eq!(params[0].type_.as_deref(), Some("Int"));
                assert_eq!(params[1].name, "b");
                assert_eq!(params[1].type_.as_deref(), Some("Int"));
                assert_eq!(return_type.as_deref(), Some("Int"));
                assert_eq!(body.len(), 1);
                assert!(!is_async);
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
            Stmt::FnDef { name, is_async, return_type, .. } => {
                assert_eq!(name, "fetch");
                assert!(is_async);
                assert_eq!(return_type.as_deref(), Some("User"));
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
        assert_eq!(parse_expr(r#""hola""#).unwrap(), Expr::Str("hola".into()));
    }

    #[test]
    fn empty_string_is_plain_str() {
        assert_eq!(parse_expr(r#""""#).unwrap(), Expr::Str("".into()));
    }

    #[test]
    fn string_with_simple_ident_interpolation() {
        // "Hola, {name}!" → StrInterp([Lit, Expr, Lit])
        assert_eq!(
            parse_expr(r#""Hola, {name}!""#).unwrap(),
            Expr::StrInterp(vec![
                StrPart::Lit("Hola, ".into()),
                StrPart::Expr(Expr::Ident("name".into())),
                StrPart::Lit("!".into()),
            ]),
        );
    }

    #[test]
    fn string_starting_with_interpolation() {
        // "{x} es el valor" → StrInterp([Expr, Lit])
        assert_eq!(
            parse_expr(r#""{x} es el valor""#).unwrap(),
            Expr::StrInterp(vec![
                StrPart::Expr(Expr::Ident("x".into())),
                StrPart::Lit(" es el valor".into()),
            ]),
        );
    }

    #[test]
    fn string_ending_with_interpolation() {
        // "valor: {x}" → StrInterp([Lit, Expr])
        assert_eq!(
            parse_expr(r#""valor: {x}""#).unwrap(),
            Expr::StrInterp(vec![
                StrPart::Lit("valor: ".into()),
                StrPart::Expr(Expr::Ident("x".into())),
            ]),
        );
    }

    #[test]
    fn string_with_only_interpolation_no_literal_parts() {
        // "{x}" — sin literales alrededor.
        assert_eq!(
            parse_expr(r#""{x}""#).unwrap(),
            Expr::StrInterp(vec![StrPart::Expr(Expr::Ident("x".into()))]),
        );
    }

    #[test]
    fn string_with_multiple_interpolations() {
        // "Hola {name}, tenés {n} mensajes"
        assert_eq!(
            parse_expr(r#""Hola {name}, tenés {n} mensajes""#).unwrap(),
            Expr::StrInterp(vec![
                StrPart::Lit("Hola ".into()),
                StrPart::Expr(Expr::Ident("name".into())),
                StrPart::Lit(", tenés ".into()),
                StrPart::Expr(Expr::Ident("n".into())),
                StrPart::Lit(" mensajes".into()),
            ]),
        );
    }

    #[test]
    fn string_with_arithmetic_interpolation() {
        // "respuesta: {40 + 2}"
        assert_eq!(
            parse_expr(r#""respuesta: {40 + 2}""#).unwrap(),
            Expr::StrInterp(vec![
                StrPart::Lit("respuesta: ".into()),
                StrPart::Expr(Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Int(40)),
                    right: Box::new(Expr::Int(2)),
                }),
            ]),
        );
    }

    #[test]
    fn escaped_braces_become_literal_in_plain_string() {
        // "\{nombre\}" → literal "{nombre}" sin interpolación.
        assert_eq!(
            parse_expr(r#""\{nombre\}""#).unwrap(),
            Expr::Str("{nombre}".into()),
        );
    }

    #[test]
    fn escaped_and_unescaped_braces_in_same_string() {
        // "\{ {x} \}" → literal "{ ", interpolación de x, literal " }"
        assert_eq!(
            parse_expr(r#""\{ {x} \}""#).unwrap(),
            Expr::StrInterp(vec![
                StrPart::Lit("{ ".into()),
                StrPart::Expr(Expr::Ident("x".into())),
                StrPart::Lit(" }".into()),
            ]),
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
            err.column, err.message,
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
            Stmt::Expr(Expr::If { condition, then, else_ }) => {
                assert_eq!(
                    *condition,
                    Expr::BinOp {
                        op: BinOpKind::Lt,
                        left: Box::new(Expr::Ident("x".into())),
                        right: Box::new(Expr::Int(5)),
                    }
                );
                assert_eq!(then.len(), 1);
                assert!(else_.is_none());
            }
            other => panic!("se esperaba Stmt::Expr(If), se obtuvo {:?}", other),
        }
    }

    #[test]
    fn if_with_else() {
        let stmt = parse_one_stmt("if x { 1 } else { 2 }");
        match stmt {
            Stmt::Expr(Expr::If { else_: Some(e), .. }) => {
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
            Stmt::Expr(Expr::If { else_: Some(outer_else), .. }) => {
                // El else exterior contiene una sola stmt: un Expr::If anidado.
                assert_eq!(outer_else.len(), 1);
                match &outer_else[0] {
                    Stmt::Expr(Expr::If { else_: Some(inner_else), .. }) => {
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
            Stmt::Assign { name, value: Expr::If { .. }, .. } => {
                assert_eq!(name, "status");
            }
            other => panic!("se esperaba Assign con If como valor, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn if_with_multiline_block() {
        let src = "if x {\n  let y = 1\n  print(y)\n}";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::If { then, .. }) => {
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
            Stmt::Expr(Expr::Match { arms, .. }) => {
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
            Stmt::Expr(Expr::Match { arms, .. }) => {
                assert_eq!(arms.len(), 2);
                assert_eq!(arms[0].pattern, Pattern::OkBinding("u".into()));
                assert_eq!(arms[1].pattern, Pattern::ErrBinding("e".into()));
            }
            other => panic!("se esperaba Match, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn match_with_newline_separated_arms() {
        let src = "match x {\n  foo => 1\n  bar => 2\n  _ => 0\n}";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::Expr(Expr::Match { arms, .. }) => assert_eq!(arms.len(), 3),
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
            Stmt::TypeDef { name, fields } => {
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
            Stmt::TypeDef { name, fields } => {
                assert_eq!(name, "User");
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].name, "id");
                assert_eq!(fields[0].type_, "Int");
                assert!(!fields[0].nullable);
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
                assert!(fields[1].nullable);
                assert_eq!(fields[1].default, Some(Expr::Null));
                // active no es nullable pero tiene default true
                assert_eq!(fields[2].name, "active");
                assert!(!fields[2].nullable);
                assert_eq!(fields[2].default, Some(Expr::Bool(true)));
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
    // Tests — HTTP endpoints (paso 8)
    // -----------------------------------------------------------------------

    #[test]
    fn http_get_minimal() {
        // @get("/")
        // fn index() => "hola"
        let src = "@get(\"/\")\nfn index() => \"hola\"";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::HttpEndpoint { method, path, handler } => {
                assert_eq!(method, HttpMethod::Get);
                assert_eq!(path, "/");
                match *handler {
                    Stmt::FnDef { name, is_async, .. } => {
                        assert_eq!(name, "index");
                        assert!(!is_async);
                    }
                    other => panic!("handler debe ser FnDef, fue {:?}", other),
                }
            }
            other => panic!("se esperaba HttpEndpoint, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn http_post_with_async_block_handler() {
        let src = "@post(\"/users\")\nasync fn create_user(body: UserInput) -> User {\n  return body\n}";
        let stmt = parse_one_stmt(src);
        match stmt {
            Stmt::HttpEndpoint { method, path, handler } => {
                assert_eq!(method, HttpMethod::Post);
                assert_eq!(path, "/users");
                match *handler {
                    Stmt::FnDef { name, is_async, return_type, params, .. } => {
                        assert_eq!(name, "create_user");
                        assert!(is_async);
                        assert_eq!(return_type.as_deref(), Some("User"));
                        assert_eq!(params.len(), 1);
                        assert_eq!(params[0].name, "body");
                    }
                    other => panic!("handler debe ser FnDef, fue {:?}", other),
                }
            }
            other => panic!("se esperaba HttpEndpoint, se obtuvo {:?}", other),
        }
    }

    #[test]
    fn http_put_and_delete_methods() {
        let put = parse_one_stmt("@put(\"/users/{id}\")\nasync fn upd(id: Int) -> User => user");
        let del = parse_one_stmt("@delete(\"/users/{id}\")\nasync fn del(id: Int) => 0");
        match put {
            Stmt::HttpEndpoint { method, path, .. } => {
                assert_eq!(method, HttpMethod::Put);
                assert_eq!(path, "/users/{id}");
            }
            _ => panic!(),
        }
        match del {
            Stmt::HttpEndpoint { method, .. } => assert_eq!(method, HttpMethod::Delete),
            _ => panic!(),
        }
    }

    #[test]
    fn http_unknown_decorator_errors() {
        let err = parse_program_str("@patch(\"/x\")\nfn h() => 0").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
        assert!(err.message.contains("@patch"));
    }

    #[test]
    fn http_non_string_path_errors() {
        // @get(42) — la ruta tiene que ser string literal.
        let err = parse_program_str("@get(42)\nfn h() => 0").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn http_decorator_without_handler_errors() {
        // @get("/x") y nada después.
        let err = parse_program_str("@get(\"/x\")").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
    }

    #[test]
    fn http_decorator_followed_by_non_fn_errors() {
        // @get("/x") let x = 1  → error claro
        let err = parse_program_str("@get(\"/x\")\nlet x = 1").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken));
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
                name: "name".into(),
                type_: None,
                value: Expr::Str("Fitz".into()),
            }
        );

        // 2. x = 10 + 5
        assert_eq!(
            program[1],
            Stmt::Assign {
                name: "x".into(),
                type_: None,
                value: Expr::BinOp {
                    op: BinOpKind::Add,
                    left: Box::new(Expr::Int(10)),
                    right: Box::new(Expr::Int(5)),
                },
            }
        );

        // 3. print("Hola, {name}!")
        assert_eq!(
            program[2],
            Stmt::Expr(Expr::Call {
                name: "print".into(),
                args: vec![Expr::StrInterp(vec![
                    StrPart::Lit("Hola, ".into()),
                    StrPart::Expr(Expr::Ident("name".into())),
                    StrPart::Lit("!".into()),
                ])],
            })
        );

        // 4. fn double(n) => n * 2
        assert_eq!(
            program[3],
            Stmt::FnDef {
                name: "double".into(),
                params: vec![Param { name: "n".into(), type_: None }],
                return_type: None,
                body: vec![Stmt::Return(Expr::BinOp {
                    op: BinOpKind::Mul,
                    left: Box::new(Expr::Ident("n".into())),
                    right: Box::new(Expr::Int(2)),
                })],
                is_async: false,
            }
        );

        // 5. print(double(x))
        assert_eq!(
            program[4],
            Stmt::Expr(Expr::Call {
                name: "print".into(),
                args: vec![Expr::Call {
                    name: "double".into(),
                    args: vec![Expr::Ident("x".into())],
                }],
            }),
        );
    }
}
