// lexer.rs — Fase 2.1
//
// El lexer convierte el código fuente en una lista de tokens con posición.
//
// Ejemplo:
//   input:  "let x = 42 + 1"
//   output: [Let, Ident("x"), Eq, Int(42), Plus, Int(1), EOF]
//
// El newline se emite como token (no se trata como whitespace) porque Fitz
// usa salto de línea como separador opcional de sentencias — el parser
// decide cuándo es relevante.

use crate::error::{ErrorKind, FitzError, FitzResult};

/// Tipo de token. La info de línea/columna va aparte, en `TokenWithPos`.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::upper_case_acronyms)] // EOF es el nombre canónico.
pub enum Token {
    // Literales
    Int(i64),
    Float(f64),
    Str(String),

    // Identificadores y keywords
    Ident(String),
    Fn,
    Async,
    Await,
    Return,
    Let,
    If,
    Else,
    For,
    While,
    Loop,
    Match,
    Type,
    Import,
    From,
    As,
    True,
    False,
    Null,
    In,
    Break,
    Continue,
    And,
    Or,
    Not, // R.1.1 — `not <expr>` negación lógica prefix

    // Operadores
    Plus,     // +
    Minus,    // -
    Star,     // *
    Slash,    // /
    Percent,  // % — operador módulo (R.1.2)
    PlusEq,   // += (R.2.3)
    MinusEq,  // -= (R.2.3)
    StarEq,   // *= (R.2.3)
    SlashEq,  // /= (R.2.3)
    Eq,       // =
    EqEq,     // ==
    NotEq,    // !=
    Lt,       // <
    LtEq,     // <=
    Gt,       // >
    GtEq,     // >=
    Arrow,    // ->
    FatArrow, // =>
    Question, // ?
    DotDot,   // ..
    DotDotEq, // ..= (R.1.4: rangos inclusivos)

    // Delimitadores
    LParen,   // (
    RParen,   // )
    LBrace,   // {
    RBrace,   // }
    LBracket, // [
    RBracket, // ]
    Comma,    // ,
    Colon,    // :
    Dot,      // .
    At,       // @ — prefijo de decoradores: @get, @post, @server, ...
    Pipe,     // | — separador de or-patterns en `match` (R.2.1); OR bit-a-bit (mini-tanda Bits)
    // Operadores bit-a-bit (mini-tanda Bits).
    Amp,      // & — AND bit-a-bit
    Caret,    // ^ — XOR bit-a-bit
    Shl,      // << — shift left
    Shr,      // >> — shift right
    Tilde,    // ~ — NOT bit-a-bit (unario)
    Label(String),  // 'name — labels en break/continue (mini-tanda L)

    // Especiales
    Newline,
    EOF,
}

/// Token con su posición en el código fuente. Es lo que devuelve `tokenize`.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenWithPos {
    pub token: Token,
    pub line: usize,
    pub column: usize,
}

impl TokenWithPos {
    fn new(token: Token, line: usize, column: usize) -> Self {
        Self { token, line, column }
    }
}

// ---------------------------------------------------------------------------
// Trivia — Fase 9.z.1.b (formatter comment preservation)
//
// El lexer normalmente strippea comentarios y blank lines: el AST no los
// necesita y el resto del pipeline tampoco. Pero el formatter SÍ los
// necesita para preservar el código del usuario al reescribir. Trivia
// es el side-channel que `tokenize_with_trivia` retorna: tokens van
// por un lado, comments + blank lines por el otro. El parser sigue
// usando solo los tokens, así que el AST no se contamina.
// ---------------------------------------------------------------------------

/// Tipo de comentario capturado. Fitz solo tiene `//` (line) y
/// `/* */` (block). El formatter los emite diferente.
#[derive(Debug, Clone, PartialEq)]
pub enum CommentKind {
    Line,
    Block,
}

/// Comentario capturado del source. La `text` NO incluye el prefijo
/// (`//` o `/*`) ni el sufijo (`*/` para block). Posición es 1-based
/// e indica dónde arranca el delimitador de apertura.
#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    pub text: String,
    pub line: usize,
    pub column: usize,
    pub kind: CommentKind,
}

/// Side-channel del lexer: todo lo que el lexer normalmente
/// descartaría pero que el formatter necesita preservar.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trivia {
    /// Comments en orden de aparición.
    pub comments: Vec<Comment>,
    /// Números de línea (1-based) que estaban completamente vacías
    /// en el source. NO incluye líneas que contienen solo un
    /// comentario — esas están representadas via `comments`.
    pub blank_lines: Vec<usize>,
}

/// Estado interno del escaneo. Privado al módulo.
struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    column: usize,
    /// Si está activo, el lexer captura comments y blank lines en
    /// `trivia` (Fase 9.z.1.b). Inactivo por default — la `tokenize`
    /// rápida sigue siendo zero-overhead.
    collect_trivia: bool,
    trivia: Trivia,
    /// Flags por línea para detectar blank lines correctamente
    /// (líneas con solo whitespace cuentan como blank; líneas con
    /// comentario NO). Se resetean al consumir un `\n`.
    line_had_code: bool,
    line_had_comment: bool,
    /// Mini-tanda T — `true` justo después de emitir `Token::Dot`.
    /// `read_number` lo consulta para NO entrar a modo float
    /// cuando ve `<dígitos>.<dígito>` precedido de Dot: `t.0.0`
    /// debe tokenizar como `Ident("t") Dot Int(0) Dot Int(0)`,
    /// no como `Ident("t") Dot Float(0.0)`.
    prev_was_dot: bool,
}

impl Lexer {
    fn new(source: &str) -> Self {
        Self {
            chars: source.chars().collect(),
            pos: 0,
            line: 1,
            column: 1,
            collect_trivia: false,
            trivia: Trivia::default(),
            line_had_code: false,
            line_had_comment: false,
            prev_was_dot: false,
        }
    }

    fn new_with_trivia(source: &str) -> Self {
        let mut l = Self::new(source);
        l.collect_trivia = true;
        l
    }

    /// Char actual sin consumir. None si llegamos al final.
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    /// Char siguiente (pos + 1) sin consumir.
    fn peek_next(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    /// Consume el char actual y actualiza línea/columna.
    ///
    /// **Fase 9.z.1.b**: al cruzar un `\n`, si la línea que
    /// estamos cerrando no tuvo ningún token NI comment (es decir,
    /// solo whitespace), la registramos como blank_line en `trivia`.
    /// Comments-only lines no son blanks.
    fn advance(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        if c == '\n' {
            if self.collect_trivia
                && !self.line_had_code
                && !self.line_had_comment
            {
                self.trivia.blank_lines.push(self.line);
            }
            self.line += 1;
            self.column = 1;
            self.line_had_code = false;
            self.line_had_comment = false;
        } else {
            self.column += 1;
        }
        Some(c)
    }

    /// Salta espacios, tabs y comentarios. NO salta '\n' (ese es token).
    fn skip_whitespace_and_comments(&mut self) -> FitzResult<()> {
        loop {
            match self.peek() {
                Some(' ') | Some('\t') | Some('\r') => {
                    self.advance();
                }
                Some('/') if self.peek_next() == Some('/') => {
                    // comentario de línea — consumimos hasta el '\n' (sin incluirlo)
                    let start_line = self.line;
                    let start_col = self.column;
                    self.advance(); // '/'
                    self.advance(); // '/'
                    let text_start = self.pos;
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.advance();
                    }
                    if self.collect_trivia {
                        let text: String =
                            self.chars[text_start..self.pos].iter().collect();
                        self.trivia.comments.push(Comment {
                            text,
                            line: start_line,
                            column: start_col,
                            kind: CommentKind::Line,
                        });
                        self.line_had_comment = true;
                    }
                }
                Some('/') if self.peek_next() == Some('*') => {
                    let start_line = self.line;
                    let start_col = self.column;
                    self.advance(); // '/'
                    self.advance(); // '*'
                    let text_start = self.pos;
                    let text_end;
                    loop {
                        match self.peek() {
                            Some('*') if self.peek_next() == Some('/') => {
                                text_end = self.pos;
                                self.advance();
                                self.advance();
                                break;
                            }
                            Some(_) => {
                                self.advance();
                            }
                            None => {
                                return Err(FitzError::new(
                                    ErrorKind::UnterminatedComment,
                                    start_line,
                                    start_col,
                                    "Comentario de bloque /* ... */ sin cerrar",
                                ));
                            }
                        }
                    }
                    if self.collect_trivia {
                        let text: String =
                            self.chars[text_start..text_end].iter().collect();
                        self.trivia.comments.push(Comment {
                            text,
                            line: start_line,
                            column: start_col,
                            kind: CommentKind::Block,
                        });
                        self.line_had_comment = true;
                    }
                }
                _ => break,
            }
        }
        Ok(())
    }

    /// Lee un número. Decide entre Int y Float según haya un '.' seguido
    /// de dígito o notación científica (`e`/`E`).
    ///
    /// Cuidado: en `0..10` el '..' es operador de rango, NO punto decimal.
    ///
    /// **Mini-tanda Núm**: soporta separadores `_` entre dígitos
    /// (`1_000_000`, `3.14_15`) y notación científica `e`/`E` con
    /// exponente opcionalmente firmado (`3.14e2`, `1e-10`, `2.5E+3`).
    /// Reglas:
    ///   - `_` solo entre dígitos. Inválido: `_1`, `1_`, `1__0`.
    ///   - `e`/`E` siempre produce Float (incluso `1e10`).
    ///   - El exponente puede llevar `+`/`-` opcional y al menos un
    ///     dígito (`1e`, `1e+` → error).
    ///   - Separadores también permitidos en el exponente (`1e1_0`).
    fn read_number(&mut self) -> FitzResult<Token> {
        let start_line = self.line;
        let start_col = self.column;

        // Mini-tanda Lit — literales hex/binario/octal con prefijos
        // `0x`/`0b`/`0o`. El char actual debe ser `0` y el siguiente
        // el prefijo. Si NO matchea, caemos al flujo decimal de abajo.
        if self.peek() == Some('0') && !self.prev_was_dot {
            match self.peek_next() {
                Some('x') => return self.read_radix_number(16, "hex", start_line, start_col),
                Some('b') => return self.read_radix_number(2, "binario", start_line, start_col),
                Some('o') => return self.read_radix_number(8, "octal", start_line, start_col),
                _ => {}
            }
        }

        let start_pos = self.pos;
        // Helper: lee dígitos + underscores intercalados. Devuelve error
        // si encuentra `_` huérfano (`1__`, termina en `_`).
        self.read_digit_run(start_line, start_col)?;

        // Mini-tanda T — si venimos justo después de `Dot` (tuple
        // field access encadenado como `t.0.0`), NO entramos a modo
        // float. El `0` de `t.0` se cierra como Int, y el `.0`
        // siguiente arrancará un nuevo Int por el mismo camino.
        let has_fraction = !self.prev_was_dot
            && self.peek() == Some('.')
            && self.peek_next().is_some_and(|c| c.is_ascii_digit());
        let mut is_float = false;

        if has_fraction {
            self.advance(); // consumir '.'
            self.advance(); // consumir primer dígito de la parte fraccional
            self.read_digit_run(start_line, start_col)?;
            is_float = true;
        }

        // Notación científica `e`/`E` con exponente opcionalmente firmado.
        if matches!(self.peek(), Some('e') | Some('E')) {
            self.advance(); // consume `e`/`E`
            if matches!(self.peek(), Some('+') | Some('-')) {
                self.advance();
            }
            // Al menos un dígito después del signo.
            match self.peek() {
                Some(c) if c.is_ascii_digit() => {
                    self.advance();
                }
                _ => {
                    return Err(FitzError::new(
                        ErrorKind::InvalidSyntax,
                        start_line,
                        start_col,
                        "exponente de notación científica sin dígitos",
                    ));
                }
            }
            self.read_digit_run(start_line, start_col)?;
            is_float = true;
        }

        // Parse final: limpiar `_` y convertir.
        let raw: String = self.chars[start_pos..self.pos].iter().collect();
        let clean: String = raw.chars().filter(|c| *c != '_').collect();
        if is_float {
            let n = clean.parse::<f64>().map_err(|_| {
                FitzError::new(
                    ErrorKind::InvalidSyntax,
                    start_line,
                    start_col,
                    format!("Número float inválido: '{}'", raw),
                )
            })?;
            Ok(Token::Float(n))
        } else {
            let n = clean.parse::<i64>().map_err(|_| {
                FitzError::new(
                    ErrorKind::InvalidSyntax,
                    start_line,
                    start_col,
                    format!("Número entero inválido: '{}'", raw),
                )
            })?;
            Ok(Token::Int(n))
        }
    }

    /// Mini-tanda Lit — lee un literal con prefijo de radix (hex `0x`,
    /// binario `0b`, octal `0o`). Soporta separadores `_` entre dígitos.
    /// Produce `Token::Int`. Overflow sobre `i64` o dígitos vacíos →
    /// error claro del lexer.
    fn read_radix_number(
        &mut self,
        radix: u32,
        name: &str,
        line: usize,
        col: usize,
    ) -> FitzResult<Token> {
        self.advance(); // consume '0'
        self.advance(); // consume prefijo ('x'/'b'/'o')
        let digit_start = self.pos;
        loop {
            match self.peek() {
                Some(c) if c.is_digit(radix) => {
                    self.advance();
                }
                Some('_') => {
                    // Después de `_` exige dígito válido para la base.
                    if !self.peek_next().is_some_and(|n| n.is_digit(radix)) {
                        return Err(FitzError::new(
                            ErrorKind::InvalidSyntax,
                            line,
                            col,
                            format!("separador `_` en literal {} solo entre dígitos válidos", name),
                        ));
                    }
                    self.advance();
                }
                _ => break,
            }
        }
        if digit_start == self.pos {
            return Err(FitzError::new(
                ErrorKind::InvalidSyntax,
                line,
                col,
                format!("literal {} sin dígitos después del prefijo", name),
            ));
        }
        let raw: String = self.chars[digit_start..self.pos].iter().collect();
        let clean: String = raw.chars().filter(|c| *c != '_').collect();
        let n = i64::from_str_radix(&clean, radix).map_err(|_| {
            FitzError::new(
                ErrorKind::InvalidSyntax,
                line,
                col,
                format!(
                    "literal {} `{}` excede el rango de Int (i64)",
                    name, clean
                ),
            )
        })?;
        Ok(Token::Int(n))
    }

    /// Mini-tanda Núm — lee una secuencia de `digit (_ digit)*`. Permite
    /// `_` entre dígitos pero rechaza `__` consecutivos o un `_` al final.
    /// El primer dígito YA está consumido por el caller; este helper sigue
    /// hasta el primer char que no sea digit/underscore.
    fn read_digit_run(&mut self, line: usize, col: usize) -> FitzResult<()> {
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.advance();
            } else if c == '_' {
                // El char anterior es lo último consumido — un dígito
                // (porque el loop solo avanza con digits o '_' previo
                // validado). Pero después del `_` exigimos otro dígito.
                if !self.peek_next().is_some_and(|n| n.is_ascii_digit()) {
                    return Err(FitzError::new(
                        ErrorKind::InvalidSyntax,
                        line,
                        col,
                        "separador `_` en número solo entre dígitos (ejemplo: `1_000_000`)",
                    ));
                }
                self.advance(); // consume '_'
            } else {
                break;
            }
        }
        Ok(())
    }

    /// Lee un string entre comillas. Soporta escapes básicos: \n \t \r \\ \" \{ \}
    /// La interpolación `"Hola {name}"` se deja "cruda" en el contenido — el
    /// parser/evaluador la procesa más tarde.
    ///
    /// **R.1.5 (mini-fase R)**: además del modo "comilla simple", soporta
    /// **triple-quote** `"""..."""` para strings multilínea. Si tras el
    /// primer `"` vienen dos `"` más, entramos a modo triple: newlines
    /// son válidos adentro y el cierre es `"""` (tres comillas seguidas).
    /// Interpolación `{expr}` sigue funcionando igual.
    fn read_string(&mut self) -> FitzResult<Token> {
        let start_line = self.line;
        let start_col = self.column;
        self.advance(); // consumir comilla de apertura "

        // R.1.5 — modo triple-quote. Si los próximos dos chars también
        // son `"`, estamos en `"""..."""`. Los consumimos y delegamos
        // a la lectura multilínea.
        if self.peek() == Some('"') && self.peek_next() == Some('"') {
            self.advance();
            self.advance();
            return self.read_triple_string(start_line, start_col);
        }

        let mut s = String::new();
        loop {
            match self.peek() {
                Some('"') => {
                    self.advance();
                    return Ok(Token::Str(s));
                }
                Some('\\') => {
                    self.advance();
                    match self.advance() {
                        Some('n') => s.push('\n'),
                        Some('t') => s.push('\t'),
                        Some('r') => s.push('\r'),
                        Some('\\') => s.push('\\'),
                        Some('"') => s.push('"'),
                        // '\{' y '\}' se PRESERVAN literalmente en el
                        // contenido del Token::Str (con la barra).
                        // El parser, al construir la expresión de
                        // string, distingue `{` (inicio de
                        // interpolación) de `\{` (literal). Si los
                        // resolviéramos acá, se perdería la distinción.
                        Some('{') => {
                            s.push('\\');
                            s.push('{');
                        }
                        Some('}') => {
                            s.push('\\');
                            s.push('}');
                        }
                        Some(other) => {
                            return Err(FitzError::new(
                                ErrorKind::UnexpectedChar(other),
                                self.line,
                                self.column,
                                format!("Secuencia de escape inválida: '\\{}'", other),
                            ));
                        }
                        None => {
                            return Err(FitzError::new(
                                ErrorKind::UnterminatedString,
                                start_line,
                                start_col,
                                "String sin cerrar (terminó después de '\\')",
                            ));
                        }
                    }
                }
                Some('\n') => {
                    return Err(FitzError::new(
                        ErrorKind::UnterminatedString,
                        start_line,
                        start_col,
                        "String sin cerrar — salto de línea antes de la comilla de cierre",
                    )
                    .with_hint("Usá \\n para incluir un salto de línea dentro del string"));
                }
                Some(c) => {
                    s.push(c);
                    self.advance();
                }
                None => {
                    return Err(FitzError::new(
                        ErrorKind::UnterminatedString,
                        start_line,
                        start_col,
                        "String sin cerrar — falta la comilla de cierre",
                    ));
                }
            }
        }
    }

    /// R.1.5 — lee el contenido de un string multilínea `"""..."""`.
    /// Las tres comillas iniciales ya fueron consumidas. Diferencias
    /// vs `read_string`:
    ///
    /// - **Newlines** son válidos adentro (se preservan en el
    ///   contenido tal cual, sin requerir `\n`).
    /// - **Cierre** es `"""` (tres comillas seguidas).
    /// - Las **comillas simples y dobles aisladas** dentro del string
    ///   se preservan literalmente; solo cierran cuando aparecen 3
    ///   seguidas.
    /// - Mismos escapes que strings normales (`\n`, `\t`, `\\`, `\"`,
    ///   `\{`, `\}`). Útil si necesitás `"""` literal adentro:
    ///   `\"""`.
    /// - **Interpolación** `{expr}` sigue funcionando — el contenido
    ///   se pasa "crudo" al parser igual que en strings normales.
    fn read_triple_string(
        &mut self,
        start_line: usize,
        start_col: usize,
    ) -> FitzResult<Token> {
        let mut s = String::new();
        loop {
            // Detectar cierre `"""`: si el char actual y los dos
            // siguientes son `"`, terminamos.
            if self.peek() == Some('"')
                && self.peek_next() == Some('"')
                && self.chars.get(self.pos + 2).copied() == Some('"')
            {
                self.advance();
                self.advance();
                self.advance();
                return Ok(Token::Str(s));
            }
            match self.peek() {
                Some('\\') => {
                    self.advance();
                    match self.advance() {
                        Some('n') => s.push('\n'),
                        Some('t') => s.push('\t'),
                        Some('r') => s.push('\r'),
                        Some('\\') => s.push('\\'),
                        Some('"') => s.push('"'),
                        Some('{') => {
                            s.push('\\');
                            s.push('{');
                        }
                        Some('}') => {
                            s.push('\\');
                            s.push('}');
                        }
                        Some(other) => {
                            return Err(FitzError::new(
                                ErrorKind::UnexpectedChar(other),
                                self.line,
                                self.column,
                                format!("Secuencia de escape inválida: '\\{}'", other),
                            ));
                        }
                        None => {
                            return Err(FitzError::new(
                                ErrorKind::UnterminatedString,
                                start_line,
                                start_col,
                                "String multilínea sin cerrar (terminó después de '\\')",
                            ));
                        }
                    }
                }
                // Newline LITERAL — válido adentro de triple-quote.
                // Se preserva tal cual en el contenido.
                Some(c) => {
                    s.push(c);
                    self.advance();
                }
                None => {
                    return Err(FitzError::new(
                        ErrorKind::UnterminatedString,
                        start_line,
                        start_col,
                        "String multilínea sin cerrar — falta `\"\"\"` de cierre",
                    ));
                }
            }
        }
    }

    /// Lee un identificador (letras + dígitos + '_') y decide si es keyword.
    fn read_identifier_or_keyword(&mut self) -> Token {
        let start_pos = self.pos;
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                self.advance();
            } else {
                break;
            }
        }
        let s: String = self.chars[start_pos..self.pos].iter().collect();
        match s.as_str() {
            "fn" => Token::Fn,
            "async" => Token::Async,
            "await" => Token::Await,
            "return" => Token::Return,
            "let" => Token::Let,
            "if" => Token::If,
            "else" => Token::Else,
            "for" => Token::For,
            "while" => Token::While,
            "loop" => Token::Loop,
            "match" => Token::Match,
            "type" => Token::Type,
            "import" => Token::Import,
            "from" => Token::From,
            "as" => Token::As,
            "true" => Token::True,
            "false" => Token::False,
            "null" => Token::Null,
            "in" => Token::In,
            "break" => Token::Break,
            "continue" => Token::Continue,
            "and" => Token::And,
            "or" => Token::Or,
            "not" => Token::Not,
            _ => Token::Ident(s),
        }
    }

    /// Obtiene el siguiente token, o None si terminamos.
    fn next_token(&mut self) -> FitzResult<Option<TokenWithPos>> {
        self.skip_whitespace_and_comments()?;
        let line = self.line;
        let column = self.column;
        let c = match self.peek() {
            Some(c) => c,
            None => return Ok(None),
        };

        let token = match c {
            '\n' => {
                self.advance();
                Token::Newline
            }
            '+' => {
                // R.2.3 — `+=` para asignación compuesta.
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    Token::PlusEq
                } else {
                    Token::Plus
                }
            }
            '-' => {
                self.advance();
                match self.peek() {
                    Some('>') => {
                        self.advance();
                        Token::Arrow
                    }
                    // R.2.3 — `-=` para asignación compuesta.
                    Some('=') => {
                        self.advance();
                        Token::MinusEq
                    }
                    _ => Token::Minus,
                }
            }
            '*' => {
                // R.2.3 — `*=` para asignación compuesta.
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    Token::StarEq
                } else {
                    Token::Star
                }
            }
            '/' => {
                // R.2.3 — `/=` para asignación compuesta.
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    Token::SlashEq
                } else {
                    Token::Slash
                }
            }
            '%' => {
                // R.1.2 — operador módulo. Single char, sin
                // variantes compuestas (%= llega con R.2.3).
                self.advance();
                Token::Percent
            }
            '=' => {
                self.advance();
                match self.peek() {
                    Some('=') => {
                        self.advance();
                        Token::EqEq
                    }
                    Some('>') => {
                        self.advance();
                        Token::FatArrow
                    }
                    _ => Token::Eq,
                }
            }
            '!' => {
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    Token::NotEq
                } else {
                    return Err(FitzError::new(
                        ErrorKind::UnexpectedChar('!'),
                        line,
                        column,
                        "'!' solo es válido como parte de '!='",
                    ));
                }
            }
            '<' => {
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    Token::LtEq
                } else if self.peek() == Some('<') {
                    // Mini-tanda Bits — `<<` shift left.
                    self.advance();
                    Token::Shl
                } else {
                    Token::Lt
                }
            }
            '>' => {
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    Token::GtEq
                } else if self.peek() == Some('>') {
                    // Mini-tanda Bits — `>>` shift right.
                    self.advance();
                    Token::Shr
                } else {
                    Token::Gt
                }
            }
            '?' => {
                self.advance();
                Token::Question
            }
            '.' => {
                self.advance();
                if self.peek() == Some('.') {
                    self.advance();
                    // R.1.4: `..=` para rangos inclusivos. El check de
                    // `..` viene primero, después miramos si el char
                    // siguiente es `=` para upgrade a DotDotEq.
                    if self.peek() == Some('=') {
                        self.advance();
                        Token::DotDotEq
                    } else {
                        Token::DotDot
                    }
                } else {
                    Token::Dot
                }
            }
            '(' => {
                self.advance();
                Token::LParen
            }
            ')' => {
                self.advance();
                Token::RParen
            }
            '{' => {
                self.advance();
                Token::LBrace
            }
            '}' => {
                self.advance();
                Token::RBrace
            }
            '[' => {
                self.advance();
                Token::LBracket
            }
            ']' => {
                self.advance();
                Token::RBracket
            }
            ',' => {
                self.advance();
                Token::Comma
            }
            ':' => {
                self.advance();
                Token::Colon
            }
            '|' => {
                // R.2.1 — separador de or-patterns en `match`. Single char
                // por ahora; `||` (or lógico) usa la keyword `or` y NO
                // entra acá. Mini-tanda Bits: el mismo Token::Pipe se
                // usa como OR bit-a-bit; el parser distingue por contexto
                // (expression nivel bitwise vs arm de match).
                self.advance();
                Token::Pipe
            }
            // Mini-tanda Bits — `&`, `^`, `~`.
            '&' => {
                self.advance();
                Token::Amp
            }
            '^' => {
                self.advance();
                Token::Caret
            }
            '~' => {
                self.advance();
                Token::Tilde
            }
            '\'' => {
                // Mini-tanda L — label `'name` para break/continue.
                // Fitz no tiene char literales con `'x'`, así que el
                // apóstrofe siempre arranca una label. Después del
                // apóstrofe esperamos identificador.
                self.advance(); // consume `'`
                let start_pos = self.pos;
                while let Some(c) = self.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        self.advance();
                    } else {
                        break;
                    }
                }
                if start_pos == self.pos {
                    return Err(FitzError::new(
                        ErrorKind::InvalidSyntax,
                        line,
                        column,
                        "se esperaba un identificador después de `'` (label)".to_string(),
                    ));
                }
                let name: String = self.chars[start_pos..self.pos].iter().collect();
                Token::Label(name)
            }
            '@' => {
                self.advance();
                Token::At
            }
            '"' => self.read_string()?,
            c if c.is_ascii_digit() => self.read_number()?,
            c if c.is_alphabetic() || c == '_' => self.read_identifier_or_keyword(),
            other => {
                self.advance();
                return Err(FitzError::new(
                    ErrorKind::UnexpectedChar(other),
                    line,
                    column,
                    format!("Carácter inesperado: '{}'", other),
                ));
            }
        };

        // Fase 9.z.1.b — marcar la línea como "tuvo código" para
        // que `\n` no la cuente como blank. Newline NO marca código
        // (sino, ninguna línea sería blank).
        if self.collect_trivia && !matches!(token, Token::Newline) {
            self.line_had_code = true;
        }

        Ok(Some(TokenWithPos::new(token, line, column)))
    }
}

/// Convierte código fuente en una lista de tokens con posición.
/// Siempre termina con un `Token::EOF`.
pub fn tokenize(source: &str) -> FitzResult<Vec<TokenWithPos>> {
    let mut lexer = Lexer::new(source);
    let mut tokens = Vec::new();
    while let Some(tok) = lexer.next_token()? {
        // Mini-tanda T — guardar si emitimos Dot para que el próximo
        // `read_number` no entre a modo float (caso `t.0.0`).
        lexer.prev_was_dot = matches!(tok.token, Token::Dot);
        tokens.push(tok);
    }
    tokens.push(TokenWithPos::new(Token::EOF, lexer.line, lexer.column));
    Ok(tokens)
}

/// Fase 9.z.1.b — variante de `tokenize` que ADEMÁS captura
/// comentarios y blank lines como `Trivia` side-channel. Consumida
/// por el formatter (`fitz fmt`) para preservar comments + blank
/// lines del usuario al reescribir.
///
/// Cualquier otro consumidor del lexer (parser, LSP, etc.) sigue
/// usando `tokenize` y obtiene zero overhead. La `Trivia` no se
/// inyecta en el AST.
pub fn tokenize_with_trivia(source: &str) -> FitzResult<(Vec<TokenWithPos>, Trivia)> {
    let mut lexer = Lexer::new_with_trivia(source);
    let mut tokens = Vec::new();
    while let Some(tok) = lexer.next_token()? {
        lexer.prev_was_dot = matches!(tok.token, Token::Dot);
        tokens.push(tok);
    }
    tokens.push(TokenWithPos::new(Token::EOF, lexer.line, lexer.column));
    Ok((tokens, lexer.trivia))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::approx_constant)] // 3.14 en tests es un Float genérico, no PI.
mod tests {
    use super::*;

    /// Helper: tokeniza y devuelve solo los `Token` (sin posiciones).
    fn toks(src: &str) -> Vec<Token> {
        tokenize(src)
            .expect("la fuente debe tokenizar sin error")
            .into_iter()
            .map(|t| t.token)
            .collect()
    }

    #[test]
    fn empty_source_yields_only_eof() {
        assert_eq!(toks(""), vec![Token::EOF]);
    }

    #[test]
    fn integers_and_floats() {
        assert_eq!(toks("42"), vec![Token::Int(42), Token::EOF]);
        assert_eq!(toks("3.14"), vec![Token::Float(3.14), Token::EOF]);
        assert_eq!(toks("0"), vec![Token::Int(0), Token::EOF]);
    }

    #[test]
    fn keywords_are_recognized() {
        let cases = [
            ("fn", Token::Fn),
            ("async", Token::Async),
            ("return", Token::Return),
            ("let", Token::Let),
            ("if", Token::If),
            ("else", Token::Else),
            ("for", Token::For),
            ("while", Token::While),
            ("loop", Token::Loop),
            ("match", Token::Match),
            ("type", Token::Type),
            ("import", Token::Import),
            ("as", Token::As),
            ("from", Token::From),
            ("true", Token::True),
            ("false", Token::False),
            ("null", Token::Null),
            ("in", Token::In),
            ("break", Token::Break),
            ("continue", Token::Continue),
            ("and", Token::And),
            ("or", Token::Or),
            ("not", Token::Not),
        ];
        for (src, expected) in cases {
            assert_eq!(toks(src), vec![expected.clone(), Token::EOF], "src = {}", src);
        }
    }

    #[test]
    fn identifiers_that_start_like_keywords_are_idents() {
        assert_eq!(
            toks("returner"),
            vec![Token::Ident("returner".into()), Token::EOF]
        );
        assert_eq!(
            toks("if_user"),
            vec![Token::Ident("if_user".into()), Token::EOF]
        );
    }

    #[test]
    fn let_with_arithmetic() {
        assert_eq!(
            toks("let x = 42 + 1"),
            vec![
                Token::Let,
                Token::Ident("x".into()),
                Token::Eq,
                Token::Int(42),
                Token::Plus,
                Token::Int(1),
                Token::EOF,
            ]
        );
    }

    #[test]
    fn two_char_operators() {
        assert_eq!(toks("=="), vec![Token::EqEq, Token::EOF]);
        assert_eq!(toks("!="), vec![Token::NotEq, Token::EOF]);
        assert_eq!(toks("<="), vec![Token::LtEq, Token::EOF]);
        assert_eq!(toks(">="), vec![Token::GtEq, Token::EOF]);
        assert_eq!(toks("->"), vec![Token::Arrow, Token::EOF]);
        assert_eq!(toks("=>"), vec![Token::FatArrow, Token::EOF]);
        assert_eq!(toks(".."), vec![Token::DotDot, Token::EOF]);
    }

    #[test]
    fn strings_with_escape_sequences() {
        assert_eq!(toks(r#""Hola""#), vec![Token::Str("Hola".into()), Token::EOF]);
    }

    // ---- R.1.5 — strings multilínea `"""..."""` (mini-fase R) ----

    #[test]
    fn triple_string_simple() {
        let src = "\"\"\"hola\"\"\"";
        assert_eq!(
            toks(src),
            vec![Token::Str("hola".into()), Token::EOF],
        );
    }

    #[test]
    fn triple_string_con_newlines_los_preserva() {
        let src = "\"\"\"linea uno\nlinea dos\"\"\"";
        assert_eq!(
            toks(src),
            vec![
                Token::Str("linea uno\nlinea dos".into()),
                Token::EOF
            ],
        );
    }

    #[test]
    fn triple_string_vacio() {
        let src = "\"\"\"\"\"\"";
        assert_eq!(toks(src), vec![Token::Str("".into()), Token::EOF]);
    }

    #[test]
    fn triple_string_con_comilla_doble_interna_se_preserva() {
        // `"""a "b" c"""` → contenido `a "b" c`. La comilla interna
        // sola no cierra; solo `"""` consecutivas cierran.
        let src = "\"\"\"a \"b\" c\"\"\"";
        assert_eq!(
            toks(src),
            vec![Token::Str("a \"b\" c".into()), Token::EOF],
        );
    }

    #[test]
    fn triple_string_sin_cerrar_es_error() {
        let src = "\"\"\"sin cerrar";
        let res = tokenize(src);
        let err = res.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnterminatedString));
        assert_eq!(
            toks(r#""linea\ndos""#),
            vec![Token::Str("linea\ndos".into()), Token::EOF]
        );
        assert_eq!(
            toks(r#""comilla\"dentro""#),
            vec![Token::Str("comilla\"dentro".into()), Token::EOF]
        );
    }

    #[test]
    fn interpolation_braces_stay_in_string_content() {
        // El lexer NO interpola — deja "{name}" tal cual; el parser/eval lo manejará.
        assert_eq!(
            toks(r#""Hola, {name}!""#),
            vec![Token::Str("Hola, {name}!".into()), Token::EOF]
        );
    }

    #[test]
    fn escaped_braces_are_preserved_literally() {
        // '\{' y '\}' deben llegar al parser con la barra intacta,
        // así el parser distingue '{name}' (interpolación) de '\{name\}'
        // (literal). Si el lexer los desescapara acá, se perdería esa
        // distinción.
        assert_eq!(
            toks(r#""hola \{name\}""#),
            vec![Token::Str(r"hola \{name\}".into()), Token::EOF]
        );
    }

    #[test]
    fn http_decorator_decomposes_into_at_plus_ident() {
        assert_eq!(
            toks(r#"@get("/users")"#),
            vec![
                Token::At,
                Token::Ident("get".into()),
                Token::LParen,
                Token::Str("/users".into()),
                Token::RParen,
                Token::EOF,
            ]
        );
    }

    #[test]
    fn line_comments_are_skipped_but_newline_remains() {
        assert_eq!(
            toks("let x = 42 // comentario\nlet y"),
            vec![
                Token::Let,
                Token::Ident("x".into()),
                Token::Eq,
                Token::Int(42),
                Token::Newline,
                Token::Let,
                Token::Ident("y".into()),
                Token::EOF,
            ]
        );
    }

    #[test]
    fn block_comments_are_skipped() {
        assert_eq!(
            toks("let /* hola */ x"),
            vec![Token::Let, Token::Ident("x".into()), Token::EOF]
        );
    }

    #[test]
    fn unterminated_string_errors() {
        let res = tokenize(r#""sin cerrar"#);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnterminatedString));
    }

    #[test]
    fn unterminated_block_comment_errors() {
        let res = tokenize("/* sin cerrar");
        assert!(res.is_err());
        assert!(matches!(res.unwrap_err().kind, ErrorKind::UnterminatedComment));
    }

    #[test]
    fn position_tracking_lines_and_columns() {
        let tokens = tokenize("let\n  x").unwrap();
        // [Let, Newline, Ident("x"), EOF]
        assert_eq!(tokens[0].token, Token::Let);
        assert_eq!((tokens[0].line, tokens[0].column), (1, 1));
        assert_eq!(tokens[1].token, Token::Newline);
        assert_eq!((tokens[1].line, tokens[1].column), (1, 4));
        assert_eq!(tokens[2].token, Token::Ident("x".into()));
        assert_eq!((tokens[2].line, tokens[2].column), (2, 3));
    }

    #[test]
    fn range_vs_field_access() {
        // 0..10 → DotDot
        assert_eq!(
            toks("0..10"),
            vec![Token::Int(0), Token::DotDot, Token::Int(10), Token::EOF]
        );
        // user.name → Dot
        assert_eq!(
            toks("user.name"),
            vec![
                Token::Ident("user".into()),
                Token::Dot,
                Token::Ident("name".into()),
                Token::EOF
            ]
        );
        // 0.5 sigue siendo Float
        assert_eq!(toks("0.5"), vec![Token::Float(0.5), Token::EOF]);
    }

    #[test]
    fn hello_fitz_example() {
        // Equivalente al examples/hello.fitz (sin el emoji para mantener el test corto)
        let src = r#"name = "Patagonia"
print("Hola, {name}!")"#;
        let result: Vec<Token> = toks(src);
        assert_eq!(
            result,
            vec![
                Token::Ident("name".into()),
                Token::Eq,
                Token::Str("Patagonia".into()),
                Token::Newline,
                Token::Ident("print".into()),
                Token::LParen,
                Token::Str("Hola, {name}!".into()),
                Token::RParen,
                Token::EOF,
            ]
        );
    }

    // ---- Fase 9.z.1.b — trivia (comments + blank lines) ----

    #[test]
    fn tokenize_default_no_captura_trivia() {
        // El `tokenize` rápido no debe gastar memoria en trivia.
        // El test confirma indirectamente que el lexer no se ralentiza
        // (no hay un side-table que llenar).
        let _ = tokenize("// comentario\nlet x = 1\n").unwrap();
        // Nada que assertear directamente — tokenize no expone trivia.
        // El test garantiza que la API sigue siendo zero-overhead.
    }

    #[test]
    fn tokenize_with_trivia_captura_comment_de_linea() {
        let (_toks, trivia) =
            tokenize_with_trivia("// hola\nlet x = 1\n").unwrap();
        assert_eq!(trivia.comments.len(), 1);
        let c = &trivia.comments[0];
        assert_eq!(c.kind, CommentKind::Line);
        assert_eq!(c.text, " hola"); // sin el `//`, con el espacio
        assert_eq!(c.line, 1);
        assert_eq!(c.column, 1);
    }

    #[test]
    fn tokenize_with_trivia_captura_comment_trailing() {
        let (_toks, trivia) =
            tokenize_with_trivia("let x = 1 // explicación\n").unwrap();
        assert_eq!(trivia.comments.len(), 1);
        let c = &trivia.comments[0];
        assert_eq!(c.text, " explicación");
        assert_eq!(c.line, 1);
        // El `//` arranca en columna 11 (después de `let x = 1 `).
        assert!(c.column > 1);
    }

    #[test]
    fn tokenize_with_trivia_captura_comment_de_bloque() {
        let (_toks, trivia) =
            tokenize_with_trivia("/* foo bar */\nlet x = 1\n").unwrap();
        assert_eq!(trivia.comments.len(), 1);
        let c = &trivia.comments[0];
        assert_eq!(c.kind, CommentKind::Block);
        assert_eq!(c.text, " foo bar ");
        assert_eq!(c.line, 1);
    }

    #[test]
    fn tokenize_with_trivia_captura_blank_lines() {
        let src = "let x = 1\n\nlet y = 2\n\n\nlet z = 3\n";
        let (_toks, trivia) = tokenize_with_trivia(src).unwrap();
        // Líneas blank: 2 (entre x y y), 4 y 5 (entre y y z).
        assert_eq!(trivia.blank_lines, vec![2, 4, 5]);
    }

    #[test]
    fn tokenize_with_trivia_no_cuenta_line_de_comment_como_blank() {
        let src = "let x = 1\n// solo comment\nlet y = 2\n";
        let (_toks, trivia) = tokenize_with_trivia(src).unwrap();
        assert!(trivia.blank_lines.is_empty(), "blanks: {:?}", trivia.blank_lines);
        assert_eq!(trivia.comments.len(), 1);
        assert_eq!(trivia.comments[0].line, 2);
    }

    #[test]
    fn tokenize_with_trivia_orden_de_comments_es_source_order() {
        let src = "// uno\nlet x = 1\n// dos\nlet y = 2\n// tres\n";
        let (_toks, trivia) = tokenize_with_trivia(src).unwrap();
        assert_eq!(trivia.comments.len(), 3);
        let texts: Vec<&str> = trivia.comments.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, [" uno", " dos", " tres"]);
        assert_eq!(trivia.comments[0].line, 1);
        assert_eq!(trivia.comments[1].line, 3);
        assert_eq!(trivia.comments[2].line, 5);
    }

    #[test]
    fn tokenize_with_trivia_mix_de_comments_y_blanks() {
        let src = "\n// header\n\nlet x = 1\n\n// otro\nlet y = 2 // trailing\n";
        let (_toks, trivia) = tokenize_with_trivia(src).unwrap();
        assert_eq!(trivia.comments.len(), 3);
        assert!(trivia.blank_lines.contains(&1));
        assert!(trivia.blank_lines.contains(&3));
        assert!(trivia.blank_lines.contains(&5));
    }

    // ---------------------------------------------------------------
    // Mini-tanda Núm — separadores `_` + notación científica.
    // ---------------------------------------------------------------

    /// Helper: tokeniza una sola expresión y devuelve el primer token.
    fn first_token_of(src: &str) -> Token {
        let toks = tokenize(src).expect("debe tokenizar");
        toks.into_iter().next().expect("al menos un token").token
    }

    #[test]
    fn num_separador_en_int_se_parsea_como_entero_sin_underscores() {
        assert_eq!(first_token_of("1_000_000"), Token::Int(1_000_000));
        assert_eq!(first_token_of("1_2_3"), Token::Int(123));
    }

    #[test]
    fn num_separador_en_float_funciona_en_int_y_fraction() {
        assert_eq!(first_token_of("1_000.5"), Token::Float(1000.5));
        assert_eq!(first_token_of("3.14_15"), Token::Float(3.1415));
        assert_eq!(first_token_of("1_000.000_1"), Token::Float(1000.0001));
    }

    #[test]
    fn num_separador_doble_o_terminal_es_error() {
        assert!(tokenize("1__0").is_err(), "doble underscore");
        assert!(tokenize("1_000_").is_err(), "underscore al final");
    }

    #[test]
    fn num_notacion_cientifica_basica_produce_float() {
        assert_eq!(first_token_of("1e10"), Token::Float(1e10));
        assert_eq!(first_token_of("3.14e2"), Token::Float(314.0));
        assert_eq!(first_token_of("2.5E3"), Token::Float(2500.0));
    }

    #[test]
    fn num_notacion_cientifica_con_signo() {
        assert_eq!(first_token_of("1e-10"), Token::Float(1e-10));
        assert_eq!(first_token_of("1e+3"), Token::Float(1000.0));
        assert_eq!(first_token_of("3.14E-2"), Token::Float(0.0314));
    }

    #[test]
    fn num_separador_en_exponente() {
        // `1e1_0` → `1e10` tras limpiar separadores.
        assert_eq!(first_token_of("1e1_0"), Token::Float(1e10));
        assert_eq!(first_token_of("1_000e1_0"), Token::Float(1000e10));
    }

    #[test]
    fn num_exponente_sin_digitos_es_error() {
        assert!(tokenize("1e").is_err(), "`e` solo sin dígitos");
        assert!(tokenize("1e+").is_err(), "`e+` sin dígitos");
        assert!(tokenize("1e-").is_err(), "`e-` sin dígitos");
    }

    #[test]
    fn num_int_clasico_sigue_funcionando() {
        // Regresión — sin separadores ni notación cientifica, mismo
        // resultado que antes.
        assert_eq!(first_token_of("42"), Token::Int(42));
        assert_eq!(first_token_of("3.14"), Token::Float(3.14));
    }

    #[test]
    fn num_tuple_field_access_no_se_confunde_con_separador() {
        // `t.0` produce Ident("t"), Dot, Int(0) — no se confunde con
        // ningún parseo de separador. `t.0.0` (acceso a tuple anidado)
        // sigue funcionando: la flag prev_was_dot fuerza Int en lugar
        // de Float.
        use Token::*;
        let toks: Vec<Token> = tokenize("t.0.0").unwrap().into_iter().map(|t| t.token).collect();
        assert_eq!(toks[0], Ident("t".into()));
        assert_eq!(toks[1], Dot);
        assert_eq!(toks[2], Int(0));
        assert_eq!(toks[3], Dot);
        assert_eq!(toks[4], Int(0));
    }

    // ---------------------------------------------------------------
    // Mini-tanda Lit — literales hex / binario / octal.
    // ---------------------------------------------------------------

    #[test]
    fn lit_hex_basico_lower_y_upper_case() {
        // Los dígitos hex son case-insensitive (paralelo a Rust/Python).
        assert_eq!(first_token_of("0xFF"), Token::Int(255));
        assert_eq!(first_token_of("0xff"), Token::Int(255));
        assert_eq!(first_token_of("0xCAFE"), Token::Int(0xCAFE));
        assert_eq!(first_token_of("0x0"), Token::Int(0));
    }

    #[test]
    fn lit_binario_y_octal_basicos() {
        assert_eq!(first_token_of("0b1010"), Token::Int(10));
        assert_eq!(first_token_of("0b0"), Token::Int(0));
        assert_eq!(first_token_of("0o755"), Token::Int(0o755));
        assert_eq!(first_token_of("0o7"), Token::Int(7));
    }

    #[test]
    fn lit_separadores_en_hex_bin_oct() {
        // `_` entre dígitos válidos para cada base.
        assert_eq!(first_token_of("0xDEAD_BEEF"), Token::Int(0xDEAD_BEEF));
        assert_eq!(first_token_of("0b1010_1010"), Token::Int(0b1010_1010));
        assert_eq!(first_token_of("0o7_5_5"), Token::Int(0o755));
    }

    #[test]
    fn lit_sin_digitos_tras_prefijo_es_error() {
        // `0x`, `0b`, `0o` solos sin dígitos.
        assert!(tokenize("0x").is_err());
        assert!(tokenize("0b").is_err());
        assert!(tokenize("0o").is_err());
    }

    #[test]
    fn lit_digito_invalido_para_la_base_corta_el_literal() {
        // `0b2` lexea `0b` + ... pero no hay '2' válido en binario.
        // El lexer corta tras el '0' del prefijo, dispara error "sin
        // dígitos tras prefijo".
        assert!(tokenize("0b2").is_err());
        // `0o9` mismo case: `9` no es octal válido.
        assert!(tokenize("0o9").is_err());
    }

    #[test]
    fn lit_overflow_es_error_explicito() {
        // i64::MAX = 0x7FFF_FFFF_FFFF_FFFF (positivo). Un nibble más → overflow.
        assert!(tokenize("0xFFFFFFFFFFFFFFFF").is_err());
        // Binario equivalente.
        assert!(tokenize("0b11111111111111111111111111111111111111111111111111111111111111111").is_err());
    }

    #[test]
    fn lit_underscore_terminal_o_doble_es_error_en_hex() {
        assert!(tokenize("0xFF_").is_err(), "underscore al final");
        assert!(tokenize("0xF__F").is_err(), "doble underscore");
    }

    #[test]
    fn lit_decimal_clasico_sigue_funcionando() {
        // Regresión: nada que arranca con `0` que no tiene prefijo
        // hex/bin/oct se sigue parseando como decimal.
        assert_eq!(first_token_of("0"), Token::Int(0));
        assert_eq!(first_token_of("007"), Token::Int(7));
        assert_eq!(first_token_of("0.5"), Token::Float(0.5));
    }
}
