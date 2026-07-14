// view/lexer.rs — dedicated lexer for `.fitzv` Single-File Components.
//
// **Isolated from the classic lexer** (`crate::lexer`). Shares no
// state with it. Classic Fitz rules like triple-quoted strings,
// `\u{...}` escapes, `//`/`/* */` comments, or byte literals
// (`b"..."`) **do not apply inside `.fitzv`** — a `.fitzv` file is
// its own dialect.
//
// This lexer is char-by-char, no regex, no allocations per token
// beyond the payload. When it sees `<template>` or `<style>` it
// switches to raw mode and captures the content up to the matching
// closing tag; the HTML parser and CSS parser work on that blob.
//
// POC scope:
//   - Keywords: `component`, `state`, `event`
//   - Identifiers, string literals `"..."`, delimiters `{ } ( ) , ; :`
//   - Number literals are emitted as `Ident` in the POC — defaults
//     are captured as raw blobs by the parser, so distinguishing
//     `42` from `foo` at the token level buys nothing here.
//   - Line comments `// ...`; silently skipped.
//   - `<template>...</template>` and `<style scoped>...</style>` as
//     single raw tokens.
//   - `<`, `>`, `?`, `[`, `]` — emitted as `Lt`, `Gt`, `Question`,
//     `LBracket`, `RBracket` when they don't open a `<template>` /
//     `<style scoped>` block. Purpose: let state field type
//     annotations AND their defaults carry classic Fitz shapes like
//     `xs: List<Str> = []`, `m: Map<K, V> = {}`, `subtitle: Str? = null`,
//     `r: Result<T, Str> = Ok(0)` through the raw-blob capture in the
//     shell parser, so `expand` (11.2.a) can re-parse them with the
//     classic parser. The shell parser never accepts these tokens at
//     the top level of a component — the "expected `state`, `event`,
//     `<template>` or `<style scoped>`" error path takes over there,
//     so `<foo />` in a component body still errors, just from the
//     parser rather than the lexer.

use std::fmt;

/// Token kind. Position lives outside in `TokenWithLoc`.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Keywords
    Component,
    State,
    Event,

    // Identifiers + literals
    Ident(String),
    /// A double-quoted string. No exotic escapes and no interpolation
    /// in the POC — only `\\` and `\"` so a literal `"` inside a
    /// default like `"He said \"hi\""` survives.
    Str(String),

    // Delimiters
    LBrace,   // {
    RBrace,   // }
    LParen,   // (
    RParen,   // )
    LBracket, // [
    RBracket, // ]
    Comma,    // ,
    Colon,    // :
    Semi,     // ; (optional; the parser treats it as a soft separator)
    Eq,       // =
    Lt,       // < (only when not opening a `<template>`/`<style scoped>` block)
    Gt,       // >
    Question, // ?

    // Blocks captured raw by the lexer.
    /// Raw content between `<template>` and `</template>` — without
    /// the tags. The HTML parser re-lexes it internally.
    TemplateRaw(String),
    /// Raw content between `<style scoped>` and `</style>`. `scoped`
    /// is mandatory in the POC (documented in the AST).
    StyleScopedRaw(String),

    Newline,
    Eof,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Component => write!(f, "`component`"),
            Token::State => write!(f, "`state`"),
            Token::Event => write!(f, "`event`"),
            Token::Ident(s) => write!(f, "identifier `{s}`"),
            Token::Str(_) => write!(f, "string literal"),
            Token::LBrace => write!(f, "`{{`"),
            Token::RBrace => write!(f, "`}}`"),
            Token::LParen => write!(f, "`(`"),
            Token::RParen => write!(f, "`)`"),
            Token::LBracket => write!(f, "`[`"),
            Token::RBracket => write!(f, "`]`"),
            Token::Comma => write!(f, "`,`"),
            Token::Colon => write!(f, "`:`"),
            Token::Semi => write!(f, "`;`"),
            Token::Eq => write!(f, "`=`"),
            Token::Lt => write!(f, "`<`"),
            Token::Gt => write!(f, "`>`"),
            Token::Question => write!(f, "`?`"),
            Token::TemplateRaw(_) => write!(f, "<template> block"),
            Token::StyleScopedRaw(_) => write!(f, "<style scoped> block"),
            Token::Newline => write!(f, "newline"),
            Token::Eof => write!(f, "end of file"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokenWithLoc {
    pub token: Token,
    pub line: usize,
    pub column: usize,
}

/// Errors from the view lexer. Deliberately separate from the
/// classic `FitzError` to make it explicit that this is a new
/// dialect, and so the POC does not have to decide yet how view
/// errors surface through the classic pipeline.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewLexError {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

impl fmt::Display for ViewLexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "view lex error at {}:{} — {}",
            self.line, self.column, self.message
        )
    }
}

impl std::error::Error for ViewLexError {}

pub type ViewLexResult<T> = Result<T, ViewLexError>;

pub fn tokenize(source: &str) -> ViewLexResult<Vec<TokenWithLoc>> {
    let mut lexer = ViewLexer::new(source);
    lexer.run()?;
    Ok(lexer.tokens)
}

struct ViewLexer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    column: usize,
    tokens: Vec<TokenWithLoc>,
}

impl ViewLexer {
    fn new(source: &str) -> Self {
        Self {
            chars: source.chars().collect(),
            pos: 0,
            line: 1,
            column: 1,
            tokens: Vec::new(),
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

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

    /// `true` iff the next chars starting at `self.pos` match
    /// `expected` exactly (case-sensitive). Does not consume.
    fn starts_with(&self, expected: &str) -> bool {
        let mut offset = 0;
        for want in expected.chars() {
            match self.chars.get(self.pos + offset) {
                Some(got) if *got == want => offset += 1,
                _ => return false,
            }
        }
        true
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            match self.peek() {
                Some(' ') | Some('\t') | Some('\r') => {
                    self.advance();
                }
                Some('/') if self.peek_at(1) == Some('/') => {
                    // Line comment — consume up to (but not
                    // including) the newline.
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.advance();
                    }
                }
                _ => break,
            }
        }
    }

    fn run(&mut self) -> ViewLexResult<()> {
        while self.pos < self.chars.len() {
            self.skip_ws_and_comments();

            let start_line = self.line;
            let start_col = self.column;

            let c = match self.peek() {
                Some(c) => c,
                None => break,
            };

            // Detect raw blocks BEFORE anything else. A `<` followed
            // by `template>` or `style scoped>` switches modes and
            // captures up to the matching closing tag. Any other
            // `<` is a plain `Lt` token — needed so state field
            // defaults can carry `List<T>` / `Map<K, V>` / `Result<T, E>`
            // through the shell parser's raw-blob capture. The shell
            // parser rejects `Lt` at the top level of a component with
            // the "expected state/event/..." error, so `<foo/>` in a
            // component body still fails cleanly, just from the parser
            // rather than the lexer.
            if c == '<' {
                if self.starts_with("<template>") {
                    self.consume_template_block(start_line, start_col)?;
                    continue;
                }
                if self.starts_with("<style scoped>") {
                    self.consume_style_block(start_line, start_col)?;
                    continue;
                }
                self.advance();
                self.tokens.push(TokenWithLoc {
                    token: Token::Lt,
                    line: start_line,
                    column: start_col,
                });
                continue;
            }

            match c {
                '\n' => {
                    self.advance();
                    self.tokens.push(TokenWithLoc {
                        token: Token::Newline,
                        line: start_line,
                        column: start_col,
                    });
                }
                '{' => {
                    self.advance();
                    self.tokens.push(TokenWithLoc {
                        token: Token::LBrace,
                        line: start_line,
                        column: start_col,
                    });
                }
                '}' => {
                    self.advance();
                    self.tokens.push(TokenWithLoc {
                        token: Token::RBrace,
                        line: start_line,
                        column: start_col,
                    });
                }
                '(' => {
                    self.advance();
                    self.tokens.push(TokenWithLoc {
                        token: Token::LParen,
                        line: start_line,
                        column: start_col,
                    });
                }
                ')' => {
                    self.advance();
                    self.tokens.push(TokenWithLoc {
                        token: Token::RParen,
                        line: start_line,
                        column: start_col,
                    });
                }
                '[' => {
                    self.advance();
                    self.tokens.push(TokenWithLoc {
                        token: Token::LBracket,
                        line: start_line,
                        column: start_col,
                    });
                }
                ']' => {
                    self.advance();
                    self.tokens.push(TokenWithLoc {
                        token: Token::RBracket,
                        line: start_line,
                        column: start_col,
                    });
                }
                ',' => {
                    self.advance();
                    self.tokens.push(TokenWithLoc {
                        token: Token::Comma,
                        line: start_line,
                        column: start_col,
                    });
                }
                ':' => {
                    self.advance();
                    self.tokens.push(TokenWithLoc {
                        token: Token::Colon,
                        line: start_line,
                        column: start_col,
                    });
                }
                ';' => {
                    self.advance();
                    self.tokens.push(TokenWithLoc {
                        token: Token::Semi,
                        line: start_line,
                        column: start_col,
                    });
                }
                '=' => {
                    self.advance();
                    self.tokens.push(TokenWithLoc {
                        token: Token::Eq,
                        line: start_line,
                        column: start_col,
                    });
                }
                '>' => {
                    self.advance();
                    self.tokens.push(TokenWithLoc {
                        token: Token::Gt,
                        line: start_line,
                        column: start_col,
                    });
                }
                '?' => {
                    self.advance();
                    self.tokens.push(TokenWithLoc {
                        token: Token::Question,
                        line: start_line,
                        column: start_col,
                    });
                }
                '"' => {
                    let s = self.read_string(start_line, start_col)?;
                    self.tokens.push(TokenWithLoc {
                        token: Token::Str(s),
                        line: start_line,
                        column: start_col,
                    });
                }
                c if is_ident_start(c) => {
                    let s = self.read_ident();
                    let token = match s.as_str() {
                        "component" => Token::Component,
                        "state" => Token::State,
                        "event" => Token::Event,
                        _ => Token::Ident(s),
                    };
                    self.tokens.push(TokenWithLoc {
                        token,
                        line: start_line,
                        column: start_col,
                    });
                }
                c if c.is_ascii_digit() => {
                    // POC: emit digits + subsequent ident chars as
                    // Ident. Defaults are captured as raw blobs by
                    // the parser; the shell (state / event) never
                    // encounters numbers directly.
                    let s = self.read_ident();
                    self.tokens.push(TokenWithLoc {
                        token: Token::Ident(s),
                        line: start_line,
                        column: start_col,
                    });
                }
                other => {
                    return Err(ViewLexError {
                        message: format!(
                            "unexpected character `{other}` at the top level of a component"
                        ),
                        line: start_line,
                        column: start_col,
                    });
                }
            }
        }

        self.tokens.push(TokenWithLoc {
            token: Token::Eof,
            line: self.line,
            column: self.column,
        });
        Ok(())
    }

    fn read_ident(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if is_ident_cont(c) {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        s
    }

    fn read_string(&mut self, start_line: usize, start_col: usize) -> ViewLexResult<String> {
        self.advance(); // consume opening "
        let mut s = String::new();
        loop {
            match self.peek() {
                Some('"') => {
                    self.advance();
                    return Ok(s);
                }
                Some('\\') => {
                    self.advance();
                    match self.advance() {
                        Some('n') => s.push('\n'),
                        Some('t') => s.push('\t'),
                        Some('r') => s.push('\r'),
                        Some('\\') => s.push('\\'),
                        Some('"') => s.push('"'),
                        Some(other) => {
                            return Err(ViewLexError {
                                message: format!(
                                    "unknown escape sequence `\\{other}` in string literal"
                                ),
                                line: self.line,
                                column: self.column,
                            });
                        }
                        None => {
                            return Err(ViewLexError {
                                message: "unterminated string — ended after `\\`".into(),
                                line: start_line,
                                column: start_col,
                            });
                        }
                    }
                }
                Some('\n') => {
                    return Err(ViewLexError {
                        message: "unterminated string — newline before closing `\"`".into(),
                        line: start_line,
                        column: start_col,
                    });
                }
                Some(c) => {
                    s.push(c);
                    self.advance();
                }
                None => {
                    return Err(ViewLexError {
                        message: "unterminated string — reached end of file".into(),
                        line: start_line,
                        column: start_col,
                    });
                }
            }
        }
    }

    fn consume_template_block(&mut self, start_line: usize, start_col: usize) -> ViewLexResult<()> {
        // Consume `<template>` — fixed 10 chars.
        for _ in 0.."<template>".len() {
            self.advance();
        }
        let mut body = String::new();
        loop {
            if self.starts_with("</template>") {
                for _ in 0.."</template>".len() {
                    self.advance();
                }
                self.tokens.push(TokenWithLoc {
                    token: Token::TemplateRaw(body),
                    line: start_line,
                    column: start_col,
                });
                return Ok(());
            }
            match self.advance() {
                Some(c) => body.push(c),
                None => {
                    return Err(ViewLexError {
                        message: "unterminated `<template>` block — expected `</template>`".into(),
                        line: start_line,
                        column: start_col,
                    });
                }
            }
        }
    }

    fn consume_style_block(&mut self, start_line: usize, start_col: usize) -> ViewLexResult<()> {
        // Consume `<style scoped>` — fixed 14 chars.
        for _ in 0.."<style scoped>".len() {
            self.advance();
        }
        let mut body = String::new();
        loop {
            if self.starts_with("</style>") {
                for _ in 0.."</style>".len() {
                    self.advance();
                }
                self.tokens.push(TokenWithLoc {
                    token: Token::StyleScopedRaw(body),
                    line: start_line,
                    column: start_col,
                });
                return Ok(());
            }
            match self.advance() {
                Some(c) => body.push(c),
                None => {
                    return Err(ViewLexError {
                        message: "unterminated `<style scoped>` block — expected `</style>`".into(),
                        line: start_line,
                        column: start_col,
                    });
                }
            }
        }
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_cont(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_pos(tokens: Vec<TokenWithLoc>) -> Vec<Token> {
        tokens
            .into_iter()
            .map(|t| t.token)
            .filter(|t| !matches!(t, Token::Newline))
            .collect()
    }

    #[test]
    fn tokenizes_empty_component_shell() {
        let src = "component Card {}";
        let toks = strip_pos(tokenize(src).unwrap());
        assert_eq!(
            toks,
            vec![
                Token::Component,
                Token::Ident("Card".into()),
                Token::LBrace,
                Token::RBrace,
                Token::Eof
            ]
        );
    }

    #[test]
    fn tokenizes_state_and_event_keywords() {
        let src = "component X { state { } event go() { } }";
        let toks = strip_pos(tokenize(src).unwrap());
        assert!(toks.contains(&Token::State));
        assert!(toks.contains(&Token::Event));
        // `go` is NOT a keyword — it stays as a plain ident.
        assert!(toks.contains(&Token::Ident("go".into())));
    }

    #[test]
    fn captures_template_block_as_raw_string() {
        let src = "component X { <template><div>hi</div></template> }";
        let toks = tokenize(src).unwrap();
        let template_tok = toks.iter().find_map(|t| match &t.token {
            Token::TemplateRaw(s) => Some(s.clone()),
            _ => None,
        });
        assert_eq!(template_tok.as_deref(), Some("<div>hi</div>"));
    }

    #[test]
    fn captures_style_scoped_block_as_raw_string() {
        let src = "component X { <style scoped>.a { color: red; }</style> }";
        let toks = tokenize(src).unwrap();
        let style_tok = toks.iter().find_map(|t| match &t.token {
            Token::StyleScopedRaw(s) => Some(s.clone()),
            _ => None,
        });
        assert_eq!(style_tok.as_deref(), Some(".a { color: red; }"));
    }

    #[test]
    fn unknown_lt_at_top_level_lexes_as_lt_token() {
        // Since view-lexer §7, the lexer no longer errors on a
        // `<` that isn't a `<template>` / `<style scoped>` opener —
        // it emits `Token::Lt`. The shell parser is the one that
        // rejects `Lt` at the top level of a component with its
        // "expected state/event/..." error path; that guard is
        // covered separately in `parser.rs::tests`.
        //
        // Uses `<foo` (no `/>`) since `/` still isn't tokenized —
        // that would trigger a real "unexpected character" from
        // the lexer and mask what this test is proving.
        let src = "component X { <foo }";
        let toks = strip_pos(tokenize(src).unwrap());
        assert!(
            toks.contains(&Token::Lt),
            "expected `Token::Lt` in {:?}",
            toks
        );
        assert!(toks.contains(&Token::Ident("foo".into())));
    }

    #[test]
    fn tokenizes_generic_type_expr_list_of_str() {
        // `List<Str>` inside a state field default must produce
        // Ident("List"), Lt, Ident("Str"), Gt so the raw capture
        // + `expand` can reconstruct the classic Fitz `TypeExpr`.
        let src = "component X { state { xs: List<Str> = [] } }";
        let toks = strip_pos(tokenize(src).unwrap());
        // Locate the sub-sequence around `List<Str>`.
        let list_idx = toks
            .iter()
            .position(|t| matches!(t, Token::Ident(s) if s == "List"))
            .expect("`List` ident");
        assert_eq!(toks[list_idx], Token::Ident("List".into()));
        assert_eq!(toks[list_idx + 1], Token::Lt);
        assert_eq!(toks[list_idx + 2], Token::Ident("Str".into()));
        assert_eq!(toks[list_idx + 3], Token::Gt);
    }

    #[test]
    fn tokenizes_nullable_type_expr_str_question() {
        // `Str?` must produce Ident("Str"), Question.
        let src = "component X { state { subtitle: Str? = null } }";
        let toks = strip_pos(tokenize(src).unwrap());
        let str_idx = toks
            .iter()
            .position(|t| matches!(t, Token::Ident(s) if s == "Str"))
            .expect("`Str` ident");
        assert_eq!(toks[str_idx], Token::Ident("Str".into()));
        assert_eq!(toks[str_idx + 1], Token::Question);
    }

    #[test]
    fn tokenizes_map_generic_with_two_args() {
        // `Map<Str, Int>` must produce Ident("Map"), Lt,
        // Ident("Str"), Comma, Ident("Int"), Gt.
        let src = "component X { state { meta: Map<Str, Int> = {} } }";
        let toks = strip_pos(tokenize(src).unwrap());
        let map_idx = toks
            .iter()
            .position(|t| matches!(t, Token::Ident(s) if s == "Map"))
            .expect("`Map` ident");
        assert_eq!(toks[map_idx], Token::Ident("Map".into()));
        assert_eq!(toks[map_idx + 1], Token::Lt);
        assert_eq!(toks[map_idx + 2], Token::Ident("Str".into()));
        assert_eq!(toks[map_idx + 3], Token::Comma);
        assert_eq!(toks[map_idx + 4], Token::Ident("Int".into()));
        assert_eq!(toks[map_idx + 5], Token::Gt);
    }

    #[test]
    fn template_and_style_blocks_still_detected_before_lt_fallback() {
        // A `<template>` opener must still route to raw-block
        // capture, even though `<` on its own now emits `Lt`. This
        // is the ordering invariant of `run()`.
        let src = "component X { <template><div>hi</div></template> }";
        let toks = tokenize(src).unwrap();
        let template_tok = toks.iter().find_map(|t| match &t.token {
            Token::TemplateRaw(s) => Some(s.clone()),
            _ => None,
        });
        assert_eq!(template_tok.as_deref(), Some("<div>hi</div>"));
        // Ensure no stray `Token::Lt` slipped in.
        assert!(
            !toks.iter().any(|t| matches!(t.token, Token::Lt)),
            "unexpected Lt token when `<template>` should have been detected: {:?}",
            toks
        );
    }

    #[test]
    fn line_comment_is_skipped() {
        let src = "component X { // a comment here\n }";
        let toks = strip_pos(tokenize(src).unwrap());
        assert_eq!(
            toks,
            vec![
                Token::Component,
                Token::Ident("X".into()),
                Token::LBrace,
                Token::RBrace,
                Token::Eof
            ]
        );
    }

    #[test]
    fn string_literal_with_escapes() {
        let src = r#"component X { state { title: Str = "hello \"world\"" } }"#;
        let toks = strip_pos(tokenize(src).unwrap());
        assert!(toks
            .iter()
            .any(|t| matches!(t, Token::Str(s) if s == "hello \"world\"")));
    }
}
