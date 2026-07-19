// view/expand.rs — Phase 11.2.a — lower a raw `.fitzv` `ViewFile`
// into an `ExpandedViewFile` where every raw source blob has been
// parsed as classic Fitz AST (`crate::ast::TypeExpr`, `Expr`, `Stmt`,
// `Param`).
//
// **Scope of this commit**: parsing only. The classic checker is NOT
// invoked from here — that lands in Phase 11.2.b. The goal is to
// prove the wiring from `.fitzv` raw blobs back through the classic
// lexer + parser is straightforward, and to give the follow-up
// checker step a well-typed AST to work against.
//
// **Isolation model** (Invariant 4 of `docs/stack.md`): the view
// parser (`src/view/parser.rs`) stays isolated from the classic
// pipeline. This module is the ONE bridge that reuses
// `crate::lexer::tokenize` + `crate::parser::parse_*_from_source`.
// A bug in the view parser cannot reach the classic pipeline, but
// this bridge deliberately consumes both — that is its role.
//
// **Position mapping**: raw blobs are tokenized as if they started at
// (1, 1) in their own coord system. Before parsing, we shift each
// token's `(line, column)` by the blob's base offset so that spans
// inside the produced AST — and error positions — sit inside the
// enclosing `.fitzv` file. The base offset today is approximate
// (the POC parser captures the field/handler/interpolation's Loc,
// not the exact start of the raw content); refining this needs the
// POC's parser to also track the blob start offset, which is debt
// documented in `docs/fase-11-plan.md` §7.

use super::ast::{
    Attr as RawAttr, Component as RawComponent, EventHandler as RawEventHandler, Loc,
    StateField as RawStateField, Style as RawStyle, StyleKind, Template as RawTemplate,
    TemplateNode as RawTemplateNode, ViewFile,
};
use super::css_parser::{apply_scope, CssParseError};
use crate::ast as fast; // Fitz AST — imported prefixed to avoid collisions with view AST names.
use crate::error::FitzError;
use crate::lexer::{tokenize, TokenWithPos};
use crate::parser::{
    parse_expression_from_source, parse_parameters_from_source, parse_statements_from_source,
    parse_type_expression_from_source,
};
use std::fmt;

// ---------------------------------------------------------------------------
// Expanded AST — mirrors the raw view AST 1:1, but with every raw
// source blob replaced by a parsed classic-Fitz AST node.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct ExpandedViewFile {
    /// §9.dd (2026-07-16) — Top-level `from X import Y1, Y2, ...`
    /// declarations propagated from the raw `ViewFile`. Enables cross-
    /// file nominal type refs; view checker registers each name as
    /// an opaque nominal stub, and view SSR emitter emits `from X
    /// import Y1, Y2` at the top of the transformed classic source.
    pub imports: Vec<ExpandedViewImport>,
    pub components: Vec<ExpandedComponent>,
}

/// §9.dd — Expanded form of `ViewImport` (name-preserving; the
/// expand pass does not transform imports, just carries them
/// through). Kept as a distinct type so future expand-time
/// validation (e.g. duplicate name detection) can hang off it
/// without changing the AST node's fields.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpandedViewImport {
    pub path: Vec<String>,
    /// Each entry is `(original, Option<alias>)` mirroring the classic
    /// Fitz `Stmt::FromImport` shape. Post S.1 (2026-07-17) — was
    /// `Vec<String>` before with the parser rejecting `as`.
    pub names: Vec<(String, Option<String>)>,
    pub loc: Loc,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExpandedComponent {
    pub name: String,
    pub loc: Loc,
    pub state: Vec<ExpandedStateField>,
    pub events: Vec<ExpandedEventHandler>,
    pub template: Option<ExpandedTemplate>,
    /// The component's `<style scoped>` or `<style global>` block
    /// after processing. `Scoped` blocks carry the CSS with every
    /// class selector suffixed with `-<scope_class>` (via
    /// `css_parser::apply_scope`) plus the synthesised
    /// `scope_class` that the template rewrite also injects into
    /// every element's `class` attribute. `Global` blocks carry
    /// the CSS verbatim — no transformation, no template rewrite.
    /// See §9.k of `docs/fase-11-plan.md` for the exact wiring.
    pub style: Option<ExpandedStyle>,
}

/// A component's `<style ...>` block after 11.3.c's wiring runs.
/// Mirrors `StyleKind` but carries the actual processed data
/// instead of the raw source.
///
/// - `Scoped`: `css_scoped` is the CSS body with `.<ident>` selectors
///   rewritten to `.<ident>-<scope_class>`; `scope_class` is
///   `<component-kebab>-c-<8hex>` where the 8-hex payload is
///   FNV-1a of `<component name>::<original css body>` — same
///   name + same CSS → same class, guaranteed. The template
///   rewrite (also 11.3.c) adds `<class>-<scope_class>` for every
///   original class token on every element's `class` attribute,
///   preserving the originals so external `.<class>` queries in
///   user JS keep working.
/// - `Global`: `css` is the raw CSS body copied verbatim. No
///   scoping, no template rewrite. Intended for cross-cutting
///   rules like resets or utility classes the component owns but
///   exposes beyond itself.
#[derive(Debug, Clone, PartialEq)]
pub enum ExpandedStyle {
    Scoped {
        css_scoped: String,
        scope_class: String,
        loc: Loc,
    },
    Global {
        css: String,
        loc: Loc,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExpandedStateField {
    pub name: String,
    pub type_expr: fast::TypeExpr,
    pub default: fast::Expr,
    pub loc: Loc,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExpandedEventHandler {
    pub name: String,
    pub params: Vec<fast::Param>,
    pub body: Vec<fast::Stmt>,
    pub loc: Loc,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExpandedTemplate {
    pub roots: Vec<ExpandedTemplateNode>,
    pub loc: Loc,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExpandedTemplateNode {
    Text(String),
    Interpolation {
        expr: fast::Expr,
        loc: Loc,
    },
    Element {
        tag: String,
        attrs: Vec<ExpandedAttr>,
        children: Vec<ExpandedTemplateNode>,
        self_closing: bool,
        loc: Loc,
    },
    /// `{#if cond}...{/if}` or `{#if cond}...{#else}...{/if}` —
    /// conditional inclusion. `cond` is parsed from the raw
    /// `cond_raw` captured by the HTML sub-parser; `children` are
    /// the then-branch, expanded recursively; `else_children` is
    /// `Some(...)` when the raw AST carried an `{#else}` branch and
    /// `None` otherwise. Since 11.2.c mini-commit 3.
    If {
        cond: fast::Expr,
        children: Vec<ExpandedTemplateNode>,
        else_children: Option<Vec<ExpandedTemplateNode>>,
        loc: Loc,
    },
    /// `{#for var in iter}...{/for}` — iterate over `iter`. `iter`
    /// is parsed from the raw `iter_raw` captured by the HTML sub-
    /// parser; `children` are expanded recursively. Type of `var`
    /// is inferred by the classic `Stmt::For` checker in
    /// `check_template_for_iters`.
    For {
        var: String,
        iter: fast::Expr,
        children: Vec<ExpandedTemplateNode>,
        loc: Loc,
    },
    /// `<slot />` (default) or `<slot name="X" />` (named) —
    /// parent/child composition marker. `fallback` (Phase 11.7.d) holds
    /// the expanded children of a `<slot>...</slot>` block, rendered when
    /// the parent provides no content for the slot; empty otherwise.
    Slot {
        name: Option<String>,
        fallback: Vec<ExpandedTemplateNode>,
        loc: Loc,
    },
    /// `<Child prop="v" />` — mount a nested component with static
    /// props. Phase 11.5.d.
    ///
    /// - `name` is the child component's declared name (e.g.
    ///   `"Card"`). Cross-checked against the file's components in
    ///   `check.rs::check_child_components`.
    /// - `props` are the static attributes to pass through to the
    ///   child's `state` fields. `raw_value` is the source text as
    ///   captured by the HTML sub-parser (e.g. `"42"` for `count`,
    ///   `"Hello"` for `title`) — the checker coerces it to the
    ///   declared state-field type.
    /// - `self_closing == true` is enforced at expand time. Fallback
    ///   children (`<Card>...</Card>`) require `<slot>` fill-in
    ///   wiring which is 11.6+ work.
    ///
    /// Dynamic props (`prop={expr}`), events on child components
    /// (`@click="..."`), and fallthrough attrs are rejected at
    /// expand time with targeted 11.6 pointers.
    ChildComponent {
        name: String,
        props: Vec<ChildComponentProp>,
        /// Phase 11.7.b R2b — the `key="{expr}"` attribute, parsed as
        /// a classic-Fitz expression. Set when the child site carries
        /// a `key` attribute (the canonical shape for a `<Child />`
        /// inside a `{#for}`: `<Card key="{x}" ... />`). `None` for
        /// static sites. The WASM emitter uses it to give each
        /// dynamic child a stable identity so its instance (and its
        /// local state) is reused across re-renders via a keyed
        /// instance cache + reconciliation. The `key` is NOT a prop —
        /// it is stripped from `props` at expand time.
        key: Option<fast::Expr>,
        /// Phase 11.7.c — event bindings on the child
        /// (`<Card @select="on_select" />`). Each maps a child event name
        /// to a handler declared by the PARENT component. When the child's
        /// event fires, the bound parent handler runs (event bubbling).
        /// Empty for children with no `@event` bindings.
        events: Vec<ChildEventBinding>,
        /// Phase 11.7.d — the parent-provided content of a non-self-closing
        /// `<Child>...</Child>`, which fills the child's `<slot />`. Empty
        /// for a self-closing `<Child />` (the child renders its `<slot />`
        /// fallback, if any). Rendered by the PARENT (parent state + events)
        /// via a synthesised `__render_slot_<n>` method on the WASM target.
        slot_content: Vec<ExpandedTemplateNode>,
        loc: Loc,
    },
}

/// An `@event="handler"` binding on a `<Child />` composition site
/// (Phase 11.7.c event bubbling). `event_name` is the child's event;
/// `handler_name` is the parent component's handler to run when it fires.
#[derive(Debug, Clone, PartialEq)]
pub struct ChildEventBinding {
    pub event_name: String,
    pub handler_name: String,
    pub loc: Loc,
}

/// A prop passed to a `<Child prop="v" />` or `<Child prop={expr} />`
/// composition site.
///
/// Two shapes:
/// - **Static** (`prop="raw"`): `raw_value` holds the source text
///   captured by the HTML sub-parser; `expr_raw` is `None`. The
///   checker coerces `raw_value` to the child's declared state-
///   field type via `check.rs::coerce_child_prop_raw_value`, and
///   the emitters produce a literal at the composition site.
/// - **Interpolated** (`prop={expr}`, K-3 remainder — post-v0.21.0):
///   `expr_raw` holds the trimmed expression source (e.g. `"seedCards"`,
///   `"state.cards"`, `"count + 1"`); `raw_value` mirrors the same
///   text for error messages but is not coerced. The SSR emitter
///   inlines the expression verbatim in the struct literal (with
///   the checker's state-field rewriting rules). The WASM emitter
///   errors out today with a clear "client-side dynamic composition
///   deferred to Phase 11.7+" pointer.
///
/// Static coercion supports `Str` / `Int` / `Float` / `Bool` /
/// `Nullable<primitive>` (Phase 11.5.d) + `List<primitive>` (K-3,
/// post-v0.21.0). Interpolated props bypass coercion — any type
/// expressible in classic Fitz that matches the child's field type
/// works.
#[derive(Debug, Clone, PartialEq)]
pub struct ChildComponentProp {
    pub field_name: String,
    pub raw_value: String,
    /// Set when the prop was written as `prop={expr}`. Holds the
    /// PARSED classic-Fitz expression (via the same `parse_expr_at`
    /// helper used by other template interpolations). The SSR
    /// emitter runs it through `format_fitz_expr_scoped` to inline
    /// the source with state-field rewriting. The WASM emitter
    /// today rejects with a Phase 11.7+ pointer.
    pub expr: Option<fast::Expr>,
    pub loc: Loc,
}

impl ChildComponentProp {
    /// True when the prop was written as `prop={expr}` — the emitter
    /// should NOT coerce; it should inline the expression source
    /// with any applicable state-field rewrites.
    pub fn is_interpolated(&self) -> bool {
        self.expr.is_some()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExpandedAttr {
    Static {
        name: String,
        value: String,
        loc: Loc,
    },
    Interpolation {
        name: String,
        expr: fast::Expr,
        loc: Loc,
    },
    /// `@click="handler"` — the value must be a bare identifier that
    /// names one of the enclosing component's `event ...` handlers.
    /// Cross-checking that identity happens in Phase 11.2.b (checker).
    Event {
        event_name: String,
        handler_name: String,
        loc: Loc,
    },
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// An expansion error carries a message plus a `Loc` inside the
/// `.fitzv` file. All classic-parser errors are shifted here — the
/// caller never sees a raw `FitzError` with blob-local coords.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpandError {
    pub message: String,
    pub loc: Loc,
    /// Optional label naming the context in which the error arose
    /// (e.g. `"state field 'title' type"`, `"event handler 'save' body"`).
    /// Helps the user find the right blob in a busy component.
    pub context: String,
}

impl fmt::Display for ExpandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "view expand error at {}:{} — {} ({})",
            self.loc.line, self.loc.column, self.message, self.context
        )
    }
}

impl std::error::Error for ExpandError {}

pub type ExpandResult<T> = Result<T, ExpandError>;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Lower a raw `.fitzv` view file into its expanded form. Every
/// state field, event handler, template interpolation, and attr
/// binding has its raw source parsed as classic Fitz AST.
///
/// Returns the first error encountered. Multi-error recovery is
/// deliberately deferred: this pass is a strict compile-time step,
/// mirroring how the classic `parse()` behaves. Recovery (for the
/// LSP) will land alongside 11.7 when the view LSP surface lands.
pub fn expand(file: &ViewFile) -> ExpandResult<ExpandedViewFile> {
    let imports = file
        .imports
        .iter()
        .map(|imp| ExpandedViewImport {
            path: imp.path.clone(),
            names: imp.names.clone(),
            loc: imp.loc,
        })
        .collect();
    let components = file
        .components
        .iter()
        .map(expand_component)
        .collect::<ExpandResult<Vec<_>>>()?;
    Ok(ExpandedViewFile {
        imports,
        components,
    })
}

fn expand_component(c: &RawComponent) -> ExpandResult<ExpandedComponent> {
    let state = c
        .state
        .iter()
        .map(|f| expand_state_field(f, &c.name))
        .collect::<ExpandResult<Vec<_>>>()?;
    let events = c
        .events
        .iter()
        .map(|e| expand_event_handler(e, &c.name))
        .collect::<ExpandResult<Vec<_>>>()?;
    let mut template = c
        .template
        .as_ref()
        .map(|t| expand_template(t, &c.name))
        .transpose()?;
    // Process the style block (if any). For `Scoped`, this also
    // mutates `template` in place to add suffixed class variants
    // — has to run AFTER `expand_template` so the walker sees the
    // already-expanded tree.
    let style = process_style(&c.style, &c.name, template.as_mut())?;
    Ok(ExpandedComponent {
        name: c.name.clone(),
        loc: c.loc,
        state,
        events,
        template,
        style,
    })
}

/// Turn a raw `Style` block into its `ExpandedStyle` form. For
/// `Scoped`, synthesises the scope class, runs the CSS through
/// `apply_scope`, and — if a template is present — rewrites every
/// element's static `class` attribute to add the suffixed variants.
/// For `Global`, copies the CSS verbatim; no template mutation.
/// `None` in → `None` out.
fn process_style(
    raw: &Option<RawStyle>,
    component_name: &str,
    template: Option<&mut ExpandedTemplate>,
) -> ExpandResult<Option<ExpandedStyle>> {
    let Some(style) = raw else {
        return Ok(None);
    };
    match style.kind {
        StyleKind::Scoped => {
            let scope_class = synth_scope_class(component_name, &style.css_raw);
            let css_scoped = apply_scope(&style.css_raw, &scope_class)
                .map_err(|e| css_parse_error_to_expand(e, style.loc, component_name))?;
            if let Some(t) = template {
                rewrite_class_attrs_in_template(&mut t.roots, &scope_class);
            }
            Ok(Some(ExpandedStyle::Scoped {
                css_scoped,
                scope_class,
                loc: style.loc,
            }))
        }
        StyleKind::Global => Ok(Some(ExpandedStyle::Global {
            css: style.css_raw.clone(),
            loc: style.loc,
        })),
    }
}

/// Shift a CSS parse error into an `ExpandError` located at the
/// `<style scoped>` block's `Loc` inside the `.fitzv` file. Precise
/// offset mapping (turning `pos` into a line + column INSIDE the
/// CSS blob) stays deferred — same debt as the other blob-parsers
/// in the view module. The context label names the component so
/// users know which style block to look at when a `.fitzv` file
/// has several components.
fn css_parse_error_to_expand(err: CssParseError, loc: Loc, component_name: &str) -> ExpandError {
    ExpandError {
        message: format!(
            "{} (char offset {} inside the CSS body)",
            err.message, err.pos
        ),
        loc,
        context: format!("component '{component_name}': <style scoped> block"),
    }
}

/// Derive the per-component scope class used to isolate scoped CSS
/// rules. Shape: `<component-kebab>-c-<8hex>`. The 8-hex suffix is
/// FNV-1a of `<component_name>::<css_raw>` truncated to the low 32
/// bits — deterministic for a given (name, css) pair, so the same
/// input always produces the same class. Two components with the
/// same name but different CSS bodies produce different classes,
/// so hot-reload during 11.3.d + 11.4 can invalidate stale styles
/// without name collisions.
fn synth_scope_class(component_name: &str, css_raw: &str) -> String {
    let kebab = to_kebab_case(component_name);
    let seed = format!("{component_name}::{css_raw}");
    let hex = fnv1a_hash_8_hex(&seed);
    format!("{kebab}-c-{hex}")
}

/// Convert a component name (typically CamelCase) to kebab-case
/// suitable for a CSS class. Insertion rule: a `-` goes before
/// every uppercase char that follows a lowercase or digit. `_`
/// becomes `-`. Non-ASCII-alphanumeric chars are dropped
/// (component names are validated by the shell parser to be Fitz
/// identifiers, so this only trips on hypothetical future
/// extensions). If the result would be empty, falls back to
/// `component` so the class is always non-empty.
fn to_kebab_case(name: &str) -> String {
    let mut out = String::new();
    let mut prev_was_lower_or_digit = false;
    for c in name.chars() {
        if c.is_ascii_uppercase() {
            if prev_was_lower_or_digit {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
            prev_was_lower_or_digit = false;
        } else if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            prev_was_lower_or_digit = true;
        } else if c == '_' {
            out.push('-');
            prev_was_lower_or_digit = false;
        }
        // Drop other chars silently.
    }
    if out.is_empty() {
        out.push_str("component");
    }
    out
}

/// FNV-1a 64-bit hash truncated to the low 32 bits, formatted as
/// 8 hex chars. Good enough for uniqueness within a single project
/// (32 bits = 4 billion permutations vs. typically <100 components
/// per file) and short enough to keep the generated class names
/// readable in DevTools. Not a cryptographic hash — collisions
/// under adversarial input are trivial to construct, but the
/// scope-class use case (per-component isolation) has no threat
/// model beyond accidental collisions between distinct CSS bodies.
fn fnv1a_hash_8_hex(input: &str) -> String {
    let mut hash: u64 = 14_695_981_039_346_656_037; // FNV offset basis
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1_099_511_628_211); // FNV prime
    }
    format!("{:08x}", (hash & 0xFFFF_FFFF) as u32)
}

/// Walk the expanded template and, for every element with a
/// static `class` attribute, add the suffixed variant of each
/// class token. Recurses through Element children, If then + else
/// children, and For children. Non-element nodes (Text,
/// Interpolation, Slot) pass through unchanged.
///
/// Handles interpolated `class` attributes (`class="{dynamic}"`)
/// as a documented limitation: they're left unchanged, so a fully
/// dynamic class binding won't pick up the scope suffix. Users who
/// want dynamic scoped classes will need to include the suffix
/// manually in the interpolation expression until a follow-up
/// wires this end-to-end. Recorded as deuda residual in
/// `docs/fase-11-plan.md` §9.k.
fn rewrite_class_attrs_in_template(nodes: &mut [ExpandedTemplateNode], scope_class: &str) {
    for node in nodes {
        match node {
            ExpandedTemplateNode::Element {
                attrs, children, ..
            } => {
                for attr in attrs.iter_mut() {
                    if let ExpandedAttr::Static { name, value, .. } = attr {
                        if name == "class" {
                            *value = rewrite_class_value(value, scope_class);
                        }
                    }
                }
                rewrite_class_attrs_in_template(children, scope_class);
            }
            ExpandedTemplateNode::If {
                children,
                else_children,
                ..
            } => {
                rewrite_class_attrs_in_template(children, scope_class);
                if let Some(nodes) = else_children {
                    rewrite_class_attrs_in_template(nodes, scope_class);
                }
            }
            ExpandedTemplateNode::For { children, .. } => {
                rewrite_class_attrs_in_template(children, scope_class);
            }
            ExpandedTemplateNode::Text(_)
            | ExpandedTemplateNode::Interpolation { .. }
            | ExpandedTemplateNode::Slot { .. }
            | ExpandedTemplateNode::ChildComponent { .. } => {}
        }
    }
}

/// Rewrite a `class="..."` value by appending the suffixed
/// variant of each original class token. `"card"` becomes
/// `"card card-<scope>"`; `"card title"` becomes
/// `"card title card-<scope> title-<scope>"`. Empty and
/// whitespace-only inputs are preserved as-is (nothing to
/// suffix). Tokens are split on ASCII whitespace and rejoined
/// with single spaces; user-written double spaces or tabs
/// normalise to single spaces, which matches how the browser
/// tokenises `class` anyway.
fn rewrite_class_value(original: &str, scope_class: &str) -> String {
    let tokens: Vec<&str> = original.split_ascii_whitespace().collect();
    if tokens.is_empty() {
        return original.to_string();
    }
    let mut out = String::with_capacity(original.len() + tokens.len() * (scope_class.len() + 2));
    // First emit the originals, then the suffixed variants. The
    // browser semantics don't care about class order, but keeping
    // originals-first keeps the intended styles from the user's
    // POV grouped in DevTools before the compiler's additions.
    let mut first = true;
    for tok in &tokens {
        if !first {
            out.push(' ');
        }
        out.push_str(tok);
        first = false;
    }
    for tok in &tokens {
        out.push(' ');
        out.push_str(tok);
        out.push('-');
        out.push_str(scope_class);
    }
    out
}

fn expand_state_field(f: &RawStateField, component_name: &str) -> ExpandResult<ExpandedStateField> {
    let ctx_type = format!(
        "component '{}': state field '{}' type",
        component_name, f.name
    );
    let ctx_default = format!(
        "component '{}': state field '{}' default",
        component_name, f.name
    );
    let type_expr = parse_type_at(&f.type_expr_raw, f.loc, ctx_type)?;
    let default = parse_expr_at(&f.default_expr_raw, f.loc, ctx_default)?;
    Ok(ExpandedStateField {
        name: f.name.clone(),
        type_expr,
        default,
        loc: f.loc,
    })
}

fn expand_event_handler(
    h: &RawEventHandler,
    component_name: &str,
) -> ExpandResult<ExpandedEventHandler> {
    let ctx_params = format!("component '{}': event '{}' params", component_name, h.name);
    let ctx_body = format!("component '{}': event '{}' body", component_name, h.name);
    let params = parse_params_at(&h.params_raw, h.loc, ctx_params)?;
    let body = parse_stmts_at(&h.body_raw, h.loc, ctx_body)?;
    Ok(ExpandedEventHandler {
        name: h.name.clone(),
        params,
        body,
        loc: h.loc,
    })
}

fn expand_template(t: &RawTemplate, component_name: &str) -> ExpandResult<ExpandedTemplate> {
    let roots = t
        .roots
        .iter()
        .map(|n| expand_template_node(n, component_name))
        .collect::<ExpandResult<Vec<_>>>()?;
    Ok(ExpandedTemplate { roots, loc: t.loc })
}

fn expand_template_node(
    node: &RawTemplateNode,
    component_name: &str,
) -> ExpandResult<ExpandedTemplateNode> {
    match node {
        RawTemplateNode::Text(s) => Ok(ExpandedTemplateNode::Text(s.clone())),
        RawTemplateNode::Interpolation { expr_raw, loc } => {
            let ctx = format!("component '{component_name}': template interpolation");
            let expr = parse_expr_at(expr_raw, *loc, ctx)?;
            Ok(ExpandedTemplateNode::Interpolation { expr, loc: *loc })
        }
        RawTemplateNode::Element {
            tag,
            attrs,
            children,
            self_closing,
            loc,
        } => {
            // Phase 11.5.d — a tag starting with an ASCII
            // uppercase letter is a child-component reference
            // (Vue/React convention: HTML tags are lowercase,
            // components are PascalCase). Route to a dedicated
            // expander that enforces the composition rules
            // (self-closing only, static props only, no events).
            if starts_with_ascii_uppercase(tag) {
                return expand_child_component(
                    tag,
                    attrs,
                    children,
                    *self_closing,
                    *loc,
                    component_name,
                );
            }
            let attrs = attrs
                .iter()
                .map(|a| expand_attr(a, component_name, tag))
                .collect::<ExpandResult<Vec<_>>>()?;
            let children = children
                .iter()
                .map(|n| expand_template_node(n, component_name))
                .collect::<ExpandResult<Vec<_>>>()?;
            Ok(ExpandedTemplateNode::Element {
                tag: tag.clone(),
                attrs,
                children,
                self_closing: *self_closing,
                loc: *loc,
            })
        }
        RawTemplateNode::If {
            cond_raw,
            children,
            else_children,
            loc,
        } => {
            let ctx = format!("component '{component_name}': template `{{#if}}` condition");
            let cond = parse_expr_at(cond_raw, *loc, ctx)?;
            let children = children
                .iter()
                .map(|n| expand_template_node(n, component_name))
                .collect::<ExpandResult<Vec<_>>>()?;
            let else_children = match else_children {
                Some(nodes) => Some(
                    nodes
                        .iter()
                        .map(|n| expand_template_node(n, component_name))
                        .collect::<ExpandResult<Vec<_>>>()?,
                ),
                None => None,
            };
            Ok(ExpandedTemplateNode::If {
                cond,
                children,
                else_children,
                loc: *loc,
            })
        }
        RawTemplateNode::For {
            var,
            iter_raw,
            children,
            loc,
        } => {
            let ctx =
                format!("component '{component_name}': template `{{#for {var} in ...}}` iter");
            let iter = parse_expr_at(iter_raw, *loc, ctx)?;
            let children = children
                .iter()
                .map(|n| expand_template_node(n, component_name))
                .collect::<ExpandResult<Vec<_>>>()?;
            Ok(ExpandedTemplateNode::For {
                var: var.clone(),
                iter,
                children,
                loc: *loc,
            })
        }
        RawTemplateNode::Slot {
            name,
            fallback,
            loc,
        } => {
            let fallback = fallback
                .iter()
                .map(|n| expand_template_node(n, component_name))
                .collect::<ExpandResult<Vec<_>>>()?;
            Ok(ExpandedTemplateNode::Slot {
                name: name.clone(),
                fallback,
                loc: *loc,
            })
        }
    }
}

/// True when `tag` starts with an ASCII uppercase letter. Used
/// by [`expand_template_node`] to route capitalised tags to the
/// child-component expander (Phase 11.5.d convention: HTML tags
/// are lowercase, components are PascalCase — same as Vue/React).
fn starts_with_ascii_uppercase(tag: &str) -> bool {
    tag.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

/// Phase 11.5.d — expand a `<Child prop="v" />` node into a
/// [`ExpandedTemplateNode::ChildComponent`]. Enforces the shape:
///
/// - **Self-closing only.** Fallback children (`<Child>...</Child>`)
///   need `<slot>` fill-in wiring, which is 11.6+ work. Rejected
///   with a targeted 11.6 pointer that names the tag.
/// - **Static props only.** Each attribute must be
///   [`RawAttr::Static`]. Dynamic props (`prop={expr}`) are
///   rejected with a 11.6 pointer.
/// - **No event attrs.** `@click="handler"` on a child component
///   would require plumbing an event upwards (parent-defined
///   handler) — rejected with a 11.6 pointer.
///
/// The child component's **existence** and each prop's
/// **type-compatibility** are validated later, in
/// `check.rs::check_child_components` — this expander doesn't
/// have access to the sibling components' state fields yet
/// (they live on the `ExpandedViewFile`, which the caller
/// assembles after each component is expanded).
fn expand_child_component(
    tag: &str,
    attrs: &[RawAttr],
    children: &[RawTemplateNode],
    _self_closing: bool,
    loc: Loc,
    component_name: &str,
) -> ExpandResult<ExpandedTemplateNode> {
    // Phase 11.7.d — a non-self-closing `<Child>...</Child>` carries slot
    // content that fills the child's `<slot />`. Expand it in the PARENT's
    // context so parent-state interpolation + events resolve normally.
    let slot_content = children
        .iter()
        .map(|n| expand_template_node(n, component_name))
        .collect::<ExpandResult<Vec<_>>>()?;

    let mut props = Vec::with_capacity(attrs.len());
    let mut key: Option<fast::Expr> = None;
    let mut events: Vec<ChildEventBinding> = Vec::new();
    for a in attrs {
        match a {
            // Phase 11.7.b R2b — `key="{expr}"` is a reserved
            // attribute, NOT a prop. It gives a `<Child />` inside a
            // `{#for}` a stable identity so the WASM emitter can reuse
            // the child instance (and its local state) across
            // re-renders. Parse the interpolated expression and stash
            // it in `key`; never push it into `props`.
            RawAttr::Interpolation {
                name,
                expr_raw,
                loc: aloc,
            } if name == "key" => {
                let ctx = format!("component template <{tag} /> key attribute");
                key = Some(parse_expr_at(expr_raw, *aloc, ctx)?);
            }
            RawAttr::Static {
                name, loc: aloc, ..
            } if name == "key" => {
                return Err(ExpandError {
                    message: format!(
                        "`key=\"...\"` on `<{tag} />` must be an interpolated \
                         expression (`key=\"{{x}}\"`), not a static string. The \
                         key gives the child a stable identity inside a `{{#for}}` \
                         loop and is typically the loop variable."
                    ),
                    loc: *aloc,
                    context: "template (child component composition)".to_string(),
                });
            }
            RawAttr::Static { name, value, loc } => {
                props.push(ChildComponentProp {
                    field_name: name.clone(),
                    raw_value: value.clone(),
                    expr: None,
                    loc: *loc,
                });
            }
            RawAttr::Interpolation {
                name,
                expr_raw,
                loc: aloc,
            } => {
                // K-3 remainder (post-v0.21.0): interpolated props
                // pass through to the emitters. The SSR emitter
                // runs the parsed expression through the standard
                // scoping pass (state-field refs get rewritten to
                // `state.<name>`) and inlines it in the struct
                // literal. The WASM emitter errors out today with
                // a Phase 11.7+ pointer — client-side dynamic
                // composition needs richer reactivity plumbing.
                //
                // `raw_value` mirrors the expression source so
                // error messages don't need to special-case the
                // shape; the discriminant is `expr.is_some()`.
                let ctx = format!("component template <{tag} /> prop '{name}' interpolation");
                let expr = parse_expr_at(expr_raw, *aloc, ctx)?;
                props.push(ChildComponentProp {
                    field_name: name.clone(),
                    raw_value: expr_raw.clone(),
                    expr: Some(expr),
                    loc: *aloc,
                });
            }
            // Phase 11.7.c — `@event="handler"` on a child component wires
            // event bubbling: when the child's `event_name` fires, the
            // parent's `handler_raw` runs. The child event's existence and
            // the parent handler's existence are validated in check.rs.
            RawAttr::Event {
                event_name,
                handler_raw,
                loc: aloc,
            } => {
                events.push(ChildEventBinding {
                    event_name: event_name.clone(),
                    handler_name: handler_raw.clone(),
                    loc: *aloc,
                });
            }
        }
    }

    Ok(ExpandedTemplateNode::ChildComponent {
        name: tag.to_string(),
        props,
        key,
        events,
        slot_content,
        loc,
    })
}

fn expand_attr(attr: &RawAttr, component_name: &str, tag: &str) -> ExpandResult<ExpandedAttr> {
    match attr {
        RawAttr::Static { name, value, loc } => Ok(ExpandedAttr::Static {
            name: name.clone(),
            value: value.clone(),
            loc: *loc,
        }),
        RawAttr::Interpolation {
            name,
            expr_raw,
            loc,
        } => {
            let ctx = format!("component '{component_name}': <{tag}> attr '{name}' interpolation");
            let expr = parse_expr_at(expr_raw, *loc, ctx)?;
            Ok(ExpandedAttr::Interpolation {
                name: name.clone(),
                expr,
                loc: *loc,
            })
        }
        RawAttr::Event {
            event_name,
            handler_raw,
            loc,
        } => {
            // The handler value must be a bare identifier. We
            // deliberately do not re-parse it as a classic expression
            // — the semantics of `@click="start"` is a *name* of an
            // event handler declared in the same component, not an
            // expression to evaluate.
            let name = handler_raw.trim();
            if name.is_empty() {
                return Err(ExpandError {
                    message: format!(
                        "attribute `@{event_name}` needs a handler name (bare identifier)"
                    ),
                    loc: *loc,
                    context: format!("component '{component_name}': <{tag}> @{event_name}"),
                });
            }
            if !is_bare_ident(name) {
                return Err(ExpandError {
                    message: format!(
                        "attribute `@{event_name}` must reference a handler by name — expressions are not supported yet (Phase 11.2 defers `@click=\"expr\"` support to later)"
                    ),
                    loc: *loc,
                    context: format!("component '{component_name}': <{tag}> @{event_name}"),
                });
            }
            Ok(ExpandedAttr::Event {
                event_name: event_name.clone(),
                handler_name: name.to_string(),
                loc: *loc,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_type_at(source: &str, base: Loc, context: String) -> ExpandResult<fast::TypeExpr> {
    // Empty raw usually means the field skipped the annotation. The
    // POC parser today requires an annotation, but be defensive.
    if source.trim().is_empty() {
        return Err(ExpandError {
            message: "missing type annotation".into(),
            loc: base,
            context,
        });
    }
    let shifted = with_shifted_tokens(source, base, context.clone())?;
    parse_type_expression_from_source(&shifted).map_err(|e| shift_error(e, base, context))
}

fn parse_expr_at(source: &str, base: Loc, context: String) -> ExpandResult<fast::Expr> {
    if source.trim().is_empty() {
        return Err(ExpandError {
            message: "missing expression".into(),
            loc: base,
            context,
        });
    }
    let shifted = with_shifted_tokens(source, base, context.clone())?;
    parse_expression_from_source(&shifted).map_err(|e| shift_error(e, base, context))
}

fn parse_stmts_at(source: &str, base: Loc, context: String) -> ExpandResult<Vec<fast::Stmt>> {
    // Empty body is legal: `event go() { }` handles nothing.
    let shifted = with_shifted_tokens(source, base, context.clone())?;
    parse_statements_from_source(&shifted).map_err(|e| shift_error(e, base, context))
}

fn parse_params_at(source: &str, base: Loc, context: String) -> ExpandResult<Vec<fast::Param>> {
    // Empty params is legal: `event go()` has no params.
    let shifted = with_shifted_tokens(source, base, context.clone())?;
    parse_parameters_from_source(&shifted).map_err(|e| shift_error(e, base, context))
}

/// Feed the raw source through the classic lexer, shift every token's
/// `(line, column)` into the enclosing `.fitzv` coord system, and
/// re-emit it as source that a subsequent `parse_*_from_source` call
/// will tokenize identically — with the same shifted positions
/// naturally coming out.
///
/// **Why re-emit instead of tokenize-then-hand-off**: the public
/// `parse_*_from_source` wrappers take `&str`, not a token vector.
/// We could expose a token-consuming variant, but for a first commit
/// the source-in / source-out shape is simpler; the double-tokenize
/// cost is negligible for blobs of the size a `.fitzv` typically has.
///
/// **Fallback**: today the tokens themselves are not consumed. The
/// helper returns `source` verbatim. Precise position shifting inside
/// spans lands when the POC parser starts tracking the blob's exact
/// start offset (documented as debt in `docs/fase-11-plan.md` §7).
fn with_shifted_tokens(source: &str, _base: Loc, _context: String) -> ExpandResult<String> {
    // Tokenize once to validate the blob is at least lexable. If the
    // classic lexer chokes here, we surface the error early.
    let _tokens: Vec<TokenWithPos> = match tokenize(source) {
        Ok(t) => t,
        Err(e) => {
            // Positions from `tokenize` are blob-local. Shift them
            // into `.fitzv` coord space so the user gets a sensible
            // error location.
            return Err(shift_error(e, _base, _context));
        }
    };
    Ok(source.to_string())
}

fn shift_error(err: FitzError, base: Loc, context: String) -> ExpandError {
    // Blob-local (1, 1) maps to (base.line, base.column). Subsequent
    // lines and columns follow the standard formula.
    let (blob_line, blob_col) = (err.line.max(1), err.column.max(1));
    let (abs_line, abs_col) = shift_position(blob_line, blob_col, base);
    ExpandError {
        message: err.message,
        loc: Loc::new(abs_line, abs_col),
        context,
    }
}

fn shift_position(blob_line: usize, blob_col: usize, base: Loc) -> (usize, usize) {
    // Standard formula for source spans lifted into an outer file:
    //   line 1 sits at base.line; column 1 sits at base.column.
    //   subsequent lines start at column 1 of the outer file too.
    let abs_line = base.line + blob_line - 1;
    let abs_col = if blob_line == 1 {
        base.column + blob_col - 1
    } else {
        blob_col
    };
    (abs_line, abs_col)
}

fn is_bare_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::parse as view_parse;

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
  </style>
}
"#;

    fn expand_str(src: &str) -> ExpandResult<ExpandedViewFile> {
        let raw = view_parse(src).expect("view parses");
        expand(&raw)
    }

    #[test]
    fn expands_the_card_component_end_to_end() {
        let file = expand_str(CARD_SRC).expect("Card expands cleanly");
        assert_eq!(file.components.len(), 1);
        let c = &file.components[0];
        assert_eq!(c.name, "Card");
        assert_eq!(c.state.len(), 2);
        assert_eq!(c.events.len(), 2);
        assert!(c.template.is_some());
        assert!(c.style.is_some());
    }

    #[test]
    fn state_field_types_are_parsed_as_type_exprs() {
        let file = expand_str(CARD_SRC).unwrap();
        let c = &file.components[0];
        assert_eq!(c.state[0].name, "title");
        assert_eq!(c.state[0].type_expr, fast::TypeExpr::Named("Str".into()));
        assert_eq!(c.state[1].name, "is_editing");
        assert_eq!(c.state[1].type_expr, fast::TypeExpr::Named("Bool".into()));
    }

    #[test]
    fn state_field_defaults_are_parsed_as_expressions() {
        let file = expand_str(CARD_SRC).unwrap();
        let c = &file.components[0];
        // `"Untitled"` — a plain string literal is an Expr::Str.
        matches!(c.state[0].default, fast::Expr::Str(_, _));
        if let fast::Expr::Str(s, _) = &c.state[0].default {
            assert_eq!(s, "Untitled");
        } else {
            panic!(
                "expected Expr::Str for title default, got {:?}",
                c.state[0].default
            );
        }
        // `false` — a Bool literal.
        if let fast::Expr::Bool(b, _) = c.state[1].default {
            assert!(!b);
        } else {
            panic!("expected Expr::Bool(false), got {:?}", c.state[1].default);
        }
    }

    #[test]
    fn event_handler_params_are_parsed_as_param_list() {
        let file = expand_str(CARD_SRC).unwrap();
        let c = &file.components[0];
        // start() — empty params.
        assert_eq!(c.events[0].name, "start");
        assert!(c.events[0].params.is_empty());
        // save(new_title: Str) — one annotated param.
        assert_eq!(c.events[1].name, "save");
        assert_eq!(c.events[1].params.len(), 1);
        let p = &c.events[1].params[0];
        assert_eq!(p.name, "new_title");
        assert_eq!(p.type_, Some(fast::TypeExpr::Named("Str".into())));
        assert!(!p.varargs);
    }

    #[test]
    fn event_handler_bodies_are_parsed_as_stmts() {
        let file = expand_str(CARD_SRC).unwrap();
        let c = &file.components[0];
        // start body: `is_editing = true` — a single Stmt::Assign.
        assert_eq!(c.events[0].body.len(), 1);
        assert!(matches!(c.events[0].body[0], fast::Stmt::Assign { .. }));
        // save body: two assignments.
        assert_eq!(c.events[1].body.len(), 2);
        assert!(matches!(c.events[1].body[0], fast::Stmt::Assign { .. }));
        assert!(matches!(c.events[1].body[1], fast::Stmt::Assign { .. }));
    }

    #[test]
    fn template_interpolation_is_parsed_as_expr() {
        let file = expand_str(CARD_SRC).unwrap();
        let template = file.components[0].template.as_ref().unwrap();
        // Find the interpolation node buried under the outer <div>.
        let outer_div = template
            .roots
            .iter()
            .find(|n| matches!(n, ExpandedTemplateNode::Element { tag, .. } if tag == "div"))
            .unwrap();
        let inner_children = match outer_div {
            ExpandedTemplateNode::Element { children, .. } => children,
            _ => unreachable!(),
        };
        let title_div = inner_children
            .iter()
            .find(|n| matches!(n, ExpandedTemplateNode::Element { tag, .. } if tag == "div"))
            .unwrap();
        let title_children = match title_div {
            ExpandedTemplateNode::Element { children, .. } => children,
            _ => unreachable!(),
        };
        let interp = title_children
            .iter()
            .find(|n| matches!(n, ExpandedTemplateNode::Interpolation { .. }))
            .expect("`{title}` interpolation exists");
        match interp {
            ExpandedTemplateNode::Interpolation { expr, .. } => match expr {
                fast::Expr::Ident(name, _) => assert_eq!(name, "title"),
                other => panic!("expected Ident, got {:?}", other),
            },
            _ => unreachable!(),
        }
    }

    #[test]
    fn attribute_event_binding_stores_handler_name_verbatim() {
        let file = expand_str(CARD_SRC).unwrap();
        let template = file.components[0].template.as_ref().unwrap();
        // Dig for the <button> element.
        fn find_button(nodes: &[ExpandedTemplateNode]) -> Option<&ExpandedTemplateNode> {
            for n in nodes {
                if let ExpandedTemplateNode::Element { tag, children, .. } = n {
                    if tag == "button" {
                        return Some(n);
                    }
                    if let Some(f) = find_button(children) {
                        return Some(f);
                    }
                }
            }
            None
        }
        let button = find_button(&template.roots).expect("button");
        match button {
            ExpandedTemplateNode::Element { attrs, .. } => {
                assert_eq!(attrs.len(), 1);
                match &attrs[0] {
                    ExpandedAttr::Event {
                        event_name,
                        handler_name,
                        ..
                    } => {
                        assert_eq!(event_name, "click");
                        assert_eq!(handler_name, "start");
                    }
                    other => panic!("expected Event attr, got {:?}", other),
                }
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn attribute_interpolation_is_parsed_as_expr() {
        let src = r#"component X {
  state { title: Str = "hi" }
  <template><input value="{title}" /></template>
}"#;
        let file = expand_str(src).unwrap();
        let input = &file.components[0].template.as_ref().unwrap().roots[0];
        match input {
            ExpandedTemplateNode::Element {
                attrs,
                self_closing,
                ..
            } => {
                assert!(*self_closing);
                match &attrs[0] {
                    ExpandedAttr::Interpolation { name, expr, .. } => {
                        assert_eq!(name, "value");
                        match expr {
                            fast::Expr::Ident(n, _) => assert_eq!(n, "title"),
                            other => panic!("expected Ident, got {:?}", other),
                        }
                    }
                    other => panic!("expected Interpolation attr, got {:?}", other),
                }
            }
            _ => panic!("expected <input/>"),
        }
    }

    #[test]
    fn state_field_default_that_fails_to_parse_reports_context() {
        // Two bare identifiers side by side: the view lexer emits
        // them as separate tokens (so the raw capture succeeds), but
        // `parse_expression_from_source` rejects with
        // "unexpected trailing tokens after expression".
        let src = r#"component X {
  state {
    count: Int = foo bar
  }
}"#;
        let err = expand_str(src).unwrap_err();
        assert!(
            err.context.contains("state field 'count' default"),
            "context = {:?}",
            err.context
        );
    }

    #[test]
    fn event_body_that_fails_to_parse_reports_context() {
        // Missing `=` after the identifier makes `parse_stmt` reject.
        let src = r#"component X {
  event go() {
    let x
  }
}"#;
        let err = expand_str(src).unwrap_err();
        assert!(
            err.context.contains("event 'go' body"),
            "context = {:?}",
            err.context
        );
    }

    #[test]
    fn event_binding_rejects_expression_value_with_clear_message() {
        let src = r#"component X {
  <template><button @click="start()">go</button></template>
}"#;
        let err = expand_str(src).unwrap_err();
        assert!(
            err.message.contains("reference a handler by name")
                || err.message.contains("handler name"),
            "message = {:?}",
            err.message
        );
    }

    #[test]
    fn empty_component_expands_to_empty_expanded() {
        let file = expand_str("component Empty {}").unwrap();
        assert_eq!(file.components.len(), 1);
        let c = &file.components[0];
        assert_eq!(c.name, "Empty");
        assert!(c.state.is_empty());
        assert!(c.events.is_empty());
        assert!(c.template.is_none());
        assert!(c.style.is_none());
    }

    #[test]
    fn multi_component_file_expands_each_component() {
        let src = r#"component A {
  state { flag: Bool = true }
}

component B {
  state { title: Str = "hello" }
}
"#;
        let file = expand_str(src).unwrap();
        assert_eq!(file.components.len(), 2);
        assert_eq!(file.components[0].name, "A");
        assert_eq!(file.components[1].name, "B");
        assert_eq!(file.components[0].state.len(), 1);
        assert_eq!(file.components[1].state.len(), 1);
    }

    #[test]
    fn shift_position_maps_line_one_col_one_to_base() {
        let base = Loc::new(10, 5);
        assert_eq!(shift_position(1, 1, base), (10, 5));
    }

    #[test]
    fn shift_position_shifts_column_only_on_line_one() {
        let base = Loc::new(10, 5);
        assert_eq!(shift_position(1, 3, base), (10, 7));
    }

    #[test]
    fn shift_position_shifts_line_and_keeps_column_on_later_lines() {
        let base = Loc::new(10, 5);
        assert_eq!(shift_position(3, 8, base), (12, 8));
    }

    // ---- 11.2.c mini-commit 1: `{#if cond}...{/if}` expand ---------

    #[test]
    fn expand_if_block_parses_cond_as_classic_expr() {
        // `{#if is_ready}` — the raw `is_ready` must expand into an
        // `Expr::Ident("is_ready")`.
        let src = r#"component X {
  state { is_ready: Bool = false }
  <template>{#if is_ready}<div>hi</div>{/if}</template>
}"#;
        let file = expand_str(src).unwrap();
        let template = file.components[0].template.as_ref().unwrap();
        match &template.roots[0] {
            ExpandedTemplateNode::If { cond, children, .. } => {
                match cond {
                    fast::Expr::Ident(name, _) => assert_eq!(name, "is_ready"),
                    other => panic!("expected Ident cond, got {:?}", other),
                }
                assert_eq!(children.len(), 1);
                match &children[0] {
                    ExpandedTemplateNode::Element { tag, .. } => assert_eq!(tag, "div"),
                    other => panic!("expected Element child, got {:?}", other),
                }
            }
            other => panic!("expected If root, got {:?}", other),
        }
    }

    #[test]
    fn expand_if_block_cond_with_binop_parses() {
        // `{#if count > 0}` — cond is a BinOp expression.
        let src = r#"component X {
  state { count: Int = 0 }
  <template>{#if count > 0}<span/>{/if}</template>
}"#;
        let file = expand_str(src).unwrap();
        let template = file.components[0].template.as_ref().unwrap();
        match &template.roots[0] {
            ExpandedTemplateNode::If { cond, .. } => {
                // Just assert it's not an Ident — the exact shape of
                // BinOp is a classic-parser detail; the point here is
                // that a non-trivial cond parses without error.
                assert!(
                    !matches!(cond, fast::Expr::Ident(_, _)),
                    "expected BinOp-like expr, got {:?}",
                    cond
                );
            }
            other => panic!("expected If, got {:?}", other),
        }
    }

    #[test]
    fn expand_if_block_bad_cond_syntax_produces_expand_error() {
        // `{#if 1 +}` — malformed expression. The classic parser
        // errors; expand shifts it into an ExpandError with the
        // `{#if}` block's loc + context label.
        let src = r#"component X {
  <template>{#if 1 +}<span/>{/if}</template>
}"#;
        let err = expand_str(src).unwrap_err();
        assert!(
            err.context.contains("`{#if}`") && err.context.contains("condition"),
            "context = {:?}",
            err.context
        );
    }

    #[test]
    fn expand_if_block_children_expanded_recursively() {
        // Interpolation `{title}` inside the If children must expand
        // into an `ExpandedTemplateNode::Interpolation` — proves the
        // recursion into children uses the same expand path.
        let src = r#"component X {
  state {
    is_ready: Bool = false
    title: Str = ""
  }
  <template>{#if is_ready}<div>{title}</div>{/if}</template>
}"#;
        let file = expand_str(src).unwrap();
        let template = file.components[0].template.as_ref().unwrap();
        match &template.roots[0] {
            ExpandedTemplateNode::If { children, .. } => match &children[0] {
                ExpandedTemplateNode::Element {
                    children: inner, ..
                } => match &inner[0] {
                    ExpandedTemplateNode::Interpolation { expr, .. } => match expr {
                        fast::Expr::Ident(name, _) => assert_eq!(name, "title"),
                        other => panic!("expected Ident interp, got {:?}", other),
                    },
                    other => panic!("expected Interpolation, got {:?}", other),
                },
                other => panic!("expected Element, got {:?}", other),
            },
            other => panic!("expected If, got {:?}", other),
        }
    }

    // ---- 11.2.c mini-commit 2: `{#for}` expand tests -----------------

    #[test]
    fn expand_for_block_parses_iter_as_classic_expr() {
        // `{#for x in xs}` — the raw `xs` must expand into an
        // `Expr::Ident("xs")`.
        let src = r#"component X {
  state { xs: List<Str> = [] }
  <template>{#for x in xs}<li>{x}</li>{/for}</template>
}"#;
        let file = expand_str(src).unwrap();
        let template = file.components[0].template.as_ref().unwrap();
        match &template.roots[0] {
            ExpandedTemplateNode::For {
                var,
                iter,
                children,
                ..
            } => {
                assert_eq!(var, "x");
                match iter {
                    fast::Expr::Ident(name, _) => assert_eq!(name, "xs"),
                    other => panic!("expected Ident iter, got {:?}", other),
                }
                assert_eq!(children.len(), 1);
            }
            other => panic!("expected For root, got {:?}", other),
        }
    }

    #[test]
    fn expand_for_block_iter_with_method_chain_parses() {
        // `{#for u in users.filter(fn(u) => u.active)}` — iter is a
        // method call chain with a closure inside; just assert
        // parsing succeeds and it's not a bare Ident.
        let src = r#"component X {
  state { users: List<Str> = [] }
  <template>{#for u in users.filter(fn(u) => len(u) > 0)}<li/>{/for}</template>
}"#;
        let file = expand_str(src).unwrap();
        let template = file.components[0].template.as_ref().unwrap();
        match &template.roots[0] {
            ExpandedTemplateNode::For { iter, .. } => {
                assert!(
                    !matches!(iter, fast::Expr::Ident(_, _)),
                    "expected method-chain expr, got {:?}",
                    iter
                );
            }
            other => panic!("expected For, got {:?}", other),
        }
    }

    #[test]
    fn expand_for_block_bad_iter_syntax_produces_expand_error() {
        // `{#for x in 1 +}` — malformed iter. Classic parser errors,
        // expand shifts it into an ExpandError citing the `{#for}`
        // block's loc + context label.
        let src = r#"component X {
  <template>{#for x in 1 +}<span/>{/for}</template>
}"#;
        let err = expand_str(src).unwrap_err();
        assert!(
            err.context.contains("`{#for") && err.context.contains("iter"),
            "context = {:?}",
            err.context
        );
    }

    #[test]
    fn expand_for_block_children_expanded_recursively() {
        // `{x}` inside the For children must expand into an
        // `ExpandedTemplateNode::Interpolation` — proves recursion.
        let src = r#"component X {
  state { xs: List<Str> = [] }
  <template>{#for x in xs}<li>{x}</li>{/for}</template>
}"#;
        let file = expand_str(src).unwrap();
        let template = file.components[0].template.as_ref().unwrap();
        match &template.roots[0] {
            ExpandedTemplateNode::For { children, .. } => match &children[0] {
                ExpandedTemplateNode::Element {
                    children: inner, ..
                } => match &inner[0] {
                    ExpandedTemplateNode::Interpolation { expr, .. } => match expr {
                        fast::Expr::Ident(name, _) => assert_eq!(name, "x"),
                        other => panic!("expected Ident interp, got {:?}", other),
                    },
                    other => panic!("expected Interpolation, got {:?}", other),
                },
                other => panic!("expected Element, got {:?}", other),
            },
            other => panic!("expected For, got {:?}", other),
        }
    }

    // ---- 11.2.c mini-commit 3: `<slot />` + `{#else}` expand tests ---

    #[test]
    fn expand_slot_without_name_becomes_expanded_slot_with_none() {
        // `<slot />` — carries no name.
        let src = r#"component X {
  <template><slot /></template>
}"#;
        let file = expand_str(src).unwrap();
        match &file.components[0].template.as_ref().unwrap().roots[0] {
            ExpandedTemplateNode::Slot { name, .. } => assert!(name.is_none()),
            other => panic!("expected Slot, got {:?}", other),
        }
    }

    #[test]
    fn expand_slot_with_name_preserves_the_slot_name() {
        // `<slot name="header" />` — captures "header".
        let src = r#"component X {
  <template><slot name="header" /></template>
}"#;
        let file = expand_str(src).unwrap();
        match &file.components[0].template.as_ref().unwrap().roots[0] {
            ExpandedTemplateNode::Slot { name, .. } => {
                assert_eq!(name.as_deref(), Some("header"));
            }
            other => panic!("expected Slot, got {:?}", other),
        }
    }

    #[test]
    fn expand_if_else_both_branches_expand_children_recursively() {
        // Interpolations in BOTH the then and the else branch must
        // expand into `ExpandedTemplateNode::Interpolation` — proves
        // the else recursion goes through the same expand path.
        let src = r#"component X {
  state {
    is_on: Bool = false
    on_label: Str = "on"
    off_label: Str = "off"
  }
  <template>{#if is_on}<span>{on_label}</span>{#else}<span>{off_label}</span>{/if}</template>
}"#;
        let file = expand_str(src).unwrap();
        match &file.components[0].template.as_ref().unwrap().roots[0] {
            ExpandedTemplateNode::If {
                children,
                else_children,
                ..
            } => {
                // then branch interpolation
                match &children[0] {
                    ExpandedTemplateNode::Element { children: c, .. } => match &c[0] {
                        ExpandedTemplateNode::Interpolation { expr, .. } => match expr {
                            fast::Expr::Ident(n, _) => assert_eq!(n, "on_label"),
                            other => panic!("expected Ident interp, got {:?}", other),
                        },
                        other => panic!("expected Interpolation in then, got {:?}", other),
                    },
                    other => panic!("expected Element in then, got {:?}", other),
                }
                // else branch interpolation
                let else_kids = else_children.as_ref().expect("else present");
                match &else_kids[0] {
                    ExpandedTemplateNode::Element { children: c, .. } => match &c[0] {
                        ExpandedTemplateNode::Interpolation { expr, .. } => match expr {
                            fast::Expr::Ident(n, _) => assert_eq!(n, "off_label"),
                            other => panic!("expected Ident interp, got {:?}", other),
                        },
                        other => panic!("expected Interpolation in else, got {:?}", other),
                    },
                    other => panic!("expected Element in else, got {:?}", other),
                }
            }
            other => panic!("expected If, got {:?}", other),
        }
    }

    #[test]
    fn expand_if_without_else_leaves_else_children_as_none() {
        // Regression: existing `{#if}...{/if}` (no else) still
        // expands and `else_children` is `None`.
        let src = r#"component X {
  state { is_on: Bool = false }
  <template>{#if is_on}<span/>{/if}</template>
}"#;
        let file = expand_str(src).unwrap();
        match &file.components[0].template.as_ref().unwrap().roots[0] {
            ExpandedTemplateNode::If { else_children, .. } => assert!(else_children.is_none()),
            other => panic!("expected If, got {:?}", other),
        }
    }

    #[test]
    fn expand_if_else_with_bad_expr_inside_else_reports_context() {
        // A malformed interpolation `{1 +}` inside the else branch
        // must still be reported — proves the recursive expand
        // walks into else_children.
        let src = r#"component X {
  state { is_on: Bool = false }
  <template>{#if is_on}<span>ok</span>{#else}<span>{1 +}</span>{/if}</template>
}"#;
        let err = expand_str(src).unwrap_err();
        assert!(
            err.context.contains("template interpolation"),
            "context = {:?}",
            err.context
        );
    }

    // -----------------------------------------------------------------
    // 11.3.c — style scoping + template class-attr rewrite
    // -----------------------------------------------------------------

    fn find_element<'a>(
        nodes: &'a [ExpandedTemplateNode],
        tag: &str,
    ) -> Option<&'a ExpandedTemplateNode> {
        for n in nodes {
            match n {
                ExpandedTemplateNode::Element {
                    tag: t, children, ..
                } => {
                    if t == tag {
                        return Some(n);
                    }
                    if let Some(nested) = find_element(children, tag) {
                        return Some(nested);
                    }
                }
                ExpandedTemplateNode::If {
                    children,
                    else_children,
                    ..
                } => {
                    if let Some(nested) = find_element(children, tag) {
                        return Some(nested);
                    }
                    if let Some(nodes) = else_children {
                        if let Some(nested) = find_element(nodes, tag) {
                            return Some(nested);
                        }
                    }
                }
                ExpandedTemplateNode::For { children, .. } => {
                    if let Some(nested) = find_element(children, tag) {
                        return Some(nested);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn class_of(node: &ExpandedTemplateNode) -> Option<&str> {
        if let ExpandedTemplateNode::Element { attrs, .. } = node {
            for a in attrs {
                if let ExpandedAttr::Static { name, value, .. } = a {
                    if name == "class" {
                        return Some(value);
                    }
                }
            }
        }
        None
    }

    #[test]
    fn scoped_style_expands_to_expanded_style_scoped_with_suffixed_css() {
        // The CARD_SRC fixture uses `<style scoped>`. After 11.3.c
        // it should expand to `ExpandedStyle::Scoped { css_scoped,
        // scope_class, .. }` where `css_scoped` carries the
        // `.card-<scope>` transformation applied by `apply_scope`.
        let file = expand_str(CARD_SRC).unwrap();
        let c = &file.components[0];
        match c.style.as_ref().unwrap() {
            ExpandedStyle::Scoped {
                css_scoped,
                scope_class,
                ..
            } => {
                let expected_selector = format!(".card-{scope_class}");
                assert!(
                    css_scoped.contains(&expected_selector),
                    "css_scoped = {css_scoped:?} should contain {expected_selector:?}"
                );
                assert!(
                    scope_class.starts_with("card-c-"),
                    "scope_class = {scope_class:?} should be `card-c-<8hex>`"
                );
                // 8 hex chars follow `card-c-`; total = 7 (`card-c-`) + 8 = 15.
                assert_eq!(scope_class.len(), 15);
            }
            other => panic!("expected Scoped variant, got {other:?}"),
        }
    }

    #[test]
    fn global_style_expands_to_expanded_style_global_verbatim() {
        let src = r#"component X {
  <template><div>hi</div></template>
  <style global>body { margin: 0; }</style>
}"#;
        let file = expand_str(src).unwrap();
        let c = &file.components[0];
        match c.style.as_ref().unwrap() {
            ExpandedStyle::Global { css, .. } => {
                assert_eq!(css.trim(), "body { margin: 0; }");
            }
            other => panic!("expected Global variant, got {other:?}"),
        }
    }

    #[test]
    fn no_style_block_produces_none_style() {
        let src = r#"component X {
  <template><div>hi</div></template>
}"#;
        let file = expand_str(src).unwrap();
        assert!(file.components[0].style.is_none());
    }

    #[test]
    fn scope_class_is_deterministic_for_same_name_and_css() {
        // Two runs of the exact same source must produce the exact
        // same scope class. This is what makes hot-reload safe:
        // unchanged input → unchanged output, no spurious
        // invalidation.
        let file1 = expand_str(CARD_SRC).unwrap();
        let file2 = expand_str(CARD_SRC).unwrap();
        let s1 = match &file1.components[0].style {
            Some(ExpandedStyle::Scoped { scope_class, .. }) => scope_class.clone(),
            _ => panic!("expected Scoped"),
        };
        let s2 = match &file2.components[0].style {
            Some(ExpandedStyle::Scoped { scope_class, .. }) => scope_class.clone(),
            _ => panic!("expected Scoped"),
        };
        assert_eq!(s1, s2);
    }

    #[test]
    fn scope_class_differs_when_css_body_differs() {
        // Same component name, different CSS body → different
        // scope class. This means editing the CSS in a hot-reload
        // scenario invalidates the stale rules cleanly.
        let src_a = r#"component X {
  <template><div class="c">hi</div></template>
  <style scoped>.c { color: red; }</style>
}"#;
        let src_b = r#"component X {
  <template><div class="c">hi</div></template>
  <style scoped>.c { color: blue; }</style>
}"#;
        let a = expand_str(src_a).unwrap();
        let b = expand_str(src_b).unwrap();
        let sa = match &a.components[0].style {
            Some(ExpandedStyle::Scoped { scope_class, .. }) => scope_class.clone(),
            _ => panic!("expected Scoped"),
        };
        let sb = match &b.components[0].style {
            Some(ExpandedStyle::Scoped { scope_class, .. }) => scope_class.clone(),
            _ => panic!("expected Scoped"),
        };
        assert_ne!(sa, sb);
    }

    #[test]
    fn scope_class_differs_when_component_name_differs() {
        // Same CSS body, different component name → different
        // scope class. Prevents cross-component collisions on
        // identical CSS.
        let src_a = r#"component A {
  <template><div class="c">hi</div></template>
  <style scoped>.c { color: red; }</style>
}"#;
        let src_b = r#"component B {
  <template><div class="c">hi</div></template>
  <style scoped>.c { color: red; }</style>
}"#;
        let a = expand_str(src_a).unwrap();
        let b = expand_str(src_b).unwrap();
        let sa = match &a.components[0].style {
            Some(ExpandedStyle::Scoped { scope_class, .. }) => scope_class.clone(),
            _ => panic!("expected Scoped"),
        };
        let sb = match &b.components[0].style {
            Some(ExpandedStyle::Scoped { scope_class, .. }) => scope_class.clone(),
            _ => panic!("expected Scoped"),
        };
        assert_ne!(sa, sb);
    }

    #[test]
    fn to_kebab_case_camelcase_component_name() {
        // Internal helper — verified via a component named
        // `LoginForm`. The scope class should start with
        // `login-form-c-`.
        let src = r#"component LoginForm {
  <template><div class="root">hi</div></template>
  <style scoped>.root { color: red; }</style>
}"#;
        let file = expand_str(src).unwrap();
        match &file.components[0].style {
            Some(ExpandedStyle::Scoped { scope_class, .. }) => {
                assert!(
                    scope_class.starts_with("login-form-c-"),
                    "scope_class = {scope_class:?}"
                );
            }
            _ => panic!("expected Scoped"),
        }
    }

    #[test]
    fn to_kebab_case_all_lowercase_component_name() {
        // A component named `card` (all lowercase) stays as-is —
        // no dashes injected mid-word.
        let src = r#"component card {
  <template><div class="c">hi</div></template>
  <style scoped>.c { color: red; }</style>
}"#;
        let file = expand_str(src).unwrap();
        match &file.components[0].style {
            Some(ExpandedStyle::Scoped { scope_class, .. }) => {
                assert!(
                    scope_class.starts_with("card-c-"),
                    "scope_class = {scope_class:?}"
                );
                // The name is `card` so the prefix before `-c-` is
                // exactly `card` (4 chars) — no extra dashes.
                assert_eq!(scope_class.len(), 15);
            }
            _ => panic!("expected Scoped"),
        }
    }

    #[test]
    fn element_with_static_class_gets_suffixed_variant_added() {
        let src = r#"component Card {
  <template><div class="card"><p>hi</p></div></template>
  <style scoped>.card { color: red; }</style>
}"#;
        let file = expand_str(src).unwrap();
        let c = &file.components[0];
        let scope = match c.style.as_ref().unwrap() {
            ExpandedStyle::Scoped { scope_class, .. } => scope_class.clone(),
            _ => unreachable!(),
        };
        let div = find_element(&c.template.as_ref().unwrap().roots, "div").unwrap();
        let cls = class_of(div).unwrap();
        // Original class kept first, suffixed variant appended.
        assert_eq!(cls, format!("card card-{scope}"));
    }

    #[test]
    fn element_with_multiple_static_classes_gets_each_suffixed() {
        let src = r#"component X {
  <template><div class="card title">hi</div></template>
  <style scoped>.card { color: red; }</style>
}"#;
        let file = expand_str(src).unwrap();
        let c = &file.components[0];
        let scope = match c.style.as_ref().unwrap() {
            ExpandedStyle::Scoped { scope_class, .. } => scope_class.clone(),
            _ => unreachable!(),
        };
        let div = find_element(&c.template.as_ref().unwrap().roots, "div").unwrap();
        let cls = class_of(div).unwrap();
        // Both original class tokens preserved first, then each
        // suffixed variant. Order: originals in input order,
        // suffixed in input order.
        assert_eq!(cls, format!("card title card-{scope} title-{scope}"));
    }

    #[test]
    fn element_without_class_attribute_is_left_alone() {
        // A `<div>` without `class` doesn't get one injected — the
        // MVP class-suffix strategy only scopes rules that target
        // classes. Type-selector-only rules (`div { ... }`) don't
        // scope; users opt into scoping by targeting classes.
        let src = r#"component X {
  <template><div><p class="inner">hi</p></div></template>
  <style scoped>.inner { color: red; }</style>
}"#;
        let file = expand_str(src).unwrap();
        let c = &file.components[0];
        let div = find_element(&c.template.as_ref().unwrap().roots, "div").unwrap();
        // No class on the div — attr list stays empty.
        if let ExpandedTemplateNode::Element { attrs, .. } = div {
            assert!(attrs.is_empty(), "div attrs = {attrs:?}");
        }
        // The `<p>` inside DOES have a class and gets scoped.
        let p = find_element(&c.template.as_ref().unwrap().roots, "p").unwrap();
        let cls = class_of(p).unwrap();
        assert!(cls.starts_with("inner "));
        assert!(cls.contains(" inner-"));
    }

    #[test]
    fn element_with_interpolated_class_is_not_transformed() {
        // Documented limitation: fully-interpolated `class="{dyn}"`
        // stays as an `ExpandedAttr::Interpolation` and the
        // rewrite skips it. Users who want scoped dynamic classes
        // need to include the suffix manually in the interpolation
        // until a follow-up wires this end-to-end.
        let src = r#"component X {
  state { theme: Str = "light" }
  <template><div class="{theme}">hi</div></template>
  <style scoped>.light { color: red; }</style>
}"#;
        let file = expand_str(src).unwrap();
        let c = &file.components[0];
        let div = find_element(&c.template.as_ref().unwrap().roots, "div").unwrap();
        // The attr is still an Interpolation, not rewritten to a
        // Static with concatenated scope class.
        if let ExpandedTemplateNode::Element { attrs, .. } = div {
            let has_interp = attrs
                .iter()
                .any(|a| matches!(a, ExpandedAttr::Interpolation { name, .. } if name == "class"));
            assert!(has_interp, "expected class Interpolation, got {attrs:?}");
        }
    }

    #[test]
    fn class_rewrite_recurses_into_element_children() {
        let src = r#"component X {
  <template>
    <div class="outer">
      <section class="middle">
        <span class="inner">hi</span>
      </section>
    </div>
  </template>
  <style scoped>.outer { color: red; }</style>
}"#;
        let file = expand_str(src).unwrap();
        let c = &file.components[0];
        let scope = match c.style.as_ref().unwrap() {
            ExpandedStyle::Scoped { scope_class, .. } => scope_class.clone(),
            _ => unreachable!(),
        };
        for tag in &["div", "section", "span"] {
            let node = find_element(&c.template.as_ref().unwrap().roots, tag).unwrap();
            let cls = class_of(node).unwrap_or_else(|| panic!("no class on <{tag}>"));
            assert!(
                cls.contains(&format!("-{scope}")),
                "tag <{tag}> class = {cls:?} lacks scope suffix -{scope}"
            );
        }
    }

    #[test]
    fn class_rewrite_recurses_into_if_then_and_else_branches() {
        let src = r#"component X {
  state { on: Bool = true }
  <template>
    {#if on}
      <span class="a">on</span>
    {#else}
      <span class="b">off</span>
    {/if}
  </template>
  <style scoped>.a { color: red; }</style>
}"#;
        let file = expand_str(src).unwrap();
        let c = &file.components[0];
        let scope = match c.style.as_ref().unwrap() {
            ExpandedStyle::Scoped { scope_class, .. } => scope_class.clone(),
            _ => unreachable!(),
        };
        // The `find_element` helper walks into If then/else, so
        // both branches should have their `<span>` scoped. We only
        // find the FIRST match (the then-branch); verify it via
        // the tree structure to make sure else also scoped.
        let roots = &c.template.as_ref().unwrap().roots;
        // Locate the If node.
        let if_node = roots
            .iter()
            .find(|n| matches!(n, ExpandedTemplateNode::If { .. }))
            .expect("if node");
        if let ExpandedTemplateNode::If {
            children,
            else_children,
            ..
        } = if_node
        {
            let then_span = find_element(children, "span").unwrap();
            assert_eq!(class_of(then_span).unwrap(), format!("a a-{scope}"));
            let else_nodes = else_children.as_ref().expect("else present");
            let else_span = find_element(else_nodes, "span").unwrap();
            assert_eq!(class_of(else_span).unwrap(), format!("b b-{scope}"));
        }
    }

    #[test]
    fn class_rewrite_recurses_into_for_body() {
        let src = r#"component X {
  state { xs: List<Str> = ["a", "b"] }
  <template>
    {#for x in xs}
      <li class="item">{x}</li>
    {/for}
  </template>
  <style scoped>.item { color: red; }</style>
}"#;
        let file = expand_str(src).unwrap();
        let c = &file.components[0];
        let scope = match c.style.as_ref().unwrap() {
            ExpandedStyle::Scoped { scope_class, .. } => scope_class.clone(),
            _ => unreachable!(),
        };
        let li = find_element(&c.template.as_ref().unwrap().roots, "li").unwrap();
        assert_eq!(class_of(li).unwrap(), format!("item item-{scope}"));
    }

    #[test]
    fn global_style_does_not_trigger_template_class_rewrite() {
        // `<style global>` must NOT rewrite template classes. The
        // whole point of `global` is "no scoping" — templates stay
        // exactly as the user wrote them.
        let src = r#"component X {
  <template><div class="card">hi</div></template>
  <style global>.card { color: red; }</style>
}"#;
        let file = expand_str(src).unwrap();
        let c = &file.components[0];
        let div = find_element(&c.template.as_ref().unwrap().roots, "div").unwrap();
        assert_eq!(class_of(div).unwrap(), "card");
        // And the style itself is Global with CSS verbatim.
        match c.style.as_ref().unwrap() {
            ExpandedStyle::Global { css, .. } => {
                assert_eq!(css.trim(), ".card { color: red; }");
            }
            _ => panic!("expected Global"),
        }
    }

    #[test]
    fn no_style_block_leaves_template_untouched() {
        // No style at all — class attrs stay as the user wrote
        // them.
        let src = r#"component X {
  <template><div class="card">hi</div></template>
}"#;
        let file = expand_str(src).unwrap();
        let c = &file.components[0];
        let div = find_element(&c.template.as_ref().unwrap().roots, "div").unwrap();
        assert_eq!(class_of(div).unwrap(), "card");
    }

    #[test]
    fn malformed_scoped_css_produces_expand_error_with_component_context() {
        // Unterminated rule body inside the scoped CSS. The
        // `apply_scope` error is remapped into an ExpandError
        // whose `context` names the offending component.
        let src = r#"component MyCard {
  <template><div class="a">hi</div></template>
  <style scoped>.a { color: red;</style>
}"#;
        let err = expand_str(src).unwrap_err();
        assert!(
            err.context.contains("component 'MyCard'"),
            "context = {:?}",
            err.context
        );
        assert!(
            err.context.contains("<style scoped>"),
            "context = {:?}",
            err.context
        );
        assert!(
            err.message.contains("unterminated"),
            "message = {:?}",
            err.message
        );
    }

    #[test]
    fn scoped_style_with_no_template_still_expands_style() {
        // A component that has a `<style scoped>` but no
        // `<template>` — the style still expands to Scoped (with
        // the CSS transformed and a scope_class synthesised); the
        // template rewrite is a no-op because there's no template.
        let src = r#"component X {
  <style scoped>.card { color: red; }</style>
}"#;
        let file = expand_str(src).unwrap();
        let c = &file.components[0];
        assert!(c.template.is_none());
        match c.style.as_ref().unwrap() {
            ExpandedStyle::Scoped {
                css_scoped,
                scope_class,
                ..
            } => {
                let expected = format!(".card-{scope_class}");
                assert!(css_scoped.contains(&expected));
            }
            _ => panic!("expected Scoped"),
        }
    }

    #[test]
    fn rewrite_class_value_helper_normalises_whitespace() {
        // Direct unit test on the free helper — user-written
        // double spaces + tabs in `class="foo  bar"` become
        // single-spaced in the output.
        let out = rewrite_class_value("foo  bar\t baz", "s");
        assert_eq!(out, "foo bar baz foo-s bar-s baz-s");
    }

    #[test]
    fn rewrite_class_value_helper_leaves_empty_and_whitespace_only_alone() {
        assert_eq!(rewrite_class_value("", "s"), "");
        assert_eq!(rewrite_class_value("   ", "s"), "   ");
    }

    // ---- Phase 11.5.d — ChildComponent expansion ----

    fn expand_ok_test(src: &str) -> ExpandedViewFile {
        let raw = super::super::parse(src).expect("view::parse");
        super::expand(&raw).expect("view::expand")
    }

    fn find_child(node: &ExpandedTemplateNode) -> Option<(&str, &[ChildComponentProp])> {
        match node {
            ExpandedTemplateNode::ChildComponent { name, props, .. } => Some((name, props)),
            ExpandedTemplateNode::Element { children, .. } => {
                for c in children {
                    if let Some(x) = find_child(c) {
                        return Some(x);
                    }
                }
                None
            }
            _ => None,
        }
    }

    #[test]
    fn phase_11_5_d_expand_capitalised_tag_emits_child_component_variant() {
        let src = r#"component Parent {
  state {}
  <template><Card /></template>
}
component Card {
  state { title: Str = "hi" }
  <template><div>{title}</div></template>
}
"#;
        let file = expand_ok_test(src);
        let parent = &file.components[0];
        let tpl = parent.template.as_ref().unwrap();
        let (name, props) = find_child(&tpl.roots[0]).expect("ChildComponent not found");
        assert_eq!(name, "Card");
        assert!(props.is_empty(), "self-closing without attrs => no props");
    }

    #[test]
    fn phase_11_5_d_expand_child_component_captures_static_props() {
        let src = r#"component Parent {
  state {}
  <template><Card title="Hello" count="3" /></template>
}
component Card {
  state {
    title: Str = "x"
    count: Int = 0
  }
  <template><div>{title}</div></template>
}
"#;
        let file = expand_ok_test(src);
        let parent = &file.components[0];
        let tpl = parent.template.as_ref().unwrap();
        let (_, props) = find_child(&tpl.roots[0]).expect("ChildComponent not found");
        assert_eq!(props.len(), 2);
        assert_eq!(props[0].field_name, "title");
        assert_eq!(props[0].raw_value, "Hello");
        assert_eq!(props[1].field_name, "count");
        assert_eq!(props[1].raw_value, "3");
    }

    #[test]
    fn phase_11_7_d_expand_child_component_non_self_closing_captures_slot_content() {
        // Phase 11.7.d — a non-self-closing `<Card>...</Card>` now captures
        // its children as slot content instead of rejecting.
        let src = r#"component Parent {
  state {}
  <template><Card>hello</Card></template>
}
"#;
        let file = expand_ok_test(src);
        let tpl = file.components[0].template.as_ref().unwrap();
        match &tpl.roots[0] {
            ExpandedTemplateNode::ChildComponent {
                name, slot_content, ..
            } => {
                assert_eq!(name, "Card");
                assert_eq!(slot_content.len(), 1);
                assert!(matches!(
                    &slot_content[0],
                    ExpandedTemplateNode::Text(t) if t.contains("hello")
                ));
            }
            other => panic!("expected ChildComponent, got {other:?}"),
        }
    }

    #[test]
    fn k3_interp_expand_child_component_accepts_dynamic_prop_and_parses_expr() {
        // K-3 remainder (post-v0.21.0): `<Card label="{title}" />`
        // used to reject at expand time with "dynamic prop —
        // deferred to 11.6+". Now the expander parses the raw
        // expression into a classic `Expr` and stores it on the
        // `ChildComponentProp.expr` field for the emitters to
        // consume.
        let src = r#"component Parent {
  state { title: Str = "hi" }
  <template><Card label="{title}" /></template>
}
"#;
        let file = expand_ok_test(src);
        let (child_name, props) =
            find_child(&file.components[0].template.as_ref().unwrap().roots[0])
                .expect("first root is a child component");
        assert_eq!(child_name, "Card");
        assert_eq!(props.len(), 1);
        assert_eq!(props[0].field_name, "label");
        assert!(props[0].is_interpolated(), "expected interpolated prop");
        // raw_value mirrors the expression source so error
        // messages don't need to special-case the shape.
        assert_eq!(props[0].raw_value, "title");
        // The parsed expression is a bare Ident("title") — the
        // SSR emitter rewrites it to `state.title`.
        match &props[0].expr {
            Some(fast::Expr::Ident(name, _)) => assert_eq!(name, "title"),
            other => panic!("expected Ident('title'), got: {other:?}"),
        }
    }

    #[test]
    fn phase_11_7_c_expand_child_component_event_attr_becomes_event_binding() {
        // Phase 11.7.c — `@event="handler"` on a child now expands into a
        // `ChildComponent.events` binding (event bubbling) instead of
        // rejecting.
        let src = r#"component Parent {
  state {}
  <template><Card @select="foo" /></template>
}
"#;
        let file = expand_ok_test(src);
        let tpl = file.components[0].template.as_ref().unwrap();
        match &tpl.roots[0] {
            ExpandedTemplateNode::ChildComponent { name, events, .. } => {
                assert_eq!(name, "Card");
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].event_name, "select");
                assert_eq!(events[0].handler_name, "foo");
            }
            other => panic!("expected ChildComponent, got {other:?}"),
        }
    }

    #[test]
    fn phase_11_5_d_expand_lowercase_tag_stays_element_not_child_component() {
        // Sanity: HTML tags (lowercase first char) must NOT be
        // treated as child components.
        let src = r#"component X {
  state {}
  <template><div class="card" /></template>
}
"#;
        let file = expand_ok_test(src);
        let tpl = file.components[0].template.as_ref().unwrap();
        match &tpl.roots[0] {
            ExpandedTemplateNode::Element { tag, .. } => assert_eq!(tag, "div"),
            other => panic!("expected Element, got {other:?}"),
        }
    }
}
