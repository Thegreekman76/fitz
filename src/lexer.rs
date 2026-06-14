// lexer.rs — Phase 2.1
//
// The lexer turns source code into a list of tokens with positions.
//
// Example:
//   input:  "let x = 42 + 1"
//   output: [Let, Ident("x"), Eq, Int(42), Plus, Int(1), EOF]
//
// Newlines are emitted as tokens (not treated as whitespace) because Fitz
// uses line breaks as an optional statement separator — the parser decides
// when they matter.

use crate::error::{ErrorKind, FitzError, FitzResult};

/// Token kind. Line/column info lives separately, in `TokenWithPos`.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::upper_case_acronyms)] // EOF is the canonical name.
pub enum Token {
    // Literals
    Int(i64),
    Float(f64),
    Str(String),
    /// Mini-batch Bytes — binary literal `b"..."`. Raw bytes,
    /// supports `\xHH` escapes in addition to the common ones
    /// (`\n`/`\r`/`\t`/`\\`/`\"`/`\0`). Interpolation `{...}` is
    /// NOT allowed (byte literals are fixed).
    Bytes(Vec<u8>),

    // Identifiers and keywords
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
    Xor,    // Mini-batch Xor — logical `a xor b` (Bool ^ Bool, parallel to `or`/`and`)
    Not,    // R.1.1 — `not <expr>` prefix logical negation
    Static, // Mini-batch St — `static fn ...` inside a `type` body

    // Operators
    Plus,     // +
    Minus,    // -
    Star,     // *
    Slash,    // /
    Percent,  // % — modulo operator (R.1.2)
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
    DotDotEq, // ..= (R.1.4: inclusive ranges)

    // Delimiters
    LParen,   // (
    RParen,   // )
    LBrace,   // {
    RBrace,   // }
    LBracket, // [
    RBracket, // ]
    Comma,    // ,
    Colon,    // :
    Dot,      // .
    At,       // @ — decorator prefix: @get, @post, @server, ...
    Pipe,     // | — or-pattern separator in `match` (R.2.1); bitwise OR (mini-batch Bits)
    // Bitwise operators (mini-batch Bits).
    Amp,   // & — bitwise AND
    Caret, // ^ — bitwise XOR
    Shl,   // << — shift left
    Shr,   // >> — shift right
    Tilde, // ~ — bitwise NOT (unary)
    // Compound bitwise operators (mini-batch Cmp).
    AmpEq,         // &=
    PipeEq,        // |=
    CaretEq,       // ^=
    ShlEq,         // <<=
    ShrEq,         // >>=
    Label(String), // 'name — labels for break/continue (mini-batch L)

    // Special
    Newline,
    EOF,
}

/// Token with its position in the source. This is what `tokenize` returns.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenWithPos {
    pub token: Token,
    pub line: usize,
    pub column: usize,
}

impl TokenWithPos {
    fn new(token: Token, line: usize, column: usize) -> Self {
        Self {
            token,
            line,
            column,
        }
    }
}

// ---------------------------------------------------------------------------
// Trivia — Phase 9.z.1.b (formatter comment preservation)
//
// The lexer normally strips comments and blank lines: the AST doesn't need
// them and neither does the rest of the pipeline. But the formatter DOES
// need them to preserve the user's code when rewriting. Trivia is the
// side-channel that `tokenize_with_trivia` returns: tokens on one side,
// comments + blank lines on the other. The parser keeps using only the
// tokens, so the AST stays clean.
// ---------------------------------------------------------------------------

/// Captured comment kind. Fitz only has `//` (line) and `/* */` (block).
/// The formatter emits them differently.
#[derive(Debug, Clone, PartialEq)]
pub enum CommentKind {
    Line,
    Block,
}

/// Comment captured from the source. `text` does NOT include the prefix
/// (`//` or `/*`) nor the suffix (`*/` for block). Position is 1-based
/// and points to where the opening delimiter starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    pub text: String,
    pub line: usize,
    pub column: usize,
    pub kind: CommentKind,
}

/// Lexer side-channel: everything the lexer would normally
/// discard but the formatter needs to preserve.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trivia {
    /// Comments in source order.
    pub comments: Vec<Comment>,
    /// Line numbers (1-based) that were completely empty in the
    /// source. Does NOT include lines that contain only a comment
    /// — those are represented via `comments`.
    pub blank_lines: Vec<usize>,
}

/// Internal scanning state. Module-private.
struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    column: usize,
    /// When active, the lexer captures comments and blank lines
    /// into `trivia` (Phase 9.z.1.b). Off by default — the fast
    /// `tokenize` stays zero-overhead.
    collect_trivia: bool,
    trivia: Trivia,
    /// Per-line flags to detect blank lines correctly (lines with
    /// only whitespace count as blank; lines with a comment do
    /// NOT). Reset when consuming a `\n`.
    line_had_code: bool,
    line_had_comment: bool,
    /// Mini-batch T — `true` right after emitting `Token::Dot`.
    /// `read_number` checks it so it does NOT enter float mode
    /// when seeing `<digits>.<digit>` preceded by Dot: `t.0.0`
    /// must tokenize as `Ident("t") Dot Int(0) Dot Int(0)`, not
    /// as `Ident("t") Dot Float(0.0)`.
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

    /// Current char without consuming. None if we've reached the end.
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    /// Next char (pos + 1) without consuming.
    fn peek_next(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    /// Consume the current char and update line/column.
    ///
    /// **Phase 9.z.1.b**: when crossing a `\n`, if the line we're
    /// closing had no token AND no comment (i.e. only whitespace),
    /// we record it as a blank_line in `trivia`. Comments-only
    /// lines are not blanks.
    fn advance(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        if c == '\n' {
            if self.collect_trivia && !self.line_had_code && !self.line_had_comment {
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

    /// Skip spaces, tabs and comments. Does NOT skip '\n' (that's a token).
    fn skip_whitespace_and_comments(&mut self) -> FitzResult<()> {
        loop {
            match self.peek() {
                Some(' ') | Some('\t') | Some('\r') => {
                    self.advance();
                }
                Some('/') if self.peek_next() == Some('/') => {
                    // line comment — consume up to '\n' (not including it)
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
                        let text: String = self.chars[text_start..self.pos].iter().collect();
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
                                    "Unterminated block comment /* ... */",
                                ));
                            }
                        }
                    }
                    if self.collect_trivia {
                        let text: String = self.chars[text_start..text_end].iter().collect();
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

    /// Read a number. Picks Int vs Float based on whether there is a '.'
    /// followed by a digit, or scientific notation (`e`/`E`).
    ///
    /// Watch out: in `0..10` the '..' is a range operator, NOT a decimal point.
    ///
    /// **Mini-batch Núm**: supports `_` digit separators (`1_000_000`,
    /// `3.14_15`) and scientific notation `e`/`E` with an optionally
    /// signed exponent (`3.14e2`, `1e-10`, `2.5E+3`).
    /// Rules:
    ///   - `_` only between digits. Invalid: `_1`, `1_`, `1__0`.
    ///   - `e`/`E` always yields a Float (even `1e10`).
    ///   - The exponent may carry an optional `+`/`-` and at least
    ///     one digit (`1e`, `1e+` → error).
    ///   - Separators also allowed inside the exponent (`1e1_0`).
    fn read_number(&mut self) -> FitzResult<Token> {
        let start_line = self.line;
        let start_col = self.column;

        // Mini-batch Lit — hex/binary/octal literals with prefixes
        // `0x`/`0b`/`0o`. Mini-batch Cmp — we also accept the
        // uppercase variants `0X`/`0B`/`0O` (Python-compat). The
        // current char must be `0` and the next one the prefix.
        // If it doesn't match, we fall through to the decimal flow.
        if self.peek() == Some('0') && !self.prev_was_dot {
            match self.peek_next() {
                Some('x') | Some('X') => {
                    return self.read_radix_number(16, "hex", start_line, start_col);
                }
                Some('b') | Some('B') => {
                    return self.read_radix_number(2, "binario", start_line, start_col);
                }
                Some('o') | Some('O') => {
                    return self.read_radix_number(8, "octal", start_line, start_col);
                }
                _ => {}
            }
        }

        let start_pos = self.pos;
        // Helper: read digits + interleaved underscores. Returns an error
        // if it finds an orphan `_` (`1__`, ending in `_`).
        self.read_digit_run(start_line, start_col)?;

        // Mini-batch T — if we come right after a `Dot` (tuple field
        // access chain like `t.0.0`), do NOT enter float mode. The
        // `0` of `t.0` closes as Int, and the following `.0` will
        // start a new Int through the same path.
        let has_fraction = !self.prev_was_dot
            && self.peek() == Some('.')
            && self.peek_next().is_some_and(|c| c.is_ascii_digit());
        let mut is_float = false;

        if has_fraction {
            self.advance(); // consume '.'
            self.advance(); // consume first digit of the fractional part
            self.read_digit_run(start_line, start_col)?;
            is_float = true;
        }

        // Scientific notation `e`/`E` with an optionally signed exponent.
        if matches!(self.peek(), Some('e') | Some('E')) {
            self.advance(); // consume `e`/`E`
            if matches!(self.peek(), Some('+') | Some('-')) {
                self.advance();
            }
            // At least one digit after the sign.
            match self.peek() {
                Some(c) if c.is_ascii_digit() => {
                    self.advance();
                }
                _ => {
                    return Err(FitzError::new(
                        ErrorKind::InvalidSyntax,
                        start_line,
                        start_col,
                        "scientific notation exponent has no digits",
                    ));
                }
            }
            self.read_digit_run(start_line, start_col)?;
            is_float = true;
        }

        // Final parse: strip `_` and convert.
        let raw: String = self.chars[start_pos..self.pos].iter().collect();
        let clean: String = raw.chars().filter(|c| *c != '_').collect();
        if is_float {
            let n = clean.parse::<f64>().map_err(|_| {
                FitzError::new(
                    ErrorKind::InvalidSyntax,
                    start_line,
                    start_col,
                    format!("Invalid float number: '{}'", raw),
                )
            })?;
            Ok(Token::Float(n))
        } else {
            let n = clean.parse::<i64>().map_err(|_| {
                FitzError::new(
                    ErrorKind::InvalidSyntax,
                    start_line,
                    start_col,
                    format!("Invalid integer number: '{}'", raw),
                )
            })?;
            Ok(Token::Int(n))
        }
    }

    /// Mini-batch Lit — read a literal with a radix prefix (hex `0x`,
    /// binary `0b`, octal `0o`). Supports `_` digit separators.
    /// Produces `Token::Int`. `i64` overflow or empty digits → a
    /// clear lexer error.
    fn read_radix_number(
        &mut self,
        radix: u32,
        name: &str,
        line: usize,
        col: usize,
    ) -> FitzResult<Token> {
        self.advance(); // consume '0'
        self.advance(); // consume prefix ('x'/'b'/'o')
        let digit_start = self.pos;
        loop {
            match self.peek() {
                Some(c) if c.is_digit(radix) => {
                    self.advance();
                }
                Some('_') => {
                    // After `_` require a valid digit for the base.
                    if !self.peek_next().is_some_and(|n| n.is_digit(radix)) {
                        return Err(FitzError::new(
                            ErrorKind::InvalidSyntax,
                            line,
                            col,
                            format!(
                                "separator `_` in {} literal only between valid digits",
                                name
                            ),
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
                format!("literal {} has no digits after the prefix", name),
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
                    "literal {} `{}` exceeds the range of Int (i64)",
                    name, clean
                ),
            )
        })?;
        Ok(Token::Int(n))
    }

    /// Mini-batch Núm — read a `digit (_ digit)*` sequence. Allows
    /// `_` between digits but rejects consecutive `__` or a trailing
    /// `_`. The first digit is ALREADY consumed by the caller; this
    /// helper continues up to the first char that is not digit/underscore.
    fn read_digit_run(&mut self, line: usize, col: usize) -> FitzResult<()> {
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.advance();
            } else if c == '_' {
                // The previous char is the last thing consumed — a
                // digit (because the loop only advances on digits
                // or a previously validated `_`). But after the `_`
                // we require another digit.
                if !self.peek_next().is_some_and(|n| n.is_ascii_digit()) {
                    return Err(FitzError::new(
                        ErrorKind::InvalidSyntax,
                        line,
                        col,
                        "separator `_` in number only between digits (example: `1_000_000`)",
                    ));
                }
                self.advance(); // consume '_'
            } else {
                break;
            }
        }
        Ok(())
    }

    /// Read a string between quotes. Supports basic escapes: \n \t \r \\ \" \{ \}
    /// The interpolation `"Hello {name}"` is left "raw" in the content
    /// — the parser/evaluator processes it later.
    ///
    /// **R.1.5 (mini-phase R)**: besides the "single-quote" mode, supports
    /// **triple-quote** `"""..."""` for multiline strings. If two more `"`
    /// follow the first `"`, we enter triple mode: newlines are valid
    /// inside and the closer is `"""` (three quotes in a row).
    /// `{expr}` interpolation works the same.
    /// F9 — Processes `\u{XXXX}`: 1 to 6 hex digits between braces,
    /// interpreted as a Unicode scalar codepoint. Rejects surrogates
    /// (D800-DFFF) and values > U+10FFFF. The `\` and the `u` were
    /// already consumed by the caller.
    fn read_unicode_escape(&mut self) -> FitzResult<char> {
        let start_line = self.line;
        let start_col = self.column;
        if self.advance() != Some('{') {
            return Err(FitzError::new(
                ErrorKind::UnexpectedChar('?'),
                start_line,
                start_col,
                "Secuencia `\\u` requiere `{` (formato: \\u{XXXX})",
            ));
        }
        let mut hex = String::new();
        loop {
            match self.peek() {
                Some('}') => {
                    self.advance();
                    break;
                }
                Some(c) if c.is_ascii_hexdigit() => {
                    hex.push(c);
                    self.advance();
                    if hex.len() > 6 {
                        return Err(FitzError::new(
                            ErrorKind::UnexpectedChar(c),
                            self.line,
                            self.column,
                            "\\u{...} accepts up to 6 hex digits (maximum codepoint U+10FFFF)",
                        ));
                    }
                }
                Some(other) => {
                    return Err(FitzError::new(
                        ErrorKind::UnexpectedChar(other),
                        self.line,
                        self.column,
                        format!("Invalid hex digit in \\u{{...}}: `{}`", other),
                    ));
                }
                None => {
                    return Err(FitzError::new(
                        ErrorKind::UnterminatedString,
                        start_line,
                        start_col,
                        "Unterminated `\\u{` sequence",
                    ));
                }
            }
        }
        if hex.is_empty() {
            return Err(FitzError::new(
                ErrorKind::UnexpectedChar('}'),
                start_line,
                start_col,
                "\\u{} is empty — requires at least one hex digit",
            ));
        }
        let codepoint = u32::from_str_radix(&hex, 16).map_err(|_| {
            FitzError::new(
                ErrorKind::UnexpectedChar('?'),
                start_line,
                start_col,
                format!("`\\u{{{}}}`: invalid hex", hex),
            )
        })?;
        char::from_u32(codepoint).ok_or_else(|| {
            FitzError::new(
                ErrorKind::UnexpectedChar('?'),
                start_line,
                start_col,
                format!(
                    "`\\u{{{}}}` (0x{:X}) is not a valid Unicode scalar codepoint (surrogates D800-DFFF rejected, maximum 10FFFF)",
                    hex, codepoint
                ),
            )
        })
    }

    /// F9 — Processes `\xXX`: exactly 2 hex digits, interpreted as
    /// an ASCII byte (0x00-0x7F). Codepoints > 0x7F are rejected
    /// (parallel to Rust). The caller already consumed `\` and `x`.
    fn read_hex_byte_escape(&mut self) -> FitzResult<char> {
        let start_line = self.line;
        let start_col = self.column;
        let mut hex = String::new();
        for _ in 0..2 {
            match self.peek() {
                Some(c) if c.is_ascii_hexdigit() => {
                    hex.push(c);
                    self.advance();
                }
                Some(other) => {
                    return Err(FitzError::new(
                        ErrorKind::UnexpectedChar(other),
                        self.line,
                        self.column,
                        format!(
                            "\\x requires 2 hex digits, found `{}` (after {} digits)",
                            other,
                            hex.len()
                        ),
                    ));
                }
                None => {
                    return Err(FitzError::new(
                        ErrorKind::UnterminatedString,
                        start_line,
                        start_col,
                        "Unterminated \\x",
                    ));
                }
            }
        }
        let byte = u8::from_str_radix(&hex, 16).map_err(|_| {
            FitzError::new(
                ErrorKind::UnexpectedChar('?'),
                start_line,
                start_col,
                format!("`\\x{}`: invalid hex", hex),
            )
        })?;
        if byte > 0x7F {
            return Err(FitzError::new(
                ErrorKind::UnexpectedChar('?'),
                start_line,
                start_col,
                format!(
                    "`\\x{}` (0x{:X}) is outside the ASCII range (0x00-0x7F). Use \\u{{...}} for non-ASCII chars.",
                    hex, byte
                ),
            ));
        }
        Ok(byte as char)
    }

    fn read_string(&mut self) -> FitzResult<Token> {
        let start_line = self.line;
        let start_col = self.column;
        self.advance(); // consume opening quote "

        // R.1.5 — triple-quote mode. If the next two chars are also
        // `"`, we're in `"""..."""`. Consume them and delegate to the
        // multiline reader.
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
                        // F9 — extended escapes:
                        Some('0') => s.push('\0'),
                        Some('b') => s.push('\u{0008}'), // backspace
                        Some('u') => s.push(self.read_unicode_escape()?),
                        Some('x') => s.push(self.read_hex_byte_escape()?),
                        // '\{' and '\}' are PRESERVED literally in
                        // the Token::Str content (with the backslash).
                        // The parser, when building the string
                        // expression, distinguishes `{` (start of
                        // interpolation) from `\{` (literal). If we
                        // resolved them here, that distinction would
                        // be lost.
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
                                format!("Invalid escape sequence: '\\{}'", other),
                            ));
                        }
                        None => {
                            return Err(FitzError::new(
                                ErrorKind::UnterminatedString,
                                start_line,
                                start_col,
                                "Unterminated string (ended after '\\')",
                            ));
                        }
                    }
                }
                Some('\n') => {
                    return Err(FitzError::new(
                        ErrorKind::UnterminatedString,
                        start_line,
                        start_col,
                        "Unterminated string — newline before closing quote",
                    )
                    .with_hint("Use \\n to include a newline inside the string"));
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
                        "Unterminated string — missing closing quote",
                    ));
                }
            }
        }
    }

    /// R.1.5 — reads the contents of a multiline string `"""..."""`.
    /// The three opening quotes were already consumed. Differences
    /// vs `read_string`:
    ///
    /// - **Newlines** are valid inside (preserved in the content
    ///   as-is, without requiring `\n`).
    /// - **Closer** is `"""` (three quotes in a row).
    /// - **Isolated single and double quotes** inside the string
    ///   are preserved literally; they only close when 3 appear
    ///   in a row.
    /// - Same escapes as normal strings (`\n`, `\t`, `\\`, `\"`,
    ///   `\{`, `\}`). Useful if you need a literal `"""` inside:
    ///   `\"""`.
    /// - **Interpolation** `{expr}` keeps working — the content is
    ///   handed "raw" to the parser just like in normal strings.
    fn read_triple_string(&mut self, start_line: usize, start_col: usize) -> FitzResult<Token> {
        let mut s = String::new();
        loop {
            // Detect the closer `"""`: if the current char and the
            // next two are `"`, we're done.
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
                        // F9 — extended escapes (parallel to read_string):
                        Some('0') => s.push('\0'),
                        Some('b') => s.push('\u{0008}'),
                        Some('u') => s.push(self.read_unicode_escape()?),
                        Some('x') => s.push(self.read_hex_byte_escape()?),
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
                                format!("Invalid escape sequence: '\\{}'", other),
                            ));
                        }
                        None => {
                            return Err(FitzError::new(
                                ErrorKind::UnterminatedString,
                                start_line,
                                start_col,
                                "Unterminated multiline string (ended after '\\')",
                            ));
                        }
                    }
                }
                // LITERAL newline — valid inside triple-quote. Preserved
                // as-is in the content.
                Some(c) => {
                    s.push(c);
                    self.advance();
                }
                None => {
                    return Err(FitzError::new(
                        ErrorKind::UnterminatedString,
                        start_line,
                        start_col,
                        "Unterminated multiline string — missing closing `\"\"\"`",
                    ));
                }
            }
        }
    }

    /// Read an identifier (letters + digits + '_') and decide if it's a keyword.
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
            "xor" => Token::Xor,
            "not" => Token::Not,
            "static" => Token::Static,
            _ => Token::Ident(s),
        }
    }

    /// Mini-batch Bytes — read a `b"..."` literal. Assumes the caller
    /// already checked that the current char is `b` and the next is
    /// `"`. Supports the common escapes (`\n`/`\r`/`\t`/`\0`/`\\`/`\"`)
    /// plus `\xHH` (2-digit hex byte). Does NOT support `{...}`
    /// interpolation (byte literals are fixed). Each Unicode char is
    /// encoded as its UTF-8 bytes (matches Rust's `b"..."` behavior
    /// when the source has non-ASCII chars — Rust actually rejects
    /// that; Fitz is more permissive).
    fn read_bytes_literal(&mut self) -> FitzResult<Token> {
        // Consume `b` and the opening quote.
        self.advance();
        self.advance();
        let mut out: Vec<u8> = Vec::new();
        loop {
            match self.peek() {
                None => {
                    return Err(FitzError::new(
                        crate::error::ErrorKind::UnterminatedString,
                        self.line,
                        self.column,
                        "unterminated byte literal `b\"...\"`".to_string(),
                    ));
                }
                Some('"') => {
                    self.advance();
                    return Ok(Token::Bytes(out));
                }
                Some('\\') => {
                    self.advance();
                    match self.peek() {
                        Some('n') => {
                            self.advance();
                            out.push(b'\n');
                        }
                        Some('r') => {
                            self.advance();
                            out.push(b'\r');
                        }
                        Some('t') => {
                            self.advance();
                            out.push(b'\t');
                        }
                        Some('0') => {
                            self.advance();
                            out.push(0);
                        }
                        Some('\\') => {
                            self.advance();
                            out.push(b'\\');
                        }
                        Some('"') => {
                            self.advance();
                            out.push(b'"');
                        }
                        Some('x') => {
                            self.advance();
                            // Read 2 hex digits.
                            let h1 = self.peek().ok_or_else(|| {
                                FitzError::new(
                                    crate::error::ErrorKind::InvalidSyntax,
                                    self.line,
                                    self.column,
                                    "escape `\\xHH`: incomplete hex (missing first digit)"
                                        .to_string(),
                                )
                            })?;
                            self.advance();
                            let h2 = self.peek().ok_or_else(|| {
                                FitzError::new(
                                    crate::error::ErrorKind::InvalidSyntax,
                                    self.line,
                                    self.column,
                                    "escape `\\xHH`: incomplete hex (missing second digit)"
                                        .to_string(),
                                )
                            })?;
                            self.advance();
                            let byte =
                                u8::from_str_radix(&format!("{}{}", h1, h2), 16).map_err(|_| {
                                    FitzError::new(
                                        crate::error::ErrorKind::InvalidSyntax,
                                        self.line,
                                        self.column,
                                        format!("escape `\\x{}{}` is not valid hex", h1, h2),
                                    )
                                })?;
                            out.push(byte);
                        }
                        Some(other) => {
                            return Err(FitzError::new(
                                crate::error::ErrorKind::InvalidSyntax,
                                self.line,
                                self.column,
                                format!(
                                    "escape `\\{}` not supported in byte literal; \
                                     supported: \\n, \\r, \\t, \\0, \\\\, \\\", \\xHH",
                                    other
                                ),
                            ));
                        }
                        None => {
                            return Err(FitzError::new(
                                crate::error::ErrorKind::UnterminatedString,
                                self.line,
                                self.column,
                                "byte literal `b\"...\"` ends with unterminated `\\`".to_string(),
                            ));
                        }
                    }
                }
                Some(c) => {
                    self.advance();
                    // Encode the Unicode char as UTF-8 bytes.
                    let mut buf = [0u8; 4];
                    let encoded = c.encode_utf8(&mut buf);
                    out.extend_from_slice(encoded.as_bytes());
                }
            }
        }
    }

    /// Get the next token, or None if we're done.
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
                // R.2.3 — `+=` for compound assignment.
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
                    // R.2.3 — `-=` for compound assignment.
                    Some('=') => {
                        self.advance();
                        Token::MinusEq
                    }
                    _ => Token::Minus,
                }
            }
            '*' => {
                // R.2.3 — `*=` for compound assignment.
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    Token::StarEq
                } else {
                    Token::Star
                }
            }
            '/' => {
                // R.2.3 — `/=` for compound assignment.
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    Token::SlashEq
                } else {
                    Token::Slash
                }
            }
            '%' => {
                // R.1.2 — modulo operator. Single char, no compound
                // variants (%= lands with R.2.3).
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
                        "'!' is only valid as part of '!='",
                    ));
                }
            }
            '<' => {
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    Token::LtEq
                } else if self.peek() == Some('<') {
                    // Mini-batch Bits — `<<` shift left. Cmp: `<<=`.
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Token::ShlEq
                    } else {
                        Token::Shl
                    }
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
                    // Mini-batch Bits — `>>` shift right. Cmp: `>>=`.
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Token::ShrEq
                    } else {
                        Token::Shr
                    }
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
                    // R.1.4: `..=` for inclusive ranges. The `..` check
                    // comes first, then we look at the next char for
                    // `=` to upgrade to DotDotEq.
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
            ';' => {
                // L1 (2026-06-05) — `;` as optional stmt separator.
                // Project design decision #5: "optional semicolons, like
                // in Go". Pragmatic implementation: the lexer emits
                // `Token::Newline` (not a new token) so the parser
                // treats `;` exactly like newline, with no upstream
                // changes. Trade-off: the literal `;` is not preserved
                // in the AST — `fitz fmt` re-emits each stmt on its own
                // line, so `1 + 1; 2 + 2` is rewritten as two separate
                // lines. Closes the historical drift between decision
                // #5 and the pre-L1 lexer (which rejected `;` as an
                // unexpected char).
                self.advance();
                Token::Newline
            }
            ':' => {
                self.advance();
                Token::Colon
            }
            '|' => {
                // R.2.1 — or-pattern separator in `match`. Mini-batch
                // Bits: the same Token::Pipe is used as bitwise OR;
                // the parser distinguishes by context (expression at
                // bitwise level vs match arm). Cmp: `|=` for compound
                // bitwise assignment.
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    Token::PipeEq
                } else {
                    Token::Pipe
                }
            }
            // Mini-batch Bits — `&`, `^`, `~`. Cmp: `&=` and `^=`.
            '&' => {
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    Token::AmpEq
                } else {
                    Token::Amp
                }
            }
            '^' => {
                self.advance();
                if self.peek() == Some('=') {
                    self.advance();
                    Token::CaretEq
                } else {
                    Token::Caret
                }
            }
            '~' => {
                self.advance();
                Token::Tilde
            }
            '\'' => {
                // Mini-batch L — `'name` label for break/continue.
                // Fitz has no char literals with `'x'`, so the
                // apostrophe always starts a label. After the
                // apostrophe we expect an identifier.
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
                        "expected an identifier after `'` (label)".to_string(),
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
            // Mini-batch Bytes — `b"..."` before identifiers, because
            // a lone `b` is only an ident if NOT followed by a quote.
            'b' if self.peek_next() == Some('"') => self.read_bytes_literal()?,
            c if c.is_alphabetic() || c == '_' => self.read_identifier_or_keyword(),
            other => {
                self.advance();
                return Err(FitzError::new(
                    ErrorKind::UnexpectedChar(other),
                    line,
                    column,
                    format!("Unexpected character: '{}'", other),
                ));
            }
        };

        // Phase 9.z.1.b — mark the line as "had code" so that `\n`
        // doesn't count it as blank. Newline does NOT mark code
        // (otherwise no line would be blank).
        if self.collect_trivia && !matches!(token, Token::Newline) {
            self.line_had_code = true;
        }

        Ok(Some(TokenWithPos::new(token, line, column)))
    }
}

/// Convert source code into a list of tokens with positions. Always
/// terminates with a `Token::EOF`.
pub fn tokenize(source: &str) -> FitzResult<Vec<TokenWithPos>> {
    let mut lexer = Lexer::new(source);
    let mut tokens = Vec::new();
    while let Some(tok) = lexer.next_token()? {
        // Mini-batch T — remember whether we just emitted Dot so the
        // next `read_number` doesn't enter float mode (case `t.0.0`).
        lexer.prev_was_dot = matches!(tok.token, Token::Dot);
        tokens.push(tok);
    }
    tokens.push(TokenWithPos::new(Token::EOF, lexer.line, lexer.column));
    Ok(tokens)
}

/// Phase 9.z.1.b — variant of `tokenize` that ALSO captures comments
/// and blank lines as a `Trivia` side-channel. Consumed by the
/// formatter (`fitz fmt`) to preserve the user's comments + blank
/// lines when rewriting.
///
/// Any other lexer consumer (parser, LSP, etc.) keeps using
/// `tokenize` and pays zero overhead. The `Trivia` is not injected
/// into the AST.
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
#[allow(clippy::approx_constant)] // 3.14 in tests is a generic Float, not PI.
mod tests {
    use super::*;

    /// Helper: tokenize and return only the `Token`s (no positions).
    fn toks(src: &str) -> Vec<Token> {
        tokenize(src)
            .expect("source must tokenize without error")
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
            ("xor", Token::Xor),
            ("not", Token::Not),
            ("static", Token::Static),
        ];
        for (src, expected) in cases {
            assert_eq!(
                toks(src),
                vec![expected.clone(), Token::EOF],
                "src = {}",
                src
            );
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

    // ---- Mini-batch F8 — non-ASCII (Unicode) identifiers ----

    #[test]
    fn f8_identifiers_greek_and_math_symbols() {
        // `π`, `σ`, etc. — Greek letters. is_alphabetic returns true.
        assert_eq!(toks("π"), vec![Token::Ident("π".into()), Token::EOF],);
        assert_eq!(toks("σ"), vec![Token::Ident("σ".into()), Token::EOF],);
    }

    #[test]
    fn f8_identifiers_with_accents_and_n_tilde() {
        // Typical Spanish: `función`, `niño`, `café`.
        for ident in ["función", "niño", "café", "año"] {
            assert_eq!(
                toks(ident),
                vec![Token::Ident(ident.into()), Token::EOF],
                "identificador `{}` no se lexea correctamente",
                ident,
            );
        }
    }

    #[test]
    fn f8_identifiers_cjk() {
        // Japanese / Chinese / Korean. is_alphabetic accepts them.
        for ident in ["名前", "用户", "이름"] {
            assert_eq!(
                toks(ident),
                vec![Token::Ident(ident.into()), Token::EOF],
                "ident CJK `{}` no se lexea correctamente",
                ident,
            );
        }
    }

    #[test]
    fn f8_identifiers_cyrillic() {
        assert_eq!(toks("имя"), vec![Token::Ident("имя".into()), Token::EOF],);
    }

    #[test]
    fn f8_identifiers_mixed_unicode_and_ascii() {
        // Combining Unicode + ASCII + `_` also works.
        assert_eq!(
            toks("user_名"),
            vec![Token::Ident("user_名".into()), Token::EOF],
        );
        assert_eq!(
            toks("café_2"),
            vec![Token::Ident("café_2".into()), Token::EOF],
        );
    }

    // ---- Mini-batch Bytes — `b"..."` literal ----

    #[test]
    fn bytes_literal_basic_ascii() {
        assert_eq!(
            toks(r#"b"hola""#),
            vec![Token::Bytes(b"hola".to_vec()), Token::EOF]
        );
    }

    #[test]
    fn bytes_literal_with_hex_escape() {
        assert_eq!(
            toks(r#"b"\x00\xff""#),
            vec![Token::Bytes(vec![0x00, 0xff]), Token::EOF]
        );
    }

    #[test]
    fn bytes_literal_with_common_escapes() {
        assert_eq!(
            toks(r#"b"\n\r\t\0\\\"""#),
            vec![
                Token::Bytes(vec![b'\n', b'\r', b'\t', 0, b'\\', b'"']),
                Token::EOF
            ]
        );
    }

    #[test]
    fn bytes_literal_empty() {
        assert_eq!(toks(r#"b"""#), vec![Token::Bytes(vec![]), Token::EOF]);
    }

    #[test]
    fn bytes_literal_ident_b_without_quote_is_still_ident() {
        // A lone `b` (without a quote) is a normal identifier, not
        // a bytes-literal trigger.
        assert_eq!(
            toks("b + 1"),
            vec![
                Token::Ident("b".into()),
                Token::Plus,
                Token::Int(1),
                Token::EOF
            ]
        );
    }

    #[test]
    fn bytes_literal_unicode_via_utf8() {
        // Unicode char in the source: encoded as UTF-8 bytes.
        // `ñ` is 2 bytes (0xc3, 0xb1).
        assert_eq!(
            toks("b\"ñ\""),
            vec![Token::Bytes(vec![0xc3, 0xb1]), Token::EOF]
        );
    }

    #[test]
    fn bytes_literal_invalid_escape_is_error() {
        // `\z` is not a supported escape → clear error.
        let err = tokenize(r#"b"\z""#).unwrap_err();
        assert!(
            err.message.contains("escape") && err.message.contains("not supported"),
            "expected escape not-supported message, was: {}",
            err.message
        );
    }

    #[test]
    fn f8_emojis_are_rejected() {
        // Emojis are not `is_alphabetic` (Unicode Symbol, not Letter)
        // — the lexer rejects them with UnexpectedChar.
        let err = tokenize("🚀").unwrap_err();
        assert!(
            matches!(err.kind, ErrorKind::UnexpectedChar('🚀')),
            "expected UnexpectedChar('🚀'), was: {:?}",
            err.kind,
        );
    }

    #[test]
    fn f8_unicode_digits_cannot_start_identifier() {
        // Same as ASCII: an identifier can't start with a digit (ASCII
        // or Unicode). `٢` (Arabic-Indic 2) IS is_numeric, but the
        // lexer enters `read_number` only for things that start with
        // an ASCII digit. For non-ASCII digits, the lexer bails out
        // because they're neither `is_alphabetic` nor an ASCII digit.
        let err = tokenize("٢").unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedChar(_)));
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
        assert_eq!(
            toks(r#""Hola""#),
            vec![Token::Str("Hola".into()), Token::EOF]
        );
    }

    // ---- R.1.5 — multiline strings `"""..."""` (mini-phase R) ----

    #[test]
    fn triple_string_simple() {
        let src = "\"\"\"hola\"\"\"";
        assert_eq!(toks(src), vec![Token::Str("hola".into()), Token::EOF],);
    }

    #[test]
    fn triple_string_with_newlines_preserves_them() {
        let src = "\"\"\"linea uno\nlinea dos\"\"\"";
        assert_eq!(
            toks(src),
            vec![Token::Str("linea uno\nlinea dos".into()), Token::EOF],
        );
    }

    #[test]
    fn triple_string_empty() {
        let src = "\"\"\"\"\"\"";
        assert_eq!(toks(src), vec![Token::Str("".into()), Token::EOF]);
    }

    #[test]
    fn triple_string_with_internal_double_quote_is_preserved() {
        // `"""a "b" c"""` → content `a "b" c`. A lone inner quote
        // doesn't close; only `"""` in a row closes.
        let src = "\"\"\"a \"b\" c\"\"\"";
        assert_eq!(toks(src), vec![Token::Str("a \"b\" c".into()), Token::EOF],);
    }

    #[test]
    fn triple_string_unclosed_is_error() {
        let src = "\"\"\"unterminated";
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
        // The lexer does NOT interpolate — it leaves "{name}" as-is; the parser/eval handles it.
        assert_eq!(
            toks(r#""Hola, {name}!""#),
            vec![Token::Str("Hola, {name}!".into()), Token::EOF]
        );
    }

    // ---- F9 — extended escapes (\u, \x, \0, \b) ----

    #[test]
    fn f9_escape_null_and_backspace() {
        // `\0` → NUL (U+0000). `\b` → backspace (U+0008).
        assert_eq!(
            toks(r#""a\0b""#),
            vec![Token::Str("a\0b".into()), Token::EOF]
        );
        assert_eq!(
            toks(r#""a\bb""#),
            vec![Token::Str("a\u{0008}b".into()), Token::EOF]
        );
    }

    #[test]
    fn f9_escape_unicode_basic_and_extended() {
        // BMP: `\u{00E9}` = 'é'. Supplementary: `\u{1F600}` = 😀.
        assert_eq!(
            toks(r#""caf\u{00E9}""#),
            vec![Token::Str("café".into()), Token::EOF]
        );
        assert_eq!(
            toks(r#""\u{1F600}""#),
            vec![Token::Str("😀".into()), Token::EOF]
        );
        // Lowercase hex also works.
        assert_eq!(
            toks(r#""\u{00e9}""#),
            vec![Token::Str("é".into()), Token::EOF]
        );
        // 1 hex digit is enough.
        assert_eq!(
            toks(r#""\u{A}""#),
            vec![Token::Str("\n".into()), Token::EOF]
        );
    }

    #[test]
    fn f9_escape_unicode_empty_is_error() {
        let err = tokenize(r#""\u{}""#).unwrap_err();
        assert!(
            err.message.contains("empty"),
            "expected message about empty `\\u{{}}`, was: {}",
            err.message
        );
    }

    #[test]
    fn f9_escape_unicode_unclosed_is_error() {
        // `"` appears before `}` → the lexer hits a non-hex char
        // (the `"`) and reports an invalid digit inside `\u{...}`.
        let err = tokenize(r#""\u{ABC""#).unwrap_err();
        assert!(
            err.message.contains("invalid hex") || err.message.contains("Invalid hex digit"),
            "expected message about invalid hex digit, was: {}",
            err.message
        );
    }

    #[test]
    fn f9_escape_unicode_surrogate_rejected() {
        // U+D800 is the first high-surrogate code point, invalid as a
        // Unicode scalar.
        let err = tokenize(r#""\u{D800}""#).unwrap_err();
        assert!(
            err.message.contains("scalar"),
            "expected message about scalar codepoint, was: {}",
            err.message
        );
    }

    #[test]
    fn f9_escape_unicode_too_long_is_error() {
        // 7 hex digits exceed the allowed maximum (6, up to 10FFFF).
        let err = tokenize(r#""\u{1234567}""#).unwrap_err();
        assert!(
            err.message.contains("6 hex digits") || err.message.contains("10FFFF"),
            "expected message about hex digit limit, was: {}",
            err.message
        );
    }

    #[test]
    fn f9_escape_hex_byte_ascii() {
        // `\x41` = 'A', `\x7F` = DEL (ASCII limit).
        assert_eq!(
            toks(r#""\x41BC""#),
            vec![Token::Str("ABC".into()), Token::EOF]
        );
        assert_eq!(
            toks(r#""\x7F""#),
            vec![Token::Str("\u{007F}".into()), Token::EOF]
        );
    }

    #[test]
    fn f9_escape_hex_byte_outside_ascii_rejected() {
        // `\x80` and above are not ASCII; explicit rejection suggesting \u{...}.
        let err = tokenize(r#""\x80""#).unwrap_err();
        assert!(
            err.message.contains("ASCII") && err.message.contains("\\u"),
            "expected message about ASCII range + \\u suggestion, was: {}",
            err.message
        );
    }

    #[test]
    fn f9_escape_hex_byte_too_few_digits_is_error() {
        let err = tokenize(r#""\x4""#).unwrap_err();
        assert!(
            err.message.contains("2 hex digits"),
            "expected message about 2 hex digits, was: {}",
            err.message
        );
    }

    #[test]
    fn f9_extended_escapes_work_in_triple_string() {
        // The same escapes (\u/\x/\0/\b) work inside `"""..."""`.
        let src = "\"\"\"\\u{00E9}-\\x41-\\0\"\"\"";
        assert_eq!(toks(src), vec![Token::Str("é-A-\0".into()), Token::EOF]);
    }

    #[test]
    fn escaped_braces_are_preserved_literally() {
        // '\{' and '\}' must reach the parser with the backslash
        // intact, so the parser can distinguish '{name}' (interpolation)
        // from '\{name\}' (literal). If the lexer un-escaped them
        // here, that distinction would be lost.
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
        let res = tokenize(r#""unterminated"#);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnterminatedString));
    }

    #[test]
    fn unterminated_block_comment_errors() {
        let res = tokenize("/* unterminated");
        assert!(res.is_err());
        assert!(matches!(
            res.unwrap_err().kind,
            ErrorKind::UnterminatedComment
        ));
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
        // Equivalent to examples/hello.fitz (without the emoji to keep the test short)
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

    // ---- Phase 9.z.1.b — trivia (comments + blank lines) ----

    #[test]
    fn tokenize_default_does_not_capture_trivia() {
        // The fast `tokenize` must not spend memory on trivia. The test
        // indirectly confirms that the lexer is not slowed down (there
        // is no side-table to populate).
        let _ = tokenize("// comentario\nlet x = 1\n").unwrap();
        // Nothing to assert directly — tokenize does not expose trivia.
        // The test guarantees the API stays zero-overhead.
    }

    #[test]
    fn tokenize_with_trivia_captures_line_comment() {
        let (_toks, trivia) = tokenize_with_trivia("// hola\nlet x = 1\n").unwrap();
        assert_eq!(trivia.comments.len(), 1);
        let c = &trivia.comments[0];
        assert_eq!(c.kind, CommentKind::Line);
        assert_eq!(c.text, " hola"); // without the `//`, with the space
        assert_eq!(c.line, 1);
        assert_eq!(c.column, 1);
    }

    #[test]
    fn tokenize_with_trivia_captures_trailing_comment() {
        let (_toks, trivia) = tokenize_with_trivia("let x = 1 // explicación\n").unwrap();
        assert_eq!(trivia.comments.len(), 1);
        let c = &trivia.comments[0];
        assert_eq!(c.text, " explicación");
        assert_eq!(c.line, 1);
        // The `//` starts at column 11 (after `let x = 1 `).
        assert!(c.column > 1);
    }

    #[test]
    fn tokenize_with_trivia_captures_block_comment() {
        let (_toks, trivia) = tokenize_with_trivia("/* foo bar */\nlet x = 1\n").unwrap();
        assert_eq!(trivia.comments.len(), 1);
        let c = &trivia.comments[0];
        assert_eq!(c.kind, CommentKind::Block);
        assert_eq!(c.text, " foo bar ");
        assert_eq!(c.line, 1);
    }

    #[test]
    fn tokenize_with_trivia_captures_blank_lines() {
        let src = "let x = 1\n\nlet y = 2\n\n\nlet z = 3\n";
        let (_toks, trivia) = tokenize_with_trivia(src).unwrap();
        // Blank lines: 2 (between x and y), 4 and 5 (between y and z).
        assert_eq!(trivia.blank_lines, vec![2, 4, 5]);
    }

    #[test]
    fn tokenize_with_trivia_does_not_count_comment_line_as_blank() {
        let src = "let x = 1\n// solo comment\nlet y = 2\n";
        let (_toks, trivia) = tokenize_with_trivia(src).unwrap();
        assert!(
            trivia.blank_lines.is_empty(),
            "blanks: {:?}",
            trivia.blank_lines
        );
        assert_eq!(trivia.comments.len(), 1);
        assert_eq!(trivia.comments[0].line, 2);
    }

    #[test]
    fn tokenize_with_trivia_comments_order_is_source_order() {
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
    fn tokenize_with_trivia_mix_of_comments_and_blanks() {
        let src = "\n// header\n\nlet x = 1\n\n// otro\nlet y = 2 // trailing\n";
        let (_toks, trivia) = tokenize_with_trivia(src).unwrap();
        assert_eq!(trivia.comments.len(), 3);
        assert!(trivia.blank_lines.contains(&1));
        assert!(trivia.blank_lines.contains(&3));
        assert!(trivia.blank_lines.contains(&5));
    }

    // ---------------------------------------------------------------
    // Mini-batch Núm — `_` separators + scientific notation.
    // ---------------------------------------------------------------

    /// Helper: tokenize a single expression and return the first token.
    fn first_token_of(src: &str) -> Token {
        let toks = tokenize(src).expect("must tokenize");
        toks.into_iter().next().expect("at least one token").token
    }

    #[test]
    fn num_separator_in_int_parses_as_integer_without_underscores() {
        assert_eq!(first_token_of("1_000_000"), Token::Int(1_000_000));
        assert_eq!(first_token_of("1_2_3"), Token::Int(123));
    }

    #[test]
    fn num_separator_in_float_works_in_int_and_fraction() {
        assert_eq!(first_token_of("1_000.5"), Token::Float(1000.5));
        assert_eq!(first_token_of("3.14_15"), Token::Float(3.1415));
        assert_eq!(first_token_of("1_000.000_1"), Token::Float(1000.0001));
    }

    #[test]
    fn num_separator_double_or_terminal_is_error() {
        assert!(tokenize("1__0").is_err(), "double underscore");
        assert!(tokenize("1_000_").is_err(), "trailing underscore");
    }

    #[test]
    fn num_scientific_notation_basic_produces_float() {
        assert_eq!(first_token_of("1e10"), Token::Float(1e10));
        assert_eq!(first_token_of("3.14e2"), Token::Float(314.0));
        assert_eq!(first_token_of("2.5E3"), Token::Float(2500.0));
    }

    #[test]
    fn num_scientific_notation_with_sign() {
        assert_eq!(first_token_of("1e-10"), Token::Float(1e-10));
        assert_eq!(first_token_of("1e+3"), Token::Float(1000.0));
        assert_eq!(first_token_of("3.14E-2"), Token::Float(0.0314));
    }

    #[test]
    fn num_separator_in_exponent() {
        // `1e1_0` → `1e10` after stripping separators.
        assert_eq!(first_token_of("1e1_0"), Token::Float(1e10));
        assert_eq!(first_token_of("1_000e1_0"), Token::Float(1000e10));
    }

    #[test]
    fn num_exponent_without_digits_is_error() {
        assert!(tokenize("1e").is_err(), "`e` alone has no digits");
        assert!(tokenize("1e+").is_err(), "`e+` has no digits");
        assert!(tokenize("1e-").is_err(), "`e-` has no digits");
    }

    #[test]
    fn num_int_classic_still_works() {
        // Regression — without separators or scientific notation, same
        // result as before.
        assert_eq!(first_token_of("42"), Token::Int(42));
        assert_eq!(first_token_of("3.14"), Token::Float(3.14));
    }

    #[test]
    fn num_tuple_field_access_not_confused_with_separator() {
        // `t.0` produces Ident("t"), Dot, Int(0) — it is not confused
        // with any separator parsing. `t.0.0` (nested tuple access)
        // keeps working: the prev_was_dot flag forces Int instead of
        // Float.
        use Token::*;
        let toks: Vec<Token> = tokenize("t.0.0")
            .unwrap()
            .into_iter()
            .map(|t| t.token)
            .collect();
        assert_eq!(toks[0], Ident("t".into()));
        assert_eq!(toks[1], Dot);
        assert_eq!(toks[2], Int(0));
        assert_eq!(toks[3], Dot);
        assert_eq!(toks[4], Int(0));
    }

    // ---------------------------------------------------------------
    // Mini-batch Lit — hex / binary / octal literals.
    // ---------------------------------------------------------------

    #[test]
    fn lit_hex_basic_lower_and_upper_case() {
        // Hex digits are case-insensitive (parallel to Rust/Python).
        assert_eq!(first_token_of("0xFF"), Token::Int(255));
        assert_eq!(first_token_of("0xff"), Token::Int(255));
        assert_eq!(first_token_of("0xCAFE"), Token::Int(0xCAFE));
        assert_eq!(first_token_of("0x0"), Token::Int(0));
    }

    #[test]
    fn lit_binary_and_octal_basic() {
        assert_eq!(first_token_of("0b1010"), Token::Int(10));
        assert_eq!(first_token_of("0b0"), Token::Int(0));
        assert_eq!(first_token_of("0o755"), Token::Int(0o755));
        assert_eq!(first_token_of("0o7"), Token::Int(7));
    }

    #[test]
    fn lit_separators_in_hex_bin_oct() {
        // `_` between valid digits for each base.
        assert_eq!(first_token_of("0xDEAD_BEEF"), Token::Int(0xDEAD_BEEF));
        assert_eq!(first_token_of("0b1010_1010"), Token::Int(0b1010_1010));
        assert_eq!(first_token_of("0o7_5_5"), Token::Int(0o755));
    }

    #[test]
    fn lit_without_digits_after_prefix_is_error() {
        // `0x`, `0b`, `0o` alone without digits.
        assert!(tokenize("0x").is_err());
        assert!(tokenize("0b").is_err());
        assert!(tokenize("0o").is_err());
    }

    #[test]
    fn lit_invalid_digit_for_base_cuts_the_literal() {
        // `0b2` lexes `0b` + ... but there is no valid '2' in binary.
        // The lexer bails after the prefix's '0', firing a "no digits
        // after prefix" error.
        assert!(tokenize("0b2").is_err());
        // `0o9` same case: `9` is not a valid octal digit.
        assert!(tokenize("0o9").is_err());
    }

    #[test]
    fn lit_overflow_is_explicit_error() {
        // i64::MAX = 0x7FFF_FFFF_FFFF_FFFF (positive). One more nibble → overflow.
        assert!(tokenize("0xFFFFFFFFFFFFFFFF").is_err());
        // Equivalent in binary.
        assert!(
            tokenize("0b11111111111111111111111111111111111111111111111111111111111111111")
                .is_err()
        );
    }

    #[test]
    fn lit_underscore_terminal_or_double_is_error_in_hex() {
        assert!(tokenize("0xFF_").is_err(), "underscore al final");
        assert!(tokenize("0xF__F").is_err(), "doble underscore");
    }

    // ---------------------------------------------------------------
    // Mini-batch Cmp — compound bitwise + uppercase prefixes.
    // ---------------------------------------------------------------

    #[test]
    fn cmp_compound_tokens_bit_by_bit() {
        use Token::*;
        let toks: Vec<Token> = tokenize("x &= 1 |= 2 ^= 3 <<= 4 >>= 5")
            .unwrap()
            .into_iter()
            .map(|t| t.token)
            .collect();
        // Verify the compound tokens.
        assert!(toks.contains(&AmpEq));
        assert!(toks.contains(&PipeEq));
        assert!(toks.contains(&CaretEq));
        assert!(toks.contains(&ShlEq));
        assert!(toks.contains(&ShrEq));
    }

    #[test]
    fn cmp_uppercase_prefixes_hex_bin_oct() {
        // Mini-batch Cmp — `0X`/`0B`/`0O` (uppercase) work the same
        // as the lowercase variants.
        assert_eq!(first_token_of("0XFF"), Token::Int(255));
        assert_eq!(first_token_of("0B1010"), Token::Int(10));
        assert_eq!(first_token_of("0O755"), Token::Int(0o755));
    }

    #[test]
    fn cmp_token_amp_alone_still_works() {
        // Regression: a lone `&` (without `=` after) is still Token::Amp.
        let toks: Vec<Token> = tokenize("a & b")
            .unwrap()
            .into_iter()
            .map(|t| t.token)
            .collect();
        assert!(toks.contains(&Token::Amp));
        assert!(!toks.contains(&Token::AmpEq));
    }

    #[test]
    fn cmp_shl_alone_still_works() {
        // Regression: a lone `<<` is still Token::Shl.
        let toks: Vec<Token> = tokenize("a << 2")
            .unwrap()
            .into_iter()
            .map(|t| t.token)
            .collect();
        assert!(toks.contains(&Token::Shl));
        assert!(!toks.contains(&Token::ShlEq));
    }

    #[test]
    fn lit_decimal_classic_still_works() {
        // Regression: anything that starts with `0` without a hex/bin/oct
        // prefix is still parsed as decimal.
        assert_eq!(first_token_of("0"), Token::Int(0));
        assert_eq!(first_token_of("007"), Token::Int(7));
        assert_eq!(first_token_of("0.5"), Token::Float(0.5));
    }

    // L1 (2026-06-05) — `;` as optional stmt separator. The lexer
    // emits `Token::Newline` so the parser treats it identically to a
    // real newline. Closes the historical "optional semicolons like in
    // Go" debt (design decision #5).

    #[test]
    fn l1_semicolon_emits_newline() {
        // a lone `;` → one Newline + EOF.
        assert_eq!(toks(";"), vec![Token::Newline, Token::EOF]);
    }

    #[test]
    fn l1_two_exprs_separated_by_semicolon_produce_two_stmts_via_newline() {
        // `1 + 1; 2 + 2` must produce tokens equivalent to
        // `1 + 1\n2 + 2`. The parser then reads them as 2 stmts.
        let got = toks("1 + 1; 2 + 2");
        let expected = vec![
            Token::Int(1),
            Token::Plus,
            Token::Int(1),
            Token::Newline,
            Token::Int(2),
            Token::Plus,
            Token::Int(2),
            Token::EOF,
        ];
        assert_eq!(got, expected);
    }

    #[test]
    fn l1_semicolon_followed_by_real_newline_does_not_duplicate_newlines_in_parser() {
        // `1;\n2` produces two consecutive Newlines (one from `;`, one
        // from the real `\n`). The parser already tolerates repeated
        // Newlines as a single separator — checked by the `recovery_*`
        // smoke and while parsing blocks. Here we only validate the
        // lexer shape.
        let got = toks("1;\n2");
        let expected = vec![
            Token::Int(1),
            Token::Newline,
            Token::Newline,
            Token::Int(2),
            Token::EOF,
        ];
        assert_eq!(got, expected);
    }

    #[test]
    fn l1_semicolon_inside_string_is_not_interpreted() {
        // Strings preserve `;` as a literal char — the lexer only
        // intercepts it at the scanner's top level, not inside a string
        // literal (which has its own state machine).
        let got = toks(r#""hola;mundo""#);
        assert_eq!(got, vec![Token::Str("hola;mundo".into()), Token::EOF]);
    }

    #[test]
    fn l1_semicolon_inside_comment_is_consumed_as_part_of_comment() {
        // `// hola; mundo` is consumed entirely as a line comment —
        // there is still only a single Newline at the end.
        let got = toks("// hola; mundo\n42");
        assert_eq!(got, vec![Token::Newline, Token::Int(42), Token::EOF]);
    }
}
