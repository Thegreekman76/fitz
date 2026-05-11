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
    True,
    False,
    Null,
    In,
    Break,
    Continue,

    // Operadores
    Plus,     // +
    Minus,    // -
    Star,     // *
    Slash,    // /
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

/// Estado interno del escaneo. Privado al módulo.
struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    column: usize,
}

impl Lexer {
    fn new(source: &str) -> Self {
        Self {
            chars: source.chars().collect(),
            pos: 0,
            line: 1,
            column: 1,
        }
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
    fn advance(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.column = 1;
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
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.advance();
                    }
                }
                Some('/') if self.peek_next() == Some('*') => {
                    let start_line = self.line;
                    let start_col = self.column;
                    self.advance(); // '/'
                    self.advance(); // '*'
                    loop {
                        match self.peek() {
                            Some('*') if self.peek_next() == Some('/') => {
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
                }
                _ => break,
            }
        }
        Ok(())
    }

    /// Lee un número. Decide entre Int y Float según haya un '.' seguido de dígito.
    /// Cuidado: en `0..10` el '..' es operador de rango, NO punto decimal.
    fn read_number(&mut self) -> FitzResult<Token> {
        let start_pos = self.pos;
        let start_line = self.line;
        let start_col = self.column;

        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.advance();
            } else {
                break;
            }
        }

        let is_float = self.peek() == Some('.')
            && self.peek_next().map_or(false, |c| c.is_ascii_digit());

        if is_float {
            self.advance(); // consumir '.'
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    self.advance();
                } else {
                    break;
                }
            }
            let s: String = self.chars[start_pos..self.pos].iter().collect();
            let n = s.parse::<f64>().map_err(|_| {
                FitzError::new(
                    ErrorKind::InvalidSyntax,
                    start_line,
                    start_col,
                    format!("Número float inválido: '{}'", s),
                )
            })?;
            Ok(Token::Float(n))
        } else {
            let s: String = self.chars[start_pos..self.pos].iter().collect();
            let n = s.parse::<i64>().map_err(|_| {
                FitzError::new(
                    ErrorKind::InvalidSyntax,
                    start_line,
                    start_col,
                    format!("Número entero inválido: '{}'", s),
                )
            })?;
            Ok(Token::Int(n))
        }
    }

    /// Lee un string entre comillas. Soporta escapes básicos: \n \t \r \\ \" \{ \}
    /// La interpolación `"Hola {name}"` se deja "cruda" en el contenido — el
    /// parser/evaluador la procesa más tarde.
    fn read_string(&mut self) -> FitzResult<Token> {
        let start_line = self.line;
        let start_col = self.column;
        self.advance(); // consumir comilla de apertura "
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
                        Some('{') => s.push('{'),
                        Some('}') => s.push('}'),
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
            "true" => Token::True,
            "false" => Token::False,
            "null" => Token::Null,
            "in" => Token::In,
            "break" => Token::Break,
            "continue" => Token::Continue,
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
                self.advance();
                Token::Plus
            }
            '-' => {
                self.advance();
                if self.peek() == Some('>') {
                    self.advance();
                    Token::Arrow
                } else {
                    Token::Minus
                }
            }
            '*' => {
                self.advance();
                Token::Star
            }
            '/' => {
                self.advance();
                Token::Slash
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
                } else {
                    Token::Lt
                }
            }
            '>' => {
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    Token::GtEq
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
                    Token::DotDot
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

        Ok(Some(TokenWithPos::new(token, line, column)))
    }
}

/// Convierte código fuente en una lista de tokens con posición.
/// Siempre termina con un `Token::EOF`.
pub fn tokenize(source: &str) -> FitzResult<Vec<TokenWithPos>> {
    let mut lexer = Lexer::new(source);
    let mut tokens = Vec::new();
    while let Some(tok) = lexer.next_token()? {
        tokens.push(tok);
    }
    tokens.push(TokenWithPos::new(Token::EOF, lexer.line, lexer.column));
    Ok(tokens)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
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
            ("from", Token::From),
            ("true", Token::True),
            ("false", Token::False),
            ("null", Token::Null),
            ("in", Token::In),
            ("break", Token::Break),
            ("continue", Token::Continue),
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
}
