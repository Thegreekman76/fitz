//! Phase 11.6.b + 11.6.c + 11.6.d + 11.6.e — SSR emitter (`view::emit_ssr`).
//!
//! Second backend paralleling `codegen_wasm.rs`. Consumes an
//! [`ExpandedViewFile`] and emits classic Fitz source text
//! targeting the `fitz-liveviews` framework runtime contract
//! (v0.20.0 `@live_component` + `@render_for` + `@on`
//! decorators + v0.20.1 implicit `flv_register` injection).
//!
//! Phase 11.6.d adds:
//! - Same-file `<Child />` composition — the composition site
//!   emits an `Expr` piece calling `<Child>_render(<Child> {
//!   <props>, ... }).raw` and splicing the result into the
//!   parent's chain-form html body.
//! - Loader-facing entry point: the module bridge
//!   [`crate::view::transform_fitzv_source`] wraps `parse →
//!   expand → check → emit_module_ssr` so the classic loader
//!   can treat a `.fitzv` file as a drop-in classic Fitz
//!   module. See `src/view/mod.rs` for the bridge helpers.
//!
//! ## The .fitzv → fitz-liveviews mapping
//!
//! Source:
//! ```text
//! component Counter {
//!   state { count: Int = 0 }
//!   event reset() { count = 0 }
//!   <template>
//!     <div>
//!       <span>{count}</span>
//!       <button @click="reset">reset</button>
//!     </div>
//!   </template>
//! }
//! ```
//!
//! Emitted classic Fitz:
//! ```text
//! from fitz_liveviews import Html, html
//!
//! @live_component("Counter")
//! type Counter {
//!   count: Int = 0
//! }
//!
//! @render_for("Counter")
//! fn Counter_render(state: Counter) -> Html {
//!   return html("<div><span>{state.count}</span>
//!     <button data-flv-click=\"reset\">reset</button></div>")
//! }
//!
//! @on("Counter", "reset")
//! fn Counter_reset(state: Counter, payload: Map<Str, Str>) -> Counter {
//!   return Counter { count: 0 }
//! }
//! ```
//!
//! Two source-to-emit transformations documented in §9.u:
//! 1. **`@click` → `data-flv-click`** in the emitted HTML —
//!    fitz-liveviews's client runtime binds `data-flv-<event>`
//!    attrs to WebSocket event frames.
//! 2. **`{field}` → `{state.field}`** in the template
//!    interpolation — classic Fitz's string interpolation
//!    resolves the identifier against the enclosing scope,
//!    and the render fn's parameter is named `state`.
//!
//! Plus event body lowering: `.fitzv` events use mutation
//! bodies (`count = count + 1`); the `@on` fn contract is
//! `(state, payload) -> new_state`, so the emitter builds a
//! fresh struct literal from ALL state fields, substituting
//! the ones the event mutated with the assigned value (with
//! bare identifiers rewritten as `state.<field>`).
//!
//! ## MVP scope (11.6.b + 11.6.c partial)
//!
//! - Single-component `.fitzv` file (multi-component
//!   composition comes in 11.6.d).
//! - State fields of any type that classic Fitz accepts
//!   (`Int` / `Float` / `Bool` / `Str` / `Nullable<T>` / ...).
//! - Event bodies with **arbitrary expression RHS** since
//!   11.6.c — `count = count + 1`, `msg = format(...)`,
//!   `xs = state.xs.map(fn(x) => x + 1)`, etc. See
//!   [`format_fitz_expr`] for the exact grammar accepted.
//! - Multiple `Stmt::Assign` per event body: OK — the emitter
//!   accumulates the mutations and builds one struct literal.
//! - Template with `Text` / `Interpolation` / `Element` /
//!   `Static` attrs / `Interpolation` attrs / `Event` attrs.
//! - Template interpolations use the same expression walker as
//!   event body RHS — `{state.count}` (bare state), `{state.name.upper()}`,
//!   `{state.count + 1}`, function calls, etc.
//! - `<style scoped>` / `<style global>` since 11.6.c — inlined
//!   at the top of the render output as a `<style>` tag
//!   (scoped CSS carries the `-<scope>` class suffix already
//!   applied by 11.3.b's `apply_scope`).
//! - Since the 11.6.c continuation:
//!   - `{#if cond} ... {/if}` and `{#if cond} ... {#else} ... {/if}`
//!     template directives lower to a Fitz if-as-expression
//!     yielding `Str`, embedded as an `Expr` piece in the
//!     render body's chain-form html(...) argument.
//!   - `{#for x in xs} ... {/for}` lowers to
//!     `__fitz_view_str_join(<iter>.map(fn(x) => <body>))`
//!     with the `x` binding shadowing any same-named state
//!     field inside the body. The `__fitz_view_str_join(xs:
//!     List<Str>) -> Str` helper is emitted at module header
//!     time (unconditionally, so dead-code when the module
//!     has no `{#for}`).
//!   - View lexer accepts `.` in event body raw-capture
//!     context, so method calls (`count = state.count.
//!     something()`) and field access work in `.fitzv`
//!     source. `Token::Dot` was added to the view lexer and
//!     the raw-body re-serialiser.
//! - Since Phase 11.6.d:
//!   - Same-file `<Child prop="v" />` composition —
//!     `<Child prop="42" />` lowers to
//!     `<Child>_render(<Child> { prop: 42 }).raw`, embedded as
//!     an `Expr` piece. Prop coercion follows the 11.5.d
//!     subset (Str / Int / Float / Bool + `Nullable<T>` of a
//!     primitive) via
//!     [`coerce_child_prop_raw_value_to_fitz_literal`]. Cross-
//!     file composition (child in a sibling `.fitzv` imported
//!     into `main.fitz`) errors clearly and points at 11.6.e
//!     — needs the loader's expanded-file cache threaded to
//!     the parent's expand/check pipeline.
//!   - `.fitzv` module loader integration — see
//!     [`crate::view::transform_fitzv_source`] for the
//!     helper the classic loader consumes when it sees a
//!     `.fitzv` file at import resolution time.
//! - Since Phase 11.6.e:
//!   - Event body widening — bodies can now contain `let`
//!     bindings (assignments to non-state-field idents),
//!     `if` guards (`Stmt::Expr(Expr::If, _)` including
//!     arbitrarily nested), and `if`-as-expression on the
//!     RHS of a `let` / mutation. Kanban's
//!     `card_editor_save` (`let new_text = if
//!     (payload.has("text")) { payload["text"] } else {
//!     state.text }`) and chat's `send_message` (nested
//!     `if (payload.has("author")) { if
//!     (payload.has("text")) { messages = messages + [
//!     Message { author: payload["author"], text:
//!     payload["text"] } ] } }`) both fall out. Trivial
//!     bodies (linear state-field mutations only) keep the
//!     compact `return X { <field>: <rhs>, ... }` shape;
//!     wider bodies switch to a shadow-local shape (prime
//!     `let <field> = state.<field>` at the top, walk body
//!     verbatim, `return X { <field>: <field>, ... }` at
//!     the bottom). See [`emit_event_fn`] for the split.
//!   - Walker widened for `Expr::If` and `Expr::StructLit`
//!     — `if(cond) { <expr> } else { <expr> }` on any RHS
//!     and `TypeName { field: <expr>, ... }` inline struct
//!     construction both work everywhere the walker runs
//!     (event body RHS, template interpolation, state
//!     field defaults). See [`format_fitz_expr_scoped`] +
//!     [`format_if_arm_value`].
//! - `<slot />` still deferred to Phase 11.7+.
//!
//! ## Emit shape: pretty vs chain form
//!
//! The render fn's `html(...)` argument uses one of two
//! serialisation shapes:
//!
//! - **Pretty form** (triple-string) — when every template
//!   piece is `Text` (no directives), the concatenation
//!   emits as `html("""<full HTML>""")`. Readable, closest
//!   to the source `.fitzv` shape.
//! - **Chain form** — when any directive is present, the
//!   pieces emit as `"txt1" + (expr1) + "txt2" + (expr2) +
//!   ...`. Each `Text` piece becomes a single-quoted Fitz
//!   string literal (only `"`, `\n`, `\r` escaped —
//!   backslash-prefixed sequences like `\{` / `\}` from CSS
//!   pre-escape pass through verbatim so classic Fitz reads
//!   them as literal `{` / `}`).
//!
//! ## Public API surface
//!
//! [`emit_module_ssr`] is the entry point the module loader
//! will call in 11.6.d. Returns the full emitted Fitz source
//! text (including the `from fitz_liveviews import ...` line,
//! one `@live_component` type, one `@render_for` fn, and N
//! `@on` fns per component in the file).
//!
//! [`SsrEmitError`] carries a message + context (naming the
//! offending component + event / template node) so the caller
//! can format a useful error. [`SsrEmitResult`] is the alias.

use super::codegen_wasm::{merge_imported_components, ImportedComponentRegistry};
use super::expand::{
    ExpandedAttr, ExpandedComponent, ExpandedEventHandler, ExpandedTemplateNode, ExpandedViewFile,
    ExpandedViewImport,
};
use crate::ast::{AssignTarget, Expr, Stmt};
use std::fmt::Write as _;

/// An error produced by the SSR emitter. `message` is the
/// user-facing description; `context` identifies where in the
/// source the emitter was working when it failed (component
/// name, event name, template node position, etc.). Both are
/// concatenated in `Display`.
#[derive(Debug, Clone, PartialEq)]
pub struct SsrEmitError {
    pub message: String,
    pub context: String,
}

impl std::fmt::Display for SsrEmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ssr emit error — {} ({})", self.message, self.context)
    }
}

impl std::error::Error for SsrEmitError {}

pub type SsrEmitResult<T> = Result<T, SsrEmitError>;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Emit the classic Fitz source text for the entire view file.
/// Consumed by the module loader (11.6.d) when it encounters
/// a `.fitzv` file: this is what gets fed to the classic Fitz
/// lexer + parser as if it were a hand-authored `.fitz`.
///
/// The output always starts with the fitz-liveviews import,
/// followed by one `@live_component` type + one `@render_for`
/// fn + N `@on` fns per component. Blank lines separate the
/// per-component blocks for readability (the emitted source
/// is auto-generated but still developer-inspectable — the
/// counter demo baseline pattern from 11.5).
pub fn emit_module_ssr(file: &ExpandedViewFile) -> SsrEmitResult<String> {
    emit_module_ssr_inner(file)
}

/// Cross-file `<Child />` composition in SSR (parallel to the WASM
/// `emit_module_with_components`). Merges the reachable imported
/// component surfaces into the file before emitting, so
/// `App_render` can compose a companion `<Badge />` declared in a
/// SEPARATE `.fitzv`. The `ChildComponent` emit (which calls
/// `<Child>_render(...)`) and the SSR-4 machinery (hydrate
/// propagation, composed-child state-script suppression, child
/// wrappers) already handle the merged tree — the only gap was the
/// classic loader never loading the imports. With an empty registry
/// this short-circuits to the byte-identical single-file path (no
/// clone), so every `.fitzv` without cross-file composition emits
/// exactly as before.
pub fn emit_module_ssr_with_components(
    file: &ExpandedViewFile,
    imported: &ImportedComponentRegistry,
    transitive_imports: &[ExpandedViewImport],
) -> SsrEmitResult<String> {
    if imported.is_empty() {
        return emit_module_ssr_inner(file);
    }
    let mut merged = merge_imported_components(file, imported);
    // The merged module must carry the TRANSITIVE imports, not just the
    // entry's: a merged child's OWN imports (e.g. the companion `Badge`'s
    // `from fitz_liveviews import flv`, or a child's nominal/fn) must be in
    // scope in the emitted classic Fitz, or its `<Child>_render` fn won't
    // resolve. `merge_imported_components` only copies the entry's
    // `file.imports`; `transitive_imports` (from
    // `collect_transitive_view_imports_with_deps`) is the full reachable set.
    //
    // Two prunings on that set:
    //   - drop imports that resolve to a merged-in COMPONENT — it is inlined
    //     now (`@live_component` + `<Child>_render`), so a verbatim
    //     `from ...Badge import Badge` would REDECLARE it. The registry keys
    //     each component under its LOCAL binding (the alias when present),
    //     which is also the import binding's local name — so they match.
    //   - drop `Html` / `html` from `fitz_liveviews` — `emit_module_header`
    //     already imports them, so keeping them here double-imports the name.
    // The wasm path sidesteps all of this by never emitting classic imports.
    let comp_names: std::collections::HashSet<&str> = imported
        .components()
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    let mut imports: Vec<ExpandedViewImport> = transitive_imports.to_vec();
    for imp in &mut imports {
        let from_flv = imp.path.len() == 1 && imp.path[0] == "fitz_liveviews";
        imp.names.retain(|(orig, alias)| {
            let local = alias.as_deref().unwrap_or(orig.as_str());
            !(comp_names.contains(local) || from_flv && matches!(orig.as_str(), "Html" | "html"))
        });
    }
    imports.retain(|imp| !imp.names.is_empty());
    merged.imports = imports;
    emit_module_ssr_inner(&merged)
}

fn emit_module_ssr_inner(file: &ExpandedViewFile) -> SsrEmitResult<String> {
    // Phase 11.12 SSR-4 — propagate the root `hydrate` marker across the whole
    // component tree (parallel to the WASM `propagate_root_hydrate`) BEFORE any
    // emit, so a composed child emits the isomorphic `fi`/`fr` markers its
    // client adopt walk expects. No component carries the marker → no-op → the
    // rest of the emit is byte-identical to pre-SSR-4.
    let propagated;
    let file: &ExpandedViewFile = if file.components.iter().any(|c| c.hydrate) {
        propagated = propagate_hydrate_clone(file);
        &propagated
    } else {
        file
    };

    let mut out = String::new();
    emit_module_header(&mut out);
    // §9.dd (2026-07-16) — Emit `from X import Y1, Y2, ...` for each
    // top-level `.fitzv` import verbatim before component blocks.
    // The classic Fitz loader then resolves the imported nominals
    // normally (`List<Message>` in state, `Message { ... }` in
    // event bodies).
    emit_user_imports(&file.imports, &mut out);

    // v0.28.4 — the `{#for}` join helper goes AFTER all imports (classic
    // Fitz requires imports at the file head, before any fn/type).
    emit_str_join_helper(&mut out);

    // K-4 (post-K-3, 2026-07-16) — Flatten the file's imports into
    // a single slice of names so `format_fitz_expr_scoped` can
    // resolve bare Idents against them (imported top-level fns /
    // types / consts). Bare Idents inside templates + event bodies
    // that match any name here emit verbatim (not `state.X`, not an
    // error); the classic checker running over the emitted module
    // then validates the reference against its import table.
    // Post S.1 (2026-07-17): each name is `(original, Option<alias>)`.
    // The name in scope inside the SFC is the alias when present, the
    // original otherwise — matching classic Fitz's `from X import Y as
    // Z` semantics (`Z` is the local binding).
    let imported_names: Vec<&str> = file
        .imports
        .iter()
        .flat_map(|imp| {
            imp.names
                .iter()
                .map(|(orig, alias)| alias.as_deref().unwrap_or(orig.as_str()))
        })
        .collect();

    // Phase 11.12 SSR-4 — a component composed as a `<Child />` by any sibling
    // derives its state from props on the client (the parent re-applies props at
    // adopt time), so it must NOT emit its own `<script id="__flv_state_<Comp>">`
    // restore payload — only the un-composed root does. This also avoids
    // duplicate `id` collisions when the same child is composed more than once.
    let composed = collect_composed_child_names(file);

    for component in &file.components {
        let emit_state_script = !composed.contains(component.name.as_str());
        emit_component_ssr_into(
            component,
            &file.components,
            &imported_names,
            emit_state_script,
            &mut out,
        )?;
    }
    Ok(out)
}

/// Phase 11.12 SSR-4 — clone `file` with the `hydrate` marker set on every
/// component. Called only when some component already carries the marker, so
/// the clone is never taken on the common (non-hydratable) path.
fn propagate_hydrate_clone(file: &ExpandedViewFile) -> ExpandedViewFile {
    let mut f = file.clone();
    for c in &mut f.components {
        c.hydrate = true;
    }
    f
}

/// Phase 11.12 SSR-4 — collect the names of every component composed as a
/// `<Child />` anywhere in the file (descending into Element / `{#if}` /
/// `{#for}` / `<slot>` fallback / nested slot content), so their render fns
/// suppress the per-component hydration state script.
fn collect_composed_child_names(file: &ExpandedViewFile) -> std::collections::HashSet<String> {
    fn walk(node: &ExpandedTemplateNode, out: &mut std::collections::HashSet<String>) {
        match node {
            ExpandedTemplateNode::ChildComponent {
                name, slot_content, ..
            } => {
                out.insert(name.clone());
                for c in slot_content {
                    walk(c, out);
                }
            }
            ExpandedTemplateNode::Element { children, .. } => {
                children.iter().for_each(|c| walk(c, out))
            }
            ExpandedTemplateNode::If {
                children,
                else_children,
                ..
            } => {
                children.iter().for_each(|c| walk(c, out));
                if let Some(els) = else_children {
                    els.iter().for_each(|c| walk(c, out));
                }
            }
            ExpandedTemplateNode::For { children, .. } => {
                children.iter().for_each(|c| walk(c, out))
            }
            ExpandedTemplateNode::Slot { fallback, .. } => {
                fallback.iter().for_each(|c| walk(c, out))
            }
            _ => {}
        }
    }
    let mut set = std::collections::HashSet::new();
    for c in &file.components {
        if let Some(t) = &c.template {
            for root in &t.roots {
                walk(root, &mut set);
            }
        }
    }
    set
}

/// Phase 11.12 SSR-4 — the `<slot />` holes a component declares. `has_default`
/// is set by a bare `<slot />`; `named` lists each `<slot name="X" />`. Mirrors
/// the WASM `component_slot_set` (kept local to avoid cross-module coupling).
struct SsrSlotShape {
    has_default: bool,
    named: Vec<String>,
}

fn ssr_slot_shape(component: &ExpandedComponent) -> SsrSlotShape {
    fn walk(node: &ExpandedTemplateNode, shape: &mut SsrSlotShape) {
        match node {
            ExpandedTemplateNode::Slot { name, .. } => match name {
                None => shape.has_default = true,
                Some(n) => {
                    if !shape.named.contains(n) {
                        shape.named.push(n.clone());
                    }
                }
            },
            ExpandedTemplateNode::Element { children, .. } => {
                children.iter().for_each(|c| walk(c, shape))
            }
            ExpandedTemplateNode::If {
                children,
                else_children,
                ..
            } => {
                children.iter().for_each(|c| walk(c, shape));
                if let Some(els) = else_children {
                    els.iter().for_each(|c| walk(c, shape));
                }
            }
            ExpandedTemplateNode::For { children, .. } => {
                children.iter().for_each(|c| walk(c, shape))
            }
            _ => {}
        }
    }
    let mut shape = SsrSlotShape {
        has_default: false,
        named: Vec::new(),
    };
    if let Some(t) = &component.template {
        for root in &t.roots {
            walk(root, &mut shape);
        }
    }
    shape
}

/// Emit one component's classic Fitz source. Rarely useful in
/// isolation — [`emit_module_ssr`] is the entry point. Kept
/// pub so tests can exercise single-component fixtures without
/// wrapping in `ExpandedViewFile { components: vec![...] }`.
///
/// Since Phase 11.6.d, a single-component emit sees only itself
/// as a sibling. `<Child />` composition in this path errors with
/// a clear message pointing at `emit_module_ssr` — a component
/// wanting to compose a sibling MUST be emitted through the
/// module entry.
pub fn emit_component_ssr(component: &ExpandedComponent) -> SsrEmitResult<String> {
    let mut out = String::new();
    emit_module_header(&mut out);
    emit_str_join_helper(&mut out);
    let siblings = std::slice::from_ref(component);
    // K-4: single-component emit sees no imports (no file wrapper);
    // tests exercising this path can't reference imported fns.
    // SSR-4: a lone component is never composed → it emits its own state script.
    emit_component_ssr_into(component, siblings, &[], true, &mut out)?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Module-level header
// ---------------------------------------------------------------------------

/// §9.dd (2026-07-16) — Emit each user-declared top-level `from X
/// import Y1, Y2, ...` from the `.fitzv` verbatim as classic Fitz
/// import stmts at the top of the transformed source. Placed AFTER
/// the framework header (`from fitz_liveviews import Html, html`)
/// so that fitz-liveviews types are always in scope even if the
/// user's imports shadow something — user imports come "later" and
/// take precedence via classic Fitz's normal name resolution.
///
/// Each `ExpandedViewImport` emits one classic Fitz stmt of the
/// shape `from <path.join(".")> import <names>` on its own line,
/// followed by a blank line for readability. Post S.1 (2026-07-17):
/// each name is `(original, Option<alias>)` — when the alias is
/// present, emit `<original> as <alias>` so the loader validates
/// the reference against `<original>` in the imported module while
/// binding `<alias>` in local scope.
fn emit_user_imports(imports: &[ExpandedViewImport], out: &mut String) {
    if imports.is_empty() {
        return;
    }
    for imp in imports {
        let path_str = imp.path.join(".");
        let names_str = imp
            .names
            .iter()
            .map(|(orig, alias)| match alias {
                Some(a) => format!("{orig} as {a}"),
                None => orig.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("from {} import {}\n", path_str, names_str));
    }
    out.push('\n');
}

/// The `from fitz_liveviews import ...` line prepended to every
/// emitted module. The imports match what the render fn signature
/// (`-> Html`) and the render body (`html("""...""")`) need.
/// `Map` is a language built-in so it doesn't need an import;
/// `Str` likewise.
fn emit_module_header(out: &mut String) {
    out.push_str(
        "// Generated by fitz view SSR emitter (Phase 11.6.b + 11.6.c).\n\
         // Do NOT edit — regenerate from the source `.fitzv`.\n\
         \n\
         from fitz_liveviews import Html, html\n\
         \n",
    );
}

/// v0.28.4 — The `{#for}` join helper. Emitted AFTER all imports
/// (the framework header + the user's `from X import ...`), because
/// classic Fitz requires every `import` / `from ... import` to sit at
/// the file head, before any `fn` / `type` declaration. Previously it
/// lived inside `emit_module_header`, so `emit_user_imports` placed the
/// user imports AFTER this fn — an import mid-file that the codegen's
/// loader didn't resolve, so a `.fitzv` whose state referenced a nominal
/// imported from a sibling `.fitz` (`state { xs: List<Member> }`) failed
/// with "unknown type Member in codegen".
fn emit_str_join_helper(out: &mut String) {
    out.push_str(
        "// Phase 11.6.c continuation — helper consumed by every\n\
         // `{#for x in xs} <body> {/for}` template directive: joins\n\
         // a `List<Str>` (typically the output of `xs.map(fn(x) =>\n\
         // <body as Str>)`) into a single `Str` for concatenation\n\
         // into the surrounding HTML. Classic Fitz's `List<Str>`\n\
         // built-in methods do not include `.join()`, so this\n\
         // helper is emitted after the imports. Unused when\n\
         // no `{#for}` is present — accepted as generated-code dead\n\
         // code.\n\
         fn __fitz_view_str_join(xs: List<Str>) -> Str {\n  \
             let out = \"\"\n  \
             for x in xs {\n    \
                 out = out + x\n  \
             }\n  \
             return out\n\
         }\n\
         \n",
    );
}

// ---------------------------------------------------------------------------
// Per-component emit
// ---------------------------------------------------------------------------

fn emit_component_ssr_into(
    component: &ExpandedComponent,
    siblings: &[ExpandedComponent],
    imported_names: &[&str],
    // Phase 11.12 SSR-4 — `false` for a component composed as a `<Child />`
    // elsewhere (its state is re-derived from props on the client), so its
    // render fn skips the `<script id="__flv_state_<Comp>">` payload.
    emit_state_script: bool,
    out: &mut String,
) -> SsrEmitResult<()> {
    // Phase 11.10 slice 4 — `derived` blocks are a client-WASM capability so
    // far; the SSR emitter doesn't lower them yet.
    if !component.derived.is_empty() {
        return Err(SsrEmitError {
            message: "`derived { ... }` blocks are supported on the client-WASM target \
                      (`fitz build --target wasm-client`); SSR support is a later slice"
                .to_string(),
            context: format!("component `{}`", component.name),
        });
    }
    emit_state_type(component, out)?;
    emit_render_fn(component, siblings, imported_names, emit_state_script, out)?;
    for event in &component.events {
        emit_event_fn(component, event, imported_names, out)?;
    }
    Ok(())
}

/// Emit the state type + `@live_component` decorator. The
/// component's name is used verbatim as both the decorator arg
/// and the type name — matches the convention `.fitzv` writers
/// follow when they hand-write `@live_component("MetricTile") type
/// MetricTile { ... }`.
///
/// Every state field carries its declared type + default. The
/// default is emitted via the classic Fitz source form of the
/// original `Expr` — done through `format_expr_source`. Since the
/// checker already validated the shape (per 11.2.b), we trust
/// the AST here.
fn emit_state_type(component: &ExpandedComponent, out: &mut String) -> SsrEmitResult<()> {
    writeln!(out, "@live_component(\"{}\")", component.name).unwrap();
    writeln!(out, "type {} {{", component.name).unwrap();
    for field in &component.state {
        let ty = format_type_expr_source(&field.type_expr);
        let default = format_expr_source(&field.default, &component.name, "state field default")?;
        writeln!(out, "  {}: {} = {}", field.name, ty, default).unwrap();
    }
    writeln!(out, "}}\n").unwrap();
    Ok(())
}

/// Emit the `@render_for` fn — a single-return fn that builds
/// the HTML string from the template. Uses classic Fitz's
/// interpolated triple-string `html("""...""")` shape.
///
/// The return type is `Html`. Every state-field reference in
/// the template becomes `{state.<field>}` inside the emitted
/// string so classic Fitz's interpolation resolver finds it in
/// the enclosing scope.
fn emit_render_fn(
    component: &ExpandedComponent,
    siblings: &[ExpandedComponent],
    imported_names: &[&str],
    emit_state_script: bool,
    out: &mut String,
) -> SsrEmitResult<()> {
    let state_field_names: Vec<&str> = component.state.iter().map(|f| f.name.as_str()).collect();

    // Phase 11.12 SSR-4 — a component that declares a default `<slot />` renders
    // parent-provided slot content, threaded in as a `__slot: Str` parameter
    // (the parent computes the slot HTML in its own scope and passes it at the
    // composition site). Named slots need one `__slot_<name>` param each — a
    // later slice; reject for now.
    let slots = ssr_slot_shape(component);
    if !slots.named.is_empty() {
        return Err(SsrEmitError {
            message: "named `<slot name=\"...\" />` is a client-WASM (`target = \
                      \"wasm-client\"`) capability; SSR support is a later slice. Use a \
                      single default `<slot />`."
                .to_string(),
            context: format!("component `{}` template", component.name),
        });
    }

    writeln!(out, "@render_for(\"{}\")", component.name).unwrap();
    if slots.has_default {
        writeln!(
            out,
            "fn {}_render(state: {}, __slot: Str) -> Html {{",
            component.name, component.name
        )
        .unwrap();
    } else {
        writeln!(
            out,
            "fn {}_render(state: {}) -> Html {{",
            component.name, component.name
        )
        .unwrap();
    }

    let mut pieces: Vec<TemplatePiece> = Vec::new();

    // Phase 11.6.c — inline `<style scoped>` / `<style global>`
    // at the top of the render output. The compiled CSS body
    // (with 11.3.b's `apply_scope` already applied for the
    // `scoped` case) gets wrapped in a `<style>` tag and
    // prepended. Braces are escaped (`{` → `\{`, `}` → `\}`)
    // because CSS `.foo { ... }` uses braces that would
    // otherwise collide with classic Fitz's string
    // interpolation `{expr}` syntax inside `html("""...""")`.
    // fitz-liveviews's HTML diff routine collapses the stable
    // `<style>` block to a no-op patch after the first render,
    // so the per-render inline cost is one-time. Real cleanup
    // (`<style>` routed to `<head>`) would need
    // framework-runtime cooperation — deferred.
    if let Some(style) = &component.style {
        let css_body = match style {
            super::expand::ExpandedStyle::Scoped { css_scoped, .. } => css_scoped,
            super::expand::ExpandedStyle::Global { css, .. } => css,
        };
        let mut buf = String::from("<style>");
        buf.push_str(&escape_css_braces_for_fitz_interp(css_body));
        buf.push_str("</style>");
        push_text(&mut pieces, &buf);
    }

    if let Some(template) = &component.template {
        emit_children_to_pieces(
            &template.roots,
            &state_field_names,
            &[],
            imported_names,
            component,
            siblings,
            false,
            &mut pieces,
        )?;
    }

    // Phase 11.12 SSR-1 — isomorphic hydration state script. When the
    // component opts into hydration (the `hydrate` marker), append a
    // trailing `<script type="application/json" id="__flv_state_<Comp>">`
    // carrying `to_json(state)`. The WASM boot reads it by id
    // (document-wide `get_element_by_id`) to restore the server state,
    // and the adopt walk skips it: it only takes the FIRST element child
    // of the mount as the template root, so a trailing sibling script is
    // never adopted. `to_json(state)` is a classic-Fitz builtin call that
    // resolves at runtime inside the `html("""...""")` triple-string; it
    // produces the flat `{"field": value}` shape the WASM reader
    // `__apply_state_json` expects.
    //
    // Gated by the explicit `hydrate` marker so components that SSR-render
    // for fitz-liveviews' WS-takeover (whose HTML diff forbids `<script>`
    // inside the LiveView root) stay byte-identical. SSR-4: additionally
    // suppressed for a composed child (`emit_state_script == false`) — its
    // state is re-derived from props on the client, so a nested per-child
    // script would be redundant (and collide on `id` across instances).
    if component.hydrate && emit_state_script {
        let script = format!(
            "<script type=\"application/json\" id=\"__flv_state_{}\">{{to_json(state)}}</script>",
            component.name
        );
        push_text(&mut pieces, &script);
    }

    let arg = serialize_pieces_as_html_arg(&pieces);
    writeln!(out, "  return html({arg})").unwrap();
    writeln!(out, "}}\n").unwrap();
    Ok(())
}

// ---------------------------------------------------------------------------
// Phase 11.6.c continuation — template pieces
// ---------------------------------------------------------------------------

/// A segment of the render fn's HTML output. Every template
/// node emits zero or more pieces; the final `html(...)` arg
/// is a `+`-concatenation of the pieces (chain form) or a
/// single triple-string when all pieces are `Text` (pretty form).
///
/// `{#if}` / `{#for}` directives emit as `Expr` pieces holding
/// a Fitz expression that evaluates to `Str`. All other nodes
/// emit as `Text`.
#[derive(Debug, Clone, PartialEq)]
enum TemplatePiece {
    /// Literal HTML content. May include:
    /// - Static markup (`<div>`, `</div>`, attribute names + values).
    /// - Classic Fitz `{state.<field>}` interpolation syntax that
    ///   the enclosing string literal will resolve at runtime.
    /// - Backslash-escaped `\{` / `\}` from CSS body pre-escape.
    Text(String),
    /// Fitz expression source (already lowered via
    /// [`format_fitz_expr_scoped`]) that evaluates to `Str` when
    /// executed. Wrapped in `(...)` at serialisation time.
    Expr(String),
}

/// Append literal HTML text to the piece list. Merges with the
/// preceding `Text` piece when possible so the pretty-form
/// triple-string case emits as a single contiguous string.
fn push_text(pieces: &mut Vec<TemplatePiece>, s: &str) {
    if s.is_empty() {
        return;
    }
    if let Some(TemplatePiece::Text(last)) = pieces.last_mut() {
        last.push_str(s);
    } else {
        pieces.push(TemplatePiece::Text(s.to_string()));
    }
}

/// Phase 11.12 SSR-2 — emit a node's children, wrapping each mixed-context
/// interpolation in `<!--fi-->` … `<!--/fi-->` split markers so the isomorphic
/// hydration walk finds distinct server-painted text nodes.
///
/// The browser coalesces adjacent text runs (`Hello, ` + `{name}` + `!`) into a
/// SINGLE text node, but the client-WASM adopt walk expects one text node per
/// significant run (it calls `__flv_next_text` per run). A comment node between
/// two runs breaks the coalescing, so we wrap each interpolation that sits next
/// to another text-producing sibling; the skip-based `__flv_next_text` steps
/// over the markers and maps 1:1.
///
/// Gated three ways (all must hold to wrap):
/// - `component.hydrate` — the same explicit opt-in SSR-1a used for the state
///   `<script>`; non-hydratable components (fitz-liveviews WS-takeover, keep-node
///   auto-hydration without the marker) stay byte-identical.
/// - `!in_region` — a `{#if}`/`{#for}` region is adopted opaquely (the walk steps
///   over its content between the `<!--fr-->` anchors), so its interpolations are
///   never individually adopted and must not carry `fi` markers.
/// - the interpolation is in a MIXED context (has an adjacent significant
///   text-producing sibling) — a sole-child interpolation (`<span>{name}</span>`)
///   is already its own text node, so it stays marker-free (byte-compat).
#[allow(clippy::too_many_arguments)]
fn emit_children_to_pieces(
    children: &[ExpandedTemplateNode],
    state_field_names: &[&str],
    local_scope: &[&str],
    imported_names: &[&str],
    component: &ExpandedComponent,
    siblings: &[ExpandedComponent],
    in_region: bool,
    pieces: &mut Vec<TemplatePiece>,
) -> SsrEmitResult<()> {
    let wrap = component.hydrate && !in_region;
    for (i, child) in children.iter().enumerate() {
        let needs_markers = wrap && interp_in_mixed_context(children, i);
        if needs_markers {
            push_text(pieces, "<!--fi-->");
        }
        emit_template_node_to_pieces(
            child,
            state_field_names,
            local_scope,
            imported_names,
            component,
            siblings,
            in_region,
            pieces,
        )?;
        if needs_markers {
            push_text(pieces, "<!--/fi-->");
        }
    }
    Ok(())
}

/// Phase 11.12 SSR-2 — is the child at `i` an interpolation that shares a text
/// run boundary with a sibling (so the browser would coalesce them without a
/// marker)? True only for an `Interpolation` whose immediately-adjacent sibling
/// (previous or next) is another text-producing node. An Element or a
/// whitespace-only Text sibling does not trigger markers: an element breaks the
/// text run on its own, and tight authoring keeps whitespace-only text from
/// pinning an interpolation (the same slice-1 authoring constraint).
fn interp_in_mixed_context(children: &[ExpandedTemplateNode], i: usize) -> bool {
    if !matches!(&children[i], ExpandedTemplateNode::Interpolation { .. }) {
        return false;
    }
    let prev = i
        .checked_sub(1)
        .is_some_and(|j| is_significant_text_producer(&children[j]));
    let next = children
        .get(i + 1)
        .is_some_and(is_significant_text_producer);
    prev || next
}

/// A node the browser renders as a text node with content: static Text with a
/// non-empty trim, or any Interpolation. Whitespace-only Text is skipped by the
/// build/adopt walk, so it is not a coalescing partner.
fn is_significant_text_producer(node: &ExpandedTemplateNode) -> bool {
    match node {
        ExpandedTemplateNode::Text(s) => !s.trim().is_empty(),
        ExpandedTemplateNode::Interpolation { .. } => true,
        _ => false,
    }
}

/// Serialise a list of template pieces into the source-text
/// argument for `html(...)`. Two modes:
///
/// 1. **Pretty form** — when every piece is `Text`, emit one
///    triple-string `"""<concat>"""` that classic Fitz reads
///    as a single interpolated string. This is the shape the
///    counter demo, dashboard tile, and every directive-free
///    fixture uses; it's the readable case.
/// 2. **Chain form** — when any piece is `Expr`, emit a
///    `+`-concatenation: `"txt1" + (expr1) + "txt2" + (expr2)`.
///    Each `Text` piece becomes a single-quoted Fitz string
///    literal (only `"`, `\n`, `\r` escaped — CSS-pre-escaped
///    `\{` / `\}` and Fitz interp `{state.<field>}` pass
///    through verbatim). Each `Expr` piece gets wrapped in
///    parens for precedence safety.
fn serialize_pieces_as_html_arg(pieces: &[TemplatePiece]) -> String {
    if pieces.is_empty() {
        return "\"\"".to_string();
    }
    let has_expr = pieces.iter().any(|p| matches!(p, TemplatePiece::Expr(_)));
    if !has_expr {
        let mut buf = String::from("\"\"\"");
        for p in pieces {
            if let TemplatePiece::Text(s) = p {
                buf.push_str(s.trim_end_matches(char::is_whitespace));
            }
        }
        // Restore trim policy from the pre-refactor path: trim
        // only trailing whitespace (multi-piece Text output
        // already handled the whitespace between segments; the
        // outer trim just avoids a dangling newline before
        // `"""`).
        buf.push_str("\"\"\"");
        buf
    } else {
        let parts: Vec<String> = pieces
            .iter()
            .map(|p| match p {
                TemplatePiece::Text(s) => fitz_str_literal_for_chain_form(s),
                TemplatePiece::Expr(e) => format!("({e})"),
            })
            .collect();
        parts.join(" + ")
    }
}

/// Quote `s` as a classic Fitz single-quoted string literal for
/// the chain-form serialisation. Escapes `"` and `\n` / `\r`.
/// Does NOT re-escape backslash-prefixed sequences that were
/// already Fitz-legal escapes at the piece-content level (like
/// `\{` / `\}` from CSS body pre-escape). Fitz's string parser
/// reads these correctly whether they appear in a
/// triple-string or a single-quoted string.
fn fitz_str_literal_for_chain_form(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Emit one `@on(<component>, <event>)` fn. Body semantics:
/// `(state, payload) -> state`, always returning a fresh struct
/// literal that carries EVERY state field.
///
/// Two emit shapes:
///
/// - **Trivial body** — every stmt is `Stmt::Assign` targeting a
///   state-field ident. The emitter produces the compact form
///   preserved since Phase 11.6.b: a single `return <Name> {
///   <field>: <rhs>, ... }` where mutated fields take the
///   assigned value (RHS lowered via [`format_event_rhs`] so
///   bare state-field idents get the `state.` prefix) and
///   untouched fields carry over from `state.<field>`.
/// - **Widened body** — as of Phase 11.6.e, when the body
///   contains `let` bindings (assignments to non-state-field
///   idents), `if` guards (`Stmt::Expr(Expr::If, _)`), or
///   nested statements, the emitter switches to the
///   shadow-local shape: prime each state field as a mutable
///   local at the top (`let <field> = state.<field>`), lower
///   the body statement-by-statement (idents resolve verbatim
///   against the shadow, `let x = ...` introduces new locals,
///   `if (...) { ... }` recurses into arms with a child scope
///   that pops on close), and return `<Name> { <field>:
///   <field>, ... }` at the bottom. Same semantics, just a
///   richer surface. Widens the set of `.fitzv` files the SSR
///   emitter can lower — kanban's `card_editor_save`
///   (`let new_text = if(...){...} else{...}`) and chat's
///   `send_message` (nested `if (payload.has(...)) { ... }`)
///   both fall out.
///
/// Phase 11.6.c widened the RHS walker to the full expression
/// grammar (BinOp / Call / Field / Index / StrInterp / List /
/// Map / Range / Ok / Err / arrow closure). Phase 11.6.e adds
/// `Expr::If` and `Expr::StructLit` to that walker, so
/// if-as-expression RHS (`let x = if(...) {...} else {...}`)
/// and inline struct construction (`Message { author:
/// payload["a"], text: payload["t"] }`) work in every
/// context that calls [`format_fitz_expr_scoped`].
fn emit_event_fn(
    component: &ExpandedComponent,
    event: &ExpandedEventHandler,
    imported_names: &[&str],
    out: &mut String,
) -> SsrEmitResult<()> {
    if !event.params.is_empty() {
        return Err(SsrEmitError {
            message: format!(
                "event `{}` declares parameters — the fitz-liveviews `@on` contract \
                 takes `(state, payload)` only. Parameters on `event` blocks are \
                 deferred to Phase 11.7+ (event bubbling with structured payloads).",
                event.name
            ),
            context: format!("component `{}` event `{}`", component.name, event.name),
        });
    }

    let state_field_names: Vec<&str> = component.state.iter().map(|f| f.name.as_str()).collect();

    if is_trivial_event_body(&event.body, &state_field_names) {
        emit_event_fn_trivial(component, event, &state_field_names, imported_names, out)
    } else {
        emit_event_fn_widened(component, event, &state_field_names, imported_names, out)
    }
}

/// Return `true` iff every stmt in the body is a direct
/// state-field mutation (`Stmt::Assign` with an `Ident` target
/// whose name is a declared state field, no type annotation).
/// The trivial path preserves the compact `return X { a:
/// <rhs>, b: state.b, ... }` shape from Phase 11.6.b/c for
/// human-readable output on the common case. Any other stmt
/// kind (including `let` bindings that introduce non-state
/// locals, `if` guards, or Field/Index assignments) routes
/// through the widened path where the shape is uniform
/// regardless of what the body does.
fn is_trivial_event_body(body: &[Stmt], state_field_names: &[&str]) -> bool {
    body.iter().all(|stmt| match stmt {
        Stmt::Assign {
            target: AssignTarget::Ident(name, _),
            type_: None,
            ..
        } => state_field_names.contains(&name.as_str()),
        _ => false,
    })
}

/// Trivial-body emitter — one `return <Name> { ... }` struct
/// literal where every state field's RHS is either the last
/// assignment's value (last-write-wins if a field is assigned
/// multiple times) or `state.<field>` if untouched.
fn emit_event_fn_trivial(
    component: &ExpandedComponent,
    event: &ExpandedEventHandler,
    state_field_names: &[&str],
    imported_names: &[&str],
    out: &mut String,
) -> SsrEmitResult<()> {
    // Accumulate the mutations. If the same field is assigned
    // multiple times inside the body, the LAST assignment wins
    // — same semantics classic Fitz has when you re-assign the
    // same var in a linear body.
    let mut mutations: Vec<(String, String)> = Vec::new();
    for stmt in &event.body {
        match stmt {
            Stmt::Assign {
                target: AssignTarget::Ident(name, _),
                value,
                type_: None,
                ..
            } => {
                let rhs =
                    format_event_rhs(value, state_field_names, imported_names, component, event)?;
                mutations.push((name.clone(), rhs));
            }
            _ => unreachable!(
                "is_trivial_event_body guarantees every stmt is Stmt::Assign to a state-field Ident"
            ),
        }
    }

    writeln!(out, "@on(\"{}\", \"{}\")", component.name, event.name).unwrap();
    writeln!(
        out,
        "fn {}_{}(state: {}, payload: Map<Str, Str>) -> {} {{",
        component.name, event.name, component.name, component.name
    )
    .unwrap();
    writeln!(out, "  return {} {{", component.name).unwrap();
    for field in &component.state {
        let assigned = mutations
            .iter()
            .rev()
            .find(|(n, _)| n == &field.name)
            .map(|(_, rhs)| rhs.clone());
        match assigned {
            Some(rhs) => writeln!(out, "    {}: {},", field.name, rhs).unwrap(),
            None => writeln!(out, "    {}: state.{},", field.name, field.name).unwrap(),
        }
    }
    writeln!(out, "  }}").unwrap();
    writeln!(out, "}}\n").unwrap();
    Ok(())
}

/// Widened-body emitter — prime shadow locals, lower body
/// stmt-by-stmt via [`lower_event_body_stmts`], return the
/// final struct literal.
///
/// Every state field becomes a mutable local `let <field> =
/// state.<field>` at the top of the fn body. The walker's
/// `local_scope` (paralleling the closure-param shadow logic
/// from Phase 11.6.c) treats those names as bare idents, so
/// `count = count + 1` in the source lowers to
/// `count = (count + 1)` and reads/writes the shadow. `let
/// new_text = ...` introduces additional locals that follow
/// the same rule. The final return builds
/// `<Name> { <field>: <field>, ... }` from the shadows — the
/// unmutated fields still hold their initial `state.<field>`
/// value, mutated ones the last write.
///
/// The wider shape is uniform: every `.fitzv` event body
/// lowers the same way regardless of what mutations happen,
/// so debugging the emitted output is easier than reasoning
/// about the trivial path's per-field rewrites. The trivial
/// path is kept only because its output is more compact for
/// the common case (single state-field mutation).
fn emit_event_fn_widened(
    component: &ExpandedComponent,
    event: &ExpandedEventHandler,
    state_field_names: &[&str],
    imported_names: &[&str],
    out: &mut String,
) -> SsrEmitResult<()> {
    writeln!(out, "@on(\"{}\", \"{}\")", component.name, event.name).unwrap();
    writeln!(
        out,
        "fn {}_{}(state: {}, payload: Map<Str, Str>) -> {} {{",
        component.name, event.name, component.name, component.name
    )
    .unwrap();

    // Prime shadow locals from state. Every state field is a
    // reassignable Fitz local now — assignments to `<field> =
    // ...` in the body mutate the shadow, and the terminal
    // return reads back from it.
    for field in &component.state {
        writeln!(out, "  let {} = state.{}", field.name, field.name).unwrap();
    }

    // Local scope seed: state fields (so the walker skips the
    // `state.` rewrite; they now reference the mutable
    // shadow) + `payload` (the fn param). New `let x = ...`
    // stmts extend the scope; `if` arms get their own
    // truncate-on-exit sub-scope so an arm-local binding
    // doesn't leak.
    let mut local_scope: Vec<String> = component.state.iter().map(|f| f.name.clone()).collect();
    local_scope.push("payload".to_string());

    lower_event_body_stmts(
        &event.body,
        state_field_names,
        &mut local_scope,
        imported_names,
        component,
        event,
        "  ",
        out,
    )?;

    writeln!(out, "  return {} {{", component.name).unwrap();
    for field in &component.state {
        writeln!(out, "    {}: {},", field.name, field.name).unwrap();
    }
    writeln!(out, "  }}").unwrap();
    writeln!(out, "}}\n").unwrap();
    Ok(())
}

/// Recursive body-lowering helper — emits classic Fitz source
/// text for the widened event-body grammar. Each stmt maps as
/// follows:
///
/// - `Stmt::Assign { target: Ident(name), value, .. }` — if
///   `name` is already in `local_scope` (either a state-field
///   shadow or a previously introduced local), emit `<name> =
///   <lowered rhs>` (a reassignment). Otherwise emit `let
///   <name> = <lowered rhs>` and push `name` to `local_scope`
///   (a new local). Fitz's AST does not preserve the `let`
///   keyword — the scope-tracking model here is the honest
///   interpretation.
/// - `Stmt::Assign { target: Field { .. } | Index { .. } }` —
///   reject with a Phase 11.7+ pointer (nested mutation via
///   `obj.field = ...` / `xs[i] = ...` requires the mutation
///   to reach outside the shadow-local model).
/// - `Stmt::Expr(Expr::If { condition, then, else_ }, _)` —
///   emit `if (<cond>) { <recurse then> } else { <recurse
///   else> }` (or without the else clause). Each arm gets a
///   child scope: extend `local_scope` for the arm, then
///   truncate back to the pre-arm length on close so an
///   arm-local `let` doesn't leak.
/// - `Stmt::Expr(other, _)` — reject. Bare expression stmts
///   are typically side-effect calls (`xs.push(item)`) that
///   escape the shadow-local model. Deferred to Phase 11.7+.
/// - Any other stmt kind (`Return`, `Break`, `Continue`,
///   `For`, `While`, `Loop`, `TypeDef`, `FnDef`, `Import`,
///   `FromImport`, `Destructure`, `ReturnStatus`, `Error`) —
///   reject with a Phase 11.7+ pointer. The event-body
///   subset is deliberately narrow: assignments + `let`
///   bindings + `if` guards.
// The 8-arg signature carries the emitter's contextual state
// through recursion (stmts, state fields, local scope, imported
// names, component, event, indent, out). A dedicated `ScopeCtx`
// struct would collapse it to 3-4 args but adds a layer of
// indirection with no functional benefit — deferred until a
// second walker helper needs the same bundle.
#[allow(clippy::too_many_arguments)]
fn lower_event_body_stmts(
    stmts: &[Stmt],
    state_field_names: &[&str],
    local_scope: &mut Vec<String>,
    imported_names: &[&str],
    component: &ExpandedComponent,
    event: &ExpandedEventHandler,
    indent: &str,
    out: &mut String,
) -> SsrEmitResult<()> {
    for stmt in stmts {
        match stmt {
            Stmt::Assign {
                target: AssignTarget::Ident(name, _),
                value,
                type_: _,
                ..
            } => {
                let local_refs: Vec<&str> = local_scope.iter().map(String::as_str).collect();
                let rhs = format_fitz_expr_scoped(
                    value,
                    state_field_names,
                    &local_refs,
                    imported_names,
                    &component.name,
                    &format!("event `{}` body RHS", event.name),
                )?;
                let already_bound = local_scope.iter().any(|s| s == name);
                if already_bound {
                    writeln!(out, "{indent}{name} = {rhs}").unwrap();
                } else {
                    // New local — annotation on the source `let x: T
                    // = ...` is dropped from the emit; classic Fitz
                    // will infer the same type from the RHS shape.
                    writeln!(out, "{indent}let {name} = {rhs}").unwrap();
                    local_scope.push(name.clone());
                }
            }
            Stmt::Assign {
                target: AssignTarget::Field { .. },
                ..
            } => {
                return Err(SsrEmitError {
                    message: format!(
                        "event `{}` body assigns to a field access (`obj.field = ...`) \
                         — only direct state-field assignments (`<field> = ...`) and \
                         local `let` bindings are supported. Deferred to Phase 11.7+.",
                        event.name
                    ),
                    context: format!("component `{}` event `{}`", component.name, event.name),
                });
            }
            Stmt::Assign {
                target: AssignTarget::Index { .. },
                ..
            } => {
                return Err(SsrEmitError {
                    message: format!(
                        "event `{}` body assigns to an index (`xs[i] = ...`) — only \
                         direct state-field assignments (`<field> = ...`) and local \
                         `let` bindings are supported. Deferred to Phase 11.7+.",
                        event.name
                    ),
                    context: format!("component `{}` event `{}`", component.name, event.name),
                });
            }
            Stmt::Expr(
                Expr::If {
                    condition,
                    then,
                    else_,
                    ..
                },
                _,
            ) => {
                let local_refs: Vec<&str> = local_scope.iter().map(String::as_str).collect();
                let cond_src = format_fitz_expr_scoped(
                    condition,
                    state_field_names,
                    &local_refs,
                    imported_names,
                    &component.name,
                    &format!("event `{}` if condition", event.name),
                )?;

                let child_indent = format!("{indent}  ");
                writeln!(out, "{indent}if ({cond_src}) {{").unwrap();
                let saved = local_scope.len();
                lower_event_body_stmts(
                    then,
                    state_field_names,
                    local_scope,
                    imported_names,
                    component,
                    event,
                    &child_indent,
                    out,
                )?;
                local_scope.truncate(saved);
                match else_ {
                    Some(else_body) => {
                        writeln!(out, "{indent}}} else {{").unwrap();
                        let saved = local_scope.len();
                        lower_event_body_stmts(
                            else_body,
                            state_field_names,
                            local_scope,
                            imported_names,
                            component,
                            event,
                            &child_indent,
                            out,
                        )?;
                        local_scope.truncate(saved);
                        writeln!(out, "{indent}}}").unwrap();
                    }
                    None => {
                        writeln!(out, "{indent}}}").unwrap();
                    }
                }
            }
            // §9.cc V-6 (2026-07-16) — Accept bare method-call stmts
            // whose base is a shadow-local Ident (a state field name
            // primed at the top of the widened event body per §9.aa).
            // Semantics: Fitz `List<T>` is `Arc<Mutex<Vec<T>>>` (per
            // F17), so mutation via `.push(x)` / `.remove(i)` / etc on
            // the shadow local propagates to `state.<field>` via the
            // shared Arc. The struct-lit return (`return X { <field>:
            // <field>, ... }`) then re-packages the (now-mutated) same
            // Arc, preserving §9.aa's shadow-local return contract
            // WITHOUT needing an immutable-return builtin like
            // `List<T>.appended(x)`. Also applies to `Map<K, V>` (also
            // `Arc<Mutex<...>>`) and to any nominal type wrapped in
            // Arc<Mutex>. Zero-copy state mutation.
            //
            // Restricted shape: callee must be
            // `Expr::Field { base: Expr::Ident(<shadow>), name }`.
            // Method chains (`xs.map(...).filter(...)`), calls on
            // nested field access (`obj.list.push(...)`), and other
            // deeper shapes are rejected with a targeted Phase 11.7+
            // pointer — extending them is a follow-up mini-fase if
            // real demand appears.
            Stmt::Expr(expr @ Expr::Call { callee, .. }, _) => {
                let is_shadow_method_call = if let Expr::Field { object, .. } = callee.as_ref() {
                    if let Expr::Ident(name, _) = object.as_ref() {
                        local_scope.iter().any(|s| s == name)
                    } else {
                        false
                    }
                } else {
                    false
                };
                if !is_shadow_method_call {
                    return Err(SsrEmitError {
                        message: format!(
                            "event `{}` body contains a bare call statement whose \
                             callee is not a method call on a shadow-local state \
                             field. Only single-level method calls on shadow locals \
                             (`<field>.push(...)`, `<field>.remove(i)`, etc.) are \
                             accepted. Method chains (`xs.map(...).filter(...)`), \
                             calls on nested field access (`obj.list.push(...)`), \
                             and free-standing calls are deferred to Phase 11.7+.",
                            event.name
                        ),
                        context: format!("component `{}` event `{}`", component.name, event.name),
                    });
                }
                let local_refs: Vec<&str> = local_scope.iter().map(String::as_str).collect();
                let call_src = format_fitz_expr_scoped(
                    expr,
                    state_field_names,
                    &local_refs,
                    imported_names,
                    &component.name,
                    &format!("event `{}` body method call", event.name),
                )?;
                writeln!(out, "{indent}{call_src}").unwrap();
            }
            Stmt::Expr(_, _) => {
                return Err(SsrEmitError {
                    message: format!(
                        "event `{}` body contains a bare expression statement (not an \
                         `if` guard nor a method call on a shadow-local state field) \
                         — only assignments (`<field> = ...`), `let` bindings, `if` \
                         guards, and single-level method calls on shadow locals \
                         (`<field>.push(...)`) are supported today. Other bare \
                         expression statements are deferred to Phase 11.7+.",
                        event.name
                    ),
                    context: format!("component `{}` event `{}`", component.name, event.name),
                });
            }
            _ => {
                return Err(SsrEmitError {
                    message: format!(
                        "event `{}` body contains an unsupported statement kind. The SSR \
                         emitter accepts assignments (state-field mutations and local \
                         `let` bindings) plus `if` guards. Loop / return / function / \
                         type / import statements are deferred to Phase 11.7+.",
                        event.name
                    ),
                    context: format!("component `{}` event `{}`", component.name, event.name),
                });
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Event body — RHS lowering
// ---------------------------------------------------------------------------

/// Format the RHS of an event body assignment. Thin wrapper
/// over [`format_fitz_expr_scoped`] with the event's context label
/// baked in — the walker handles the actual grammar.
///
/// Phase 11.6.c widened this from literal-only to the full
/// walker grammar (BinOp arithmetic, function calls,
/// StrInterp, Field access, Index, List, Map, Range,
/// Ok/Err, arrow closures for `.map(...)` etc.).
///
/// Phase 11.6.e — the emitted `@on` fn signature is
/// `fn <Name>_<event>(state: <Name>, payload: Map<Str, Str>) -> <Name>`,
/// so `payload` is a valid free identifier inside the body. It is
/// added to the walker's `local_scope` here (paralleling closure
/// params) so RHS expressions like `payload["author"]` or
/// `payload.has("text")` emit verbatim without tripping the "not a
/// declared state field" rejection.
fn format_event_rhs(
    expr: &Expr,
    state_field_names: &[&str],
    imported_names: &[&str],
    component: &ExpandedComponent,
    event: &ExpandedEventHandler,
) -> SsrEmitResult<String> {
    format_fitz_expr_scoped(
        expr,
        state_field_names,
        &["payload"],
        imported_names,
        &component.name,
        &format!("event `{}` body RHS", event.name),
    )
}

// ---------------------------------------------------------------------------
// Template — HTML string emission
// ---------------------------------------------------------------------------

/// Phase 11.12 SSR-3 — push a `{#if}`/`{#for}` region's Fitz Str-expression,
/// wrapping it in `<!--fr-->` … `<!--/fr-->` anchor markers when the component
/// is hydratable and this region is TOP-LEVEL (`!in_region`).
///
/// The client-WASM adopt walk (`__flv_next_comment`) acquires those two anchors
/// into the region's `__astart_<r>` / `__aend_<r>` handles and leaves the
/// server-painted content between them in place — a later state change patches
/// only that region. The anchors are ALWAYS emitted, even when the region
/// renders empty (a false `{#if}` → `<!--fr--><!--/fr-->`), because the build
/// walk always creates both comment anchors regardless of the condition, and
/// the adopt must find both to line up `keep_region_index()`.
///
/// A NESTED region (already `in_region` — e.g. the `{#if}` inside a `{#for}`
/// body) is rebuilt naively on the client inside its parent region's fragment,
/// so it has no anchors and stays marker-free. Non-hydratable components stay
/// byte-identical (same `component.hydrate` gate as SSR-1/SSR-2).
fn push_region_expr(
    pieces: &mut Vec<TemplatePiece>,
    expr_src: String,
    component: &ExpandedComponent,
    in_region: bool,
) {
    let wrap = component.hydrate && !in_region;
    if wrap {
        push_text(pieces, "<!--fr-->");
    }
    pieces.push(TemplatePiece::Expr(expr_src));
    if wrap {
        push_text(pieces, "<!--/fr-->");
    }
}

/// Recursively emit a template node into `out` as HTML source
/// text. State-field identifiers in interpolations get
/// rewritten with the `state.` prefix so classic Fitz's
/// string interpolation resolver finds them in the render
/// fn's scope.
///
/// 11.6.b MVP handles:
/// - `Text` — appended verbatim.
/// - `Interpolation { expr }` — `{state.<field>}` when the
///   inner expr is a bare state-field identifier; otherwise
///   the full expr source (also with state-field renaming).
/// - `Element { tag, attrs, children }` — with `@click`
///   attrs translated to `data-flv-click`, `{field}` interp
///   attrs rewritten with `state.`, and static attrs emitted
///   verbatim. HTML-escaped attribute values.
/// - Since Phase 11.6.d, `ChildComponent` — composed
///   inline as `Expr("<Child>_render(<Child> { <props>,
///   ... }).raw")`, where each declared child prop takes
///   the supplied literal (coerced to the field's declared
///   type) and every other child state field is omitted so
///   the struct literal picks up the child's declared
///   default. Reject with a same-file pointer when the
///   composed name is not declared in `siblings` — the
///   only cross-file resolution today lives at the module
///   loader layer (`.fitzv` sibling imported by
///   `main.fitz`).
/// - `If` / `For` — Since Phase 11.6.c continuation, lower to
///   Fitz if-as-expression / `__fitz_view_str_join(iter.map(...))`.
///   Since Phase 11.12 SSR-3, a TOP-LEVEL region is wrapped in
///   `<!--fr-->` … `<!--/fr-->` anchors via [`push_region_expr`]
///   when the component is hydratable.
/// - `Slot` — rejected with a 11.7+ pointer.
#[allow(clippy::too_many_arguments)]
fn emit_template_node_to_pieces(
    node: &ExpandedTemplateNode,
    state_field_names: &[&str],
    local_scope: &[&str],
    imported_names: &[&str],
    component: &ExpandedComponent,
    siblings: &[ExpandedComponent],
    // Phase 11.12 SSR-2 — `true` when this node is inside a `{#if}`/`{#for}`
    // region's content. A region is adopted opaquely on the client (the walk
    // steps over its content between the `<!--fr-->` anchors), so mixed-text
    // interpolations inside it must NOT get `<!--fi-->` split markers.
    in_region: bool,
    pieces: &mut Vec<TemplatePiece>,
) -> SsrEmitResult<()> {
    match node {
        ExpandedTemplateNode::Text(s) => {
            push_text(pieces, s);
            Ok(())
        }
        ExpandedTemplateNode::Interpolation { expr, .. } => {
            // CW.9 (1b/1c) — `{raw_html(x)}` / `{html(x)}` is the explicit
            // raw-HTML marker that dual-targets with the wasm `set_inner_html`
            // sink. On SSR, classic `{expr}` interpolation is ALREADY raw
            // (unescaped — escaping is opt-in via `flv()`), so strip the
            // marker and emit the inner expr verbatim. This keeps a template
            // authored with `{raw_html(icon.raw)}` byte-identical to the
            // idiomatic `{icon.raw}`, while letting the same source compile to
            // the wasm target. (Left unwrapped, `raw_html`/`html` return an
            // `Html` struct that would serialise as its debug repr here.)
            let target = raw_html_marker_arg(expr).unwrap_or(expr);
            let rendered = format_fitz_expr_scoped(
                target,
                state_field_names,
                local_scope,
                imported_names,
                &component.name,
                "template interpolation",
            )?;
            let mut wrapped = String::from("{");
            wrapped.push_str(&rendered);
            wrapped.push('}');
            push_text(pieces, &wrapped);
            Ok(())
        }
        ExpandedTemplateNode::Element {
            tag,
            attrs,
            children,
            self_closing,
            ..
        } => {
            let mut open = String::from("<");
            open.push_str(tag);
            push_text(pieces, &open);
            for attr in attrs {
                push_text(pieces, " ");
                emit_attr_to_pieces(
                    attr,
                    state_field_names,
                    local_scope,
                    imported_names,
                    component,
                    pieces,
                )?;
            }
            if *self_closing {
                push_text(pieces, " />");
                return Ok(());
            }
            push_text(pieces, ">");
            emit_children_to_pieces(
                children,
                state_field_names,
                local_scope,
                imported_names,
                component,
                siblings,
                in_region,
                pieces,
            )?;
            let mut close = String::from("</");
            close.push_str(tag);
            close.push('>');
            push_text(pieces, &close);
            Ok(())
        }
        // Phase 11.6.c continuation — `{#if cond}...{/if}` and
        // `{#if cond}...{#else}...{/if}` lower to a Fitz
        // if-as-expression that yields a `Str`. Both branches
        // recursively emit pieces; each branch's pieces
        // serialise via `serialize_pieces_as_html_arg` so
        // nested directives work naturally.
        ExpandedTemplateNode::If {
            cond,
            children,
            else_children,
            ..
        } => {
            let cond_src = format_fitz_expr_scoped(
                cond,
                state_field_names,
                local_scope,
                imported_names,
                &component.name,
                "template `{#if}` condition",
            )?;
            let mut then_pieces: Vec<TemplatePiece> = Vec::new();
            emit_children_to_pieces(
                children,
                state_field_names,
                local_scope,
                imported_names,
                component,
                siblings,
                true,
                &mut then_pieces,
            )?;
            let then_src = serialize_pieces_as_html_arg(&then_pieces);
            let else_src = match else_children {
                Some(kids) => {
                    let mut ep: Vec<TemplatePiece> = Vec::new();
                    emit_children_to_pieces(
                        kids,
                        state_field_names,
                        local_scope,
                        imported_names,
                        component,
                        siblings,
                        true,
                        &mut ep,
                    )?;
                    serialize_pieces_as_html_arg(&ep)
                }
                None => "\"\"".to_string(),
            };
            push_region_expr(
                pieces,
                format!("if ({cond_src}) {{ {then_src} }} else {{ {else_src} }}"),
                component,
                in_region,
            );
            Ok(())
        }
        // Phase 11.6.c continuation — `{#for x in xs}...{/for}`
        // lowers to `__fitz_view_str_join(<iter>.map(fn(x) =>
        // <body as Str>))`. The `x` binding shadows any
        // same-named state field inside the body via the
        // walker's `local_scope` param.
        ExpandedTemplateNode::For {
            var,
            iter,
            children,
            ..
        } => {
            let iter_src = format_fitz_expr_scoped(
                iter,
                state_field_names,
                local_scope,
                imported_names,
                &component.name,
                "template `{#for}` iterable",
            )?;
            let mut new_scope: Vec<&str> = local_scope.to_vec();
            new_scope.push(var.as_str());
            let mut body_pieces: Vec<TemplatePiece> = Vec::new();
            emit_children_to_pieces(
                children,
                state_field_names,
                &new_scope,
                imported_names,
                component,
                siblings,
                true,
                &mut body_pieces,
            )?;
            let body_src = serialize_pieces_as_html_arg(&body_pieces);
            push_region_expr(
                pieces,
                format!("__fitz_view_str_join({iter_src}.map(fn({var}) => {body_src}))"),
                component,
                in_region,
            );
            Ok(())
        }
        // Phase 11.12 SSR-4 — a default `<slot />` renders the parent-provided
        // slot content, threaded in as the `__slot: Str` render-fn parameter.
        // With a fallback, `if (__slot == "") { <fallback> } else { __slot }`
        // paints the child's own fallback only when the parent passed nothing
        // (a self-closing `<Child />` passes `""`), mirroring the WASM
        // `emit_slot` (`__slot None` → fallback). Named slots are rejected in
        // `emit_render_fn` before we get here.
        ExpandedTemplateNode::Slot { name, fallback, .. } => {
            if name.is_some() {
                return Err(SsrEmitError {
                    message: "named `<slot name=\"...\" />` is a client-WASM (`target = \
                              \"wasm-client\"`) capability; SSR support is a later slice."
                        .to_string(),
                    context: format!("component `{}` template", component.name),
                });
            }
            if fallback.is_empty() {
                pieces.push(TemplatePiece::Expr("__slot".to_string()));
            } else {
                // The fallback belongs to the CHILD, so it renders in the child's
                // scope (the same `state_field_names` / `local_scope` /
                // `imported_names` this render fn already uses).
                let mut fb: Vec<TemplatePiece> = Vec::new();
                emit_children_to_pieces(
                    fallback,
                    state_field_names,
                    local_scope,
                    imported_names,
                    component,
                    siblings,
                    in_region,
                    &mut fb,
                )?;
                let fb_src = serialize_pieces_as_html_arg(&fb);
                pieces.push(TemplatePiece::Expr(format!(
                    "if (__slot == \"\") {{ {fb_src} }} else {{ __slot }}"
                )));
            }
            Ok(())
        }
        // Phase 11.6.d — Same-file `<Child />` composition. Look
        // up the child in the sibling list, build a struct
        // literal with each supplied prop coerced to a Fitz
        // literal, and emit the piece as
        // `Expr("<Child>_render(<Child> { <props> }).raw")`. The
        // `.raw` unwraps the `Html { raw: Str }` value that
        // fitz-liveviews's `html(...)` constructor produced,
        // yielding the child's rendered HTML as a `Str` we splice
        // into the parent's chain-form html body.
        //
        // Cross-file composition (child declared in a DIFFERENT
        // `.fitzv` imported into main.fitz) requires the
        // expand/check pipeline to see the sibling — deferred
        // until 11.6.e wires the loader's expanded-file cache
        // through the checker.
        ExpandedTemplateNode::ChildComponent {
            name,
            props,
            events,
            slot_content,
            ..
        } => {
            // Phase 11.7.c — event bubbling (`<Child @event="..." />`) is a
            // client-WASM capability today. The SSR target re-renders the
            // whole tree per event over the wire, so child→parent callbacks
            // don't apply the same way; reject rather than silently drop.
            if let Some(binding) = events.first() {
                return Err(SsrEmitError {
                    message: format!(
                        "event binding `@{}=\"...\"` on `<{name} />` — child→parent event \
                         bubbling is a client-WASM (`target = \"wasm-client\"`) capability \
                         (Phase 11.7.c); it is not supported on the SSR target.",
                        binding.event_name
                    ),
                    context: format!("component `{}` template", component.name),
                });
            }
            let child = siblings
                .iter()
                .find(|c| &c.name == name)
                .ok_or_else(|| SsrEmitError {
                    message: format!(
                        "child component composition `<{name} />` — component `{name}` \
                         is not declared in the same `.fitzv` file. Cross-file `<Child />` \
                         composition (importing `Comp.fitzv` from `main.fitz` and \
                         composing `<Comp />` inside another component's template) is \
                         deferred to Phase 11.6.e. For now, declare both parent and \
                         child in the same `.fitzv` module."
                    ),
                    context: format!("component `{}` template", component.name),
                })?;

            // Phase 11.12 SSR-4 — slot fill (`<Child>content</Child>`) inlines
            // the parent-provided content at the child's default `<slot />`. The
            // content is rendered in the PARENT's scope (its state fields + event
            // handlers → `data-flv-*`) and passed to the child render fn as the
            // `__slot: Str` argument. A child with no default `<slot />` can't
            // receive content — reject (the checker already guards this, but keep
            // a targeted emit-time message).
            let child_slots = ssr_slot_shape(child);
            if !slot_content.is_empty() && !child_slots.has_default {
                return Err(SsrEmitError {
                    message: format!(
                        "slot content `<{name}>...</{name}>` — component `{name}` declares \
                         no default `<slot />` to receive it, so the content would be \
                         dropped. Add `<slot />` to `{name}`, or use a self-closing \
                         `<{name} />`."
                    ),
                    context: format!("component `{}` template", component.name),
                });
            }
            // When the child declares a default `<slot />`, always pass a slot
            // argument (possibly `""` for a self-closing `<Child />`, so the
            // child falls back to its own `<slot>` fallback).
            let slot_arg: Option<String> = if child_slots.has_default {
                let mut sp: Vec<TemplatePiece> = Vec::new();
                emit_children_to_pieces(
                    slot_content,
                    state_field_names,
                    local_scope,
                    imported_names,
                    component,
                    siblings,
                    in_region,
                    &mut sp,
                )?;
                Some(serialize_pieces_as_html_arg(&sp))
            } else {
                None
            };

            let expr_src = format_child_composition(
                child,
                props,
                component,
                state_field_names,
                local_scope,
                imported_names,
                slot_arg.as_deref(),
            )?;

            // Phase 11.12 SSR-4 — a hydratable parent wraps the child render in
            // `<div class="__fitz-child-<Name>">`, matching the WASM build walk
            // (`emit_child_component`) so the isomorphic client adopt
            // (`emit_child_component_adopt`) finds the wrapper with
            // `__flv_next_element`. Non-hydratable parents keep the bare inline
            // splice (byte-identical to pre-SSR-4).
            if component.hydrate {
                push_text(pieces, &format!("<div class=\"__fitz-child-{name}\">"));
                pieces.push(TemplatePiece::Expr(expr_src));
                push_text(pieces, "</div>");
            } else {
                pieces.push(TemplatePiece::Expr(expr_src));
            }
            Ok(())
        }
    }
}

/// Emit a single attribute to HTML. Handles the three shapes:
/// - `Static { name, value }` — `<name>="<value>"` verbatim.
/// - `Interpolation { name, expr }` — `<name>="{state.<field>}"`
///   with the state-field rewrite.
/// - `Event { event_name, handler_name }` — translated to
///   `data-flv-<event>="<handler>"` per the fitz-liveviews
///   client runtime contract.
fn emit_attr_to_pieces(
    attr: &ExpandedAttr,
    state_field_names: &[&str],
    local_scope: &[&str],
    imported_names: &[&str],
    component: &ExpandedComponent,
    pieces: &mut Vec<TemplatePiece>,
) -> SsrEmitResult<()> {
    match attr {
        ExpandedAttr::Static { name, value, .. } => {
            // Escape `"` in value for the surrounding `"..."` in
            // the emitted HTML string.
            let escaped = value.replace('"', "\\\"");
            push_text(pieces, &format!("{name}=\"{escaped}\""));
            Ok(())
        }
        ExpandedAttr::Interpolation { name, expr, .. } => {
            let rendered = format_fitz_expr_scoped(
                expr,
                state_field_names,
                local_scope,
                imported_names,
                &component.name,
                "template attribute interpolation",
            )?;
            push_text(pieces, &format!("{name}=\"{{{rendered}}}\""));
            Ok(())
        }
        // Mixed attribute value (`class="toast toast-{kind}"`): literal
        // chunks pass through verbatim, `{expr}` segments get the same
        // state-field rewriting as a full interpolation so they resolve
        // to `state.<field>` in the emitted classic-Fitz string. The
        // reassembled value is one Text piece (interpolations ride
        // inside the emitted string, exactly like the full-interp arm) —
        // never an Expr piece (those are reserved for `{#if}`/`{#for}`).
        ExpandedAttr::MixedInterpolation { name, segments, .. } => {
            let mut value = String::new();
            for seg in segments {
                match seg {
                    super::expand::AttrValueSegment::Literal(lit) => value.push_str(lit),
                    super::expand::AttrValueSegment::Expr(expr) => {
                        let rendered = format_fitz_expr_scoped(
                            expr,
                            state_field_names,
                            local_scope,
                            imported_names,
                            &component.name,
                            "template attribute interpolation",
                        )?;
                        value.push('{');
                        value.push_str(&rendered);
                        value.push('}');
                    }
                }
            }
            push_text(pieces, &format!("{name}=\"{value}\""));
            Ok(())
        }
        // Form B (gotcha #6) — conditional boolean attribute
        // `checked={expr}`: present in the DOM iff `expr` is truthy.
        // Emit a Fitz if-as-expression that yields the bare attribute
        // name (present) or "" (absent). A bare boolean attribute is
        // valid HTML5 and matches the `{#if}checked{/if}` workaround's
        // output. The unconditional leading space the element loop
        // pushes before each attr is harmless when the attr is absent.
        ExpandedAttr::BoolInterpolation { name, expr, .. } => {
            let cond = format_fitz_expr_scoped(
                expr,
                state_field_names,
                local_scope,
                imported_names,
                &component.name,
                "conditional boolean attribute",
            )?;
            pieces.push(TemplatePiece::Expr(format!(
                "if ({cond}) {{ \"{name}\" }} else {{ \"\" }}"
            )));
            Ok(())
        }
        ExpandedAttr::Event {
            event_name,
            handler_name,
            ..
        } => {
            push_text(pieces, &format!("data-flv-{event_name}=\"{handler_name}\""));
            Ok(())
        }
    }
}

// Note: `format_template_interpolation` (11.6.b's dedicated
// wrapper) was removed by 11.6.c continuation. The template
// interpolation call site in `emit_template_node_to_pieces`
// calls `format_fitz_expr_scoped` directly so it can pass the
// active `local_scope` (needed for `{#for x in xs}` bodies
// where `x` shadows any same-named state field). Downstream
// consumers that need the pre-`local_scope` wrapper can use
// `format_fitz_expr` (empty local scope).

// ---------------------------------------------------------------------------
// Type + default source formatting
// ---------------------------------------------------------------------------

/// Print a `TypeExpr` as classic Fitz source text. Mirrors
/// `TypeExpr::display_name` (which does the same job) but kept
/// local so future emit-specific tweaks don't affect other
/// consumers.
fn format_type_expr_source(ty: &crate::ast::TypeExpr) -> String {
    ty.display_name()
}

/// Escape `{` and `}` in a CSS body so they survive intact when
/// inlined into a classic Fitz `html("""...""")` string. Fitz's
/// string interpolation syntax `"{expr}"` would otherwise try to
/// parse CSS `.foo { color: red; }` as an interpolated
/// expression `{ color: red; }`. Fitz recognises `\{` and `\}`
/// as literal-brace escapes; this helper does the doubling
/// (backslash-brace).
fn escape_css_braces_for_fitz_interp(css: &str) -> String {
    let mut out = String::with_capacity(css.len() + 8);
    for ch in css.chars() {
        match ch {
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Phase 11.6.d — `<Child />` composition
// ---------------------------------------------------------------------------

/// Emit a `<Child />` composition site as a Fitz expression that
/// evaluates to `Str` for chain-form html arg splicing.
///
/// The shape is:
/// ```text
/// <Child>_render(<Child> { <prop1>: <lit1>, <prop2>: <lit2>, ... }).raw
/// ```
///
/// - `<Child>_render` is the render fn the SSR emitter generates
///   for the child component.
/// - The struct literal supplies each **explicitly-mentioned**
///   prop coerced to a Fitz literal. Every unmentioned state
///   field is omitted so the struct literal picks up the child
///   type's declared default. This matches classic Fitz's rule
///   for struct literals with defaults: unmentioned fields fall
///   back to `default:` if present, error if not (Phase 3.2).
/// - `.raw` unwraps `Html { raw: Str }` (fitz-liveviews's
///   canonical type) to a bare `Str` we can concat with the
///   surrounding html body.
///
/// The MVP delegates the "which types are legal for a prop"
/// question to Phase 11.5.d's [`super::check::coerce_child_prop_raw_value`]
/// via [`coerce_child_prop_raw_value_to_fitz_literal`], which
/// enforces the same primitive subset (Str / Int / Float / Bool
/// / `Nullable<T>` of a primitive).
fn format_child_composition(
    child: &ExpandedComponent,
    props: &[super::expand::ChildComponentProp],
    parent: &ExpandedComponent,
    parent_state_field_names: &[&str],
    parent_local_scope: &[&str],
    parent_imported_names: &[&str],
    // Phase 11.12 SSR-4 — the parent-rendered slot content, as a serialized
    // classic-Fitz `Str` expression, when the child declares a default
    // `<slot />` (passed as the second `__slot` render-fn argument). `None`
    // when the child has no slot (byte-identical to the pre-SSR-4 call shape).
    slot_arg: Option<&str>,
) -> SsrEmitResult<String> {
    let child_name = &child.name;
    let mut out = format!("{child_name}_render({child_name} {{");
    for (i, prop) in props.iter().enumerate() {
        let field = child
            .state
            .iter()
            .find(|f| f.name == prop.field_name)
            .ok_or_else(|| SsrEmitError {
                message: format!(
                    "internal error: prop `{}=\"{}\"` on `<{child_name} />` \
                         matches no declared state field of `{child_name}`. The \
                         checker should have caught this — please report a bug \
                         with the offending `.fitzv` source.",
                    prop.field_name, prop.raw_value
                ),
                context: format!("component `{}` template", parent.name),
            })?;
        // K-3 remainder: interpolated props (`prop={expr}`) inline
        // the parsed expression source with the PARENT's state-
        // field rewriting rules — a bare `count` in the expression
        // refers to the parent's `count` state field (rewritten to
        // `state.count`), NOT the child's. Closure-parameter locals
        // (e.g. `{#for x in xs}` around the `<Child />`) shadow via
        // the parent's `local_scope`.
        let value_src = if let Some(expr) = &prop.expr {
            format_fitz_expr_scoped(
                expr,
                parent_state_field_names,
                parent_local_scope,
                parent_imported_names,
                &parent.name,
                &format!(
                    "child composition <{child_name} /> prop '{}'",
                    prop.field_name
                ),
            )?
        } else {
            coerce_child_prop_raw_value_to_fitz_literal(
                &prop.raw_value,
                &field.type_expr,
                child_name,
                &prop.field_name,
            )?
        };
        if i > 0 {
            out.push(',');
        }
        out.push(' ');
        out.push_str(&prop.field_name);
        out.push_str(": ");
        out.push_str(&value_src);
    }
    // Trailing space keeps the emitted source pretty in both
    // shapes: `Child { }` (no props → all state fields use their
    // declared defaults) and `Child { count: 42, msg: "hi" }`
    // (some props supplied).
    out.push_str(" }");
    // SSR-4: append the `__slot` argument for a child that declares a
    // default `<slot />`.
    if let Some(sa) = slot_arg {
        out.push_str(", ");
        out.push_str(sa);
    }
    out.push_str(").raw");
    Ok(out)
}

/// Parallel to [`super::check::coerce_child_prop_raw_value`]
/// (Phase 11.5.d) but returns a **classic Fitz literal** (source
/// text) instead of a Rust literal. Used by
/// [`format_child_composition`] to build the struct literal at
/// the `<Child />` composition site.
///
/// Same accepted subset:
/// - `Str` → `"<raw>"` with `"` and `\` escaped.
/// - `Int` → `<raw>` (parsed as `i64`).
/// - `Float` → `<raw>` (parsed as `f64`).
/// - `Bool` → `true` / `false`.
/// - `Nullable<T>` → `null` when raw is literally `null`,
///   otherwise recurse on the inner type.
/// - `List<T>` → comma-separated raw values; empty string yields
///   `[]`, otherwise each item recursively coerces to the classic
///   Fitz literal form (K-3, MVP: no comma escaping).
///
/// Rejects nominals, `Map<K, V>`, and function types with a
/// targeted 11.7+ pointer — those need richer static-prop
/// coercion which the MVP defers.
pub(crate) fn coerce_child_prop_raw_value_to_fitz_literal(
    raw: &str,
    type_expr: &crate::ast::TypeExpr,
    child_name: &str,
    field_name: &str,
) -> SsrEmitResult<String> {
    use crate::ast::TypeExpr as T;
    match type_expr {
        T::Named(name) => match name.as_str() {
            "Str" => {
                let mut out = String::from("\"");
                for ch in raw.chars() {
                    match ch {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\r' => out.push_str("\\r"),
                        c => out.push(c),
                    }
                }
                out.push('"');
                Ok(out)
            }
            "Int" => raw
                .parse::<i64>()
                .map(|n| n.to_string())
                .map_err(|_| SsrEmitError {
                    message: format!(
                        "expected an integer literal for `Int` field `{field_name}` of \
                         `<{child_name} />`, got `{raw}`. Use a bare integer like \
                         `{field_name}=\"42\"`."
                    ),
                    context: format!("child composition `<{child_name} />`"),
                }),
            "Float" => raw
                .parse::<f64>()
                .map(|n| {
                    // Preserve `.0` for integer-valued floats so the
                    // classic Fitz parser reads it as `Float`, not
                    // `Int`.
                    if n.fract() == 0.0 && n.is_finite() {
                        format!("{n:.1}")
                    } else {
                        format!("{n}")
                    }
                })
                .map_err(|_| SsrEmitError {
                    message: format!(
                        "expected a float literal for `Float` field `{field_name}` of \
                         `<{child_name} />`, got `{raw}`. Use a decimal like \
                         `{field_name}=\"1.5\"`."
                    ),
                    context: format!("child composition `<{child_name} />`"),
                }),
            "Bool" => match raw {
                "true" => Ok("true".to_string()),
                "false" => Ok("false".to_string()),
                _ => Err(SsrEmitError {
                    message: format!(
                        "expected `\"true\"` or `\"false\"` for `Bool` field \
                         `{field_name}` of `<{child_name} />`, got `{raw}`."
                    ),
                    context: format!("child composition `<{child_name} />`"),
                }),
            },
            other => Err(SsrEmitError {
                message: format!(
                    "prop coercion to nominal `{other}` on `<{child_name} />` — \
                     static props for user-defined types are deferred to Phase \
                     11.7+ (needs richer prop coercion in the checker). Today's \
                     MVP accepts only `Str`, `Int`, `Float`, `Bool`, and their \
                     `Nullable<T>` wrappers."
                ),
                context: format!("child composition `<{child_name} />`"),
            }),
        },
        T::Nullable(inner) => {
            if raw == "null" {
                Ok("null".to_string())
            } else {
                coerce_child_prop_raw_value_to_fitz_literal(raw, inner, child_name, field_name)
            }
        }
        T::Generic { name, args } => {
            if name == "List" && args.len() == 1 {
                // K-3: List<primitive> via comma-separated raw values.
                // Empty string → empty list literal `[]`. Whitespace around
                // commas is trimmed. Each item is recursively coerced to the
                // inner type using the classic Fitz literal syntax. No
                // escaping of commas today (MVP scope).
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return Ok("[]".to_string());
                }
                let inner = &args[0];
                let mut lits: Vec<String> = Vec::new();
                for item in trimmed.split(',') {
                    let piece = item.trim();
                    let lit = coerce_child_prop_raw_value_to_fitz_literal(
                        piece, inner, child_name, field_name,
                    )?;
                    lits.push(lit);
                }
                Ok(format!("[{}]", lits.join(", ")))
            } else if name == "Map" && args.len() == 2 {
                // S.2 (2026-07-17): Map<Str, Str> static props via
                // `k=v,k=v` convention — Fitz literal form. Only Str,Str
                // supported (see check.rs::coerce_child_prop_raw_value
                // for the rationale). Empty raw → `{}`. No escaping
                // today; users needing richer maps use interpolation.
                let is_str = |t: &crate::ast::TypeExpr| {
                    matches!(
                        t,
                        crate::ast::TypeExpr::Named(n) if n == "Str"
                    )
                };
                if !(is_str(&args[0]) && is_str(&args[1])) {
                    return Err(SsrEmitError {
                        message: format!(
                            "prop coercion to `Map<{}, {}>` on `<{child_name} />` — \
                             static props for `Map` are only supported for \
                             `Map<Str, Str>` today. Use interpolation \
                             `<{child_name} {field_name}=\"{{someMap}}\" />` for richer \
                             Map shapes.",
                            args[0].head_name(),
                            args[1].head_name()
                        ),
                        context: format!("child composition `<{child_name} />`"),
                    });
                }
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return Ok("{}".to_string());
                }
                let mut lits: Vec<String> = Vec::new();
                for pair in trimmed.split(',') {
                    let (k, v) = pair.split_once('=').ok_or_else(|| SsrEmitError {
                        message: format!(
                            "Map<Str, Str> pair `{}` on `<{child_name} />` prop \
                             `{field_name}` is not in `key=value` form. Use \
                             comma-separated `k=v,k2=v2` pairs.",
                            pair.trim()
                        ),
                        context: format!("child composition `<{child_name} />`"),
                    })?;
                    // Both key and value are Str literals in the Fitz
                    // source — quote + escape `"` and `\` inline.
                    let quote_str = |s: &str| {
                        let mut out = String::from("\"");
                        for ch in s.chars() {
                            match ch {
                                '"' => out.push_str("\\\""),
                                '\\' => out.push_str("\\\\"),
                                '\n' => out.push_str("\\n"),
                                '\r' => out.push_str("\\r"),
                                c => out.push(c),
                            }
                        }
                        out.push('"');
                        out
                    };
                    lits.push(format!("{}: {}", quote_str(k.trim()), quote_str(v.trim())));
                }
                Ok(format!("{{{}}}", lits.join(", ")))
            } else {
                Err(SsrEmitError {
                    message: format!(
                        "prop coercion to `{name}<...>` on `<{child_name} />` — static props \
                         for this compound type are deferred to Phase 11.7+. Today's MVP \
                         accepts primitives, their `Nullable<T>` wrappers, \
                         `List<primitive>` (comma-separated), and `Map<Str, Str>` \
                         (comma-separated `k=v,k=v`)."
                    ),
                    context: format!("child composition `<{child_name} />`"),
                })
            }
        }
        T::Function { .. } => Err(SsrEmitError {
            message: format!(
                "prop coercion to a function type on `<{child_name} />` — passing \
                 callbacks as props is deferred to Phase 11.7+ (event bubbling from \
                 children needs framework-level plumbing). Today's MVP accepts only \
                 primitives + `Nullable<T>` primitives."
            ),
            context: format!("child composition `<{child_name} />`"),
        }),
        T::Tuple(_) => Err(SsrEmitError {
            message: format!(
                "prop coercion to a tuple type on `<{child_name} />` — tuple props \
                 are deferred to Phase 11.7+. Today's MVP accepts only primitives + \
                 `Nullable<T>` primitives."
            ),
            context: format!("child composition `<{child_name} />`"),
        }),
    }
}

/// Print an `Expr` as classic Fitz source text. Thin wrapper
/// over [`format_fitz_expr`] with no state fields in scope —
/// suitable for state field defaults where referencing another
/// state field would be circular anyway.
///
/// 11.6.c widened the accepted shape from literal-only to the
/// full walker's grammar (BinOp / UnaryOp / Call / Field /
/// Index / StrInterp / List / Map / Range / Ok / Err /
/// FnExpr-arrow). Statement / control-flow shapes still
/// reject with 11.7+ pointers.
fn format_expr_source(
    expr: &Expr,
    component_name: &str,
    context_label: &str,
) -> SsrEmitResult<String> {
    // `format_expr_source` is used for state-field defaults, which
    // are evaluated before any imports resolve; passing `&[]` here
    // matches the classic Fitz surface (default exprs can only
    // reference constants + literals, not free vars).
    format_fitz_expr(expr, &[], &[], component_name, context_label)
}

// ---------------------------------------------------------------------------
// Phase 11.6.c — Expression → Fitz source walker
// ---------------------------------------------------------------------------

/// Print an `Expr` as classic Fitz source text, rewriting
/// bare state-field identifiers as `state.<field>` so the
/// emitted code resolves them against the render fn's / event
/// fn's parameter named `state`.
///
/// This is the shared workhorse for:
/// - Event body RHS expressions ([`format_event_rhs`]).
/// - Template `{expr}` interpolations
///   ([`format_template_interpolation`]).
/// - State field defaults ([`format_expr_source`], which
///   passes an empty `state_field_names` slice — defaults
///   cannot reference other state fields).
///
/// **11.6.c accepts** (in addition to 11.6.b's literals +
/// bare state ident):
/// - `BinOp` — always emitted with outer parentheses to
///   preserve precedence across arbitrary nesting.
/// - `UnaryOp` — `Neg`/`Not`/`BitNot`, again with outer
///   parens.
/// - `Call` — receiver + args recursively walked. Method
///   calls (`callee` is `Expr::Field`) work naturally
///   because `Field` is one of the walked variants.
/// - `Field` — `<obj>.<field>`. `state.count` on a
///   pre-rewritten ident is idempotent.
/// - `Index` — `<obj>[<index>]`.
/// - `StrInterp` — each `StrPart::Lit` copied verbatim,
///   each `StrPart::Expr` recursively walked. Interpolated
///   segments respect the state-field rewrite.
/// - `List` — `[<a>, <b>, ...]`.
/// - `Map` — `{<k>: <v>, ...}` — heterogeneous OK because
///   we emit source text, classic Fitz will type-check.
/// - `Range` — `<start>..<end>` or `..=` with inclusive flag.
/// - `Ok`/`Err` — Result constructors.
/// - `FnExpr` — arrow form only (single `Return` statement in
///   body). Multi-statement closures deferred to Phase 11.7+.
///
/// **Phase 11.6.e widens the walker further**:
/// - `If` as expression — `if(<cond>) { <arm> } else { <arm> }`.
///   Each arm body is a single `Stmt::Expr(<value>, _)` (see
///   [`format_if_arm_value`]); without-else falls back to
///   `null` so the arm has a value. Kanban's `let seed =
///   if(payload.has(...)) { payload["..."] } else {
///   state.text }` uses this. Multi-stmt arms deferred to
///   Phase 11.7+.
/// - `StructLit` — `TypeName { field: <expr>, ... }` for
///   building fresh instances of state-list elements or
///   auxiliary types. Chat's `Message { author: ...,
///   text: ... }` in a `send_message` event body uses this.
///
/// **11.6.c/e rejects** with a 11.7+ pointer:
/// - `Slice` (`xs[a..b]`) — rarely useful in the SSR path.
/// - `ListComp`/`MapComp` — the whole comprehension surface.
/// - `Tuple` — tuple types don't cross the fitz-liveviews API
///   boundary cleanly today.
/// - `Match` as an expression — the SSR walker does not lower
///   match-arms yet.
/// - `Await`/`Try`/`NamedArg` — async and error propagation
///   both make no sense inside a synchronous render body.
/// - `Bytes` — not a template-friendly type.
/// - `Ident` for a NON-state field, NOT in `local_scope` —
///   could be a free variable the render's enclosing scope
///   resolves (e.g. a `let` in the caller), but the SSR
///   emitter cannot know without scope analysis. Deferred
///   to 11.7+ when the loader integration lands.
/// - `Error` — the parser's error-recovery sentinel; should
///   never appear in a checked AST but reject defensively.
fn format_fitz_expr(
    expr: &Expr,
    state_field_names: &[&str],
    imported_names: &[&str],
    component_name: &str,
    context_label: &str,
) -> SsrEmitResult<String> {
    format_fitz_expr_scoped(
        expr,
        state_field_names,
        &[],
        imported_names,
        component_name,
        context_label,
    )
}

/// Inner walker that also tracks a `local_scope` of identifiers
/// introduced by enclosing `FnExpr`s (closure params). Ident
/// resolution priority:
///
/// 1. `local_scope` — closure params take precedence and emit
///    verbatim (bare identifier, no rewrite).
/// 2. `state_field_names` — declared state fields rewrite as
///    `state.<field>`.
/// 3. Everything else — reject with a 11.7+ pointer (needs
///    module-loader scope resolution to know if it's a free
///    var in the caller's scope).
///
/// CW.9 — if `expr` is `raw_html(x)` / `html(x)` (single arg), returns the
/// inner `x`. The interpolation handler strips this raw-HTML marker on SSR
/// (classic `{expr}` is already raw), so a dual-target template written with
/// `{raw_html(...)}` renders byte-identically to the idiomatic raw `{x}` and
/// still compiles to the wasm `set_inner_html` sink. Mirrors
/// `codegen_wasm::raw_html_sink_arg`.
fn raw_html_marker_arg(expr: &Expr) -> Option<&Expr> {
    if let Expr::Call { callee, args, .. } = expr {
        if let Expr::Ident(name, _) = callee.as_ref() {
            if (name == "raw_html" || name == "html") && args.len() == 1 {
                return Some(&args[0]);
            }
        }
    }
    None
}

/// Kept as an inner impl so the public [`format_fitz_expr`]
/// signature stays terse for the common case (no locals).
fn format_fitz_expr_scoped(
    expr: &Expr,
    state_field_names: &[&str],
    local_scope: &[&str],
    imported_names: &[&str],
    component_name: &str,
    context_label: &str,
) -> SsrEmitResult<String> {
    match expr {
        // ---- Literals ----
        Expr::Int(n, _) => Ok(format!("{n}")),
        Expr::Float(f, _) => Ok(format!("{f}")),
        Expr::Bool(b, _) => Ok(b.to_string()),
        Expr::Str(s, _) => Ok(format!("{s:?}")),
        Expr::Null(_) => Ok("null".to_string()),

        // ---- Ident resolution order (K-4, post-K-3) ----
        //
        // 1. `local_scope` — closure params (`{#for x in xs}` inside
        //    the template, `fn(c) => ...` inside event bodies) shadow
        //    everything. Emit verbatim.
        // 2. `state_field_names` — bare state field ref. Rewrite to
        //    `state.<name>`.
        // 3. `imported_names` — top-level fn / type / const brought
        //    into scope via `from X import Y` at the top of the
        //    `.fitzv` file (§9.dd). Emit verbatim; the classic checker
        //    validates the call against the emitted module.
        // 4. Otherwise — hard error. Not a state field, not a local,
        //    not imported. Real free-var (module-loader resolution)
        //    remains a Phase 11.7+ concern.
        Expr::Ident(name, _) => {
            if local_scope.contains(&name.as_str()) {
                Ok(name.clone())
            } else if state_field_names.contains(&name.as_str()) {
                Ok(format!("state.{name}"))
            } else if imported_names.contains(&name.as_str()) {
                Ok(name.clone())
            } else {
                Err(SsrEmitError {
                    message: format!(
                        "identifier `{name}` in {context_label} for component \
                         `{component_name}` is not a declared state field nor an \
                         imported name. Add `from <module> import {name}` at the top \
                         of the `.fitzv` file if it's a top-level fn / type / const, \
                         or introduce it via a local binding. Free-var refs beyond \
                         the imports table remain a Phase 11.7+ concern."
                    ),
                    context: format!("component `{component_name}` {context_label}"),
                })
            }
        }

        // ---- BinOp / UnaryOp — outer parens for precedence safety ----
        Expr::BinOp {
            op, left, right, ..
        } => {
            let l = format_fitz_expr_scoped(
                left,
                state_field_names,
                local_scope,
                imported_names,
                component_name,
                context_label,
            )?;
            let r = format_fitz_expr_scoped(
                right,
                state_field_names,
                local_scope,
                imported_names,
                component_name,
                context_label,
            )?;
            let op_str = format_binop_source(op);
            Ok(format!("({l} {op_str} {r})"))
        }
        Expr::UnaryOp { op, operand, .. } => {
            let inner = format_fitz_expr_scoped(
                operand,
                state_field_names,
                local_scope,
                imported_names,
                component_name,
                context_label,
            )?;
            let op_str = format_unaryop_source(op);
            Ok(format!("({op_str}{inner})"))
        }

        // ---- Call / Field / Index — recursive walk ----
        Expr::Call { callee, args, .. } => {
            let callee_src = format_fitz_expr_scoped(
                callee,
                state_field_names,
                local_scope,
                imported_names,
                component_name,
                context_label,
            )?;
            let mut arg_srcs: Vec<String> = Vec::with_capacity(args.len());
            for a in args {
                arg_srcs.push(format_fitz_expr_scoped(
                    a,
                    state_field_names,
                    local_scope,
                    imported_names,
                    component_name,
                    context_label,
                )?);
            }
            Ok(format!("{callee_src}({})", arg_srcs.join(", ")))
        }
        Expr::Field { object, field, .. } => {
            let obj_src = format_fitz_expr_scoped(
                object,
                state_field_names,
                local_scope,
                imported_names,
                component_name,
                context_label,
            )?;
            Ok(format!("{obj_src}.{field}"))
        }
        Expr::Index { object, index, .. } => {
            let obj_src = format_fitz_expr_scoped(
                object,
                state_field_names,
                local_scope,
                imported_names,
                component_name,
                context_label,
            )?;
            let idx_src = format_fitz_expr_scoped(
                index,
                state_field_names,
                local_scope,
                imported_names,
                component_name,
                context_label,
            )?;
            Ok(format!("{obj_src}[{idx_src}]"))
        }

        // ---- StrInterp — walk each interpolated part ----
        Expr::StrInterp(parts, _) => {
            let mut out = String::from("\"");
            for part in parts {
                match part {
                    crate::ast::StrPart::Lit(s) => {
                        // Escape `"` and `\` for the surrounding `"..."`.
                        for ch in s.chars() {
                            match ch {
                                '"' => out.push_str("\\\""),
                                '\\' => out.push_str("\\\\"),
                                c => out.push(c),
                            }
                        }
                    }
                    crate::ast::StrPart::Expr(e, _fmt) => {
                        // Format specs (FormatSpec) are deferred: the
                        // 11.6.c walker doesn't try to preserve
                        // `{x:0.2f}` shape — that surface is rarely
                        // used in template interpolations and would
                        // need dedicated formatting.
                        let inner = format_fitz_expr_scoped(
                            e,
                            state_field_names,
                            local_scope,
                            imported_names,
                            component_name,
                            context_label,
                        )?;
                        out.push('{');
                        out.push_str(&inner);
                        out.push('}');
                    }
                }
            }
            out.push('"');
            Ok(out)
        }

        // ---- List / Map / Range ----
        Expr::List(items, _) => {
            let mut srcs = Vec::with_capacity(items.len());
            for it in items {
                srcs.push(format_fitz_expr_scoped(
                    it,
                    state_field_names,
                    local_scope,
                    imported_names,
                    component_name,
                    context_label,
                )?);
            }
            Ok(format!("[{}]", srcs.join(", ")))
        }
        Expr::Map(entries, _) => {
            let mut srcs = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                let ks = format_fitz_expr_scoped(
                    k,
                    state_field_names,
                    local_scope,
                    imported_names,
                    component_name,
                    context_label,
                )?;
                let vs = format_fitz_expr_scoped(
                    v,
                    state_field_names,
                    local_scope,
                    imported_names,
                    component_name,
                    context_label,
                )?;
                srcs.push(format!("{ks}: {vs}"));
            }
            Ok(format!("{{{}}}", srcs.join(", ")))
        }
        Expr::Range {
            start,
            end,
            inclusive,
            ..
        } => {
            let start_src = format_fitz_expr_scoped(
                start,
                state_field_names,
                local_scope,
                imported_names,
                component_name,
                context_label,
            )?;
            let end_src = format_fitz_expr_scoped(
                end,
                state_field_names,
                local_scope,
                imported_names,
                component_name,
                context_label,
            )?;
            let op = if *inclusive { "..=" } else { ".." };
            Ok(format!("{start_src}{op}{end_src}"))
        }

        // ---- Ok / Err constructors ----
        Expr::Ok(inner, _) => {
            let inner_src = format_fitz_expr_scoped(
                inner,
                state_field_names,
                local_scope,
                imported_names,
                component_name,
                context_label,
            )?;
            Ok(format!("Ok({inner_src})"))
        }
        Expr::Err(inner, _) => {
            let inner_src = format_fitz_expr_scoped(
                inner,
                state_field_names,
                local_scope,
                imported_names,
                component_name,
                context_label,
            )?;
            Ok(format!("Err({inner_src})"))
        }

        // ---- FnExpr — arrow form only ----
        Expr::FnExpr {
            params,
            body,
            is_async,
            ..
        } => {
            if *is_async {
                return Err(SsrEmitError {
                    message: format!(
                        "async closure inside {context_label} for component `{component_name}` — \
                         async makes no sense inside a synchronous render body. Deferred to \
                         Phase 11.7+."
                    ),
                    context: format!("component `{component_name}` {context_label}"),
                });
            }
            // Only the arrow form (single `Return` stmt) is
            // supported. Multi-stmt closure bodies would need
            // the statement lowerer.
            if body.len() != 1 {
                return Err(SsrEmitError {
                    message: format!(
                        "closure inside {context_label} for component `{component_name}` has a \
                         multi-statement body — 11.6.c MVP only supports the arrow form \
                         (`fn(x) => <expr>`). Multi-statement closures deferred to Phase 11.7+."
                    ),
                    context: format!("component `{component_name}` {context_label}"),
                });
            }
            // Extend the local scope with this closure's params
            // so bare `Ident` refs inside the body emit verbatim
            // (rather than being rewritten as `state.<name>` or
            // rejected as free vars).
            let mut new_scope: Vec<&str> = local_scope.to_vec();
            for p in params {
                new_scope.push(p.name.as_str());
            }
            let inner = match &body[0] {
                Stmt::Return(e, _) => format_fitz_expr_scoped(
                    e,
                    state_field_names,
                    &new_scope,
                    imported_names,
                    component_name,
                    context_label,
                )?,
                _ => {
                    return Err(SsrEmitError {
                        message: format!(
                            "closure inside {context_label} for component `{component_name}` has \
                             a non-return statement — 11.6.c MVP only supports the arrow form \
                             (`fn(x) => <expr>`). Deferred to Phase 11.7+."
                        ),
                        context: format!("component `{component_name}` {context_label}"),
                    });
                }
            };
            let params_src: Vec<String> = params
                .iter()
                .map(|p| match &p.type_ {
                    Some(ty) => format!("{}: {}", p.name, ty.display_name()),
                    None => p.name.clone(),
                })
                .collect();
            Ok(format!("fn({}) => {inner}", params_src.join(", ")))
        }

        // ---- If-as-expression (Phase 11.6.e) ----
        //
        // `if (cond) { <arm> } else { <arm> }` where each arm
        // body is a single expression statement — matches the
        // kanban pattern `let seed = if (payload.has(...))
        // { payload["..."] } else { state.text }`. Multi-stmt
        // arms defer to Phase 11.7+ via
        // [`format_if_arm_value`]; without-else falls back to
        // `null` so the arm always has a value.
        Expr::If {
            condition,
            then,
            else_,
            ..
        } => {
            let cond_src = format_fitz_expr_scoped(
                condition,
                state_field_names,
                local_scope,
                imported_names,
                component_name,
                context_label,
            )?;
            let then_src = format_if_arm_value(
                then,
                state_field_names,
                local_scope,
                imported_names,
                component_name,
                context_label,
            )?;
            let else_src = match else_ {
                Some(body) => format_if_arm_value(
                    body,
                    state_field_names,
                    local_scope,
                    imported_names,
                    component_name,
                    context_label,
                )?,
                None => "null".to_string(),
            };
            Ok(format!(
                "if ({cond_src}) {{ {then_src} }} else {{ {else_src} }}"
            ))
        }

        // ---- StructLit (Phase 11.6.e) ----
        //
        // `TypeName { field: <expr>, ... }` for building fresh
        // instances of state-list elements or auxiliary types.
        // Chat's `send_message` uses this to construct
        // `Message { author: payload["author"], text:
        // payload["text"] }` before appending to a state list.
        // Each field's RHS walks recursively through the same
        // scope machinery. The emitter does not validate that
        // `type_name` is declared — classic Fitz's type
        // checker handles that on the round-trip.
        Expr::StructLit {
            type_name, fields, ..
        } => {
            let mut field_srcs = Vec::with_capacity(fields.len());
            for (fname, fexpr) in fields {
                let val_src = format_fitz_expr_scoped(
                    fexpr,
                    state_field_names,
                    local_scope,
                    imported_names,
                    component_name,
                    context_label,
                )?;
                field_srcs.push(format!("{fname}: {val_src}"));
            }
            Ok(format!("{type_name} {{ {} }}", field_srcs.join(", ")))
        }

        // ---- Explicit rejections with targeted pointers ----
        Expr::Try(_, _)
        | Expr::Await(_, _)
        | Expr::NamedArg { .. }
        | Expr::Match { .. }
        | Expr::Slice { .. }
        | Expr::ListComp { .. }
        | Expr::MapComp { .. }
        | Expr::Bytes(_, _) => Err(SsrEmitError {
            message: format!(
                "{context_label} for component `{component_name}` uses an expression \
                 shape (`{}`) that the SSR walker does not yet handle. Deferred to \
                 Phase 11.7+.",
                expr_kind_label(expr)
            ),
            context: format!("component `{component_name}` {context_label}"),
        }),
        Expr::Error(_) => Err(SsrEmitError {
            message: format!(
                "{context_label} for component `{component_name}` contains an `Expr::Error` \
                 recovery sentinel — the parser did not produce a valid AST. Fix upstream \
                 parse errors first."
            ),
            context: format!("component `{component_name}` {context_label}"),
        }),
        _ => Err(SsrEmitError {
            message: format!(
                "{context_label} for component `{component_name}` uses an expression \
                 shape not covered by the 11.6.c SSR walker. Deferred to Phase 11.7+."
            ),
            context: format!("component `{component_name}` {context_label}"),
        }),
    }
}

/// Emit the value expression of an if-as-expression arm body.
///
/// Phase 11.6.e MVP: an arm body must be a single
/// `Stmt::Expr(_)` — the sole statement's expression IS the
/// arm's value. Multi-stmt arms would need the emitter to
/// choose between "sequence all stmts and pick the last" and
/// "wrap in a block that classic Fitz reads as an
/// if-expression". Both work in classic Fitz semantics but
/// the sequential-emit model would require lowering the
/// full event-body grammar recursively, and the block-wrap
/// interacts oddly with `let` bindings inside arms. Deferred.
fn format_if_arm_value(
    body: &[Stmt],
    state_field_names: &[&str],
    local_scope: &[&str],
    imported_names: &[&str],
    component_name: &str,
    context_label: &str,
) -> SsrEmitResult<String> {
    if body.len() != 1 {
        return Err(SsrEmitError {
            message: format!(
                "{context_label} for component `{component_name}` uses an `if`-as-expression \
                 with a multi-statement arm body — the SSR walker only supports single \
                 expression-stmt arms today (`if (...) {{ <expr> }} else {{ <expr> }}`). \
                 Deferred to Phase 11.7+."
            ),
            context: format!("component `{component_name}` {context_label}"),
        });
    }
    match &body[0] {
        Stmt::Expr(e, _) => format_fitz_expr_scoped(
            e,
            state_field_names,
            local_scope,
            imported_names,
            component_name,
            context_label,
        ),
        _ => Err(SsrEmitError {
            message: format!(
                "{context_label} for component `{component_name}` uses an `if`-as-expression \
                 whose arm body is not a single expression statement — the SSR walker only \
                 supports single expression-stmt arms today (the arm's final value is the \
                 last statement's expression). Deferred to Phase 11.7+."
            ),
            context: format!("component `{component_name}` {context_label}"),
        }),
    }
}

fn format_binop_source(op: &crate::ast::BinOpKind) -> &'static str {
    use crate::ast::BinOpKind::*;
    match op {
        Add => "+",
        Sub => "-",
        Mul => "*",
        Div => "/",
        Mod => "%",
        Eq => "==",
        NotEq => "!=",
        Lt => "<",
        LtEq => "<=",
        Gt => ">",
        GtEq => ">=",
        And => "and",
        Or => "or",
        Xor => "xor",
        BitAnd => "&",
        BitOr => "|",
        BitXor => "^",
        Shl => "<<",
        Shr => ">>",
    }
}

fn format_unaryop_source(op: &crate::ast::UnaryOpKind) -> &'static str {
    use crate::ast::UnaryOpKind::*;
    match op {
        Neg => "-",
        Not => "not ",
        BitNot => "~",
    }
}

fn expr_kind_label(expr: &Expr) -> &'static str {
    match expr {
        Expr::Try(_, _) => "Try (`?`)",
        Expr::Await(_, _) => "Await (`.await`)",
        Expr::NamedArg { .. } => "NamedArg (`name: value`)",
        Expr::Match { .. } => "Match",
        Expr::If { .. } => "If-as-expression",
        Expr::StructLit { .. } => "StructLit (`Type { field: value }`)",
        Expr::Slice { .. } => "Slice (`xs[a..b]`)",
        Expr::ListComp { .. } => "ListComp",
        Expr::MapComp { .. } => "MapComp",
        Expr::Bytes(_, _) => "Bytes literal",
        _ => "unsupported",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{expand, parse};

    fn parse_expand(src: &str) -> ExpandedViewFile {
        let raw = parse(src).expect("view::parse");
        expand(&raw).expect("view::expand")
    }

    #[test]
    fn emit_module_ssr_with_components_empty_is_byte_identical() {
        // v0.41.3 — the cross-file entry with an EMPTY imported registry emits
        // byte-for-byte the same as the single-file path (`emit_module_ssr`).
        // Guards byte-compat for every `.fitzv` without cross-file composition.
        let file = parse_expand(
            "component App {\n  state { n: Int = 0 }\n  <template><div><span>{n}</span></div></template>\n}\n",
        );
        let plain = emit_module_ssr(&file).expect("emit");
        let with_empty =
            emit_module_ssr_with_components(&file, &ImportedComponentRegistry::new(), &[])
                .expect("emit");
        assert_eq!(
            plain, with_empty,
            "an empty imported registry must emit byte-identical SSR"
        );
    }

    // ---- Header ----------------------------------------------

    #[test]
    fn emit_module_header_imports_html_and_html_ctor() {
        let file = parse_expand("component Empty { state {} <template><div>hi</div></template> }");
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("from fitz_liveviews import Html, html"),
            "header missing fitz_liveviews import:\n{out}"
        );
    }

    // ---- State type + @live_component ------------------------

    #[test]
    fn emit_state_type_carries_live_component_decorator_and_fields() {
        let file = parse_expand(
            "component MetricTile { state { count: Int = 0 } <template><div>{count}</div></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("@live_component(\"MetricTile\")"),
            "missing @live_component decorator:\n{out}"
        );
        assert!(
            out.contains("type MetricTile {"),
            "missing type declaration:\n{out}"
        );
        assert!(
            out.contains("count: Int = 0"),
            "missing state field:\n{out}"
        );
    }

    // ---- gotcha #1: nested quotes in an attribute interp --------

    #[test]
    fn ssr_emits_attr_interp_with_nested_quotes_v0_37_17() {
        // An attribute interpolation whose expr carries a string
        // literal (i18n in an attribute, `placeholder="{t(locale,
        // "ph.key")}"`) emits the call verbatim in the SSR render fn —
        // no `ph_*` helper needed. `locale` is scoped to `state.locale`
        // and the string literal survives the attribute-value capture.
        let file = parse_expand(
            r#"from i18n import t
component X { state { locale: Str = "es" } <template><input placeholder="{t(locale, "ph.key")}" /></template> }"#,
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("state.locale") && out.contains("\"ph.key\""),
            "esperaba `state.locale` + el string `\"ph.key\"` en el emit SSR:\n{out}"
        );
    }

    // ---- SSR-1: hydration state script -----------------------

    #[test]
    fn ssr1_hydrate_marker_emits_state_script() {
        // A component with the `hydrate` marker gets a trailing
        // <script id="__flv_state_App"> carrying to_json(state), the
        // state payload the WASM adopt reader restores at boot.
        let file = parse_expand(
            "component App hydrate { state { name: Str = \"world\" } <template><div><span>{name}</span></div></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("__flv_state_App"),
            "hydratable component should emit the state script:\n{out}"
        );
        assert!(
            out.contains("{to_json(state)}"),
            "state script should serialize state via to_json(state):\n{out}"
        );
    }

    #[test]
    fn ssr1_no_marker_no_state_script() {
        // Without the `hydrate` marker the SSR output is unchanged
        // (byte-compat with existing components + fitz-liveviews's
        // WS-takeover path, whose HTML diff forbids <script> in the root).
        let file = parse_expand(
            "component App { state { name: Str = \"world\" } <template><div><span>{name}</span></div></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            !out.contains("__flv_state"),
            "non-hydratable component must NOT emit a state script:\n{out}"
        );
        assert!(
            !out.contains("to_json"),
            "non-hydratable component must NOT call to_json:\n{out}"
        );
    }

    // ---- SSR-2: mixed-text hydration markers -----------------

    #[test]
    fn ssr2_mixed_text_in_hydratable_wraps_interp_with_markers() {
        // A hydratable component with mixed static+interpolated text
        // (`Hello, {name}!`) emits the dynamic run wrapped in comment
        // markers so the browser paints three distinct text nodes and the
        // adopt walk maps 1:1 (`Hello, ` | `{name}` | `!`).
        let file = parse_expand(
            "component App hydrate { state { name: Str = \"world\" } \
             <template><p>Hello, {name}!</p></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("Hello, <!--fi-->{state.name}<!--/fi-->!"),
            "mixed text should wrap the interpolation in fi markers:\n{out}"
        );
    }

    #[test]
    fn ssr2_sole_child_interp_no_markers() {
        // A sole-child interpolation (`<span>{name}</span>`) is already its
        // own text node — no coalescing partner — so it stays marker-free,
        // byte-compatible with the slice-1 `hydrate` example.
        let file = parse_expand(
            "component App hydrate { state { name: Str = \"world\" } \
             <template><p>Hello, <span>{name}</span></p></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            !out.contains("<!--fi-->"),
            "sole-child interpolation must NOT get fi markers:\n{out}"
        );
    }

    #[test]
    fn ssr2_non_hydratable_mixed_text_no_markers() {
        // Without the `hydrate` marker the mixed text stays byte-identical
        // (no fi markers) — fitz-liveviews's WS-takeover output is unchanged.
        let file = parse_expand(
            "component App { state { name: Str = \"world\" } \
             <template><p>Hello, {name}!</p></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            !out.contains("<!--fi-->"),
            "non-hydratable mixed text must NOT get fi markers:\n{out}"
        );
    }

    #[test]
    fn ssr2_interp_inside_region_no_markers() {
        // A mixed-text interpolation INSIDE a `{#if}` region is not
        // individually adopted (the client adopts the region opaquely between
        // its `<!--fr-->` anchors), so it must NOT carry fi markers even in a
        // hydratable component. That is SSR-3 territory (region anchors).
        let file = parse_expand(
            "component App hydrate { state { name: Str = \"world\" } \
             <template><div>{#if name != \"\"}<p>Hi {name}!</p>{/if}</div></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            !out.contains("<!--fi-->"),
            "interpolation inside a region must NOT get fi markers:\n{out}"
        );
    }

    // ---- SSR-3: region anchor markers ------------------------

    #[test]
    fn ssr3_top_level_if_in_hydratable_wraps_region_in_fr_markers() {
        // A top-level `{#if}` in a hydratable component paints its content
        // bounded by `<!--fr-->` … `<!--/fr-->` anchors so the client-WASM
        // adopt walk (`__flv_next_comment`) acquires them into the region's
        // `__astart_<r>` / `__aend_<r>` handles.
        let file = parse_expand(
            "component App hydrate { state { name: Str = \"world\" } \
             <template><div>{#if name != \"\"}<p>hi</p>{/if}</div></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("<!--fr-->"),
            "top-level region should open with an fr anchor:\n{out}"
        );
        assert!(
            out.contains("<!--/fr-->"),
            "top-level region should close with a /fr anchor:\n{out}"
        );
    }

    #[test]
    fn ssr3_top_level_for_in_hydratable_wraps_region_in_fr_markers() {
        // A top-level `{#for}` region gets the same anchors — the WASM adopt
        // treats every top-level `{#if}`/`{#for}` as a region.
        let file = parse_expand(
            "component App hydrate { state { items: List<Str> = [\"a\"] } \
             <template><ul>{#for it in items}<li>{it}</li>{/for}</ul></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("<!--fr-->") && out.contains("<!--/fr-->"),
            "top-level `{{#for}}` region should be wrapped in fr anchors:\n{out}"
        );
    }

    #[test]
    fn ssr3_non_hydratable_region_no_fr_markers() {
        // Without the `hydrate` marker the region stays byte-identical (no fr
        // anchors) — fitz-liveviews's WS-takeover output is unchanged.
        let file = parse_expand(
            "component App { state { name: Str = \"world\" } \
             <template><div>{#if name != \"\"}<p>hi</p>{/if}</div></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            !out.contains("<!--fr-->"),
            "non-hydratable region must NOT get fr markers:\n{out}"
        );
    }

    #[test]
    fn ssr3_nested_region_inside_for_no_fr_markers() {
        // The `{#if}` nested inside a `{#for}` body is rebuilt naively on the
        // client (inside the parent region's fragment, no anchors of its own),
        // so it must NOT carry fr markers — only the top-level `{#for}` region
        // is wrapped. Exactly one anchor pair is emitted (the `{#for}`).
        let file = parse_expand(
            "component App hydrate { state { items: List<Str> = [\"a\"]\n name: Str = \"world\" } \
             <template><ul>{#for it in items}{#if it != name}<li>{it}</li>{/if}{/for}</ul></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert_eq!(
            out.matches("<!--fr-->").count(),
            1,
            "exactly one top-level region (the `{{#for}}`) should be wrapped:\n{out}"
        );
        assert_eq!(
            out.matches("<!--/fr-->").count(),
            1,
            "exactly one closing anchor for the top-level region:\n{out}"
        );
    }

    #[test]
    fn ssr3_regions_and_mixed_text_coexist_in_hydratable() {
        // The region anchors (SSR-3) and the mixed-text markers (SSR-2) live
        // side by side in a hydratable component: the greeting keeps its `fi`
        // markers, the region keeps its `fr` anchors, and the region's own
        // interpolation stays marker-free (adopted opaquely).
        let file = parse_expand(
            "component App hydrate { state { name: Str = \"world\" } \
             <template><div><p>Hello, {name}!</p>{#if name != \"\"}<p>Hi {name}!</p>{/if}</div></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("Hello, <!--fi-->{state.name}<!--/fi-->!"),
            "the mixed-text greeting keeps its fi markers:\n{out}"
        );
        assert!(
            out.contains("<!--fr-->") && out.contains("<!--/fr-->"),
            "the region keeps its fr anchors:\n{out}"
        );
        // The region's interpolation (`Hi {name}!`) is adopted opaquely, so it
        // has no fi markers — only the greeting's pair exists.
        assert_eq!(
            out.matches("<!--fi-->").count(),
            1,
            "only the greeting's interpolation carries fi markers:\n{out}"
        );
    }

    #[test]
    fn emit_state_type_supports_multiple_fields_of_different_primitives() {
        let src = r#"component Multi {
  state {
    n: Int = 0
    msg: Str = "hi"
    flag: Bool = false
  }
  <template><div>{n}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(out.contains("n: Int = 0"));
        assert!(out.contains("msg: Str = \"hi\""));
        assert!(out.contains("flag: Bool = false"));
    }

    // ---- Render fn -------------------------------------------

    #[test]
    fn emit_render_fn_uses_component_name_snake_style_suffix() {
        let file = parse_expand(
            "component MetricTile { state { count: Int = 0 } <template><span>{count}</span></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("@render_for(\"MetricTile\")"),
            "missing @render_for decorator:\n{out}"
        );
        assert!(
            out.contains("fn MetricTile_render(state: MetricTile) -> Html {"),
            "render fn signature wrong:\n{out}"
        );
        assert!(
            out.contains("return html(\"\"\""),
            "render fn should use triple-quote html() call:\n{out}"
        );
    }

    #[test]
    fn emit_render_fn_rewrites_field_interpolation_with_state_prefix() {
        let file = parse_expand(
            "component X { state { count: Int = 0 } <template><span>{count}</span></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("<span>{state.count}</span>"),
            "`{{count}}` must be rewritten as `{{state.count}}`:\n{out}"
        );
    }

    #[test]
    fn emit_render_fn_translates_click_attr_to_data_flv_click() {
        let file = parse_expand(
            "component X { state {} <template><button @click=\"tap\">bump</button></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains(r#"data-flv-click="tap""#),
            "`@click` must translate to `data-flv-click`:\n{out}"
        );
        assert!(
            !out.contains("@click="),
            "raw `@click=` must NOT appear in the emitted HTML:\n{out}"
        );
    }

    #[test]
    fn emit_render_fn_carries_static_attrs_verbatim() {
        let file = parse_expand(
            "component X { state {} <template><div class=\"card\" id=\"main\"><span>hi</span></div></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(out.contains(r#"class="card""#));
        assert!(out.contains(r#"id="main""#));
    }

    // ---- Event fn --------------------------------------------

    #[test]
    fn emit_event_fn_signature_matches_fitz_liveviews_contract() {
        let file = parse_expand(
            "component X { state { n: Int = 0 } event reset() { n = 0 } <template><div>{n}</div></template> }",
        );
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains(r#"@on("X", "reset")"#),
            "missing @on decorator:\n{out}"
        );
        assert!(
            out.contains("fn X_reset(state: X, payload: Map<Str, Str>) -> X {"),
            "event fn signature wrong:\n{out}"
        );
    }

    #[test]
    fn emit_event_fn_returns_fresh_struct_literal_with_all_fields() {
        // Two state fields, one event mutates only ONE — the
        // emitted body must return a struct literal that
        // carries both fields (the untouched one via
        // `state.<field>`).
        let src = r#"component X {
  state {
    n: Int = 0
    msg: Str = "hi"
  }
  event bump() { n = 5 }
  <template><div>{n}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("return X {"),
            "must build a fresh struct literal:\n{out}"
        );
        assert!(
            out.contains("n: 5,"),
            "mutated field must use the assigned value:\n{out}"
        );
        assert!(
            out.contains("msg: state.msg,"),
            "untouched field must carry over from `state.<field>`:\n{out}"
        );
    }

    #[test]
    fn emit_event_fn_supports_multiple_mutations_last_write_wins() {
        // Two events with different subsets; also test that
        // an event body with N mutations does the right thing.
        let src = r#"component X {
  state {
    a: Int = 0
    b: Int = 0
  }
  event set_both() {
    a = 1
    b = 2
  }
  <template><div>{a}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(out.contains("a: 1,"));
        assert!(out.contains("b: 2,"));
    }

    #[test]
    fn emit_event_fn_rewrites_bare_state_field_rhs_as_state_prefix() {
        // `a = b` where both are state fields — the RHS `b` must
        // rewrite to `state.b`.
        let src = r#"component X {
  state {
    a: Int = 0
    b: Int = 42
  }
  event copy_b_to_a() { a = b }
  <template><div>{a}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("a: state.b,"),
            "bare RHS `b` must rewrite as `state.b`:\n{out}"
        );
    }

    // Phase 11.6.e — the emitted @on signature has `payload:
    // Map<Str, Str>`, so event body RHS is allowed to read from
    // it directly. The walker's local_scope now contains
    // "payload" so bare `payload["key"]` / `payload.has(...)`
    // pass without the "not a declared state field" rejection.
    #[test]
    fn phase_11_6_e_emit_accepts_payload_index_access_in_event_body_rhs() {
        // Payload indexing survives the walker unchanged (Index
        // over Ident where Ident is in local_scope emits as the
        // bare Fitz source).
        let src = r#"component Widget {
  state { title: Str = "" }
  event set_title() { title = payload["text"] }
  <template><span>{title}</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("title: payload[\"text\"],"),
            "expected `title: payload[\"text\"],` in emitted event fn:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_e_emit_accepts_payload_method_call_in_event_body_rhs() {
        // Method call on payload (`payload.get("author")` style)
        // walks as Call(Field(payload, "get"), ["author"]). The
        // Field object walk resolves `payload` via local_scope.
        // Assigning the result to a `Str` state field via a full
        // string interp (matches how the chat example composes
        // an author-tagged message).
        let src = r#"component Notes {
  state { last_author: Str = "" }
  event tag() { last_author = payload["author"] }
  <template><span>{last_author}</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("last_author: payload[\"author\"],"),
            "expected `last_author: payload[\"author\"],` in emitted source:\n{out}"
        );
    }

    // ---- Rejections (MVP scope guards) -----------------------

    // Phase 11.6.c widened the RHS grammar. `count = count + 1`
    // now emits `count: (state.count + 1i64)` (sort of — the
    // literal is `1` not `1i64` in Fitz source). The old
    // rejection test is inverted below to a positive check.
    #[test]
    fn phase_11_6_c_emit_accepts_binop_rhs_in_event_body() {
        let src = r#"component X {
  state { count: Int = 0 }
  event bump() { count = count + 1 }
  <template><div>{count}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // BinOp lowered with outer parens for precedence safety.
        assert!(
            out.contains("count: (state.count + 1),"),
            "expected `count: (state.count + 1),` in emitted source:\n{out}"
        );
    }

    // Phase 11.6.c inlined `<style scoped>` as a `<style>` tag
    // at the top of the render output. The old rejection is
    // replaced by a positive check.
    #[test]
    fn phase_11_6_c_emit_inlines_scoped_style_at_top_of_render_body() {
        let src = r#"component X {
  state {}
  <template><div>hi</div></template>
  <style scoped>
    .foo { color: red; }
  </style>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // The scoped CSS body carries a `-<scope>` class
        // suffix from 11.3.b's `apply_scope`. Between the
        // `<style>` tag we always emit and that suffix, we
        // can grep-match a stable substring.
        assert!(
            out.contains("<style>"),
            "expected inline `<style>` tag:\n{out}"
        );
        assert!(
            out.contains("</style>"),
            "expected closing `</style>` tag:\n{out}"
        );
        // The scoped CSS body should appear before the template
        // content in the emitted HTML string.
        let style_pos = out.find("<style>").unwrap();
        let template_pos = out.find("<div>hi</div>").unwrap();
        assert!(
            style_pos < template_pos,
            "style tag must appear before template content"
        );
    }

    #[test]
    fn phase_11_6_c_emit_inlines_global_style_with_escaped_braces() {
        let src = r#"component X {
  state {}
  <template><div>hi</div></template>
  <style global>
    body { margin: 0; }
  </style>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(out.contains("<style>"));
        // Global CSS body is passed through with `{` / `}`
        // escaped to `\{` / `\}` so classic Fitz's string
        // interpolation doesn't try to parse the CSS as
        // `{expr}`. See `escape_css_braces_for_fitz_interp`.
        assert!(
            out.contains(r"body \{ margin: 0; \}"),
            "global CSS body must appear with escaped braces:\n{out}"
        );
    }

    // Phase 11.6.c continuation — `{#if}` and `{#for}` now
    // lower to Fitz expression pieces. Old rejection tests
    // inverted below to positive checks. See
    // `phase_11_6_c_cont_*` further down for the detailed
    // shape assertions.
    #[test]
    fn phase_11_6_c_cont_emit_accepts_if_directive() {
        let src = r#"component X {
  state { flag: Bool = false }
  <template><div>{#if flag}<span>yes</span>{/if}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // The `{#if}` directive emits as an if-as-expression
        // piece in the chain-form html(...) call.
        assert!(
            out.contains("if (state.flag)"),
            "expected `if (state.flag)` in emit:\n{out}"
        );
        // Then-branch is a triple-string of the rendered
        // children; else-branch is empty string.
        assert!(
            out.contains("<span>yes</span>"),
            "then-branch content missing:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_c_cont_emit_accepts_for_directive() {
        let src = r#"component X {
  state { xs: List<Int> = [] }
  <template><ul>{#for x in xs}<li>{x}</li>{/for}</ul></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // The `{#for}` directive emits as
        // `__fitz_view_str_join(<iter>.map(fn(x) => <body>))`
        // — verified below.
        assert!(
            out.contains("__fitz_view_str_join(state.xs.map(fn(x) =>"),
            "expected join+map lowering:\n{out}"
        );
        // The `x` binding shadows any same-named state field
        // inside the body: `{x}` stays as `{x}` (not
        // `{state.x}`) inside the closure.
        assert!(
            out.contains("<li>{x}</li>"),
            "closure body must use bare `x`, not `state.x`:\n{out}"
        );
    }

    // Phase 11.6.d — `<Child />` composition (same-file MVP)
    // ---------------------------------------------------------

    #[test]
    fn phase_11_6_d_emit_composes_same_file_child_with_no_props() {
        // Baseline: `<Child />` inside `Parent` lowers to an
        // Expr piece that calls `Child_render(Child { }).raw`
        // and splices the result into the parent's html chain.
        let src = r#"component Parent {
  state {}
  <template><Child /></template>
}
component Child {
  state {}
  <template><span>hi</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).expect("same-file <Child /> composes");
        assert!(
            out.contains("Child_render(Child { }).raw"),
            "parent must call child_render on an empty struct literal:\n{out}"
        );
        // Child_render itself is a fully-emitted render fn.
        assert!(
            out.contains("fn Child_render(state: Child) -> Html {"),
            "child render fn must be emitted:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_d_emit_composes_same_file_child_with_primitive_props() {
        // Props coerce to Fitz literals: Str stays quoted, Int
        // bare, Float keeps `.0`, Bool bare.
        let src = r#"component Parent {
  state {}
  <template>
    <Card title="hello" count="42" rate="1.5" active="true" />
  </template>
}
component Card {
  state { title: Str = "", count: Int = 0, rate: Float = 0.0, active: Bool = false }
  <template><div>{title}: {count}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).expect("prop coercion accepts primitives");
        assert!(
            out.contains(
                "Card_render(Card { title: \"hello\", count: 42, rate: 1.5, active: true }).raw"
            ),
            "child composition must supply each prop as a Fitz literal:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_d_emit_omits_undeclared_props_so_defaults_apply() {
        // Only `title` supplied — `count` defaults to `0`. The
        // emitted struct literal supplies title only, letting
        // classic Fitz's default-application kick in.
        let src = r#"component Parent {
  state {}
  <template><Card title="hi" /></template>
}
component Card {
  state { title: Str = "", count: Int = 7 }
  <template><span>{title}({count})</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).expect("undeclared props use defaults");
        assert!(
            out.contains("Card_render(Card { title: \"hi\" }).raw"),
            "only supplied props must appear in the struct literal:\n{out}"
        );
        assert!(
            !out.contains("count: 0"),
            "un-supplied `count` must NOT appear (default kicks in):\n{out}"
        );
    }

    #[test]
    fn phase_11_6_d_emit_rejects_child_declared_in_a_different_file() {
        // The single-component convenience wrapper does not see
        // siblings, so a `<Child />` reference resolves to only
        // the parent itself → not found → clear cross-file error
        // pointing at 11.6.e.
        let parent_src = r#"component Parent {
  state {}
  <template><Comp /></template>
}"#;
        let file = parse_expand(parent_src);
        let err = emit_module_ssr(&file).unwrap_err();
        assert!(
            err.message.contains("<Comp"),
            "error must cite the child tag:\n{}",
            err.message
        );
        assert!(
            err.message.contains("11.6.e"),
            "error must point to Phase 11.6.e for cross-file:\n{}",
            err.message
        );
    }

    #[test]
    fn phase_11_6_d_emit_rejects_nullable_str_null_literal_and_string_prop() {
        // `Nullable<Str>` with raw `"null"` lowers to Fitz
        // `null`; any other value recurses on `Str` → quoted.
        let src = r#"component Parent {
  state {}
  <template>
    <Widget name="null" other="present" />
  </template>
}
component Widget {
  state { name: Str? = null, other: Str? = null }
  <template><span>{name}</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).expect("Nullable primitives coerce");
        assert!(
            out.contains("Widget_render(Widget { name: null, other: \"present\" }).raw"),
            "nullable str: `null` bare, other value quoted:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_d_composition_forces_chain_form_html_arg() {
        // A pure text template uses pretty triple-string form;
        // adding `<Child />` introduces an Expr piece which
        // forces chain form.
        let src = r#"component Parent {
  state {}
  <template>hello <Child /> world</template>
}
component Child {
  state {}
  <template><i>x</i></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).expect("composes");
        assert!(
            out.contains("return html(\"hello \" + (Child_render(Child { }).raw) + \" world\")"),
            "parent render must use chain form when a child is composed:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_d_module_with_composition_round_trips_through_classic_fitz() {
        // Sanity: the composed output lexes + parses cleanly
        // through classic Fitz, just like every other emitted
        // module. The check is best-effort (the emitted source
        // still needs `Html`/`html` from `fitz_liveviews` to
        // typecheck end-to-end).
        let src = r#"component Parent {
  state {}
  <template><div><Card title="hi" count="7" /></div></template>
}
component Card {
  state { title: Str = "", count: Int = 0 }
  <template><span>{title} ({count})</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).expect("composes");
        let tokens = crate::lexer::tokenize(&out)
            .unwrap_or_else(|e| panic!("emitted source failed classic lex: {e}\n\nSource:\n{out}"));
        crate::parser::parse(tokens).unwrap_or_else(|e| {
            panic!("emitted source failed classic parse: {e}\n\nSource:\n{out}")
        });
    }

    // Phase 11.12 SSR-4 — a bare default `<slot />` is no longer rejected: the
    // render fn gains a `__slot: Str` param and emits it at the slot position.
    #[test]
    fn ssr4_default_slot_emits_slot_param() {
        let src = r#"component X {
  state {}
  <template><div><slot /></div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).expect("default <slot /> now emits");
        assert!(
            out.contains("fn X_render(state: X, __slot: Str) -> Html"),
            "render fn takes __slot param:\n{out}"
        );
        // No fallback → the slot position emits the `__slot` argument directly.
        assert!(
            out.contains("(__slot)"),
            "slot emits the __slot arg:\n{out}"
        );
    }

    // Phase 11.12 SSR-4 — a default `<slot>fallback</slot>` emits the parent
    // content when provided, otherwise the child's own fallback.
    #[test]
    fn ssr4_default_slot_with_fallback_emits_conditional() {
        let src = r#"component X {
  state {}
  <template><div><slot><em>none</em></slot></div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).expect("slot with fallback emits");
        assert!(
            out.contains("if (__slot == \"\") {") && out.contains("} else { __slot }"),
            "slot with fallback emits the conditional:\n{out}"
        );
        assert!(
            out.contains("<em>none</em>"),
            "fallback content is present:\n{out}"
        );
    }

    // Phase 11.12 SSR-4 — named slots are still deferred on the SSR target.
    #[test]
    fn ssr4_named_slot_rejected() {
        let src = r#"component X {
  state {}
  <template><div><slot name="header" /></div></template>
}"#;
        let file = parse_expand(src);
        let err = emit_module_ssr(&file).unwrap_err();
        assert!(err.message.contains("named"), "msg: {}", err.message);
    }

    // Phase 11.12 SSR-4 — the full composition shape: a hydratable root `App`
    // composing a `<Card>content</Card>` whose `Card` declares a `<slot>`. The
    // emit must (a) wrap the child in `<div class="__fitz-child-Card">`, (b) pass
    // the parent-rendered slot content as `Card`'s `__slot` arg, (c) give `App`
    // (root) a state script but NOT `Card` (composed child), and (d) render the
    // parent's `{title}` in the slot content as `state.title` (parent scope).
    #[test]
    fn ssr4_hydratable_composition_wraps_inlines_slot_and_scopes_script() {
        let src = r#"component App hydrate {
  state { title: Str = "d" }
  <template>
    <div class="page">
      <Card>
        <span class="ttl">{title}</span>
      </Card>
    </div>
  </template>
}

component Card {
  state { taps: Int = 0 }
  <template>
    <section class="card">
      <div class="body"><slot /></div>
      <span class="tn">{taps}</span>
    </section>
  </template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).expect("composition + slot emit");

        assert!(
            out.contains("<div class=\\\"__fitz-child-Card\\\">"),
            "hydratable parent wraps the child:\n{out}"
        );
        assert!(
            out.contains("Card_render(Card {") && out.contains(").raw"),
            "child composed via Card_render(...).raw:\n{out}"
        );
        // Card takes a __slot param (it declares a <slot />).
        assert!(
            out.contains("fn Card_render(state: Card, __slot: Str) -> Html"),
            "Card render fn takes __slot:\n{out}"
        );
        // The slot content is rendered in the PARENT scope: `{title}` → state.title.
        assert!(
            out.contains("{state.title}"),
            "slot content interp uses parent state:\n{out}"
        );
        // Root App gets a restore script; composed Card does not.
        assert!(
            out.contains("id=\\\"__flv_state_App\\\">"),
            "root App emits its state script:\n{out}"
        );
        assert!(
            !out.contains("__flv_state_Card"),
            "composed Card must NOT emit its own state script:\n{out}"
        );
        // The whole tree is hydratable → Card's `{taps}` is a plain (sole-child)
        // interp, so no `fi` markers here; but the child render fn still exists.
        assert!(
            out.contains("fn Card_render("),
            "Card render fn emitted:\n{out}"
        );
    }

    // Phase 11.12 SSR-4 — a self-closing `<Card />` where Card has a slot passes
    // `""` so the child falls back to its own `<slot>` fallback.
    #[test]
    fn ssr4_self_closing_child_with_slot_passes_empty_arg() {
        let src = r#"component App hydrate {
  state {}
  <template><div><Card /></div></template>
}

component Card {
  state {}
  <template><section><slot><em>fb</em></slot></section></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).expect("self-closing child with slot");
        // The composition passes an empty-string slot arg.
        assert!(
            out.contains("Card_render(Card { }, \"\").raw")
                || out.contains("Card_render(Card {  }, \"\").raw"),
            "self-closing child passes empty slot arg:\n{out}"
        );
    }

    // Phase 11.12 SSR-4 — a NON-hydratable parent composing a slotless child
    // stays byte-identical to the pre-SSR-4 shape: no wrapper, no slot arg.
    #[test]
    fn ssr4_non_hydratable_slotless_composition_byte_identical() {
        let src = r#"component App {
  state { n: Int = 0 }
  <template><div><Badge count="3" /></div></template>
}

component Badge {
  state { count: Int = 0 }
  <template><span>{count}</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).expect("non-hydratable slotless composition");
        assert!(
            !out.contains("__fitz-child-"),
            "non-hydratable parent emits no wrapper:\n{out}"
        );
        assert!(
            out.contains("Badge_render(Badge { count: 3 }).raw"),
            "slotless child keeps the bare composition shape:\n{out}"
        );
        assert!(
            !out.contains("__flv_state_"),
            "non-hydratable tree emits no state script:\n{out}"
        );
    }

    // Phase 11.12 SSR-4 — providing slot content to a child with no default
    // `<slot />` is rejected at emit time (the checker also guards it).
    #[test]
    fn ssr4_slot_content_without_child_slot_rejected() {
        let src = r#"component App hydrate {
  state {}
  <template><div><Card><span>x</span></Card></div></template>
}

component Card {
  state {}
  <template><section>no slot</section></template>
}"#;
        let file = parse_expand(src);
        let err = emit_module_ssr(&file).unwrap_err();
        assert!(
            err.message.contains("no default `<slot />`"),
            "msg: {}",
            err.message
        );
    }

    #[test]
    fn emit_rejects_non_state_ident_in_template_interpolation() {
        // `{other}` where `other` is not a state field. Phase
        // 11.6.c's walker rejects with a 11.7+ pointer (needs
        // module-loader scope resolution to know if it's a
        // free variable in the caller's scope).
        let src = r#"component X {
  state { count: Int = 0 }
  <template><span>{other}</span></template>
}"#;
        let file = parse_expand(src);
        let err = emit_module_ssr(&file).unwrap_err();
        assert!(
            err.message.contains("not a declared state field"),
            "msg: {}",
            err.message
        );
        assert!(err.message.contains("11.7"), "msg: {}", err.message);
    }

    // ---- End-to-end round-trip -------------------------------

    #[test]
    fn emit_output_round_trips_through_classic_fitz_lexer_and_parser() {
        // The emitted Fitz source must lex + parse cleanly.
        // This is the acceptance criterion for 11.6.b: the SSR
        // emitter produces code that CLASSIC Fitz accepts as
        // input. Type-checker validation is a bonus — the
        // emitted references to fitz_liveviews types would
        // fail without the dep loaded, so we ONLY validate
        // lex + parse here.
        let src = r#"component Score {
  state { points: Int = 0 }
  event zero() { points = 0 }
  event ten() { points = 10 }
  <template>
    <div>
      <span>{points}</span>
      <button @click="zero">zero</button>
      <button @click="ten">set 10</button>
    </div>
  </template>
}"#;
        let file = parse_expand(src);
        let emitted = emit_module_ssr(&file).unwrap();

        let tokens = crate::lexer::tokenize(&emitted).unwrap_or_else(|e| {
            panic!("emitted source failed to lex:\n{emitted}\n--- err ---\n{e}")
        });
        let _program = crate::parser::parse(tokens).unwrap_or_else(|e| {
            panic!("emitted source failed to parse:\n{emitted}\n--- err ---\n{e}")
        });
    }

    #[test]
    fn emit_component_single_matches_module_wrapper() {
        // `emit_component_ssr` (single) and `emit_module_ssr`
        // (with a 1-component file) produce the same output —
        // the module entry point is just a convenience wrapper.
        let src = "component X { state { n: Int = 0 } <template><div>{n}</div></template> }";
        let file = parse_expand(src);
        let via_module = emit_module_ssr(&file).unwrap();
        let via_component = emit_component_ssr(&file.components[0]).unwrap();
        assert_eq!(via_module, via_component);
    }

    // ---- Phase 11.6.c — widened grammar coverage -------------

    #[test]
    fn phase_11_6_c_emit_accepts_arithmetic_rhs_multiple_ops() {
        let src = r#"component X {
  state {
    a: Int = 0
    b: Int = 0
  }
  event compute() {
    a = a + b * 2
    b = a - 1
  }
  <template><div>{a}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // Precedence-preserving parens on every BinOp node.
        assert!(
            out.contains("a: (state.a + (state.b * 2)),"),
            "expected precedence-preserving parens:\n{out}"
        );
        assert!(out.contains("b: (state.a - 1),"));
    }

    #[test]
    fn phase_11_6_c_emit_accepts_str_interp_rhs_with_field_rewrite() {
        let src = r#"component X {
  state {
    count: Int = 0
    msg: Str = ""
  }
  event describe() { msg = "count is {count}" }
  <template><div>{msg}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // The `{count}` inside the interpolation must rewrite
        // to `{state.count}`.
        assert!(
            out.contains("msg: \"count is {state.count}\","),
            "expected StrInterp with state-field rewrite:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_c_emit_accepts_method_call_rhs() {
        // The view lexer accepts `.` inside event body raw-capture
        // since the 11.6.c continuation (Token::Dot). This test
        // exercises the SSR walker's method-call support via a
        // natural `.fitzv` source — the AST-construction dance is
        // no longer required.
        let src = r#"component X {
  state { msg: Str = "hi" }
  event shout() { msg = msg.upper() }
  <template><div>{msg}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("msg: state.msg.upper(),"),
            "expected method call in emitted source:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_c_emit_accepts_richer_template_interpolation() {
        let src = r#"component X {
  state { count: Int = 0 }
  <template><span>Count is {count + 1}</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // `{count + 1}` in the template becomes `{(state.count + 1)}`
        // inside the emitted HTML string.
        assert!(
            out.contains("Count is {(state.count + 1)}"),
            "expected rich template interp:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_c_emit_accepts_field_access_in_template_interp() {
        let src = r#"component X {
  state { name: Str = "world" }
  <template><span>Hi {name.upper()}</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("Hi {state.name.upper()}"),
            "expected method call on state field:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_c_emit_accepts_arrow_closure_rhs() {
        // Same story as the method-call test: since the 11.6.c
        // continuation added Token::Dot to the view lexer, we
        // can exercise the closure lowering via a natural `.fitzv`
        // source (no AST-construction dance).
        let src = r#"component X {
  state { xs: List<Int> = [] }
  event bump_all() { xs = xs.map(fn(x) => x + 1) }
  <template><div>hi</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("xs: state.xs.map(fn(x) => (x + 1)),"),
            "expected arrow closure passed to .map():\n{out}"
        );
    }

    #[test]
    fn phase_11_6_c_cont_module_header_emits_str_join_helper() {
        // The `__fitz_view_str_join(xs: List<Str>) -> Str`
        // helper is prepended to every emitted module (dead
        // code when unused). Verified once here so the shape
        // is regression-guarded.
        let src = "component X { state {} <template><div>hi</div></template> }";
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("fn __fitz_view_str_join(xs: List<Str>) -> Str {"),
            "join helper signature missing:\n{out}"
        );
        assert!(
            out.contains("for x in xs"),
            "join helper loop missing:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_c_round_trip_end_to_end_with_widened_grammar() {
        // Real fixture exercising the widened grammar end-to-end.
        // Must lex + parse through classic Fitz cleanly.
        let src = r#"component Counter {
  state {
    count: Int = 0
    label: Str = "Count"
  }
  event increment() { count = count + 1 }
  event decrement() { count = count - 1 }
  event reset() { count = 0 }
  <template>
    <div class="counter">
      <span class="label">{label}: {count}</span>
      <button @click="decrement">-</button>
      <button @click="increment">+</button>
      <button @click="reset">reset</button>
    </div>
  </template>
  <style scoped>
    .counter { display: flex; gap: 8px; }
  </style>
}"#;
        let file = parse_expand(src);
        let emitted = emit_module_ssr(&file).unwrap();

        let tokens = crate::lexer::tokenize(&emitted).unwrap_or_else(|e| {
            panic!("emitted source failed to lex:\n{emitted}\n--- err ---\n{e}")
        });
        let _program = crate::parser::parse(tokens).unwrap_or_else(|e| {
            panic!("emitted source failed to parse:\n{emitted}\n--- err ---\n{e}")
        });

        // Sanity: BinOp arithmetic emitted correctly.
        assert!(emitted.contains("count: (state.count + 1),"));
        assert!(emitted.contains("count: (state.count - 1),"));
        // Sanity: `{label}: {count}` template interp with both
        // state fields rewritten.
        assert!(emitted.contains("{state.label}: {state.count}"));
        // Sanity: scoped `<style>` inline at top of render.
        assert!(emitted.contains("<style>"));
    }

    // ---- Phase 11.6.c continuation — template directives ----

    #[test]
    fn phase_11_6_c_cont_emit_if_with_else_branch() {
        let src = r#"component X {
  state { flag: Bool = false }
  <template>{#if flag}<span>on</span>{#else}<span>off</span>{/if}</template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(out.contains("if (state.flag)"), "cond missing:\n{out}");
        assert!(out.contains("<span>on</span>"), "then missing:\n{out}");
        assert!(out.contains("<span>off</span>"), "else missing:\n{out}");
    }

    #[test]
    fn phase_11_6_c_cont_emit_if_without_else_uses_empty_string() {
        let src = r#"component X {
  state { flag: Bool = false }
  <template>{#if flag}<span>on</span>{/if}</template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // No `{#else}` → else-branch is empty string literal.
        assert!(
            out.contains("} else { \"\" }"),
            "expected `else {{ \"\" }}` for no-else case:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_c_cont_for_body_uses_bare_var_not_state() {
        // Regression: inside `{#for x in state.xs}...{x}...{/for}`,
        // the `x` interpolation must emit as bare `{x}` (closure
        // param), NOT `{state.x}` (state-field rewrite would be
        // wrong).
        let src = r#"component X {
  state { items: List<Str> = [] }
  <template>{#for name in items}<li>{name}</li>{/for}</template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("state.items.map(fn(name) =>"),
            "iter should use `state.items.map(fn(name) => ...)`:\n{out}"
        );
        // `{name}` inside body stays bare — that's the closure
        // param, not a state field.
        assert!(
            out.contains("<li>{name}</li>"),
            "closure body must use bare `name`, not `state.name`:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_c_cont_directive_bearing_module_round_trips_through_classic_fitz() {
        // End-to-end: `.fitzv` with both `{#if}` and `{#for}` +
        // BinOp arithmetic + template interp + scoped style
        // emits classic Fitz source that lexes + parses cleanly.
        let src = r#"component TodoList {
  state {
    items: List<Str> = []
    show_empty: Bool = true
  }
  <template>
    <div>
      {#if show_empty}<p>(list is empty when hidden)</p>{/if}
      <ul>
        {#for name in items}<li>{name}</li>{/for}
      </ul>
    </div>
  </template>
  <style scoped>
    ul { list-style: none; }
  </style>
}"#;
        let file = parse_expand(src);
        let emitted = emit_module_ssr(&file).unwrap();
        // Lex + parse the emitted source through classic Fitz.
        let tokens = crate::lexer::tokenize(&emitted).unwrap_or_else(|e| {
            panic!("emitted source failed to lex:\n{emitted}\n--- err ---\n{e}")
        });
        let _program = crate::parser::parse(tokens).unwrap_or_else(|e| {
            panic!("emitted source failed to parse:\n{emitted}\n--- err ---\n{e}")
        });

        // Shape sanity.
        assert!(emitted.contains("fn __fitz_view_str_join("));
        assert!(emitted.contains("if (state.show_empty)"));
        assert!(emitted.contains("__fitz_view_str_join(state.items.map(fn(name) =>"));
        assert!(emitted.contains("<style>"));
    }

    // ---- Phase 11.6.e — Event body widening -----------------
    //
    // Event bodies now accept:
    // - `let x = ...` bindings (assign to non-state-field
    //   idents). Wide path.
    // - `if (cond) { <body> }` guards at stmt level, arbitrarily
    //   nested. Wide path.
    // - `Expr::If` on the RHS of a `let` / mutation. Walker.
    // - `Expr::StructLit` on any RHS. Walker.
    //
    // Trivial bodies (linear state-field mutations only) keep
    // the compact `return X { <field>: <rhs>, ... }` shape.

    #[test]
    fn phase_11_6_e_widened_body_primes_shadow_locals_from_state() {
        // The presence of a non-state-field `let` binding
        // forces the wide path. The emitter primes a shadow
        // local for every state field, so subsequent
        // mutations to state fields flow through the
        // reassignable local (not `state.<field>`, which is
        // read-only).
        let src = r#"component X {
  state {
    count: Int = 0
    msg: Str = "hi"
  }
  event bump() {
    let bumped = count + 1
    count = bumped
  }
  <template><div>{count}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // Prime lines exist.
        assert!(
            out.contains("let count = state.count"),
            "widened emit must prime `count` shadow from state:\n{out}"
        );
        assert!(
            out.contains("let msg = state.msg"),
            "widened emit must prime `msg` shadow from state:\n{out}"
        );
        // Let binding + state-field mutation via shadow.
        assert!(
            out.contains("let bumped = (count + 1)"),
            "expected `let bumped = (count + 1)` in widened body:\n{out}"
        );
        assert!(
            out.contains("count = bumped"),
            "expected `count = bumped` mutation via shadow:\n{out}"
        );
        // Return reads back from shadows for every field.
        assert!(
            out.contains("count: count,"),
            "widened return must read from shadow local `count`:\n{out}"
        );
        assert!(
            out.contains("msg: msg,"),
            "widened return must read from shadow local `msg` (unmutated):\n{out}"
        );
    }

    #[test]
    fn phase_11_6_e_widened_body_lowers_if_guard_at_stmt_level() {
        // The canonical `if (payload.has(...)) { <mutation> }`
        // guard. Lowers to Fitz `if (payload.has("t")) { title
        // = payload["t"] }` at stmt level.
        let src = r#"component Widget {
  state { title: Str = "" }
  event set_title() {
    if (payload.has("t")) {
      title = payload["t"]
    }
  }
  <template><span>{title}</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("let title = state.title"),
            "wide emit must prime `title`:\n{out}"
        );
        assert!(
            out.contains(r#"if (payload.has("t")) {"#),
            "expected `if (payload.has(\"t\")) {{` at stmt level:\n{out}"
        );
        assert!(
            out.contains(r#"title = payload["t"]"#),
            "expected shadow mutation inside if arm:\n{out}"
        );
        assert!(
            out.contains("title: title,"),
            "widened return must read from shadow:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_e_widened_body_lowers_nested_if_guards() {
        // Chat's `send_message` shape (simplified). Both
        // guards must lower, mutation reaches the shadow.
        let src = r#"component Chat {
  state { last_msg: Str = "" }
  event send() {
    if (payload.has("author")) {
      if (payload.has("text")) {
        last_msg = payload["text"]
      }
    }
  }
  <template><div>{last_msg}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains(r#"if (payload.has("author"))"#),
            "outer if must lower:\n{out}"
        );
        assert!(
            out.contains(r#"if (payload.has("text"))"#),
            "inner if must lower:\n{out}"
        );
        assert!(
            out.contains(r#"last_msg = payload["text"]"#),
            "inner mutation must reach shadow:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_e_widened_body_lowers_if_as_expression_in_let_rhs() {
        // Kanban's canonical `card_editor_save` shape:
        //   let new_text = if (payload.has("text")) {
        //     payload["text"]
        //   } else {
        //     text
        //   }
        //   text = new_text
        // The `if`-as-expression on the RHS of `let` walks
        // through the widened walker's Expr::If arm.
        let src = r#"component CardEditor {
  state {
    is_editing: Bool = false
    text: Str = ""
  }
  event save() {
    let new_text = if (payload.has("text")) { payload["text"] } else { text }
    is_editing = false
    text = new_text
  }
  <template><form></form></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // Prime shadows.
        assert!(out.contains("let is_editing = state.is_editing"));
        assert!(out.contains("let text = state.text"));
        // `let new_text = if(...) { ... } else { ... }`.
        assert!(
            out.contains(
                r#"let new_text = if (payload.has("text")) { payload["text"] } else { text }"#
            ),
            "expected let-with-if-as-expression RHS:\n{out}"
        );
        // Mutations.
        assert!(
            out.contains("is_editing = false"),
            "expected `is_editing = false` mutation:\n{out}"
        );
        assert!(
            out.contains("text = new_text"),
            "expected `text = new_text` mutation:\n{out}"
        );
        // Return from shadows for every state field.
        assert!(out.contains("is_editing: is_editing,"));
        assert!(out.contains("text: text,"));
    }

    #[test]
    fn phase_11_6_e_walker_accepts_struct_literal_in_wide_body_let() {
        // Chat's inline `Message { author: ..., text: ... }`
        // construction in an event body. Wide path (let
        // binding forces it).
        let src = r#"component Chat {
  state { last_author: Str = "" }
  event send() {
    let m = Message { author: payload["a"], text: payload["b"] }
    last_author = m.author
  }
  <template><div>{last_author}</div></template>
}"#;
        // The classic checker will complain about the free
        // ident `Message` (it isn't imported) — but the emit
        // pass runs even on programs with checker errors,
        // and the walker only cares about AST shape. The
        // view checker's own error path is a separate
        // pipeline; we call the emitter directly so we can
        // observe the emitted shape without relying on the
        // checker's approval.
        let raw = parse(src).expect("view::parse");
        let file = expand(&raw).expect("view::expand");
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains(r#"let m = Message { author: payload["a"], text: payload["b"] }"#),
            "expected struct-lit `let m = Message {{ ... }}`:\n{out}"
        );
        assert!(
            out.contains("last_author = m.author"),
            "expected `last_author = m.author` mutation:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_e_walker_accepts_struct_literal_in_trivial_body_rhs() {
        // StructLit on the RHS of a trivial state-field
        // mutation. Trivial path — the walker widens but the
        // emit shape stays compact.
        let src = r#"component Chat {
  state { title: Str = "" }
  event tag() {
    title = Message { author: payload["a"], text: payload["b"] }.author
  }
  <template><span>{title}</span></template>
}"#;
        let raw = parse(src).expect("view::parse");
        let file = expand(&raw).expect("view::expand");
        let out = emit_module_ssr(&file).unwrap();
        // Trivial path: compact `title: <rhs>,` shape.
        assert!(
            out.contains(r#"title: Message { author: payload["a"], text: payload["b"] }.author,"#),
            "expected compact trivial shape with struct-lit RHS:\n{out}"
        );
        // No shadow-local prime — trivial path stays compact.
        assert!(
            !out.contains("let title = state.title"),
            "trivial body must NOT prime shadow locals:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_e_trivial_body_still_uses_compact_shape() {
        // Regression: trivial bodies (linear state-field
        // mutations only) keep the compact
        // `return X { <field>: <rhs>, ... }` shape from
        // 11.6.b/c. No shadow-local prime.
        let src = r#"component X {
  state {
    a: Int = 0
    b: Int = 0
  }
  event reset() {
    a = 0
    b = 0
  }
  <template><div>{a}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            !out.contains("let a = state.a"),
            "trivial body should NOT prime shadow locals:\n{out}"
        );
        assert!(
            out.contains("a: 0,"),
            "compact trivial shape must have `a: 0,`:\n{out}"
        );
        assert!(
            out.contains("b: 0,"),
            "compact trivial shape must have `b: 0,`:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_e_if_arm_scope_does_not_leak_after_arm_closes() {
        // Local `let` bindings inside an if arm live only
        // inside that arm. After the arm closes, referencing
        // the arm-local from the outer scope would fail
        // classic Fitz's checker. The emitter models this by
        // truncating `local_scope` back on arm exit.
        //
        // Constructing a test that OBSERVES the truncation
        // is hard because a body that references a leaked
        // ident post-arm would be an emit-time success + a
        // classic-Fitz-checker failure (which we don't
        // exercise here). Instead, the assertion below
        // sanity-checks that the arm-local binding IS
        // emitted inside the arm, and that the mutation
        // AFTER the arm still works — i.e. the arm's own
        // scope doesn't shadow state fields.
        let src = r#"component X {
  state { n: Int = 0 }
  event bump() {
    if (payload.has("k")) {
      let x = 5
      n = x
    }
    n = n + 1
  }
  <template><div>{n}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // Arm-local `let x = 5` inside the if.
        assert!(
            out.contains("let x = 5"),
            "arm-local `let x = 5` must be emitted:\n{out}"
        );
        // Arm mutation reaches shadow.
        assert!(
            out.contains("n = x"),
            "arm mutation `n = x` must be emitted:\n{out}"
        );
        // Post-arm mutation using the shadow local `n`.
        assert!(
            out.contains("n = (n + 1)"),
            "post-arm mutation `n = (n + 1)` must be emitted:\n{out}"
        );
    }

    #[test]
    fn phase_11_6_e_widened_body_rejects_bare_expression_stmt() {
        // A bare expression stmt that is NOT an if guard NOR a
        // method call on a shadow-local state field must be
        // rejected. Post §9.cc V-6, method calls on shadow locals
        // like `payload.has("k")` or `<field>.push(x)` ARE
        // accepted — so this test uses a truly-bare-arithmetic
        // expression stmt (`n + 1` with the result discarded) to
        // exercise the fallthrough rejection path. Force wide-body
        // via a `let` binding first.
        let src = r#"component X {
  state { n: Int = 0 }
  event bump() {
    let dummy = 1
    n + 1
  }
  <template><div>{n}</div></template>
}"#;
        let raw = parse(src).expect("view::parse");
        let file = expand(&raw).expect("view::expand");
        let err = emit_module_ssr(&file).unwrap_err();
        assert!(
            err.message.contains("bare expression statement"),
            "error must cite bare expression stmt:\n{}",
            err.message
        );
        assert!(
            err.message.contains("11.7+"),
            "error must point at Phase 11.7+:\n{}",
            err.message
        );
    }

    #[test]
    fn phase_11_6_e_widened_body_rejects_index_target_assign() {
        // `xs[0] = value` requires nested-mutation support
        // outside the shadow-local model. Force wide path
        // via a `let` binding, then Index-target assign.
        let src = r#"component X {
  state { xs: List<Int> = [] }
  event set_first() {
    let n = 5
    xs[0] = n
  }
  <template><div>hi</div></template>
}"#;
        let raw = parse(src).expect("view::parse");
        let file = expand(&raw).expect("view::expand");
        let err = emit_module_ssr(&file).unwrap_err();
        assert!(
            err.message.contains("index"),
            "expected `index` in error msg:\n{}",
            err.message
        );
        assert!(
            err.message.contains("11.7+"),
            "error must point at Phase 11.7+:\n{}",
            err.message
        );
    }

    #[test]
    fn phase_11_6_e_widened_body_round_trips_through_classic_fitz() {
        // Kanban's canonical `card_editor_save` shape lexes
        // + parses cleanly through classic Fitz after the
        // SSR emitter runs. This is the acceptance
        // criterion: whatever we emit, classic Fitz reads.
        let src = r#"component CardEditor {
  state {
    is_editing: Bool = false
    text: Str = ""
  }
  event save() {
    let new_text = if (payload.has("text")) { payload["text"] } else { text }
    is_editing = false
    text = new_text
  }
  event start() {
    let seed = if (payload.has("current_title")) { payload["current_title"] } else { text }
    is_editing = true
    text = seed
  }
  event cancel() {
    is_editing = false
    text = ""
  }
  <template>
    <div class="editor">
      <button @click="start">edit</button>
    </div>
  </template>
}"#;
        let file = parse_expand(src);
        let emitted = emit_module_ssr(&file).unwrap();
        let tokens = crate::lexer::tokenize(&emitted).unwrap_or_else(|e| {
            panic!("widened emit failed classic lex:\n---\n{emitted}\n---\nerr: {e}")
        });
        crate::parser::parse(tokens).unwrap_or_else(|e| {
            panic!("widened emit failed classic parse:\n---\n{emitted}\n---\nerr: {e}")
        });
    }

    // -----------------------------------------------------------------------
    // §9.cc V-6 — Accept bare method calls on shadow-local state fields
    // -----------------------------------------------------------------------
    //
    // Fitz `List<T>` is `Arc<Mutex<Vec<T>>>` per F17. Mutation via
    // `.push(x)` on the shadow local `<field>` mutates the same
    // underlying Arc as `state.<field>`. The struct-lit return then
    // re-packages the (now-mutated) same Arc, preserving §9.aa's
    // shadow-local return contract WITHOUT needing an immutable-
    // return builtin like `List<T>.appended(x)`.

    #[test]
    fn v6_widened_body_accepts_bare_push_on_shadow_local_list() {
        // The canonical chat-migration pattern: `messages.push(new)` on
        // a shadow-local List<T>. Pre-fix this errored with "bare
        // expression statement". Post-fix accepts + emits verbatim.
        let src = r#"component ChatRoom {
  state { messages: List<Str> = [] }
  event append() {
    let dummy = 1
    messages.push("hi")
  }
  <template><div>x</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // The emitted event fn primes `let messages = state.messages`
        // + walks the body verbatim. `messages.push("hi")` should
        // appear as-is on its own line.
        assert!(
            out.contains("messages.push(\"hi\")"),
            "expected `messages.push(\"hi\")` emitted verbatim:\n{out}"
        );
        // Struct-lit return re-packages messages (mutated shadow local).
        assert!(
            out.contains("messages: messages,") || out.contains("messages: messages\n"),
            "expected return struct lit to re-package messages:\n{out}"
        );
    }

    #[test]
    fn v6_widened_body_accepts_bare_push_within_nested_if_guards() {
        // Chat migration's exact 2-level nested guard shape post
        // §9.aa event-body widening. The `.push()` sits in the
        // innermost arm.
        let src = r#"component ChatRoom {
  state { messages: List<Str> = [] }
  event send_message() {
    if (payload.has("author")) {
      if (payload.has("text")) {
        let text = payload["text"]
        messages.push(text)
      }
    }
  }
  <template><div>x</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // Emitted body should contain both if guards + the push.
        assert!(out.contains("if ("), "expected if guard in emit:\n{out}");
        assert!(
            out.contains("messages.push(text)"),
            "expected `messages.push(text)` verbatim inside nested if:\n{out}"
        );
        // Round-trip: emitted classic Fitz must lex + parse clean.
        let tokens = crate::lexer::tokenize(&out)
            .unwrap_or_else(|e| panic!("V-6 emit failed classic lex:\n---\n{out}\n---\nerr: {e}"));
        crate::parser::parse(tokens).unwrap_or_else(|e| {
            panic!("V-6 emit failed classic parse:\n---\n{out}\n---\nerr: {e}")
        });
    }

    #[test]
    fn v6_widened_body_accepts_bare_method_call_on_payload_shadow() {
        // `payload` is a shadow local per §9.z (populated in the
        // walker's local_scope for event bodies). A bare
        // `payload.has("k")` — which pre-V-6 was rejected as "bare
        // expression stmt" — now accepted (though semantically a
        // no-op since return value is discarded).
        let src = r#"component X {
  state { n: Int = 0 }
  event bump() {
    let dummy = 1
    payload.has("k")
  }
  <template><div>{n}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file)
            .expect("V-6 accepts bare method call on `payload` (a shadow local per §9.z)");
        assert!(
            out.contains("payload.has(\"k\")"),
            "expected `payload.has(\"k\")` emitted verbatim:\n{out}"
        );
    }

    #[test]
    fn v6_widened_body_rejects_bare_method_call_on_non_shadow_ident() {
        // A bare method call on a Non-shadow-local ident MUST still
        // be rejected. `some_fn` is not a state field and not a
        // shadow local — the callee's base `some_fn` fails the
        // `local_scope` check. Rejection is the safe behavior.
        let src = r#"component X {
  state { n: Int = 0 }
  event bump() {
    let dummy = 1
    unknown_var.frobnicate("x")
  }
  <template><div>{n}</div></template>
}"#;
        let file = parse_expand(src);
        let err = emit_module_ssr(&file).unwrap_err();
        assert!(
            err.message.contains("not a method call on a shadow-local"),
            "expected V-6 rejection message on non-shadow base:\n{}",
            err.message
        );
        assert!(
            err.message.contains("11.7+"),
            "error must point at Phase 11.7+:\n{}",
            err.message
        );
    }

    #[test]
    fn v6_widened_body_rejects_method_chain_on_shadow_local() {
        // Method CHAINS on shadow locals (`messages.map(f).filter(g)`)
        // are rejected because the callee's base is a Call, not an
        // Ident. Rejection preserves the "single-level method call"
        // constraint of V-6 MVP.
        let src = r#"component X {
  state { messages: List<Str> = [] }
  event process() {
    let dummy = 1
    messages.map(fn(m) => m).filter(fn(m) => true)
  }
  <template><div>x</div></template>
}"#;
        let file = parse_expand(src);
        let err = emit_module_ssr(&file).unwrap_err();
        // Base of the outer .filter(...) is Call, not Ident → V-6 rejects.
        assert!(
            err.message.contains("not a method call on a shadow-local"),
            "expected V-6 rejection on method chain (base is Call, not Ident):\n{}",
            err.message
        );
    }

    #[test]
    fn v6_regression_events_without_bare_calls_still_type_check() {
        // Regression: the V-6 fix only ADDS a new acceptance path;
        // events that don't use bare method calls (only re-assigns +
        // if guards + let) must still work bit-for-bit as before.
        let src = r#"component Counter {
  state { count: Int = 0 }
  event increment() {
    count = count + 1
  }
  event conditional_reset() {
    if (payload.has("hard")) {
      count = 0
    }
  }
  <template><div>{count}</div></template>
}"#;
        let file = parse_expand(src);
        let out =
            emit_module_ssr(&file).expect("V-6 must not regress plain re-assign + if guard events");
        // `increment` is single-assign → trivial body path emits
        // direct struct-lit return (`return Counter { count: state.count + 1 }`),
        // NOT the widened shadow-local shape.
        assert!(
            out.contains("count: (state.count + 1)") || out.contains("count: state.count + 1"),
            "increment must use trivial body emit (`count: state.count + 1` or with parens):\n{out}"
        );
        // `conditional_reset` has an `if` guard → widened body path
        // with shadow-local prime + assign inside arm.
        assert!(
            out.contains("if ("),
            "conditional_reset body must emit if guard (widened path):\n{out}"
        );
        assert!(
            out.contains("let count = state.count"),
            "conditional_reset widened path must prime shadow local:\n{out}"
        );
    }

    // -----------------------------------------------------------------------
    // §9.dd V-3 + V-5 — emit `from X import Y` at top of transformed source
    // -----------------------------------------------------------------------

    #[test]
    fn v3_from_import_emitted_verbatim_at_top_of_transformed_source() {
        // Canonical case: `.fitzv` declares `from message import Message`
        // + uses `List<Message>` in state. Transformed source must
        // include the `from message import Message` line so the classic
        // Fitz loader resolves the nominal from the sibling module.
        let src = r#"from message import Message

component ChatRoom {
  state { messages: List<Message> = [] }
  <template><div>chat</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("from message import Message"),
            "expected `from message import Message` in output:\n{out}"
        );
        // Assert order: the user import should come AFTER the framework
        // import (`from fitz_liveviews import Html, html`) but BEFORE
        // the `@live_component` block.
        let framework_idx = out
            .find("from fitz_liveviews import Html, html")
            .expect("framework import present");
        let user_idx = out.find("from message import Message").unwrap();
        let component_idx = out
            .find("@live_component")
            .expect("component block present");
        assert!(
            framework_idx < user_idx,
            "user import must come AFTER framework import: framework={framework_idx}, user={user_idx}"
        );
        assert!(
            user_idx < component_idx,
            "user import must come BEFORE component block: user={user_idx}, component={component_idx}"
        );
    }

    #[test]
    fn v3_multi_name_from_import_emits_comma_separated() {
        let src = r#"from users import User, Post, Comment

component X {
  state { count: Int = 0 }
  <template><div>{count}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("from users import User, Post, Comment"),
            "expected multi-name import verbatim:\n{out}"
        );
    }

    #[test]
    fn v3_dotted_path_from_import_emitted_verbatim() {
        let src = r#"from utils.shared import Widget

component X {
  state { count: Int = 0 }
  <template><div>{count}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("from utils.shared import Widget"),
            "expected dotted-path import verbatim:\n{out}"
        );
    }

    #[test]
    fn v3_no_user_imports_no_extra_blank_lines_regression() {
        // Regression: files without any user imports must NOT emit
        // extra blank lines in place of the imports block. Counter/
        // MetricTile shape must be preserved.
        let src = r#"component Counter {
  state { count: Int = 0 }
  <template><div>{count}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // Just check that user-import lines aren't present.
        assert!(
            !out.contains("from message"),
            "expected no user imports emitted:\n{out}"
        );
        assert!(
            !out.contains("from utils"),
            "expected no user imports emitted:\n{out}"
        );
    }

    #[test]
    fn v3_multiple_from_imports_emit_in_order() {
        let src = r#"from message import Message
from user import User

component X {
  state { count: Int = 0 }
  <template><div>{count}</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        let msg_idx = out.find("from message import Message").unwrap();
        let user_idx = out.find("from user import User").unwrap();
        assert!(
            msg_idx < user_idx,
            "imports must emit in source order:\n{out}"
        );
    }

    #[test]
    fn v3_emitted_source_with_imports_round_trips_through_classic_fitz() {
        // Acceptance criterion: whatever we emit, classic Fitz reads.
        // Post V-3 + V-5 fixes, the emitted source with user imports
        // must lex + parse cleanly through classic Fitz.
        let src = r#"from message import Message

component ChatRoom {
  state { messages: List<Message> = [] }
  event send() {
    if (payload.has("author")) {
      if (payload.has("text")) {
        let author = payload["author"]
        let text = payload["text"]
        messages.push(Message { author: author, text: text })
      }
    }
  }
  <template><div>chat</div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        let tokens = crate::lexer::tokenize(&out).unwrap_or_else(|e| {
            panic!("emitted source failed classic lex:\n---\n{out}\n---\nerr: {e}")
        });
        crate::parser::parse(tokens).unwrap_or_else(|e| {
            panic!("emitted source failed classic parse:\n---\n{out}\n---\nerr: {e}")
        });
    }

    #[test]
    fn phase_11_6_c_cont_all_text_template_still_uses_pretty_triple_string() {
        // Regression: a template with no directives should still
        // use the pretty triple-string form for the html(...)
        // argument (not chain form).
        let src = r#"component X {
  state { count: Int = 0 }
  <template><div><span>{count}</span></div></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // Look for the triple-string form.
        assert!(
            out.contains("return html(\"\"\"<div><span>{state.count}</span></div>\"\"\")"),
            "expected pretty triple-string form:\n{out}"
        );
    }

    // ---------------------------------------------------------------------
    // K-3 — SSR coerce_child_prop_raw_value_to_fitz_literal for List<T>
    // ---------------------------------------------------------------------

    #[test]
    fn k3_ssr_coerce_helper_list_str_produces_bracket_list_of_string_literals() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::Named("Str".into())],
        };
        let lit = coerce_child_prop_raw_value_to_fitz_literal("a,b,c", &ty, "Child", "tags")
            .expect("List<Str> coerces");
        assert!(lit.starts_with('['), "got: {lit}");
        assert!(lit.ends_with(']'), "got: {lit}");
        assert!(lit.contains("\"a\""), "got: {lit}");
        assert!(lit.contains("\"c\""), "got: {lit}");
        // Fitz literal — NO `.to_string()` suffix here (that's the
        // Rust-literal form). SSR emits classic Fitz source.
        assert!(!lit.contains(".to_string()"), "got: {lit}");
    }

    #[test]
    fn k3_ssr_coerce_helper_list_int_trims_whitespace_around_commas() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::Named("Int".into())],
        };
        let lit = coerce_child_prop_raw_value_to_fitz_literal(" 1 , 2 , 3 ", &ty, "Child", "nums")
            .expect("List<Int> coerces");
        assert_eq!(lit, "[1, 2, 3]");
    }

    #[test]
    fn k3_ssr_coerce_helper_list_empty_string_produces_empty_list() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::Named("Str".into())],
        };
        assert_eq!(
            coerce_child_prop_raw_value_to_fitz_literal("", &ty, "Child", "tags").unwrap(),
            "[]"
        );
    }

    #[test]
    fn k3_ssr_coerce_helper_list_nullable_int_recurses() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::Nullable(Box::new(TypeExpr::Named("Int".into())))],
        };
        let lit = coerce_child_prop_raw_value_to_fitz_literal("1,null,3", &ty, "Child", "nums")
            .expect("List<Int?> coerces");
        // Fitz literal form: `null` for None, bare Int for Some(n).
        assert_eq!(lit, "[1, null, 3]");
    }

    #[test]
    fn k3_ssr_coerce_helper_map_str_int_rejected_only_str_str() {
        // Post S.2 (2026-07-17): `Map<Str, Str>` is accepted, but
        // `Map<Str, Int>` (and other non-Str,Str shapes) still reject.
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::Named("Str".into()), TypeExpr::Named("Int".into())],
        };
        let err = coerce_child_prop_raw_value_to_fitz_literal("k=v", &ty, "Child", "meta")
            .expect_err("Map<Str, Int> not supported");
        assert!(
            err.message.contains("Map<Str, Int>"),
            "got: {}",
            err.message
        );
        assert!(
            err.message.contains("interpolation"),
            "err must cite workaround: {}",
            err.message
        );
    }

    // ---------------------------------------------------------------------
    // S.2 — SSR Map<Str, Str> static props via `k=v,k=v` convention
    // ---------------------------------------------------------------------

    #[test]
    fn s2_ssr_coerce_helper_map_str_str_produces_fitz_map_literal() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::Named("Str".into()), TypeExpr::Named("Str".into())],
        };
        let lit = coerce_child_prop_raw_value_to_fitz_literal(
            "role=admin,scope=full",
            &ty,
            "Card",
            "meta",
        )
        .expect("Map<Str, Str> coerces");
        // {"role": "admin", "scope": "full"}
        assert!(lit.starts_with('{'), "got: {lit}");
        assert!(lit.ends_with('}'), "got: {lit}");
        assert!(lit.contains("\"role\""), "got: {lit}");
        assert!(lit.contains("\"admin\""), "got: {lit}");
        assert!(lit.contains("\"scope\""), "got: {lit}");
    }

    #[test]
    fn s2_ssr_coerce_helper_map_str_str_empty_produces_empty_map() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::Named("Str".into()), TypeExpr::Named("Str".into())],
        };
        assert_eq!(
            coerce_child_prop_raw_value_to_fitz_literal("", &ty, "Card", "meta").unwrap(),
            "{}"
        );
    }

    #[test]
    fn s2_ssr_coerce_helper_map_pair_without_equals_reports_error() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::Named("Str".into()), TypeExpr::Named("Str".into())],
        };
        let err =
            coerce_child_prop_raw_value_to_fitz_literal("role=admin,noequals", &ty, "Card", "meta")
                .expect_err("pair without `=` should fail");
        assert!(err.message.contains("noequals"), "err: {}", err.message);
        assert!(err.message.contains("key=value"), "err: {}", err.message);
    }

    // ---------------------------------------------------------------------
    // K-3 remainder — SSR interpolated props (`<Child prop={expr} />`)
    // ---------------------------------------------------------------------
    //
    // End-to-end tests: `.fitzv` source with `<Card label={title} />`
    // (bare Ident referring to a parent state field) emits a child
    // composition where `label: state.title` is inlined in the
    // struct literal, matching the state-field rewriting rule the
    // SSR emitter applies to template interpolations.

    #[test]
    fn k3_interp_ssr_bare_ident_prop_rewrites_to_state_dot_field() {
        // Parent has a state field `title: Str`. `<Card label="{title}" />`
        // must emit `label: state.title` at the composition site.
        // The POC parser wraps interpolated values as `"{expr}"` —
        // the shape recognized by `extract_full_interp` in the
        // attribute reader.
        let src = r#"component Parent {
  state { title: Str = "hi" }
  <template><Card label="{title}" /></template>
}
component Card {
  state { label: Str = "" }
  <template><span>{label}</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // The composition site emits the child struct literal with
        // the parent's state.title rewrite inline (not `title` bare,
        // not `"title"` string literal).
        assert!(
            out.contains("Card { label: state.title }"),
            "expected interpolated prop rewritten to state.title:\n{out}"
        );
    }

    #[test]
    fn k3_interp_ssr_int_expression_inline_math_ok() {
        // Slightly richer interpolation: `<Card count="{n + 1}" />`
        // with parent state `n: Int`. The SSR emitter runs the
        // BinOp through `format_fitz_expr_scoped`, so `n` rewrites
        // to `state.n` and the operator is preserved.
        let src = r#"component Parent {
  state { n: Int = 0 }
  <template><Card count="{n + 1}" /></template>
}
component Card {
  state { count: Int = 0 }
  <template><span>{count}</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        // `format_fitz_expr_scoped` wraps BinOp in parens for
        // precedence safety, so the exact substring is `(state.n + 1)`
        // (or however the scoping helper renders it — the important
        // bit is that `n` was rewritten to `state.n`, not left bare).
        assert!(
            out.contains("state.n"),
            "expected `n` rewritten to `state.n`:\n{out}"
        );
        assert!(
            out.contains("Card { count:"),
            "expected `Card {{ count: ...`:\n{out}"
        );
    }

    #[test]
    fn k3_interp_ssr_static_and_interpolated_props_coexist() {
        // Mix static + interpolated props on the same child.
        // Both routes must emit the correct value form: static
        // via the coerce helper (string literal), interpolated
        // via the scoping helper (state rewrite).
        let src = r#"component Parent {
  state { title: Str = "hi" }
  <template><Card label="{title}" kind="primary" /></template>
}
component Card {
  state {
    label: Str = ""
    kind: Str = ""
  }
  <template><span>{label} {kind}</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module_ssr(&file).unwrap();
        assert!(
            out.contains("label: state.title"),
            "expected interpolated label rewrite:\n{out}"
        );
        assert!(
            out.contains(r#"kind: "primary""#),
            "expected static kind literal:\n{out}"
        );
    }

    // ---------------------------------------------------------------------
    // K-4 — SSR emitter accepts imported top-level fn refs in templates
    //        AND event bodies
    // ---------------------------------------------------------------------
    //
    // Post-K-3: interpolated props work; here we let the SFC's
    // template + event bodies call ANY top-level fn imported via
    // `from X import Y` at the top of the `.fitzv` file. The
    // resolution rule in `format_fitz_expr_scoped` is:
    //   1. local_scope   (closure params, `{#for x in xs}` bindings)
    //   2. state_field   (bare state ident → `state.<name>`)
    //   3. imported_name (top-level fn / type / const → emit verbatim)
    //   4. otherwise     → error citing Phase 11.7+.
    //
    // These tests exercise (3): the emitter no longer errors on bare
    // idents that match a name from `ExpandedViewFile.imports`.

    #[test]
    fn k4_ssr_template_can_call_imported_fn_from_from_import() {
        // The template calls `pick(cards, "todo")` — `pick` is an
        // imported top-level fn brought into scope via `from
        // helpers import pick`. Pre-K-4, this errored with "free-var
        // references need module loader — deferred to Phase 11.7+".
        // Post-K-4, `pick` is a known imported name and is emitted
        // verbatim; the classic checker on the emitted module
        // validates the actual call against `helpers.fitz`.
        let src = r#"from helpers import pick

component Board {
  state { count: Int = 0 }
  <template><span>{pick(count)}</span></template>
}"#;
        let raw = crate::view::parse(src).expect("parse");
        let file = crate::view::expand::expand(&raw).expect("expand");
        let out = emit_module_ssr(&file).expect("SSR emit must accept imported fn ref");
        // The interpolation renders `pick(state.count)` — `pick`
        // stays verbatim, `count` is the parent state field so it
        // gets the state-dot rewrite.
        assert!(
            out.contains("pick(state.count)"),
            "expected imported fn call + state rewrite:\n{out}"
        );
        // Regression: the `from helpers import pick` header is
        // propagated to the emitted classic-Fitz module (§9.dd
        // shipped this; K-4 depends on the same import table so
        // the emitted source stays valid).
        assert!(
            out.contains("from helpers import pick"),
            "emitted module must carry the import stmt:\n{out}"
        );
    }

    #[test]
    fn k4_ssr_event_body_can_call_imported_fn_via_closure_arg() {
        // Event body uses `cards.map(fn(c) => move_one(target, c))`
        // where `move_one` is imported. Pre-K-4 this errored inside
        // the closure arg. Post-K-4 it emits verbatim, and the
        // classic checker validates the call.
        let src = r#"from helpers import move_one

component Board {
  state { cards: List<Str> = [] }
  event tick() {
    let target = "x"
    cards = cards.map(fn(c) => move_one(target, c))
  }
  <template><span>hi</span></template>
}"#;
        let raw = crate::view::parse(src).expect("parse");
        let file = crate::view::expand::expand(&raw).expect("expand");
        let out = emit_module_ssr(&file).expect("SSR emit must accept imported fn in event body");
        assert!(
            out.contains("move_one(target, c)"),
            "expected imported fn call inside closure arg:\n{out}"
        );
    }

    #[test]
    fn k4_ssr_unknown_ident_still_errors_with_updated_hint() {
        // A bare ident that is NEITHER a state field NOR an imported
        // name still errors. The message now mentions the imports
        // table as a possible fix hint.
        let src = r#"component X {
  state { count: Int = 0 }
  <template><span>{unknown_thing}</span></template>
}"#;
        let raw = crate::view::parse(src).expect("parse");
        let file = crate::view::expand::expand(&raw).expect("expand");
        let err = emit_module_ssr(&file).expect_err("unknown ident must still fail");
        assert!(
            err.message.contains("unknown_thing"),
            "message must name the offending ident: {}",
            err.message
        );
        assert!(
            err.message.contains("imported name") || err.message.contains("from <module> import"),
            "message must mention the imports table as a fix hint: {}",
            err.message
        );
    }

    #[test]
    fn k4_ssr_local_shadows_imported_name() {
        // `x` is imported AND used as a closure param — the closure
        // param wins (shadow rule from Phase 11.6.c is preserved).
        let src = r#"from helpers import x

component X {
  state { xs: List<Str> = [] }
  <template><ul>{#for x in xs}<li>{x}</li>{/for}</ul></template>
}"#;
        let raw = crate::view::parse(src).expect("parse");
        let file = crate::view::expand::expand(&raw).expect("expand");
        let out = emit_module_ssr(&file).expect("emit ok");
        // The `{x}` inside the `{#for x in xs}` body must resolve
        // to the loop var — NOT be state-dot-rewritten and NOT be
        // the imported name (which would surprise the reader).
        // The emitter serialises `.map(fn(x) => ...)`.
        assert!(out.contains("fn(x) =>"), "expected closure over x:\n{out}");
    }

    // ---------------------------------------------------------------------
    // S.1 — Alias en imports SFC (`from X import Y as Z`)
    // ---------------------------------------------------------------------

    #[test]
    fn s1_ssr_import_with_alias_emits_from_x_import_y_as_z() {
        // `Message as Msg` in the SFC must round-trip verbatim in
        // the emitted classic Fitz module so the loader binds `Msg`
        // in local scope but validates against `message.Message`.
        let src = r#"from message import Message as Msg

component X {
  state { count: Int = 0 }
  <template><div>{count}</div></template>
}"#;
        let raw = crate::view::parse(src).expect("parse");
        let file = crate::view::expand::expand(&raw).expect("expand");
        let out = emit_module_ssr(&file).expect("emit");
        assert!(
            out.contains("from message import Message as Msg"),
            "expected alias round-trip in emitted classic:\n{out}"
        );
    }

    #[test]
    fn s1_ssr_alias_is_the_name_in_scope_not_the_original() {
        // Bare `Msg` in the template must emit verbatim (the SFC
        // sees `Msg`, not `Message` — the alias is what's bound in
        // local scope). Regression: if `imported_names` used the
        // original (`Message`), then `Msg` in the template would
        // error as "unknown identifier".
        let src = r#"from message import Message as Msg

component X {
  state { count: Int = 0 }
  <template><div>{Msg}</div></template>
}"#;
        let raw = crate::view::parse(src).expect("parse");
        let file = crate::view::expand::expand(&raw).expect("expand");
        let out = emit_module_ssr(&file).expect("emit — Msg must be in scope");
        assert!(
            out.contains("{Msg}"),
            "expected alias emitted verbatim in template:\n{out}"
        );
    }

    #[test]
    fn s1_ssr_mixed_aliased_and_bare_names_both_work() {
        // Mixed shape: some names aliased, some bare. The emitter
        // must render each entry per its (original, Option<alias>)
        // shape.
        let src = r#"from utils import User as U, Post, Comment as C

component X {
  state { count: Int = 0 }
  <template><div>{count}</div></template>
}"#;
        let raw = crate::view::parse(src).expect("parse");
        let file = crate::view::expand::expand(&raw).expect("expand");
        let out = emit_module_ssr(&file).expect("emit");
        assert!(
            out.contains("from utils import User as U, Post, Comment as C"),
            "expected mixed aliased + bare in emitted classic:\n{out}"
        );
    }

    // ---- Mixed attribute interpolation -----------------------
    // `attr="prefix-{field}"` — a state field interpolated inside a
    // partly-static attribute value. Rewrites the `{field}` segment to
    // `state.field` in the emitted classic-Fitz string, same as a text
    // interpolation or a full `attr="{field}"`. Regression guard for
    // the Admin ABM Toast (`class="toast toast-{kind}"`), which built
    // to `unknown variable kind` before the fix (mixed values fell to
    // the Static path and emitted the brace verbatim).

    #[test]
    fn mixed_attr_interp_rewrites_state_field_in_class() {
        let file = parse_expand(
            r#"component Badge { state { kind: Str = "ok" } <template><span class="badge badge-{kind}">x</span></template> }"#,
        );
        let out = emit_module_ssr(&file).expect("emit");
        assert!(
            out.contains("badge badge-{state.kind}"),
            "mixed attr must rewrite {{kind}} to {{state.kind}}:\n{out}"
        );
        assert!(
            !out.contains("badge-{kind}"),
            "bare {{kind}} must not survive into the emitted attribute:\n{out}"
        );
    }

    #[test]
    fn mixed_attr_interp_multiple_segments() {
        let file = parse_expand(
            "component P {\n  state {\n    a: Str = \"1\"\n    b: Str = \"2\"\n  }\n  <template><div data-x=\"p-{a}-q-{b}\">x</div></template>\n}",
        );
        let out = emit_module_ssr(&file).expect("emit");
        assert!(
            out.contains("p-{state.a}-q-{state.b}"),
            "both segments must rewrite:\n{out}"
        );
    }

    #[test]
    fn mixed_attr_interp_inside_if_block() {
        // The exact Admin ABM Toast shape: a mixed-interp attribute on
        // an element nested inside a `{#if}` directive.
        let file = parse_expand(
            "component Toast {\n  state {\n    open: Bool = false\n    kind: Str = \"success\"\n  }\n  <template><div>{#if open}<div class=\"toast toast-{kind}\">x</div>{/if}</div></template>\n}",
        );
        let out = emit_module_ssr(&file).expect("emit");
        assert!(
            out.contains("toast toast-{state.kind}"),
            "mixed attr inside {{#if}} must rewrite {{kind}}:\n{out}"
        );
    }

    #[test]
    fn mixed_attr_interp_imported_name_passes_through() {
        // An imported helper referenced inside a mixed attribute stays
        // verbatim (not rewritten to state.<name>), same rule as text.
        let file = parse_expand(
            r#"from helpers import cls
component X { state { n: Int = 0 } <template><div class="row {cls(n)}">x</div></template> }"#,
        );
        let out = emit_module_ssr(&file).expect("emit");
        assert!(
            out.contains("row {cls(state.n)}"),
            "imported name stays bare, state field rewrites:\n{out}"
        );
    }

    #[test]
    fn full_attr_interp_still_works_after_mixed_support() {
        // Regression: a whole-value `attr="{field}"` still routes
        // through the Interpolation path (not MixedInterpolation).
        let file = parse_expand(
            r#"component X { state { q: Str = "" } <template><input value="{q}" /></template> }"#,
        );
        let out = emit_module_ssr(&file).expect("emit");
        assert!(
            out.contains("value=\"{state.q}\""),
            "full interpolation must still rewrite to state.q:\n{out}"
        );
    }

    // ---- Form B (gotcha #6, v0.38.0): conditional boolean attribute ----
    // `checked={expr}` lowers to a Fitz if-as-expression yielding the bare
    // attribute name (present) or "" (absent) — present-iff-truthy.

    #[test]
    fn bool_attr_ssr_emits_conditional_present_v0_38_0() {
        let file = parse_expand(
            r#"component X { state { done: Bool = false } <template><input checked={done} /></template> }"#,
        );
        let out = emit_module_ssr(&file).expect("emit");
        assert!(
            out.contains("if (state.done) { \"checked\" } else { \"\" }"),
            "bool attr must lower to a present-iff-truthy if-expr:\n{out}"
        );
    }

    #[test]
    fn bool_attr_ssr_binop_cond_v0_38_0() {
        let file = parse_expand(
            r#"component X { state { n: Int = 0 } <template><input disabled={n > 0} /></template> }"#,
        );
        let out = emit_module_ssr(&file).expect("emit");
        // `format_fitz_expr_scoped` parenthesises a BinOp, so the cond
        // arrives as `(state.n > 0)` inside the emitter's own `if (...)`.
        assert!(
            out.contains("(state.n > 0)) { \"disabled\" } else { \"\" }"),
            "binop cond must rewrite the state field and yield `disabled`:\n{out}"
        );
    }

    #[test]
    fn bool_attr_ssr_inside_for_uses_loop_var_v0_38_0() {
        // Inside `{#for b in flags}` the loop var `b` is NOT rewritten to
        // `state.b` (it shadows state), so the cond stays `b`.
        let file = parse_expand(
            r#"component X { state { flags: List<Bool> = [] } <template>{#for b in flags}<input checked={b} />{/for}</template> }"#,
        );
        let out = emit_module_ssr(&file).expect("emit");
        assert!(
            out.contains("if (b) { \"checked\" } else { \"\" }"),
            "loop var must stay bare (not state.b):\n{out}"
        );
    }

    #[test]
    fn static_attr_without_braces_unchanged() {
        // Regression: a plain static attribute with no `{` stays a
        // verbatim Static value.
        let file = parse_expand(
            r#"component X { state {} <template><div class="a b c">x</div></template> }"#,
        );
        let out = emit_module_ssr(&file).expect("emit");
        assert!(
            out.contains("class=\"a b c\""),
            "static class must pass through verbatim:\n{out}"
        );
    }

    #[test]
    fn mixed_class_in_scoped_component_suffixes_only_pure_tokens() {
        // A mixed `class` in a `<style scoped>` component: the COMPLETE
        // literal token (`badge`) gets the scope suffix, but the token
        // glued to the `{expr}` (`badge-{kind}`) is a runtime value and
        // stays unscoped. Regression guard for v0.28.8 — v0.28.7 appended
        // the bare scope class as a separate token instead of suffixing.
        let file = parse_expand(
            "component Badge {\n  state { kind: Str = \"ok\" }\n  <template><span class=\"badge badge-{kind}\">x</span></template>\n  <style scoped>.badge { color: red; }</style>\n}",
        );
        let out = emit_module_ssr(&file).expect("emit");
        // The pure token `badge` is suffixed with the scope (`badge-badge-c-<hex>`).
        assert!(
            out.contains("badge-badge-c-"),
            "pure literal token `badge` must be suffixed with the scope:\n{out}"
        );
        // The glued `badge-{kind}` stays as a runtime interpolation, not scoped.
        assert!(
            out.contains("badge-{state.kind}"),
            "glued `badge-{{kind}}` must stay a runtime interpolation:\n{out}"
        );
        // The old bug appended a bare ` badge-c-<hex>` token (scope class on
        // its own, not attached to a class name). It must not reappear.
        assert!(
            !out.contains(" badge-c-"),
            "scope must be a suffix on a token, never a standalone class:\n{out}"
        );
    }

    // ---- keyed `{#for x in xs key=x.id}` sugar (SSR emit) -----------

    #[test]
    fn ssr_for_with_key_emits_data_flv_key_interpolation() {
        // `{#for r in rows key=r.id}<li>{r.name}</li>{/for}` lowers to
        // `__fitz_view_str_join(state.rows.map(fn(r) => "<li data-flv-key=
        // \"{r.id}\">{r.name}</li>"))`. The `r.id` interpolation renders
        // per-item, so the client keyed-diff engine matches list items.
        let file = parse_expand(
            "component List {\n  state { rows: List<Str> = [] }\n  <template>{#for r in rows key=r}<li>{r}</li>{/for}</template>\n}",
        );
        let out = emit_module_ssr(&file).expect("emit");
        assert!(
            out.contains("__fitz_view_str_join(state.rows.map(fn(r) =>"),
            "expected for-loop lowering:\n{out}"
        );
        assert!(
            out.contains("data-flv-key=\"{r}\""),
            "expected `data-flv-key` interpolation on the list item:\n{out}"
        );
    }

    #[test]
    fn ssr_for_without_key_emits_no_data_flv_key() {
        // Regression: a keyless `{#for}` is byte-for-byte unchanged (no
        // `data-flv-key` leaks into the emitted body).
        let file = parse_expand(
            "component List {\n  state { rows: List<Str> = [] }\n  <template>{#for r in rows}<li>{r}</li>{/for}</template>\n}",
        );
        let out = emit_module_ssr(&file).expect("emit");
        assert!(
            !out.contains("data-flv-key"),
            "keyless for-loop must not emit data-flv-key:\n{out}"
        );
    }

    #[test]
    fn cw9_ssr_raw_html_marker_stripped_in_interpolation() {
        // CW.9 — `{raw_html(x)}` / `{html(x)}` in an interpolation is the
        // raw-HTML marker that dual-targets with the wasm `set_inner_html`
        // sink. On SSR, classic `{expr}` is already raw, so the emitter strips
        // the marker to `{x}` (byte-identical to the idiomatic raw form, and
        // never serialising the `Html` struct's debug repr).
        let file = parse_expand(
            "component App {\n  state { svg: Str = \"\" }\n  <template><span>{raw_html(svg)}</span></template>\n}",
        );
        let out = emit_module_ssr(&file).expect("emit");
        assert!(
            out.contains("{state.svg}"),
            "the marker must strip to the inner (raw) expr:\n{out}"
        );
        assert!(
            !out.contains("raw_html("),
            "the raw_html wrapper must not survive into the emitted SSR:\n{out}"
        );
    }
}
