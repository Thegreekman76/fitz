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
        let mut components = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek().token {
                Token::Eof => break,
                Token::Component => {
                    components.push(self.parse_component()?);
                }
                _ => {
                    let cur = self.peek().clone();
                    return Err(ViewParseError {
                        message: format!(
                            "expected `component` at the top level, got {}",
                            cur.token
                        ),
                        line: cur.line,
                        column: cur.column,
                    });
                }
            }
        }
        Ok(ViewFile { components })
    }

    fn parse_component(&mut self) -> ViewParseResult<Component> {
        let kw = self.expect(Token::Component)?;
        let loc = Loc::new(kw.line, kw.column);
        let name_tok = self.expect(Token::Ident(String::new()))?;
        let name = match name_tok.token {
            Token::Ident(s) => s,
            _ => unreachable!(),
        };
        self.expect(Token::LBrace)?;

        let mut state = Vec::new();
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
                Token::StyleScopedRaw(_) => {
                    if style.is_some() {
                        let cur = self.peek().clone();
                        return Err(ViewParseError {
                            message: "duplicate `<style scoped>` block — only one per component"
                                .into(),
                            line: cur.line,
                            column: cur.column,
                        });
                    }
                    let tok = self.advance();
                    let css = match tok.token {
                        Token::StyleScopedRaw(s) => s,
                        _ => unreachable!(),
                    };
                    style = Some(Style {
                        css_raw: css,
                        loc: Loc::new(tok.line, tok.column),
                    });
                }
                _ => {
                    let cur = self.peek().clone();
                    return Err(ViewParseError {
                        message: format!(
                            "expected `state`, `event`, `<template>` or `<style scoped>` inside component body, got {}",
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
            state,
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
        Token::Question => out.push('?'),
        Token::Newline => out.push('\n'),
        Token::TemplateRaw(_) | Token::StyleScopedRaw(_) | Token::Eof => {
            // Should not appear inside a `capture_*` call. If they
            // do (unreachable in practice), silently ignore.
        }
    }
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
        | Token::Question => false,
        _ => matches!(
            last,
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | ')' | '}' | ']' | '"' | '>' | '?'
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
    let roots = p.parse_nodes(None, None)?;
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
    ///   - the blob ends (when both are `None`).
    ///
    /// The two parents are independent: a template can nest an
    /// `{#if}` inside an element, or an element inside an `{#if}`,
    /// and the parser walks them uniformly. Recursion follows the
    /// same shape.
    fn parse_nodes(
        &mut self,
        parent: Option<&str>,
        directive_parent: Option<&str>,
    ) -> ViewParseResult<Vec<TemplateNode>> {
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
                return Ok(nodes);
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
                    return Ok(nodes);
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
                    Some(expected) if expected == name => return Ok(nodes),
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
                    // Directive opener: `{#if ...}`, `{#for ...}`
                    // (future). Currently only `#if` is supported.
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

    /// Parse a directive opener like `{#if cond}` (currently the
    /// only supported one). Recurses through `parse_nodes` with
    /// `directive_parent = Some("if")` to collect children up to
    /// the matching `{/if}`. `{#for}` and `<slot>` are deferred to
    /// 11.2.c mini-commits 2 and 3.
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
        if name != "if" {
            return Err(ViewParseError {
                message: format!(
                    "unknown template directive `{{#{name}}}` — the POC supports `{{#if}}` only; `{{#for}}` lands in 11.2.c mini-commit 2"
                ),
                line: start_line,
                column: start_col,
            });
        }
        // Capture the cond expression as raw text up to the
        // matching `}` at directive-open depth 0. Nested `{}` (e.g.
        // struct or map literals inside the cond) are tracked so we
        // don't stop at the wrong brace.
        self.skip_ws_inline();
        let cond_raw = self.capture_directive_arg_raw(start_line, start_col)?;
        // Recurse for the body up to `{/if}`.
        let children = self.parse_nodes(None, Some("if"))?;
        Ok(TemplateNode::If {
            cond_raw: cond_raw.trim().to_string(),
            children,
            loc: Loc::new(start_line, start_col),
        })
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

    fn parse_element(&mut self) -> ViewParseResult<TemplateNode> {
        let start_line = self.line;
        let start_col = self.column;
        self.advance(); // `<`
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
                    let children = self.parse_nodes(Some(&tag), None)?;
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
        if self.peek() != Some('=') {
            return Err(ViewParseError {
                message: format!(
                    "attribute `{name}` requires a value in the POC — bare boolean attrs land in Phase 11.2+"
                ),
                line: self.line,
                column: self.column,
            });
        }
        self.advance(); // `=`
        self.skip_ws_inside_tag();
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
        let mut s = String::new();
        loop {
            match self.peek() {
                Some('"') => {
                    self.advance();
                    return Ok(s);
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
                    return Err(ViewParseError {
                        message: "unterminated attribute value — expected closing `\"`".into(),
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
    fn duplicate_style_block_errors_clearly() {
        let src = r#"component X {
  <style scoped>a{}</style>
  <style scoped>b{}</style>
}"#;
        let err = parse(src).unwrap_err();
        assert!(err.message.contains("duplicate `<style scoped>`"));
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
        // `{#for ...}` — not supported in mini-commit 1. The message
        // should point at 11.2.c mini-commit 2 so the user knows this
        // is a planned feature, not a bug.
        let src = r#"component X {
  <template>{#for x in xs}<li/>{/for}</template>
}"#;
        let err = parse(src).unwrap_err();
        assert!(
            err.message.contains("unknown template directive") && err.message.contains("for"),
            "unexpected error message: {}",
            err.message
        );
    }
}
