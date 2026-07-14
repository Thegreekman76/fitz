// view/css_parser.rs — CSS mini-parser for `.fitzv` scoped styles.
//
// **Purpose**: the `.fitzv` compiler needs to rewrite a component's
// `<style scoped>` block so its rules only apply to that component's
// markup. The strategy chosen for 11.3 is **class-suffix scoping**:
// every class selector `.<ident>` in the CSS becomes
// `.<ident>-<scope>`, and every element in the template picks up
// suffixed variants of its classes on top of the originals (that
// template rewrite lives in 11.3.c). External JS / CSS querying
// `.<ident>` keeps working; the scoped CSS only matches the
// component's own elements because only they carry the suffixed
// classes.
//
// **Trade-offs** (documented instead of solved):
//   - Type selectors (`div { ... }`) are NOT scoped — they still
//     match every `<div>` on the page. Users who want per-component
//     styling opt in by targeting classes. This is a Svelte-style
//     posture — scoping is by class or nothing.
//   - ID selectors (`#foo`) and attribute selectors (`[data-x]`)
//     are also NOT scoped, same rationale.
//   - Pseudo-classes / pseudo-elements pass through unchanged; the
//     compound they modify carries the scope on its preceding class.
//   - `:not(.foo)` DOES scope the inner class, because the naive
//     walk doesn't discriminate between selector-arg pseudos
//     (`:not(...)`, `:is(...)`) and non-selector-arg pseudos
//     (`:nth-child(2n+1)` — no `.` inside, no false positive).
//     Contrived cases like `:nth-child(.foo)` (invalid CSS) would
//     "scope" the inner but they were never valid anyway.
//   - `@media` / `@supports` / `@container` bodies recurse — inner
//     rules get their selectors scoped. Other at-rules
//     (`@keyframes`, `@font-face`, `@page`, `@charset`, `@import`,
//     `@namespace`) are treated as opaque: their body is copied
//     verbatim without walking into it, because their body contains
//     keyframe steps, declarations, or URLs — not selectors.
//
// **Isolation posture** (Invariant 4 of `docs/stack.md`): this
// module is standalone. It does not depend on the classic lexer /
// parser / AST. It takes an `&str` in and returns an owned `String`
// out; the caller (11.3.c: `expand`) decides where to plug the
// output into the ExpandedComponent.
//
// **What this module deliberately does NOT do**:
//   - Full CSS grammar validation — a malformed selector or a
//     stray comma passes through as-is. Best-effort scoping over
//     strict parsing.
//   - `:global(...)` escape hatch — deferred. When it lands, the
//     walker learns "if we're inside `:global(...)`, skip the
//     `.<ident>` transformation, and drop the `:global(...)`
//     wrapper from the output".
//   - CSS Modules interop, CSS-in-JS, or any preprocessor syntax.
//   - Position tracking of transforms — errors carry a `pos`
//     (char offset) but transformations don't emit any events.
//     11.3.c will wire the whole output through as one blob.

use std::fmt;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Rewrite a CSS blob so every class selector `.<ident>` gets the
/// `-<scope>` suffix. Everything else — declarations, comments,
/// strings, at-rules with opaque bodies — passes through verbatim.
///
/// Returns the transformed CSS. Fails with a `CssParseError` if the
/// input has an unterminated block, unbalanced braces / brackets,
/// or an unterminated string / comment.
///
/// `scope` should be a valid CSS ident continuation string (ASCII
/// alphanumerics + `_` + `-`). The parser trusts the caller here.
/// 11.3.c is where we synthesise the scope from the component name
/// + FNV-1a hash of the CSS body, so the shape is under our control.
///
/// # Example
///
/// ```
/// use fitz::view::css_parser::apply_scope;
/// // The scope in the caller is typically `<component>-c-<hash>`
/// // synthesised by 11.3.c. Here we use a short scope for clarity.
/// let out = apply_scope(".card { color: red; }", "c-a1b2c3d4").unwrap();
/// assert_eq!(out, ".card-c-a1b2c3d4 { color: red; }");
/// ```
pub fn apply_scope(css_raw: &str, scope: &str) -> Result<String, CssParseError> {
    let mut parser = CssParser::new(css_raw, scope);
    parser.parse_stylesheet()?;
    Ok(parser.output)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A parse error carries a message plus a byte-ish char offset into
/// the input. The offset is 0-based over the `chars()` iteration,
/// which lines up with `Loc::column - 1` for single-line inputs.
/// For multi-line inputs, the caller (11.3.c) will map the offset
/// back to a `Loc` inside the `.fitzv` file — the parser stays
/// dumb about lines to keep the surface small.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssParseError {
    pub message: String,
    pub pos: usize,
}

impl fmt::Display for CssParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "css parse error at char {}: {}", self.pos, self.message)
    }
}

impl std::error::Error for CssParseError {}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct CssParser<'a> {
    input: Vec<char>,
    pos: usize,
    scope: &'a str,
    output: String,
}

impl<'a> CssParser<'a> {
    fn new(css_raw: &'a str, scope: &'a str) -> Self {
        Self {
            input: css_raw.chars().collect(),
            pos: 0,
            scope,
            output: String::with_capacity(css_raw.len() + 32),
        }
    }

    fn peek(&self) -> Option<char> {
        self.input.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.input.get(self.pos + offset).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.input.len()
    }

    /// Copy whitespace + comments verbatim into `self.output`. Stops
    /// at the first non-ws, non-comment char (or EOF). Comments are
    /// scanned char by char so that `/*` inside strings would NOT
    /// start a comment — but at the top level of a stylesheet,
    /// strings can't appear outside of a declaration body, so the
    /// distinction only matters when a comment happens to contain
    /// unbalanced quotes. Handled by scanning until the matching
    /// `*/` regardless of intervening quotes.
    fn copy_ws_and_comments(&mut self) -> Result<(), CssParseError> {
        loop {
            match self.peek() {
                Some(c) if c.is_ascii_whitespace() => {
                    self.output.push(c);
                    self.advance();
                }
                Some('/') if self.peek_at(1) == Some('*') => {
                    self.copy_block_comment()?;
                }
                _ => return Ok(()),
            }
        }
    }

    fn copy_block_comment(&mut self) -> Result<(), CssParseError> {
        let start = self.pos;
        self.output.push('/');
        self.output.push('*');
        self.advance(); // /
        self.advance(); // *
        loop {
            match self.peek() {
                None => {
                    return Err(CssParseError {
                        message: "unterminated `/* ... */` block comment".into(),
                        pos: start,
                    });
                }
                Some('*') if self.peek_at(1) == Some('/') => {
                    self.output.push('*');
                    self.output.push('/');
                    self.advance();
                    self.advance();
                    return Ok(());
                }
                Some(c) => {
                    self.output.push(c);
                    self.advance();
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // Stylesheet + rule dispatch
    // -----------------------------------------------------------------

    /// Parse the top-level stylesheet: a sequence of rules until
    /// EOF. Whitespace and comments between rules are preserved.
    fn parse_stylesheet(&mut self) -> Result<(), CssParseError> {
        loop {
            self.copy_ws_and_comments()?;
            if self.is_eof() {
                return Ok(());
            }
            self.parse_rule()?;
        }
    }

    /// Parse the body of an at-rule that contains nested rules
    /// (`@media`, `@supports`, `@container`). Runs the same loop as
    /// `parse_stylesheet` but stops at the matching `}` and copies
    /// it to the output.
    fn parse_nested_stylesheet(&mut self, open_pos: usize) -> Result<(), CssParseError> {
        loop {
            self.copy_ws_and_comments()?;
            match self.peek() {
                None => {
                    return Err(CssParseError {
                        message: "unterminated at-rule body — expected `}`".into(),
                        pos: open_pos,
                    });
                }
                Some('}') => {
                    self.output.push('}');
                    self.advance();
                    return Ok(());
                }
                _ => self.parse_rule()?,
            }
        }
    }

    fn parse_rule(&mut self) -> Result<(), CssParseError> {
        match self.peek() {
            Some('@') => self.parse_at_rule(),
            _ => self.parse_qualified_rule(),
        }
    }

    // -----------------------------------------------------------------
    // Qualified rule: `<selector-list> { <declarations> }`
    // -----------------------------------------------------------------

    fn parse_qualified_rule(&mut self) -> Result<(), CssParseError> {
        let start = self.pos;
        // Read the selector prelude up to the first `{` at depth 0.
        // Selectors can contain `(...)` and `[...]` — track them so
        // a stray `{` inside a bracket doesn't fool us. Selectors
        // cannot legally contain a `{` at the top level, so seeing
        // one at depth 0 always means the rule body starts.
        let selectors = self.read_selector_prelude(start)?;
        let transformed = transform_selector_list(&selectors, self.scope);
        self.output.push_str(&transformed);
        // Now consume the `{`, copy the declaration body verbatim
        // up to the matching `}`, and copy the `}`.
        let brace_pos = self.pos;
        self.expect_char('{', brace_pos, "expected `{` after selector list")?;
        self.output.push('{');
        self.copy_declaration_body(brace_pos)?;
        Ok(())
    }

    /// Read the selector prelude into a buffer (NOT into
    /// `self.output`; the caller transforms it first). Stops at the
    /// first `{` at bracket depth 0. Errors on EOF (rule needs a
    /// body).
    fn read_selector_prelude(&mut self, start: usize) -> Result<String, CssParseError> {
        let mut out = String::new();
        loop {
            match self.peek() {
                None => {
                    return Err(CssParseError {
                        message: "unexpected end of input while reading selector list — \
                                  expected `{` to open the rule body"
                            .into(),
                        pos: start,
                    });
                }
                Some('{') => return Ok(out),
                Some('}') => {
                    return Err(CssParseError {
                        message: "unexpected `}` while reading selector list — mismatched brace"
                            .into(),
                        pos: self.pos,
                    });
                }
                Some('/') if self.peek_at(1) == Some('*') => {
                    // Consume comment into the prelude — will be
                    // preserved by the selector transformer.
                    self.read_block_comment_into(&mut out)?;
                }
                Some(c @ '(') | Some(c @ '[') => {
                    let close = if c == '(' { ')' } else { ']' };
                    out.push(c);
                    self.advance();
                    self.read_balanced_into(&mut out, c, close)?;
                }
                Some(c @ ('"' | '\'')) => {
                    out.push(c);
                    self.advance();
                    self.read_string_into(&mut out, c)?;
                }
                Some(c) => {
                    out.push(c);
                    self.advance();
                }
            }
        }
    }

    /// Copy a declaration body (between `{` and matching `}`) into
    /// `self.output`. Handles nested braces (rare but legal — e.g.
    /// custom-property values with `{}` in them, or `@supports`
    /// selectors that leak into declarations by malformation).
    /// Handles strings and comments so their contents don't fool
    /// the brace counter.
    fn copy_declaration_body(&mut self, open_pos: usize) -> Result<(), CssParseError> {
        let mut depth = 1_usize;
        loop {
            match self.peek() {
                None => {
                    return Err(CssParseError {
                        message: "unterminated rule body — expected `}`".into(),
                        pos: open_pos,
                    });
                }
                Some('{') => {
                    depth += 1;
                    self.output.push('{');
                    self.advance();
                }
                Some('}') => {
                    depth -= 1;
                    self.output.push('}');
                    self.advance();
                    if depth == 0 {
                        return Ok(());
                    }
                }
                Some('/') if self.peek_at(1) == Some('*') => {
                    self.copy_block_comment()?;
                }
                Some(c @ ('"' | '\'')) => {
                    self.output.push(c);
                    self.advance();
                    self.copy_string_body(c)?;
                }
                Some(c) => {
                    self.output.push(c);
                    self.advance();
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // At-rule dispatch
    // -----------------------------------------------------------------

    fn parse_at_rule(&mut self) -> Result<(), CssParseError> {
        let start = self.pos;
        // Copy `@` and read the at-rule name.
        self.output.push('@');
        self.advance();
        let name = self.read_at_rule_name();
        self.output.push_str(&name);
        // Read the prelude up to `{` or `;` at depth 0, copying to
        // output verbatim (preludes never need scoping — they're
        // media queries, feature queries, URLs, keyframe names,
        // etc.).
        let terminator = self.copy_at_rule_prelude(start)?;
        match terminator {
            AtRuleTerminator::Semicolon => {
                self.output.push(';');
                self.advance();
                Ok(())
            }
            AtRuleTerminator::Brace => {
                self.output.push('{');
                let brace_pos = self.pos;
                self.advance();
                if at_rule_nests_selectors(&name) {
                    self.parse_nested_stylesheet(brace_pos)
                } else {
                    // Opaque body: copy verbatim, respect brace
                    // depth and strings/comments.
                    self.copy_declaration_body(brace_pos)
                }
            }
            AtRuleTerminator::Eof => Err(CssParseError {
                message: "unterminated at-rule — expected `;` or `{ ... }`".into(),
                pos: start,
            }),
        }
    }

    fn read_at_rule_name(&mut self) -> String {
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

    fn copy_at_rule_prelude(&mut self, start: usize) -> Result<AtRuleTerminator, CssParseError> {
        loop {
            match self.peek() {
                None => return Ok(AtRuleTerminator::Eof),
                Some(';') => return Ok(AtRuleTerminator::Semicolon),
                Some('{') => return Ok(AtRuleTerminator::Brace),
                Some('/') if self.peek_at(1) == Some('*') => {
                    self.copy_block_comment()?;
                }
                Some(c @ '(') => {
                    self.output.push(c);
                    self.advance();
                    self.copy_balanced_body(c, ')', start)?;
                }
                Some(c @ '[') => {
                    self.output.push(c);
                    self.advance();
                    self.copy_balanced_body(c, ']', start)?;
                }
                Some(c @ ('"' | '\'')) => {
                    self.output.push(c);
                    self.advance();
                    self.copy_string_body(c)?;
                }
                Some(c) => {
                    self.output.push(c);
                    self.advance();
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // Helpers: strings, comments, balanced brackets
    // -----------------------------------------------------------------

    fn expect_char(
        &mut self,
        expected: char,
        pos: usize,
        message: &str,
    ) -> Result<(), CssParseError> {
        match self.peek() {
            Some(c) if c == expected => {
                self.advance();
                Ok(())
            }
            _ => Err(CssParseError {
                message: message.into(),
                pos,
            }),
        }
    }

    fn copy_string_body(&mut self, quote: char) -> Result<(), CssParseError> {
        let start = self.pos - 1; // for error reporting
        loop {
            match self.peek() {
                None => {
                    return Err(CssParseError {
                        message: format!("unterminated string — expected closing `{quote}`"),
                        pos: start,
                    });
                }
                Some('\\') => {
                    // CSS escape: copy the `\` and the next char
                    // verbatim (could be `\"`, `\\`, `\A ` for
                    // newline, etc.).
                    self.output.push('\\');
                    self.advance();
                    if let Some(c) = self.peek() {
                        self.output.push(c);
                        self.advance();
                    }
                }
                Some(c) if c == quote => {
                    self.output.push(c);
                    self.advance();
                    return Ok(());
                }
                Some('\n') => {
                    // Unescaped newline in a string is a hard error
                    // in CSS. Report with the string's start pos.
                    return Err(CssParseError {
                        message: format!("unterminated string — newline before closing `{quote}`"),
                        pos: start,
                    });
                }
                Some(c) => {
                    self.output.push(c);
                    self.advance();
                }
            }
        }
    }

    fn copy_balanced_body(
        &mut self,
        open: char,
        close: char,
        start: usize,
    ) -> Result<(), CssParseError> {
        let mut depth = 1_usize;
        loop {
            match self.peek() {
                None => {
                    return Err(CssParseError {
                        message: format!("unbalanced `{open}` — expected closing `{close}`"),
                        pos: start,
                    });
                }
                Some(c) if c == open => {
                    depth += 1;
                    self.output.push(c);
                    self.advance();
                }
                Some(c) if c == close => {
                    depth -= 1;
                    self.output.push(c);
                    self.advance();
                    if depth == 0 {
                        return Ok(());
                    }
                }
                Some('/') if self.peek_at(1) == Some('*') => {
                    self.copy_block_comment()?;
                }
                Some(c @ ('"' | '\'')) => {
                    self.output.push(c);
                    self.advance();
                    self.copy_string_body(c)?;
                }
                Some(c) => {
                    self.output.push(c);
                    self.advance();
                }
            }
        }
    }

    // Variants that write into an external buffer instead of
    // `self.output` — used while reading the selector prelude into
    // a scratch string that the transformer will process.
    fn read_block_comment_into(&mut self, out: &mut String) -> Result<(), CssParseError> {
        let start = self.pos;
        out.push('/');
        out.push('*');
        self.advance();
        self.advance();
        loop {
            match self.peek() {
                None => {
                    return Err(CssParseError {
                        message: "unterminated `/* ... */` block comment".into(),
                        pos: start,
                    });
                }
                Some('*') if self.peek_at(1) == Some('/') => {
                    out.push('*');
                    out.push('/');
                    self.advance();
                    self.advance();
                    return Ok(());
                }
                Some(c) => {
                    out.push(c);
                    self.advance();
                }
            }
        }
    }

    fn read_string_into(&mut self, out: &mut String, quote: char) -> Result<(), CssParseError> {
        let start = self.pos - 1;
        loop {
            match self.peek() {
                None => {
                    return Err(CssParseError {
                        message: format!("unterminated string — expected closing `{quote}`"),
                        pos: start,
                    });
                }
                Some('\\') => {
                    out.push('\\');
                    self.advance();
                    if let Some(c) = self.peek() {
                        out.push(c);
                        self.advance();
                    }
                }
                Some(c) if c == quote => {
                    out.push(c);
                    self.advance();
                    return Ok(());
                }
                Some('\n') => {
                    return Err(CssParseError {
                        message: format!("unterminated string — newline before closing `{quote}`"),
                        pos: start,
                    });
                }
                Some(c) => {
                    out.push(c);
                    self.advance();
                }
            }
        }
    }

    fn read_balanced_into(
        &mut self,
        out: &mut String,
        open: char,
        close: char,
    ) -> Result<(), CssParseError> {
        let start = self.pos - 1;
        let mut depth = 1_usize;
        loop {
            match self.peek() {
                None => {
                    return Err(CssParseError {
                        message: format!("unbalanced `{open}` — expected closing `{close}`"),
                        pos: start,
                    });
                }
                Some(c) if c == open => {
                    depth += 1;
                    out.push(c);
                    self.advance();
                }
                Some(c) if c == close => {
                    depth -= 1;
                    out.push(c);
                    self.advance();
                    if depth == 0 {
                        return Ok(());
                    }
                }
                Some('/') if self.peek_at(1) == Some('*') => {
                    self.read_block_comment_into(out)?;
                }
                Some(c @ ('"' | '\'')) => {
                    out.push(c);
                    self.advance();
                    self.read_string_into(out, c)?;
                }
                Some(c) => {
                    out.push(c);
                    self.advance();
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Selector transformer
// ---------------------------------------------------------------------------

/// The two forms an at-rule can take: `<prelude> ;` (statement) or
/// `<prelude> { <body> }` (block).
enum AtRuleTerminator {
    Semicolon,
    Brace,
    Eof,
}

/// At-rules whose body is a stylesheet (nested rules with real
/// selectors). Everything else is opaque — its body gets copied
/// verbatim without walking into it, because it contains keyframe
/// steps, declarations, URLs, or feature queries — none of which
/// need scoping.
fn at_rule_nests_selectors(name: &str) -> bool {
    matches!(name, "media" | "supports" | "container")
}

/// Split a selector list on top-level commas (respecting `(...)`,
/// `[...]`, strings, and comments) and transform each selector.
/// Rejoin with `,`.
fn transform_selector_list(input: &str, scope: &str) -> String {
    let mut out = String::with_capacity(input.len() + 32);
    let mut buf = String::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut first = true;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ',' => {
                if !first {
                    out.push(',');
                }
                out.push_str(&transform_selector(&buf, scope));
                buf.clear();
                first = false;
                i += 1;
            }
            '(' => {
                let (chunk, consumed) = capture_balanced_slice(&chars, i, '(', ')');
                buf.push_str(&chunk);
                i += consumed;
            }
            '[' => {
                let (chunk, consumed) = capture_balanced_slice(&chars, i, '[', ']');
                buf.push_str(&chunk);
                i += consumed;
            }
            '"' | '\'' => {
                let (chunk, consumed) = capture_string_slice(&chars, i, c);
                buf.push_str(&chunk);
                i += consumed;
            }
            '/' if chars.get(i + 1).copied() == Some('*') => {
                let (chunk, consumed) = capture_comment_slice(&chars, i);
                buf.push_str(&chunk);
                i += consumed;
            }
            _ => {
                buf.push(c);
                i += 1;
            }
        }
    }
    // Trailing selector after the last (or only) comma.
    if !first {
        out.push(',');
    }
    out.push_str(&transform_selector(&buf, scope));
    out
}

/// Rewrite a single selector: append `-<scope>` to every class
/// token `.<ident>`. Everything else — type selectors, IDs,
/// attribute selectors, pseudo-classes, pseudo-elements, combinators
/// — passes through unchanged.
///
/// Walks the input char-by-char. Skips over strings, comments, and
/// attribute selector bodies (`[...]`) so a `.` inside those does
/// not get transformed. Does NOT skip over parenthesised bodies
/// (`(...)`) because selector-arg pseudos like `:not(.foo)`,
/// `:is(.a, .b)`, `:has(.c)` DO want their inner class tokens
/// scoped. Non-selector-arg pseudos (`:nth-child(2n+1)`,
/// `:lang(en)`) don't have `.` inside so the naive walk doesn't
/// touch them; contrived cases like `:nth-child(.foo)` are invalid
/// CSS anyway.
fn transform_selector(input: &str, scope: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len() + 16);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '.' if is_class_start(chars.get(i + 1).copied()) => {
                // Emit `.`, read the identifier, emit
                // `<ident>-<scope>`.
                out.push('.');
                i += 1;
                let ident_end = scan_ident_end(&chars, i);
                let ident: String = chars[i..ident_end].iter().collect();
                out.push_str(&ident);
                out.push('-');
                out.push_str(scope);
                i = ident_end;
            }
            '[' => {
                // Attribute selector — copy verbatim including any
                // `.` inside string values.
                let (chunk, consumed) = capture_balanced_slice(&chars, i, '[', ']');
                out.push_str(&chunk);
                i += consumed;
            }
            '"' | '\'' => {
                let (chunk, consumed) = capture_string_slice(&chars, i, c);
                out.push_str(&chunk);
                i += consumed;
            }
            '/' if chars.get(i + 1).copied() == Some('*') => {
                let (chunk, consumed) = capture_comment_slice(&chars, i);
                out.push_str(&chunk);
                i += consumed;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Return `true` iff `c` can start a CSS ident. Accepts ASCII
/// letters, `_`, and `-` (as a lead — `-webkit-thing`). Refuses
/// digits and other punctuation.
fn is_class_start(c: Option<char>) -> bool {
    matches!(c, Some(ch) if ch.is_ascii_alphabetic() || ch == '_' || ch == '-')
}

/// Return `true` iff `c` can continue a CSS ident. Accepts ASCII
/// alphanumerics, `_`, and `-`.
fn is_ident_cont(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Scan forward from `start` while the chars are ident-continuation.
/// Returns the position immediately after the last ident char.
fn scan_ident_end(chars: &[char], start: usize) -> usize {
    let mut i = start;
    while i < chars.len() && is_ident_cont(chars[i]) {
        i += 1;
    }
    i
}

/// Capture chars from `start` (which is the opening `open`) up to
/// the matching `close`, tracking nesting. Handles strings and
/// comments so their contents don't fool the depth counter.
/// Returns the captured substring (including both brackets) and the
/// number of chars consumed. If the input is malformed
/// (unterminated bracket, string, or comment), returns the rest of
/// the input as the chunk — the outer caller will still surface a
/// downstream error if there really was a problem. This helper is
/// used from the selector transformer where errors have already
/// been surfaced by the main parser during prelude capture.
fn capture_balanced_slice(
    chars: &[char],
    start: usize,
    open: char,
    close: char,
) -> (String, usize) {
    let mut out = String::new();
    let mut i = start;
    let mut depth = 0usize;
    while i < chars.len() {
        let c = chars[i];
        out.push(c);
        i += 1;
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return (out, i - start);
            }
        } else if c == '"' || c == '\'' {
            let (chunk, consumed) = capture_string_slice(chars, i - 1, c);
            // Undo the char we already pushed (it's the opening
            // quote, already at start of `chunk`) and append the
            // full chunk instead.
            out.pop();
            out.push_str(&chunk);
            i = (i - 1) + consumed;
        } else if c == '/' && chars.get(i).copied() == Some('*') {
            // Undo the `/` we pushed and re-emit via comment capture.
            out.pop();
            let (chunk, consumed) = capture_comment_slice(chars, i - 1);
            out.push_str(&chunk);
            i = (i - 1) + consumed;
        }
    }
    (out, i - start)
}

fn capture_string_slice(chars: &[char], start: usize, quote: char) -> (String, usize) {
    let mut out = String::new();
    // The opening quote lives at `start`.
    out.push(chars[start]);
    let mut i = start + 1;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            out.push(c);
            i += 1;
            if i < chars.len() {
                out.push(chars[i]);
                i += 1;
            }
            continue;
        }
        out.push(c);
        i += 1;
        if c == quote {
            return (out, i - start);
        }
    }
    (out, i - start)
}

fn capture_comment_slice(chars: &[char], start: usize) -> (String, usize) {
    let mut out = String::new();
    // `/` and `*` are at start and start+1.
    out.push('/');
    out.push('*');
    let mut i = start + 2;
    while i < chars.len() {
        if chars[i] == '*' && chars.get(i + 1).copied() == Some('/') {
            out.push('*');
            out.push('/');
            return (out, (i + 2) - start);
        }
        out.push(chars[i]);
        i += 1;
    }
    (out, i - start)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = "c-abc";

    #[test]
    fn empty_input_returns_empty_output() {
        assert_eq!(apply_scope("", S).unwrap(), "");
    }

    #[test]
    fn whitespace_only_passes_through_unchanged() {
        assert_eq!(apply_scope("  \n  \t\n", S).unwrap(), "  \n  \t\n");
    }

    #[test]
    fn single_class_selector_gets_scope_suffix() {
        let out = apply_scope(".card { color: red; }", S).unwrap();
        assert_eq!(out, ".card-c-abc { color: red; }");
    }

    #[test]
    fn compound_class_selectors_each_get_scoped() {
        // `.card.title` — two classes on the same element. Both get
        // the suffix. The order matches the input.
        let out = apply_scope(".card.title { padding: 8px; }", S).unwrap();
        assert_eq!(out, ".card-c-abc.title-c-abc { padding: 8px; }");
    }

    #[test]
    fn class_with_hyphen_is_captured_fully() {
        let out = apply_scope(".my-card-x { color: red; }", S).unwrap();
        assert_eq!(out, ".my-card-x-c-abc { color: red; }");
    }

    #[test]
    fn class_with_underscore_and_digits_is_captured() {
        let out = apply_scope("._foo_bar_2 { color: red; }", S).unwrap();
        assert_eq!(out, "._foo_bar_2-c-abc { color: red; }");
    }

    #[test]
    fn multiple_rules_all_get_scoped() {
        let src = ".card { color: red; }\n.title { font-weight: bold; }";
        let out = apply_scope(src, S).unwrap();
        assert_eq!(
            out,
            ".card-c-abc { color: red; }\n.title-c-abc { font-weight: bold; }"
        );
    }

    #[test]
    fn comma_separated_selectors_each_get_scoped_independently() {
        let out = apply_scope(".a, .b { color: red; }", S).unwrap();
        assert_eq!(out, ".a-c-abc, .b-c-abc { color: red; }");
    }

    #[test]
    fn descendant_combinator_scopes_each_side() {
        let out = apply_scope(".a .b { color: red; }", S).unwrap();
        assert_eq!(out, ".a-c-abc .b-c-abc { color: red; }");
    }

    #[test]
    fn child_combinator_scopes_each_side() {
        let out = apply_scope(".a > .b { color: red; }", S).unwrap();
        assert_eq!(out, ".a-c-abc > .b-c-abc { color: red; }");
    }

    #[test]
    fn adjacent_sibling_combinator_scopes_each_side() {
        let out = apply_scope(".a + .b { color: red; }", S).unwrap();
        assert_eq!(out, ".a-c-abc + .b-c-abc { color: red; }");
    }

    #[test]
    fn general_sibling_combinator_scopes_each_side() {
        let out = apply_scope(".a ~ .b { color: red; }", S).unwrap();
        assert_eq!(out, ".a-c-abc ~ .b-c-abc { color: red; }");
    }

    #[test]
    fn pseudo_class_after_class_is_preserved_and_class_gets_scoped() {
        let out = apply_scope(".btn:hover { color: red; }", S).unwrap();
        assert_eq!(out, ".btn-c-abc:hover { color: red; }");
    }

    #[test]
    fn pseudo_element_after_class_is_preserved_and_class_gets_scoped() {
        let out = apply_scope(".btn::before { content: \"x\"; }", S).unwrap();
        assert_eq!(out, ".btn-c-abc::before { content: \"x\"; }");
    }

    #[test]
    fn not_pseudo_scopes_inner_class_argument() {
        // `:not(.foo)` — the inner `.foo` is a selector argument to
        // `:not(...)` and DOES get scoped. The parens don't block
        // the walk because they're commonly used by selector-arg
        // pseudos.
        let out = apply_scope(".a:not(.b) { color: red; }", S).unwrap();
        assert_eq!(out, ".a-c-abc:not(.b-c-abc) { color: red; }");
    }

    #[test]
    fn nth_child_pseudo_passes_through_unchanged() {
        let out = apply_scope(".a:nth-child(2n+1) { color: red; }", S).unwrap();
        assert_eq!(out, ".a-c-abc:nth-child(2n+1) { color: red; }");
    }

    #[test]
    fn type_selector_is_not_scoped() {
        // Documented MVP trade-off: only class selectors get the
        // scope. `div` targets every `<div>` on the page. Users who
        // want per-component styling target classes.
        let out = apply_scope("div { color: red; }", S).unwrap();
        assert_eq!(out, "div { color: red; }");
    }

    #[test]
    fn id_selector_is_not_scoped() {
        let out = apply_scope("#foo { color: red; }", S).unwrap();
        assert_eq!(out, "#foo { color: red; }");
    }

    #[test]
    fn attribute_selector_body_is_not_transformed() {
        // A `.` inside `[data-x="..."]` is a string char, not a
        // class opener. The parser skips the whole `[...]`.
        let out = apply_scope("[data-x=\"a.b\"] { color: red; }", S).unwrap();
        assert_eq!(out, "[data-x=\"a.b\"] { color: red; }");
    }

    #[test]
    fn class_after_attribute_still_gets_scoped() {
        let out = apply_scope("[data-x] .card { color: red; }", S).unwrap();
        assert_eq!(out, "[data-x] .card-c-abc { color: red; }");
    }

    #[test]
    fn declaration_body_is_copied_verbatim() {
        // The `.foo` inside the `content:` string must NOT be
        // transformed — it's a string literal, not a selector.
        let src = ".x { content: \".foo\"; background: url(\"a.b\"); }";
        let out = apply_scope(src, S).unwrap();
        assert_eq!(
            out,
            ".x-c-abc { content: \".foo\"; background: url(\"a.b\"); }"
        );
    }

    #[test]
    fn declaration_body_with_url_no_quotes_is_verbatim() {
        // `url(foo.png)` — no quotes, still a URL. The `.png` looks
        // like a class opener BUT it's inside a declaration body,
        // which is copied verbatim regardless.
        let src = ".x { background: url(foo.png); }";
        let out = apply_scope(src, S).unwrap();
        assert_eq!(out, ".x-c-abc { background: url(foo.png); }");
    }

    #[test]
    fn media_at_rule_recurses_into_nested_rules() {
        let src = "@media (min-width: 800px) { .card { color: red; } }";
        let out = apply_scope(src, S).unwrap();
        assert_eq!(
            out,
            "@media (min-width: 800px) { .card-c-abc { color: red; } }"
        );
    }

    #[test]
    fn supports_at_rule_recurses_into_nested_rules() {
        let src = "@supports (display: grid) { .grid { display: grid; } }";
        let out = apply_scope(src, S).unwrap();
        assert_eq!(
            out,
            "@supports (display: grid) { .grid-c-abc { display: grid; } }"
        );
    }

    #[test]
    fn nested_media_queries_all_recurse() {
        let src = "@media X { @media Y { .a { c: r; } } }";
        let out = apply_scope(src, S).unwrap();
        assert_eq!(out, "@media X { @media Y { .a-c-abc { c: r; } } }");
    }

    #[test]
    fn keyframes_at_rule_body_is_opaque() {
        // `@keyframes fade { 0% { opacity: 0 } 100% { opacity: 1 } }`
        // — the `0%` and `100%` are keyframe steps, NOT selectors.
        // The body copies verbatim; no `.<ident>` transformation
        // happens inside. Passthrough is proved by the absence of a
        // `-c-abc` suffix anywhere.
        let src = "@keyframes fade { 0% { opacity: 0; } 100% { opacity: 1; } }";
        let out = apply_scope(src, S).unwrap();
        assert_eq!(out, src);
    }

    #[test]
    fn font_face_at_rule_body_is_opaque() {
        let src = "@font-face { font-family: \"Foo\"; src: url(foo.woff); }";
        let out = apply_scope(src, S).unwrap();
        assert_eq!(out, src);
    }

    #[test]
    fn import_at_rule_terminated_by_semicolon() {
        let src = "@import \"reset.css\";";
        let out = apply_scope(src, S).unwrap();
        assert_eq!(out, src);
    }

    #[test]
    fn block_comment_before_rule_is_preserved() {
        let out = apply_scope("/* top */ .a { color: red; }", S).unwrap();
        assert_eq!(out, "/* top */ .a-c-abc { color: red; }");
    }

    #[test]
    fn block_comment_inside_selector_is_preserved() {
        let out = apply_scope(".a /* mid */ .b { c: r; }", S).unwrap();
        assert_eq!(out, ".a-c-abc /* mid */ .b-c-abc { c: r; }");
    }

    #[test]
    fn block_comment_inside_declaration_body_is_preserved() {
        let out = apply_scope(".a { /* note */ color: red; }", S).unwrap();
        assert_eq!(out, ".a-c-abc { /* note */ color: red; }");
    }

    #[test]
    fn unterminated_rule_body_errors_with_open_pos() {
        let err = apply_scope(".a { color: red;", S).unwrap_err();
        assert!(err.message.contains("unterminated rule body"));
        assert!(err.message.contains("`}`"));
    }

    #[test]
    fn unterminated_string_in_declaration_errors() {
        let err = apply_scope(".a { content: \"unclosed ", S).unwrap_err();
        assert!(
            err.message.contains("unterminated string"),
            "unexpected: {}",
            err.message
        );
    }

    #[test]
    fn unterminated_block_comment_errors() {
        let err = apply_scope(".a { /* never closes", S).unwrap_err();
        assert!(err.message.contains("unterminated"));
        assert!(err.message.contains("comment"));
    }

    #[test]
    fn unterminated_media_body_errors_with_expected_close() {
        let err = apply_scope("@media X { .a { c: r; } ", S).unwrap_err();
        assert!(err.message.contains("unterminated"));
        assert!(err.message.contains("`}`"));
    }

    #[test]
    fn universal_selector_passes_through() {
        // Universal selector — not a class, not touched.
        let out = apply_scope("* { box-sizing: border-box; }", S).unwrap();
        assert_eq!(out, "* { box-sizing: border-box; }");
    }

    #[test]
    fn descendant_of_type_and_class_scopes_only_class() {
        // `div .card` — type selector on the left, class on the
        // right. Only the class gets scoped; the type stays as is.
        let out = apply_scope("div .card { color: red; }", S).unwrap();
        assert_eq!(out, "div .card-c-abc { color: red; }");
    }

    #[test]
    fn compound_type_plus_class_scopes_only_class() {
        // `div.card` — a `div` element with class `card`. The class
        // gets scoped; the type stays.
        let out = apply_scope("div.card { color: red; }", S).unwrap();
        assert_eq!(out, "div.card-c-abc { color: red; }");
    }

    #[test]
    fn class_after_id_gets_scoped() {
        let out = apply_scope("#header .card { color: red; }", S).unwrap();
        assert_eq!(out, "#header .card-c-abc { color: red; }");
    }

    #[test]
    fn multiline_rule_preserves_whitespace() {
        let src = ".card {\n  color: red;\n  padding: 8px;\n}\n";
        let out = apply_scope(src, S).unwrap();
        assert_eq!(out, ".card-c-abc {\n  color: red;\n  padding: 8px;\n}\n");
    }

    #[test]
    fn is_pseudo_scopes_inner_class_list() {
        // `:is(.a, .b)` — a selector-arg pseudo with a comma-
        // separated list of classes. Each inner class gets scoped
        // because the walk into `(...)` transforms `.<ident>`
        // tokens.
        let out = apply_scope(".x:is(.a, .b) { color: red; }", S).unwrap();
        assert_eq!(out, ".x-c-abc:is(.a-c-abc, .b-c-abc) { color: red; }");
    }

    #[test]
    fn dot_inside_pseudo_arg_gets_scoped_intentionally() {
        // Whether the pseudo-class actually wants a selector inside
        // (`:not(...)`) or not (`:nth-child(...)`), the walk
        // transforms `.<ident>`. For `:nth-child(2n+1)` there's no
        // `.` so nothing happens. For `:not(.foo)` the `.foo` is
        // intended to be scoped, which is correct.
        let out = apply_scope(":not(.foo) { color: red; }", S).unwrap();
        assert_eq!(out, ":not(.foo-c-abc) { color: red; }");
    }

    #[test]
    fn plain_dot_without_ident_start_is_preserved() {
        // `.5` (invalid CSS but grammar-legal in some places) is
        // NOT a class opener because `5` isn't an ident start.
        // Same for a bare `.` at end of input or before a space.
        let src = ".a { padding: .5em; }";
        let out = apply_scope(src, S).unwrap();
        assert_eq!(out, ".a-c-abc { padding: .5em; }");
    }

    #[test]
    fn scope_string_is_used_verbatim() {
        // The parser trusts the caller to pass a valid CSS ident
        // continuation. Different scope strings appear unchanged in
        // the output.
        let out = apply_scope(".a {}", "card-c-01234567").unwrap();
        assert_eq!(out, ".a-card-c-01234567 {}");
    }

    #[test]
    fn nested_media_scoping_preserves_whitespace_around_braces() {
        let src = "@media (max-width: 800px) {\n  .a {\n    color: red;\n  }\n}\n";
        let out = apply_scope(src, S).unwrap();
        assert_eq!(
            out,
            "@media (max-width: 800px) {\n  .a-c-abc {\n    color: red;\n  }\n}\n"
        );
    }
}
