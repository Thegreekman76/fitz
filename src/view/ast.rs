// view/ast.rs — AST for `.fitzv` Single-File Components (Phase 11 POC).
//
// **POC scope.** Field defaults, event handler bodies, and template
// interpolations are captured as **raw source strings**. Later
// sub-phases (11.2+) will:
//   - parse defaults / interpolations as `crate::ast::Expr` (reusing
//     the classic lexer plus a sub-parser);
//   - parse event handler bodies as `Vec<crate::ast::Stmt>`;
//   - grow the template AST with `{#if}` / `{#for}` / `{#slot}`;
//   - grow attribute values with binding kinds (event `@click`,
//     one-way `:prop`, two-way `v-model`-ish).
//
// The POC deliberately does NOT feed the AST to the checker,
// evaluator, or codegen — it only proves the parser recognises the
// SFC shape end-to-end.

/// A parsed `.fitzv` file — one or more components. The POC only
/// exercises single-component files; multi-component is legal per
/// spec but the plan defers wiring to 11.2+.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewFile {
    pub components: Vec<Component>,
}

/// A single component block: `component Name { ... }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    pub name: String,
    /// Position of the `component` keyword. 1-based (line, column).
    pub loc: Loc,
    pub state: Vec<StateField>,
    pub events: Vec<EventHandler>,
    pub template: Option<Template>,
    pub style: Option<Style>,
}

/// A `state { ... }` field. POC captures the type annotation and
/// default as opaque strings — checker + evaluator wiring lands in
/// 11.2+.
#[derive(Debug, Clone, PartialEq)]
pub struct StateField {
    pub name: String,
    /// Type annotation as raw source text (e.g. `"Str"`, `"List<Int>"`,
    /// `"Bool?"`). Always present in the POC — the plan requires
    /// annotated state.
    pub type_expr_raw: String,
    /// Default expression as raw source text (e.g. `"\"Untitled\""`,
    /// `"false"`, `"[]"`). Always present in the POC — defaults are
    /// mandatory (parallel to `@live_component` types today, so the
    /// compiler can synthesise `TypeName {}` at boot).
    pub default_expr_raw: String,
    pub loc: Loc,
}

/// An `event name(params) { body }` handler.
#[derive(Debug, Clone, PartialEq)]
pub struct EventHandler {
    pub name: String,
    /// Params as raw source text between `(` and `)`. Empty string
    /// when there are no params. POC does not parse them yet.
    pub params_raw: String,
    /// Body as raw source text between `{` and `}`, WITHOUT the
    /// braces. Preserves the user's exact formatting for later
    /// re-lexing. POC does not parse it yet.
    pub body_raw: String,
    pub loc: Loc,
}

/// A `<template>...</template>` block parsed as a small tree of
/// nodes (Text / Interpolation / Element). Whitespace between
/// sibling elements is preserved as `Text` nodes.
#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    pub roots: Vec<TemplateNode>,
    pub loc: Loc,
}

/// A node inside a `<template>` block. POC covers:
///   - `Text` — raw text between tags (no interpolation).
///   - `Interpolation` — `{expr}` — captured as raw source text.
///   - `Element` — `<tag attr="v" @click="ev">children</tag>` and
///     the self-closing variant `<tag/>`.
///   - `If` — `{#if cond}...{/if}` — since 11.2.c mini-commit 1.
///     The `cond` is captured raw and re-parsed as a classic Fitz
///     `Expr` in `expand`; the `children` are the nodes between
///     opener and closer, allowing arbitrary nesting.
///
/// Deferred to 11.2.c mini-commits 2/3:
///   - `{#if cond} ... {#else} ... {/if}` — `#else` branch
///   - `{#for x in xs} ... {/for}` blocks
///   - `<slot name="X" />` for component composition
///
/// Not planned for 11.2:
///   - HTML doctype, XML processing instructions, CDATA
#[derive(Debug, Clone, PartialEq)]
pub enum TemplateNode {
    Text(String),
    /// `{expr_raw}` — captured verbatim without the braces.
    Interpolation {
        expr_raw: String,
        loc: Loc,
    },
    Element {
        tag: String,
        attrs: Vec<Attr>,
        children: Vec<TemplateNode>,
        self_closing: bool,
        loc: Loc,
    },
    /// `{#if cond_raw} ... {/if}` — conditional inclusion of the
    /// nested `children`. `cond_raw` is the raw source text between
    /// `{#if ` and the matching `}` (trimmed). The `#else` branch is
    /// deferred to 11.2.c mini-commit 2.
    If {
        cond_raw: String,
        children: Vec<TemplateNode>,
        loc: Loc,
    },
}

/// An HTML attribute inside an element start tag. POC covers three
/// kinds:
///   - `Static { name, value }` — `class="card"`.
///   - `Interpolation { name, expr_raw }` — `value="{title}"`. The
///     POC only accepts values that are fully interpolated (start
///     with `{` and end with `}`).
///   - `Event { event_name, handler_raw }` — `@click="handler"`.
///
/// Deferred to 11.2+:
///   - Attributes with mixed static + interpolated fragments
///     (`class="btn btn-{kind}"`) — POC allows only fully-static or
///     fully-interpolated values.
///   - Boolean attributes without a value (`disabled`, `hidden`) —
///     POC requires `attr="value"` shape.
///   - Two-way binding (`v-model`-style).
#[derive(Debug, Clone, PartialEq)]
pub enum Attr {
    Static {
        name: String,
        value: String,
        loc: Loc,
    },
    Interpolation {
        name: String,
        expr_raw: String,
        loc: Loc,
    },
    Event {
        event_name: String,
        handler_raw: String,
        loc: Loc,
    },
}

/// A `<style scoped>...</style>` block. POC captures the raw CSS
/// text without parsing it. The `scoped` attribute is mandatory in
/// the POC — un-scoped styles are deferred to 11.3+ pending a
/// decision on the global-style story.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub css_raw: String,
    pub loc: Loc,
}

/// 1-based (line, column). Mirrors `crate::ast::Span` but stays
/// intentionally private to `view/` so the classic AST does not
/// grow a dependency on `view`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Loc {
    pub line: usize,
    pub column: usize,
}

impl Loc {
    pub fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }
}
