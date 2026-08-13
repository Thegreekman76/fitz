// view/parser.rs — recursive-descent parser for `.fitzv` (Phase 11 POC).
//
// Two layers:
//
//   1. **Component shell** — consumes tokens from `view::lexer` to
//      recognise `component Name { state {...} event ... }`. State
//      field defaults and event handler bodies are captured as raw
//      source strings; the POC does not parse them as
//      `crate::ast::Expr` yet (deferred to Phase 11.2, see
//      `docs/fase-11-plan.md`).
//
//   2. **HTML sub-parser** — builds the `TemplateNode` tree from the
//      raw blob returned by `Token::TemplateRaw(String)`. It is
//      char-by-char and does NOT reuse the classic lexer. POC
//      coverage: Text / Interpolation `{expr}` / Element
//      `<tag attr...>...</tag>` with attrs of kind Static /
//      Interpolation / Event (`@click="handler"`).
//
// The CSS inside `<style scoped>` stays as an opaque blob — CSS
// parsing lands in 11.3+ (once the scoping strategy is settled).

use super::ast::*;
use super::lexer::{Token, TokenWithLoc, ViewLexError};
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct ViewParseError {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

impl fmt::Display for ViewParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "view parse error at {}:{} — {}",
            self.line, self.column, self.message
        )
    }
}

impl std::error::Error for ViewParseError {}

impl From<ViewLexError> for ViewParseError {
    fn from(e: ViewLexError) -> Self {
        Self {
            message: e.message,
            line: e.line,
            column: e.column,
        }
    }
}

pub type ViewParseResult<T> = Result<T, ViewParseError>;

pub fn parse(source: &str) -> ViewParseResult<ViewFile> {
    let tokens = super::lexer::tokenize(source)?;
    let mut parser = ViewParser::new(tokens);
    parser.parse_view_file()
}

struct ViewParser {
    tokens: Vec<TokenWithLoc>,
    pos: usize,
}

impl ViewParser {
    fn new(tokens: Vec<TokenWithLoc>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &TokenWithLoc {
        // Guaranteed: `tokenize` always appends Eof.
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn advance(&mut self) -> TokenWithLoc {
        let t = self.tokens[self.pos].clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek().token, Token::Newline) {
            self.advance();
        }
    }

    fn expect(&mut self, want: Token) -> ViewParseResult<TokenWithLoc> {
        self.skip_newlines();
        let cur = self.peek().clone();
        if std::mem::discriminant(&cur.token) == std::mem::discriminant(&want) {
            Ok(self.advance())
        } else {
            Err(ViewParseError {
                message: format!("expected {want}, got {}", cur.token),
                line: cur.line,
                column: cur.column,
            })
        }
    }

    fn parse_view_file(&mut self) -> ViewParseResult<ViewFile> {
        let mut imports = Vec::new();
        let mut components = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek().token {
                Token::Eof => break,
                // §9.dd (2026-07-16) — `from X import Y1, Y2, ...` at
                // top of `.fitzv`. Emitted verbatim as classic Fitz
                // import at the top of the transformed source so the
                // classic loader resolves cross-file nominals normally.
                // Imports MUST precede all `component` blocks; a `from`
                // after a `component` errors (classic Fitz convention
                // is imports at file head).
                Token::From => {
                    if !components.is_empty() {
                        let cur = self.peek().clone();
                        return Err(ViewParseError {
                            message: "`from ... import ...` must appear \
                                      before any `component` block (classic Fitz \
                                      convention: imports at file head)"
                                .to_string(),
                            line: cur.line,
                            column: cur.column,
                        });
                    }
                    imports.push(self.parse_view_import()?);
                }
                Token::Component => {
                    components.push(self.parse_component()?);
                }
                _ => {
                    let cur = self.peek().clone();
                    return Err(ViewParseError {
                        message: format!(
                            "expected `from ... import ...` or `component ...` \
                             at the top level, got {}",
                            cur.token
                        ),
                        line: cur.line,
                        column: cur.column,
                    });
                }
            }
        }
        Ok(ViewFile {
            imports,
            components,
        })
    }

    /// §9.dd (2026-07-16) — Parse `from <path> import <name1>, <name2>,
    /// ...` at the top of a `.fitzv` file. Grammar:
    ///
    /// ```text
    /// FromImport := "from" IdentPath "import" IdentList
    /// IdentPath  := Ident ("." Ident)*
    /// IdentList  := Ident ("," Ident)*
    /// ```
    ///
    /// The `as` alias syntax (`import X as Y`) is REJECTED with a
    /// targeted Phase 11.7+ pointer — aliases would require alias-
    /// aware TypeEnv patching + emit-side rewriting; deferred.
    fn parse_view_import(&mut self) -> ViewParseResult<ViewImport> {
        // Consume `from`
        let from_tok = self.advance();
        let loc = Loc::new(from_tok.line, from_tok.column);
        // Read dotted path.
        let mut path = Vec::new();
        loop {
            self.skip_newlines_soft();
            let cur = self.peek().clone();
            match &cur.token {
                Token::Ident(s) => {
                    path.push(s.clone());
                    self.advance();
                }
                _ => {
                    return Err(ViewParseError {
                        message: format!(
                            "expected identifier in `from` module path, got {}",
                            cur.token
                        ),
                        line: cur.line,
                        column: cur.column,
                    });
                }
            }
            self.skip_newlines_soft();
            match self.peek().token {
                Token::Dot => {
                    self.advance(); // consume `.`
                    continue;
                }
                _ => break,
            }
        }
        if path.is_empty() {
            return Err(ViewParseError {
                message: "empty module path in `from ... import ...`".into(),
                line: loc.line,
                column: loc.column,
            });
        }
        // Expect `import`
        self.skip_newlines_soft();
        let cur = self.peek().clone();
        if !matches!(cur.token, Token::Import) {
            return Err(ViewParseError {
                message: format!(
                    "expected `import` after module path in `from ... import ...`, \
                     got {}",
                    cur.token
                ),
                line: cur.line,
                column: cur.column,
            });
        }
        self.advance(); // consume `import`
                        // Read comma-separated identifiers. Each ident
                        // may be followed by `as <alias-ident>` (S.1,
                        // post-v0.21.1) — the original identifier
                        // survives for the emitted classic Fitz `from
                        // X import Y as Z`, but the alias is what the
                        // SFC's template + event bodies reference (so
                        // `imported_names` in the SSR emitter derives
                        // from the alias when present, from the
                        // original when not).
        let mut names: Vec<(String, Option<String>)> = Vec::new();
        loop {
            self.skip_newlines_soft();
            let cur = self.peek().clone();
            let original = match &cur.token {
                Token::Ident(s) => {
                    self.advance();
                    s.clone()
                }
                _ => {
                    return Err(ViewParseError {
                        message: format!(
                            "expected identifier in `import` name list, got {}",
                            cur.token
                        ),
                        line: cur.line,
                        column: cur.column,
                    });
                }
            };
            // Optional `as <alias>`.
            self.skip_newlines_soft();
            let alias = if matches!(self.peek().token, Token::As) {
                self.advance(); // consume `as`
                self.skip_newlines_soft();
                let alias_tok = self.peek().clone();
                match &alias_tok.token {
                    Token::Ident(a) => {
                        self.advance();
                        Some(a.clone())
                    }
                    _ => {
                        return Err(ViewParseError {
                            message: format!(
                                "expected identifier after `as` in `import ... as`, got {}",
                                alias_tok.token
                            ),
                            line: alias_tok.line,
                            column: alias_tok.column,
                        });
                    }
                }
            } else {
                None
            };
            names.push((original, alias));
            self.skip_newlines_soft();
            match self.peek().token {
                Token::Comma => {
                    self.advance(); // consume `,`
                    continue;
                }
                _ => break,
            }
        }
        if names.is_empty() {
            return Err(ViewParseError {
                message: "`from ... import` requires at least one name".into(),
                line: loc.line,
                column: loc.column,
            });
        }
        Ok(ViewImport { path, names, loc })
    }

    /// Skip a bounded number of newline tokens WITHOUT hitting the
    /// end of the token stream. Used inside `parse_view_import` where
    /// the import can span multiple lines (`from foo\n  import Bar,\n
    /// Baz`) but shouldn't consume newlines that terminate the whole
    /// stmt.
    fn skip_newlines_soft(&mut self) {
        while matches!(self.peek().token, Token::Newline) {
            self.advance();
        }
    }

    fn parse_component(&mut self) -> ViewParseResult<Component> {
        let kw = self.expect(Token::Component)?;
        let loc = Loc::new(kw.line, kw.column);
        let name_tok = self.expect(Token::Ident(String::new()))?;
        let name = match name_tok.token {
            Token::Ident(s) => s,
            _ => unreachable!(),
        };

        // Phase 11.12 slice 4 — optional `hydrate` opt-in marker between the
        // component name and its body (`component App hydrate { ... }`). It is
        // a bare ident (not a lexer keyword — same treatment as `as` in
        // imports), so it only takes effect in this exact position. On the ROOT
        // component it makes the whole naive composition tree hydrate the
        // server-painted DOM instead of fresh-mounting.
        let mut hydrate = false;
        self.skip_newlines();
        if matches!(&self.peek().token, Token::Ident(s) if s == "hydrate") {
            self.advance();
            hydrate = true;
        }

        self.expect(Token::LBrace)?;

        let mut state = Vec::new();
        let mut derived = Vec::new();
        let mut events = Vec::new();
        let mut template: Option<Template> = None;
        let mut style: Option<Style> = None;

        loop {
            self.skip_newlines();
            match &self.peek().token {
                Token::RBrace => {
                    self.advance();
                    break;
                }
                Token::State => {
                    let block = self.parse_state_block()?;
                    state.extend(block);
                }
                Token::Derived => {
                    let block = self.parse_derived_block()?;
                    derived.extend(block);
                }
                Token::Event => {
                    events.push(self.parse_event_handler()?);
                }
                Token::TemplateRaw(_) => {
                    if template.is_some() {
                        let cur = self.peek().clone();
                        return Err(ViewParseError {
                            message: "duplicate `<template>` block — only one per component".into(),
                            line: cur.line,
                            column: cur.column,
                        });
                    }
                    let tok = self.advance();
                    let raw = match tok.token {
                        Token::TemplateRaw(s) => s,
                        _ => unreachable!(),
                    };
                    let roots = parse_template_body(&raw, tok.line, tok.column)?;
                    template = Some(Template {
                        roots,
                        loc: Loc::new(tok.line, tok.column),
                    });
                }
                Token::StyleRaw { .. } => {
                    if style.is_some() {
                        let cur = self.peek().clone();
                        return Err(ViewParseError {
                            message: "duplicate `<style>` block — only one `<style scoped>` or \
                                 `<style global>` per component"
                                .into(),
                            line: cur.line,
                            column: cur.column,
                        });
                    }
                    let tok = self.advance();
                    let (kind, css) = match tok.token {
                        Token::StyleRaw { kind, body } => (kind, body),
                        _ => unreachable!(),
                    };
                    style = Some(Style {
                        kind,
                        css_raw: css,
                        loc: Loc::new(tok.line, tok.column),
                    });
                }
                _ => {
                    let cur = self.peek().clone();
                    return Err(ViewParseError {
                        message: format!(
                            "expected `state`, `derived`, `event`, `<template>`, `<style scoped>` \
                             or `<style global>` inside component body, got {}",
                            cur.token
                        ),
                        line: cur.line,
                        column: cur.column,
                    });
                }
            }
        }

        Ok(Component {
            name,
            loc,
            hydrate,
            state,
            derived,
            events,
            template,
            style,
        })
    }

    fn parse_state_block(&mut self) -> ViewParseResult<Vec<StateField>> {
        self.expect(Token::State)?;
        self.expect(Token::LBrace)?;
        let mut fields = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek().token, Token::RBrace) {
                self.advance();
                break;
            }
            fields.push(self.parse_state_field()?);
        }
        Ok(fields)
    }

    /// Phase 11.10 slice 4 — `derived { name: T = expr }`. Same entry shape
    /// as a state field (reuses [`Self::parse_state_field`]); the `default`
    /// slot holds the derived expression.
    fn parse_derived_block(&mut self) -> ViewParseResult<Vec<StateField>> {
        self.expect(Token::Derived)?;
        self.expect(Token::LBrace)?;
        let mut fields = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.peek().token, Token::RBrace) {
                self.advance();
                break;
            }
            fields.push(self.parse_state_field()?);
        }
        Ok(fields)
    }

    fn parse_state_field(&mut self) -> ViewParseResult<StateField> {
        let name_tok = self.expect(Token::Ident(String::new()))?;
        let name = match name_tok.token {
            Token::Ident(s) => s,
            _ => unreachable!(),
        };
        let loc = Loc::new(name_tok.line, name_tok.column);
        self.expect(Token::Colon)?;
        // Capture the type as raw text up to `=`. Enough for the POC
        // — the real TypeExpr parser hooks up in 11.2.
        let type_expr_raw = self.capture_raw_until(&[Token::Eq])?;
        self.expect(Token::Eq)?;
        // Capture the default as raw text up to the next stmt
        // boundary (newline, comma, semicolon, or `}` of the state
        // block).
        let default_expr_raw =
            self.capture_raw_until(&[Token::Newline, Token::Comma, Token::Semi, Token::RBrace])?;
        // Consume the separator (except `}`, which the caller of
        // the state block consumes).
        if matches!(
            self.peek().token,
            Token::Newline | Token::Comma | Token::Semi
        ) {
            self.advance();
        }
        Ok(StateField {
            name,
            type_expr_raw: type_expr_raw.trim().to_string(),
            default_expr_raw: default_expr_raw.trim().to_string(),
            loc,
        })
    }

    fn parse_event_handler(&mut self) -> ViewParseResult<EventHandler> {
        let kw = self.expect(Token::Event)?;
        let name_tok = self.expect(Token::Ident(String::new()))?;
        let name = match name_tok.token {
            Token::Ident(s) => s,
            _ => unreachable!(),
        };
        let loc = Loc::new(kw.line, kw.column);
        self.expect(Token::LParen)?;
        let params_raw = self.capture_raw_until(&[Token::RParen])?.trim().to_string();
        self.expect(Token::RParen)?;
        // Handler bodies: capture EVERYTHING between the opening `{`
        // and its matched `}` (brace-balanced). POC does not re-lex
        // the body, so preserving chars verbatim is enough.
        self.expect(Token::LBrace)?;
        let body_raw = self.capture_balanced_body_raw()?;
        // The closing `}` for the body was already consumed by
        // `capture_balanced_body_raw`.
        Ok(EventHandler {
            name,
            params_raw,
            body_raw,
            loc,
        })
    }

    /// Capture raw text starting at the current position (excluding
    /// separator tokens) up to (but NOT consuming) one of the tokens
    /// in `stops`. Reconstructs an approximate source rendering from
    /// the tokens — enough for opaque blobs that the POC does not
    /// re-analyse.
    ///
    /// **Balance-aware**: `{`/`}`, `(`/`)`, and `[`/`]` are tracked
    /// as nesting pairs; a stop token only ends the capture when the
    /// bracket depth is zero. This lets defaults carry Map / List /
    /// paren-grouped literals (`{}`, `[]`, `(1 + 2)`) without the
    /// inner `}`/`]`/`)` being mistaken for the outer block's
    /// closer. `<`/`>` are NOT counted — a `<` in a state default
    /// context could be a comparison operator, so tracking it as a
    /// bracket would confuse `count < 5`. Generic type annotations
    /// (`List<Str>`) live BEFORE the `=` and never contain nested
    /// braces, so their `<` / `>` also round-trip fine without depth
    /// tracking.
    fn capture_raw_until(&mut self, stops: &[Token]) -> ViewParseResult<String> {
        let mut out = String::new();
        let mut depth: usize = 0;
        loop {
            let cur = self.peek();
            if cur.token == Token::Eof {
                return Err(ViewParseError {
                    message: "unexpected end of file while capturing raw text".into(),
                    line: cur.line,
                    column: cur.column,
                });
            }
            if depth == 0
                && stops
                    .iter()
                    .any(|s| std::mem::discriminant(s) == std::mem::discriminant(&cur.token))
            {
                return Ok(out);
            }
            let tok = self.advance();
            match tok.token {
                Token::LBrace | Token::LParen | Token::LBracket => depth += 1,
                Token::RBrace | Token::RParen | Token::RBracket => {
                    depth = depth.saturating_sub(1);
                }
                _ => {}
            }
            append_token_source(&mut out, &tok.token);
        }
    }

    /// Consume tokens until the enclosing `{` is closed (counting
    /// nested `{`/`}` pairs — needed for nested map literals or if
    /// bodies inside a default). Returns the body as raw source text
    /// WITHOUT the outer braces.
    fn capture_balanced_body_raw(&mut self) -> ViewParseResult<String> {
        let mut out = String::new();
        let mut depth = 1_usize;
        loop {
            let cur = self.peek();
            match cur.token {
                Token::Eof => {
                    return Err(ViewParseError {
                        message: "unterminated event handler body — expected `}`".into(),
                        line: cur.line,
                        column: cur.column,
                    });
                }
                Token::LBrace => {
                    depth += 1;
                    let tok = self.advance();
                    append_token_source(&mut out, &tok.token);
                }
                Token::RBrace => {
                    depth -= 1;
                    if depth == 0 {
                        self.advance(); // consume the closing `}` of the body
                        return Ok(out);
                    }
                    let tok = self.advance();
                    append_token_source(&mut out, &tok.token);
                }
                _ => {
                    let tok = self.advance();
                    append_token_source(&mut out, &tok.token);
                }
            }
        }
    }
}

fn append_token_source(out: &mut String, tok: &Token) {
    if !out.is_empty() && needs_space_before(out, tok) {
        out.push(' ');
    }
    match tok {
        Token::Component => out.push_str("component"),
        Token::State => out.push_str("state"),
        Token::Event => out.push_str("event"),
        Token::Derived => out.push_str("derived"),
        Token::From => out.push_str("from"),
        Token::Import => out.push_str("import"),
        Token::As => out.push_str("as"),
        Token::Ident(s) => out.push_str(s),
        Token::Str(s) => {
            out.push('"');
            for c in s.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '"' => out.push_str("\\\""),
                    '\n' => out.push_str("\\n"),
                    '\t' => out.push_str("\\t"),
                    '\r' => out.push_str("\\r"),
                    other => out.push(other),
                }
            }
            out.push('"');
        }
        Token::LBrace => out.push('{'),
        Token::RBrace => out.push('}'),
        Token::LParen => out.push('('),
        Token::RParen => out.push(')'),
        Token::LBracket => out.push('['),
        Token::RBracket => out.push(']'),
        Token::Comma => out.push(','),
        Token::Colon => out.push(':'),
        Token::Semi => out.push(';'),
        Token::Eq => out.push('='),
        Token::Lt => out.push('<'),
        Token::Gt => out.push('>'),
        Token::EqEq => out.push_str("=="),
        Token::Neq => out.push_str("!="),
        Token::Le => out.push_str("<="),
        Token::Ge => out.push_str(">="),
        Token::Question => out.push('?'),
        Token::Plus => out.push('+'),
        Token::Minus => out.push('-'),
        Token::Star => out.push('*'),
        Token::Slash => out.push('/'),
        Token::Percent => out.push('%'),
        Token::Dot => out.push('.'),
        Token::Newline => out.push('\n'),
        Token::TemplateRaw(_) | Token::StyleRaw { .. } | Token::Eof => {
            // Should not appear inside a `capture_*` call. If they
            // do (unreachable in practice), silently ignore.
        }
    }
}

/// Split a captured `{#for ... }` argument into the iterable
/// expression and an optional `key=<expr>` clause. The `key` marker
/// is the standalone identifier `key` at bracket/brace/paren depth 0,
/// outside string literals, immediately followed (after optional
/// whitespace) by a single `=` (not `==`). Everything before the
/// marker is the iterable; everything after the `=` is the key
/// expression. Both sides are trimmed. When no marker is found,
/// returns `(raw.trim(), None)`.
///
/// Depth + string tracking keeps a `key=` that lives inside the iter
/// expression from being mistaken for the clause — e.g.
/// `items[key]` (bracket depth 1), `lookup(key)` (paren depth 1),
/// `where("key=1")` (string literal), and `key_list` / `keyboard`
/// (no word boundary after `key`) all stay part of the iterable.
fn split_for_iter_key(raw: &str) -> (String, Option<String>) {
    let chars: Vec<char> = raw.chars().collect();
    let n = chars.len();
    let mut depth: i32 = 0;
    let mut in_str = false;
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if in_str {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                i += 1;
            }
            '(' | '[' | '{' => {
                depth += 1;
                i += 1;
            }
            ')' | ']' | '}' => {
                depth -= 1;
                i += 1;
            }
            'k' if depth == 0
                && (i == 0 || chars[i - 1].is_whitespace())
                && chars.get(i + 1) == Some(&'e')
                && chars.get(i + 2) == Some(&'y')
                && matches!(
                    chars.get(i + 3),
                    None | Some(' ' | '\t' | '\r' | '\n' | '=')
                ) =>
            {
                // `key` at a word boundary — skip whitespace to the
                // `=`. A single `=` (not `==`) confirms the clause.
                let mut j = i + 3;
                while j < n && chars[j].is_whitespace() {
                    j += 1;
                }
                if chars.get(j) == Some(&'=') && chars.get(j + 1) != Some(&'=') {
                    let iter_part: String = chars[..i].iter().collect();
                    let key_part: String = chars[j + 1..].iter().collect();
                    return (
                        iter_part.trim().to_string(),
                        Some(key_part.trim().to_string()),
                    );
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    (raw.trim().to_string(), None)
}

fn needs_space_before(prev_out: &str, tok: &Token) -> bool {
    let last = prev_out.chars().last().unwrap();
    if last == '\n' {
        return false;
    }
    match tok {
        Token::LParen
        | Token::RParen
        | Token::LBracket
        | Token::RBracket
        | Token::Comma
        | Token::Colon
        | Token::Semi
        | Token::LBrace
        | Token::RBrace
        // `<`, `>`, `?` bind tightly to the surrounding identifier so
        // that `List<Str>` and `Str?` reconstruct verbatim from
        // `Ident("List") Lt Ident("Str") Gt` and `Ident("Str") Question`
        // — no stray whitespace that would confuse the eye when the
        // captured raw blob gets logged in an error message.
        | Token::Lt
        | Token::Gt
        | Token::Question
        // `.` binds tightly to the surrounding identifier so
        // that `state.count`, `xs.map(fn)`, `msg.upper()`
        // reconstruct verbatim from `Ident("state") Dot
        // Ident("count")` etc. — no stray whitespace that
        // would break the classic Fitz lexer's re-tokenisation
        // of the raw blob.
        | Token::Dot => false,
        _ => matches!(
            last,
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | ')' | '}' | ']' | '"' | '>' | '?'
            // Arithmetic operators added to the space-triggers set so
            // round-trips like `count = count + 1` reconstruct as
            // `count = count + 1` (with the space after the `+`), not
            // `count = count +1`. Both forms are lex-equivalent for the
            // classic Fitz lexer, but the spaced form reads clean when
            // the raw blob shows up in an error message.
            | '+' | '-' | '*' | '/' | '%'
            // `=` was implicit before the arithmetic follow-up (event
            // bodies never contained anything but simple literal
            // assigns, so nobody noticed the missing space after `=`).
            // Once bodies gained arithmetic, `count =count + 1` looked
            // jarring next to the tidy `count + 1` fragment. Adding
            // `=` here produces `count = count + 1` — idiomatic and
            // consistent with how the arithmetic ops space out.
            | '='
        ),
    }
}

// ---------------------------------------------------------------------------
// HTML sub-parser for the `<template>...` blob
// ---------------------------------------------------------------------------

/// Parse the raw `<template>` blob into a tree of `TemplateNode`s.
/// The `base_line` / `base_col` offsets are used to emit `Loc`s
/// close to the position of the `<template>` in the original
/// source — the POC does not yet map precisely to the offset
/// inside the blob (11.2 will add the fine mapping once the
/// checker needs to locate errors).
pub fn parse_template_body(
    raw: &str,
    base_line: usize,
    base_col: usize,
) -> ViewParseResult<Vec<TemplateNode>> {
    let mut p = HtmlParser::new(raw, base_line, base_col);
    let (roots, _terminated_by_else) = p.parse_nodes(None, None, false)?;
    Ok(roots)
}

struct HtmlParser<'a> {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    column: usize,
    _raw: &'a str,
}

impl<'a> HtmlParser<'a> {
    fn new(raw: &'a str, base_line: usize, base_col: usize) -> Self {
        Self {
            chars: raw.chars().collect(),
            pos: 0,
            line: base_line,
            column: base_col,
            _raw: raw,
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

    /// Parse child nodes until:
    ///   - `</tag>` is found (when `parent` is `Some`), or
    ///   - `{/name}` is found (when `directive_parent` is `Some`), or
    ///   - `{#else}` is found (when `accept_else` is true — the caller
    ///     must be the then-body of an `{#if}` and knows to parse the
    ///     else branch next), or
    ///   - the blob ends (when both are `None`).
    ///
    /// The two parents are independent: a template can nest an
    /// `{#if}` inside an element, or an element inside an `{#if}`,
    /// and the parser walks them uniformly. Recursion follows the
    /// same shape.
    ///
    /// The returned bool is `true` iff `parse_nodes` terminated by
    /// encountering `{#else}` (only possible when `accept_else` was
    /// `true`); `false` for every other termination reason. Callers
    /// that don't accept `{#else}` can discard the bool safely.
    fn parse_nodes(
        &mut self,
        parent: Option<&str>,
        directive_parent: Option<&str>,
        accept_else: bool,
    ) -> ViewParseResult<(Vec<TemplateNode>, bool)> {
        let mut nodes = Vec::new();
        loop {
            if self.peek().is_none() {
                if let Some(tag) = parent {
                    return Err(ViewParseError {
                        message: format!("unterminated `<{tag}>` — expected `</{tag}>`"),
                        line: self.line,
                        column: self.column,
                    });
                }
                if let Some(name) = directive_parent {
                    return Err(ViewParseError {
                        message: format!("unterminated `{{#{name}}}` — expected `{{/{name}}}`"),
                        line: self.line,
                        column: self.column,
                    });
                }
                return Ok((nodes, false));
            }

            // Closing tag for parent? `</tag>`
            if let Some(parent_tag) = parent {
                if self.peek() == Some('<') && self.peek_at(1) == Some('/') {
                    // Consume `</`
                    self.advance();
                    self.advance();
                    let closing = self.read_tag_name();
                    self.skip_ws_inside_tag();
                    if self.peek() != Some('>') {
                        return Err(ViewParseError {
                            message: format!("expected `>` closing `</{closing}>`"),
                            line: self.line,
                            column: self.column,
                        });
                    }
                    self.advance();
                    if closing != parent_tag {
                        return Err(ViewParseError {
                            message: format!(
                                "mismatched closing tag `</{closing}>` — expected `</{parent_tag}>`"
                            ),
                            line: self.line,
                            column: self.column,
                        });
                    }
                    return Ok((nodes, false));
                }
            }

            // Closing directive for parent? `{/name}` — an opening
            // `{` followed by `/` at any nesting level of the parent
            // directive expects to match the directive_parent name.
            if self.peek() == Some('{') && self.peek_at(1) == Some('/') {
                let start_line = self.line;
                let start_col = self.column;
                self.advance(); // `{`
                self.advance(); // `/`
                let name = self.read_directive_name();
                self.skip_ws_inline();
                if self.peek() != Some('}') {
                    return Err(ViewParseError {
                        message: format!("expected `}}` closing `{{/{name}}}`"),
                        line: self.line,
                        column: self.column,
                    });
                }
                self.advance();
                match directive_parent {
                    Some(expected) if expected == name => return Ok((nodes, false)),
                    Some(expected) => {
                        return Err(ViewParseError {
                            message: format!(
                                "mismatched closing directive `{{/{name}}}` — expected `{{/{expected}}}`"
                            ),
                            line: start_line,
                            column: start_col,
                        });
                    }
                    None => {
                        return Err(ViewParseError {
                            message: format!(
                                "unexpected `{{/{name}}}` — no matching `{{#{name}}}` opener"
                            ),
                            line: start_line,
                            column: start_col,
                        });
                    }
                }
            }

            match self.peek() {
                Some('<') => {
                    let el = self.parse_element()?;
                    nodes.push(el);
                }
                Some('{') if self.peek_at(1) == Some('#') => {
                    // Directive opener: `{#if ...}`, `{#for ...}`, or
                    // — when we're inside an if body and accepting
                    // `{#else}` — the `{#else}` terminator itself.
                    // Peek the directive name without consuming so we
                    // can distinguish the terminator case.
                    if accept_else {
                        if let Some(peeked) = self.peek_directive_name() {
                            if peeked == "else" {
                                self.consume_else_marker()?;
                                return Ok((nodes, true));
                            }
                        }
                    }
                    let node = self.parse_directive_open()?;
                    nodes.push(node);
                }
                Some('{') => {
                    let interp = self.parse_interpolation()?;
                    nodes.push(interp);
                }
                Some(_) => {
                    let text = self.read_text_until_special();
                    if !text.is_empty() {
                        nodes.push(TemplateNode::Text(text));
                    }
                }
                None => unreachable!(),
            }
        }
    }

    /// Peek the directive name that would follow a `{#` sequence
    /// currently under the cursor, WITHOUT consuming any characters.
    /// Returns `None` if the following identifier is empty. Used to
    /// distinguish `{#else}` (an if-body terminator when
    /// `accept_else` is true) from a regular directive opener.
    fn peek_directive_name(&self) -> Option<String> {
        debug_assert_eq!(self.peek(), Some('{'));
        debug_assert_eq!(self.peek_at(1), Some('#'));
        let mut i = self.pos + 2;
        let mut s = String::new();
        while let Some(&c) = self.chars.get(i) {
            if c.is_ascii_alphanumeric() || c == '_' {
                s.push(c);
                i += 1;
            } else {
                break;
            }
        }
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    /// Consume the `{#else}` marker under the cursor. Called only
    /// after `peek_directive_name` confirmed the directive name is
    /// `"else"`. Emits a clear error if the closing `}` is missing.
    fn consume_else_marker(&mut self) -> ViewParseResult<()> {
        self.advance(); // `{`
        self.advance(); // `#`
        let name = self.read_directive_name();
        debug_assert_eq!(name, "else");
        self.skip_ws_inline();
        if self.peek() != Some('}') {
            return Err(ViewParseError {
                message: "expected `}` closing `{#else}`".into(),
                line: self.line,
                column: self.column,
            });
        }
        self.advance();
        Ok(())
    }

    /// Parse a directive opener like `{#if cond}` or `{#for x in
    /// xs}`. Recurses through `parse_nodes` with the matching
    /// `directive_parent` to collect children up to `{/if}` or
    /// `{/for}`. `{#else}` is not dispatched here: when a valid
    /// `{#else}` appears inside an `{#if}` body, `parse_nodes`
    /// intercepts it as a terminator BEFORE reaching this fn;
    /// hitting `{#else}` inside `parse_directive_open` therefore
    /// means either a stray `{#else}` outside any if or a double
    /// `{#else}` inside an else branch — both are errors reported
    /// with a targeted message.
    fn parse_directive_open(&mut self) -> ViewParseResult<TemplateNode> {
        let start_line = self.line;
        let start_col = self.column;
        self.advance(); // `{`
        self.advance(); // `#`
        let name = self.read_directive_name();
        if name.is_empty() {
            return Err(ViewParseError {
                message: "expected directive name after `{#`".into(),
                line: start_line,
                column: start_col,
            });
        }
        match name.as_str() {
            "if" => self.parse_if_directive(start_line, start_col),
            "for" => self.parse_for_directive(start_line, start_col),
            "else" => Err(ViewParseError {
                message: "unexpected `{#else}` — must appear inside an `{#if}` body, and only one `{#else}` per `{#if}` is allowed".into(),
                line: start_line,
                column: start_col,
            }),
            other => Err(ViewParseError {
                message: format!(
                    "unknown template directive `{{#{other}}}` — the template supports `{{#if}}`, `{{#for}}` and `{{#else}}` (inside an `{{#if}}`)"
                ),
                line: start_line,
                column: start_col,
            }),
        }
    }

    /// Parse an `{#if cond} ... {/if}` or `{#if cond} ... {#else}
    /// ... {/if}` block. Called after `parse_directive_open` has
    /// consumed `{#if`. The then branch parses with
    /// `accept_else = true`, so `{#else}` in that scope terminates
    /// the branch; if hit, the else branch parses next with
    /// `accept_else = false`, which turns any second `{#else}` into
    /// a targeted error via `parse_directive_open`'s "else" arm.
    fn parse_if_directive(
        &mut self,
        start_line: usize,
        start_col: usize,
    ) -> ViewParseResult<TemplateNode> {
        // Capture the cond expression as raw text up to the
        // matching `}` at directive-open depth 0. Nested `{}` (e.g.
        // struct or map literals inside the cond) are tracked so we
        // don't stop at the wrong brace.
        self.skip_ws_inline();
        let cond_raw = self.capture_directive_arg_raw(start_line, start_col)?;
        // Recurse for the then-body up to `{/if}` or `{#else}`.
        let (children, saw_else) = self.parse_nodes(None, Some("if"), true)?;
        let else_children = if saw_else {
            // Parse the else branch, up to `{/if}`. `accept_else =
            // false` here rejects a second `{#else}` cleanly at
            // `parse_directive_open`'s "else" arm.
            let (else_kids, _) = self.parse_nodes(None, Some("if"), false)?;
            Some(else_kids)
        } else {
            None
        };
        Ok(TemplateNode::If {
            cond_raw: cond_raw.trim().to_string(),
            children,
            else_children,
            loc: Loc::new(start_line, start_col),
        })
    }

    /// Parse a `{#for x in xs} ... {/for}` block. Called after
    /// `parse_directive_open` has consumed `{#for`. Only accepts a
    /// single bare identifier as the binding — compound patterns
    /// like `(k, v)` for Map and index tuples like `(x, i)` are
    /// deferred (would need `Pattern::Tuple` from the classic AST,
    /// out of scope for this mini-commit). The iter expression is
    /// captured raw up to the closing `}` at directive depth 0 and
    /// re-parsed in `expand`.
    fn parse_for_directive(
        &mut self,
        start_line: usize,
        start_col: usize,
    ) -> ViewParseResult<TemplateNode> {
        self.skip_ws_inline();
        let var_line = self.line;
        let var_col = self.column;
        let var = self.read_directive_name();
        if var.is_empty() {
            return Err(ViewParseError {
                message: "expected binding identifier after `{#for` (only bare identifiers are supported — compound patterns `(k, v)` for Map deferred to later mini-commit)".into(),
                line: var_line,
                column: var_col,
            });
        }
        self.skip_ws_inline();
        // Expect literal `in` keyword. We can't just `read_directive_name`
        // and compare because the user might type `,` or `=` here —
        // spot the specific failures for a targeted message.
        if !self.consume_keyword_in() {
            return Err(ViewParseError {
                message: format!(
                    "expected `in` after `{{#for {var}` — only `{{#for x in xs}}` shape is supported (compound patterns and index bindings deferred to later mini-commit)"
                ),
                line: self.line,
                column: self.column,
            });
        }
        self.skip_ws_inline();
        let raw_arg = self.capture_directive_arg_raw(start_line, start_col)?;
        // Split off an optional `key=<expr>` clause (keyed-diffing
        // sugar) from the iterable expression. The `key` marker is a
        // standalone identifier at bracket/brace/paren depth 0 outside
        // string literals, immediately followed (after optional
        // whitespace) by a single `=`.
        let (iter_trimmed, key_trimmed) = split_for_iter_key(&raw_arg);
        if iter_trimmed.is_empty() {
            return Err(ViewParseError {
                message: format!(
                    "expected iter expression after `{{#for {var} in` — `{{#for {var} in }}` is empty"
                ),
                line: start_line,
                column: start_col,
            });
        }
        if let Some(ref k) = key_trimmed {
            if k.is_empty() {
                return Err(ViewParseError {
                    message: format!(
                        "expected key expression after `key=` in `{{#for {var} in {iter_trimmed} key=}}` — the `key=` clause is empty"
                    ),
                    line: start_line,
                    column: start_col,
                });
            }
        }
        let (children, _) = self.parse_nodes(None, Some("for"), false)?;
        Ok(TemplateNode::For {
            var,
            iter_raw: iter_trimmed,
            key_raw: key_trimmed,
            children,
            loc: Loc::new(start_line, start_col),
        })
    }

    /// Consume the literal keyword `in` if present at the cursor.
    /// Requires a word boundary after the keyword (so `interior`
    /// doesn't match) — the next char must be whitespace or `{`
    /// (unlikely, but keeps the check symmetric).
    fn consume_keyword_in(&mut self) -> bool {
        if self.peek() != Some('i') || self.peek_at(1) != Some('n') {
            return false;
        }
        // Look at the char after `in` — must be whitespace or `{`
        // for a word boundary. This distinguishes `in xs` from
        // `interior_var`.
        let next = self.peek_at(2);
        let is_boundary = matches!(next, Some(' ' | '\t' | '\n' | '\r') | Some('{') | None);
        if !is_boundary {
            return false;
        }
        self.advance(); // `i`
        self.advance(); // `n`
        true
    }

    /// Read the identifier following `{#` or `{/` — restricted to
    /// ASCII alphanumeric + `_`. Matches the shape of the view
    /// lexer's `read_ident`, kept separate because the HTML sub-
    /// parser walks char-by-char instead of holding tokens.
    fn read_directive_name(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        s
    }

    /// Skip inline whitespace (space + tab) but NOT newlines.
    /// Directive openers like `{#if cond}` must fit on the "logical
    /// line" between the opener and its closing `}` — a newline in
    /// the middle is legal, but we don't want to swallow one when
    /// looking for the `}` boundary.
    fn skip_ws_inline(&mut self) {
        while matches!(self.peek(), Some(' ') | Some('\t')) {
            self.advance();
        }
    }

    /// Capture the raw argument of a directive opener up to the
    /// matching `}` at depth 0. Tracks nested `{}` so a struct or
    /// map literal inside the cond doesn't terminate early. On EOF
    /// before the closer, error with the directive opener's
    /// position.
    fn capture_directive_arg_raw(
        &mut self,
        start_line: usize,
        start_col: usize,
    ) -> ViewParseResult<String> {
        let mut out = String::new();
        let mut depth: usize = 0;
        loop {
            match self.peek() {
                None => {
                    return Err(ViewParseError {
                        message: "unterminated directive opener — expected `}`".into(),
                        line: start_line,
                        column: start_col,
                    });
                }
                Some('{') => {
                    depth += 1;
                    out.push('{');
                    self.advance();
                }
                Some('}') if depth > 0 => {
                    depth -= 1;
                    out.push('}');
                    self.advance();
                }
                Some('}') => {
                    self.advance();
                    return Ok(out);
                }
                Some(c) => {
                    out.push(c);
                    self.advance();
                }
            }
        }
    }

    fn parse_interpolation(&mut self) -> ViewParseResult<TemplateNode> {
        let start_line = self.line;
        let start_col = self.column;
        self.advance(); // `{`
        let mut expr = String::new();
        let mut depth = 1_usize;
        loop {
            match self.peek() {
                None => {
                    return Err(ViewParseError {
                        message: "unterminated interpolation — expected `}`".into(),
                        line: start_line,
                        column: start_col,
                    });
                }
                Some('{') => {
                    depth += 1;
                    expr.push('{');
                    self.advance();
                }
                Some('}') => {
                    depth -= 1;
                    if depth == 0 {
                        self.advance();
                        return Ok(TemplateNode::Interpolation {
                            expr_raw: expr.trim().to_string(),
                            loc: Loc::new(start_line, start_col),
                        });
                    }
                    expr.push('}');
                    self.advance();
                }
                Some(c) => {
                    expr.push(c);
                    self.advance();
                }
            }
        }
    }

    /// §9.ee V-1 (2026-07-16) — Consume an HTML5 comment
    /// `<!-- ... -->` starting from the `!` char (the leading `<`
    /// was already advanced by `parse_element`). Returns `Ok(())`
    /// on clean consumption; error if the shape doesn't match
    /// (`<!DOCTYPE ...>` etc — deferred) or if the comment is
    /// unterminated. Nested comments are NOT supported (matching
    /// the HTML5 spec — `<!-- outer <!-- inner --> outer -->`
    /// closes on the first `-->`).
    fn parse_html_comment(&mut self, start_line: usize, start_col: usize) -> ViewParseResult<()> {
        // Consume `!`
        self.advance();
        // Expect `--` to open the comment. Anything else (e.g.
        // `<!DOCTYPE html>`, `<![CDATA[ ... ]]>`) is rejected with
        // a targeted hint that these forms are deferred.
        if self.peek() != Some('-') || self.peek_at(1) != Some('-') {
            return Err(ViewParseError {
                message: "expected `<!-- ...` for HTML comment; only \
                          `<!-- ... -->` is supported today. `<!DOCTYPE ...>` \
                          and `<![CDATA[ ... ]]>` are deferred (rare in \
                          component templates — top-level `<!DOCTYPE html>` \
                          belongs to the framework layout, not templates)."
                    .into(),
                line: start_line,
                column: start_col,
            });
        }
        self.advance(); // first `-`
        self.advance(); // second `-`
                        // Read until `-->`.
        loop {
            match self.peek() {
                Some('-') if self.peek_at(1) == Some('-') && self.peek_at(2) == Some('>') => {
                    self.advance(); // `-`
                    self.advance(); // `-`
                    self.advance(); // `>`
                    return Ok(());
                }
                Some(_) => {
                    self.advance();
                }
                None => {
                    return Err(ViewParseError {
                        message: "unterminated HTML comment — expected `-->` \
                                  to close"
                            .into(),
                        line: start_line,
                        column: start_col,
                    });
                }
            }
        }
    }

    fn parse_element(&mut self) -> ViewParseResult<TemplateNode> {
        let start_line = self.line;
        let start_col = self.column;
        self.advance(); // `<`
                        // §9.ee V-1 (2026-07-16) — HTML5 comment `<!-- ... -->`
                        // support. When the char after `<` is `!`, dispatch to the
                        // comment consumer (discards the content and returns an empty
                        // Text node — comments produce no user-visible output; the
                        // downstream SSR/WASM emit for Text("") is a no-op). Only
                        // `<!-- ... -->` is accepted today; `<!DOCTYPE ...>` and
                        // `<![CDATA[ ... ]]>` remain deferred (rare in `.fitzv`
                        // component templates — the top-level `<!DOCTYPE html>`
                        // belongs to the framework's `live_layout`, not to a
                        // component template).
        if self.peek() == Some('!') {
            self.parse_html_comment(start_line, start_col)?;
            return Ok(TemplateNode::Text(String::new()));
        }
        let tag = self.read_tag_name();
        if tag.is_empty() {
            return Err(ViewParseError {
                message: "expected tag name after `<`".into(),
                line: start_line,
                column: start_col,
            });
        }
        let mut attrs = Vec::new();
        loop {
            self.skip_ws_inside_tag();
            match self.peek() {
                Some('/') => {
                    self.advance();
                    if self.peek() != Some('>') {
                        return Err(ViewParseError {
                            message: format!("expected `>` after `/` in self-closing `<{tag}/>`"),
                            line: self.line,
                            column: self.column,
                        });
                    }
                    self.advance();
                    // `<slot ... />` is a distinct AST node (opaque
                    // parent/child composition marker) — the only
                    // attribute we accept is `name="X"`, no events,
                    // no interpolations. Reject anything else with a
                    // targeted message that points at 11.5.
                    if tag == "slot" {
                        return build_slot(attrs, Vec::new(), start_line, start_col);
                    }
                    return Ok(TemplateNode::Element {
                        tag,
                        attrs,
                        children: Vec::new(),
                        self_closing: true,
                        loc: Loc::new(start_line, start_col),
                    });
                }
                Some('>') => {
                    self.advance();
                    // Phase 11.7.d — `<slot>...</slot>` captures its children
                    // as fallback content, rendered when the parent provides
                    // nothing for the slot.
                    if tag == "slot" {
                        let (fallback, _) = self.parse_nodes(Some(&tag), None, false)?;
                        return build_slot(attrs, fallback, start_line, start_col);
                    }
                    let (children, _) = self.parse_nodes(Some(&tag), None, false)?;
                    return Ok(TemplateNode::Element {
                        tag,
                        attrs,
                        children,
                        self_closing: false,
                        loc: Loc::new(start_line, start_col),
                    });
                }
                Some(_) => {
                    let attr = self.parse_attribute()?;
                    attrs.push(attr);
                }
                None => {
                    return Err(ViewParseError {
                        message: format!("unterminated `<{tag}...>` — expected `>` or `/>`"),
                        line: start_line,
                        column: start_col,
                    });
                }
            }
        }
    }

    fn parse_attribute(&mut self) -> ViewParseResult<Attr> {
        let start_line = self.line;
        let start_col = self.column;
        let name = self.read_attr_name();
        if name.is_empty() {
            return Err(ViewParseError {
                message: "expected attribute name".into(),
                line: start_line,
                column: start_col,
            });
        }
        self.skip_ws_inside_tag();
        // §9.ee V-2 (2026-07-16) — Bare boolean HTML attribute
        // support. HTML5 spec permits `<input required>` as a shorthand
        // for `<input required="">` (empty-string value) or `<input
        // required="required">` (self-referential). We normalise to
        // empty-string value in the AST — SSR emitters can render as
        // bare or with `=""` at their preference. Covers the common
        // real-world cases: `required`, `disabled`, `readonly`,
        // `checked`, `selected`, `autofocus`, `autoplay`, `controls`,
        // `loop`, `muted`, `open`, `hidden`, plus the fitz-liveviews
        // conventions `data-flv-clear`, `data-flv-root`. If the caller
        // wants a specific value they can still use `attr="val"`.
        if self.peek() != Some('=') {
            return Ok(Attr::Static {
                name,
                value: String::new(),
                loc: Loc::new(start_line, start_col),
            });
        }
        self.advance(); // `=`
        self.skip_ws_inside_tag();
        // Form B (gotcha #6) — an UNQUOTED brace after `=` binds a
        // conditional boolean attribute: `checked={expr}`. The attribute
        // is present in the DOM iff `expr` is truthy (the HTML
        // boolean-attribute model). Distinct from the QUOTED
        // `checked="{expr}"`, which is always-present with a stringified
        // value. Events (`@click=…`) must stay quoted, so exclude `@`.
        if self.peek() == Some('{') && !name.starts_with('@') {
            self.advance(); // `{`
            let mut expr = String::new();
            let mut depth = 1_usize;
            loop {
                match self.peek() {
                    None => {
                        return Err(ViewParseError {
                            message: format!(
                                "unmatched `{{` in boolean attribute `{name}` — expected `}}`"
                            ),
                            line: start_line,
                            column: start_col,
                        });
                    }
                    Some('{') => {
                        depth += 1;
                        expr.push('{');
                        self.advance();
                    }
                    Some('}') => {
                        depth -= 1;
                        if depth == 0 {
                            self.advance();
                            break;
                        }
                        expr.push('}');
                        self.advance();
                    }
                    Some(c) => {
                        expr.push(c);
                        self.advance();
                    }
                }
            }
            return Ok(Attr::BoolInterpolation {
                name,
                expr_raw: expr.trim().to_string(),
                loc: Loc::new(start_line, start_col),
            });
        }
        if self.peek() != Some('"') {
            return Err(ViewParseError {
                message: format!("attribute `{name}` value must be double-quoted in the POC"),
                line: self.line,
                column: self.column,
            });
        }
        self.advance(); // opening `"`
        let raw_value = self.read_attr_value()?;

        // Classify: `@click="handler"` -> Event, `="{expr}"` ->
        // Interpolation, everything else -> Static.
        if let Some(event_name) = name.strip_prefix('@') {
            return Ok(Attr::Event {
                event_name: event_name.to_string(),
                handler_raw: raw_value,
                loc: Loc::new(start_line, start_col),
            });
        }
        if let Some(inner) = extract_full_interp(&raw_value) {
            return Ok(Attr::Interpolation {
                name,
                expr_raw: inner.trim().to_string(),
                loc: Loc::new(start_line, start_col),
            });
        }
        Ok(Attr::Static {
            name,
            value: raw_value,
            loc: Loc::new(start_line, start_col),
        })
    }

    fn read_attr_value(&mut self) -> ViewParseResult<String> {
        // v0.37.17 — gotcha #1: an attribute value may contain a Fitz
        // interpolation `{expr}` whose expression has string literals
        // with double quotes (`placeholder="{t(locale, "dep.ph")}"`).
        // A naive "close on the first `\"`" terminated the value early
        // and produced "expected attribute name". We track `{...}`
        // brace depth and a nested-string flag: only a `"` at brace
        // depth 0 (outside any interpolation) closes the attribute; a
        // `"` inside a `{...}` toggles the nested-string state and is
        // kept verbatim (so the `{`/`}` inside that string are literal
        // and the downstream classic Fitz parser re-lexes it). Values
        // without a nested `"` behave identically to before
        // (byte-compatible: `brace_depth` only matters once a nested
        // quote appears).
        let mut s = String::new();
        let mut brace_depth: i32 = 0;
        let mut in_expr_str = false;
        loop {
            match self.peek() {
                // Top-level `"` closes the attribute value.
                Some('"') if brace_depth == 0 => {
                    self.advance();
                    return Ok(s);
                }
                // A `"` inside an interpolation opens/closes a nested
                // string literal; keep it and do NOT terminate.
                Some('"') => {
                    in_expr_str = !in_expr_str;
                    s.push('"');
                    self.advance();
                }
                // Inside an interpolation, preserve a Fitz escape (`\"`,
                // `\\`, ...) verbatim so `parse_expr_at` re-lexes it.
                // Only active within braces to stay byte-compatible with
                // static HTML values (where `\` is a literal char).
                Some('\\') if brace_depth > 0 => {
                    s.push('\\');
                    self.advance();
                    if let Some(n) = self.advance() {
                        s.push(n);
                    }
                }
                // `{`/`}` change interpolation depth, but not inside a
                // nested string (there they are literal).
                Some('{') if !in_expr_str => {
                    brace_depth += 1;
                    s.push('{');
                    self.advance();
                }
                Some('}') if !in_expr_str && brace_depth > 0 => {
                    brace_depth -= 1;
                    s.push('}');
                    self.advance();
                }
                Some('\n') => {
                    return Err(ViewParseError {
                        message:
                            "attribute value contains a raw newline — quote the value carefully"
                                .into(),
                        line: self.line,
                        column: self.column,
                    });
                }
                Some(c) => {
                    s.push(c);
                    self.advance();
                }
                None => {
                    // A run-away brace depth at EOF means the interpolation
                    // `{...}` in the value was never closed (e.g.
                    // `class="badge-{kind"` — the `"` looked like a nested
                    // string quote because it sat inside the open `{`).
                    // Report it as an unmatched brace (clearer, and keeps
                    // the pre-v0.37.17 error contract for that input).
                    let message = if brace_depth > 0 {
                        "unmatched `{` in attribute value — the interpolation was not closed with `}`"
                    } else {
                        "unterminated attribute value — expected closing `\"`"
                    };
                    return Err(ViewParseError {
                        message: message.into(),
                        line: self.line,
                        column: self.column,
                    });
                }
            }
        }
    }

    fn read_tag_name(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        s
    }

    fn read_attr_name(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '@' || c == ':' {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        s
    }

    fn skip_ws_inside_tag(&mut self) {
        while let Some(c) = self.peek() {
            if c == ' ' || c == '\t' || c == '\n' || c == '\r' {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn read_text_until_special(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c == '<' || c == '{' {
                break;
            }
            s.push(c);
            self.advance();
        }
        s
    }
}

/// Convert the attribute list captured for a self-closing `<slot />`
/// element into a `TemplateNode::Slot`. Accepts at most one `name`
/// attribute (`Static`), rejects everything else with a targeted
/// message that points at 11.5 (composition wiring) as the sub-phase
/// that will add richer slot APIs.
fn build_slot(
    attrs: Vec<Attr>,
    fallback: Vec<TemplateNode>,
    line: usize,
    column: usize,
) -> ViewParseResult<TemplateNode> {
    let mut name: Option<String> = None;
    for attr in attrs {
        match attr {
            Attr::Static {
                name: attr_name,
                value,
                ..
            } if attr_name == "name" => {
                if name.is_some() {
                    return Err(ViewParseError {
                        message: "`<slot>` accepts a single `name` attribute at most; duplicates are not allowed".into(),
                        line,
                        column,
                    });
                }
                name = Some(value);
            }
            Attr::Static {
                name: attr_name, ..
            } => {
                return Err(ViewParseError {
                    message: format!(
                        "`<slot>` accepts only the `name` attribute; got `{attr_name}` (11.5 will add richer slot APIs — props, defaults, scoped bindings)"
                    ),
                    line,
                    column,
                });
            }
            Attr::Interpolation {
                name: attr_name, ..
            } => {
                return Err(ViewParseError {
                    message: format!(
                        "`<slot>` does not accept interpolated attributes — `{attr_name}` must be a plain string literal"
                    ),
                    line,
                    column,
                });
            }
            Attr::Event { event_name, .. } => {
                return Err(ViewParseError {
                    message: format!(
                        "`<slot>` does not accept event bindings; `@{event_name}` is not allowed"
                    ),
                    line,
                    column,
                });
            }
            Attr::BoolInterpolation {
                name: attr_name, ..
            } => {
                return Err(ViewParseError {
                    message: format!(
                        "`<slot>` does not accept conditional boolean attributes — `{attr_name}={{…}}` is not allowed"
                    ),
                    line,
                    column,
                });
            }
        }
    }
    Ok(TemplateNode::Slot {
        name,
        fallback,
        loc: Loc::new(line, column),
    })
}

/// Returns the inside of `"{...}"` when the value is 100%
/// interpolation (no literal parts); `None` if it's a mix or pure
/// static. The POC requires attribute values to be fully static or
/// fully interpolated.
fn extract_full_interp(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    let stripped = trimmed.strip_prefix('{')?.strip_suffix('}')?;
    // Reject `{a}{b}` mixed — if there's an unbalanced `{` or `}`
    // inside, fall back to the Static path to avoid confusing the
    // user.
    let mut depth = 0_i32;
    for c in stripped.chars() {
        match c {
            '{' => depth += 1,
            '}' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    if depth == 0 {
        Some(stripped)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical Card SFC used across the POC:
    ///   - state with 2 annotated fields + literal defaults
    ///   - 2 event handlers, one without params and one with params
    ///   - template with nested Elements + static attr + event attr + interpolation
    ///   - opaque scoped style
    const CARD_SRC: &str = r#"component Card {
  state {
    title: Str = "Untitled"
    is_editing: Bool = false
  }

  event start() {
    is_editing = true
  }

  event save(new_title: Str) {
    title = new_title
    is_editing = false
  }

  <template>
    <div class="card">
      <div class="title">{title}</div>
      <button @click="start">Edit</button>
    </div>
  </template>

  <style scoped>
    .card { border: 1px solid #ccc; padding: 1rem; }
    .title { font-weight: bold; }
  </style>
}
"#;

    #[test]
    fn parses_the_card_component_shell() {
        let file = parse(CARD_SRC).expect("Card parses cleanly");
        assert_eq!(file.components.len(), 1);
        let c = &file.components[0];
        assert_eq!(c.name, "Card");
        assert_eq!(c.state.len(), 2);
        assert_eq!(c.events.len(), 2);
        assert!(c.template.is_some());
        assert!(c.style.is_some());
    }

    #[test]
    fn state_fields_capture_type_and_default_raw() {
        let file = parse(CARD_SRC).unwrap();
        let c = &file.components[0];
        assert_eq!(c.state[0].name, "title");
        assert_eq!(c.state[0].type_expr_raw, "Str");
        assert_eq!(c.state[0].default_expr_raw, "\"Untitled\"");
        assert_eq!(c.state[1].name, "is_editing");
        assert_eq!(c.state[1].type_expr_raw, "Bool");
        assert_eq!(c.state[1].default_expr_raw, "false");
    }

    #[test]
    fn event_handlers_capture_params_and_body_raw() {
        let file = parse(CARD_SRC).unwrap();
        let c = &file.components[0];
        assert_eq!(c.events[0].name, "start");
        assert_eq!(c.events[0].params_raw, "");
        assert!(c.events[0].body_raw.contains("is_editing"));
        assert!(c.events[0].body_raw.contains("true"));

        assert_eq!(c.events[1].name, "save");
        assert!(c.events[1].params_raw.contains("new_title"));
        assert!(c.events[1].params_raw.contains(":"));
        assert!(c.events[1].body_raw.contains("title"));
        assert!(c.events[1].body_raw.contains("new_title"));
    }

    #[test]
    fn template_body_produces_nested_element_tree() {
        let file = parse(CARD_SRC).unwrap();
        let template = file.components[0].template.as_ref().unwrap();
        // Walking the roots ignores leading whitespace text nodes.
        let outer_div = template
            .roots
            .iter()
            .find(|n| matches!(n, TemplateNode::Element { tag, .. } if tag == "div"))
            .expect("outer <div> exists");
        let (children,) = match outer_div {
            TemplateNode::Element { children, .. } => (children,),
            _ => unreachable!(),
        };
        let inner_elements: Vec<_> = children
            .iter()
            .filter(|n| matches!(n, TemplateNode::Element { .. }))
            .collect();
        assert_eq!(
            inner_elements.len(),
            2,
            "inner elements: title div + button"
        );
        // First inner is `<div class="title">{title}</div>`.
        match inner_elements[0] {
            TemplateNode::Element {
                tag,
                attrs,
                children,
                ..
            } => {
                assert_eq!(tag, "div");
                assert_eq!(attrs.len(), 1);
                assert!(matches!(&attrs[0], Attr::Static { name, value, .. }
                    if name == "class" && value == "title"));
                assert!(children
                    .iter()
                    .any(|c| matches!(c, TemplateNode::Interpolation { expr_raw, .. }
                    if expr_raw == "title")));
            }
            _ => unreachable!(),
        }
        // Second inner is `<button @click="start">Edit</button>`.
        match inner_elements[1] {
            TemplateNode::Element {
                tag,
                attrs,
                children,
                ..
            } => {
                assert_eq!(tag, "button");
                assert_eq!(attrs.len(), 1);
                assert!(
                    matches!(&attrs[0], Attr::Event { event_name, handler_raw, .. }
                    if event_name == "click" && handler_raw == "start")
                );
                assert!(children
                    .iter()
                    .any(|c| matches!(c, TemplateNode::Text(s) if s.contains("Edit"))));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn style_scoped_body_captured_as_opaque_css() {
        let file = parse(CARD_SRC).unwrap();
        let css = &file.components[0].style.as_ref().unwrap().css_raw;
        assert!(css.contains(".card"));
        assert!(css.contains("border: 1px solid #ccc"));
        assert!(css.contains(".title"));
    }

    #[test]
    fn parse_prints_the_ast_for_inspection() {
        // Run with `cargo test -- --nocapture` to see the AST.
        let file = parse(CARD_SRC).unwrap();
        println!("--- Parsed .fitzv AST (Phase 11 POC) ---\n{:#?}", file);
    }

    #[test]
    fn empty_component_shell_parses() {
        let file = parse("component Empty {}").unwrap();
        assert_eq!(file.components.len(), 1);
        assert_eq!(file.components[0].name, "Empty");
        assert!(file.components[0].state.is_empty());
        assert!(file.components[0].events.is_empty());
        assert!(file.components[0].template.is_none());
        assert!(file.components[0].style.is_none());
    }

    #[test]
    fn duplicate_template_block_errors_clearly() {
        let src = r#"component X {
  <template><div>a</div></template>
  <template><div>b</div></template>
}"#;
        let err = parse(src).unwrap_err();
        assert!(err.message.contains("duplicate `<template>`"));
    }

    #[test]
    fn duplicate_scoped_style_block_errors_clearly() {
        // Two `<style scoped>` in a row — the second one triggers
        // the duplicate check. The message names both accepted
        // opt-ins since 11.3.a so users know they can also use
        // `<style global>` (just not more than one style block
        // total per component).
        let src = r#"component X {
  <style scoped>a{}</style>
  <style scoped>b{}</style>
}"#;
        let err = parse(src).unwrap_err();
        assert!(err.message.contains("duplicate `<style>`"));
        assert!(err.message.contains("<style scoped>"));
        assert!(err.message.contains("<style global>"));
    }

    #[test]
    fn duplicate_global_style_block_errors_clearly() {
        let src = r#"component X {
  <style global>a{}</style>
  <style global>b{}</style>
}"#;
        let err = parse(src).unwrap_err();
        assert!(err.message.contains("duplicate `<style>`"));
    }

    #[test]
    fn scoped_and_global_style_together_rejected_as_duplicate() {
        // MVP rule: one `<style>` block per component regardless of
        // kind. Vue and Svelte both allow "one scoped + one global"
        // side by side, and Fitz will get there once demand appears
        // — but 11.3.a keeps the cap at one to avoid deciding
        // ordering / merge semantics prematurely.
        let src = r#"component X {
  <style scoped>a{}</style>
  <style global>b{}</style>
}"#;
        let err = parse(src).unwrap_err();
        assert!(err.message.contains("duplicate `<style>`"));
    }

    #[test]
    fn global_style_parses_with_kind_global() {
        // Since 11.3.a: `<style global>` is a first-class style
        // block; the parser reads the raw CSS and stores it on the
        // component with `StyleKind::Global`.
        let src = r#"component X {
  <style global>body { margin: 0; }</style>
}"#;
        let file = parse(src).unwrap();
        let style = file.components[0].style.as_ref().unwrap();
        assert_eq!(style.kind, super::super::ast::StyleKind::Global);
        assert_eq!(style.css_raw.trim(), "body { margin: 0; }");
    }

    #[test]
    fn scoped_style_parses_with_kind_scoped() {
        // Regression for the pre-11.3.a shape: what used to be the
        // only style block form still parses correctly, and the
        // `kind` field now carries `StyleKind::Scoped` explicitly.
        let src = r#"component X {
  <style scoped>.card { padding: 8px; }</style>
}"#;
        let file = parse(src).unwrap();
        let style = file.components[0].style.as_ref().unwrap();
        assert_eq!(style.kind, super::super::ast::StyleKind::Scoped);
        assert_eq!(style.css_raw.trim(), ".card { padding: 8px; }");
    }

    #[test]
    fn expected_shape_error_names_both_style_forms() {
        // The "expected `state`, `event`, ..." error path — hit
        // when a stray identifier or delimiter shows up at the top
        // level of the component body — now names both accepted
        // style shapes so users looking at the error see the two
        // options at once.
        let src = r#"component X { foo }"#;
        let err = parse(src).unwrap_err();
        assert!(err.message.contains("<style scoped>"));
        assert!(err.message.contains("<style global>"));
    }

    #[test]
    fn interpolation_with_dotted_expression_is_captured_raw() {
        let src = r#"component X {
  <template><span>{user.name}</span></template>
}"#;
        let file = parse(src).unwrap();
        let template = file.components[0].template.as_ref().unwrap();
        let span = &template.roots[0];
        match span {
            TemplateNode::Element { children, .. } => {
                assert!(children
                    .iter()
                    .any(|c| matches!(c, TemplateNode::Interpolation { expr_raw, .. }
                    if expr_raw == "user.name")));
            }
            _ => panic!("expected <span> element"),
        }
    }

    #[test]
    fn attribute_full_interpolation_becomes_interpolation_kind() {
        let src = r#"component X {
  <template><input value="{title}" /></template>
}"#;
        let file = parse(src).unwrap();
        let input = &file.components[0].template.as_ref().unwrap().roots[0];
        match input {
            TemplateNode::Element {
                attrs,
                self_closing,
                ..
            } => {
                assert!(*self_closing);
                assert!(
                    matches!(&attrs[0], Attr::Interpolation { name, expr_raw, .. }
                    if name == "value" && expr_raw == "title")
                );
            }
            _ => panic!("expected <input/> element"),
        }
    }

    #[test]
    fn attr_value_nested_double_quotes_full_interp_v0_37_17() {
        // gotcha #1 — a `"` inside the `{...}` interpolation of an
        // attribute value must NOT terminate the value early.
        let src = r#"component X {
  <template><input placeholder="{t(locale, "dep.ph")}" /></template>
}"#;
        let file = parse(src).expect("parse ok con comillas anidadas en atributo");
        let input = &file.components[0].template.as_ref().unwrap().roots[0];
        match input {
            TemplateNode::Element { attrs, .. } => {
                assert!(
                    matches!(&attrs[0], Attr::Interpolation { name, expr_raw, .. }
                    if name == "placeholder" && expr_raw == "t(locale, \"dep.ph\")"),
                    "esperaba Interpolation con expr_raw completo, got: {:?}",
                    attrs[0]
                );
            }
            _ => panic!("expected <input/> element"),
        }
    }

    #[test]
    fn attr_value_nested_quotes_mixed_preserves_full_value_v0_37_17() {
        // Mixed literal + interpolation with nested quotes → Static
        // whose value survives whole (expand turns it into
        // MixedInterpolation later).
        let src = r#"component X {
  <template><span title="Hi {t(l, "x")}">y</span></template>
}"#;
        let file = parse(src).expect("parse ok mixed con comillas");
        let span = &file.components[0].template.as_ref().unwrap().roots[0];
        match span {
            TemplateNode::Element { attrs, .. } => {
                assert!(
                    matches!(&attrs[0], Attr::Static { name, value, .. }
                    if name == "title" && value == "Hi {t(l, \"x\")}"),
                    "esperaba Static con el valor completo, got: {:?}",
                    attrs[0]
                );
            }
            _ => panic!("expected <span> element"),
        }
    }

    #[test]
    fn attr_value_plain_static_unchanged_v0_37_17() {
        // Byte-compat: un valor sin comillas anidadas parsea idéntico.
        let src = r#"component X {
  <template><div class="card"></div></template>
}"#;
        let file = parse(src).unwrap();
        let div = &file.components[0].template.as_ref().unwrap().roots[0];
        match div {
            TemplateNode::Element { attrs, .. } => {
                assert!(matches!(&attrs[0], Attr::Static { name, value, .. }
                    if name == "class" && value == "card"));
            }
            _ => panic!("expected <div> element"),
        }
    }

    #[test]
    fn mismatched_closing_tag_errors_clearly() {
        let src = r#"component X {
  <template><div><span></div></template>
}"#;
        let err = parse(src).unwrap_err();
        assert!(err.message.contains("mismatched closing tag"));
    }

    // ---- 11.2.c mini-commit 1: `{#if cond}...{/if}` -----------------

    #[test]
    fn template_if_block_captured_with_cond_raw_and_children() {
        // Basic `{#if}` opener + child text + `{/if}` closer.
        let src = r#"component X {
  <template>{#if is_ready}<div>hi</div>{/if}</template>
}"#;
        let file = parse(src).unwrap();
        let template = file.components[0].template.as_ref().unwrap();
        assert_eq!(template.roots.len(), 1, "one root If node expected");
        match &template.roots[0] {
            TemplateNode::If {
                cond_raw, children, ..
            } => {
                assert_eq!(cond_raw, "is_ready");
                // Expect one Element("div") child.
                assert_eq!(children.len(), 1);
                match &children[0] {
                    TemplateNode::Element { tag, .. } => assert_eq!(tag, "div"),
                    other => panic!("expected Element child, got {:?}", other),
                }
            }
            other => panic!("expected If, got {:?}", other),
        }
    }

    #[test]
    fn template_if_block_cond_captures_nested_braces_in_expr() {
        // The cond expression contains a struct-literal-ish `{...}`
        // inside a function call — the capture must respect brace
        // depth and not stop at the inner `}`.
        let src = r#"component X {
  <template>{#if has_key(m, "x")}<span/>{/if}</template>
}"#;
        let file = parse(src).unwrap();
        let template = file.components[0].template.as_ref().unwrap();
        match &template.roots[0] {
            TemplateNode::If { cond_raw, .. } => {
                assert_eq!(cond_raw, "has_key(m, \"x\")");
            }
            other => panic!("expected If, got {:?}", other),
        }
    }

    #[test]
    fn template_if_block_nested_inside_element() {
        // `{#if}` living inside an `<ul>`. The element sees the If
        // as one of its children; walk asserts both layers.
        let src = r#"component X {
  <template><ul>{#if any}<li>x</li>{/if}</ul></template>
}"#;
        let file = parse(src).unwrap();
        let template = file.components[0].template.as_ref().unwrap();
        match &template.roots[0] {
            TemplateNode::Element {
                tag: outer,
                children: outer_children,
                ..
            } => {
                assert_eq!(outer, "ul");
                assert_eq!(outer_children.len(), 1);
                match &outer_children[0] {
                    TemplateNode::If {
                        cond_raw,
                        children: inner,
                        ..
                    } => {
                        assert_eq!(cond_raw, "any");
                        assert_eq!(inner.len(), 1);
                    }
                    other => panic!("expected If inside <ul>, got {:?}", other),
                }
            }
            other => panic!("expected <ul>, got {:?}", other),
        }
    }

    #[test]
    fn template_if_block_element_nested_inside_if() {
        // Reverse: `<div>` nested inside `{#if}`.
        let src = r#"component X {
  <template>{#if any}<div><span/></div>{/if}</template>
}"#;
        let file = parse(src).unwrap();
        let template = file.components[0].template.as_ref().unwrap();
        match &template.roots[0] {
            TemplateNode::If { children, .. } => match &children[0] {
                TemplateNode::Element {
                    tag,
                    children: inner,
                    ..
                } => {
                    assert_eq!(tag, "div");
                    assert_eq!(inner.len(), 1);
                }
                other => panic!("expected <div> inside If, got {:?}", other),
            },
            other => panic!("expected If, got {:?}", other),
        }
    }

    #[test]
    fn template_if_block_nested_if_inside_if() {
        // Two levels of `{#if}` nesting. Each closes at its own
        // `{/if}` in the right order.
        let src = r#"component X {
  <template>{#if a}{#if b}<span/>{/if}{/if}</template>
}"#;
        let file = parse(src).unwrap();
        let template = file.components[0].template.as_ref().unwrap();
        match &template.roots[0] {
            TemplateNode::If {
                cond_raw: outer_cond,
                children: outer_children,
                ..
            } => {
                assert_eq!(outer_cond, "a");
                match &outer_children[0] {
                    TemplateNode::If {
                        cond_raw: inner_cond,
                        ..
                    } => {
                        assert_eq!(inner_cond, "b");
                    }
                    other => panic!("expected inner If, got {:?}", other),
                }
            }
            other => panic!("expected outer If, got {:?}", other),
        }
    }

    #[test]
    fn template_if_block_unterminated_errors_clearly() {
        // Opener without matching closer.
        let src = r#"component X {
  <template>{#if x}<div/></template>
}"#;
        let err = parse(src).unwrap_err();
        assert!(
            err.message.contains("unterminated `{#if}`")
                || err.message.contains("expected `{/if}`"),
            "unexpected error message: {}",
            err.message
        );
    }

    #[test]
    fn template_if_block_mismatched_directive_close_errors_clearly() {
        // `{#if}` opened, closed with `{/for}` (wrong name).
        let src = r#"component X {
  <template>{#if x}<div/>{/for}</template>
}"#;
        let err = parse(src).unwrap_err();
        assert!(
            err.message.contains("mismatched closing directive") && err.message.contains("if"),
            "unexpected error message: {}",
            err.message
        );
    }

    #[test]
    fn template_directive_close_without_opener_errors_clearly() {
        // Only a closer, no opener. Should error at the closer's
        // position, not silently pass.
        let src = r#"component X {
  <template>{/if}</template>
}"#;
        let err = parse(src).unwrap_err();
        assert!(
            err.message.contains("no matching") && err.message.contains("if"),
            "unexpected error message: {}",
            err.message
        );
    }

    #[test]
    fn template_unknown_directive_name_errors_clearly() {
        // Some hypothetical `{#while}` — not supported. The message
        // should mention `{#if}` and `{#for}` so the user sees the
        // supported set, plus 11.2.c mini-commit 3 for `#else` /
        // `<slot>`.
        let src = r#"component X {
  <template>{#while cond}<li/>{/while}</template>
}"#;
        let err = parse(src).unwrap_err();
        assert!(
            err.message.contains("unknown template directive") && err.message.contains("while"),
            "unexpected error message: {}",
            err.message
        );
    }

    // ---- 11.2.c mini-commit 2: `{#for x in xs}...{/for}` -------------

    #[test]
    fn template_for_block_basic_shape() {
        // Basic `{#for}` opener with a text-content child and `{/for}`
        // closer. The var + iter_raw are captured; children are text
        // between the delimiters.
        let src = r#"component X {
  <template>{#for x in xs}<li>{x}</li>{/for}</template>
}"#;
        let file = parse(src).expect("parse should succeed");
        let template = &file.components[0].template.as_ref().unwrap();
        assert_eq!(template.roots.len(), 1);
        match &template.roots[0] {
            TemplateNode::For {
                var,
                iter_raw,
                children,
                ..
            } => {
                assert_eq!(var, "x");
                assert_eq!(iter_raw, "xs");
                // <li>{x}</li>
                assert_eq!(children.len(), 1);
                match &children[0] {
                    TemplateNode::Element { tag, children, .. } => {
                        assert_eq!(tag, "li");
                        assert_eq!(children.len(), 1);
                        match &children[0] {
                            TemplateNode::Interpolation { expr_raw, .. } => {
                                assert_eq!(expr_raw, "x");
                            }
                            other => panic!("expected interpolation, got {other:?}"),
                        }
                    }
                    other => panic!("expected element, got {other:?}"),
                }
            }
            other => panic!("expected For, got {other:?}"),
        }
    }

    #[test]
    fn template_for_block_iter_is_complex_expr() {
        // The iter expression can be a call chain with nested braces
        // (map literals inside args). `capture_directive_arg_raw`
        // must track nesting depth, same as `{#if}`.
        let src = r#"component X {
  <template>{#for row in rows.filter(fn(r) => r.active)}<span/>{/for}</template>
}"#;
        let file = parse(src).expect("parse should succeed");
        match &file.components[0].template.as_ref().unwrap().roots[0] {
            TemplateNode::For { var, iter_raw, .. } => {
                assert_eq!(var, "row");
                assert_eq!(iter_raw, "rows.filter(fn(r) => r.active)");
            }
            other => panic!("expected For, got {other:?}"),
        }
    }

    #[test]
    fn template_for_block_nested_inside_element() {
        // `<ul>{#for x in xs}<li/>{/for}</ul>` — the `<ul>` element
        // sees the For as a single child.
        let src = r#"component X {
  <template><ul>{#for x in xs}<li/>{/for}</ul></template>
}"#;
        let file = parse(src).expect("parse should succeed");
        let roots = &file.components[0].template.as_ref().unwrap().roots;
        assert_eq!(roots.len(), 1);
        match &roots[0] {
            TemplateNode::Element { tag, children, .. } => {
                assert_eq!(tag, "ul");
                assert_eq!(children.len(), 1);
                assert!(matches!(children[0], TemplateNode::For { .. }));
            }
            other => panic!("expected <ul>, got {other:?}"),
        }
    }

    #[test]
    fn template_for_block_nested_element_inside() {
        // `{#for x in xs}<div><span/></div>{/for}` — the For body
        // contains a nested element.
        let src = r#"component X {
  <template>{#for x in xs}<div><span/></div>{/for}</template>
}"#;
        let file = parse(src).expect("parse should succeed");
        match &file.components[0].template.as_ref().unwrap().roots[0] {
            TemplateNode::For { children, .. } => {
                assert_eq!(children.len(), 1);
                match &children[0] {
                    TemplateNode::Element {
                        tag,
                        children: inner,
                        ..
                    } => {
                        assert_eq!(tag, "div");
                        assert_eq!(inner.len(), 1);
                        assert!(matches!(inner[0], TemplateNode::Element { .. }));
                    }
                    other => panic!("expected <div>, got {other:?}"),
                }
            }
            other => panic!("expected For, got {other:?}"),
        }
    }

    #[test]
    fn template_for_block_nested_for_and_if() {
        // Two levels of `{#for}` with an `{#if}` between them, each
        // closes at its own `{/…}` in the right order.
        let src = r#"component X {
  <template>{#for x in xs}{#if x.active}{#for tag in x.tags}<span>{tag}</span>{/for}{/if}{/for}</template>
}"#;
        let file = parse(src).expect("parse should succeed");
        match &file.components[0].template.as_ref().unwrap().roots[0] {
            TemplateNode::For {
                var,
                iter_raw,
                children,
                ..
            } => {
                assert_eq!(var, "x");
                assert_eq!(iter_raw, "xs");
                assert_eq!(children.len(), 1);
                match &children[0] {
                    TemplateNode::If {
                        cond_raw,
                        children: if_children,
                        ..
                    } => {
                        assert_eq!(cond_raw, "x.active");
                        assert_eq!(if_children.len(), 1);
                        match &if_children[0] {
                            TemplateNode::For {
                                var: v2,
                                iter_raw: i2,
                                ..
                            } => {
                                assert_eq!(v2, "tag");
                                assert_eq!(i2, "x.tags");
                            }
                            other => panic!("expected inner For, got {other:?}"),
                        }
                    }
                    other => panic!("expected If, got {other:?}"),
                }
            }
            other => panic!("expected For, got {other:?}"),
        }
    }

    #[test]
    fn template_for_block_missing_var_errors_clearly() {
        // `{#for in xs}` — no binding identifier.
        let src = r#"component X {
  <template>{#for in xs}<li/>{/for}</template>
}"#;
        let err = parse(src).unwrap_err();
        assert!(
            err.message.contains("binding identifier") || err.message.contains("expected"),
            "unexpected error message: {}",
            err.message
        );
    }

    #[test]
    fn template_for_block_missing_in_errors_clearly() {
        // `{#for x , xs}` — no `in` keyword between var and iter.
        // Compound patterns like `(k, v)` or `(x, i)` are the
        // canonical case for this — error must guide the user.
        let src = r#"component X {
  <template>{#for x , xs}<li/>{/for}</template>
}"#;
        let err = parse(src).unwrap_err();
        assert!(
            err.message.contains("expected `in`"),
            "unexpected error message: {}",
            err.message
        );
    }

    #[test]
    fn template_for_block_empty_iter_errors_clearly() {
        // `{#for x in }` — iter expression is missing after `in`.
        let src = r#"component X {
  <template>{#for x in }<li/>{/for}</template>
}"#;
        let err = parse(src).unwrap_err();
        assert!(
            err.message.contains("expected iter expression") && err.message.contains("empty"),
            "unexpected error message: {}",
            err.message
        );
    }

    #[test]
    fn template_for_block_unterminated_errors_clearly() {
        // `{#for x in xs}...` — no `{/for}`. Reaches EOF before the
        // closer, must error with the directive's position.
        let src = r#"component X {
  <template>{#for x in xs}<li/></template>
}"#;
        let err = parse(src).unwrap_err();
        assert!(
            err.message.contains("unterminated `{#for}`")
                || err.message.contains("expected `{/for}`"),
            "unexpected error message: {}",
            err.message
        );
    }

    #[test]
    fn template_for_block_var_named_in_is_rejected() {
        // Using `in` as the binding identifier is a nasty
        // ambiguity: `{#for in in xs}` would read as `#for` with
        // binding `in` and then no `in` keyword. The `read_directive_name`
        // greedy-reads `in`, then `consume_keyword_in` looks at the
        // next `in` — this should actually work correctly, but the
        // test documents behavior. The binding `in` is legal
        // syntactically; it's ugly Fitz but not a parser error.
        // We just verify the parser doesn't crash.
        let src = r#"component X {
  <template>{#for in in xs}<li/>{/for}</template>
}"#;
        let file = parse(src).expect("parse should succeed (odd but legal)");
        match &file.components[0].template.as_ref().unwrap().roots[0] {
            TemplateNode::For { var, iter_raw, .. } => {
                assert_eq!(var, "in");
                assert_eq!(iter_raw, "xs");
            }
            other => panic!("expected For, got {other:?}"),
        }
    }

    // ---- 11.2.c mini-commit 3: `<slot />` + `{#else}` ----------------

    #[test]
    fn template_slot_self_closing_without_name_is_default_slot() {
        // `<slot />` — the default (unnamed) slot.
        let src = r#"component X {
  <template><slot /></template>
}"#;
        let file = parse(src).expect("parse should succeed");
        let roots = &file.components[0].template.as_ref().unwrap().roots;
        assert_eq!(roots.len(), 1);
        match &roots[0] {
            TemplateNode::Slot { name, .. } => assert!(name.is_none()),
            other => panic!("expected Slot, got {other:?}"),
        }
    }

    #[test]
    fn template_slot_self_closing_with_name_captures_the_slot_name() {
        // `<slot name="header" />` — a named slot.
        let src = r#"component X {
  <template><slot name="header" /></template>
}"#;
        let file = parse(src).expect("parse should succeed");
        match &file.components[0].template.as_ref().unwrap().roots[0] {
            TemplateNode::Slot { name, .. } => {
                assert_eq!(name.as_deref(), Some("header"));
            }
            other => panic!("expected Slot, got {other:?}"),
        }
    }

    #[test]
    fn template_slot_nested_inside_element_is_a_regular_child() {
        // `<div><slot /></div>` — the `<div>` sees the Slot as its
        // sole child, like any other node type.
        let src = r#"component X {
  <template><div><slot name="body" /></div></template>
}"#;
        let file = parse(src).expect("parse should succeed");
        match &file.components[0].template.as_ref().unwrap().roots[0] {
            TemplateNode::Element { tag, children, .. } => {
                assert_eq!(tag, "div");
                assert_eq!(children.len(), 1);
                match &children[0] {
                    TemplateNode::Slot { name, .. } => {
                        assert_eq!(name.as_deref(), Some("body"));
                    }
                    other => panic!("expected Slot child, got {other:?}"),
                }
            }
            other => panic!("expected <div>, got {other:?}"),
        }
    }

    #[test]
    fn phase_11_7_d_template_slot_open_close_captures_fallback() {
        // Phase 11.7.d — `<slot>...</slot>` now captures its children as
        // fallback content instead of rejecting.
        let src = r#"component X {
  <template><slot><em>fallback</em></slot></template>
}"#;
        let file = parse(src).expect("slot with fallback should parse");
        let tpl = file.components[0].template.as_ref().unwrap();
        match &tpl.roots[0] {
            TemplateNode::Slot { name, fallback, .. } => {
                assert!(name.is_none());
                assert_eq!(fallback.len(), 1, "fallback child captured");
            }
            other => panic!("expected Slot, got {other:?}"),
        }
    }

    #[test]
    fn template_slot_extra_static_attribute_is_rejected() {
        // Any attribute other than `name` on `<slot />` is rejected.
        let src = r#"component X {
  <template><slot foo="bar" /></template>
}"#;
        let err = parse(src).unwrap_err();
        assert!(
            err.message.contains("`name`") && err.message.contains("foo"),
            "unexpected error message: {}",
            err.message
        );
    }

    #[test]
    fn template_slot_event_attribute_is_rejected() {
        // `@click` (or any event binding) is meaningless on `<slot>`.
        let src = r#"component X {
  <template><slot @click="handle" /></template>
}"#;
        let err = parse(src).unwrap_err();
        assert!(
            err.message.contains("event bindings")
                && (err.message.contains("click") || err.message.contains("@click")),
            "unexpected error message: {}",
            err.message
        );
    }

    #[test]
    fn template_slot_interpolated_name_attribute_is_rejected() {
        // `name="{dynamic}"` — MVP requires a static string literal
        // for the slot name (11.5 may relax when demand appears).
        let src = r#"component X {
  <template><slot name="{header}" /></template>
}"#;
        let err = parse(src).unwrap_err();
        assert!(
            err.message.contains("interpolated attributes") || err.message.contains("plain string"),
            "unexpected error message: {}",
            err.message
        );
    }

    #[test]
    fn template_slot_duplicate_name_attribute_is_rejected() {
        // `<slot name="a" name="b" />` — the HTML sub-parser accepts
        // repeated attribute names generically; `<slot>` explicitly
        // rejects duplicates on the `name` attr.
        let src = r#"component X {
  <template><slot name="a" name="b" /></template>
}"#;
        let err = parse(src).unwrap_err();
        assert!(
            err.message.contains("duplicates") || err.message.contains("single `name`"),
            "unexpected error message: {}",
            err.message
        );
    }

    #[test]
    fn template_if_else_captures_then_and_else_children() {
        // `{#if a}<div>x</div>{#else}<span>y</span>{/if}` — the If
        // node carries both `children` (then branch) and
        // `else_children` (Some, one <span>).
        let src = r#"component X {
  <template>{#if a}<div>x</div>{#else}<span>y</span>{/if}</template>
}"#;
        let file = parse(src).expect("parse should succeed");
        match &file.components[0].template.as_ref().unwrap().roots[0] {
            TemplateNode::If {
                cond_raw,
                children,
                else_children,
                ..
            } => {
                assert_eq!(cond_raw, "a");
                assert_eq!(children.len(), 1);
                match &children[0] {
                    TemplateNode::Element { tag, .. } => assert_eq!(tag, "div"),
                    other => panic!("expected <div> in then, got {other:?}"),
                }
                let else_kids = else_children.as_ref().expect("else branch present");
                assert_eq!(else_kids.len(), 1);
                match &else_kids[0] {
                    TemplateNode::Element { tag, .. } => assert_eq!(tag, "span"),
                    other => panic!("expected <span> in else, got {other:?}"),
                }
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn template_if_without_else_preserves_none_else_children() {
        // Regression: the old shape `{#if x}<div/>{/if}` still parses
        // and produces `else_children = None`.
        let src = r#"component X {
  <template>{#if x}<div/>{/if}</template>
}"#;
        let file = parse(src).expect("parse should succeed");
        match &file.components[0].template.as_ref().unwrap().roots[0] {
            TemplateNode::If { else_children, .. } => {
                assert!(else_children.is_none());
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn template_if_else_with_empty_else_body_is_legal() {
        // `{#if x}<a/>{#else}{/if}` — an else branch with no
        // children. Legal; `else_children = Some(vec![])`.
        let src = r#"component X {
  <template>{#if x}<a/>{#else}{/if}</template>
}"#;
        let file = parse(src).expect("parse should succeed");
        match &file.components[0].template.as_ref().unwrap().roots[0] {
            TemplateNode::If { else_children, .. } => match else_children {
                Some(v) => assert!(v.is_empty()),
                None => panic!("expected Some(vec![]), got None"),
            },
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn template_if_else_nested_each_if_scopes_its_own_else() {
        // Two nested ifs each with their own else. Outer's else
        // branch must not be captured by the inner's else, and vice
        // versa.
        let src = r#"component X {
  <template>{#if a}{#if b}p{#else}q{/if}{#else}r{/if}</template>
}"#;
        let file = parse(src).expect("parse should succeed");
        match &file.components[0].template.as_ref().unwrap().roots[0] {
            TemplateNode::If {
                cond_raw: outer_cond,
                children: outer_children,
                else_children: outer_else,
                ..
            } => {
                assert_eq!(outer_cond, "a");
                // Outer then: contains the inner If.
                assert_eq!(outer_children.len(), 1);
                match &outer_children[0] {
                    TemplateNode::If {
                        cond_raw: inner_cond,
                        children: inner_then,
                        else_children: inner_else,
                        ..
                    } => {
                        assert_eq!(inner_cond, "b");
                        // Inner then: text "p"
                        match &inner_then[0] {
                            TemplateNode::Text(t) => assert!(t.contains("p")),
                            other => panic!("expected Text 'p', got {other:?}"),
                        }
                        // Inner else: text "q"
                        let inner_else = inner_else.as_ref().expect("inner else present");
                        match &inner_else[0] {
                            TemplateNode::Text(t) => assert!(t.contains("q")),
                            other => panic!("expected Text 'q', got {other:?}"),
                        }
                    }
                    other => panic!("expected inner If, got {other:?}"),
                }
                // Outer else: text "r".
                let outer_else = outer_else.as_ref().expect("outer else present");
                match &outer_else[0] {
                    TemplateNode::Text(t) => assert!(t.contains("r")),
                    other => panic!("expected Text 'r', got {other:?}"),
                }
            }
            other => panic!("expected outer If, got {other:?}"),
        }
    }

    #[test]
    fn template_if_else_double_else_is_rejected() {
        // Two `{#else}`s inside one `{#if}` — the second one lands
        // during the else-branch parse (accept_else = false), so
        // `parse_directive_open`'s "else" arm fires with a targeted
        // message.
        let src = r#"component X {
  <template>{#if x}<a/>{#else}<b/>{#else}<c/>{/if}</template>
}"#;
        let err = parse(src).unwrap_err();
        assert!(
            err.message.contains("unexpected `{#else}`")
                && (err.message.contains("one `{#else}` per")
                    || err.message.contains("inside an `{#if}`")),
            "unexpected error message: {}",
            err.message
        );
    }

    #[test]
    fn template_else_outside_if_is_rejected() {
        // Stray `{#else}` at template top-level — no enclosing
        // `{#if}`.
        let src = r#"component X {
  <template>{#else}<div/>{/if}</template>
}"#;
        let err = parse(src).unwrap_err();
        assert!(
            err.message.contains("unexpected `{#else}`")
                || err.message.contains("must appear inside"),
            "unexpected error message: {}",
            err.message
        );
    }

    #[test]
    fn template_unknown_directive_error_message_mentions_else() {
        // Regression: the mini-commit 2 error message pointed at
        // "mini-commit 3 for `#else` and `<slot>`". Mini-commit 3
        // closes both, so the message now names the supported
        // directives directly.
        let src = r#"component X {
  <template>{#while cond}<li/>{/while}</template>
}"#;
        let err = parse(src).unwrap_err();
        assert!(
            err.message.contains("unknown template directive") && err.message.contains("while"),
            "unexpected error message: {}",
            err.message
        );
        // The message should list the supported directives explicitly.
        assert!(
            err.message.contains("{#if}")
                && err.message.contains("{#for}")
                && err.message.contains("{#else}"),
            "unexpected error message: {}",
            err.message
        );
    }

    // -----------------------------------------------------------------
    // Arithmetic body round-trip — pre-req follow-up of 11.4.b (§9.n)
    // -----------------------------------------------------------------

    /// An event body with a `count = count + 1` statement round-trips
    /// through `capture_balanced_body_raw` to a raw string that
    /// `append_token_source` reconstructs with tidy spacing around
    /// each operator. Before the follow-up, the `+` char failed at
    /// lex time before the parser ever ran.
    #[test]
    fn event_body_with_add_round_trips_verbatim() {
        let src = r#"component X {
  state { count: Int = 0 }
  event increment() { count = count + 1 }
}"#;
        let file = parse(src).expect("counter with `+` should parse");
        assert_eq!(file.components.len(), 1);
        assert_eq!(file.components[0].events.len(), 1);
        let body = &file.components[0].events[0].body_raw;
        // Spaces around the `=` and the `+` come from
        // `needs_space_before` triggers (Ident before `=`, `+`, and
        // literal digits).
        assert_eq!(body.trim(), "count = count + 1", "body_raw:\n{}", body);
    }

    #[test]
    fn event_body_with_all_arithmetic_ops_round_trips() {
        let src = r#"component X {
  state { n: Int = 0 }
  event mix() { n = n + 1 - 2 * 3 / 4 % 5 }
}"#;
        let file = parse(src).expect("all-ops body should parse");
        let body = &file.components[0].events[0].body_raw;
        assert_eq!(
            body.trim(),
            "n = n + 1 - 2 * 3 / 4 % 5",
            "body_raw:\n{}",
            body
        );
    }

    /// The captured raw body must be re-lexable by the classic Fitz
    /// lexer + parser via `expand::parse_statements_from_source`
    /// (that's what `expand::expand_event_handler` calls). Rather
    /// than pulling in `expand` here, delegate to the classic
    /// re-lex entry point directly.
    #[test]
    fn arithmetic_body_re_lexes_through_classic_parser() {
        use crate::ast::{BinOpKind, Expr, Stmt};
        use crate::parser::parse_statements_from_source;

        let src = r#"component X {
  state { count: Int = 0 }
  event increment() { count = count + 1 }
}"#;
        let file = parse(src).expect("parse OK");
        let body = &file.components[0].events[0].body_raw;
        // The classic parser must accept the reconstructed body.
        let stmts = parse_statements_from_source(body).expect("classic parser accepts round-trip");
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Stmt::Assign {
                target: _, value, ..
            } => match value {
                Expr::BinOp {
                    op, left, right, ..
                } => {
                    assert_eq!(*op, BinOpKind::Add);
                    assert!(matches!(left.as_ref(), Expr::Ident(name, _) if name == "count"));
                    assert!(matches!(right.as_ref(), Expr::Int(1, _)));
                }
                other => panic!("expected BinOp, got {:?}", other),
            },
            other => panic!("expected Assign, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // §9.ee V-1 — HTML comment `<!-- ... -->` support in template block
    // -----------------------------------------------------------------------

    #[test]
    fn v1_html_comment_in_template_accepted_and_discarded() {
        // Canonical case from the chat migration probe: an HTML
        // comment between two elements. Pre-fix: "expected tag name
        // after `<`". Post-fix: parses clean; comment consumed as
        // an empty Text node (no user-visible output).
        let src = r#"component X {
  state { count: Int = 0 }
  <template>
    <div>
      <!-- This is a comment -->
      <p>{count}</p>
    </div>
  </template>
}"#;
        let file = parse(src).expect("V-1: HTML comment should parse clean");
        // Should have one component with a template.
        assert_eq!(file.components.len(), 1);
        let tmpl = file.components[0]
            .template
            .as_ref()
            .expect("template present");
        // Roots may include whitespace text — assert at least ONE
        // Element root (the `<div>`).
        let has_div_element = tmpl
            .roots
            .iter()
            .any(|n| matches!(n, TemplateNode::Element { tag, .. } if tag == "div"));
        assert!(
            has_div_element,
            "expected `<div>` root: got {:?}",
            tmpl.roots
        );
    }

    #[test]
    fn v1_html_comment_at_start_of_template_accepted() {
        let src = r#"component X {
  state { count: Int = 0 }
  <template>
    <!-- top-of-template comment -->
    <div>{count}</div>
  </template>
}"#;
        let file = parse(src).expect("comment at start of template should parse clean");
        assert_eq!(file.components.len(), 1);
    }

    #[test]
    fn v1_html_comment_multi_line_body_accepted() {
        let src = r#"component X {
  state { count: Int = 0 }
  <template>
    <!--
      Multi-line
      HTML comment
      with several lines
    -->
    <div>{count}</div>
  </template>
}"#;
        let file = parse(src).expect("multi-line comment should parse clean");
        assert_eq!(file.components.len(), 1);
    }

    #[test]
    fn v1_html_comment_with_dashes_inside_accepted() {
        // Single `-` chars inside a comment body should not
        // prematurely close the comment. Only `-->` closes.
        let src = r#"component X {
  state { count: Int = 0 }
  <template>
    <!-- foo - bar - baz -->
    <div>{count}</div>
  </template>
}"#;
        let file = parse(src).expect("single dashes inside comment should not close");
        assert_eq!(file.components.len(), 1);
    }

    #[test]
    fn v1_html_comment_unterminated_errors_clearly() {
        let src = r#"component X {
  state { count: Int = 0 }
  <template>
    <!-- never closes
    <div>{count}</div>
  </template>
}"#;
        let err = parse(src).expect_err("unterminated comment should error");
        assert!(
            err.message.contains("unterminated HTML comment"),
            "error must cite unterminated: {}",
            err.message
        );
    }

    #[test]
    fn v1_doctype_still_errors_with_targeted_hint() {
        // `<!DOCTYPE ...>` NOT accepted — belongs to framework layout,
        // not templates. Error must be actionable.
        let src = r#"component X {
  state { count: Int = 0 }
  <template>
    <!DOCTYPE html>
    <div>{count}</div>
  </template>
}"#;
        let err = parse(src).expect_err("DOCTYPE should error");
        assert!(
            err.message.contains("<!--"),
            "error should reference the accepted form: {}",
            err.message
        );
        assert!(
            err.message.contains("DOCTYPE"),
            "error should mention DOCTYPE as unsupported: {}",
            err.message
        );
    }

    // -----------------------------------------------------------------------
    // §9.ee V-2 — Bare boolean HTML attribute support
    // -----------------------------------------------------------------------

    #[test]
    fn v2_bare_required_attribute_accepted_as_empty_static() {
        // Canonical case from chat migration: `<input required>`
        // which HTML5 spec permits as sugar for
        // `<input required="">`. Pre-fix: error "requires a value".
        // Post-fix: parses as Attr::Static with empty value.
        let src = r#"component X {
  state { count: Int = 0 }
  <template>
    <input name="user" required autocomplete="off" />
  </template>
}"#;
        let file = parse(src).expect("V-2: bare `required` should parse clean");
        let tmpl = file.components[0]
            .template
            .as_ref()
            .expect("template present");
        // Find the input element and check its attrs.
        let input = tmpl
            .roots
            .iter()
            .find_map(|n| match n {
                TemplateNode::Element { tag, attrs, .. } if tag == "input" => Some(attrs),
                _ => None,
            })
            .expect("input element present");
        let has_bare_required = input.iter().any(|a| matches!(a, Attr::Static { name, value, .. } if name == "required" && value.is_empty()));
        assert!(
            has_bare_required,
            "expected `required` as Attr::Static with empty value; got {:?}",
            input
        );
    }

    #[test]
    fn v2_multiple_bare_boolean_attrs_all_accepted() {
        // Real-world case: `<input required disabled readonly>` —
        // three bare booleans in a row, all should parse clean.
        let src = r#"component X {
  state { count: Int = 0 }
  <template>
    <input required disabled readonly />
  </template>
}"#;
        let file = parse(src).expect("multiple bare booleans should parse clean");
        let tmpl = file.components[0]
            .template
            .as_ref()
            .expect("template present");
        let input = tmpl
            .roots
            .iter()
            .find_map(|n| match n {
                TemplateNode::Element { tag, attrs, .. } if tag == "input" => Some(attrs),
                _ => None,
            })
            .expect("input element present");
        let bare_count = input
            .iter()
            .filter(|a| matches!(a, Attr::Static { value, .. } if value.is_empty()))
            .count();
        assert_eq!(
            bare_count, 3,
            "expected 3 bare boolean attrs; got {:?}",
            input
        );
    }

    // Form B (gotcha #6, v0.38.0) — conditional boolean attribute
    // `checked={expr}` (unquoted brace) parses as Attr::BoolInterpolation.

    fn first_input_attrs(src: &str) -> Vec<Attr> {
        let file = parse(src).expect("parse ok");
        let tmpl = file.components[0]
            .template
            .as_ref()
            .expect("template present");
        fn find(nodes: &[TemplateNode]) -> Option<&Vec<Attr>> {
            for n in nodes {
                if let TemplateNode::Element {
                    tag,
                    attrs,
                    children,
                    ..
                } = n
                {
                    if tag == "input" {
                        return Some(attrs);
                    }
                    if let Some(a) = find(children) {
                        return Some(a);
                    }
                }
            }
            None
        }
        find(&tmpl.roots).expect("input element present").clone()
    }

    #[test]
    fn bool_attr_parses_as_bool_interpolation_v0_38_0() {
        let attrs = first_input_attrs(
            "component X { state { done: Bool = false }\n  <template><input checked={done} /></template> }",
        );
        assert!(
            attrs.iter().any(
                |a| matches!(a, Attr::BoolInterpolation { name, expr_raw, .. }
                if name == "checked" && expr_raw == "done")
            ),
            "esperaba BoolInterpolation checked=done, got: {attrs:?}"
        );
    }

    #[test]
    fn bool_attr_with_complex_expr_v0_38_0() {
        let attrs = first_input_attrs(
            "component X { state { n: Int = 0 }\n  <template><input disabled={n > 0} /></template> }",
        );
        assert!(
            attrs.iter().any(
                |a| matches!(a, Attr::BoolInterpolation { name, expr_raw, .. }
                if name == "disabled" && expr_raw == "n > 0")
            ),
            "esperaba BoolInterpolation disabled con expr `n > 0`, got: {attrs:?}"
        );
    }

    #[test]
    fn bool_attr_mixed_with_static_and_quoted_v0_38_0() {
        // A bool attr sitting between a static and a quoted-interp attr
        // parses cleanly, and the quoted `checked="{x}"` stays a normal
        // (always-present, stringified) Interpolation — disjoint syntax.
        let attrs = first_input_attrs(
            "component X { state { done: Bool = false, cls: Str = \"\" }\n  <template><input type=\"checkbox\" checked={done} class=\"{cls}\" /></template> }",
        );
        assert!(
            attrs.iter().any(|a| matches!(a, Attr::Static { name, value, .. } if name == "type" && value == "checkbox")),
            "esperaba type static, got: {attrs:?}"
        );
        assert!(
            attrs
                .iter()
                .any(|a| matches!(a, Attr::BoolInterpolation { name, .. } if name == "checked")),
            "esperaba checked BoolInterpolation, got: {attrs:?}"
        );
        assert!(
            attrs
                .iter()
                .any(|a| matches!(a, Attr::Interpolation { name, .. } if name == "class")),
            "esperaba class Interpolation (quoted queda stringify), got: {attrs:?}"
        );
    }

    #[test]
    fn bool_attr_unmatched_brace_is_error_v0_38_0() {
        let src = "component X { state { done: Bool = false }\n  <template><input checked={done /></template> }";
        let err = parse(src).expect_err("unmatched brace en bool attr debe fallar");
        assert!(
            err.message.contains("unmatched `{`") && err.message.contains("checked"),
            "esperaba error de unmatched brace nombrando checked, got: {}",
            err.message
        );
    }

    #[test]
    fn v2_bare_data_flv_clear_accepted() {
        // fitz-liveviews convention: `data-flv-clear` bare means
        // "clear this input on next re-render". Pre-fix: rejected.
        let src = r#"component X {
  state { count: Int = 0 }
  <template>
    <input name="msg" data-flv-clear />
  </template>
}"#;
        let file = parse(src).expect("data-flv-clear bare should parse clean");
        let tmpl = file.components[0]
            .template
            .as_ref()
            .expect("template present");
        let input = tmpl
            .roots
            .iter()
            .find_map(|n| match n {
                TemplateNode::Element { tag, attrs, .. } if tag == "input" => Some(attrs),
                _ => None,
            })
            .expect("input element present");
        let has_flv_clear = input.iter().any(|a| matches!(a, Attr::Static { name, value, .. } if name == "data-flv-clear" && value.is_empty()));
        assert!(
            has_flv_clear,
            "expected `data-flv-clear` bare; got {:?}",
            input
        );
    }

    // -----------------------------------------------------------------------
    // §9.dd V-3 + V-5 — `from X import Y1, Y2, ...` at top of `.fitzv`
    // -----------------------------------------------------------------------
    //
    // Enables cross-file nominal type refs in state annotations and
    // struct literals inside event bodies. Emitted verbatim as classic
    // Fitz `from ... import ...` at the top of the transformed source.

    #[test]
    fn v3_single_from_import_parses() {
        let src = r#"from message import Message

component X {
  state { count: Int = 0 }
}"#;
        let file = parse(src).expect("V-3: from-import should parse clean");
        assert_eq!(file.imports.len(), 1);
        assert_eq!(file.imports[0].path, vec!["message"]);
        assert_eq!(file.imports[0].names, vec![("Message".to_string(), None)]);
        assert_eq!(file.components.len(), 1);
    }

    #[test]
    fn v3_multi_name_from_import_parses() {
        let src = r#"from utils import User, Post, Comment

component X {
  state { count: Int = 0 }
}"#;
        let file = parse(src).expect("multi-name from-import should parse");
        assert_eq!(file.imports.len(), 1);
        assert_eq!(
            file.imports[0].names,
            vec![
                ("User".to_string(), None),
                ("Post".to_string(), None),
                ("Comment".to_string(), None),
            ]
        );
    }

    #[test]
    fn v3_dotted_path_from_import_parses() {
        let src = r#"from foo.bar.baz import Widget

component X {
  state { count: Int = 0 }
}"#;
        let file = parse(src).expect("dotted-path from-import should parse");
        assert_eq!(file.imports.len(), 1);
        assert_eq!(file.imports[0].path, vec!["foo", "bar", "baz"]);
        assert_eq!(file.imports[0].names, vec![("Widget".to_string(), None)]);
    }

    #[test]
    fn v3_multiple_from_imports_parse() {
        let src = r#"from message import Message
from user import User

component X {
  state { count: Int = 0 }
}"#;
        let file = parse(src).expect("multiple from-imports should parse");
        assert_eq!(file.imports.len(), 2);
        assert_eq!(file.imports[0].names, vec![("Message".to_string(), None)]);
        assert_eq!(file.imports[1].names, vec![("User".to_string(), None)]);
    }

    // S.1 (2026-07-17) — `from X import Y as Z` alias support.

    #[test]
    fn s1_from_import_with_single_alias_parses() {
        // `Message` is the original name (validated against the
        // source module's exports); `Msg` is the local binding
        // that the SFC's template + event bodies reference.
        let src = r#"from message import Message as Msg

component X {
  state { count: Int = 0 }
}"#;
        let file = parse(src).expect("from-import with alias should parse");
        assert_eq!(file.imports.len(), 1);
        assert_eq!(
            file.imports[0].names,
            vec![("Message".to_string(), Some("Msg".to_string()))]
        );
    }

    #[test]
    fn s1_from_import_mixed_aliased_and_bare_names_parse() {
        // Mixed — some aliased, some not — must all round-trip
        // through the tuple shape.
        let src = r#"from utils import User as U, Post, Comment as C

component X {
  state { count: Int = 0 }
}"#;
        let file = parse(src).expect("mixed aliased + bare should parse");
        assert_eq!(file.imports.len(), 1);
        assert_eq!(
            file.imports[0].names,
            vec![
                ("User".to_string(), Some("U".to_string())),
                ("Post".to_string(), None),
                ("Comment".to_string(), Some("C".to_string())),
            ]
        );
    }

    #[test]
    fn s1_from_import_as_without_alias_ident_errors() {
        let src = r#"from message import Message as

component X {
  state { count: Int = 0 }
}"#;
        let err = parse(src).expect_err("bare `as` should be a parse error");
        assert!(
            err.message.contains("expected identifier after `as`"),
            "err: {}",
            err.message
        );
    }

    #[test]
    fn v3_from_import_after_component_errors_clearly() {
        // Classic Fitz convention: imports at the top of the file,
        // before any type/fn declarations. Same for `.fitzv`.
        let src = r#"component X {
  state { count: Int = 0 }
}

from message import Message"#;
        let err = parse(src).expect_err("from-after-component should error");
        assert!(
            err.message.contains("must appear before any `component`"),
            "error must cite ordering rule: {}",
            err.message
        );
    }

    #[test]
    fn v3_from_import_missing_import_keyword_errors() {
        let src = r#"from message Message

component X {
  state { count: Int = 0 }
}"#;
        let err = parse(src).expect_err("missing `import` should error");
        assert!(
            err.message.contains("expected `import`"),
            "error must cite missing import keyword: {}",
            err.message
        );
    }

    #[test]
    fn v3_from_import_empty_name_list_errors() {
        // `from message import` with nothing after — malformed.
        // In practice this will trip the "expected identifier" error
        // when trying to read the first name.
        let src = r#"from message import

component X {
  state { count: Int = 0 }
}"#;
        let err = parse(src).expect_err("empty name list should error");
        assert!(
            err.message.contains("identifier") || err.message.contains("import"),
            "error must cite malformed import: {}",
            err.message
        );
    }

    #[test]
    fn v3_no_imports_still_produces_empty_vec() {
        // Regression: files without any imports must still parse
        // (backward compat with counter/dashboard/MetricTile).
        let src = r#"component X {
  state { count: Int = 0 }
}"#;
        let file = parse(src).expect("no-imports file should parse");
        assert!(file.imports.is_empty());
        assert_eq!(file.components.len(), 1);
    }

    #[test]
    fn v2_explicit_value_still_supported_regression() {
        // Regression: attrs with explicit values MUST still work
        // (`name="user"`, `required="required"`, etc.).
        let src = r#"component X {
  state { count: Int = 0 }
  <template>
    <input name="user" required="required" data-flv-clear="true" />
  </template>
}"#;
        let file = parse(src).expect("explicit values regression should parse clean");
        let tmpl = file.components[0]
            .template
            .as_ref()
            .expect("template present");
        let input = tmpl
            .roots
            .iter()
            .find_map(|n| match n {
                TemplateNode::Element { tag, attrs, .. } if tag == "input" => Some(attrs),
                _ => None,
            })
            .expect("input element present");
        assert!(input.iter().any(
            |a| matches!(a, Attr::Static { name, value, .. } if name == "name" && value == "user")
        ));
        assert!(input.iter().any(|a| matches!(a, Attr::Static { name, value, .. } if name == "required" && value == "required")));
        assert!(input.iter().any(|a| matches!(a, Attr::Static { name, value, .. } if name == "data-flv-clear" && value == "true")));
    }

    // ---- keyed `{#for x in xs key=x.id}` sugar --------------------

    #[test]
    fn split_for_iter_key_no_clause_returns_iter_only() {
        assert_eq!(split_for_iter_key("xs"), ("xs".into(), None));
        assert_eq!(
            split_for_iter_key("  items.filter(fn(x) => x.active)  "),
            ("items.filter(fn(x) => x.active)".into(), None)
        );
    }

    #[test]
    fn split_for_iter_key_basic_clause() {
        assert_eq!(
            split_for_iter_key("xs key=x.id"),
            ("xs".into(), Some("x.id".into()))
        );
        // No space before `=`, spaces around `key`.
        assert_eq!(
            split_for_iter_key("items  key = row.uuid"),
            ("items".into(), Some("row.uuid".into()))
        );
    }

    #[test]
    fn split_for_iter_key_ignores_key_inside_brackets_parens_strings() {
        // `key` inside `[]` / `()` / string literal is part of the iter.
        assert_eq!(
            split_for_iter_key("items[key]"),
            ("items[key]".into(), None)
        );
        assert_eq!(
            split_for_iter_key("lookup(key)"),
            ("lookup(key)".into(), None)
        );
        assert_eq!(
            split_for_iter_key(r#"where("key=1")"#),
            (r#"where("key=1")"#.into(), None)
        );
    }

    #[test]
    fn split_for_iter_key_word_boundary_not_a_prefix() {
        // `key_list` / `keyboard` are NOT the `key` marker.
        assert_eq!(split_for_iter_key("key_list"), ("key_list".into(), None));
        assert_eq!(split_for_iter_key("keyboards"), ("keyboards".into(), None));
        // Iterating a variable literally named `key` (no `=`).
        assert_eq!(split_for_iter_key("key"), ("key".into(), None));
    }

    #[test]
    fn split_for_iter_key_preserves_comparison_in_key_expr() {
        // A `==` inside the key expression survives; only the first
        // single `=` after `key` is the marker.
        assert_eq!(
            split_for_iter_key(r#"xs key=x.name == "a""#),
            ("xs".into(), Some(r#"x.name == "a""#.into()))
        );
    }

    #[test]
    fn parse_for_with_key_populates_key_raw() {
        let src = r#"component X {
  state { rows: List<Int> = [] }
  <template>{#for r in rows key=r}<li>{r}</li>{/for}</template>
}"#;
        let file = parse(src).expect("parse should succeed");
        match &file.components[0].template.as_ref().unwrap().roots[0] {
            TemplateNode::For {
                var,
                iter_raw,
                key_raw,
                ..
            } => {
                assert_eq!(var, "r");
                assert_eq!(iter_raw, "rows");
                assert_eq!(key_raw.as_deref(), Some("r"));
            }
            other => panic!("expected For root, got {other:?}"),
        }
    }

    #[test]
    fn parse_for_without_key_leaves_key_raw_none() {
        let src = r#"component X {
  state { rows: List<Int> = [] }
  <template>{#for r in rows}<li>{r}</li>{/for}</template>
}"#;
        let file = parse(src).expect("parse should succeed");
        match &file.components[0].template.as_ref().unwrap().roots[0] {
            TemplateNode::For { key_raw, .. } => {
                assert_eq!(*key_raw, None);
            }
            other => panic!("expected For root, got {other:?}"),
        }
    }

    #[test]
    fn parse_for_empty_key_clause_is_error() {
        let src = r#"component X {
  state { rows: List<Int> = [] }
  <template>{#for r in rows key=}<li>{r}</li>{/for}</template>
}"#;
        let err = parse(src).expect_err("empty key clause should error");
        assert!(
            err.to_string().contains("key="),
            "error should mention the empty key clause: {err}"
        );
    }
}
