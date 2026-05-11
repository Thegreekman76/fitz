// lexer.rs — Fase 2.1
//
// El lexer convierte el código fuente en una lista de tokens.
//
// Ejemplo:
//   input:  "let x = 42 + 1"
//   output: [Let, Ident("x"), Eq, Int(42), Plus, Int(1), EOF]
//
// TODO: implementar en Fase 2

/// Representa un token del lenguaje Fitz
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Literales
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,

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

    // Decoradores HTTP
    Get(String),     // @get("/ruta")
    Post(String),    // @post("/ruta")
    Put(String),     // @put("/ruta")
    Delete(String),  // @delete("/ruta")

    // Operadores
    Plus,       // +
    Minus,      // -
    Star,       // *
    Slash,      // /
    Eq,         // =
    EqEq,       // ==
    NotEq,      // !=
    Lt,         // <
    LtEq,       // <=
    Gt,         // >
    GtEq,       // >=
    Arrow,      // ->
    FatArrow,   // =>
    Question,   // ?

    // Delimitadores
    LParen,     // (
    RParen,     // )
    LBrace,     // {
    RBrace,     // }
    LBracket,   // [
    RBracket,   // ]
    Comma,      // ,
    Colon,      // :
    Dot,        // .

    // Especiales
    Newline,
    EOF,
}

/// Convierte código fuente en una lista de tokens
pub fn tokenize(_source: &str) -> Vec<Token> {
    // TODO: implementar en Fase 2
    vec![Token::EOF]
}
