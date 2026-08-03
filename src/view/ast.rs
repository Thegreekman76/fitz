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
    /// §9.dd (2026-07-16) — Top-level `from X import Y1, Y2, ...`
    /// declarations. Emitted verbatim as classic Fitz `from ...
    /// import ...` at the top of the transformed source so the
    /// classic loader resolves the nominals across files. Enables
    /// cross-file nominal type refs in state annotations (`state {
    /// messages: List<Message> = [] }` where `Message` is in a
    /// sibling `message.fitz`) and struct literals inside event
    /// bodies (`messages.push(Message { ... })`).
    pub imports: Vec<ViewImport>,
    pub components: Vec<Component>,
}

/// §9.dd — A `from X import Y1, Y2, ...` declaration at the top
/// of a `.fitzv` file. Module path is a dotted sequence of segments
/// (`from foo.bar import Baz` → `path: ["foo", "bar"]`). Names are
/// the identifiers imported; multi-name shortcuts like `from foo
/// import Baz, Quux` produce a single `ViewImport` with two names.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewImport {
    /// Module path (dotted, no trailing empty). `["Counter"]` for
    /// `from Counter import ...`; `["utils", "shared"]` for
    /// `from utils.shared import ...`.
    pub path: Vec<String>,
    /// Names imported from the module. Each entry is `(original,
    /// Option<alias>)` mirroring the classic Fitz `Stmt::FromImport`
    /// shape (PreF8.4). Aliases (`from X import Y as Z`) put `Z`
    /// in the SFC's scope; the emitted classic Fitz module keeps
    /// the alias verbatim so the loader validates the call against
    /// `X`'s exports. Post S.1 (2026-07-17) — was `Vec<String>`
    /// before with the parser rejecting `as`.
    pub names: Vec<(String, Option<String>)>,
    /// Position of the `from` keyword. 1-based (line, column).
    pub loc: Loc,
}

/// A single component block: `component Name { ... }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    pub name: String,
    /// Position of the `component` keyword. 1-based (line, column).
    pub loc: Loc,
    /// Phase 11.12 slice 4 — hydration opt-in marker (`component App
    /// hydrate { ... }`). When set on the ROOT component, the whole
    /// file's component tree hydrates the server-painted DOM instead of
    /// fresh-mounting: the entry wrapper adopts, and every naive
    /// component in the tree emits a `hydrate()` that adopts its DOM +
    /// wires listeners (composition included — `<Child />` + `<slot>`).
    /// Keep-node components auto-hydrate regardless of this flag (slices
    /// 1–3); this opt-in only lifts the NAIVE (composition) path, kept
    /// off by default so pre-11.12 examples regenerate byte-identical.
    pub hydrate: bool,
    pub state: Vec<StateField>,
    /// Phase 11.10 slice 4 — read-only derived values (`derived { name: T =
    /// expr }`). Same shape as a state field, but recomputed from state (+
    /// other derived) instead of stored + mutated.
    pub derived: Vec<StateField>,
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
///   - `If` — `{#if cond}...{/if}` or `{#if cond}...{#else}...{/if}`
///     since 11.2.c (mini-commit 1 added the base form, mini-commit
///     3 added the `{#else}` branch). The `cond` is captured raw and
///     re-parsed as a classic Fitz `Expr` in `expand`; the
///     `children` are the then-branch nodes and `else_children` is
///     `Some(...)` when the block has an `{#else}` branch, `None`
///     for plain `{#if}...{/if}`. Nested if bodies work
///     independently (each `{#if}` scopes its own `{#else}`).
///   - `For` — `{#for x in xs}...{/for}` — since 11.2.c mini-commit 2.
///     The binding `var` is a bare identifier; `iter_raw` is the raw
///     source text of the iter expression, re-parsed as a classic
///     Fitz `Expr` in `expand`. Type inference of `x` delegates to
///     the classic `Stmt::For` checker via synth-and-delegate.
///     Compound patterns (`(k, v)` for Map) and index bindings
///     (`(x, i)`) are deferred to later mini-commits.
///   - `Slot` — `<slot />` or `<slot name="X" />` — since 11.2.c
///     mini-commit 3. Opaque parent/child composition marker;
///     `name` is `None` for the default slot, `Some("X")` for a
///     named slot. Only the self-closing form is accepted today
///     (fallback children `<slot>...</slot>` and cross-checking
///     the slot set against the child component's declared slots
///     land in Phase 11.5 when composition wires up).
///
/// Deferred to later mini-commits (not blocking 11.2.c):
///   - `<slot>...</slot>` with fallback children.
///   - `{#elseif cond}` chains (Fitz stays at just `{#if}` /
///     `{#else}` — the `elseif` shorthand is refinable if demand
///     appears).
///
/// Not planned for 11.2:
///   - HTML doctype, XML processing instructions, CDATA.
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
    /// `{#if cond_raw} ... {/if}` — conditional inclusion. `cond_raw`
    /// is the raw source text between `{#if ` and the matching `}`
    /// (trimmed). `children` are the then-branch nodes;
    /// `else_children` is `Some(...)` when the block has an
    /// `{#else}` branch and `None` otherwise. Only one `{#else}` per
    /// `{#if}` is allowed; nested if bodies scope their own
    /// `{#else}` independently.
    If {
        cond_raw: String,
        children: Vec<TemplateNode>,
        else_children: Option<Vec<TemplateNode>>,
        loc: Loc,
    },
    /// `{#for var in iter_raw} ... {/for}` — iterate over an iterable
    /// expression. `var` is a bare identifier bound in `children`'s
    /// scope with the element type of the iter (List<T> → T, Range →
    /// Int, Map<K,V> → Tuple[K,V]) as resolved by the classic
    /// `Stmt::For` checker. `iter_raw` is the raw source text between
    /// `in` and the matching `}` (trimmed). `key_raw` holds the raw
    /// source of a `key=<expr>` clause (`{#for x in xs key=x.id}`),
    /// trimmed, or `None` when absent — the keyed-diffing sugar that
    /// desugars to a `data-flv-key="{<expr>}"` attribute on the loop
    /// body's root element in `expand`.
    For {
        var: String,
        iter_raw: String,
        key_raw: Option<String>,
        children: Vec<TemplateNode>,
        loc: Loc,
    },
    /// `<slot />` (default) or `<slot name="X" />` (named) —
    /// parent/child composition marker. `name` is `None` for the
    /// default slot, `Some("X")` for a named slot. `fallback` holds
    /// the children of a `<slot>...</slot>` block (Phase 11.7.d),
    /// rendered when the parent provides no content for the slot;
    /// empty for a self-closing `<slot />`.
    Slot {
        name: Option<String>,
        fallback: Vec<TemplateNode>,
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

/// A `<style scoped>...</style>` or `<style global>...</style>`
/// block. The `kind` distinguishes the two shapes; `css_raw` is the
/// CSS body without the tags.
///
/// Since Phase 11.3.a: the block MUST carry either `scoped` or
/// `global` as its opt-in attribute — bare `<style>` is rejected at
/// the lexer with a clear message pointing at the two accepted
/// forms. Rationale: explicit intent is cheap to write once and
/// removes ambiguity in the tree ("is this component's `<style>`
/// scoped by default like Svelte, or global by default like Vue?").
///
/// The POC caps at ONE `<style>` block per component regardless of
/// kind. Allowing "one scoped + one global" side by side (Vue /
/// Svelte convention) is refinable in a follow-up if demand
/// appears; today, if you need both, factor the global rules into a
/// dedicated global-only component or use `@global(...)` targeted
/// rules once that shape lands.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub kind: StyleKind,
    pub css_raw: String,
    pub loc: Loc,
}

/// Discriminates a `<style ...>` block by its scoping opt-in.
///
///   - `Scoped` — `<style scoped>`. Since 11.3.c, its CSS gets a
///     per-component class-prefix applied to every simple selector
///     (`.foo` → `.foo-c-XXXXXXXX`), and every element in the
///     component's template receives that same prefix class on its
///     `class` attribute. The result is CSS that only applies to
///     this component's markup, verified against a per-component
///     hash of the CSS body.
///   - `Global` — `<style global>`. The CSS is emitted as-is,
///     shared across the whole document. Intended for cross-cutting
///     rules like resets, base typography, or utility classes that
///     the component owns but wants to expose beyond itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleKind {
    Scoped,
    Global,
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
