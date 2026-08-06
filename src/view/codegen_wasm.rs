//! WASM code generator for `.fitzv` view components — Phase 11.4.b.
//!
//! Consumes an `ExpandedComponent` (post-11.3.c pipeline: parse →
//! expand → check) and emits Rust source code that, when compiled
//! for the `wasm32-unknown-unknown` target with the deps declared
//! under the opt-in feature `client-wasm`, produces a WebAssembly
//! bundle that mounts the component into a DOM node.
//!
//! **Approach A2** — hand-rolled `wasm-bindgen` + `web-sys` directly,
//! no framework in the loop. See `docs/fase-11-plan.md` §9.l for the
//! decision analysis (why A2 over A1 framework delegation, A3 in-tree
//! runtime crate, B1 JS-vanilla, B2 JS compiler delegation).
//!
//! ## Scope of 11.4.b (POC counter subset — strictly enforced)
//!
//! Cubre exactamente lo que hace falta para que un `.fitzv` de
//! counter compile a WASM y funcione en el browser:
//!
//! - **State**: solo `Int` primitivos con default literal
//!   (`state { count: Int = 0 }`).
//! - **Event handlers**: sync, cero params, body = un solo
//!   `Stmt::Assign` a state field con RHS numérico
//!   (`Expr::Int` / `Expr::Ident` / `Expr::BinOp` con Add/Sub/Mul/Div/Mod).
//! - **Template**: `Text`, `Element` (tag estático, attrs `Static` y
//!   `Event` con `@click`), `Interpolation` con `Expr::Ident` de un
//!   state field.
//! - **Style**: `Scoped` inyectado 1x via dedup helper con
//!   `AtomicBool`, o `Global` inyectado directo. Cero style: no-op.
//!
//! Todo lo demás (If / For / Slot / attrs interpolados / handlers
//! con params / eventos que no sean `@click` / state con tipo
//! distinto a `Int`) devuelve `EmitError` con un mensaje que cita el
//! sub-fase donde se cierra (11.4.c o 11.5).
//!
//! ## Reactivity model del POC (D1 de 11.4.b)
//!
//! Naive re-render on state mutation. Cada `event fn` termina
//! llamando a `self.render()`, que limpia el DOM subtree del
//! componente y lo reconstruye desde cero usando el state actual.
//! Sin signals, sin VDOM, sin diffing. Aceptable para counter /
//! forms chicos / dashboards con updates ocasionales. Refinement a
//! signals fine-grained cae en A3 (in-tree runtime crate) — sub-fase
//! futura si la evidencia de bloat con N componentes lo empuja.
//!
//! ## API pública (D2 de 11.4.b)
//!
//! - [`emit_component`] — emite el Rust de UN componente (struct +
//!   new() + event fns + mount() + render() + style helper).
//!   NO emite `use` imports ni `#[wasm_bindgen(start)]`.
//! - [`emit_module`] — emite un módulo Rust entero: preludio con
//!   imports + N componentes. Conveniencia para 11.4.c/11.5 smoke.
//!
//! ## Testing strategy (D4 de 11.4.b)
//!
//! Todos los tests unitarios validan substrings de la string
//! emitida (paralelo bit-a-bit a `view::css_parser::tests`). NO
//! buildean el output real a WASM — eso requiere target
//! `wasm32-unknown-unknown` + `wasm-pack` que no viven en
//! `cargo test`. El browser smoke real llega en 11.4.c.
//!
//! ## Invariant 4 (isolation)
//!
//! Este módulo vive 100% adentro de `src/view/`. NO importa nada de
//! `src/codegen.rs` (el codegen server-side clásico) ni del runtime
//! HTTP. La única superficie compartida es `crate::ast::{Expr, Stmt,
//! TypeExpr, AssignTarget, BinOpKind}` — nodos AST que el
//! `expand::ExpandedComponent` ya carga como fields públicos. Cero
//! coupling nuevo con el resto del compilador.

use crate::ast::{AssignTarget, BinOpKind, Expr, Stmt, StrPart, TypeExpr};
use crate::view::{
    ExpandedAttr, ExpandedComponent, ExpandedEventHandler, ExpandedStateField, ExpandedStyle,
    ExpandedTemplateNode, ExpandedViewFile,
};
use std::fmt::Write;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Error surfaced while emitting WASM Rust code from an
/// `ExpandedComponent`. Every unsupported shape of the current POC
/// subset produces one of these with a `context` label that names
/// the exact component + field / handler / template node where the
/// problem lives, plus a `message` that either explains the misuse
/// or cites the sub-phase where the missing capability lands.
#[derive(Debug, Clone, PartialEq)]
pub struct EmitError {
    pub message: String,
    pub context: String,
}

impl std::fmt::Display for EmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "wasm emit error — {} ({})", self.message, self.context)
    }
}

impl std::error::Error for EmitError {}

pub type EmitResult<T> = Result<T, EmitError>;

// ---------------------------------------------------------------------------
// Nominal registry (Phase 11.7 R3)
// ---------------------------------------------------------------------------

/// The classic `type Foo { ... }` definitions imported into a
/// `.fitzv` (via `from foo import Foo`), loaded so the WASM emitter
/// can emit a real Rust `struct Foo { ... }` inline in the bundle.
///
/// The SSR emitter never needs this — it re-emits the user's `from
/// foo import Foo` verbatim and defers all nominal resolution to the
/// classic loader in a second compilation pass (see
/// `codegen_ssr.rs`). The WASM emitter has NO downstream classic
/// pass: it produces a standalone `wasm32` crate, so every nominal
/// touchpoint (`List<Foo>` state, `{#for c in cards}`, `{c.title}`,
/// `Foo { ... }` construction) must lower to real Rust here. That
/// requires the field list, which the view pipeline does not carry —
/// hence this registry, populated by
/// [`super::wasm_build::load_imported_nominals`] which reads + parses
/// the sibling `.fitz` before emit.
///
/// Keyed by the LOCAL binding name (the alias when `from foo import
/// Foo as Bar` is used, else the original), because that is the name
/// that appears in the `.fitzv` state annotations + struct literals.
/// Fields are stored in declaration order so the emitted struct
/// mirrors the source `type`.
#[derive(Debug, Clone, Default)]
pub struct NominalRegistry {
    defs: std::collections::BTreeMap<String, Vec<(String, TypeExpr)>>,
}

impl NominalRegistry {
    /// An empty registry — the common case (no imported nominals) and
    /// what [`emit_module`] / [`emit_component`] pass so the legacy
    /// primitive-only path is byte-for-byte unchanged.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a nominal under its local binding name with its
    /// ordered `(field_name, field_type)` list.
    pub fn insert(&mut self, name: String, fields: Vec<(String, TypeExpr)>) {
        self.defs.insert(name, fields);
    }

    /// True when `name` names a registered nominal — the gate that
    /// lets `type_expr_to_rust` accept it and `emit_for` iterate it.
    pub fn contains(&self, name: &str) -> bool {
        self.defs.contains_key(name)
    }

    /// True when nothing is registered. When empty, no `struct` is
    /// emitted and nominal types still reject, preserving the
    /// pre-R3 output bit-for-bit.
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// The ordered `(field_name, field_type)` list of a registered
    /// nominal, or `None` when unknown. Phase 11.12 slice 3 — the
    /// hydration state restore reads these to deserialize a nominal
    /// state field from its JSON object, field by field.
    fn fields(&self, name: &str) -> Option<&[(String, TypeExpr)]> {
        self.defs.get(name).map(|v| v.as_slice())
    }

    /// Iterate registered nominals in deterministic (sorted-by-name)
    /// order so the emitted structs are stable across runs.
    fn iter(&self) -> impl Iterator<Item = (&String, &Vec<(String, TypeExpr)>)> {
        self.defs.iter()
    }
}

// ---------------------------------------------------------------------------
// Imported classic-fn registry (Phase 11.7 R3.5a.2)
// ---------------------------------------------------------------------------

/// One imported classic `fn` transpiled into the WASM bundle.
///
/// The kanban's pure helpers (`cards_in`, `move_one`, `next_column`,
/// `make_card`, ...) live in a sibling classic `.fitz` module. The SSR
/// target gets them for free (it re-emits classic Fitz + a second pass);
/// the WASM target has no second pass, so each helper is lowered to a
/// real Rust `fn` here. Params + return type are required (the WASM
/// lowerer has no inference); the body lowers with the shared
/// `lower_stmt` walker, with the params as the initial local scope and
/// NO state fields (a free function has no `self`).
#[derive(Debug, Clone)]
pub struct ImportedFn {
    name: String,
    /// `(param_name, declared_type)`. The type is required — an
    /// un-annotated param rejects at emit.
    params: Vec<(String, Option<TypeExpr>)>,
    ret: Option<TypeExpr>,
    body: Vec<Stmt>,
    /// Phase 11.11.c — `true` if this fn carries `@rpc` (a server
    /// function). Its body is NOT transpiled to the wasm crate (it
    /// runs on the server); instead the emitter produces an async
    /// `fetch`-based stub that POSTs the JSON args to
    /// `/__rpc/<name>` and deserializes the `Result<T>` reply.
    is_rpc: bool,
}

/// The classic `fn`s reachable through a `.fitzv`'s imports, loaded so
/// the WASM emitter can transpile each to a Rust `fn` (Phase 11.7
/// R3.5a.2). Populated by
/// [`super::wasm_build::load_imported_fns`], which parses every sibling
/// module named in the imports and registers ALL its top-level `fn`s
/// (not just the explicitly-imported names — an imported helper like
/// `move_one` calls internal siblings like `next_column`).
///
/// Keyed by function name (deterministic order), so the emitted `fn`s
/// are stable across runs.
#[derive(Debug, Clone, Default)]
pub struct ImportedFnRegistry {
    fns: std::collections::BTreeMap<String, ImportedFn>,
}

impl ImportedFnRegistry {
    /// An empty registry — the common case (a `.fitzv` importing no
    /// classic helpers) and what the primitive-only emit path passes so
    /// the pre-R3.5a.2 output is byte-for-byte unchanged.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or overwrite) a transpiled function by name.
    pub fn insert(
        &mut self,
        name: String,
        params: Vec<(String, Option<TypeExpr>)>,
        ret: Option<TypeExpr>,
        body: Vec<Stmt>,
        is_rpc: bool,
    ) {
        self.fns.insert(
            name.clone(),
            ImportedFn {
                name,
                params,
                ret,
                body,
                is_rpc,
            },
        );
    }

    /// True when no functions are registered — no `fn`s are emitted, so
    /// the output stays byte-identical to the pre-R3.5a.2 path.
    pub fn is_empty(&self) -> bool {
        self.fns.is_empty()
    }

    /// Phase 11.11.c — `true` when at least one registered fn is
    /// `@rpc`. Drives whether the wasm crate gets the fetch runtime:
    /// the `__fitz_fetch_post` helper, `serde`/`serde_json`/
    /// `wasm-bindgen-futures`/`js-sys` deps, the web-sys
    /// `Request`/`Response`/`Headers` features, and `serde` derives on
    /// the nominal structs.
    pub fn has_rpc(&self) -> bool {
        self.fns.values().any(|f| f.is_rpc)
    }

    /// True when a function named `name` is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.fns.contains_key(name)
    }

    fn iter(&self) -> impl Iterator<Item = &ImportedFn> {
        self.fns.values()
    }
}

// ---------------------------------------------------------------------------
// Imported-component registry (Phase 11.7 — cross-file `<Child />`)
// ---------------------------------------------------------------------------

/// The `.fitzv` components a `.fitzv` imports (`from Card import Card`),
/// loaded so the WASM emitter can inline each imported component's WHOLE
/// emit (struct + `new` + event handlers + render + `<style scoped>`)
/// into the same standalone `wasm32` crate.
///
/// Cross-file `<Child />` composition needs more than a nominal's field
/// list (Phase 11.7 R3) or a helper's body (R3.5a.2): the imported child
/// is a first-class component with its own state, events, slots, and
/// scoped style. The WASM target has no downstream classic pass to
/// resolve the reference, so the child's full `ExpandedComponent` is
/// carried here and re-emitted inline. Populated by
/// [`super::wasm_build::load_imported_components`], which reads + parses +
/// expands the sibling `.fitzv` files.
///
/// Every component declared in each imported `.fitzv` is registered
/// (not just the explicitly-imported names) so an imported `Card` can
/// compose its own file-local siblings — the same "load the whole file"
/// policy as [`ImportedFnRegistry`]. Keyed by the component's declared
/// name; first-registration wins on a cross-file name collision.
#[derive(Debug, Clone, Default)]
pub struct ImportedComponentRegistry {
    comps: Vec<ExpandedComponent>,
}

impl ImportedComponentRegistry {
    /// An empty registry — the common case (no imported components) and
    /// what the same-file emit path passes, so the pre-cross-file
    /// examples regenerate byte-for-byte.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an imported component. First-registration wins on a name
    /// collision (a component named `X` imported from two files → the
    /// first import's version is kept), so the merge is deterministic.
    pub fn insert(&mut self, component: ExpandedComponent) {
        if !self.comps.iter().any(|c| c.name == component.name) {
            self.comps.push(component);
        }
    }

    /// True when no components are registered — no imported component is
    /// emitted, so the same-file output stays byte-identical.
    pub fn is_empty(&self) -> bool {
        self.comps.is_empty()
    }

    /// Look an imported component up by declared name.
    pub fn get(&self, name: &str) -> Option<&ExpandedComponent> {
        self.comps.iter().find(|c| c.name == name)
    }

    /// True when a component named `name` is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.comps.iter().any(|c| c.name == name)
    }

    /// All imported component surfaces, in registration order. Consumed
    /// by the view checker (to include cross-file children in its
    /// component map) and the emitter's reachability walk.
    pub fn components(&self) -> &[ExpandedComponent] {
        &self.comps
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Emit the Rust source code for ONE component. Output includes:
///
/// - `pub struct <Name>` con state fields (envueltos en
///   `RefCell<...>`) + `root: RefCell<Option<HtmlElement>>`.
/// - `impl <Name>` con `pub fn new() -> Rc<Self>`, un `fn <event>()`
///   privado por cada handler declarado, `pub fn mount(...)`, y
///   `fn render()` que reconstruye el DOM subtree del componente
///   desde el `ExpandedTemplate`.
/// - Cuando el componente tiene `<style scoped>` o `<style global>`,
///   una fn libre `__inject_style_<sanitized>()` que injecta el
///   `<style>` en `document.head` una sola vez (dedup por
///   `AtomicBool` para scoped, sin dedup para global — cada mount de
///   un componente distinto podría querer sus propias reglas
///   globales).
///
/// NO emite `use` imports ni `#[wasm_bindgen(start)]`. El caller
/// (típicamente [`emit_module`] o el CLI de 11.5) compone.
pub fn emit_component(component: &ExpandedComponent) -> EmitResult<String> {
    // Wrap the single component in a synthetic file so
    // `emit_component_impl` has the same shape it gets from
    // `emit_module`. Child-component composition (`<Child />`)
    // in isolation is not sensible — a `<Child />` reference
    // rejects at check-time when the sibling is missing —
    // but tests that exercise the single-component emitter
    // don't use composition anyway.
    let synthetic_file = ExpandedViewFile {
        imports: Vec::new(),
        components: vec![component.clone()],
    };
    let mut out = String::new();
    let nominals = NominalRegistry::new();
    let bubbled = collect_bubbled_events(&synthetic_file);
    emit_component_impl(component, &synthetic_file, &nominals, &bubbled, &mut out)?;
    Ok(out)
}

/// Map each component name → the set of its event names that some
/// `<Child @event="..." />` site binds (Phase 11.7.c event bubbling). A
/// child gains a callback slot + a bubble call in its handler ONLY for
/// events in this set, so components with no bubbled events emit
/// byte-for-byte unchanged. `BTreeMap`/`BTreeSet` keep the output stable.
fn collect_bubbled_events(
    file: &ExpandedViewFile,
) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
    let mut map: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
        std::collections::BTreeMap::new();
    fn walk(
        node: &ExpandedTemplateNode,
        map: &mut std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    ) {
        match node {
            ExpandedTemplateNode::ChildComponent { name, events, .. } => {
                for ev in events {
                    map.entry(name.clone())
                        .or_default()
                        .insert(ev.event_name.clone());
                }
            }
            ExpandedTemplateNode::Element { children, .. } => {
                for c in children {
                    walk(c, map);
                }
            }
            ExpandedTemplateNode::If {
                children,
                else_children,
                ..
            } => {
                for c in children {
                    walk(c, map);
                }
                if let Some(els) = else_children {
                    for c in els {
                        walk(c, map);
                    }
                }
            }
            ExpandedTemplateNode::For { children, .. } => {
                for c in children {
                    walk(c, map);
                }
            }
            _ => {}
        }
    }
    for component in &file.components {
        if let Some(t) = &component.template {
            for node in &t.roots {
                walk(node, &mut map);
            }
        }
    }
    map
}

/// Emit un módulo Rust entero ready to build con `wasm-pack build`:
///
/// - Preludio con `use` de `wasm_bindgen`, `web_sys`, `std::cell`,
///   `std::rc`, `std::sync::atomic`.
/// - Todos los componentes concatenados via [`emit_component`].
///
/// Convenience para 11.4.c smoke (que va a llamar esta fn y volcar
/// el resultado a un `src/lib.rs` de un crate WASM temporal) y para
/// 11.5 CLI wiring. En 11.5 el CLI probablemente construye su
/// propio preludio (para meter `#[wasm_bindgen(start)]` con
/// composición multi-componente); esta fn queda como default
/// razonable para tests + POCs.
pub fn emit_module(file: &ExpandedViewFile) -> EmitResult<String> {
    emit_module_with_nominals(file, &NominalRegistry::new())
}

/// Like [`emit_module`], but with a [`NominalRegistry`] of imported
/// classic `type` definitions loaded from the sibling `.fitz` files
/// (Phase 11.7 R3). When the registry is non-empty, a Rust `struct`
/// is emitted for each nominal right after the module header, so
/// `List<Card>` state fields, `{#for c in cards}` loops, `{c.title}`
/// field access, and `Card { ... }` construction lower to real Rust.
///
/// When the registry is empty this is byte-for-byte identical to
/// [`emit_module`] — no struct is emitted and nominal types still
/// reject, so the four pre-R3 examples regenerate unchanged.
pub fn emit_module_with_nominals(
    file: &ExpandedViewFile,
    nominals: &NominalRegistry,
) -> EmitResult<String> {
    emit_module_with_imports(file, nominals, &ImportedFnRegistry::new())
}

/// Like [`emit_module_with_nominals`], but also transpiles imported
/// classic `fn`s into the module (Phase 11.7 R3.5a.2). The imported
/// functions are emitted right after the nominal structs and before the
/// components, so component render/event code can call them.
///
/// When `fns` is empty this is byte-for-byte identical to
/// [`emit_module_with_nominals`], so the pre-R3.5a.2 examples regenerate
/// unchanged.
pub fn emit_module_with_imports(
    file: &ExpandedViewFile,
    nominals: &NominalRegistry,
    fns: &ImportedFnRegistry,
) -> EmitResult<String> {
    emit_module_with_components(file, nominals, fns, &ImportedComponentRegistry::new())
}

/// Like [`emit_module_with_imports`], but also inlines imported `.fitzv`
/// components so cross-file `<Child />` composition lowers to real Rust
/// (Phase 11.7 — cross-file). Populate `components` with
/// [`super::wasm_build::load_imported_components`] before calling.
///
/// The reachable subset of imported components (the transitive closure of
/// `<Child />` refs starting from the local components) is merged ahead of
/// the local components into ONE synthetic file, then every existing pass
/// — bubbled-event collection, per-component emit, and the same-file child
/// resolution inside [`emit_child_component`] — runs over the merge. So an
/// imported child's struct/`new`/handlers/render/style are emitted inline,
/// its `__on_<event>` bubble slots are wired when a local parent binds
/// them, and the parent's cache field references its (now in-module) type.
///
/// Only *reachable* imported components are emitted — an imported component
/// no local (or transitively-reached) component composes is left out, so an
/// unused import can't drag in a nominal/helper the parent never imported.
///
/// When `components` is empty the merge is a structural clone of `file`, so
/// this is byte-for-byte identical to the same-file path and the pre-cross-
/// file examples regenerate unchanged.
pub fn emit_module_with_components(
    file: &ExpandedViewFile,
    nominals: &NominalRegistry,
    fns: &ImportedFnRegistry,
    components: &ImportedComponentRegistry,
) -> EmitResult<String> {
    let mut out = String::new();
    emit_module_header(&mut out);
    emit_nominal_structs(nominals, fns.has_rpc(), &mut out)?;
    // CW.9 (1c) — the `Html` shim, emitted once before the imported fns that
    // reference it (e.g. `icon -> Html`). Only when the bundle actually uses
    // `Html`, so bundles without it stay byte-identical.
    if bundle_uses_html(fns) {
        out.push_str(HTML_SHIM);
    }
    emit_imported_fns(fns, nominals, &mut out)?;
    let mut merged = merge_imported_components(file, components);
    // Phase 11.12 slice 4 — hydration opt-in propagates from the ROOT of the
    // emitted tree to every component in it. The `component App hydrate { ... }`
    // marker lives on the root; a composed child (`<Child />` / `<slot>`) has no
    // marker of its own but must still emit `hydrate()` so the whole tree adopts
    // the server-painted DOM. Keep-node components hydrate regardless (slices
    // 1–3); this only lifts the naive composition path.
    propagate_root_hydrate(&mut merged);
    let bubbled = collect_bubbled_events(&merged);
    // Phase 11.12 — cursor helpers for the hydration adopt walk, emitted once
    // when any component is hydratable (byte-identical output otherwise). The
    // region-anchor comment cursor is only pulled in when a hydratable
    // component actually has `{#if}`/`{#for}` regions (slice 2), so region-free
    // hydratable crates carry no unused helper.
    if any_component_hydratable(&merged, &bubbled) {
        emit_hydration_helpers(
            &mut out,
            any_component_hydratable_with_regions(&merged, &bubbled),
        );
    }
    for component in &merged.components {
        emit_component_impl(component, &merged, nominals, &bubbled, &mut out)?;
    }
    Ok(out)
}

/// Merge the reachable imported components ahead of `file`'s local
/// components into a single synthetic [`ExpandedViewFile`], so the emit +
/// analysis passes treat cross-file children as if they lived in the same
/// file. See [`emit_module_with_components`] for the rationale.
///
/// Reachability = the transitive closure of `<Child />` names referenced
/// from the local components (and, recursively, from each reached imported
/// component). A local component name is never treated as imported (local
/// wins on a name collision), and an unknown name is left for the checker
/// to report — it is simply skipped here.
///
/// When `imported` is empty this returns a structural clone of `file`, so
/// downstream emit stays byte-for-byte identical to the same-file path.
pub fn merge_imported_components(
    file: &ExpandedViewFile,
    imported: &ImportedComponentRegistry,
) -> ExpandedViewFile {
    if imported.is_empty() {
        return file.clone();
    }

    let local_names: std::collections::BTreeSet<&str> =
        file.components.iter().map(|c| c.name.as_str()).collect();

    let mut worklist: Vec<String> = Vec::new();
    for c in &file.components {
        collect_child_names(c, &mut worklist);
    }

    let mut reached: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut reachable: Vec<ExpandedComponent> = Vec::new();
    while let Some(name) = worklist.pop() {
        // Local components win — never resolve a local name to an import.
        if local_names.contains(name.as_str()) {
            continue;
        }
        if !reached.insert(name.clone()) {
            continue;
        }
        if let Some(comp) = imported.get(&name) {
            collect_child_names(comp, &mut worklist);
            reachable.push(comp.clone());
        }
        // Unknown names (not local, not imported) fall through — the
        // checker reports them as unknown components with a hint.
    }

    // Deterministic order regardless of worklist traversal.
    reachable.sort_by(|a, b| a.name.cmp(&b.name));

    let mut components = reachable;
    components.extend(file.components.iter().cloned());
    ExpandedViewFile {
        imports: file.imports.clone(),
        components,
    }
}

/// Phase 11.12 slice 4 — propagate the hydration opt-in from the ROOT
/// component to every component in the emitted tree.
///
/// The `component App hydrate { ... }` marker sits on the root (the entry
/// component, `components[0]` after the merge places local components ahead
/// of imported ones — but the merge prepends *imported* reachable ones, so
/// we look for the flag on ANY component rather than assume position). When
/// the tree hydrates, every naive component in it (the composed children —
/// `<Child />` + `<slot>`) must emit a `hydrate()` so the whole tree adopts
/// the server-painted DOM in one boot instead of fresh-mounting.
///
/// Keep-node components already hydrate regardless of this flag (slices
/// 1–3). When no component carries the marker this is a no-op, so pre-11.12
/// crates stay byte-identical.
fn propagate_root_hydrate(file: &mut ExpandedViewFile) {
    if file.components.iter().any(|c| c.hydrate) {
        for c in &mut file.components {
            c.hydrate = true;
        }
    }
}

/// Collect the names of every `<Child />` referenced anywhere in a
/// component's template (including inside `{#if}` / `{#for}` branches,
/// `<slot>` fallbacks, and `<Child>...</Child>` slot content), for the
/// cross-file reachability walk.
fn collect_child_names(component: &ExpandedComponent, out: &mut Vec<String>) {
    fn walk(node: &ExpandedTemplateNode, out: &mut Vec<String>) {
        match node {
            ExpandedTemplateNode::ChildComponent {
                name, slot_content, ..
            } => {
                out.push(name.clone());
                for c in slot_content {
                    walk(c, out);
                }
            }
            ExpandedTemplateNode::Element { children, .. } => {
                for c in children {
                    walk(c, out);
                }
            }
            ExpandedTemplateNode::If {
                children,
                else_children,
                ..
            } => {
                for c in children {
                    walk(c, out);
                }
                if let Some(els) = else_children {
                    for c in els {
                        walk(c, out);
                    }
                }
            }
            ExpandedTemplateNode::For { children, .. } => {
                for c in children {
                    walk(c, out);
                }
            }
            ExpandedTemplateNode::Slot { fallback, .. } => {
                for c in fallback {
                    walk(c, out);
                }
            }
            _ => {}
        }
    }
    if let Some(t) = &component.template {
        for node in &t.roots {
            walk(node, out);
        }
    }
}

/// Emit a `#[derive(Clone)] pub struct <Name> { ... }` for every
/// registered nominal (Phase 11.7 R3). `Clone` is required because
/// `emit_for` snapshots the state `Vec` (`.iter().cloned()`) and
/// keyed composition clones field values into child `RefCell`s.
/// Fields are non-`pub` — all emitted code lives in one module, so
/// intra-module access (`c.title`, cross-instance prop writes) works
/// without exposing the fields on the crate surface.
///
/// `#[allow(dead_code)]` is emitted per struct because a `type` may
/// declare fields the template never reads (e.g. a `done` flag kept
/// for logic that lands later) — a structural consequence of nominals,
/// not a bug. The allow is item-level (not crate-level) so the four
/// primitive-only pre-R3 examples' output stays byte-for-byte
/// unchanged (no nominal struct → no allow emitted).
fn emit_nominal_structs(
    nominals: &NominalRegistry,
    uses_rpc: bool,
    out: &mut String,
) -> EmitResult<()> {
    if nominals.is_empty() {
        return Ok(());
    }
    for (name, fields) in nominals.iter() {
        writeln!(out, "#[allow(dead_code)]").unwrap();
        // Phase 11.11.c — when the crate uses `@rpc`, nominals cross the
        // wire (as args and/or `Result<T>` replies), so they gain
        // `serde` derives. Without rpc the derive is omitted so the
        // output stays byte-identical to the pre-11.11 path. (Map fields
        // `Vec<(K,V)>` serialize as pair arrays, not JSON objects — a
        // documented limitation for rpc payloads.)
        if uses_rpc {
            writeln!(
                out,
                "#[derive(Clone, serde::Serialize, serde::Deserialize)]"
            )
            .unwrap();
        } else {
            writeln!(out, "#[derive(Clone)]").unwrap();
        }
        writeln!(out, "pub struct {} {{", name).unwrap();
        for (fname, fty) in fields {
            let rust_ty = type_expr_to_rust(fty, nominals).map_err(|mut e| {
                e.context = format!("imported nominal `{name}` field `{fname}`");
                e
            })?;
            writeln!(out, "    {}: {},", fname, rust_ty).unwrap();
        }
        writeln!(out, "}}\n").unwrap();
    }
    Ok(())
}

/// Emit a Rust `fn` for every registered imported classic helper (Phase
/// 11.7 R3.5a.2). Each `fn` is `#[allow(dead_code)]` (a `.fitzv` may
/// import a helper that a sibling calls but the template itself never
/// does) and lowers its body with the shared `lower_stmt` walker: NO
/// state fields (`state_names = &[]`), and the params seed the initial
/// local scope.
///
/// Params + return type are required — the WASM lowerer has no type
/// inference, so an un-annotated param or a missing return type rejects
/// with a clear message. The body may use the subset the lowerer
/// supports (`if`/`let`/`return`, struct literals, field access,
/// `.map`/`.filter`, free-fn calls, comparisons); an unsupported
/// construct (`match`, loops, `Result`/`?`) rejects, naming the fn.
/// CW.9 (1c) — client-WASM shim for fitz-liveviews's `Html` newtype. `Html`
/// wraps a raw (unescaped) markup string; on the wasm target we model it as a
/// struct so `.raw` field access + the `html`/`raw_html` constructors
/// transpile. A helper that returns `Html` (e.g. `icon` → an SVG string) then
/// compiles into the bundle, and the markup renders as DOM via the raw-HTML
/// sink at the interpolation site (`{raw_html(x.raw)}` → `set_inner_html`).
/// Escaping (`flv`, identity) and `List<Html>` folding
/// (`h_join`/`h_when`/`h_either`, still SSR-only) are unaffected.
const HTML_SHIM: &str = "\
#[allow(dead_code)]\n\
#[derive(Clone)]\n\
struct __FlvHtml {\n    raw: String,\n}\n\
#[allow(dead_code)]\n\
fn html(__s: String) -> __FlvHtml {\n    __FlvHtml { raw: __s }\n}\n\
#[allow(dead_code)]\n\
fn raw_html(__s: String) -> __FlvHtml {\n    __FlvHtml { raw: __s }\n}\n\n";

/// CW.9 (1c) — does the bundle need the [`HTML_SHIM`]? True when any imported
/// classic fn's signature references `Html` (param or return) — the common
/// case (`icon -> Html`). The `html`/`raw_html` constructors + `.raw` access
/// all key off that.
fn bundle_uses_html(fns: &ImportedFnRegistry) -> bool {
    fns.iter().any(|f| {
        f.ret.as_ref().is_some_and(type_expr_mentions_html)
            || f.params
                .iter()
                .any(|(_, t)| t.as_ref().is_some_and(type_expr_mentions_html))
    })
}

fn type_expr_mentions_html(ty: &TypeExpr) -> bool {
    match ty {
        TypeExpr::Named(n) => n == "Html",
        TypeExpr::Nullable(inner) => type_expr_mentions_html(inner),
        TypeExpr::Generic { args, .. } => args.iter().any(type_expr_mentions_html),
        TypeExpr::Tuple(items) => items.iter().any(type_expr_mentions_html),
        TypeExpr::Function { .. } => false,
    }
}

fn emit_imported_fns(
    fns: &ImportedFnRegistry,
    nominals: &NominalRegistry,
    out: &mut String,
) -> EmitResult<()> {
    if fns.is_empty() {
        return Ok(());
    }
    // Phase 11.11.c — emit the shared `fetch` runtime once when any
    // imported fn is `@rpc`.
    if fns.has_rpc() {
        emit_rpc_fetch_helper(out);
    }
    for f in fns.iter() {
        // Phase 11.11.c — an `@rpc` fn is a server function: emit an
        // async `fetch` stub instead of transpiling its body.
        if f.is_rpc {
            emit_rpc_stub(f, nominals, out)?;
            continue;
        }
        let mut param_sig: Vec<String> = Vec::with_capacity(f.params.len());
        let mut local_scope: Vec<String> = Vec::with_capacity(f.params.len());
        for (pname, pty) in &f.params {
            let ty = pty.as_ref().ok_or_else(|| EmitError {
                message: format!(
                    "imported fn `{}` param `{pname}` needs a type annotation for the \
                     client-WASM target (no type inference)",
                    f.name
                ),
                context: format!("imported fn `{}`", f.name),
            })?;
            let rust_ty = type_expr_to_rust(ty, nominals).map_err(|mut e| {
                e.context = format!("imported fn `{}` param `{pname}`", f.name);
                e
            })?;
            param_sig.push(format!("{pname}: {rust_ty}"));
            local_scope.push(pname.clone());
        }
        let ret = f.ret.as_ref().ok_or_else(|| EmitError {
            message: format!(
                "imported fn `{}` needs a return-type annotation for the client-WASM \
                 target (no type inference)",
                f.name
            ),
            context: format!("imported fn `{}`", f.name),
        })?;
        let ret_rust = type_expr_to_rust(ret, nominals).map_err(|mut e| {
            e.context = format!("imported fn `{}` return type", f.name);
            e
        })?;

        writeln!(out, "#[allow(dead_code)]").unwrap();
        writeln!(
            out,
            "fn {}({}) -> {} {{",
            f.name,
            param_sig.join(", "),
            ret_rust
        )
        .unwrap();
        let reassigned = collect_reassigned_locals(&f.body);
        for stmt in &f.body {
            lower_stmt(stmt, &[], &mut local_scope, "    ", &reassigned, out).map_err(
                |mut e| {
                    e.context = format!("imported fn `{}` (body)", f.name);
                    e
                },
            )?;
        }
        writeln!(out, "}}\n").unwrap();
    }
    Ok(())
}

/// Phase 11.11.c — the shared client-side `fetch` runtime. POSTs a
/// JSON body to a same-origin URL and returns `(status, text)`. The
/// session cookie rides along automatically on a same-origin request
/// (auth on the `@rpc` endpoint is a post-MVP refinement). Emitted
/// once per crate when any imported fn is `@rpc`. Mirrors the spike
/// validated in Chrome (web-sys 0.3.x `set_*` API).
fn emit_rpc_fetch_helper(out: &mut String) {
    out.push_str(
        "#[allow(dead_code)]\n\
         async fn __fitz_fetch_post(url: &str, body: &str) -> Result<(u16, String), String> {\n\
         \x20   use wasm_bindgen::JsCast;\n\
         \x20   use wasm_bindgen_futures::JsFuture;\n\
         \x20   use web_sys::{Headers, Request, RequestInit, Response};\n\
         \x20   let opts = RequestInit::new();\n\
         \x20   opts.set_method(\"POST\");\n\
         \x20   let headers = Headers::new().map_err(|_| \"rpc: headers\".to_string())?;\n\
         \x20   headers\n\
         \x20       .set(\"Content-Type\", \"application/json\")\n\
         \x20       .map_err(|_| \"rpc: content-type\".to_string())?;\n\
         \x20   opts.set_headers(&headers);\n\
         \x20   opts.set_body(&wasm_bindgen::JsValue::from_str(body));\n\
         \x20   let req = Request::new_with_str_and_init(url, &opts).map_err(|e| format!(\"rpc: {:?}\", e))?;\n\
         \x20   let win = web_sys::window().ok_or_else(|| \"rpc: no window\".to_string())?;\n\
         \x20   let rv = JsFuture::from(win.fetch_with_request(&req))\n\
         \x20       .await\n\
         \x20       .map_err(|e| format!(\"rpc: fetch failed: {:?}\", e))?;\n\
         \x20   let resp: Response = rv.dyn_into().map_err(|_| \"rpc: not a Response\".to_string())?;\n\
         \x20   let status = resp.status();\n\
         \x20   let tp = resp.text().map_err(|e| format!(\"rpc: {:?}\", e))?;\n\
         \x20   let t = JsFuture::from(tp).await.map_err(|e| format!(\"rpc: {:?}\", e))?;\n\
         \x20   Ok((status, t.as_string().unwrap_or_default()))\n\
         }\n\n",
    );
}

/// Phase 11.11.c — emit the client stub for an `@rpc async fn`. The
/// stub serializes the args into a JSON object, POSTs to
/// `/__rpc/<name>`, and maps the reply back to `Result<T, String>`:
/// HTTP 200 → deserialize `T` from the body; any other status → read
/// `{"error": ...}` into the `Err`. `T` is the inner type of the
/// declared `Result<T>` (bit-by-bit with the server's response
/// convention). Nominal params/returns rely on `serde` derives added
/// to the wasm structs when the crate uses rpc.
fn emit_rpc_stub(f: &ImportedFn, nominals: &NominalRegistry, out: &mut String) -> EmitResult<()> {
    let mut param_sig: Vec<String> = Vec::with_capacity(f.params.len());
    for (pname, pty) in &f.params {
        let ty = pty.as_ref().ok_or_else(|| EmitError {
            message: format!(
                "@rpc fn `{}` param `{pname}` needs a type annotation for the \
                 client-WASM target",
                f.name
            ),
            context: format!("@rpc fn `{}`", f.name),
        })?;
        let rust_ty = type_expr_to_rust(ty, nominals).map_err(|mut e| {
            e.context = format!("@rpc fn `{}` param `{pname}`", f.name);
            e
        })?;
        param_sig.push(format!("{pname}: {rust_ty}"));
    }
    // The return must be `Result<T>`; the stub returns
    // `Result<T_rust, String>`.
    let ret = f.ret.as_ref().ok_or_else(|| EmitError {
        message: format!(
            "@rpc fn `{}` must declare a `Result<T>` return type",
            f.name
        ),
        context: format!("@rpc fn `{}`", f.name),
    })?;
    let inner = match ret {
        TypeExpr::Generic { name, args, .. } if name == "Result" && args.len() == 1 => &args[0],
        _ => {
            return Err(EmitError {
                message: format!(
                    "@rpc fn `{}` must return `Result<T>` for the client-WASM target",
                    f.name
                ),
                context: format!("@rpc fn `{}`", f.name),
            })
        }
    };
    let ret_rust = type_expr_to_rust(inner, nominals).map_err(|mut e| {
        e.context = format!("@rpc fn `{}` return type", f.name);
        e
    })?;

    writeln!(out, "#[allow(dead_code)]").unwrap();
    writeln!(
        out,
        "async fn {}({}) -> Result<{}, String> {{",
        f.name,
        param_sig.join(", "),
        ret_rust
    )
    .unwrap();
    out.push_str("    let mut __args = serde_json::Map::new();\n");
    for (pname, _) in &f.params {
        writeln!(
            out,
            "    __args.insert(\"{pname}\".to_string(), serde_json::to_value(&{pname}).map_err(|e| e.to_string())?);",
        )
        .unwrap();
    }
    out.push_str("    let __body = serde_json::Value::Object(__args).to_string();\n");
    writeln!(
        out,
        "    let (__status, __text) = __fitz_fetch_post(\"/__rpc/{}\", &__body).await?;",
        f.name
    )
    .unwrap();
    out.push_str("    if __status == 200 {\n");
    writeln!(
        out,
        "        serde_json::from_str::<{}>(&__text).map_err(|e| format!(\"rpc {}: bad response: {{}}\", e))",
        ret_rust, f.name
    )
    .unwrap();
    out.push_str("    } else {\n");
    out.push_str(
        "        let __err: serde_json::Value = serde_json::from_str(&__text).unwrap_or(serde_json::Value::Null);\n",
    );
    out.push_str(
        "        Err(__err.get(\"error\").and_then(|e| e.as_str()).map(|s| s.to_string()).unwrap_or(__text))\n",
    );
    out.push_str("    }\n");
    out.push_str("}\n\n");
    Ok(())
}

// ---------------------------------------------------------------------------
// Module-level preludio
// ---------------------------------------------------------------------------

fn emit_module_header(out: &mut String) {
    out.push_str(
        "// Generated by fitz view WASM emitter (Fase 11.4.b).\n\
         // Do NOT edit — regenerate from the source `.fitzv`.\n\
         \n\
         // Phase 11.5.e (§9.o Debt residual) — crate-level allows for\n\
         // the two cosmetic warnings that are structural to the\n\
         // emitter output, not correctness bugs:\n\
         //   - `non_snake_case`: component names are PascalCase by\n\
         //     convention (11.5.d), and the synthesised style helper\n\
         //     `__inject_style_<Component>_<scope>` reflects that.\n\
         //   - `unused_parens`: BinOp lowering wraps sub-expressions\n\
         //     in `(...)` to preserve precedence when nested. When\n\
         //     the BinOp is the entire RHS of an assignment the outer\n\
         //     parens are redundant but harmless.\n\
         #![allow(non_snake_case, unused_parens)]\n\
         \n\
         use std::cell::RefCell;\n\
         use std::rc::Rc;\n\
         use std::sync::atomic::{AtomicBool, Ordering};\n\
         use wasm_bindgen::prelude::*;\n\
         use wasm_bindgen::JsCast;\n\
         use web_sys::{Event, HtmlElement};\n\
         \n",
    );
}

// ---------------------------------------------------------------------------
// Per-component emit
// ---------------------------------------------------------------------------

fn emit_component_impl(
    component: &ExpandedComponent,
    file: &ExpandedViewFile,
    nominals: &NominalRegistry,
    bubbled: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    out: &mut String,
) -> EmitResult<()> {
    // Phase 11.10 slice 4 — derived names read like state (`(*self.d.borrow())`),
    // backed by a cached `RefCell` field refreshed by `__recompute_derived()`.
    let state_names: Vec<String> = read_names(component);

    // Phase 11.7.c — this component's event names that some parent binds
    // via `<ThisComponent @event="..." />`. Each gets a callback slot +
    // a bubble call in its handler. Empty for non-bubbled components.
    let empty = std::collections::BTreeSet::new();
    let this_bubbled = bubbled.get(&component.name).unwrap_or(&empty);

    // Phase 11.10 slice 1 — keep-node reconciliation. When a component
    // has a live form control (`@input`/`@change`) over a static template,
    // emit the build-once + patch-in-place model instead of the naive
    // re-render, so a keystroke doesn't re-mount the `<input>` and drop the
    // caret. Every other component keeps the byte-identical naive path.
    if use_keep_node_reconciliation(component, this_bubbled) {
        return emit_component_keepnode(component, file, nominals, this_bubbled, out);
    }

    // Phase 11.7.e / R2b — collect every `<Child />` composition site
    // (in DFS order) so the struct can carry a typed instance-cache
    // slot per site. Each site is classified STATIC (fixed position,
    // one instance → `__child_slot_<n>: RefCell<Option<Rc<T>>>`) or
    // DYNAMIC (inside a `{#for}`, one instance PER key →
    // `__child_map_<n>: RefCell<HashMap<String, Rc<T>>>`). The render
    // walk descends identically (Element / If / For), assigning the
    // same static + dynamic indices in the same order, so the field
    // an index names here matches the one the render reads.
    let child_sites = collect_child_site_types(component);

    // Phase 11.7.d + named slots — the `<slot />` holes this component
    // declares. A default `<slot />` gives it a `__slot` field; each
    // `<slot name="X" />` a `__slot_<X>` field, set by a parent that
    // fills it via `<ThisComponent>...<el slot="X">...</el></ThisComponent>`.
    let slot_set = component_slot_set(component);
    validate_slot_set(&component.name, &slot_set)?;

    // Phase 11.12 slice 4 — a naive component hydrates when the tree opted in
    // (`component App hydrate { ... }` on the root, propagated). When hydratable
    // it carries `__hslot` adopt-callback fields and emits a `hydrate()` +
    // `__apply_state_json` + `__hydrate_slot_<n>` alongside the build path. When
    // not (every pre-11.12 example), none of that is emitted → byte-identical.
    let hydratable = component_is_hydratable(component, this_bubbled);

    emit_struct_and_new(
        component,
        &child_sites,
        nominals,
        this_bubbled,
        &slot_set,
        hydratable,
        out,
    )?;
    emit_event_handlers(component, &state_names, this_bubbled, out)?;
    emit_recompute_derived(component, &state_names, out)?;
    emit_mount_and_render(
        component,
        &state_names,
        file,
        nominals,
        this_bubbled,
        hydratable,
        out,
    )?;
    if let Some(style) = &component.style {
        emit_style_helper(&component.name, style, out);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Phase 11.10 slice 4 — derived values (`derived { name: T = expr }`)
// ---------------------------------------------------------------------------

/// State field names + derived names. Derived read exactly like state
/// (`(*self.<name>.borrow())`) — they are cached `RefCell` fields kept fresh
/// by `__recompute_derived()`, so `lower_expr` needs no separate resolution.
fn read_names(component: &ExpandedComponent) -> Vec<String> {
    component
        .state
        .iter()
        .chain(component.derived.iter())
        .map(|f| f.name.clone())
        .collect()
}

/// The zero value a derived `RefCell<T>` field is constructed with. It is
/// never read (mount runs `__recompute_derived()` before any render), so it
/// only needs to be a valid value of the derived's declared type. Primitive
/// types only in this slice; compound derived (List/Map/nominal) defer.
fn zero_value_for_type(type_expr: &TypeExpr, ctx: &str) -> EmitResult<String> {
    let v = match type_expr {
        TypeExpr::Named(n) => match n.as_str() {
            "Str" => "String::new()",
            "Int" => "0i64",
            "Float" => "0.0f64",
            "Bool" => "false",
            _ => {
                return Err(EmitError {
                    message: format!(
                        "derived of type `{}` — only Str / Int / Float / Bool derived are \
                         supported in this slice",
                        n
                    ),
                    context: ctx.to_string(),
                })
            }
        },
        _ => {
            return Err(EmitError {
                message: "compound derived type (List / Map / nominal) — only primitive \
                          derived are supported in this slice"
                    .to_string(),
                context: ctx.to_string(),
            })
        }
    };
    Ok(v.to_string())
}

/// Emit the `__recompute_derived()` method: assign each derived field its
/// freshly-computed value, in declaration order (so a derived may read an
/// earlier derived's just-set field). Emits nothing when the component has no
/// derived (byte-identical output for components that don't use the feature).
/// `render()` calls this once before build/patch.
fn emit_recompute_derived(
    component: &ExpandedComponent,
    read: &[String],
    out: &mut String,
) -> EmitResult<()> {
    if component.derived.is_empty() {
        return Ok(());
    }
    writeln!(out, "    fn __recompute_derived(self: &Rc<Self>) {{").unwrap();
    for d in &component.derived {
        let expr_rust = lower_expr(&d.default, read, &[]).map_err(|mut e| {
            e.context = format!("derived `{}` of component `{}`", d.name, component.name);
            e
        })?;
        writeln!(
            out,
            "        *self.{}.borrow_mut() = {};",
            d.name, expr_rust
        )
        .unwrap();
    }
    writeln!(out, "    }}\n").unwrap();
    Ok(())
}

// ---------------------------------------------------------------------------
// Phase 11.10 slice 1 — keep-node reconciliation (build once, patch in place)
// ---------------------------------------------------------------------------

/// Decide whether `component` uses the keep-node reconciliation model
/// (Phase 11.10) instead of the naive re-render.
///
/// Gated to the case it fixes — a live form control (`@input`/`@change`)
/// whose surrounding text/attributes interpolate state (the caret case) —
/// and restricted so the model stays sound:
///
/// - **no composition** (no `<slot>`/`<Child />` anywhere): child instance
///   caches + slot callbacks are a separate mechanism the keep-node path
///   doesn't drive yet, so a component that composes stays naive.
///   `{#if}`/`{#for}` ARE allowed — slice 3 rebuilds each as an anchored
///   dynamic region while a live sibling `<input>` keeps its caret.
/// - **not a bubbled child** (`this_bubbled` empty): a component whose
///   events a parent binds is re-mounted by that parent's naive render,
///   which would reset `__built`; keeping it naive avoids a stale-handle
///   patch. A root/standalone component (the live-input case) mounts once.
///
/// Every other component keeps the byte-identical naive re-render.
fn use_keep_node_reconciliation(
    component: &ExpandedComponent,
    this_bubbled: &std::collections::BTreeSet<String>,
) -> bool {
    this_bubbled.is_empty()
        && component_uses_value_input(component)
        && template_has_no_composition(component)
}

/// `true` when the template composes no other component and declares no
/// `<slot>` anywhere (`{#if}`/`{#for}`/`Element` are fine). Composition
/// drives the child instance cache + slot callbacks, which the keep-node
/// path does not manage — such components stay on the naive re-render.
fn template_has_no_composition(component: &ExpandedComponent) -> bool {
    fn node_ok(node: &ExpandedTemplateNode) -> bool {
        match node {
            ExpandedTemplateNode::Slot { .. } | ExpandedTemplateNode::ChildComponent { .. } => {
                false
            }
            ExpandedTemplateNode::Element { children, .. } => children.iter().all(node_ok),
            ExpandedTemplateNode::If {
                children,
                else_children,
                ..
            } => {
                children.iter().all(node_ok)
                    && else_children
                        .as_ref()
                        .is_none_or(|els| els.iter().all(node_ok))
            }
            ExpandedTemplateNode::For { children, .. } => children.iter().all(node_ok),
            _ => true,
        }
    }
    component
        .template
        .as_ref()
        .is_some_and(|t| t.roots.iter().all(node_ok))
}

/// `true` when a keep-node component has at least one `{#if}`/`{#for}`, so
/// the emitter needs `Comment` + `DocumentFragment` web-sys features for the
/// dynamic-region anchors + fragment (Phase 11.10 slice 3). Only consulted
/// for value-input components, so caret-free crates keep their feature set.
fn component_uses_keep_regions(component: &ExpandedComponent) -> bool {
    fn has_control_flow(node: &ExpandedTemplateNode) -> bool {
        match node {
            ExpandedTemplateNode::If { .. } | ExpandedTemplateNode::For { .. } => true,
            ExpandedTemplateNode::Element { children, .. } => children.iter().any(has_control_flow),
            _ => false,
        }
    }
    component_uses_value_input(component)
        && template_has_no_composition(component)
        && component
            .template
            .as_ref()
            .is_some_and(|t| t.roots.iter().any(has_control_flow))
}

/// Phase 11.12 — `true` when a component can be HYDRATED.
///
/// Two roads to hydration:
///
/// - **Keep-node (auto, slices 1–3)** — a live-form component takes the
///   keep-node path (`__ktext_<n>` / `__kattr_<n>` handles + `__build`/`__patch`
///   model); its adopt walk populates those handles from the server DOM and
///   also adopts `{#if}`/`{#for}` regions. This needs no opt-in.
/// - **Naive composition (opt-in, slice 4)** — a naive component (composes
///   `<Child />` / has a `<slot>`, or is the root that does) hydrates when the
///   `hydrate` flag is set. The flag is authored on the ROOT via `component App
///   hydrate { ... }` and propagated to the whole tree (`propagate_root_hydrate`),
///   so a composed child hydrates alongside its parent. The naive `hydrate()`
///   adopts the DOM + wires listeners once; a later state change still re-renders
///   naively (the composition model has no in-place patch). Opt-in keeps
///   pre-11.12 composition examples byte-identical.
///
/// `this_bubbled` must be the component's real bubbled-event set (empty for a
/// root component — nothing composes it).
pub fn component_is_hydratable(
    component: &ExpandedComponent,
    this_bubbled: &std::collections::BTreeSet<String>,
) -> bool {
    component.hydrate || use_keep_node_reconciliation(component, this_bubbled)
}

/// Phase 11.12 — `true` when any component in `file` will emit a `hydrate()`
/// method, so the module needs the `__flv_next_element` / `__flv_next_text`
/// cursor helpers. Uses each component's real bubbled-event set.
fn any_component_hydratable(
    file: &ExpandedViewFile,
    bubbled: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
) -> bool {
    let empty = std::collections::BTreeSet::new();
    file.components.iter().any(|c| {
        let tb = bubbled.get(&c.name).unwrap_or(&empty);
        component_is_hydratable(c, tb)
    })
}

/// Phase 11.12 slice 2 — `true` when some hydratable component in `file` has a
/// `{#if}`/`{#for}` region, so its adopt walk calls `__flv_next_comment`. Gates
/// the region-anchor cursor helper so a region-free hydratable crate carries no
/// unused function.
fn any_component_hydratable_with_regions(
    file: &ExpandedViewFile,
    bubbled: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
) -> bool {
    let empty = std::collections::BTreeSet::new();
    file.components.iter().any(|c| {
        let tb = bubbled.get(&c.name).unwrap_or(&empty);
        component_is_hydratable(c, tb) && component_uses_keep_regions(c)
    })
}

/// Phase 11.12 — `true` when a `.fitzv` file compiles any hydratable
/// component, so its wasm crate needs the `serde_json` dep (state restore).
/// Used by `wasm_build::write_wasm_crate_scaffold` to gate the Cargo.toml dep.
pub fn file_uses_hydration(file: &ExpandedViewFile) -> bool {
    let bubbled = collect_bubbled_events(file);
    any_component_hydratable(file, &bubbled)
}

/// Phase 11.12 — the shared cursor helpers the adopt walk calls. Emitted once
/// per module, only when some component is hydratable (so non-hydration crates
/// stay byte-identical). Each advances a sibling cursor to the next element /
/// text node, skipping whitespace/comment nodes, so the adopt walk lines up
/// with the build DFS regardless of insignificant server-side whitespace.
/// Slice 2 adds `__flv_next_comment`, which advances to a tagged comment (a
/// `{#if}`/`{#for}` region anchor); it is only emitted when `with_regions`, so
/// a region-free hydratable crate carries no unused helper.
fn emit_hydration_helpers(out: &mut String, with_regions: bool) {
    out.push_str(
        "// Phase 11.12 — hydration cursor helpers. Advance a sibling cursor to\n\
         // the next element / text node so the adopt walk maps template nodes\n\
         // onto the server-painted DOM in DFS order without re-creating them.\n\
         fn __flv_next_element(__cursor: &mut Option<web_sys::Node>) -> Option<web_sys::Element> {\n\
         \x20   while let Some(__n) = __cursor.clone() {\n\
         \x20       *__cursor = __n.next_sibling();\n\
         \x20       if let Some(__el) = __n.dyn_ref::<web_sys::Element>() {\n\
         \x20           return Some(__el.clone());\n\
         \x20       }\n\
         \x20   }\n\
         \x20   None\n\
         }\n\
         fn __flv_next_text(__cursor: &mut Option<web_sys::Node>) -> Option<web_sys::Text> {\n\
         \x20   while let Some(__n) = __cursor.clone() {\n\
         \x20       *__cursor = __n.next_sibling();\n\
         \x20       if let Some(__t) = __n.dyn_ref::<web_sys::Text>() {\n\
         \x20           return Some(__t.clone());\n\
         \x20       }\n\
         \x20   }\n\
         \x20   None\n\
         }\n",
    );
    if with_regions {
        out.push_str(
            "// Phase 11.12 slice 2 — advance the cursor to the next comment node\n\
             // whose text matches `__data`, skipping everything else (elements,\n\
             // text, and comments with other tags). Used to adopt a `{#if}`/`{#for}`\n\
             // region's server-painted anchors (`<!--fr-->` / `<!--/fr-->`) while\n\
             // stepping over the region content and any interpolation markers it\n\
             // contains. `text_content()` reads the comment data without needing the\n\
             // `web_sys::Comment` feature.\n\
             fn __flv_next_comment(__cursor: &mut Option<web_sys::Node>, __data: &str) -> Option<web_sys::Node> {\n\
             \x20   while let Some(__n) = __cursor.clone() {\n\
             \x20       *__cursor = __n.next_sibling();\n\
             \x20       if __n.node_type() == web_sys::Node::COMMENT_NODE\n\
             \x20           && __n.text_content().as_deref() == Some(__data) {\n\
             \x20           return Some(__n);\n\
             \x20       }\n\
             \x20   }\n\
             \x20   None\n\
             }\n",
        );
    }
    out.push('\n');
}

/// Phase 11.12 slice 3 — build a Rust expression that restores a state field
/// of type `ty` from a `&serde_json::Value` bound to `val`, evaluating to
/// `Option<T_rust>` (`Some` = restored, `None` = the JSON did not match, so the
/// field keeps its default). Recurses so composite state — `List<T>`,
/// `Map<Str, V>`, `Nullable<T>`, imported nominals, and their nestings —
/// restores from the SSR state payload instead of staying at the default.
///
/// Returns `None` (the outer Option) when the type can't be restored from JSON
/// at all: a `Map` with a non-`Str` key (JSON objects key on strings), tuples,
/// functions, or an unknown nominal. Such fields keep their default, exactly as
/// before this slice.
///
/// Variable naming is role-specific (`__no` nominal object, `__nf` nominal
/// field, `__le` list element, `__mk`/`__mv` map key/value) so nested closures
/// never shadow ambiguously.
fn json_restore_value(ty: &TypeExpr, nominals: &NominalRegistry, val: &str) -> Option<String> {
    match ty {
        TypeExpr::Named(name) => match name.as_str() {
            "Int" => Some(format!("{val}.as_i64()")),
            "Float" => Some(format!("{val}.as_f64()")),
            "Bool" => Some(format!("{val}.as_bool()")),
            "Str" => Some(format!("{val}.as_str().map(|__s| __s.to_string())")),
            other => {
                // An imported nominal: build the struct field by field. Every
                // field must be present and restorable, else the whole nominal
                // stays `None` (keep default) — we never synthesise a partial.
                let fields = nominals.fields(other)?;
                let mut field_lines: Vec<String> = Vec::with_capacity(fields.len());
                for (fname, fty) in fields {
                    let inner = json_restore_value(fty, nominals, "__nf")?;
                    field_lines.push(format!(
                        "{fname}: {{ let __nf = __no.get({fname:?})?; ({inner})? }}"
                    ));
                }
                Some(format!(
                    "(|__no: &serde_json::Value| -> Option<{other}> {{ Some({other} {{ {} }}) }})({val})",
                    field_lines.join(", ")
                ))
            }
        },
        TypeExpr::Nullable(inner) => {
            // `null` restores to `Some(None)` (a successful parse of the null
            // value); a valid inner restores to `Some(Some(v))`; anything else
            // is `None` (keep default). `val` is always a plain binding here, so
            // evaluating it twice is pure.
            let inner_expr = json_restore_value(inner, nominals, val)?;
            Some(format!(
                "if {val}.is_null() {{ Some(None) }} else {{ ({inner_expr}).map(Some) }}"
            ))
        }
        TypeExpr::Generic { name, args } if name == "List" && args.len() == 1 => {
            let inner_expr = json_restore_value(&args[0], nominals, "__le")?;
            let inner_rust = type_expr_to_rust(&args[0], nominals).ok()?;
            Some(format!(
                "{val}.as_array().map(|__arr| __arr.iter().filter_map(|__le| {inner_expr}).collect::<Vec<{inner_rust}>>())"
            ))
        }
        TypeExpr::Generic { name, args } if name == "Map" && args.len() == 2 => {
            // JSON objects key on strings, so only `Map<Str, V>` round-trips.
            match &args[0] {
                TypeExpr::Named(k) if k == "Str" => {}
                _ => return None,
            }
            let inner_v = json_restore_value(&args[1], nominals, "__mv")?;
            let v_rust = type_expr_to_rust(&args[1], nominals).ok()?;
            Some(format!(
                "{val}.as_object().map(|__obj| __obj.iter().filter_map(|(__mk, __mv)| ({inner_v}).map(|__vv| (__mk.clone(), __vv))).collect::<Vec<(String, {v_rust})>>())"
            ))
        }
        _ => None,
    }
}

/// Phase 11.12 — map a primitive state field's Rust type to the
/// `serde_json::Value` accessor + the RHS expression used to restore it in
/// `__apply_state_json`. Scalars keep this exact form (byte-identical to
/// slice 1/2); composite fields restore via [`json_restore_value`].
fn json_state_accessor(rust_ty: &str) -> Option<(&'static str, &'static str)> {
    match rust_ty {
        "String" => Some(("as_str()", "__x.to_string()")),
        "i64" => Some(("as_i64()", "__x")),
        "f64" => Some(("as_f64()", "__x")),
        "bool" => Some(("as_bool()", "__x")),
        _ => None,
    }
}

/// Phase 11.12 — emit `__apply_state_json`, which restores the serialized
/// state (the SSR `<script type="application/json">` payload) into the
/// component's state cells. Slice 1/2 restored only primitive scalars; slice 3
/// also restores composite state — `List<T>`, `Map<Str, V>`, `Nullable<T>`, and
/// imported nominals (recursively, via [`json_restore_value`]). A field whose
/// JSON does not match its type keeps its default; types that cannot round-trip
/// through JSON at all (tuples, functions, `Map` with a non-`Str` key) are
/// skipped.
fn emit_apply_state_json(
    component: &ExpandedComponent,
    nominals: &NominalRegistry,
    out: &mut String,
) -> EmitResult<()> {
    writeln!(
        out,
        "    fn __apply_state_json(self: &Rc<Self>, __json: &str) {{"
    )
    .unwrap();
    writeln!(
        out,
        "        let __v: serde_json::Value = match serde_json::from_str(__json) {{ Ok(__j) => __j, Err(_) => return, }};"
    )
    .unwrap();
    // Silence unused warnings when the component has no primitive state.
    writeln!(out, "        let _ = (&__v, self);").unwrap();
    for field in &component.state {
        let rust_ty = type_expr_to_rust(&field.type_expr, nominals).map_err(|mut e| {
            e.context = format!(
                "state field `{}` of component `{}` (hydrate state apply)",
                field.name, component.name
            );
            e
        })?;
        if let Some((accessor, rhs)) = json_state_accessor(&rust_ty) {
            // Scalar — byte-identical to slice 1/2.
            writeln!(
                out,
                "        if let Some(__x) = __v.get(\"{}\").and_then(|__j| __j.{}) {{ *self.{}.borrow_mut() = {}; }}",
                field.name, accessor, field.name, rhs
            )
            .unwrap();
        } else if let Some(restore) = json_restore_value(&field.type_expr, nominals, "__fv") {
            // Phase 11.12 slice 3 — composite state (List / Map / Nullable /
            // nominal). Restore from the payload; a shape mismatch keeps the
            // default (both `if let Some` guards fall through).
            writeln!(
                out,
                "        if let Some(__fv) = __v.get(\"{}\") {{",
                field.name
            )
            .unwrap();
            writeln!(
                out,
                "            if let Some(__restored) = {{ {restore} }} {{ *self.{}.borrow_mut() = __restored; }}",
                field.name
            )
            .unwrap();
            writeln!(out, "        }}").unwrap();
        }
    }
    writeln!(out, "    }}\n").unwrap();
    Ok(())
}

/// Phase 11.13 slice-3 — inverse of [`json_restore_value`]: builds a Rust
/// expression that serializes a composite state field into a
/// `serde_json::Value` (recursively over `List<T>`, `Map<Str, V>`,
/// `Nullable<T>`, and imported nominals). `ref_expr` is a `&T_rust` binding
/// (every recursion level threads a fresh `&T` binding — `__le` list element,
/// `__mv` map value, `__iv` nullable inner, `__fv` nominal field — so scalar
/// leaves never need a `*&` deref that clippy would flag).
///
/// Returns `None` for types that cannot round-trip through JSON (a `Map` with a
/// non-`Str` key, tuples, functions, unknown nominal) — those fields are simply
/// omitted from the snapshot and reset to their default on reload, symmetric
/// with `json_restore_value` returning `None` for the same shapes.
fn json_dump_value(ty: &TypeExpr, nominals: &NominalRegistry, ref_expr: &str) -> Option<String> {
    match ty {
        TypeExpr::Named(name) => match name.as_str() {
            "Int" | "Float" | "Bool" | "Str" => Some(format!(
                "serde_json::to_value({ref_expr}).unwrap_or(serde_json::Value::Null)"
            )),
            other => {
                // Imported nominal: build a JSON object field by field. Each
                // field is bound to a fresh `&T` (`__fv`) before recursing.
                let fields = nominals.fields(other)?;
                let mut lines: Vec<String> =
                    vec!["let mut __o = serde_json::Map::new();".to_string()];
                for (fname, fty) in fields {
                    let inner = json_dump_value(fty, nominals, "__fv")?;
                    lines.push(format!(
                        "__o.insert({fname:?}.to_string(), {{ let __fv = &__nv.{fname}; {inner} }});"
                    ));
                }
                lines.push("serde_json::Value::Object(__o)".to_string());
                Some(format!(
                    "(|__nv: &{other}| -> serde_json::Value {{ {} }})({ref_expr})",
                    lines.join(" ")
                ))
            }
        },
        TypeExpr::Nullable(inner) => {
            let inner_dump = json_dump_value(inner, nominals, "__iv")?;
            Some(format!(
                "match {ref_expr} {{ Some(__iv) => {inner_dump}, None => serde_json::Value::Null }}"
            ))
        }
        TypeExpr::Generic { name, args } if name == "List" && args.len() == 1 => {
            let inner_dump = json_dump_value(&args[0], nominals, "__le")?;
            Some(format!(
                "serde_json::Value::Array({ref_expr}.iter().map(|__le| {inner_dump}).collect::<Vec<serde_json::Value>>())"
            ))
        }
        TypeExpr::Generic { name, args } if name == "Map" && args.len() == 2 => {
            // JSON objects key on strings, so only `Map<Str, V>` round-trips.
            match &args[0] {
                TypeExpr::Named(k) if k == "Str" => {}
                _ => return None,
            }
            let inner_v = json_dump_value(&args[1], nominals, "__mv")?;
            Some(format!(
                "serde_json::Value::Object({ref_expr}.iter().map(|(__mk, __mv)| (__mk.clone(), {inner_v})).collect::<serde_json::Map<String, serde_json::Value>>())"
            ))
        }
        _ => None,
    }
}

/// Phase 11.13 slice-2/3 (`fitz dev` hot reload — state preservation) —
/// emit a dev-only `impl <Name>` with `__fitz_dev_snapshot()` (state →
/// JSON string) and `__fitz_dev_apply(&str)` (JSON → state, then
/// `render()`). The composed dev entry wrapper stashes the snapshot in
/// `sessionStorage` on `beforeunload` and re-applies it after mount, so
/// a hot reload preserves live state. **Only emitted in `fitz dev`'s
/// dev-mode build** (`fitz build` never carries this — the prod `lib.rs`
/// stays byte-identical).
///
/// Covers **primitive** state (`Int`/`Float`/`Str`/`Bool` — via
/// [`json_state_accessor`]) AND, since slice-3, **composite** state:
/// `List<T>`, `Map<Str, V>`, `Nullable<T>`, and imported nominals restore
/// via [`json_restore_value`] and serialize via its inverse
/// [`json_dump_value`], both recursive. A field whose type can't round-trip
/// through JSON (a `Map` with a non-`Str` key, tuples, functions) is omitted
/// from the snapshot and keeps its default on reload.
pub fn emit_dev_state_methods(
    component: &ExpandedComponent,
    nominals: &NominalRegistry,
    out: &mut String,
) -> EmitResult<()> {
    let name = &component.name;
    writeln!(out, "impl {name} {{").unwrap();

    // snapshot: build a JSON object of the primitive state fields.
    writeln!(
        out,
        "    pub fn __fitz_dev_snapshot(self: &Rc<Self>) -> String {{"
    )
    .unwrap();
    writeln!(out, "        let _ = self;").unwrap();
    writeln!(out, "        let mut __m = serde_json::Map::new();").unwrap();
    for field in &component.state {
        let rust_ty = type_expr_to_rust(&field.type_expr, nominals).map_err(|mut e| {
            e.context = format!(
                "state field `{}` of component `{}` (dev snapshot)",
                field.name, component.name
            );
            e
        })?;
        if json_state_accessor(&rust_ty).is_some() {
            // Scalar — byte-identical to slice 2.
            let val = if rust_ty == "String" {
                format!("self.{}.borrow().clone()", field.name)
            } else {
                format!("*self.{}.borrow()", field.name)
            };
            writeln!(
                out,
                "        __m.insert(\"{}\".to_string(), serde_json::json!({}));",
                field.name, val
            )
            .unwrap();
        } else if let Some(dump) = json_dump_value(
            &field.type_expr,
            nominals,
            // Parenthesized so a trailing `.iter()` / `match` binds to the
            // `&T`, not to the `Ref` from `borrow()` (method-call precedence).
            &format!("(&*self.{}.borrow())", field.name),
        ) {
            // Composite (List / Map<Str,V> / Nullable / nominal) — slice 3.
            writeln!(
                out,
                "        __m.insert(\"{}\".to_string(), {{ {} }});",
                field.name, dump
            )
            .unwrap();
        }
    }
    writeln!(out, "        serde_json::Value::Object(__m).to_string()").unwrap();
    writeln!(out, "    }}").unwrap();

    // apply: restore the primitive fields, then render.
    writeln!(
        out,
        "    pub fn __fitz_dev_apply(self: &Rc<Self>, __json: &str) {{"
    )
    .unwrap();
    writeln!(
        out,
        "        let __v: serde_json::Value = match serde_json::from_str(__json) {{ Ok(__j) => __j, Err(_) => return, }};"
    )
    .unwrap();
    writeln!(out, "        let _ = &__v;").unwrap();
    for field in &component.state {
        let rust_ty = type_expr_to_rust(&field.type_expr, nominals).map_err(|mut e| {
            e.context = format!(
                "state field `{}` of component `{}` (dev apply)",
                field.name, component.name
            );
            e
        })?;
        if let Some((accessor, rhs)) = json_state_accessor(&rust_ty) {
            // Scalar — byte-identical to slice 2.
            writeln!(
                out,
                "        if let Some(__x) = __v.get(\"{}\").and_then(|__j| __j.{}) {{ *self.{}.borrow_mut() = {}; }}",
                field.name, accessor, field.name, rhs
            )
            .unwrap();
        } else if let Some(restore) = json_restore_value(&field.type_expr, nominals, "__fv") {
            // Composite (List / Map<Str,V> / Nullable / nominal) — slice 3.
            // A shape mismatch keeps the default (both `if let` guards fall
            // through), mirroring `emit_apply_state_json`.
            writeln!(
                out,
                "        if let Some(__fv) = __v.get(\"{}\") {{",
                field.name
            )
            .unwrap();
            writeln!(
                out,
                "            if let Some(__restored) = {{ {restore} }} {{ *self.{}.borrow_mut() = __restored; }}",
                field.name
            )
            .unwrap();
            writeln!(out, "        }}").unwrap();
        }
    }
    writeln!(out, "        self.render();").unwrap();
    writeln!(out, "    }}").unwrap();

    writeln!(out, "}}\n").unwrap();
    Ok(())
}

/// Phase 11.12 — emit `hydrate(root)`: inject the scoped style, restore the
/// serialized state from the SSR `<script>`, refresh derived cells, then run
/// the adopt walk over the server-painted DOM (no wipe, no rebuild) and mark
/// the component built so later state changes patch in place.
fn emit_hydrate_method(
    component: &ExpandedComponent,
    name: &str,
    hydrate_body: &str,
    mark_built: bool,
    out: &mut String,
) {
    writeln!(
        out,
        "    pub fn hydrate(self: &Rc<Self>, root: HtmlElement) -> Result<(), JsValue> {{"
    )
    .unwrap();
    if let Some(style) = &component.style {
        let helper = style_helper_ident(name, style);
        writeln!(out, "        {}();", helper).unwrap();
    }
    // Restore the serialized state from the SSR `<script type="application/
    // json" id="__flv_state_<Component>">`, if present. Absent → keep defaults.
    writeln!(
        out,
        "        if let Some(__sel) = web_sys::window().unwrap().document().unwrap().get_element_by_id(\"__flv_state_{}\") {{",
        name
    )
    .unwrap();
    writeln!(
        out,
        "            if let Some(__txt) = __sel.text_content() {{ self.__apply_state_json(&__txt); }}"
    )
    .unwrap();
    writeln!(out, "        }}").unwrap();
    if !component.derived.is_empty() {
        writeln!(out, "        self.__recompute_derived();").unwrap();
    }
    writeln!(out, "        let mut __cur_root = root.first_child();").unwrap();
    writeln!(out, "        *self.root.borrow_mut() = Some(root);").unwrap();
    out.push_str(hydrate_body);
    // Keep-node components flip `__built` so the next state change patches in
    // place (slices 1–3). The naive composition path (slice 4) has no `__built`
    // field — it re-renders wholesale on the next change — so this is skipped.
    if mark_built {
        writeln!(out, "        *self.__built.borrow_mut() = true;").unwrap();
    }
    writeln!(out, "        Ok(())").unwrap();
    writeln!(out, "    }}\n").unwrap();
}

/// Emit a component under the keep-node reconciliation model
/// (Phase 11.10 slice 1). The render walk runs once into a `__build()`
/// body that also stashes a handle per dynamic point into a struct field;
/// `render()` builds on first mount then dispatches to `__patch()`, which
/// updates only those handles. The `<input>` element itself is never
/// re-created, so the caret survives a keystroke.
fn emit_component_keepnode(
    component: &ExpandedComponent,
    file: &ExpandedViewFile,
    nominals: &NominalRegistry,
    this_bubbled: &std::collections::BTreeSet<String>,
    out: &mut String,
) -> EmitResult<()> {
    let name = &component.name;
    // Phase 11.10 slice 4 — derived read like state (cached `RefCell` fields).
    let state_names: Vec<String> = read_names(component);

    // 1. Run the build walk into a buffer, collecting keep-node handle
    //    fields + patch statements as a side effect (see `RenderCtx.keep`).
    let mut build_body = String::new();
    let mut keep_fields: Vec<(String, String)> = Vec::new();
    let mut patch_stmts: Vec<String> = Vec::new();
    let mut region_methods: Vec<String> = Vec::new();
    if let Some(template) = &component.template {
        let mut ctx = RenderCtx::new(
            name,
            &state_names,
            &component.state,
            file,
            nominals,
            &component.events,
            this_bubbled,
        );
        ctx.keep = Some(KeepAccum::default());
        for root_node in &template.roots {
            emit_template_node(root_node, "root", &mut ctx, &mut build_body)?;
        }
        let accum = ctx.keep.take().unwrap();
        keep_fields = accum.fields;
        patch_stmts = accum.patch;
        region_methods = accum.region_methods;
        // The keep-node gate guarantees no `<slot>`/`<Child />`, so the
        // ctx accumulated no slot_methods and no child-cache sites.
    }

    // Phase 11.12 — every keep-node component is HYDRATABLE (slice 2 lifted the
    // region restriction). Run the walk a second time in adopt mode to produce
    // the `hydrate()` body: it acquires each node from the server-painted DOM
    // into the same `__ktext_<n>` / `__kattr_<n>` handles the build walk
    // declared, and adopts each `{#if}`/`{#for}` region's server anchors into
    // the same `__astart_<r>` / `__aend_<r>` handles. The walk is structurally
    // identical, so `keep_index()` / `keep_region_index()` yield the same
    // indices; the region's `__mount_region` / `__patch_region` methods (built
    // by the first walk) still drive later state changes.
    let mut hydrate_body = String::new();
    if let Some(template) = &component.template {
        let mut hctx = RenderCtx::new(
            name,
            &state_names,
            &component.state,
            file,
            nominals,
            &component.events,
            this_bubbled,
        );
        hctx.keep = Some(KeepAccum::default());
        hctx.hydrate = true;
        for root_node in &template.roots {
            emit_template_node(root_node, "__cur_root", &mut hctx, &mut hydrate_body)?;
        }
        // The adopt accum is discarded — the build walk already collected the
        // handle fields + patch statements + region methods.
    }

    // 2. struct — state cells + keep-node handle fields + build flag + root.
    writeln!(out, "pub struct {} {{", name).unwrap();
    for field in &component.state {
        let rust_ty = type_expr_to_rust(&field.type_expr, nominals).map_err(|mut e| {
            e.context = format!(
                "state field `{}` of component `{}` (type)",
                field.name, name
            );
            e
        })?;
        writeln!(out, "    {}: RefCell<{}>,", field.name, rust_ty).unwrap();
    }
    // Phase 11.10 slice 4 — cached derived cells.
    for field in &component.derived {
        let rust_ty = type_expr_to_rust(&field.type_expr, nominals).map_err(|mut e| {
            e.context = format!("derived `{}` of component `{}` (type)", field.name, name);
            e
        })?;
        writeln!(out, "    {}: RefCell<{}>,", field.name, rust_ty).unwrap();
    }
    for (fname, fty) in &keep_fields {
        writeln!(out, "    {}: RefCell<Option<{}>>,", fname, fty).unwrap();
    }
    writeln!(out, "    __built: RefCell<bool>,").unwrap();
    writeln!(out, "    root: RefCell<Option<HtmlElement>>,").unwrap();
    writeln!(out, "}}\n").unwrap();

    // 3. impl + new()
    writeln!(out, "impl {} {{", name).unwrap();
    writeln!(out, "    pub fn new() -> Rc<Self> {{").unwrap();
    writeln!(out, "        Rc::new({} {{", name).unwrap();
    for field in &component.state {
        let default_rust =
            default_expr_to_rust(&field.default, &field.type_expr).map_err(|mut e| {
                e.context = format!(
                    "state field `{}` of component `{}` (default)",
                    field.name, name
                );
                e
            })?;
        writeln!(
            out,
            "            {}: RefCell::new({}),",
            field.name, default_rust
        )
        .unwrap();
    }
    // Phase 11.10 slice 4 — derived cells zero-init; `__recompute_derived()`
    // fills them before the first render.
    for field in &component.derived {
        let zero = zero_value_for_type(
            &field.type_expr,
            &format!("derived `{}` of component `{}`", field.name, name),
        )?;
        writeln!(out, "            {}: RefCell::new({}),", field.name, zero).unwrap();
    }
    for (fname, _) in &keep_fields {
        writeln!(out, "            {}: RefCell::new(None),", fname).unwrap();
    }
    writeln!(out, "            __built: RefCell::new(false),").unwrap();
    writeln!(out, "            root: RefCell::new(None),").unwrap();
    writeln!(out, "        }})").unwrap();
    writeln!(out, "    }}\n").unwrap();

    // 4. event handlers — unchanged; each mutates a state cell then calls
    //    `self.render()`, which now dispatches to `__patch()`.
    emit_event_handlers(component, &state_names, this_bubbled, out)?;
    emit_recompute_derived(component, &state_names, out)?;

    // 5. mount / mount_into / render dispatch / __build / __patch.
    writeln!(
        out,
        "    pub fn mount(self: &Rc<Self>, selector: &str) -> Result<(), JsValue> {{"
    )
    .unwrap();
    writeln!(
        out,
        "        let document = web_sys::window().unwrap().document().unwrap();"
    )
    .unwrap();
    writeln!(out, "        let root = document").unwrap();
    writeln!(out, "            .query_selector(selector)?").unwrap();
    writeln!(
        out,
        "            .ok_or_else(|| JsValue::from_str(\"mount: selector matched no element\"))?"
    )
    .unwrap();
    writeln!(out, "            .dyn_into::<HtmlElement>()?;").unwrap();
    writeln!(out, "        self.mount_into(root)").unwrap();
    writeln!(out, "    }}\n").unwrap();

    writeln!(
        out,
        "    pub fn mount_into(self: &Rc<Self>, root: HtmlElement) -> Result<(), JsValue> {{"
    )
    .unwrap();
    if let Some(style) = &component.style {
        let helper = style_helper_ident(name, style);
        writeln!(out, "        {}();", helper).unwrap();
    }
    // A fresh mount rebuilds (any prior handles point into a discarded DOM).
    writeln!(out, "        *self.__built.borrow_mut() = false;").unwrap();
    writeln!(out, "        *self.root.borrow_mut() = Some(root);").unwrap();
    writeln!(out, "        self.render();").unwrap();
    writeln!(out, "        Ok(())").unwrap();
    writeln!(out, "    }}\n").unwrap();

    writeln!(out, "    fn render(self: &Rc<Self>) {{").unwrap();
    // Phase 11.10 slice 4 — refresh derived cells before build/patch read them.
    if !component.derived.is_empty() {
        writeln!(out, "        self.__recompute_derived();").unwrap();
    }
    writeln!(out, "        if *self.__built.borrow() {{").unwrap();
    writeln!(out, "            self.__patch();").unwrap();
    writeln!(out, "        }} else {{").unwrap();
    writeln!(out, "            self.__build();").unwrap();
    writeln!(out, "            *self.__built.borrow_mut() = true;").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "    }}\n").unwrap();

    writeln!(out, "    fn __build(self: &Rc<Self>) {{").unwrap();
    writeln!(out, "        let root_ref = self.root.borrow();").unwrap();
    writeln!(out, "        let root = match root_ref.as_ref() {{").unwrap();
    writeln!(out, "            Some(r) => r,").unwrap();
    writeln!(out, "            None => return,").unwrap();
    writeln!(out, "        }};").unwrap();
    writeln!(out, "        while let Some(child) = root.first_child() {{").unwrap();
    writeln!(out, "            let _ = root.remove_child(&child);").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(
        out,
        "        let document = web_sys::window().unwrap().document().unwrap();"
    )
    .unwrap();
    out.push_str(&build_body);
    writeln!(out, "    }}\n").unwrap();

    writeln!(out, "    fn __patch(self: &Rc<Self>) {{").unwrap();
    if patch_stmts.is_empty() {
        writeln!(out, "        let _ = self;").unwrap();
    }
    for stmt in &patch_stmts {
        writeln!(out, "{}", stmt).unwrap();
    }
    writeln!(out, "    }}").unwrap();

    // Phase 11.12 — hydration: a `__apply_state_json` + `hydrate()` pair that
    // restores the serialized state and adopts the server-painted DOM instead
    // of rebuilding it. Every keep-node component gets it (slice 2 lifted the
    // region restriction); the entry wrapper calls `hydrate()` when the mount
    // root already has server-painted DOM.
    writeln!(out).unwrap();
    emit_apply_state_json(component, nominals, out)?;
    emit_hydrate_method(component, name, &hydrate_body, true, out);

    // Phase 11.10 slice 3 — one `__mount_region_<n>` + `__patch_region_<n>`
    // pair per `{#if}`/`{#for}` dynamic region. A component with no regions
    // emits nothing here, byte-identical to the slice-1 keep-node output.
    for m in &region_methods {
        writeln!(out).unwrap();
        out.push_str(m);
    }

    writeln!(out, "}}\n").unwrap();

    if let Some(style) = &component.style {
        emit_style_helper(name, style, out);
    }
    Ok(())
}

/// A `<Child />` composition site discovered in a component's
/// template. Phase 11.7.e (static) + R2b (dynamic).
struct ChildSite {
    /// The child component's declared name (`"Card"`), used both as
    /// the Rust type in the cache field and to look the child up when
    /// emitting props.
    child_ty: String,
    /// `true` when the site sits inside a `{#for}` loop. A dynamic
    /// site maps a stable `key` → one child instance
    /// (`HashMap<String, Rc<T>>`); a static site holds a single
    /// optional instance (`Option<Rc<T>>`).
    dynamic: bool,
}

/// Collect every `<Child />` composition site in `component`'s
/// template, in DFS order, classified static vs dynamic (Phase
/// 11.7.e + R2b).
///
/// Descends into `Element`, `{#if}` (both branches), and `{#for}`
/// children — the SAME descent the render walk does — so the static
/// / dynamic index a site is assigned here matches the counter the
/// render advances. Sites inside a `{#for}` are marked `dynamic`.
fn collect_child_site_types(component: &ExpandedComponent) -> Vec<ChildSite> {
    let mut sites = Vec::new();
    if let Some(template) = &component.template {
        for node in &template.roots {
            collect_sites_in_node(node, false, &mut sites);
        }
    }
    sites
}

fn collect_sites_in_node(node: &ExpandedTemplateNode, in_for: bool, sites: &mut Vec<ChildSite>) {
    match node {
        ExpandedTemplateNode::ChildComponent { name, .. } => sites.push(ChildSite {
            child_ty: name.clone(),
            dynamic: in_for,
        }),
        ExpandedTemplateNode::Element { children, .. } => {
            for child in children {
                collect_sites_in_node(child, in_for, sites);
            }
        }
        ExpandedTemplateNode::If {
            children,
            else_children,
            ..
        } => {
            for child in children {
                collect_sites_in_node(child, in_for, sites);
            }
            if let Some(else_kids) = else_children {
                for child in else_kids {
                    collect_sites_in_node(child, in_for, sites);
                }
            }
        }
        ExpandedTemplateNode::For { children, .. } => {
            for child in children {
                collect_sites_in_node(child, true, sites);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Struct + new()
// ---------------------------------------------------------------------------

fn emit_struct_and_new(
    component: &ExpandedComponent,
    child_sites: &[ChildSite],
    nominals: &NominalRegistry,
    this_bubbled: &std::collections::BTreeSet<String>,
    slot_set: &SlotSet,
    hydratable: bool,
    out: &mut String,
) -> EmitResult<()> {
    let name = &component.name;

    writeln!(out, "pub struct {} {{", name).unwrap();
    for field in &component.state {
        let rust_ty = type_expr_to_rust(&field.type_expr, nominals).map_err(|mut e| {
            e.context = format!(
                "state field `{}` of component `{}` (type)",
                field.name, name
            );
            e
        })?;
        writeln!(out, "    {}: RefCell<{}>,", field.name, rust_ty).unwrap();
    }
    // Phase 11.10 slice 4 — one cached `RefCell<T>` field per derived value,
    // refreshed by `__recompute_derived()` (read like state).
    for field in &component.derived {
        let rust_ty = type_expr_to_rust(&field.type_expr, nominals).map_err(|mut e| {
            e.context = format!("derived `{}` of component `{}` (type)", field.name, name);
            e
        })?;
        writeln!(out, "    {}: RefCell<{}>,", field.name, rust_ty).unwrap();
    }
    // Phase 11.7.e / R2b — one instance-cache field per `<Child />`
    // site. STATIC sites hold a single optional instance so the child
    // survives parent re-renders (its state persists — the DOM is
    // rebuilt but the cached instance is reused instead of
    // `Child::new()`). DYNAMIC sites (inside a `{#for}`) map a stable
    // `key` → one instance, so each keyed child keeps its own state
    // across re-renders and reconciliation evicts vanished keys.
    let mut static_idx = 0usize;
    let mut dyn_idx = 0usize;
    for site in child_sites {
        if site.dynamic {
            writeln!(
                out,
                "    __child_map_{}: RefCell<std::collections::HashMap<String, Rc<{}>>>,",
                dyn_idx, site.child_ty
            )
            .unwrap();
            dyn_idx += 1;
        } else {
            writeln!(
                out,
                "    __child_slot_{}: RefCell<Option<Rc<{}>>>,",
                static_idx, site.child_ty
            )
            .unwrap();
            static_idx += 1;
        }
    }
    // Phase 11.7.c — one callback slot per bubbled event, set by the
    // parent that binds `<ThisComponent @event="..." />` and invoked by
    // this component's own `event` handler when it fires. Phase 11.7
    // payload bubbling — the callback carries the child's event payload
    // (`&HashMap<String, String>`), so the parent handler can tell which
    // child fired + read its `data-flv-value-*` data.
    for ev in this_bubbled {
        writeln!(
            out,
            "    __on_{}: RefCell<Option<Box<dyn Fn(&std::collections::HashMap<String, String>)>>>,",
            ev
        )
        .unwrap();
    }
    // Phase 11.7.d + named slots — parent-provided slot-content
    // renderers. `__slot` backs the default `<slot />`; `__slot_<name>`
    // backs each `<slot name="X" />`. `None` when the parent didn't fill
    // that slot (the `<slot />` renders its own fallback instead).
    if slot_set.has_default {
        writeln!(
            out,
            "    __slot: RefCell<Option<Rc<dyn Fn(&web_sys::Node)>>>,"
        )
        .unwrap();
    }
    for slot_name in &slot_set.named {
        writeln!(
            out,
            "    {}: RefCell<Option<Rc<dyn Fn(&web_sys::Node)>>>,",
            slot_field_name(Some(slot_name))
        )
        .unwrap();
    }
    // Phase 11.12 slice 4 — hydration adopt callbacks, one per slot the
    // component declares, holding a cursor-consuming renderer that adopts the
    // parent-painted slot content during the initial `hydrate()`. Only emitted
    // for a hydratable component (byte-identical for pre-11.12 slot examples).
    if hydratable {
        if slot_set.has_default {
            writeln!(
                out,
                "    __hslot: RefCell<Option<Rc<dyn Fn(&mut Option<web_sys::Node>)>>>,"
            )
            .unwrap();
        }
        for slot_name in &slot_set.named {
            writeln!(
                out,
                "    {}: RefCell<Option<Rc<dyn Fn(&mut Option<web_sys::Node>)>>>,",
                hslot_field_name(Some(slot_name))
            )
            .unwrap();
        }
    }
    writeln!(out, "    root: RefCell<Option<HtmlElement>>,").unwrap();
    writeln!(out, "}}\n").unwrap();

    writeln!(out, "impl {} {{", name).unwrap();
    writeln!(out, "    pub fn new() -> Rc<Self> {{").unwrap();
    writeln!(out, "        Rc::new({} {{", name).unwrap();
    for field in &component.state {
        let default_rust =
            default_expr_to_rust(&field.default, &field.type_expr).map_err(|mut e| {
                e.context = format!(
                    "state field `{}` of component `{}` (default)",
                    field.name, name
                );
                e
            })?;
        writeln!(
            out,
            "            {}: RefCell::new({}),",
            field.name, default_rust
        )
        .unwrap();
    }
    // Phase 11.10 slice 4 — derived cells start at a type-appropriate zero;
    // `__recompute_derived()` fills them before the first render reads them.
    for field in &component.derived {
        let zero = zero_value_for_type(
            &field.type_expr,
            &format!("derived `{}` of component `{}`", field.name, name),
        )?;
        writeln!(out, "            {}: RefCell::new({}),", field.name, zero).unwrap();
    }
    let mut static_idx = 0usize;
    let mut dyn_idx = 0usize;
    for site in child_sites {
        if site.dynamic {
            writeln!(
                out,
                "            __child_map_{}: RefCell::new(std::collections::HashMap::new()),",
                dyn_idx
            )
            .unwrap();
            dyn_idx += 1;
        } else {
            writeln!(
                out,
                "            __child_slot_{}: RefCell::new(None),",
                static_idx
            )
            .unwrap();
            static_idx += 1;
        }
    }
    for ev in this_bubbled {
        writeln!(out, "            __on_{}: RefCell::new(None),", ev).unwrap();
    }
    if slot_set.has_default {
        writeln!(out, "            __slot: RefCell::new(None),").unwrap();
    }
    for slot_name in &slot_set.named {
        writeln!(
            out,
            "            {}: RefCell::new(None),",
            slot_field_name(Some(slot_name))
        )
        .unwrap();
    }
    if hydratable {
        if slot_set.has_default {
            writeln!(out, "            __hslot: RefCell::new(None),").unwrap();
        }
        for slot_name in &slot_set.named {
            writeln!(
                out,
                "            {}: RefCell::new(None),",
                hslot_field_name(Some(slot_name))
            )
            .unwrap();
        }
    }
    writeln!(out, "            root: RefCell::new(None),").unwrap();
    writeln!(out, "        }})").unwrap();
    writeln!(out, "    }}\n").unwrap();
    // (impl block stays open — event handlers + mount + render close it)
    Ok(())
}

// ---------------------------------------------------------------------------
// Event handlers
// ---------------------------------------------------------------------------

fn emit_event_handlers(
    component: &ExpandedComponent,
    state_names: &[String],
    this_bubbled: &std::collections::BTreeSet<String>,
    out: &mut String,
) -> EmitResult<()> {
    for handler in &component.events {
        emit_event_handler(&component.name, handler, state_names, this_bubbled, out)?;
    }
    Ok(())
}

fn emit_event_handler(
    component_name: &str,
    handler: &ExpandedEventHandler,
    state_names: &[String],
    this_bubbled: &std::collections::BTreeSet<String>,
    out: &mut String,
) -> EmitResult<()> {
    if !handler.params.is_empty() {
        return Err(EmitError {
            message: "event handlers with parameters are deferred to Phase 11.5".to_string(),
            context: format!(
                "event handler `{}` of component `{}`",
                handler.name, component_name
            ),
        });
    }

    // Phase 11.7 R3.5b.1 — a handler that reads `payload` (`payload["k"]`
    // / `payload.has("k")`) takes a `payload: &HashMap<String, String>`
    // param. A handler that does NOT reference payload keeps its
    // zero-arg signature, so the pre-R3.5b examples emit byte-for-byte
    // unchanged. The `data-flv-click` / `data-flv-submit` wiring builds
    // the payload and calls with the matching arity.
    //
    // Phase 11.7 payload bubbling — a bubbled handler ALSO takes the
    // `payload` param (even with a body that never reads it), so it can
    // forward the payload it received up to the parent via `__cb(payload)`.
    let uses_payload = handler_uses_payload(handler) || this_bubbled.contains(&handler.name);
    let bubbles = this_bubbled.contains(&handler.name);
    // Phase 11.11.c — a handler whose body `.await`s (a call to an
    // `@rpc` stub) can't run in the sync DOM-event closure. Emit a sync
    // wrapper that clones `self` (and `payload`) and hands the work to
    // an owned-`Rc<Self>` async worker via `spawn_local`. The worker
    // returns `Result<(), String>` so the source `.await?` propagates;
    // on success it mutates state and re-renders, on error it logs to
    // the console. The naive re-render fires ONCE at the end of the
    // worker (a mid-body "loading" flash is a later signals slice).
    if handler_uses_await(handler) {
        let (sync_params, worker_params, call_args) = if uses_payload {
            (
                ", payload: &std::collections::HashMap<String, String>",
                ", payload: std::collections::HashMap<String, String>",
                "__c, __pl",
            )
        } else {
            ("", "", "__c")
        };
        writeln!(
            out,
            "    fn {}(self: &Rc<Self>{}) {{",
            handler.name, sync_params
        )
        .unwrap();
        writeln!(out, "        let __c = self.clone();").unwrap();
        if uses_payload {
            writeln!(out, "        let __pl = payload.clone();").unwrap();
        }
        writeln!(
            out,
            "        wasm_bindgen_futures::spawn_local(async move {{"
        )
        .unwrap();
        writeln!(
            out,
            "            if let Err(__e) = Self::__{}_async({}).await {{",
            handler.name, call_args
        )
        .unwrap();
        writeln!(
            out,
            "                web_sys::console::error_1(&format!(\"rpc: {{}}\", __e).into());"
        )
        .unwrap();
        writeln!(out, "            }}").unwrap();
        writeln!(out, "        }});").unwrap();
        writeln!(out, "    }}\n").unwrap();

        writeln!(
            out,
            "    async fn __{}_async(self: Rc<Self>{}) -> Result<(), String> {{",
            handler.name, worker_params
        )
        .unwrap();
        let mut locals: Vec<String> = Vec::new();
        let reassigned = collect_reassigned_locals(&handler.body);
        // Phase 11.10 slice 2 — mid-flight render. When a statement writes
        // state and a later statement suspends on `.await`, flush a render
        // right before that suspension so a "loading" state set before the
        // await is painted while the async work is in flight. Handlers with
        // no state write before their await (the plain fetch-then-assign
        // pattern) emit byte-identically — no extra render is inserted.
        let mut pending_state_write = false;
        for stmt in &handler.body {
            if pending_state_write && stmt_uses_await(stmt) {
                writeln!(out, "        self.render();").unwrap();
                pending_state_write = false;
            }
            lower_stmt(stmt, state_names, &mut locals, "        ", &reassigned, out).map_err(
                |mut e| {
                    e.context = format!(
                        "async event handler `{}` of component `{}` (body)",
                        handler.name, component_name
                    );
                    e
                },
            )?;
            if stmt_assigns_state(stmt, state_names) {
                pending_state_write = true;
            }
        }
        writeln!(out, "        self.render();").unwrap();
        if bubbles {
            writeln!(
                out,
                "        if let Some(__cb) = self.__on_{}.borrow().as_ref() {{ __cb(&payload); }}",
                handler.name
            )
            .unwrap();
        }
        writeln!(out, "        Ok(())").unwrap();
        writeln!(out, "    }}\n").unwrap();
        return Ok(());
    }

    if uses_payload {
        writeln!(
            out,
            "    fn {}(self: &Rc<Self>, payload: &std::collections::HashMap<String, String>) {{",
            handler.name
        )
        .unwrap();
    } else {
        writeln!(out, "    fn {}(self: &Rc<Self>) {{", handler.name).unwrap();
    }
    // Locals introduced by `let`-style statements accumulate across the
    // body so later statements (and closures) can reference them.
    let mut locals: Vec<String> = Vec::new();
    let reassigned = collect_reassigned_locals(&handler.body);
    for stmt in &handler.body {
        lower_stmt(stmt, state_names, &mut locals, "        ", &reassigned, out).map_err(
            |mut e| {
                e.context = format!(
                    "event handler `{}` of component `{}` (body)",
                    handler.name, component_name
                );
                e
            },
        )?;
    }
    writeln!(out, "        self.render();").unwrap();
    // Phase 11.7.c — if a parent bound `@<name>` on this component, fire
    // the registered bubble callback after the handler ran + re-rendered.
    // Phase 11.7 payload bubbling — forward this handler's `payload` (the
    // param the bubbled signature always carries) up to the parent.
    if this_bubbled.contains(&handler.name) {
        writeln!(
            out,
            "        if let Some(__cb) = self.__on_{}.borrow().as_ref() {{ __cb(payload); }}",
            handler.name
        )
        .unwrap();
    }
    writeln!(out, "    }}\n").unwrap();
    Ok(())
}

/// Phase 11.11.c — `true` when an event handler `.await`s anywhere in
/// its body (a call to an `@rpc` stub). Such handlers are emitted as a
/// sync wrapper + an async worker (see `emit_event_handler`). Mirrors
/// the `handler_uses_payload` walk.
fn handler_uses_await(handler: &ExpandedEventHandler) -> bool {
    handler.body.iter().any(stmt_uses_await)
}

fn stmt_uses_await(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Assign { value, .. } => expr_uses_await(value),
        Stmt::Return(e, _) => expr_uses_await(e),
        Stmt::Expr(e, _) => expr_uses_await(e),
        Stmt::For { iter, body, .. } => expr_uses_await(iter) || body.iter().any(stmt_uses_await),
        _ => false,
    }
}

/// Phase 11.10 slice 2 — `true` when `stmt` is a top-level reassignment of a
/// state field (`loading = true`), the writes an async worker must paint
/// before it suspends on an `.await`. Nested writes (inside `{#if}`/`{#for}`)
/// are conservatively ignored — the loading pattern is a top-level write.
fn stmt_assigns_state(stmt: &Stmt, state_names: &[String]) -> bool {
    matches!(
        stmt,
        Stmt::Assign {
            target: AssignTarget::Ident(name, _),
            is_let: false,
            ..
        } if state_names.iter().any(|s| s == name)
    )
}

fn expr_uses_await(expr: &Expr) -> bool {
    match expr {
        Expr::Await(_, _) => true,
        Expr::Try(inner, _) => expr_uses_await(inner),
        Expr::BinOp { left, right, .. } => expr_uses_await(left) || expr_uses_await(right),
        Expr::UnaryOp { operand, .. } => expr_uses_await(operand),
        Expr::Call { callee, args, .. } => {
            expr_uses_await(callee) || args.iter().any(expr_uses_await)
        }
        Expr::Field { object, .. } => expr_uses_await(object),
        Expr::Index { object, index, .. } => expr_uses_await(object) || expr_uses_await(index),
        Expr::List(items, _) => items.iter().any(expr_uses_await),
        Expr::StructLit { fields, .. } => fields.iter().any(|(_, e)| expr_uses_await(e)),
        Expr::If {
            condition,
            then,
            else_,
            ..
        } => {
            expr_uses_await(condition)
                || then.iter().any(stmt_uses_await)
                || else_
                    .as_ref()
                    .is_some_and(|els| els.iter().any(stmt_uses_await))
        }
        Expr::Match { value, arms, .. } => {
            expr_uses_await(value) || arms.iter().any(|a| a.body.iter().any(stmt_uses_await))
        }
        Expr::FnExpr { body, .. } => body.iter().any(stmt_uses_await),
        Expr::StrInterp(parts, _) => parts.iter().any(|p| match p {
            StrPart::Expr(e, _) => expr_uses_await(e),
            StrPart::Lit(_) => false,
        }),
        Expr::Range { start, end, .. } => expr_uses_await(start) || expr_uses_await(end),
        _ => false,
    }
}

/// True when an event handler references `payload` anywhere in its body
/// (Phase 11.7 R3.5b.1). Both `payload["k"]` and `payload.has("k")`
/// contain `Expr::Ident("payload")` as a sub-expression, so a walk for
/// that identifier catches every use.
fn handler_uses_payload(handler: &ExpandedEventHandler) -> bool {
    handler.body.iter().any(stmt_uses_payload)
}

fn stmt_uses_payload(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Assign { value, .. } => expr_uses_payload(value),
        Stmt::Return(e, _) => expr_uses_payload(e),
        Stmt::Expr(e, _) => expr_uses_payload(e),
        _ => false,
    }
}

fn expr_uses_payload(expr: &Expr) -> bool {
    match expr {
        Expr::Ident(name, _) => name == "payload",
        Expr::BinOp { left, right, .. } => expr_uses_payload(left) || expr_uses_payload(right),
        Expr::UnaryOp { operand, .. } => expr_uses_payload(operand),
        Expr::Call { callee, args, .. } => {
            expr_uses_payload(callee) || args.iter().any(expr_uses_payload)
        }
        Expr::Field { object, .. } => expr_uses_payload(object),
        Expr::Index { object, index, .. } => expr_uses_payload(object) || expr_uses_payload(index),
        Expr::List(items, _) => items.iter().any(expr_uses_payload),
        Expr::StructLit { fields, .. } => fields.iter().any(|(_, e)| expr_uses_payload(e)),
        Expr::If {
            condition,
            then,
            else_,
            ..
        } => {
            expr_uses_payload(condition)
                || then.iter().any(stmt_uses_payload)
                || else_
                    .as_ref()
                    .is_some_and(|els| els.iter().any(stmt_uses_payload))
        }
        Expr::FnExpr { body, .. } => body.iter().any(stmt_uses_payload),
        Expr::StrInterp(parts, _) => parts.iter().any(|p| match p {
            StrPart::Expr(e, _) => expr_uses_payload(e),
            StrPart::Lit(_) => false,
        }),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// mount() + render()
// ---------------------------------------------------------------------------

fn emit_mount_and_render(
    component: &ExpandedComponent,
    state_names: &[String],
    file: &ExpandedViewFile,
    nominals: &NominalRegistry,
    this_bubbled: &std::collections::BTreeSet<String>,
    hydratable: bool,
    out: &mut String,
) -> EmitResult<()> {
    let name = &component.name;

    // ---- mount() -------------------------------------------------
    // Phase 11.5.d — factored into `mount(selector)` (public entry
    // point for the composed WASM `start()` — resolves the
    // selector in the DOM) and `mount_into(root)` (used by child
    // components: `<Child />` composition sites hand the parent's
    // element directly). Both share the "inject style once + store
    // root + render" logic via `mount_into`.
    writeln!(
        out,
        "    pub fn mount(self: &Rc<Self>, selector: &str) -> Result<(), JsValue> {{"
    )
    .unwrap();
    writeln!(
        out,
        "        let document = web_sys::window().unwrap().document().unwrap();"
    )
    .unwrap();
    writeln!(out, "        let root = document").unwrap();
    writeln!(out, "            .query_selector(selector)?").unwrap();
    writeln!(
        out,
        "            .ok_or_else(|| JsValue::from_str(\"mount: selector matched no element\"))?"
    )
    .unwrap();
    writeln!(out, "            .dyn_into::<HtmlElement>()?;").unwrap();
    writeln!(out, "        self.mount_into(root)").unwrap();
    writeln!(out, "    }}\n").unwrap();

    // ---- mount_into() --------------------------------------------
    // Attaches the component into an existing `HtmlElement` root.
    // Consumed by `<Child />` composition sites (Phase 11.5.d) —
    // the parent creates a wrapper element and hands it to the
    // child via this entry point. Also the underlying implementation
    // of `mount(selector)`.
    writeln!(
        out,
        "    pub fn mount_into(self: &Rc<Self>, root: HtmlElement) -> Result<(), JsValue> {{"
    )
    .unwrap();
    if let Some(style) = &component.style {
        let helper = style_helper_ident(name, style);
        writeln!(out, "        {}();", helper).unwrap();
    }
    writeln!(out, "        *self.root.borrow_mut() = Some(root);").unwrap();
    writeln!(out, "        self.render();").unwrap();
    writeln!(out, "        Ok(())").unwrap();
    writeln!(out, "    }}\n").unwrap();

    // ---- render() ------------------------------------------------
    writeln!(out, "    fn render(self: &Rc<Self>) {{").unwrap();
    // Phase 11.10 slice 4 — refresh derived cells from current state before
    // the render reads them.
    if !component.derived.is_empty() {
        writeln!(out, "        self.__recompute_derived();").unwrap();
    }
    writeln!(out, "        let root_ref = self.root.borrow();").unwrap();
    writeln!(out, "        let root = match root_ref.as_ref() {{").unwrap();
    writeln!(out, "            Some(r) => r,").unwrap();
    writeln!(out, "            None => return,").unwrap();
    writeln!(out, "        }};").unwrap();
    writeln!(out, "        while let Some(child) = root.first_child() {{").unwrap();
    writeln!(out, "            let _ = root.remove_child(&child);").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(
        out,
        "        let document = web_sys::window().unwrap().document().unwrap();"
    )
    .unwrap();

    let mut slot_methods: Vec<Vec<ExpandedTemplateNode>> = Vec::new();
    if let Some(template) = &component.template {
        let mut ctx = RenderCtx::new(
            name,
            state_names,
            &component.state,
            file,
            nominals,
            &component.events,
            this_bubbled,
        );
        for root_node in &template.roots {
            emit_template_node(root_node, "root", &mut ctx, out)?;
        }
        slot_methods = ctx.slot_methods;
    }

    writeln!(out, "    }}").unwrap();

    // Phase 11.7.d — one `__render_slot_<n>` method per `<Child>content
    // </Child>` site, rendering the parent-provided content (in PARENT
    // scope: parent state + events) into a target node handed by the
    // child at its `<slot />`.
    for (i, content) in slot_methods.iter().enumerate() {
        writeln!(
            out,
            "    fn __render_slot_{}(self: &Rc<Self>, __target: &web_sys::Node) {{",
            i
        )
        .unwrap();
        writeln!(
            out,
            "        let document = web_sys::window().unwrap().document().unwrap();"
        )
        .unwrap();
        let mut ctx = RenderCtx::new(
            name,
            state_names,
            &component.state,
            file,
            nominals,
            &component.events,
            this_bubbled,
        );
        for node in content {
            emit_template_node(node, "__target", &mut ctx, out)?;
        }
        writeln!(out, "    }}\n").unwrap();
    }

    // Phase 11.12 slice 4 — naive hydration. When the tree opted in
    // (`component App hydrate { ... }`, propagated), a naive composition
    // component adopts the server-painted DOM on boot instead of fresh-mounting:
    // it restores its serialized state, then runs the template walk in ADOPT
    // mode (acquiring nodes from a cursor, wiring listeners, `child.hydrate()`ing
    // composed children, adopting `<slot>` content) — WITHOUT wiping. It has no
    // `__built` flag, so the next state change still re-renders wholesale via the
    // naive `render()`; hydration only removes the initial flash + preserves the
    // server DOM (JS witnesses survive) for the first paint.
    if hydratable {
        let mut hydrate_body = String::new();
        let mut hslot_methods: Vec<Vec<ExpandedTemplateNode>> = Vec::new();
        if let Some(template) = &component.template {
            let mut hctx = RenderCtx::new(
                name,
                state_names,
                &component.state,
                file,
                nominals,
                &component.events,
                this_bubbled,
            );
            // Adopt mode; `keep` stays `None` (naive → no keep-node handles).
            hctx.hydrate = true;
            for root_node in &template.roots {
                emit_template_node(root_node, "__cur_root", &mut hctx, &mut hydrate_body)?;
            }
            hslot_methods = hctx.slot_methods;
        }
        emit_apply_state_json(component, nominals, out)?;
        // Naive `hydrate()` — no `__built` to flip.
        emit_hydrate_method(component, name, &hydrate_body, false, out);

        // One `__hydrate_slot_<n>` per `<Child>content</Child>` site (parallel
        // to `__render_slot_<n>`), adopting the parent-provided slot content —
        // in PARENT scope — from the child's cursor at its `<slot />`. The index
        // matches `__render_slot_<n>` because both walks traverse the template in
        // the same DFS order (so `__slot`→`__render_slot_n`, `__hslot`→
        // `__hydrate_slot_n` refer to the same content).
        for (i, content) in hslot_methods.iter().enumerate() {
            writeln!(
                out,
                "    fn __hydrate_slot_{}(self: &Rc<Self>, __cursor: &mut Option<web_sys::Node>) {{",
                i
            )
            .unwrap();
            let mut hctx = RenderCtx::new(
                name,
                state_names,
                &component.state,
                file,
                nominals,
                &component.events,
                this_bubbled,
            );
            hctx.hydrate = true;
            // `__cursor` is already a `&mut Option<Node>` param; the adopt
            // emitters wrap the cursor var in `&mut {...}`, so pass the deref
            // place `(*__cursor)` to reborrow it (`&mut (*__cursor)`) instead of
            // taking `&mut &mut …`. Inner element cursors are fresh owned locals.
            for node in content {
                emit_template_node(node, "(*__cursor)", &mut hctx, out)?;
            }
            writeln!(out, "    }}\n").unwrap();
        }
    }

    writeln!(out, "}}\n").unwrap();
    Ok(())
}

// ---------------------------------------------------------------------------
// Template rendering emit
// ---------------------------------------------------------------------------

/// Ctx that walks the template tree accumulating unique var names
/// (`__el0`, `__el1`, `__t0`, `__interp0`, ...) so multiple elements
/// in the same `render()` body don't shadow each other.
struct RenderCtx<'a> {
    component_name: &'a str,
    state_names: &'a [String],
    /// Phase 11.5.d — every OTHER component in the same file,
    /// keyed by name. Consumed by `emit_child_component` to
    /// resolve the child's declared state-field types when
    /// coercing static prop values into Rust literals.
    file: &'a ExpandedViewFile,
    /// Phase 11.7.b — the component's state fields, so `{#for x in
    /// items}` can resolve `items`'s element type (`List<T>` → `T`).
    state_fields: &'a [ExpandedStateField],
    var_counter: usize,
    /// Phase 11.7.e — running index of the STATIC `<Child />` site
    /// being emitted (matches `__child_slot_<n>`). Incremented per
    /// static site in the same DFS order as `collect_child_site_types`.
    static_site_counter: usize,
    /// Phase 11.7.b R2b — running index of the DYNAMIC `<Child />`
    /// site being emitted (matches `__child_map_<n>`). Incremented per
    /// dynamic site (one inside a `{#for}`) in the same DFS order.
    dyn_site_counter: usize,
    /// Phase 11.7.b R2b — `true` while emitting the children of a
    /// `{#for}` loop. Tells `emit_child_component` to reconcile the
    /// child through the keyed instance cache (`__child_map_<n>`)
    /// instead of the single static slot.
    in_for: bool,
    /// Phase 11.7.b — loop variables currently in scope (from
    /// enclosing `{#for x in ...}`). An `Expr::Ident` that names a
    /// local resolves to the Rust loop var directly, shadowing state.
    locals: Vec<String>,
    /// Phase 11.7 R3 — imported classic nominals, so `{#for c in
    /// cards}` can accept a `List<Card>` element type and reject an
    /// unknown nominal with a clear message.
    nominals: &'a NominalRegistry,
    /// Phase 11.7 R3.5b.1 — the component's event handlers, so a
    /// `data-flv-click="handler"` binding can resolve the target and
    /// know whether it takes a `payload` argument.
    events: &'a [ExpandedEventHandler],
    /// Phase 11.7 payload bubbling — this component's event names that
    /// some parent binds via `<ThisComponent @event="..." />`. A bubbled
    /// handler always carries a `payload` param (so it can forward the
    /// payload it received up to the parent), even when its own body
    /// doesn't read `payload`. `event_takes_payload` consults this so
    /// every call site emits the matching arity.
    this_bubbled: &'a std::collections::BTreeSet<String>,
    /// Phase 11.7.d — slot-content node lists accumulated from
    /// `<Child>content</Child>` sites during the render walk. Each entry
    /// becomes a `__render_slot_<idx>` method; the index is the position
    /// in this vec, matched by `emit_child_component` when it wires the
    /// child's `__slot` callback.
    slot_methods: Vec<Vec<ExpandedTemplateNode>>,
    /// Phase 11.10 slice 1 — keep-node reconciliation accumulator. `Some`
    /// only while emitting a keep-node component's `__build()` body: each
    /// dynamic point (text interpolation, interpolated / mixed attribute)
    /// stashes a DOM-node handle into a component struct field and records
    /// a patch statement, so a later state change patches that node in
    /// place instead of rebuilding the whole subtree (which would re-mount
    /// a live `<input>` and drop the caret). `None` for the naive
    /// re-render path — the emitted code is then byte-identical to before.
    keep: Option<KeepAccum>,
    /// Phase 11.12 — hydration adopt walk. When `true`, the node emitters
    /// take over the server-painted DOM instead of creating it: an element
    /// is acquired from a sibling cursor (`__flv_next_element`) rather than
    /// `create_element`, a text node from `__flv_next_text` rather than
    /// `create_text_node`, and neither is appended. Interpolation/attribute
    /// keep-node handles are still stashed (from the adopted node), and event
    /// listeners still attach — so a later state change patches in place. A
    /// `{#if}`/`{#for}` region (slice 2) adopts its server anchors into the
    /// same `__astart_<r>` / `__aend_<r>` handles the build walk declared and
    /// leaves the content in place. In this mode `parent_var` holds the NAME of
    /// the parent's child cursor (`Option<web_sys::Node>`), not the parent
    /// element var. `false` for the build walk — that path stays byte-identical.
    hydrate: bool,
}

/// Phase 11.10 slices 1 + 3 — accumulator threaded through the keep-node
/// build walk. `next` numbers each dynamic point in DFS order; `fields` are
/// the `RefCell<Option<web_sys::Text|Element|Node>>` handle fields the
/// component struct carries; `patch` are the statements of the component's
/// `__patch()` body (one per dynamic point + one per dynamic region).
///
/// Slice 3 adds **dynamic regions** for `{#if}`/`{#for}`: `region_next`
/// numbers each region, and `region_methods` holds the emitted
/// `__mount_region_<n>` / `__patch_region_<n>` methods (a region rebuilds
/// wholesale between two comment anchors, reusing the naive `{#if}`/`{#for}`
/// emit into a `DocumentFragment`, so a live sibling `<input>` keeps its
/// caret while the region's content changes).
#[derive(Default)]
struct KeepAccum {
    next: usize,
    fields: Vec<(String, String)>,
    patch: Vec<String>,
    region_next: usize,
    region_methods: Vec<String>,
}

impl<'a> RenderCtx<'a> {
    fn new(
        component_name: &'a str,
        state_names: &'a [String],
        state_fields: &'a [ExpandedStateField],
        file: &'a ExpandedViewFile,
        nominals: &'a NominalRegistry,
        events: &'a [ExpandedEventHandler],
        this_bubbled: &'a std::collections::BTreeSet<String>,
    ) -> Self {
        RenderCtx {
            component_name,
            state_names,
            state_fields,
            file,
            var_counter: 0,
            static_site_counter: 0,
            dyn_site_counter: 0,
            in_for: false,
            locals: Vec::new(),
            nominals,
            events,
            this_bubbled,
            slot_methods: Vec::new(),
            keep: None,
            hydrate: false,
        }
    }

    /// Phase 11.10 — allocate the next keep-node handle index (DFS order).
    /// Only valid while `self.keep.is_some()` (emitting a `__build()` body).
    fn keep_index(&mut self) -> usize {
        let accum = self
            .keep
            .as_mut()
            .expect("keep_index called outside keep-node mode");
        let k = accum.next;
        accum.next += 1;
        k
    }

    /// Phase 11.10 slice 3 — allocate the next dynamic-region index.
    fn keep_region_index(&mut self) -> usize {
        let accum = self
            .keep
            .as_mut()
            .expect("keep_region_index called outside keep-node mode");
        let r = accum.region_next;
        accum.region_next += 1;
        r
    }

    fn fresh(&mut self, prefix: &str) -> String {
        let id = self.var_counter;
        self.var_counter += 1;
        format!("__{}{}", prefix, id)
    }

    /// Return the slot index for the current STATIC `<Child />` site
    /// and advance the counter.
    fn next_static_site(&mut self) -> usize {
        let idx = self.static_site_counter;
        self.static_site_counter += 1;
        idx
    }

    /// Return the map index for the current DYNAMIC `<Child />` site
    /// (inside a `{#for}`) and advance the counter.
    fn next_dyn_site(&mut self) -> usize {
        let idx = self.dyn_site_counter;
        self.dyn_site_counter += 1;
        idx
    }
}

fn emit_template_node(
    node: &ExpandedTemplateNode,
    parent_var: &str,
    ctx: &mut RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    // Phase 11.12 — the hydration adopt walk reuses this dispatch but routes
    // each node to its adopt emitter (acquire the existing node from the cursor
    // instead of creating it). Slice 2 adopts `{#if}`/`{#for}` regions (keep-node
    // path). Slice 4 adopts composition — `<slot>` and `<Child />` — for the
    // naive (opt-in) hydration path.
    if ctx.hydrate {
        return match node {
            ExpandedTemplateNode::Text(text) => {
                emit_text_adopt(text, parent_var, out);
                Ok(())
            }
            ExpandedTemplateNode::Interpolation { .. } => {
                emit_interpolation_adopt(parent_var, ctx, out);
                Ok(())
            }
            ExpandedTemplateNode::Element {
                tag,
                attrs,
                children,
                ..
            } => emit_element_adopt(tag, attrs, children, parent_var, ctx, out),
            ExpandedTemplateNode::If { .. } | ExpandedTemplateNode::For { .. } => {
                if ctx.keep.is_some() {
                    // Keep-node path (slice 2): adopt the region's server anchors.
                    emit_keep_region_adopt(ctx, parent_var, out)
                } else {
                    // Naive hydration (slice 4) does not model `{#if}`/`{#for}`
                    // adoption yet — a naive region re-renders inline and the
                    // adopt walk has no anchors to line up on. The composition
                    // demo has no regions; a naive component that both composes
                    // AND has a region is out of scope for this slice.
                    // PRIORITY TODO (docs/deudas-post-5b.md): naive-region adopt
                    // (evaluate the restored condition, walk the taken branch's
                    // nodes against the cursor).
                    Err(EmitError {
                        message: "hydration of a `{#if}`/`{#for}` region inside a \
                             NAIVE (composition) component is not supported in this \
                             slice — only keep-node components adopt regions. Move \
                             the region into a keep-node child, or drop `hydrate` \
                             from this component."
                            .to_string(),
                        context: format!("component `{}`", ctx.component_name),
                    })
                }
            }
            ExpandedTemplateNode::Slot { name, fallback, .. } => {
                emit_slot_adopt(name.as_deref(), fallback, parent_var, ctx, out)
            }
            ExpandedTemplateNode::ChildComponent {
                name,
                props,
                key,
                events,
                slot_content,
                ..
            } => emit_child_component_adopt(
                name,
                props,
                key.as_ref(),
                events,
                slot_content,
                parent_var,
                ctx,
                out,
            ),
        };
    }
    match node {
        ExpandedTemplateNode::Text(text) => {
            emit_text(text, parent_var, ctx, out);
            Ok(())
        }
        ExpandedTemplateNode::Interpolation { expr, .. } => {
            emit_interpolation(expr, parent_var, ctx, out)
        }
        ExpandedTemplateNode::Element {
            tag,
            attrs,
            children,
            ..
        } => emit_element(tag, attrs, children, parent_var, ctx, out),
        ExpandedTemplateNode::If {
            cond,
            children,
            else_children,
            ..
        } => {
            // Phase 11.10 slice 3 — under keep-node, a `{#if}` becomes a
            // dynamic region rebuilt in place between anchors; otherwise the
            // naive inline `if`.
            if ctx.keep.is_some() {
                emit_keep_region(node, parent_var, ctx, out)
            } else {
                emit_if(
                    cond,
                    children,
                    else_children.as_deref(),
                    parent_var,
                    ctx,
                    out,
                )
            }
        }
        ExpandedTemplateNode::For {
            var,
            iter,
            children,
            ..
        } => {
            if ctx.keep.is_some() {
                emit_keep_region(node, parent_var, ctx, out)
            } else {
                emit_for(var, iter, children, parent_var, ctx, out)
            }
        }
        ExpandedTemplateNode::Slot { name, fallback, .. } => {
            emit_slot(name.as_deref(), fallback, parent_var, ctx, out)
        }
        ExpandedTemplateNode::ChildComponent {
            name,
            props,
            key,
            events,
            slot_content,
            ..
        } => emit_child_component(
            name,
            props,
            key.as_ref(),
            events,
            slot_content,
            parent_var,
            ctx,
            out,
        ),
    }
}

/// Emit a `<slot />` or `<slot name="X" />`. If the parent filled the
/// slot (the backing `__slot` / `__slot_<name>` field is `Some`), invoke
/// the parent-provided renderer with the current element as the target;
/// otherwise render the slot's own fallback content.
fn emit_slot(
    name: Option<&str>,
    fallback: &[ExpandedTemplateNode],
    parent_var: &str,
    ctx: &mut RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    let field = slot_field_name(name);
    writeln!(
        out,
        "        if let Some(__cb) = self.{}.borrow().as_ref() {{",
        field
    )
    .unwrap();
    writeln!(
        out,
        "            let __slot_target: &web_sys::Node = {}.as_ref();",
        parent_var
    )
    .unwrap();
    writeln!(out, "            __cb(__slot_target);").unwrap();
    writeln!(out, "        }} else {{").unwrap();
    for node in fallback {
        emit_template_node(node, parent_var, ctx, out)?;
    }
    writeln!(out, "        }}").unwrap();
    Ok(())
}

fn emit_text(text: &str, parent_var: &str, ctx: &mut RenderCtx, out: &mut String) {
    // Skip pure whitespace-only text nodes to keep the emitted DOM
    // small and readable. HTML collapses whitespace anyway.
    if text.trim().is_empty() {
        return;
    }
    let var = ctx.fresh("t");
    writeln!(
        out,
        "        let {} = document.create_text_node({});",
        var,
        rust_string_literal(text)
    )
    .unwrap();
    writeln!(
        out,
        "        {}.append_child(&{}).unwrap();",
        parent_var, var
    )
    .unwrap();
}

/// CW.9 (1b) — if `expr` is a call to the raw-HTML framework helper
/// `raw_html(x)` or `html(x)` (single arg), return the inner markup-producing
/// expression. Used by [`emit_interpolation`] to route it to `set_inner_html`
/// instead of an escaping text node. Only the single-string form is a sink;
/// `h_join`/`h_when`/`h_either` (List<Html> folding) have no wasm form and
/// keep rejecting in `lower_call`.
fn raw_html_sink_arg(expr: &Expr) -> Option<&Expr> {
    if let Expr::Call { callee, args, .. } = expr {
        if let Expr::Ident(name, _) = callee.as_ref() {
            if (name == "raw_html" || name == "html") && args.len() == 1 {
                return Some(&args[0]);
            }
        }
    }
    None
}

fn emit_interpolation(
    expr: &Expr,
    parent_var: &str,
    ctx: &mut RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    // CW.9 (1b) — raw-HTML sink. `{raw_html(x)}` / `{html(x)}` as an element
    // child injects UNescaped markup via `set_inner_html` on the parent
    // element, instead of a text node (which escapes intrinsically). This is
    // what unblocks the SSR companion components whose helpers build markup
    // strings (`icon`, chart/grid helpers) on the client-WASM target. The
    // parent element OWNS the injected markup: `set_inner_html` replaces all
    // its children, so the raw-HTML interpolation must be the SOLE content of
    // its parent (mirrors React's `dangerouslySetInnerHTML`). The
    // List<Html>-folding helpers (`h_join`/`h_when`/`h_either`) still reject
    // in `lower_call` — they have no single-string form. Not supported inside
    // keep-node / hydratable components yet (they patch individual text nodes;
    // a raw-HTML node has no in-place patch path).
    if let Some(inner) = raw_html_sink_arg(expr) {
        if ctx.keep.is_some() || ctx.hydrate {
            return Err(EmitError {
                message: "raw-HTML interpolation (`{raw_html(...)}` / `{html(...)}`) is not \
                          yet supported inside a keep-node / hydratable component on the \
                          client-WASM target — keep this component on naive re-render or \
                          the SSR target"
                    .to_string(),
                context: "expression".to_string(),
            });
        }
        let inner_rust = lower_expr(inner, ctx.state_names, &ctx.locals)?;
        writeln!(
            out,
            "        {}.set_inner_html(&format!(\"{{}}\", {}));",
            parent_var, inner_rust
        )
        .unwrap();
        return Ok(());
    }
    let var_interp = ctx.fresh("interp");
    let var_node = ctx.fresh("t");
    let expr_rust = lower_expr(expr, ctx.state_names, &ctx.locals)?;
    writeln!(
        out,
        "        let {} = format!(\"{{}}\", {});",
        var_interp, expr_rust
    )
    .unwrap();
    writeln!(
        out,
        "        let {} = document.create_text_node(&{});",
        var_node, var_interp
    )
    .unwrap();
    writeln!(
        out,
        "        {}.append_child(&{}).unwrap();",
        parent_var, var_node
    )
    .unwrap();
    // Phase 11.10 — keep-node reconciliation: stash the text node so a
    // later state change patches its content in place (`set_data`) instead
    // of rebuilding the subtree.
    if ctx.keep.is_some() {
        let k = ctx.keep_index();
        let field = format!("__ktext_{}", k);
        writeln!(
            out,
            "        *self.{}.borrow_mut() = Some({}.clone());",
            field, var_node
        )
        .unwrap();
        let accum = ctx.keep.as_mut().unwrap();
        accum
            .fields
            .push((field.clone(), "web_sys::Text".to_string()));
        accum.patch.push(format!(
            "        if let Some(__n) = self.{}.borrow().as_ref() {{ __n.set_data(&format!(\"{{}}\", {})); }}",
            field, expr_rust
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Phase 11.10 slice 3 — dynamic regions ({#if}/{#for} under keep-node)
// ---------------------------------------------------------------------------

/// Emit a `{#if}`/`{#for}` as a keep-node **dynamic region** (Phase 11.10
/// slice 3). The region is bounded by two comment anchors; its content is
/// (re)built by `__mount_region_<n>`, which runs the naive `{#if}`/`{#for}`
/// emit into a `DocumentFragment` and inserts it before the end anchor.
/// `__patch_region_<n>` clears the nodes between the anchors and re-mounts.
///
/// Because the region rebuilds wholesale — but only the region — a live
/// sibling `<input>` outside it is never touched, so its caret survives a
/// state change that also updates the region (search/filter as you type).
/// Nodes inside the region ARE re-created on each change (no per-item state
/// or caret is preserved inside a region — that is a later, finer slice).
fn emit_keep_region(
    node: &ExpandedTemplateNode,
    parent_var: &str,
    ctx: &mut RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    let r = ctx.keep_region_index();
    let start_field = format!("__astart_{}", r);
    let end_field = format!("__aend_{}", r);
    let rs = ctx.fresh("rs");
    let re = ctx.fresh("re");

    // Build: two comment anchors bounding the region, stashed as handles,
    // then mount the region's content between them.
    writeln!(
        out,
        "        let {}: web_sys::Node = document.create_comment(\"\").into();",
        rs
    )
    .unwrap();
    writeln!(
        out,
        "        {}.append_child(&{}).unwrap();",
        parent_var, rs
    )
    .unwrap();
    writeln!(
        out,
        "        *self.{}.borrow_mut() = Some({}.clone());",
        start_field, rs
    )
    .unwrap();
    writeln!(
        out,
        "        let {}: web_sys::Node = document.create_comment(\"\").into();",
        re
    )
    .unwrap();
    writeln!(
        out,
        "        {}.append_child(&{}).unwrap();",
        parent_var, re
    )
    .unwrap();
    writeln!(
        out,
        "        *self.{}.borrow_mut() = Some({}.clone());",
        end_field, re
    )
    .unwrap();
    writeln!(out, "        self.__mount_region_{}();", r).unwrap();

    {
        let accum = ctx.keep.as_mut().unwrap();
        accum
            .fields
            .push((start_field.clone(), "web_sys::Node".to_string()));
        accum
            .fields
            .push((end_field.clone(), "web_sys::Node".to_string()));
        accum
            .patch
            .push(format!("        self.__patch_region_{}();", r));
    }

    // The region's mount + patch methods, emitted after the build/patch fns.
    let mut m = String::new();
    writeln!(m, "    fn __mount_region_{}(self: &Rc<Self>) {{", r).unwrap();
    writeln!(
        m,
        "        let document = web_sys::window().unwrap().document().unwrap();"
    )
    .unwrap();
    writeln!(
        m,
        "        let __frag = document.create_document_fragment();"
    )
    .unwrap();
    // Naive `{#if}`/`{#for}` emit into the fragment (keep = None → the region
    // content is rebuilt each time; a fresh ctx gives it its own var names).
    let mut sub = RenderCtx::new(
        ctx.component_name,
        ctx.state_names,
        ctx.state_fields,
        ctx.file,
        ctx.nominals,
        ctx.events,
        ctx.this_bubbled,
    );
    emit_template_node(node, "__frag", &mut sub, &mut m)?;
    writeln!(
        m,
        "        if let Some(__e) = self.{}.borrow().as_ref() {{",
        end_field
    )
    .unwrap();
    writeln!(m, "            if let Some(__p) = __e.parent_node() {{").unwrap();
    writeln!(
        m,
        "                let _ = __p.insert_before(&__frag, Some(__e));"
    )
    .unwrap();
    writeln!(m, "            }}").unwrap();
    writeln!(m, "        }}").unwrap();
    writeln!(m, "    }}\n").unwrap();

    writeln!(m, "    fn __patch_region_{}(self: &Rc<Self>) {{", r).unwrap();
    writeln!(
        m,
        "        if let (Some(__s), Some(__e)) = (self.{}.borrow().clone(), self.{}.borrow().clone()) {{",
        start_field, end_field
    )
    .unwrap();
    writeln!(m, "            while let Some(__n) = __s.next_sibling() {{").unwrap();
    writeln!(
        m,
        "                if __n.is_same_node(Some(&__e)) {{ break; }}"
    )
    .unwrap();
    writeln!(
        m,
        "                if let Some(__p) = __n.parent_node() {{ let _ = __p.remove_child(&__n); }}"
    )
    .unwrap();
    writeln!(m, "            }}").unwrap();
    writeln!(m, "        }}").unwrap();
    writeln!(m, "        self.__mount_region_{}();", r).unwrap();
    writeln!(m, "    }}\n").unwrap();

    ctx.keep.as_mut().unwrap().region_methods.push(m);
    Ok(())
}

/// Phase 11.12 slice 2 — hydration adopt of a `{#if}`/`{#for}` **dynamic
/// region**. The build walk (`emit_keep_region`) creates two comment anchors
/// and mounts the region content between them; the SERVER paints the same
/// shape with *tagged* anchors (`<!--fr-->` … `<!--/fr-->`) around the
/// already-rendered content. This adopt acquires those two anchors into the
/// same `__astart_<r>` / `__aend_<r>` handles the build walk declared, and
/// does NOT re-mount — the content is already in the DOM. A later state change
/// patches it via the `__patch_region_<r>` the build walk emitted (which
/// clears between the anchors and rebuilds).
///
/// The anchors are matched by comment DATA so the scan steps over the region's
/// server-painted content — including any interpolation markers (`<!--fi-->`)
/// inside it — landing on the region's own `<!--fr-->` / `<!--/fr-->`. Uses the
/// same `keep_region_index()` the build walk did, so (visiting regions in the
/// same DFS order) the field names line up.
fn emit_keep_region_adopt(
    ctx: &mut RenderCtx,
    cursor_var: &str,
    out: &mut String,
) -> EmitResult<()> {
    let r = ctx.keep_region_index();
    let start_field = format!("__astart_{}", r);
    let end_field = format!("__aend_{}", r);
    writeln!(
        out,
        "        if let Some(__rs) = __flv_next_comment(&mut {}, \"fr\") {{ *self.{}.borrow_mut() = Some(__rs); }}",
        cursor_var, start_field
    )
    .unwrap();
    writeln!(
        out,
        "        if let Some(__re) = __flv_next_comment(&mut {}, \"/fr\") {{ *self.{}.borrow_mut() = Some(__re); }}",
        cursor_var, end_field
    )
    .unwrap();
    Ok(())
}

// ---------------------------------------------------------------------------
// Control-flow directives — `{#if}` / `{#for}` (Phase 11.7.b)
// ---------------------------------------------------------------------------

/// `{#if cond}...{/if}` / `{#if cond}...{#else}...{/if}`.
///
/// Naive re-render model: the condition is evaluated at render time
/// and the matching branch's children are emitted into `parent_var`.
/// `cond` must lower to a Rust `bool` via [`lower_cond_expr`].
fn emit_if(
    cond: &Expr,
    children: &[ExpandedTemplateNode],
    else_children: Option<&[ExpandedTemplateNode]>,
    parent_var: &str,
    ctx: &mut RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    let cond_rust = lower_cond_expr(cond, ctx.state_names, &ctx.locals).map_err(|mut e| {
        e.context = format!("template of component `{}`", ctx.component_name);
        e
    })?;
    writeln!(out, "        if {} {{", cond_rust).unwrap();
    for child in children {
        emit_template_node(child, parent_var, ctx, out)?;
    }
    if let Some(else_kids) = else_children {
        writeln!(out, "        }} else {{").unwrap();
        for child in else_kids {
            emit_template_node(child, parent_var, ctx, out)?;
        }
    }
    writeln!(out, "        }}").unwrap();
    Ok(())
}

/// `{#for x in <iter>}...{/for}`.
///
/// MVP (Phase 11.7.b): the iterable must be a bare state field of
/// `List<primitive>` type. Snapshots the state `Vec` (`.clone()`) and
/// iterates by value so the loop body can mutate other state freely,
/// binding `x` as a Rust loop variable in scope for the children.
///
/// Deferred with clear pointers: nominal-element lists (e.g.
/// `List<Card>`, needed by the kanban) wait on nominal-type support in
/// the WASM target (Phase 11.7 R3 prereq); non-ident iterables (method
/// calls, imported fns) wait on richer expr lowering or the SSR target.
fn emit_for(
    var: &str,
    iter: &Expr,
    children: &[ExpandedTemplateNode],
    parent_var: &str,
    ctx: &mut RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    let ctx_label = format!("template of component `{}`", ctx.component_name);

    // Decide where the loop draws its elements from.
    //
    // - A bare state-field ident (`{#for c in cards}`) keeps the
    //   snapshot-and-`.iter().cloned()` path with its element-type gate
    //   (nice errors + byte-identical output for the pre-R3.5 examples).
    // - Any OTHER expression (`{#for c in cards_in(cards, "todo")}`,
    //   `{#for n in nums.filter(...)}`) is a general iterable (Phase 11.7
    //   R3.5a.1): lower it to an owned `Vec`-producing expression and
    //   iterate with `.into_iter()`. The loop var's element type is left
    //   to rustc inference — field access on it (`{c.title}`) validates
    //   against the produced `Vec`'s element type.
    enum ForSrc {
        StateField(String),
        General(String),
    }
    let src = match iter {
        Expr::Ident(n, _) if ctx.state_fields.iter().any(|f| &f.name == n) => {
            let field = ctx
                .state_fields
                .iter()
                .find(|f| &f.name == n)
                .expect("state field presence just checked");
            let elem_ty = match &field.type_expr {
                TypeExpr::Generic { name, args } if name == "List" && args.len() == 1 => &args[0],
                _ => {
                    return Err(EmitError {
                        message: format!(
                            "`{{#for x in {n}}}` requires `{n}` to be a `List<...>` \
                             state field"
                        ),
                        context: ctx_label,
                    })
                }
            };
            // Phase 11.7 R3 — the element type may be a primitive OR an
            // imported nominal (e.g. `List<Card>`). A nominal loop var
            // binds as the emitted struct (owned, from `.iter().cloned()`),
            // so its fields are reachable via `{c.title}` field access.
            // Reject only types the emitter can't lower.
            let elem_is_nominal = matches!(elem_ty, TypeExpr::Named(t) if ctx.nominals.contains(t));
            if !is_wasm_prop_simple_target(elem_ty) && !elem_is_nominal {
                return Err(EmitError {
                    message: format!(
                        "`{{#for}}` over `{n}`: the client-WASM target iterates \
                         `List<Int|Float|Str|Bool>` and `List<Nominal>` where the \
                         nominal is imported from a sibling `.fitz` (Phase 11.7 R3). \
                         This element type is neither — use a primitive-element list, \
                         import the nominal, or the SSR target."
                    ),
                    context: ctx_label,
                });
            }
            ForSrc::StateField(n.clone())
        }
        _ => {
            let iter_rust = lower_expr(iter, ctx.state_names, &ctx.locals).map_err(|mut e| {
                e.context = ctx_label.clone();
                e
            })?;
            ForSrc::General(iter_rust)
        }
    };

    // Phase 11.7.b R2b — keyed `<Child />` composition inside `{#for}`.
    // Pre-scan the body for the DYNAMIC child sites it contains (the
    // same descent the render walk does, NOT into a nested `{#for}`).
    // Their `__child_map_<n>` indices are the contiguous range starting
    // at the current dynamic counter. For each, declare a per-render
    // `__seen_<n>` set BEFORE the loop; the child mount inserts its key
    // into it; after the loop, `retain` evicts every child whose key
    // vanished this render (reconciliation). The instance for a
    // surviving key is reused from the map, so its local state persists.
    let dyn_base = ctx.dyn_site_counter;
    let dyn_count = count_dynamic_child_sites(children);
    for i in dyn_base..dyn_base + dyn_count {
        writeln!(
            out,
            "        let mut __seen_{} = std::collections::HashSet::<String>::new();",
            i
        )
        .unwrap();
    }

    let snap = ctx.fresh("for");
    match &src {
        ForSrc::StateField(field_name) => {
            writeln!(
                out,
                "        let {} = (*self.{}.borrow()).clone();",
                snap, field_name
            )
            .unwrap();
            writeln!(out, "        for {} in {}.iter().cloned() {{", var, snap).unwrap();
        }
        ForSrc::General(iter_rust) => {
            writeln!(out, "        let {} = {};", snap, iter_rust).unwrap();
            writeln!(out, "        for {} in {}.into_iter() {{", var, snap).unwrap();
        }
    }
    ctx.locals.push(var.to_string());
    let prev_in_for = ctx.in_for;
    ctx.in_for = true;
    let mut result = Ok(());
    for child in children {
        if let Err(e) = emit_template_node(child, parent_var, ctx, out) {
            result = Err(e);
            break;
        }
    }
    ctx.in_for = prev_in_for;
    ctx.locals.pop();
    result?;
    writeln!(out, "        }}").unwrap();

    // Reconciliation sweep — evict any keyed child not touched this
    // render so vanished list items release their cached instance.
    for i in dyn_base..dyn_base + dyn_count {
        writeln!(
            out,
            "        self.__child_map_{}.borrow_mut().retain(|__k, _| __seen_{}.contains(__k));",
            i, i
        )
        .unwrap();
    }
    Ok(())
}

/// Count the DYNAMIC `<Child />` sites directly inside a `{#for}`
/// body — the ones that share this loop's keyed instance caches.
/// Descends into `Element` and `{#if}` children (the render walk
/// reaches child sites through both) but NOT into a nested `{#for}`
/// (that inner loop owns its own contiguous range of dynamic
/// indices, pre-scanned when its own `emit_for` runs). Kept in lock-
/// step with the DFS descent of `collect_child_site_types` /
/// `emit_template_node` so the index range reserved here matches the
/// `__child_map_<n>` fields the children actually read.
fn count_dynamic_child_sites(children: &[ExpandedTemplateNode]) -> usize {
    fn count_node(node: &ExpandedTemplateNode) -> usize {
        match node {
            ExpandedTemplateNode::ChildComponent { .. } => 1,
            ExpandedTemplateNode::Element { children, .. } => children.iter().map(count_node).sum(),
            ExpandedTemplateNode::If {
                children,
                else_children,
                ..
            } => {
                let then: usize = children.iter().map(count_node).sum();
                let els: usize = else_children
                    .as_deref()
                    .map(|kids| kids.iter().map(count_node).sum())
                    .unwrap_or(0);
                then + els
            }
            _ => 0,
        }
    }
    children.iter().map(count_node).sum()
}

/// Lower a `{#if}` condition to a Rust `bool` expression. Supports
/// bool literals, bool state fields / loop vars used directly,
/// comparisons (`==`/`!=`/`<`/`<=`/`>`/`>=`, on **numbers or strings** —
/// both sides lower via `lower_expr`, so `variant == "error"` compiles as
/// `String == String`), `payload.has(...)`, and `&&` / `||` / `!` over
/// those. Other condition shapes (a bare method call, an index, …) defer
/// to the SSR target.
fn lower_cond_expr(expr: &Expr, state_names: &[String], locals: &[String]) -> EmitResult<String> {
    match expr {
        Expr::Bool(b, _) => Ok(b.to_string()),
        // A bool state field / loop var used directly as a condition.
        // The checker guarantees it's Bool-typed.
        Expr::Ident(..) => lower_expr(expr, state_names, locals),
        // CW.9 — a bool FIELD access as a condition (`{#if o.on}` where `o`
        // is a `{#for}` loop var of nominal type and `.on` is Bool). Common
        // in list-of-options components (`Select` / `RadioGroup`). The checker
        // guarantees Bool; `lower_expr` emits `o.on.clone()` (bool is `Copy`,
        // so the clone is a no-op).
        Expr::Field { .. } => lower_expr(expr, state_names, locals),
        // Phase 11.7 R3.5b.1 — `payload.has("key")` as a guard condition
        // (`if (payload.has("title")) { ... }`). Delegates to `lower_expr`,
        // which routes the call to the `payload.has` special case
        // (`payload.contains_key(...)`, a `bool`).
        Expr::Call { callee, .. }
            if matches!(
                callee.as_ref(),
                Expr::Field { object, field, .. }
                    if field == "has"
                        && matches!(object.as_ref(), Expr::Ident(n, _) if n == "payload")
            ) =>
        {
            lower_expr(expr, state_names, locals)
        }
        Expr::UnaryOp {
            op: crate::ast::UnaryOpKind::Not,
            operand,
            ..
        } => {
            let inner = lower_cond_expr(operand, state_names, locals)?;
            Ok(format!("(!{})", inner))
        }
        Expr::BinOp {
            op, left, right, ..
        } => match op {
            BinOpKind::And => {
                let l = lower_cond_expr(left, state_names, locals)?;
                let r = lower_cond_expr(right, state_names, locals)?;
                Ok(format!("({} && {})", l, r))
            }
            BinOpKind::Or => {
                let l = lower_cond_expr(left, state_names, locals)?;
                let r = lower_cond_expr(right, state_names, locals)?;
                Ok(format!("({} || {})", l, r))
            }
            BinOpKind::Eq
            | BinOpKind::NotEq
            | BinOpKind::Lt
            | BinOpKind::LtEq
            | BinOpKind::Gt
            | BinOpKind::GtEq => {
                let cmp = match op {
                    BinOpKind::Eq => "==",
                    BinOpKind::NotEq => "!=",
                    BinOpKind::Lt => "<",
                    BinOpKind::LtEq => "<=",
                    BinOpKind::Gt => ">",
                    BinOpKind::GtEq => ">=",
                    _ => unreachable!(),
                };
                let l = lower_expr(left, state_names, locals)?;
                let r = lower_expr(right, state_names, locals)?;
                Ok(format!("({} {} {})", l, cmp, r))
            }
            _ => Err(EmitError {
                message: "`{#if}` condition — only comparisons (==/!=/</<=/>/>=) and \
                          &&/||/! are booleans on the client-WASM target (Phase 11.7.b)"
                    .to_string(),
                context: "if condition".to_string(),
            }),
        },
        _ => Err(EmitError {
            message: "`{#if}` condition — supported: a bool state field / loop var, a \
                      comparison (==/!=/</<=/>/>=, on numbers or strings), \
                      `payload.has(...)`, and &&/||/!. This condition shape isn't one of \
                      those — compute a bool state field in an event body, or use the \
                      SSR target"
                .to_string(),
            context: "if condition".to_string(),
        }),
    }
}

fn emit_element(
    tag: &str,
    attrs: &[ExpandedAttr],
    children: &[ExpandedTemplateNode],
    parent_var: &str,
    ctx: &mut RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    let var = ctx.fresh("el");
    writeln!(
        out,
        "        let {} = document.create_element({}).unwrap();",
        var,
        rust_string_literal(tag)
    )
    .unwrap();

    // Phase 11.7 R3.5b.1 — collect the `data-flv-*` event wiring while
    // emitting the real DOM attributes. `data-flv-click` / `data-flv-submit`
    // are directives (not set as attrs); `data-flv-value-*` ARE set (the
    // click listener reads them back to build the payload).
    let mut data_flv_click: Option<String> = None;
    let mut data_flv_submit: Option<String> = None;
    let mut data_flv_file: Option<String> = None;
    let mut value_keys: Vec<String> = Vec::new();

    for attr in attrs {
        match attr {
            ExpandedAttr::Static { name, value, .. } => {
                if name == "data-flv-click" {
                    data_flv_click = Some(value.clone());
                } else if name == "data-flv-submit" {
                    data_flv_submit = Some(value.clone());
                } else if name == "data-flv-file" {
                    data_flv_file = Some(value.clone());
                } else {
                    if let Some(key) = name.strip_prefix("data-flv-value-") {
                        value_keys.push(key.to_string());
                    }
                    emit_static_attr(name, value, &var, out);
                }
            }
            ExpandedAttr::Event {
                event_name,
                handler_name,
                ..
            } => {
                emit_event_attr(event_name, handler_name, &var, ctx, out)?;
            }
            // Mixed attribute interpolation (`style="width: {pct}%"`,
            // `class="toast toast-{kind}"`). Build a `format!` that interleaves
            // the literal segments (escaped for the format string) with a `{}`
            // for each interpolated expr, then `set_attribute`. Full-value
            // interpolation (`attr="{expr}"`) is the `Interpolation` arm below.
            ExpandedAttr::MixedInterpolation { name, segments, .. } => {
                if let Some(key) = name.strip_prefix("data-flv-value-") {
                    value_keys.push(key.to_string());
                }
                let mut fmt = String::new();
                let mut args: Vec<String> = Vec::new();
                for seg in segments {
                    match seg {
                        super::expand::AttrValueSegment::Literal(lit) => {
                            fmt.push_str(&escape_for_rust_format(lit));
                        }
                        super::expand::AttrValueSegment::Expr(expr) => {
                            fmt.push_str("{}");
                            let a =
                                lower_expr(expr, ctx.state_names, &ctx.locals).map_err(|mut e| {
                                    e.context = format!(
                                        "mixed-interpolated attribute `{}` of element `<{}>` in component `{}`",
                                        name, tag, ctx.component_name
                                    );
                                    e
                                })?;
                            args.push(a);
                        }
                    }
                }
                let args_str = if args.is_empty() {
                    String::new()
                } else {
                    format!(", {}", args.join(", "))
                };
                writeln!(
                    out,
                    "        {}.set_attribute({}, &format!(\"{}\"{})).unwrap();",
                    var,
                    rust_string_literal(name),
                    fmt,
                    args_str
                )
                .unwrap();
                // Phase 11.10 — keep-node: stash the element so a later
                // state change re-sets this mixed attribute in place.
                if ctx.keep.is_some() {
                    let k = ctx.keep_index();
                    let field = format!("__kattr_{}", k);
                    writeln!(
                        out,
                        "        *self.{}.borrow_mut() = Some({}.clone());",
                        field, var
                    )
                    .unwrap();
                    let accum = ctx.keep.as_mut().unwrap();
                    accum
                        .fields
                        .push((field.clone(), "web_sys::Element".to_string()));
                    accum.patch.push(format!(
                        "        if let Some(__el) = self.{}.borrow().as_ref() {{ let _ = __el.set_attribute({}, &format!(\"{}\"{})); }}",
                        field,
                        rust_string_literal(name),
                        fmt,
                        args_str
                    ));
                }
            }
            // Phase 11.7 R3.5b.1 — an interpolated attribute
            // (`data-flv-value-card_id="{c.id}"`, `class="{cls}"`).
            ExpandedAttr::Interpolation { name, expr, .. } => {
                if let Some(key) = name.strip_prefix("data-flv-value-") {
                    value_keys.push(key.to_string());
                }
                let value_rust =
                    lower_expr(expr, ctx.state_names, &ctx.locals).map_err(|mut e| {
                        e.context = format!(
                            "interpolated attribute `{}` of element `<{}>` in component `{}`",
                            name, tag, ctx.component_name
                        );
                        e
                    })?;
                writeln!(
                    out,
                    "        {}.set_attribute({}, &format!(\"{{}}\", {})).unwrap();",
                    var,
                    rust_string_literal(name),
                    value_rust
                )
                .unwrap();
                // Phase 11.10 — keep-node: stash the element so a later
                // state change re-sets this interpolated attribute in place.
                // For a live `<input value="{name}">` this re-sets the value
                // content attribute without touching the caret (the current
                // value property, dirtied by the user's typing, is untouched).
                if ctx.keep.is_some() {
                    let k = ctx.keep_index();
                    let field = format!("__kattr_{}", k);
                    writeln!(
                        out,
                        "        *self.{}.borrow_mut() = Some({}.clone());",
                        field, var
                    )
                    .unwrap();
                    let accum = ctx.keep.as_mut().unwrap();
                    accum
                        .fields
                        .push((field.clone(), "web_sys::Element".to_string()));
                    accum.patch.push(format!(
                        "        if let Some(__el) = self.{}.borrow().as_ref() {{ let _ = __el.set_attribute({}, &format!(\"{{}}\", {})); }}",
                        field,
                        rust_string_literal(name),
                        value_rust
                    ));
                }
            }
        }
    }

    if let Some(handler) = data_flv_submit {
        let fields = collect_form_fields(children);
        emit_data_flv_submit(&handler, &fields, &var, ctx, out)?;
    }
    if let Some(handler) = data_flv_click {
        emit_data_flv_click(&handler, &value_keys, &var, ctx, out)?;
    }
    if let Some(handler) = data_flv_file {
        emit_data_flv_file(&handler, &var, ctx, out)?;
    }

    for child in children {
        emit_template_node(child, &var, ctx, out)?;
    }

    writeln!(
        out,
        "        {}.append_child(&{}).unwrap();",
        parent_var, var
    )
    .unwrap();
    Ok(())
}

// ---------------------------------------------------------------------------
// Phase 11.12 — hydration adopt walk
// ---------------------------------------------------------------------------
//
// The adopt walk mirrors the keep-node `__build` DFS exactly, but instead of
// `create_element`/`create_text_node` + `append_child` it ACQUIRES each node
// from the server-painted DOM via a sibling cursor and stashes it into the
// same `__ktext_<n>` / `__kattr_<n>` handle field. Because the walk is
// structurally identical, `keep_index()` yields the same indices as the build
// walk, so the adopted handles line up with the struct fields the build path
// declared. Event listeners still attach; static / interpolated attributes are
// NOT re-set (the server already rendered them). `cursor_var` names an
// `Option<web_sys::Node>` over the parent's child nodes.

fn emit_text_adopt(text: &str, cursor_var: &str, out: &mut String) {
    // Whitespace-only text is skipped by the build walk (never created), so
    // the server did not paint a node for it either — nothing to advance past.
    if text.trim().is_empty() {
        return;
    }
    // Static text occupies one server-painted text node; consume it so the
    // cursor stays aligned with the build DFS order. Its content is static —
    // no handle to keep.
    writeln!(out, "        let _ = __flv_next_text(&mut {});", cursor_var).unwrap();
}

fn emit_interpolation_adopt(cursor_var: &str, ctx: &mut RenderCtx, out: &mut String) {
    // Phase 11.12 slice 4 — a NAIVE component (composition, `ctx.keep` is
    // `None`) has no `__ktext_<n>` handle: it re-renders wholesale on the next
    // state change, so there is nothing to patch in place. Just consume the
    // server-painted text node to keep the cursor aligned with the build DFS.
    if ctx.keep.is_none() {
        writeln!(out, "        let _ = __flv_next_text(&mut {});", cursor_var).unwrap();
        return;
    }
    // Keep-node path (slices 1–3): same keep index the build walk allocated for
    // this interpolation (the adopt walk visits dynamic points in the same DFS
    // order). Adopt the existing text node into the handle; the server already
    // rendered its value, so no `set_data` — a later state change patches it via
    // `__patch`.
    let k = ctx.keep_index();
    let field = format!("__ktext_{}", k);
    writeln!(
        out,
        "        if let Some(__hn) = __flv_next_text(&mut {}) {{ *self.{}.borrow_mut() = Some(__hn); }}",
        cursor_var, field
    )
    .unwrap();
}

fn emit_element_adopt(
    tag: &str,
    attrs: &[ExpandedAttr],
    children: &[ExpandedTemplateNode],
    cursor_var: &str,
    ctx: &mut RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    let _ = tag;
    let el = ctx.fresh("hel");
    let inner_cursor = ctx.fresh("hcur");
    writeln!(
        out,
        "        if let Some({}) = __flv_next_element(&mut {}) {{",
        el, cursor_var
    )
    .unwrap();

    // Collect the `data-flv-*` directives + value keys exactly as the build
    // walk does, but skip re-setting static / interpolated attributes (the
    // server painted them). Event listeners attach; interpolated / mixed
    // attributes stash their keep-node handle from the adopted element.
    let mut data_flv_click: Option<String> = None;
    let mut data_flv_submit: Option<String> = None;
    let mut data_flv_file: Option<String> = None;
    let mut value_keys: Vec<String> = Vec::new();

    for attr in attrs {
        match attr {
            ExpandedAttr::Static { name, value, .. } => {
                if name == "data-flv-click" {
                    data_flv_click = Some(value.clone());
                } else if name == "data-flv-submit" {
                    data_flv_submit = Some(value.clone());
                } else if name == "data-flv-file" {
                    data_flv_file = Some(value.clone());
                } else if let Some(key) = name.strip_prefix("data-flv-value-") {
                    value_keys.push(key.to_string());
                }
                // Static attribute already on the server-painted element.
            }
            ExpandedAttr::Event {
                event_name,
                handler_name,
                ..
            } => {
                emit_event_attr(event_name, handler_name, &el, ctx, out)?;
            }
            ExpandedAttr::MixedInterpolation { name, .. } => {
                if let Some(key) = name.strip_prefix("data-flv-value-") {
                    value_keys.push(key.to_string());
                }
                // Naive component (slice 4, `ctx.keep` is `None`): no keep-node
                // handle to stash — the server already painted the attribute and
                // a later state change re-renders wholesale. Only the payload
                // `value_keys` collected above matters here.
                if ctx.keep.is_some() {
                    let k = ctx.keep_index();
                    writeln!(
                        out,
                        "        *self.__kattr_{}.borrow_mut() = Some({}.clone());",
                        k, el
                    )
                    .unwrap();
                }
            }
            ExpandedAttr::Interpolation { name, .. } => {
                if let Some(key) = name.strip_prefix("data-flv-value-") {
                    value_keys.push(key.to_string());
                }
                if ctx.keep.is_some() {
                    let k = ctx.keep_index();
                    writeln!(
                        out,
                        "        *self.__kattr_{}.borrow_mut() = Some({}.clone());",
                        k, el
                    )
                    .unwrap();
                }
            }
        }
    }

    if let Some(handler) = data_flv_submit {
        let fields = collect_form_fields(children);
        emit_data_flv_submit(&handler, &fields, &el, ctx, out)?;
    }
    if let Some(handler) = data_flv_click {
        emit_data_flv_click(&handler, &value_keys, &el, ctx, out)?;
    }
    if let Some(handler) = data_flv_file {
        emit_data_flv_file(&handler, &el, ctx, out)?;
    }

    // Recurse into the adopted element's own children via a fresh cursor.
    // A childless element (`<input />`) needs no cursor.
    if !children.is_empty() {
        writeln!(
            out,
            "        let mut {} = {}.first_child();",
            inner_cursor, el
        )
        .unwrap();
        for child in children {
            emit_template_node(child, &inner_cursor, ctx, out)?;
        }
    }
    writeln!(out, "        }}").unwrap();
    Ok(())
}

fn emit_static_attr(name: &str, value: &str, el_var: &str, out: &mut String) {
    writeln!(
        out,
        "        {}.set_attribute({}, {}).unwrap();",
        el_var,
        rust_string_literal(name),
        rust_string_literal(value)
    )
    .unwrap();
}

/// Phase 11.5.d — emit the Rust for a `<Child prop="v" />` node.
/// Creates a wrapper `<div>` inside the parent (so the child owns
/// a stable root), instantiates `Child::new()`, writes each
/// coerced prop into the corresponding `RefCell<T>` state field,
/// then calls `mount_into` handing the wrapper to the child.
///
/// The wrapper class is `__fitz-child-<ChildName>` so scoped CSS
/// and dev-tools inspection stay predictable. The prop values are
/// pre-coerced by `check.rs` via `check_child_components` — the
/// emitter trusts the shape and just formats each prop as a Rust
/// literal (`i64` / `f64` / `String` / `bool` / `Option<T>`).
///
/// Reflow semantics: when the parent re-renders (state change +
/// `render()` clears the root), the child is re-instantiated
/// from scratch. Child state resets on parent re-render — a
/// consequence of naive-render (§9.m D1) that the composition
/// wiring inherits. Persistent child state across parent renders
/// is a Phase 11.6+ concern that would need a component-instance
/// cache keyed by position in the render tree.
#[allow(clippy::too_many_arguments)]
fn emit_child_component(
    child_name: &str,
    props: &[super::expand::ChildComponentProp],
    key: Option<&Expr>,
    events: &[super::expand::ChildEventBinding],
    slot_content: &[ExpandedTemplateNode],
    parent_var: &str,
    ctx: &mut RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    let wrapper_var = ctx.fresh("el");
    writeln!(
        out,
        "        let {} = document.create_element(\"div\").unwrap();",
        wrapper_var
    )
    .unwrap();
    writeln!(
        out,
        "        {}.set_attribute(\"class\", \"__fitz-child-{}\").unwrap();",
        wrapper_var, child_name
    )
    .unwrap();
    writeln!(
        out,
        "        {}.append_child(&{}).unwrap();",
        parent_var, wrapper_var
    )
    .unwrap();

    let child_var = ctx.fresh("child");
    emit_child_get_or_create(&child_var, child_name, key, ctx, out)?;
    emit_child_props(&child_var, child_name, props, ctx, out)?;
    emit_child_event_bindings(&child_var, events, ctx, out)?;
    emit_child_slot_wiring(&child_var, child_name, slot_content, false, ctx, out)?;

    writeln!(
        out,
        "        let {wrapper_var}_html = {wrapper_var}.clone().dyn_into::<HtmlElement>().unwrap();"
    )
    .unwrap();
    // `render()` is `fn(...) -> ()` (no `Result`), so we cannot use
    // `?` here. `mount_into` failure implies a JS runtime error
    // (root already detached, etc.) — surface it via `unwrap()` so
    // the browser console shows the panic trace via
    // `console_error_panic_hook`.
    writeln!(
        out,
        "        {}.mount_into({wrapper_var}_html).unwrap();",
        child_var
    )
    .unwrap();
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared `<Child />` wiring — reused by the build path (`emit_child_component`)
// and the hydration adopt path (`emit_child_component_adopt`, Phase 11.12
// slice 4). Extracting these keeps the two paths in lockstep: same
// instance-cache resolution, same prop coercion, same event bindings, same
// slot routing. The only difference is the framing (create-wrapper +
// `mount_into` vs adopt-wrapper + `hydrate`) and that the adopt path also
// registers the `__hslot` cursor callback.
// ---------------------------------------------------------------------------

/// Emit `let {child_var} = { ... get-or-create ... };` — resolve the child from
/// this site's instance cache (`__child_slot_<n>` static, `__child_map_<n>`
/// dynamic inside a `{#for}`) so its state survives parent re-renders.
fn emit_child_get_or_create(
    child_var: &str,
    child_name: &str,
    key: Option<&Expr>,
    ctx: &mut RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    if ctx.in_for {
        // Phase 11.7.b R2b — DYNAMIC site inside a `{#for}`. Reconcile
        // the child through this site's keyed instance cache
        // (`__child_map_<idx>`) so the instance for a stable key — and
        // its local state — survives re-renders. The `key` attribute
        // gives the identity; `__seen_<idx>` (declared by the enclosing
        // `emit_for`) records the keys touched this render so the
        // post-loop `retain` can evict vanished ones.
        let key_expr = key.ok_or_else(|| EmitError {
            message: format!(
                "`<{child_name} />` inside a `{{#for}}` needs a `key=\"{{...}}\"` \
                 attribute so each item keeps a stable identity across re-renders \
                 (e.g. `<{child_name} key=\"{{x}}\" ... />`, where `x` is the loop \
                 variable). Without a key the client-WASM target can't reconcile \
                 the keyed instance cache."
            ),
            context: format!("template of component `{}`", ctx.component_name),
        })?;
        let key_rust = lower_expr(key_expr, ctx.state_names, &ctx.locals).map_err(|mut e| {
            e.message = format!(
                "`<{child_name} key=\"{{...}}\" />`: {}. The key must lower to a \
                 primitive (typically the loop variable) on the client-WASM target.",
                e.message
            );
            e.context = format!("template of component `{}`", ctx.component_name);
            e
        })?;
        let map_idx = ctx.next_dyn_site();
        writeln!(out, "        let __key = format!(\"{{}}\", {});", key_rust).unwrap();
        writeln!(out, "        __seen_{}.insert(__key.clone());", map_idx).unwrap();
        writeln!(out, "        let {} = {{", child_var).unwrap();
        writeln!(
            out,
            "            let mut __map = self.__child_map_{}.borrow_mut();",
            map_idx
        )
        .unwrap();
        writeln!(
            out,
            "            __map.entry(__key.clone()).or_insert_with(|| {}::new()).clone()",
            child_name
        )
        .unwrap();
        writeln!(out, "        }};").unwrap();
    } else {
        // Phase 11.7.e — STATIC site. Get-or-create the child from this
        // site's single instance-cache slot instead of `Child::new()`ing
        // it fresh every render. Reusing the cached `Rc` preserves the
        // child's state (its `RefCell` fields) across parent re-renders;
        // only the child's DOM is rebuilt (via `mount_into` → `render`).
        let slot_idx = ctx.next_static_site();
        writeln!(out, "        let {} = {{", child_var).unwrap();
        writeln!(
            out,
            "            let mut __slot = self.__child_slot_{}.borrow_mut();",
            slot_idx
        )
        .unwrap();
        writeln!(
            out,
            "            if __slot.is_none() {{ *__slot = Some({}::new()); }}",
            child_name
        )
        .unwrap();
        writeln!(out, "            __slot.as_ref().unwrap().clone()").unwrap();
        writeln!(out, "        }};").unwrap();
    }
    Ok(())
}

/// Emit the prop assignments `*{child_var}.<field>.borrow_mut() = <value>;` for
/// each `<Child prop=... />`. Static props coerce to a Rust literal; `prop={expr}`
/// props lower the interpolated expression in the PARENT scope.
fn emit_child_props(
    child_var: &str,
    child_name: &str,
    props: &[super::expand::ChildComponentProp],
    ctx: &mut RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    // Look up the child component to know each prop's declared
    // type. The checker already validated the child exists +
    // props coerce; we trust the shape and re-derive the Rust
    // literal here (both routes share
    // `check::coerce_child_prop_raw_value`, so the coerced
    // representation is guaranteed identical bit-for-bit).
    let child = ctx
        .file
        .components
        .iter()
        .find(|c| c.name == child_name)
        .ok_or_else(|| EmitError {
            message: format!(
                "internal error: child component `{child_name}` not found in the \
                 expanded view file — the checker should have caught this."
            ),
            context: format!("template of component `{}`", ctx.component_name),
        })?;
    for prop in props {
        let field = child
            .state
            .iter()
            .find(|f| f.name == prop.field_name)
            .ok_or_else(|| EmitError {
                message: format!(
                    "internal error: prop `{}` on `<{child_name} />` matches no \
                     state field — the checker should have caught this.",
                    prop.field_name
                ),
                context: format!("template of component `{}`", ctx.component_name),
            })?;
        // Phase 11.7.a — interpolated props (`prop={expr}`) into the
        // client-WASM target. Under the dirty-flag reactivity model the
        // parent re-renders (and recreates children) on every state
        // change, so the child receives a freshly-computed prop value
        // each render — reactive propagation falls out of the naive
        // re-render for free. Persistent child state (keyed instance
        // cache, so the child is NOT recreated) is the R2 work (11.7.e).
        // Static props keep the K-3 coerce-to-literal path.
        let value_rust = if prop.is_interpolated() {
            let expr = prop.expr.as_ref().ok_or_else(|| EmitError {
                message: format!(
                    "internal error: interpolated prop `{}` on `<{child_name} />` \
                     has no parsed expression — expand should have populated it.",
                    prop.field_name
                ),
                context: format!("template of component `{}`", ctx.component_name),
            })?;
            lower_child_prop_value(
                expr,
                &prop.field_name,
                &field.type_expr,
                ctx.state_names,
                &ctx.locals,
            )
            .map_err(|mut e| {
                e.context = format!("template of component `{}`", ctx.component_name);
                e
            })?
        } else {
            super::check::coerce_child_prop_raw_value(&prop.raw_value, &field.type_expr).map_err(
                |msg| EmitError {
                    message: format!(
                        "internal error: prop `{}=\"{}\"` on `<{child_name} />` — \
                 coerce_child_prop_raw_value: {msg}. The checker should have \
                 caught this.",
                        prop.field_name, prop.raw_value
                    ),
                    context: format!("template of component `{}`", ctx.component_name),
                },
            )?
        };
        writeln!(
            out,
            "        *{}.{}.borrow_mut() = {};",
            child_var, prop.field_name, value_rust
        )
        .unwrap();
    }
    Ok(())
}

/// Emit the `@event="handler"` bindings: register a callback on the child that
/// calls the PARENT's handler (with the bubbled payload when it consumes one).
fn emit_child_event_bindings(
    child_var: &str,
    events: &[super::expand::ChildEventBinding],
    ctx: &mut RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    // Phase 11.7.c — wire each `@event="handler"` binding: register a
    // callback on the child that calls the PARENT's handler when the
    // child's event fires. The parent handler is validated to exist
    // (and its payload arity resolved) via `handler_takes_payload`.
    //
    // Phase 11.7 payload bubbling — the callback carries the child's
    // event payload (`__pl: &HashMap<String, String>`). It's passed on
    // to the parent handler when the parent handler consumes a payload;
    // otherwise the closure ignores it (`|_|`) to avoid an unused-var
    // warning while keeping the uniform slot type.
    for binding in events {
        let takes_payload = handler_takes_payload(ctx, &binding.handler_name)?;
        writeln!(out, "        {{").unwrap();
        writeln!(out, "            let __parent = self.clone();").unwrap();
        if takes_payload {
            writeln!(
                out,
                "            *{}.__on_{}.borrow_mut() = Some(Box::new(move |__pl: &std::collections::HashMap<String, String>| {{",
                child_var, binding.event_name
            )
            .unwrap();
            writeln!(
                out,
                "                {}::{}(&__parent, __pl);",
                ctx.component_name, binding.handler_name
            )
            .unwrap();
        } else {
            writeln!(
                out,
                "            *{}.__on_{}.borrow_mut() = Some(Box::new(move |_: &std::collections::HashMap<String, String>| {{",
                child_var, binding.event_name
            )
            .unwrap();
            writeln!(
                out,
                "                {}::{}(&__parent);",
                ctx.component_name, binding.handler_name
            )
            .unwrap();
        }
        writeln!(out, "            }}));").unwrap();
        writeln!(out, "        }}").unwrap();
    }
    Ok(())
}

/// Emit the `<Child>content</Child>` slot wiring. Registers each content bucket
/// as a `__render_slot_<n>` renderer on the child's `__slot` / `__slot_<name>`
/// field (build path, `adopt = false`). When `adopt` is true (Phase 11.12 slice
/// 4) it ALSO registers a `__hydrate_slot_<n>` cursor callback on the child's
/// `__hslot` / `__hslot_<name>` field, so the initial `hydrate()` adopts the
/// parent-painted slot content in place. The `__render_slot_<n>` wiring is kept
/// on the adopt path too, so a later naive rebuild of the child fills its slot.
fn emit_child_slot_wiring(
    child_var: &str,
    child_name: &str,
    slot_content: &[ExpandedTemplateNode],
    adopt: bool,
    ctx: &mut RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    // Phase 11.7.d + named slots — if the parent provided slot content
    // (`<Child>content</Child>`), register a renderer on the child that
    // fills its `<slot />` (or `<slot name="X" />`) with that content
    // (rendered in PARENT scope). `<Child />` nested inside slot content
    // is rejected (the parent has no child-cache field for it).
    if slot_content.is_empty() {
        return Ok(());
    }
    if let Some(bad) = slot_content.iter().find_map(first_nested_child_name) {
        return Err(EmitError {
            message: format!(
                "`<{bad} />` nested inside `<{child_name}>...</{child_name}>` slot \
                 content is not supported on the client-WASM target yet. Slot \
                 content may contain elements, text, interpolation, `{{#if}}`/\
                 `{{#for}}`, and event handlers, but not another component."
            ),
            context: format!("template of component `{}`", ctx.component_name),
        });
    }

    // Named slots: route each top-level element carrying a
    // `slot="X"` attribute to the child's `<slot name="X" />`; every
    // other node (text, interpolation, `{#if}`/`{#for}`, or an
    // element without `slot=`) fills the default `<slot />`. When no
    // node uses `slot=`, take the byte-for-byte default-slot path
    // (11.7.d).
    let child = ctx
        .file
        .components
        .iter()
        .find(|c| c.name == child_name)
        .ok_or_else(|| EmitError {
            message: format!(
                "internal error: child component `{child_name}` not found in the \
                 expanded view file — the checker should have caught this."
            ),
            context: format!("template of component `{}`", ctx.component_name),
        })?;
    let child_slots = component_slot_set(child);
    let has_named_routing = slot_content.iter().any(|n| element_slot_attr(n).is_some());

    // Each entry: (slot name — `None` = default slot, content nodes) → one
    // `__render_slot_<n>` (+ `__hydrate_slot_<n>` when adopting) method.
    let mut wirings: Vec<(Option<String>, Vec<ExpandedTemplateNode>)> = Vec::new();
    if has_named_routing {
        let mut default_bucket: Vec<ExpandedTemplateNode> = Vec::new();
        let mut named_buckets: Vec<(String, Vec<ExpandedTemplateNode>)> = Vec::new();
        for node in slot_content {
            if let Some(slot_name) = element_slot_attr(node) {
                let slot_name = slot_name.to_string();
                if !child_slots.named.contains(&slot_name) {
                    return Err(EmitError {
                        message: format!(
                            "`slot=\"{slot_name}\"` inside `<{child_name}>...\
                             </{child_name}>` targets no `<slot name=\"{slot_name}\" />` \
                             declared in `{child_name}`. Declared named slots: {}.",
                            describe_named_slots(&child_slots)
                        ),
                        context: format!("template of component `{}`", ctx.component_name),
                    });
                }
                let stripped = strip_slot_attr(node);
                match named_buckets.iter_mut().find(|(n, _)| *n == slot_name) {
                    Some((_, bucket)) => bucket.push(stripped),
                    None => named_buckets.push((slot_name, vec![stripped])),
                }
            } else {
                default_bucket.push(node.clone());
            }
        }
        // Unslotted content only fills the default slot when it has
        // real (non-whitespace) nodes — incidental formatting between
        // slotted elements shouldn't force a `<slot />`.
        if default_bucket.iter().any(|n| !is_whitespace_text(n)) {
            if !child_slots.has_default {
                return Err(EmitError {
                    message: format!(
                        "`<{child_name}>...</{child_name}>` provides default (unslotted) \
                         content but `{child_name}` declares no default `<slot />`. Wrap \
                         the content in an element with `slot=\"<name>\"` targeting one of \
                         its named slots, or add a `<slot />` to `{child_name}`."
                    ),
                    context: format!("template of component `{}`", ctx.component_name),
                });
            }
            wirings.push((None, default_bucket));
        }
        for (slot_name, bucket) in named_buckets {
            wirings.push((Some(slot_name), bucket));
        }
    } else {
        // Byte-for-byte default-slot path (11.7.d): the whole content
        // fills the child's `<slot />`.
        if !child_slots.has_default {
            return Err(EmitError {
                message: format!(
                    "`<{child_name}>...</{child_name}>` provides slot content but \
                     `{child_name}` declares no `<slot />` to fill. Add a `<slot />` \
                     to `{child_name}`, or route the content with `slot=\"<name>\"` to \
                     one of its named slots."
                ),
                context: format!("template of component `{}`", ctx.component_name),
            });
        }
        wirings.push((None, slot_content.to_vec()));
    }

    for (slot_name, content) in wirings {
        let slot_idx = ctx.slot_methods.len();
        ctx.slot_methods.push(content);
        let field = slot_field_name(slot_name.as_deref());
        writeln!(out, "        {{").unwrap();
        writeln!(out, "            let __parent = self.clone();").unwrap();
        writeln!(
            out,
            "            *{}.{}.borrow_mut() = Some(Rc::new(move |__t: &web_sys::Node| __parent.__render_slot_{}(__t)));",
            child_var, field, slot_idx
        )
        .unwrap();
        writeln!(out, "        }}").unwrap();
        // Phase 11.12 slice 4 — also register the hydration adopt callback so
        // the initial `hydrate()` adopts the parent-painted slot content from
        // the child's cursor at its `<slot />`.
        if adopt {
            let hfield = hslot_field_name(slot_name.as_deref());
            writeln!(out, "        {{").unwrap();
            writeln!(out, "            let __parent = self.clone();").unwrap();
            writeln!(
                out,
                "            *{}.{}.borrow_mut() = Some(Rc::new(move |__c: &mut Option<web_sys::Node>| __parent.__hydrate_slot_{}(__c)));",
                child_var, hfield, slot_idx
            )
            .unwrap();
            writeln!(out, "        }}").unwrap();
        }
    }
    Ok(())
}

/// Phase 11.12 slice 4 — hydration adopt of a `<Child />` composition site.
/// Mirrors [`emit_child_component`] but ACQUIRES the child's server-painted
/// wrapper (`<div class="__fitz-child-<Name>">`) from the parent cursor instead
/// of creating it, get-or-creates the same cached instance, wires props/events/
/// slots (registering the `__hslot` adopt callback too), and calls
/// `child.hydrate(wrapper)` instead of `mount_into`. A later parent re-render
/// falls back to the naive build path (`emit_child_component`), reusing the same
/// cached instance so child state persists.
#[allow(clippy::too_many_arguments)]
fn emit_child_component_adopt(
    child_name: &str,
    props: &[super::expand::ChildComponentProp],
    key: Option<&Expr>,
    events: &[super::expand::ChildEventBinding],
    slot_content: &[ExpandedTemplateNode],
    cursor_var: &str,
    ctx: &mut RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    if ctx.in_for {
        // A dynamic `{#for}` child would need region adoption, which the naive
        // hydration path does not model in this slice (see the `{#if}`/`{#for}`
        // guard in `emit_template_node`).
        return Err(EmitError {
            message: format!(
                "hydration of a `<{child_name} />` inside a `{{#for}}` (dynamic \
                 composition) is not supported in this slice — a naive component \
                 hydrates static composition only. Drop `hydrate`, or move the loop \
                 into a keep-node child."
            ),
            context: format!("template of component `{}`", ctx.component_name),
        });
    }

    // Adopt the wrapper `<div class="__fitz-child-<Name>">` the server painted.
    let wrapper_var = ctx.fresh("hchild");
    writeln!(
        out,
        "        if let Some({}) = __flv_next_element(&mut {}) {{",
        wrapper_var, cursor_var
    )
    .unwrap();

    let child_var = ctx.fresh("child");
    emit_child_get_or_create(&child_var, child_name, key, ctx, out)?;
    emit_child_props(&child_var, child_name, props, ctx, out)?;
    emit_child_event_bindings(&child_var, events, ctx, out)?;
    emit_child_slot_wiring(&child_var, child_name, slot_content, true, ctx, out)?;

    writeln!(
        out,
        "        let {wrapper_var}_html = {wrapper_var}.clone().dyn_into::<HtmlElement>().unwrap();"
    )
    .unwrap();
    writeln!(
        out,
        "        {}.hydrate({wrapper_var}_html).unwrap();",
        child_var
    )
    .unwrap();
    writeln!(out, "        }}").unwrap();
    Ok(())
}

/// Phase 11.12 slice 4 — hydration adopt of a `<slot />`. If the parent filled
/// the slot (`__hslot` / `__hslot_<name>` is `Some`), invoke the adopt callback
/// with the child's cursor so it adopts the parent-painted content in PARENT
/// scope, advancing the cursor past it. Otherwise adopt the slot's own fallback
/// content (which the server painted) from the cursor.
fn emit_slot_adopt(
    name: Option<&str>,
    fallback: &[ExpandedTemplateNode],
    cursor_var: &str,
    ctx: &mut RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    let field = hslot_field_name(name);
    writeln!(out, "        let __hcb = self.{}.borrow().clone();", field).unwrap();
    writeln!(out, "        if let Some(__hcb) = __hcb {{").unwrap();
    writeln!(out, "            __hcb(&mut {});", cursor_var).unwrap();
    writeln!(out, "        }} else {{").unwrap();
    for node in fallback {
        emit_template_node(node, cursor_var, ctx, out)?;
    }
    writeln!(out, "        }}").unwrap();
    Ok(())
}

fn emit_event_attr(
    event_name: &str,
    handler_name: &str,
    el_var: &str,
    ctx: &RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    let component_name = ctx.component_name;
    // `@click` maps directly. CW.9 adds `@input` / `@change`, which read the
    // target element's live value into the handler's payload under the
    // `"value"` key (parallel to the SSR emitter, which lowers any `@event`
    // to `data-flv-<event>`). Other event names are still deferred.
    let dom_event = match event_name {
        "click" => "click",
        "input" => "input",
        "change" => "change",
        other => {
            return Err(EmitError {
                message: format!(
                    "event `@{other}` is not supported on the client-WASM target — \
                     only `@click`, `@input`, and `@change` are wired"
                ),
                context: format!(
                    "event attribute on element in template of component `{component_name}`"
                ),
            });
        }
    };

    // `@input` / `@change` deliver the element's live value — the handler
    // must read it (there is nothing else the event carries).
    let reads_value = matches!(event_name, "input" | "change");

    // Phase 11.7 R3.5b.1 — a `@click` handler that reads `payload` still
    // takes the param; with no `data-flv-value-*` attrs it receives an
    // empty map. Handlers that don't use payload keep the exact zero-arg
    // call, so the pre-R3.5b examples emit byte-for-byte unchanged.
    let takes_payload = handler_takes_payload(ctx, handler_name)?;

    if reads_value && !takes_payload {
        return Err(EmitError {
            message: format!(
                "event `@{event_name}` handler `{handler_name}` must read \
                 `payload[\"value\"]` — an `@input`/`@change` handler receives the \
                 element's current value as its payload"
            ),
            context: format!(
                "event attribute on element in template of component `{component_name}`"
            ),
        });
    }

    // The `@click` path keeps the unused `_evt` param name so its emit stays
    // byte-for-byte; the value-reading path uses `__evt`.
    let evt_param = if reads_value { "__evt" } else { "_evt" };
    writeln!(out, "        {{").unwrap();
    writeln!(out, "            let __self_clone = self.clone();").unwrap();
    writeln!(
        out,
        "            let __closure = Closure::wrap(Box::new(move |{evt_param}: Event| {{"
    )
    .unwrap();
    if reads_value {
        // Read the live value off the event target — cover <input>, <select>,
        // and <textarea>. The handler always takes payload (checked above).
        writeln!(
            out,
            "                let mut __payload = std::collections::HashMap::<String, String>::new();"
        )
        .unwrap();
        writeln!(out, "                if let Some(__t) = __evt.target() {{").unwrap();
        writeln!(
            out,
            "                    if let Some(__el) = __t.dyn_ref::<web_sys::HtmlInputElement>() {{"
        )
        .unwrap();
        writeln!(
            out,
            "                        __payload.insert(\"value\".to_string(), __el.value());"
        )
        .unwrap();
        writeln!(
            out,
            "                    }} else if let Some(__el) = __t.dyn_ref::<web_sys::HtmlSelectElement>() {{"
        )
        .unwrap();
        writeln!(
            out,
            "                        __payload.insert(\"value\".to_string(), __el.value());"
        )
        .unwrap();
        writeln!(
            out,
            "                    }} else if let Some(__el) = __t.dyn_ref::<web_sys::HtmlTextAreaElement>() {{"
        )
        .unwrap();
        writeln!(
            out,
            "                        __payload.insert(\"value\".to_string(), __el.value());"
        )
        .unwrap();
        writeln!(out, "                    }}").unwrap();
        writeln!(out, "                }}").unwrap();
        writeln!(
            out,
            "                {component_name}::{handler_name}(&__self_clone, &__payload);"
        )
        .unwrap();
    } else if takes_payload {
        writeln!(
            out,
            "                let __payload = std::collections::HashMap::<String, String>::new();"
        )
        .unwrap();
        writeln!(
            out,
            "                {}::{}(&__self_clone, &__payload);",
            component_name, handler_name
        )
        .unwrap();
    } else {
        writeln!(
            out,
            "                {}::{}(&__self_clone);",
            component_name, handler_name
        )
        .unwrap();
    }
    writeln!(out, "            }}) as Box<dyn FnMut(Event)>);").unwrap();
    writeln!(
        out,
        "            {}.add_event_listener_with_callback({}, __closure.as_ref().unchecked_ref()).unwrap();",
        el_var,
        rust_string_literal(dom_event)
    )
    .unwrap();
    writeln!(out, "            __closure.forget();").unwrap();
    writeln!(out, "        }}").unwrap();
    Ok(())
}

/// Resolve `handler_name` to a declared event of the current component
/// and report whether its emitted signature takes a `payload` param.
/// True when the body reads `payload` (Phase 11.7 R3.5b.1) OR the event
/// is bubbled to a parent (Phase 11.7 payload bubbling — a bubbled
/// handler always carries a payload so it can forward it up). Every call
/// site that emits a call to this handler consults this so the arity
/// matches its actual signature. Errors if the name is not a declared
/// `event` — catching a typo in a `@click` / `data-flv-click` binding at
/// emit time.
fn handler_takes_payload(ctx: &RenderCtx, handler_name: &str) -> EmitResult<bool> {
    let handler = ctx
        .events
        .iter()
        .find(|h| h.name == handler_name)
        .ok_or_else(|| EmitError {
            message: format!(
                "event binding references `{handler_name}`, which is not an `event` \
                 declared by component `{}`",
                ctx.component_name
            ),
            context: format!("template of component `{}`", ctx.component_name),
        })?;
    Ok(handler_uses_payload(handler) || ctx.this_bubbled.contains(handler_name))
}

/// Emit a `data-flv-click="handler"` click listener (Phase 11.7
/// R3.5b.1). Builds a `payload: HashMap<String, String>` by reading the
/// element's `data-flv-value-*` attributes back from the DOM (the same
/// element the SSR client runtime reads), then calls the target handler.
/// A handler that doesn't read `payload` is called with no argument (and
/// the payload build is skipped).
fn emit_data_flv_click(
    handler_name: &str,
    value_keys: &[String],
    el_var: &str,
    ctx: &RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    let takes_payload = handler_takes_payload(ctx, handler_name)?;
    let component_name = ctx.component_name;

    writeln!(out, "        {{").unwrap();
    writeln!(out, "            let __self_clone = self.clone();").unwrap();
    if takes_payload {
        // Capture a handle to the element so the listener can read its
        // `data-flv-value-*` attributes when the click fires.
        writeln!(out, "            let __evt_el = {}.clone();", el_var).unwrap();
    }
    writeln!(
        out,
        "            let __closure = Closure::wrap(Box::new(move |_evt: Event| {{"
    )
    .unwrap();
    if takes_payload {
        writeln!(
            out,
            "                let mut __payload = std::collections::HashMap::<String, String>::new();"
        )
        .unwrap();
        for key in value_keys {
            let attr = format!("data-flv-value-{key}");
            writeln!(
                out,
                "                __payload.insert({}.to_string(), __evt_el.get_attribute({}).unwrap_or_default());",
                rust_string_literal(key),
                rust_string_literal(&attr)
            )
            .unwrap();
        }
        writeln!(
            out,
            "                {}::{}(&__self_clone, &__payload);",
            component_name, handler_name
        )
        .unwrap();
    } else {
        writeln!(
            out,
            "                {}::{}(&__self_clone);",
            component_name, handler_name
        )
        .unwrap();
    }
    writeln!(out, "            }}) as Box<dyn FnMut(Event)>);").unwrap();
    writeln!(
        out,
        "            {}.add_event_listener_with_callback(\"click\", __closure.as_ref().unchecked_ref()).unwrap();",
        el_var
    )
    .unwrap();
    writeln!(out, "            __closure.forget();").unwrap();
    writeln!(out, "        }}").unwrap();
    Ok(())
}

/// A named form field discovered inside a `data-flv-submit` form (Phase
/// 11.7 R3.5b.2). `clear` is `true` when the input carries
/// `data-flv-clear` (reset to "" after submit).
struct FormField {
    name: String,
    clear: bool,
}

/// Collect the named `<input>` / `<textarea>` / `<select>` fields inside
/// a `data-flv-submit` form, in document order (Phase 11.7 R3.5b.2).
/// Descends through nested elements + `{#if}` / `{#for}` so a field
/// wrapped in a `<div>` or a directive is still found.
fn collect_form_fields(children: &[ExpandedTemplateNode]) -> Vec<FormField> {
    let mut fields = Vec::new();
    for node in children {
        collect_fields_in_node(node, &mut fields);
    }
    fields
}

fn collect_fields_in_node(node: &ExpandedTemplateNode, fields: &mut Vec<FormField>) {
    match node {
        ExpandedTemplateNode::Element {
            tag,
            attrs,
            children,
            ..
        } => {
            if matches!(tag.as_str(), "input" | "textarea" | "select") {
                let mut name: Option<String> = None;
                let mut clear = false;
                for attr in attrs {
                    if let ExpandedAttr::Static { name: n, value, .. } = attr {
                        if n == "name" {
                            name = Some(value.clone());
                        } else if n == "data-flv-clear" {
                            clear = true;
                        }
                    }
                }
                if let Some(nm) = name {
                    fields.push(FormField { name: nm, clear });
                }
            }
            for c in children {
                collect_fields_in_node(c, fields);
            }
        }
        ExpandedTemplateNode::If {
            children,
            else_children,
            ..
        } => {
            for c in children {
                collect_fields_in_node(c, fields);
            }
            if let Some(els) = else_children {
                for c in els {
                    collect_fields_in_node(c, fields);
                }
            }
        }
        ExpandedTemplateNode::For { children, .. } => {
            for c in children {
                collect_fields_in_node(c, fields);
            }
        }
        _ => {}
    }
}

/// Emit a `data-flv-submit="handler"` submit listener (Phase 11.7
/// R3.5b.2). Prevents the default navigation, reads each named field's
/// value into a `payload: HashMap<String, String>`, calls the target
/// handler, then clears every field marked `data-flv-clear`.
///
/// Fields are read + cleared via `form.query_selector("[name=\"x\"]")`
/// cast to `HtmlInputElement` — so this path needs the `HtmlInputElement`
/// web-sys feature, added to the emitted `Cargo.toml` only when a form is
/// present (see `wasm_extra_web_sys_features`).
fn emit_data_flv_submit(
    handler_name: &str,
    fields: &[FormField],
    el_var: &str,
    ctx: &RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    let takes_payload = handler_takes_payload(ctx, handler_name)?;
    let component_name = ctx.component_name;

    writeln!(out, "        {{").unwrap();
    writeln!(out, "            let __self_clone = self.clone();").unwrap();
    writeln!(out, "            let __form_el = {}.clone();", el_var).unwrap();
    writeln!(
        out,
        "            let __closure = Closure::wrap(Box::new(move |__evt: Event| {{"
    )
    .unwrap();
    writeln!(out, "                __evt.prevent_default();").unwrap();
    if takes_payload {
        writeln!(
            out,
            "                let mut __payload = std::collections::HashMap::<String, String>::new();"
        )
        .unwrap();
        for f in fields {
            let sel = format!("[name=\"{}\"]", f.name);
            writeln!(
                out,
                "                if let Some(__f) = __form_el.query_selector({}).ok().flatten() {{",
                rust_string_literal(&sel)
            )
            .unwrap();
            writeln!(
                out,
                "                    if let Ok(__inp) = __f.dyn_into::<web_sys::HtmlInputElement>() {{"
            )
            .unwrap();
            writeln!(
                out,
                "                        __payload.insert({}.to_string(), __inp.value());",
                rust_string_literal(&f.name)
            )
            .unwrap();
            writeln!(out, "                    }}").unwrap();
            writeln!(out, "                }}").unwrap();
        }
        writeln!(
            out,
            "                {}::{}(&__self_clone, &__payload);",
            component_name, handler_name
        )
        .unwrap();
    } else {
        writeln!(
            out,
            "                {}::{}(&__self_clone);",
            component_name, handler_name
        )
        .unwrap();
    }
    // Clear inputs marked `data-flv-clear` (after the handler ran).
    for f in fields.iter().filter(|f| f.clear) {
        let sel = format!("[name=\"{}\"]", f.name);
        writeln!(
            out,
            "                if let Some(__f) = __form_el.query_selector({}).ok().flatten() {{",
            rust_string_literal(&sel)
        )
        .unwrap();
        writeln!(
            out,
            "                    if let Ok(__inp) = __f.dyn_into::<web_sys::HtmlInputElement>() {{ __inp.set_value(\"\"); }}"
        )
        .unwrap();
        writeln!(out, "                }}").unwrap();
    }
    writeln!(out, "            }}) as Box<dyn FnMut(Event)>);").unwrap();
    writeln!(
        out,
        "            {}.add_event_listener_with_callback(\"submit\", __closure.as_ref().unchecked_ref()).unwrap();",
        el_var
    )
    .unwrap();
    writeln!(out, "            __closure.forget();").unwrap();
    writeln!(out, "        }}").unwrap();
    Ok(())
}

/// The extra `web-sys` features the emitted crate needs beyond the base
/// set, derived from the `.fitzv`'s template shapes (Phase 11.7 R3.5b.2).
/// A `data-flv-submit` form reads inputs via `HtmlInputElement`. Returns
/// an empty slice for form-free components, so the pre-R3.5b.2 examples'
/// `Cargo.toml` is unchanged.
pub fn wasm_extra_web_sys_features(file: &ExpandedViewFile) -> Vec<&'static str> {
    let mut features: Vec<&'static str> = Vec::new();
    if file.components.iter().any(component_uses_form_submit) {
        features.push("HtmlInputElement");
    }
    if file.components.iter().any(component_uses_file_input) {
        // `<input type="file">` + `data-flv-file`: read the picked file via
        // `HtmlInputElement.files()` → `FileList.get()` → `FileReader`
        // (`read_as_data_url` takes a `&Blob`; `File` derefs to it).
        for f in ["HtmlInputElement", "FileReader", "File", "FileList", "Blob"] {
            if !features.contains(&f) {
                features.push(f);
            }
        }
    }
    if file.components.iter().any(component_uses_value_input) {
        // CW.9 — `@input` / `@change` read the target's `.value()` by casting
        // to the concrete element type (input / select / textarea).
        for f in [
            "HtmlInputElement",
            "HtmlSelectElement",
            "HtmlTextAreaElement",
        ] {
            if !features.contains(&f) {
                features.push(f);
            }
        }
    }
    if file.components.iter().any(component_uses_keep_regions) {
        // Phase 11.10 slice 3 — keep-node `{#if}`/`{#for}` dynamic regions:
        // comment anchors (`create_comment`) bound each region, and its
        // content is (re)built into a `DocumentFragment`.
        for f in ["Comment", "DocumentFragment"] {
            if !features.contains(&f) {
                features.push(f);
            }
        }
    }
    features
}

/// Find a nested `<Child />` inside slot content (Phase 11.7.d) — such
/// composition is rejected because the parent's slot-render method has no
/// child-instance cache field for it. Returns the offending component
/// name if found.
fn first_nested_child_name(node: &ExpandedTemplateNode) -> Option<String> {
    match node {
        ExpandedTemplateNode::ChildComponent { name, .. } => Some(name.clone()),
        ExpandedTemplateNode::Element { children, .. } => {
            children.iter().find_map(first_nested_child_name)
        }
        ExpandedTemplateNode::If {
            children,
            else_children,
            ..
        } => children
            .iter()
            .find_map(first_nested_child_name)
            .or_else(|| {
                else_children
                    .as_ref()
                    .and_then(|els| els.iter().find_map(first_nested_child_name))
            }),
        ExpandedTemplateNode::For { children, .. } => {
            children.iter().find_map(first_nested_child_name)
        }
        _ => None,
    }
}

/// The set of `<slot />` holes a component's template declares (Phase
/// 11.7.d default slot + named slots). `has_default` is true when a
/// bare `<slot />` appears; `named` lists every distinct
/// `<slot name="X" />` name in first-seen order.
#[derive(Default)]
struct SlotSet {
    has_default: bool,
    named: Vec<String>,
}

/// Collect the `<slot />` holes in `component`'s template, descending
/// into Element / `{#if}` / `{#for}` the same way the render walk does.
fn component_slot_set(component: &ExpandedComponent) -> SlotSet {
    fn walk(node: &ExpandedTemplateNode, set: &mut SlotSet) {
        match node {
            ExpandedTemplateNode::Slot { name, .. } => match name {
                None => set.has_default = true,
                Some(n) => {
                    if !set.named.contains(n) {
                        set.named.push(n.clone());
                    }
                }
            },
            ExpandedTemplateNode::Element { children, .. } => {
                children.iter().for_each(|c| walk(c, set))
            }
            ExpandedTemplateNode::If {
                children,
                else_children,
                ..
            } => {
                children.iter().for_each(|c| walk(c, set));
                if let Some(els) = else_children {
                    els.iter().for_each(|c| walk(c, set));
                }
            }
            ExpandedTemplateNode::For { children, .. } => {
                children.iter().for_each(|c| walk(c, set))
            }
            _ => {}
        }
    }
    let mut set = SlotSet::default();
    if let Some(t) = &component.template {
        t.roots.iter().for_each(|n| walk(n, &mut set));
    }
    set
}

/// The Rust struct-field name backing a slot. The default slot keeps
/// the bare `__slot` field (unchanged since 11.7.d — byte-for-byte for
/// default-only components); a named slot `X` gets `__slot_<X>` with
/// hyphens folded to underscores.
fn slot_field_name(name: Option<&str>) -> String {
    match name {
        None => "__slot".to_string(),
        Some(n) => format!("__slot_{}", sanitize_slot_ident(n)),
    }
}

/// Phase 11.12 slice 4 — the struct-field name backing a slot's HYDRATION
/// callback. Parallel to [`slot_field_name`] but stores a cursor-consuming
/// adopt renderer (`Fn(&mut Option<web_sys::Node>)`) instead of the build
/// renderer (`Fn(&web_sys::Node)`). A slot-declaring component that hydrates
/// carries both: `__slot` for a later naive rebuild, `__hslot` for the one
/// initial adopt. Only emitted for hydratable components, so pre-11.12
/// slot examples stay byte-identical.
fn hslot_field_name(name: Option<&str>) -> String {
    match name {
        None => "__hslot".to_string(),
        Some(n) => format!("__hslot_{}", sanitize_slot_ident(n)),
    }
}

/// Fold a slot name into a Rust-identifier-safe fragment (`-` → `_`).
fn sanitize_slot_ident(name: &str) -> String {
    name.replace('-', "_")
}

/// Human-readable list of a child's declared named slots, for error
/// messages.
fn describe_named_slots(set: &SlotSet) -> String {
    if set.named.is_empty() {
        "none".to_string()
    } else {
        set.named
            .iter()
            .map(|n| format!("`{n}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Validate a component's named slots: each name must be a legal Rust
/// identifier fragment (after folding `-` → `_`) and no two names may
/// collide on the same backing field. Errors surface at WASM emit time
/// with a clear pointer.
fn validate_slot_set(component_name: &str, slot_set: &SlotSet) -> EmitResult<()> {
    let mut seen_fields: Vec<(String, String)> = Vec::new();
    for name in &slot_set.named {
        let sane = sanitize_slot_ident(name);
        let mut chars = sane.chars();
        let ok = match chars.next() {
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
            }
            _ => false,
        };
        if !ok {
            return Err(EmitError {
                message: format!(
                    "slot name `{name}` is not a valid identifier on the client-WASM \
                     target. Use letters, digits, `_` or `-` (hyphens fold to `_`), \
                     starting with a letter or `_` (e.g. `<slot name=\"header\" />`)."
                ),
                context: format!("template of component `{component_name}`"),
            });
        }
        let field = format!("__slot_{sane}");
        if let Some((other, _)) = seen_fields.iter().find(|(_, f)| f == &field) {
            return Err(EmitError {
                message: format!(
                    "slot names `{other}` and `{name}` both map to the same backing \
                     field `{field}` on the client-WASM target — rename one so the \
                     two named slots stay distinct."
                ),
                context: format!("template of component `{component_name}`"),
            });
        }
        seen_fields.push((name.clone(), field));
    }
    Ok(())
}

/// The value of an element's `slot="X"` static attribute (a named-slot
/// routing directive on parent-provided slot content), if present.
fn element_slot_attr(node: &ExpandedTemplateNode) -> Option<&str> {
    if let ExpandedTemplateNode::Element { attrs, .. } = node {
        for a in attrs {
            if let ExpandedAttr::Static { name, value, .. } = a {
                if name == "slot" {
                    return Some(value);
                }
            }
        }
    }
    None
}

/// Clone an element node with its `slot="..."` routing attribute
/// removed — the attribute directs placement, it isn't real content.
fn strip_slot_attr(node: &ExpandedTemplateNode) -> ExpandedTemplateNode {
    let mut cloned = node.clone();
    if let ExpandedTemplateNode::Element { attrs, .. } = &mut cloned {
        attrs.retain(|a| !matches!(a, ExpandedAttr::Static { name, .. } if name == "slot"));
    }
    cloned
}

/// True for a whitespace-only text node (incidental formatting between
/// slotted elements), which shouldn't force a default `<slot />`.
fn is_whitespace_text(node: &ExpandedTemplateNode) -> bool {
    matches!(node, ExpandedTemplateNode::Text(t) if t.trim().is_empty())
}

/// Emit a `data-flv-file="handler"` change listener on an `<input type="file">`.
/// On selection it reads the first file via `FileReader::read_as_data_url` and
/// calls the handler with a payload map — `data` (the data-URL string), `name`
/// (the filename), `type` (the MIME type). The handler stores what it needs in
/// state (`state.img = payload["data"]`) and the template renders it
/// (`<img src="{img}">`). Reading is async, so the `FileReader.onload` closure
/// is `.forget()`-leaked to outlive the read (same discipline as the other
/// event closures). The handler must read `payload` — client-WASM file
/// selection always carries one; a handler that ignores it gets a signature
/// mismatch at compile time.
fn emit_data_flv_file(
    handler_name: &str,
    el_var: &str,
    ctx: &RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    let component_name = ctx.component_name;
    writeln!(out, "        {{").unwrap();
    writeln!(out, "            let __self_clone = self.clone();").unwrap();
    writeln!(
        out,
        "            let __closure = Closure::wrap(Box::new(move |__evt: Event| {{"
    )
    .unwrap();
    writeln!(
        out,
        "                let __input = match __evt.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {{ Some(i) => i, None => return }};"
    )
    .unwrap();
    writeln!(
        out,
        "                let __file = match __input.files().and_then(|fs| fs.get(0)) {{ Some(f) => f, None => return }};"
    )
    .unwrap();
    writeln!(
        out,
        "                let __reader = match web_sys::FileReader::new() {{ Ok(r) => r, Err(_) => return }};"
    )
    .unwrap();
    writeln!(out, "                let __reader2 = __reader.clone();").unwrap();
    writeln!(out, "                let __name = __file.name();").unwrap();
    writeln!(out, "                let __type = __file.type_();").unwrap();
    writeln!(out, "                let __self2 = __self_clone.clone();").unwrap();
    writeln!(
        out,
        "                let __onload = Closure::wrap(Box::new(move |_e: Event| {{"
    )
    .unwrap();
    writeln!(
        out,
        "                    let __data = __reader2.result().ok().and_then(|v| v.as_string()).unwrap_or_default();"
    )
    .unwrap();
    writeln!(
        out,
        "                    let mut __payload = std::collections::HashMap::<String, String>::new();"
    )
    .unwrap();
    writeln!(
        out,
        "                    __payload.insert(\"data\".to_string(), __data);"
    )
    .unwrap();
    writeln!(
        out,
        "                    __payload.insert(\"name\".to_string(), __name.clone());"
    )
    .unwrap();
    writeln!(
        out,
        "                    __payload.insert(\"type\".to_string(), __type.clone());"
    )
    .unwrap();
    writeln!(
        out,
        "                    {}::{}(&__self2, &__payload);",
        component_name, handler_name
    )
    .unwrap();
    writeln!(out, "                }}) as Box<dyn FnMut(Event)>);").unwrap();
    writeln!(
        out,
        "                __reader.set_onload(Some(__onload.as_ref().unchecked_ref()));"
    )
    .unwrap();
    writeln!(out, "                __onload.forget();").unwrap();
    writeln!(
        out,
        "                let _ = __reader.read_as_data_url(&__file);"
    )
    .unwrap();
    writeln!(out, "            }}) as Box<dyn FnMut(Event)>);").unwrap();
    writeln!(
        out,
        "            {}.add_event_listener_with_callback(\"change\", __closure.as_ref().unchecked_ref()).unwrap();",
        el_var
    )
    .unwrap();
    writeln!(out, "            __closure.forget();").unwrap();
    writeln!(out, "        }}").unwrap();
    Ok(())
}

/// True when any element in the component carries `data-flv-file` (needs the
/// `FileReader` / `File` / `FileList` web-sys features — see
/// [`wasm_extra_web_sys_features`]).
fn component_uses_file_input(component: &ExpandedComponent) -> bool {
    fn node_has_file(node: &ExpandedTemplateNode) -> bool {
        match node {
            ExpandedTemplateNode::Element {
                attrs, children, ..
            } => {
                attrs.iter().any(
                    |a| matches!(a, ExpandedAttr::Static { name, .. } if name == "data-flv-file"),
                ) || children.iter().any(node_has_file)
            }
            ExpandedTemplateNode::If {
                children,
                else_children,
                ..
            } => {
                children.iter().any(node_has_file)
                    || else_children
                        .as_ref()
                        .is_some_and(|els| els.iter().any(node_has_file))
            }
            ExpandedTemplateNode::For { children, .. } => children.iter().any(node_has_file),
            _ => false,
        }
    }
    component
        .template
        .as_ref()
        .is_some_and(|t| t.roots.iter().any(node_has_file))
}

fn component_uses_form_submit(component: &ExpandedComponent) -> bool {
    fn node_has_submit(node: &ExpandedTemplateNode) -> bool {
        match node {
            ExpandedTemplateNode::Element {
                attrs, children, ..
            } => {
                attrs.iter().any(
                    |a| matches!(a, ExpandedAttr::Static { name, .. } if name == "data-flv-submit"),
                ) || children.iter().any(node_has_submit)
            }
            ExpandedTemplateNode::If {
                children,
                else_children,
                ..
            } => {
                children.iter().any(node_has_submit)
                    || else_children
                        .as_ref()
                        .is_some_and(|els| els.iter().any(node_has_submit))
            }
            ExpandedTemplateNode::For { children, .. } => children.iter().any(node_has_submit),
            _ => false,
        }
    }
    component
        .template
        .as_ref()
        .is_some_and(|t| t.roots.iter().any(node_has_submit))
}

/// CW.9 — true when the component wires an `@input` / `@change` handler,
/// which reads the target element's live value. The emitted closure casts
/// the event target to `HtmlInputElement` / `HtmlSelectElement` /
/// `HtmlTextAreaElement`, so those web-sys features must be enabled.
fn component_uses_value_input(component: &ExpandedComponent) -> bool {
    fn node_has_value_event(node: &ExpandedTemplateNode) -> bool {
        match node {
            ExpandedTemplateNode::Element {
                attrs, children, ..
            } => {
                attrs.iter().any(|a| {
                    matches!(
                        a,
                        ExpandedAttr::Event { event_name, .. }
                            if event_name == "input" || event_name == "change"
                    )
                }) || children.iter().any(node_has_value_event)
            }
            ExpandedTemplateNode::If {
                children,
                else_children,
                ..
            } => {
                children.iter().any(node_has_value_event)
                    || else_children
                        .as_ref()
                        .is_some_and(|els| els.iter().any(node_has_value_event))
            }
            ExpandedTemplateNode::For { children, .. } => children.iter().any(node_has_value_event),
            _ => false,
        }
    }
    component
        .template
        .as_ref()
        .is_some_and(|t| t.roots.iter().any(node_has_value_event))
}

// ---------------------------------------------------------------------------
// Style helper
// ---------------------------------------------------------------------------

/// Emit the scoped/global style injection helper as a free `fn`.
/// Scoped variants dedup via `AtomicBool` so N mounted instances of
/// the same component share one `<style>` block. Global variants
/// dedup by component name + a body hash so re-mounts of the same
/// component do not re-inject; but different components with
/// `<style global>` blocks that happen to have identical CSS still
/// inject twice, since we can't cheaply know at emit time whether
/// two Global blocks are the same. Acceptable trade-off for POC.
fn emit_style_helper(component_name: &str, style: &ExpandedStyle, out: &mut String) {
    let helper = style_helper_ident(component_name, style);
    let css = match style {
        ExpandedStyle::Scoped { css_scoped, .. } => css_scoped,
        ExpandedStyle::Global { css, .. } => css,
    };
    writeln!(out, "fn {}() {{", helper).unwrap();
    writeln!(
        out,
        "    static INJECTED: AtomicBool = AtomicBool::new(false);"
    )
    .unwrap();
    writeln!(
        out,
        "    if INJECTED.swap(true, Ordering::SeqCst) {{ return; }}"
    )
    .unwrap();
    writeln!(
        out,
        "    let document = web_sys::window().unwrap().document().unwrap();"
    )
    .unwrap();
    writeln!(out, "    let head = document.head().unwrap();").unwrap();
    writeln!(
        out,
        "    let style_el = document.create_element(\"style\").unwrap();"
    )
    .unwrap();
    writeln!(
        out,
        "    style_el.set_text_content(Some({}));",
        rust_string_literal(css)
    )
    .unwrap();
    writeln!(out, "    let _ = head.append_child(&style_el);").unwrap();
    writeln!(out, "}}\n").unwrap();
}

/// Compute the Rust fn name for the style injection helper.
/// Format: `__inject_style_<component_name>_<scope_class_sanitized>`
/// para Scoped, o `__inject_style_<component_name>_global` para
/// Global. Sanitiza hyphens del scope_class (que vienen del
/// kebab-case) reemplazándolos por `_` para producir un ident Rust
/// válido.
fn style_helper_ident(component_name: &str, style: &ExpandedStyle) -> String {
    match style {
        ExpandedStyle::Scoped { scope_class, .. } => {
            format!(
                "__inject_style_{}_{}",
                sanitize_ident(component_name),
                sanitize_ident(scope_class)
            )
        }
        ExpandedStyle::Global { .. } => {
            format!("__inject_style_{}_global", sanitize_ident(component_name))
        }
    }
}

// ---------------------------------------------------------------------------
// Expr / Stmt lowerers (mini-lowering for the POC subset)
// ---------------------------------------------------------------------------

/// Lower a Fitz `Expr` to a Rust expression string. Supports ONLY
/// the subset needed for the counter POC:
///
/// - `Int` / `Float` / `Bool` literals (Str deferred to 11.4.c since
///   the POC only inspects state fields of type Int).
/// - `Ident` referring to a state field name — emits
///   `(*self.<name>.borrow())`.
/// - `BinOp` with numeric ops (Add/Sub/Mul/Div/Mod) recursing into
///   the sub-expressions.
///
/// Todo lo demás (Str literal, StrInterp, Ident no-state, Call,
/// Field, Index, comparisons, logical ops, etc.) devuelve `EmitError`
/// citando la sub-fase donde se cierra.
fn lower_expr(expr: &Expr, state_names: &[String], locals: &[String]) -> EmitResult<String> {
    match expr {
        Expr::Int(n, _) => Ok(format!("{}i64", n)),
        Expr::Float(n, _) => Ok(format!("{}f64", n)),
        Expr::Bool(b, _) => Ok(b.to_string()),
        Expr::Ident(name, _) => {
            if locals.iter().any(|s| s == name) {
                // Phase 11.7.b — a loop var (`{#for x in ...}`) is a
                // plain Rust binding in scope; emit it directly.
                Ok(name.clone())
            } else if state_names.iter().any(|s| s == name) {
                Ok(format!("(*self.{}.borrow())", name))
            } else {
                Err(EmitError {
                    message: format!(
                        "identifier `{}` is not a state field nor a loop variable — non-state refs deferred to Phase 11.4.c",
                        name
                    ),
                    context: "expression".to_string(),
                })
            }
        }
        Expr::BinOp {
            op, left, right, ..
        } => {
            let l = lower_expr(left, state_names, locals)?;
            let r = lower_expr(right, state_names, locals)?;
            // String concatenation: Fitz `+` on strings maps to Rust `String +
            // &str`, but both sides here lower to owned `String`s (and a numeric
            // `+` would be wrong). When either operand is clearly a string
            // (a literal or an interpolation), emit a `format!` concat.
            if matches!(op, BinOpKind::Add) && (expr_is_stringy(left) || expr_is_stringy(right)) {
                Ok(format!("format!(\"{{}}{{}}\", {}, {})", l, r))
            } else {
                let op_str = lower_binop(op)?;
                Ok(format!("({} {} {})", l, op_str, r))
            }
        }
        // Phase 11.7 R3 — a Str literal, needed for nominal struct
        // construction (`Card { title: "New", .. }`) and interpolation.
        // Emits an owned `String` so it drops into a struct field or a
        // `RefCell<String>` uniformly.
        Expr::Str(s, _) => Ok(format!("{s:?}.to_string()")),
        // Phase 11.7 R3 — field access on a nominal (`c.title`, where
        // `c` is a `{#for}` loop var of nominal type, or a nominal
        // state field). Clones the field so the result is owned
        // (`String`/`i64`/`f64`/`bool` all impl `Clone`), which drops
        // into `format!("{}", ...)`, a keyed `key`, or a struct field.
        Expr::Field { object, field, .. } => {
            let obj = lower_expr(object, state_names, locals)?;
            Ok(format!("{obj}.{field}.clone()"))
        }
        // Phase 11.7 R3 — nominal construction (`Card { id: next_id,
        // title: "New", done: false }`) inside event bodies + defaults.
        // Each field value lowers recursively. All declared fields must
        // be supplied (the emitter does not fill defaults) — the classic
        // check pass validates the type name; missing fields surface as
        // a rustc error in the generated crate (documented R3 limit).
        Expr::StructLit {
            type_name, fields, ..
        } => {
            let mut parts: Vec<String> = Vec::with_capacity(fields.len());
            for (fname, fexpr) in fields {
                let val = lower_expr(fexpr, state_names, locals)?;
                parts.push(format!("{fname}: {val}"));
            }
            Ok(format!("{type_name} {{ {} }}", parts.join(", ")))
        }
        // Phase 11.7 R3.5a.1 — list literal, so a list state field can be
        // re-seeded (`nums = [1, 2, 3]`) and small literals appear in
        // event bodies. Emits `vec![...]`; the element expressions lower
        // recursively (primitive literals in the common case).
        Expr::List(items, _) => {
            let mut parts: Vec<String> = Vec::with_capacity(items.len());
            for it in items {
                parts.push(lower_expr(it, state_names, locals)?);
            }
            Ok(format!("vec![{}]", parts.join(", ")))
        }
        // Phase 11.7 R3.5a.1 — `if` used as a VALUE
        // (`let col = if (dir == "right") { next(c) } else { prev(c) }`).
        // Both branches must be single-expression blocks; the condition
        // lowers via `lower_cond_expr`. Statement-position `if` (a guard
        // with side effects) is handled in `lower_stmt`, not here.
        Expr::If {
            condition,
            then,
            else_,
            ..
        } => {
            let cond = lower_cond_expr(condition, state_names, locals)?;
            let then_rust = lower_single_expr_block(then, state_names, locals)?;
            let else_stmts = else_.as_ref().ok_or_else(|| EmitError {
                message: "`if` used as a value needs an `else` branch on the \
                          client-WASM target (Phase 11.7 R3.5a.1)"
                    .to_string(),
                context: "expression".to_string(),
            })?;
            let else_rust = lower_single_expr_block(else_stmts, state_names, locals)?;
            Ok(format!(
                "(if {cond} {{ {then_rust} }} else {{ {else_rust} }})"
            ))
        }
        // Phase 11.7 R3.5a.1 — calls. Method calls on lists
        // (`.map`/`.filter`/`.len`) lower to Rust iterator chains here;
        // free-function calls (`cards_in(cards, "todo")`) are deferred to
        // R3.5a.2, when imported classic helpers are transpiled into the
        // bundle.
        Expr::Call { callee, args, .. } => lower_call(callee, args, state_names, locals),
        // Phase 11.7 R3.5b.1 — `payload["key"]`. `payload` is the
        // `&HashMap<String, String>` param on a payload-using event
        // handler; a missing key yields "" (Map<Str,Str> lookup
        // semantics). Other indexing (`xs[i]` on a list/map) is still
        // deferred.
        Expr::Index { object, index, .. } => {
            if let Expr::Ident(name, _) = object.as_ref() {
                if name == "payload" {
                    let key = lower_expr(index, state_names, locals)?;
                    return Ok(format!(
                        "payload.get(&({key})).cloned().unwrap_or_default()"
                    ));
                }
            }
            Err(EmitError {
                message: "indexing `xs[i]` — only `payload[\"key\"]` is supported on the \
                          client-WASM target (Phase 11.7 R3.5b.1); list/map indexing \
                          defers to a later slice"
                    .to_string(),
                context: "expression".to_string(),
            })
        }
        // Phase 11.7 R3.5c — string interpolation (`"{next_id}"`,
        // `"card-{id}"`) → a Rust `format!`. Literal parts are copied
        // (braces doubled for `format!`); each interpolated expression
        // becomes a `{}` placeholder with the lowered expr as the arg.
        // Format specs are ignored on the WASM target (plain `{}`).
        Expr::StrInterp(parts, _) => {
            let mut fmt = String::new();
            let mut args: Vec<String> = Vec::new();
            for part in parts {
                match part {
                    StrPart::Lit(s) => {
                        for ch in s.chars() {
                            if ch == '{' || ch == '}' {
                                fmt.push(ch);
                            }
                            fmt.push(ch);
                        }
                    }
                    StrPart::Expr(e, _spec) => {
                        fmt.push_str("{}");
                        args.push(lower_expr(e, state_names, locals)?);
                    }
                }
            }
            if args.is_empty() {
                Ok(format!("format!({})", rust_string_literal(&fmt)))
            } else {
                Ok(format!(
                    "format!({}, {})",
                    rust_string_literal(&fmt),
                    args.join(", ")
                ))
            }
        }
        // `match <value> { <pat> => <expr>, … }` as a value (e.g. `let checked =
        // match val == selected { true => " checked", false => "" }`). Each arm
        // body must be a single expression. If any pattern is a Str, match on
        // `<value>.as_str()` so `"…"` patterns type-check against `&str`.
        Expr::Match { value, arms, .. } => {
            let scrutinee_base = lower_expr(value, state_names, locals)?;
            let has_str_pat = arms
                .iter()
                .any(|a| matches!(a.pattern, crate::ast::Pattern::Str(_)));
            let scrutinee = if has_str_pat {
                format!("({scrutinee_base}).as_str()")
            } else {
                scrutinee_base
            };
            let mut arm_strs: Vec<String> = Vec::with_capacity(arms.len());
            for arm in arms {
                let pat = lower_pattern_wasm(&arm.pattern)?;
                // CW.9 (1a) — bring the arm's pattern bindings (`Ok(v)`,
                // `Err(e)`, a bare ident) into scope for the arm body.
                let mut arm_locals = locals.to_vec();
                for b in pattern_bindings(&arm.pattern) {
                    if !arm_locals.iter().any(|l| l == &b) {
                        arm_locals.push(b);
                    }
                }
                let val = lower_single_expr_block(&arm.body, state_names, &arm_locals)?;
                arm_strs.push(format!("{pat} => {val}"));
            }
            Ok(format!("match {} {{ {} }}", scrutinee, arm_strs.join(", ")))
        }
        // Phase 11.11.c — `<expr>.await` inside an async event handler
        // (a call to an `@rpc` stub). Legal only in the async fn the
        // handler emits when its body contains an await.
        Expr::Await(inner, _) => {
            let base = lower_expr(inner, state_names, locals)?;
            Ok(format!("{base}.await"))
        }
        // Phase 11.11.c — `<expr>?`. The async handler fn returns
        // `Result<(), String>`, so `?` on the `Result<T, String>` of an
        // `@rpc` stub propagates its error string and unwraps `T`.
        Expr::Try(inner, _) => {
            let base = lower_expr(inner, state_names, locals)?;
            Ok(format!("{base}?"))
        }
        // CW.9 (1a) — `Ok(v)` / `Err(e)` constructors in a helper-fn body.
        // The enclosing fn returns `Result<T, String>` (see
        // `type_expr_to_rust`), so these build that value directly. `Err`'s
        // inner lowers to an owned `String` (a Str literal / interpolation),
        // matching the pinned error type.
        Expr::Ok(inner, _) => {
            let base = lower_expr(inner, state_names, locals)?;
            Ok(format!("Ok({base})"))
        }
        Expr::Err(inner, _) => {
            let base = lower_expr(inner, state_names, locals)?;
            Ok(format!("Err({base})"))
        }
        Expr::Null(_)
        | Expr::Bytes(_, _)
        | Expr::UnaryOp { .. }
        | Expr::NamedArg { .. }
        | Expr::FnExpr { .. } => Err(EmitError {
            message: format!(
                "expression kind `{}` — deferred to a later 11.7 slice / the SSR target",
                expr_kind_name(expr)
            ),
            context: "expression".to_string(),
        }),
        _ => Err(EmitError {
            message: "unsupported expression kind — deferred to a later 11.7 slice".to_string(),
            context: "expression".to_string(),
        }),
    }
}

/// Heuristic: does this expression clearly produce a `String`? Routes `+` to a
/// `format!` concat (Fitz string concatenation) instead of a numeric `+`.
/// Recognises string literals, interpolations, and a `+` that is itself stringy.
fn expr_is_stringy(e: &Expr) -> bool {
    match e {
        Expr::Str(..) | Expr::StrInterp(..) => true,
        Expr::BinOp {
            op: BinOpKind::Add,
            left,
            right,
            ..
        } => expr_is_stringy(left) || expr_is_stringy(right),
        _ => false,
    }
}

fn expr_kind_name(expr: &Expr) -> &'static str {
    match expr {
        Expr::Int(..) => "Int",
        Expr::Float(..) => "Float",
        Expr::Str(..) => "Str",
        Expr::StrInterp(..) => "StrInterp",
        Expr::Bool(..) => "Bool",
        Expr::Null(..) => "Null",
        Expr::Bytes(..) => "Bytes",
        Expr::Ident(..) => "Ident",
        Expr::BinOp { .. } => "BinOp",
        Expr::UnaryOp { .. } => "UnaryOp",
        Expr::Call { .. } => "Call",
        Expr::NamedArg { .. } => "NamedArg",
        Expr::FnExpr { .. } => "FnExpr",
        Expr::Field { .. } => "Field",
        Expr::Index { .. } => "Index",
        _ => "Other",
    }
}

fn lower_binop(op: &BinOpKind) -> EmitResult<&'static str> {
    match op {
        BinOpKind::Add => Ok("+"),
        BinOpKind::Sub => Ok("-"),
        BinOpKind::Mul => Ok("*"),
        BinOpKind::Div => Ok("/"),
        BinOpKind::Mod => Ok("%"),
        // Phase 11.7 R3.5a.1 — comparison operators, so closure bodies
        // (`fn(n) => n % 2 == 0`), `if`-as-expression conditions, and
        // `{#if}` guards can lower to a Rust `bool`. String equality
        // (`c.column == col`) falls out because `Expr::Field` lowering
        // clones to an owned `String` and Rust's `String: PartialEq`
        // does the compare. Logical (`&&`/`||`/`!`) + bitwise stay
        // deferred — `lower_cond_expr` already covers `&&`/`||`/`!` in
        // condition position, which is where the kanban needs them.
        BinOpKind::Eq => Ok("=="),
        BinOpKind::NotEq => Ok("!="),
        BinOpKind::Lt => Ok("<"),
        BinOpKind::LtEq => Ok("<="),
        BinOpKind::Gt => Ok(">"),
        BinOpKind::GtEq => Ok(">="),
        // Logical `and`/`or` in general expression position (e.g. a `.filter`
        // closure body: `fn(x) => q == "" or x.lower().contains(q)`). Both
        // operands must lower to `bool`; rustc enforces that. Condition
        // position (`{#if}` / `if`-expr) is handled separately by
        // `lower_cond_expr`, which also does `!`.
        BinOpKind::And => Ok("&&"),
        BinOpKind::Or => Ok("||"),
        BinOpKind::Xor
        | BinOpKind::BitAnd
        | BinOpKind::BitOr
        | BinOpKind::BitXor
        | BinOpKind::Shl => Err(EmitError {
            message: "binary op — arithmetic (+/-/*//%), comparisons \
                      (==/!=/</<=/>/>=), and logical (and/or) supported on the \
                      client-WASM target; bitwise ops are deferred"
                .to_string(),
            context: "expression".to_string(),
        }),
        _ => Err(EmitError {
            message: "unsupported binary op — deferred to a later 11.7 slice".to_string(),
            context: "expression".to_string(),
        }),
    }
}

/// Lower a call expression (Phase 11.7 R3.5a.1).
///
/// - **Method calls** on a list receiver — `xs.map(fn(x) => ...)`,
///   `xs.filter(fn(x) => ...)`, `xs.len()` — lower to Rust iterator
///   chains. The receiver is snapshotted with `.clone().into_iter()`
///   (every value in the WASM target is `Clone`), so mutating the
///   original state field later in the same body is safe.
/// - **Free-function calls** (`cards_in(cards, "todo")`) are deferred
///   to R3.5a.2, where imported classic helpers get transpiled into the
///   bundle — until then there is nothing to call.
fn lower_call(
    callee: &Expr,
    args: &[Expr],
    state_names: &[String],
    locals: &[String],
) -> EmitResult<String> {
    match callee {
        // Phase 11.7 R3.5b.1 — `payload.has("key")` on the event
        // handler's `&HashMap<String, String>` param. Special-cased ahead
        // of the list-method dispatch (a bare `payload` ident is not a
        // state field / local, so the generic path would reject it).
        Expr::Field { object, field, .. }
            if field == "has"
                && args.len() == 1
                && matches!(object.as_ref(), Expr::Ident(n, _) if n == "payload") =>
        {
            let key = lower_expr(&args[0], state_names, locals)?;
            Ok(format!("payload.contains_key(&({key}))"))
        }
        Expr::Field { object, field, .. } => {
            let obj = lower_expr(object, state_names, locals)?;
            match (field.as_str(), args.len()) {
                ("len", 0) => Ok(format!("(({obj}).len() as i64)")),
                ("map", 1) => {
                    let (param, body) = extract_unary_closure(&args[0])?;
                    let mut inner = locals.to_vec();
                    inner.push(param.clone());
                    let body_rust = lower_expr(body, state_names, &inner)?;
                    // `.map` receives each element BY VALUE from
                    // `into_iter()`, matching the closure's owned param.
                    Ok(format!(
                        "({obj}).clone().into_iter().map(|{param}| {body_rust}).collect::<Vec<_>>()"
                    ))
                }
                ("filter", 1) => {
                    let (param, body) = extract_unary_closure(&args[0])?;
                    let mut inner = locals.to_vec();
                    inner.push(param.clone());
                    let body_rust = lower_expr(body, state_names, &inner)?;
                    // `.filter` hands the closure a `&T`; clone into an
                    // owned binding so the lowered body (which expects an
                    // owned param, e.g. `c.column == col`) type-checks.
                    Ok(format!(
                        "({obj}).clone().into_iter().filter(|__it| {{ let {param} = __it.clone(); {body_rust} }}).collect::<Vec<_>>()"
                    ))
                }
                // Str methods (parity with classic Fitz / the SSR target). The
                // receiver lowers to an owned `String`, so each maps to a `str`
                // method via Deref. Unblocks case-insensitive filters
                // (`x.lower().contains(q.lower())`) and similar on client-WASM.
                ("upper", 0) => Ok(format!("({obj}).to_uppercase()")),
                ("lower", 0) => Ok(format!("({obj}).to_lowercase()")),
                ("trim", 0) => Ok(format!("({obj}).trim().to_string()")),
                ("contains", 1) => {
                    let a = lower_expr(&args[0], state_names, locals)?;
                    Ok(format!("({obj}).contains(({a}).as_str())"))
                }
                ("starts_with", 1) => {
                    let a = lower_expr(&args[0], state_names, locals)?;
                    Ok(format!("({obj}).starts_with(({a}).as_str())"))
                }
                ("ends_with", 1) => {
                    let a = lower_expr(&args[0], state_names, locals)?;
                    Ok(format!("({obj}).ends_with(({a}).as_str())"))
                }
                ("replace", 2) => {
                    let a = lower_expr(&args[0], state_names, locals)?;
                    let b = lower_expr(&args[1], state_names, locals)?;
                    Ok(format!("({obj}).replace(({a}).as_str(), ({b}).as_str())"))
                }
                (other, n) => Err(EmitError {
                    message: format!(
                        "method `.{other}()` ({n} arg(s)) — the client-WASM target \
                         supports `.map`/`.filter`/`.len` on lists and \
                         `.upper`/`.lower`/`.trim`/`.contains`/`.starts_with`/\
                         `.ends_with`/`.replace` on strings; other methods \
                         (`.split`/`.to_int`, which return List/Result) defer to a \
                         later slice or the SSR target"
                    ),
                    context: "expression".to_string(),
                }),
            }
        }
        // CW.6 (dual-target) — `flv(x)` is the fitz-liveviews HTML-escaping
        // helper (`fn flv(s: Str) -> Str`). SSR needs it because it builds a
        // raw HTML string; on the client-WASM target a `create_text_node` /
        // `set_attribute` escapes intrinsically, so `flv` is the IDENTITY
        // here — pass the single arg through. This lets an SSR companion
        // component authored with `{flv(label)}` + `from fitz_liveviews
        // import flv` compile to `--target wasm-client` UNCHANGED (the import
        // line is already skipped by `load_imported_fns` — it is not a
        // sibling `.fitzv`). Output is byte-identical to writing `{label}`.
        Expr::Ident(name, _) if name == "flv" && args.len() == 1 => {
            lower_expr(&args[0], state_names, locals)
        }
        // CW.9 (1c) — the `Html` constructors `html(x)` / `raw_html(x)` in
        // VALUE position (a helper body, e.g. `icon` returning
        // `html("<svg>" + body + "</svg>")`). Both build a `__FlvHtml` via the
        // per-bundle shim (see `HTML_SHIM`). In interpolation position
        // `{raw_html(x)}` / `{html(x)}` is intercepted earlier by
        // `emit_interpolation` (the `set_inner_html` sink), so this arm only
        // fires inside expressions.
        Expr::Ident(name, _) if matches!(name.as_str(), "html" | "raw_html") && args.len() == 1 => {
            let arg = lower_expr(&args[0], state_names, locals)?;
            Ok(format!("{name}({arg})"))
        }
        // CW.6 (dual-target) — the `List<Html>`-folding framework helpers
        // (`h_join`/`h_when`/`h_either`) have no single-string form and no
        // client-WASM equivalent. A component using them stays SSR-only;
        // hard-error with a clear pointer rather than emitting a broken call.
        Expr::Ident(name, _) if matches!(name.as_str(), "h_join" | "h_when" | "h_either") => {
            Err(EmitError {
                message: format!(
                    "`{name}(...)` is an SSR-only fitz-liveviews helper (List<Html> \
                     folding) with no client-WASM equivalent. Keep this component on \
                     the SSR target, or restructure to avoid folding `Html` values."
                ),
                context: "expression".to_string(),
            })
        }
        // Phase 11.7 R3.5a.2 — a free-function call to an imported classic
        // helper (`cards_in(cards, "todo")`, `move_one(id, "right", c)`).
        // The helper is transpiled into the bundle by `emit_imported_fns`;
        // the view checker (K-4) already validated the name is imported,
        // so we trust it and emit the call. Bare-ident arguments are
        // `.clone()`d: a String/nominal argument captured by an enclosing
        // `.map`/`.filter` closure would otherwise be MOVED out of the
        // `FnMut` capture on the first call and fail to compile. Cloning
        // per call is the price for the WASM target's by-value discipline.
        Expr::Ident(name, _) => {
            let mut arg_rust: Vec<String> = Vec::with_capacity(args.len());
            for arg in args {
                let a = lower_expr(arg, state_names, locals)?;
                // `Expr::Field` already lowers with a trailing `.clone()`;
                // literals / nested calls produce owned values. Only a
                // bare ident needs an explicit clone.
                if matches!(arg, Expr::Ident(..)) {
                    arg_rust.push(format!("{a}.clone()"));
                } else {
                    arg_rust.push(a);
                }
            }
            Ok(format!("{name}({})", arg_rust.join(", ")))
        }
        _ => Err(EmitError {
            message: "unsupported call target on the client-WASM target — only method \
                      calls (`recv.method(...)`) and free-function calls are recognised"
                .to_string(),
            context: "expression".to_string(),
        }),
    }
}

/// Extract the single parameter name + body expression of an inline
/// unary closure `fn(x) => <expr>` (Phase 11.7 R3.5a.1). Used by
/// `.map`/`.filter` lowering. Only the arrow / single-expression form
/// is supported; multi-statement closure bodies and async closures
/// defer to a later 11.7 slice.
fn extract_unary_closure(arg: &Expr) -> EmitResult<(String, &Expr)> {
    let Expr::FnExpr {
        params,
        body,
        is_async,
        ..
    } = arg
    else {
        return Err(EmitError {
            message: "`.map`/`.filter` on the client-WASM target take an inline \
                      closure `fn(x) => <expr>`; a function reference or other \
                      argument is not yet supported (Phase 11.7 R3.5a.1)"
                .to_string(),
            context: "expression".to_string(),
        });
    };
    if *is_async {
        return Err(EmitError {
            message: "async closures are not supported inside `.map`/`.filter` on the \
                      client-WASM target"
                .to_string(),
            context: "expression".to_string(),
        });
    }
    if params.len() != 1 {
        return Err(EmitError {
            message: format!(
                "`.map`/`.filter` closures take exactly one parameter on the \
                 client-WASM target; got {}",
                params.len()
            ),
            context: "expression".to_string(),
        });
    }
    if body.len() == 1 {
        match &body[0] {
            Stmt::Return(e, _) | Stmt::Expr(e, _) => return Ok((params[0].name.clone(), e)),
            _ => {}
        }
    }
    Err(EmitError {
        message: "`.map`/`.filter` closures must be single-expression \
                  (`fn(x) => <expr>`) on the client-WASM target; multi-statement \
                  closure bodies defer to a later 11.7 slice"
            .to_string(),
        context: "expression".to_string(),
    })
}

/// Lower a single-expression block (the `then`/`else` arm of an
/// `if`-as-expression) to a Rust expression string (Phase 11.7
/// R3.5a.1). The arm must be exactly one `Stmt::Expr` or `Stmt::Return`.
fn lower_single_expr_block(
    stmts: &[Stmt],
    state_names: &[String],
    locals: &[String],
) -> EmitResult<String> {
    if stmts.len() == 1 {
        match &stmts[0] {
            Stmt::Expr(e, _) | Stmt::Return(e, _) => return lower_expr(e, state_names, locals),
            _ => {}
        }
    }
    Err(EmitError {
        message: "an `if` branch used as a value must be a single expression on the \
                  client-WASM target (Phase 11.7 R3.5a.1)"
            .to_string(),
        context: "expression".to_string(),
    })
}

/// Collect the names of locals that are REASSIGNED (`x = …` with `is_let =
/// false`, target a bare ident) anywhere in a statement list — including nested
/// `for` / `if` / `match` bodies. `lower_stmt` uses this to decide whether a
/// local declaration needs `let mut` (a string accumulator like `let out = ""`
/// reassigned inside a loop) vs a plain `let`, so bodies that never reassign
/// stay byte-identical.
fn collect_reassigned_locals(stmts: &[Stmt]) -> std::collections::HashSet<String> {
    fn walk_stmt(s: &Stmt, set: &mut std::collections::HashSet<String>) {
        match s {
            Stmt::Assign {
                target,
                value,
                is_let,
                ..
            } => {
                if !*is_let {
                    if let AssignTarget::Ident(name, _) = target {
                        set.insert(name.clone());
                    }
                }
                walk_expr(value, set);
            }
            Stmt::For { body, .. } => {
                for st in body {
                    walk_stmt(st, set);
                }
            }
            Stmt::Expr(e, _) | Stmt::Return(e, _) => walk_expr(e, set),
            _ => {}
        }
    }
    fn walk_expr(e: &Expr, set: &mut std::collections::HashSet<String>) {
        match e {
            Expr::If { then, else_, .. } => {
                for s in then {
                    walk_stmt(s, set);
                }
                if let Some(els) = else_ {
                    for s in els {
                        walk_stmt(s, set);
                    }
                }
            }
            Expr::Match { arms, .. } => {
                for arm in arms {
                    for s in &arm.body {
                        walk_stmt(s, set);
                    }
                }
            }
            _ => {}
        }
    }
    let mut set = std::collections::HashSet::new();
    for s in stmts {
        walk_stmt(s, &mut set);
    }
    set
}

/// Lower a `match` [`Pattern`] to a Rust pattern string. Supports the literal
/// patterns (Int / Float / Str / Bool), an ident binding, and `_`. When a Str
/// pattern is present the caller matches on `scrutinee.as_str()` so the `"…"`
/// patterns type-check against `&str`.
fn lower_pattern_wasm(pat: &crate::ast::Pattern) -> EmitResult<String> {
    use crate::ast::Pattern;
    match pat {
        Pattern::Int(n) => Ok(format!("{n}i64")),
        Pattern::Float(f) => Ok(format!("{f}f64")),
        Pattern::Str(s) => Ok(format!("{s:?}")),
        Pattern::Bool(b) => Ok(b.to_string()),
        Pattern::Ident(name, _) => Ok(name.clone()),
        Pattern::Wildcard => Ok("_".to_string()),
        // CW.9 (1a) — Result arms, so a helper's `Result<T>` can be matched:
        // `match parse(s) { Ok(v) => v, Err(_) => 0 }`.
        Pattern::OkBinding(name, _) => Ok(format!("Ok({name})")),
        Pattern::ErrBinding(name, _) => Ok(format!("Err({name})")),
        Pattern::OkWildcard => Ok("Ok(_)".to_string()),
        Pattern::ErrWildcard => Ok("Err(_)".to_string()),
        _ => Err(EmitError {
            message: "`match` pattern — the client-WASM target supports literal patterns \
                      (Int/Float/Str/Bool), an ident binding, `_`, and Result arms \
                      (`Ok(x)` / `Err(e)` / `Ok(_)` / `Err(_)`)"
                .to_string(),
            context: "match pattern".to_string(),
        }),
    }
}

/// The variable names a `match` pattern binds, so the arm body can see them.
/// `Ok(v)` / `Err(e)` bind their inner; a bare `Ident` binds itself; literal
/// and wildcard patterns bind nothing. (CW.9 1a — before this, an arm's bound
/// var was never threaded into the arm body's scope, so binding patterns
/// effectively couldn't be used on the wasm target.)
fn pattern_bindings(pat: &crate::ast::Pattern) -> Vec<String> {
    use crate::ast::Pattern;
    match pat {
        Pattern::Ident(n, _) | Pattern::OkBinding(n, _) | Pattern::ErrBinding(n, _) => {
            vec![n.clone()]
        }
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Interpolated child prop lowering (Phase 11.7.a)
// ---------------------------------------------------------------------------

/// True when `ty` is a bare primitive (`Int`/`Float`/`Str`/`Bool`).
///
/// Phase 11.7.a restricts interpolated props on the client-WASM target
/// to primitive child fields so the emitted read is a plain `.clone()`
/// with no risk of a Rust type mismatch. Nullable / list / map /
/// nominal targets propagate via the SSR backend today; their WASM
/// path (which needs richer reactive-propagation plumbing) lands in a
/// later 11.7 slice.
fn is_wasm_prop_simple_target(ty: &TypeExpr) -> bool {
    matches!(ty, TypeExpr::Named(n) if matches!(n.as_str(), "Int" | "Float" | "Str" | "Bool"))
}

/// Lower an interpolated child prop (`<Child field={expr} />`) into a
/// Rust expression assignable to the child's `RefCell<T>` state field,
/// reading the PARENT's state where the expression references it.
///
/// **Phase 11.7.a scope (R1 — the "simple case")**: the prop
/// expression is either a bare parent state-field reference (`{title}`)
/// or numeric arithmetic over parent state fields (`{n + 1}`), and the
/// child field's declared type is a bare primitive. Under the
/// dirty-flag reactivity model the parent re-renders (and recreates
/// children) on every state change, so the child receives a
/// freshly-computed prop value each render — reactive propagation
/// falls out of the naive re-render for free. Persistent child state
/// (keyed instance cache, so the child is NOT recreated) is the R2
/// work (11.7.e).
///
/// Richer shapes (nullable / nominal / list targets, method calls, Str
/// concat, field access, imported names) reject with a clear pointer
/// to a later 11.7 slice / the SSR target.
fn lower_child_prop_value(
    expr: &Expr,
    field_name: &str,
    target_type: &TypeExpr,
    state_names: &[String],
    locals: &[String],
) -> EmitResult<String> {
    if !is_wasm_prop_simple_target(target_type) {
        return Err(EmitError {
            message: format!(
                "interpolated prop `{field_name}={{...}}` targets a \
                 non-primitive / nullable field — the client-WASM target \
                 supports interpolated props into bare `Int`/`Float`/`Str`/`Bool` \
                 fields in Phase 11.7.a; nominal / list / map / nullable prop \
                 propagation lands in a later 11.7 slice. For the SSR target \
                 (fitz-liveviews) these work today. Workaround: a static value \
                 or the SSR backend."
            ),
            context: "interpolated child prop".to_string(),
        });
    }
    match expr {
        // Bare parent state-field ref — the dominant case (`{title}`,
        // `{count}`). `(*self.<name>.borrow()).clone()` works uniformly
        // for every primitive (i64/f64/String/bool all impl Clone). The
        // checker (S.3 `light_check_interpolated_prop`) already verified
        // the parent field's type matches the child field's type.
        // Loop-var ref (`{#for c in ...}<Child prop="{c}" />`) — the
        // local is an owned primitive value; clone it for the child's
        // `RefCell<T>` assignment. Phase 11.7.b.
        Expr::Ident(name, _) if locals.iter().any(|s| s == name) => Ok(format!("{name}.clone()")),
        Expr::Ident(name, _) if state_names.iter().any(|s| s == name) => {
            Ok(format!("(*self.{name}.borrow()).clone()"))
        }
        // Numeric literals + arithmetic over parent state fields
        // (`{n + 1}`). Only for numeric child targets — reusing the
        // event-body lowerer produces Copy values (no clone needed).
        Expr::Int(..) | Expr::Float(..) | Expr::BinOp { .. } if matches!(target_type, TypeExpr::Named(n) if n == "Int" || n == "Float") => {
            lower_expr(expr, state_names, locals).map_err(|_| EmitError {
                message: format!(
                    "interpolated prop `{field_name}={{...}}` — only bare parent \
                     state fields and numeric arithmetic over them are supported \
                     on the client-WASM target in Phase 11.7.a; richer expressions \
                     (Str concat, method calls, imported names) land \
                     in a later 11.7 slice or the SSR target."
                ),
                context: "interpolated child prop".to_string(),
            })
        }
        // Phase 11.7 R3 — field access on a `{#for c in cards}` loop
        // var of nominal type (`<Card title="{c.title}" n="{c.id}" />`).
        // The target is a primitive (top gate) and the nominal field is
        // a matching primitive, so `c.title.clone()` drops straight into
        // the child's `RefCell<T>`. This is how a nominal list item
        // fans its fields out into a keyed child's primitive props.
        Expr::Field { .. } => lower_expr(expr, state_names, locals).map_err(|mut e| {
            e.context = "interpolated child prop".to_string();
            e
        }),
        _ => Err(EmitError {
            message: format!(
                "interpolated prop `{field_name}={{...}}` — expression kind `{}` \
                 is not supported on the client-WASM target in Phase 11.7.a (bare \
                 parent state field or numeric arithmetic into a numeric field \
                 only). Richer shapes land in a later 11.7 slice or the SSR \
                 target.",
                expr_kind_name(expr)
            ),
            context: "interpolated child prop".to_string(),
        }),
    }
}

/// Lower a Fitz `Stmt` to Rust statements appended to `out` with the
/// given `indent` prefix.
///
/// `locals` carries the in-scope local bindings accumulated by earlier
/// `let`-style statements in the same body (Fitz does not distinguish
/// `let x = ...` from `x = ...` at the AST level — an `Assign` whose
/// target names something OTHER than a state field is a local binding).
/// It is threaded mutably so a later statement can reference a name a
/// prior one introduced, and cloned for nested scopes (`{#if}` arms) so
/// branch-local names don't leak.
///
/// Supported (Phase 11.7 R3.5a.1):
/// - `Stmt::Assign` to a state field → `*self.<f>.borrow_mut() = <rhs>`
///   (covers list reassignment `cards = cards.filter(...)`).
/// - `Stmt::Assign` to a non-state ident → `let <name> = <rhs>` local.
/// - `Stmt::Expr(Expr::If ...)` → statement-position guard.
/// - `Stmt::Expr(Expr::Call ...)` → `<state_list>.push/clear`.
/// - `Stmt::Return` → `return <expr>` (used by transpiled helpers in
///   R3.5a.2; harmless in event bodies, which never return).
fn lower_stmt(
    stmt: &Stmt,
    state_names: &[String],
    locals: &mut Vec<String>,
    indent: &str,
    reassigned: &std::collections::HashSet<String>,
    out: &mut String,
) -> EmitResult<()> {
    match stmt {
        Stmt::Assign {
            target,
            value,
            is_let,
            ..
        } => match target {
            AssignTarget::Ident(name, _) => {
                if state_names.iter().any(|s| s == name) {
                    // Reassign a state field. The RHS is fully evaluated
                    // into `__rhs` first (dropping any read-borrow of the
                    // same field, e.g. `cards = cards.filter(...)`) before
                    // the `borrow_mut()`, so there is no double-borrow.
                    let rhs = lower_expr(value, state_names, locals)?;
                    writeln!(out, "{}let __rhs = {};", indent, rhs).unwrap();
                    writeln!(out, "{}*self.{}.borrow_mut() = __rhs;", indent, name).unwrap();
                } else if *is_let {
                    // A local declaration (`let x = …`). Emit `let mut` when the
                    // name is reassigned later in this body (a loop accumulator
                    // like `let out = ""`), else a plain `let` — so bodies that
                    // never reassign stay byte-identical.
                    let rhs = lower_expr(value, state_names, locals)?;
                    let mut_kw = if reassigned.contains(name) {
                        "mut "
                    } else {
                        ""
                    };
                    writeln!(out, "{}let {}{} = {};", indent, mut_kw, name, rhs).unwrap();
                    if !locals.iter().any(|s| s == name) {
                        locals.push(name.clone());
                    }
                } else {
                    // Reassignment to an existing local (`out = out + …`).
                    let rhs = lower_expr(value, state_names, locals)?;
                    writeln!(out, "{}{} = {};", indent, name, rhs).unwrap();
                }
                Ok(())
            }
            AssignTarget::Field { .. } => Err(EmitError {
                message: "assign to field (`obj.field = ...`) — deferred to a later \
                          11.7 slice"
                    .to_string(),
                context: "statement".to_string(),
            }),
            AssignTarget::Index { .. } => Err(EmitError {
                message: "assign to index (`xs[i] = ...`) — deferred to a later 11.7 slice"
                    .to_string(),
                context: "statement".to_string(),
            }),
        },
        // A range `for` loop (`for n in 1..(max+1) { … }`) — used by
        // transpiled helper string builders. Only a bare loop var + a
        // `Range` iterable on the wasm target.
        Stmt::For {
            var, iter, body, ..
        } => {
            let var_name = match var {
                crate::ast::Pattern::Ident(n, _) => n.clone(),
                _ => {
                    return Err(EmitError {
                        message: "`for` loop var — the client-WASM target supports a bare \
                                  identifier (`for n in …`)"
                            .to_string(),
                        context: "statement".to_string(),
                    })
                }
            };
            // The loop header: a range (`for n in a..b`) or a list
            // (`for b in bars` — CW.9, e.g. `bar_scale`'s max scan). A list is
            // iterated by cloned value (the loop var is owned, elements impl
            // `Clone` under R3) so the list stays available afterwards — e.g.
            // `bar_scale` does `bars.map(...)` after the loop.
            match iter {
                Expr::Range {
                    start,
                    end,
                    inclusive,
                    ..
                } => {
                    let s = lower_expr(start, state_names, locals)?;
                    let e = lower_expr(end, state_names, locals)?;
                    let op = if *inclusive { "..=" } else { ".." };
                    writeln!(out, "{}for {} in {}{}{} {{", indent, var_name, s, op, e).unwrap();
                }
                _ => {
                    let it = lower_expr(iter, state_names, locals)?;
                    writeln!(
                        out,
                        "{}for {} in ({}).iter().cloned() {{",
                        indent, var_name, it
                    )
                    .unwrap();
                }
            }
            let inner_indent = format!("{indent}    ");
            let mut body_locals = locals.clone();
            if !body_locals.iter().any(|l| l == &var_name) {
                body_locals.push(var_name.clone());
            }
            for st in body {
                lower_stmt(
                    st,
                    state_names,
                    &mut body_locals,
                    &inner_indent,
                    reassigned,
                    out,
                )?;
            }
            writeln!(out, "{}}}", indent).unwrap();
            Ok(())
        }
        // Phase 11.7 R3.5a.1 — `return <expr>` (transpiled helper bodies
        // in R3.5a.2 use it; event bodies never return, so this arm is
        // dormant there).
        Stmt::Return(e, _) => {
            let rhs = lower_expr(e, state_names, locals)?;
            writeln!(out, "{}return {};", indent, rhs).unwrap();
            Ok(())
        }
        Stmt::Expr(inner, _) => {
            lower_expr_stmt(inner, state_names, locals, indent, reassigned, out)
        }
        _ => Err(EmitError {
            message: "statement kind — supported on the client-WASM target: state-field \
                      reassignment, local `let` / reassignment, `for` range loop, `if` \
                      guard, `<state_list>.push/clear`, and `return`"
                .to_string(),
            context: "statement".to_string(),
        }),
    }
}

/// Lower an expression used in statement position (Phase 11.7 R3.5a.1).
/// Handles a statement-`if` guard and `<state_list>.push/clear`.
///
/// `locals` is read-only here — a statement-`if` opens fresh sub-scopes
/// (cloned via `to_vec`) for its arms, so branch-local `let`s never leak
/// into the enclosing body.
fn lower_expr_stmt(
    expr: &Expr,
    state_names: &[String],
    locals: &[String],
    indent: &str,
    reassigned: &std::collections::HashSet<String>,
    out: &mut String,
) -> EmitResult<()> {
    match expr {
        // Statement-position `if` guard: `if (cond) { <stmts> }` with an
        // optional `else`. Each arm is a fresh sub-scope (locals cloned)
        // so branch-local `let`s don't leak.
        Expr::If {
            condition,
            then,
            else_,
            ..
        } => {
            let cond = lower_cond_expr(condition, state_names, locals)?;
            writeln!(out, "{}if {} {{", indent, cond).unwrap();
            let inner_indent = format!("{indent}    ");
            let mut then_locals = locals.to_vec();
            for s in then {
                lower_stmt(
                    s,
                    state_names,
                    &mut then_locals,
                    &inner_indent,
                    reassigned,
                    out,
                )?;
            }
            if let Some(else_stmts) = else_ {
                writeln!(out, "{}}} else {{", indent).unwrap();
                let mut else_locals = locals.to_vec();
                for s in else_stmts {
                    lower_stmt(
                        s,
                        state_names,
                        &mut else_locals,
                        &inner_indent,
                        reassigned,
                        out,
                    )?;
                }
            }
            writeln!(out, "{}}}", indent).unwrap();
            Ok(())
        }
        // List mutation on a state `List<...>` field: `cards.push(...)` /
        // `cards.clear()`. This is what makes keys appear / vanish at
        // runtime, so reconciliation runs live.
        Expr::Call { callee, args, .. } => {
            if let Expr::Field { object, field, .. } = callee.as_ref() {
                if let Expr::Ident(list_name, _) = object.as_ref() {
                    if state_names.iter().any(|s| s == list_name) {
                        match (field.as_str(), args.len()) {
                            ("push", 1) => {
                                let arg = lower_expr(&args[0], state_names, locals)?;
                                writeln!(out, "{indent}self.{list_name}.borrow_mut().push({arg});")
                                    .unwrap();
                                return Ok(());
                            }
                            ("clear", 0) => {
                                writeln!(out, "{indent}self.{list_name}.borrow_mut().clear();")
                                    .unwrap();
                                return Ok(());
                            }
                            _ => {}
                        }
                    }
                }
            }
            Err(EmitError {
                message: "method-call statement — only `<state_list>.push(<expr>)` and \
                          `<state_list>.clear()` are supported in statement position on \
                          the client-WASM target. Transform-then-reassign \
                          (`xs = xs.map(...)`) is an assignment, not a bare call."
                    .to_string(),
                context: "statement".to_string(),
            })
        }
        _ => Err(EmitError {
            message: "expression-statement kind — only an `if` guard or \
                      `<state_list>.push/clear` is supported in statement position on \
                      the client-WASM target"
                .to_string(),
            context: "statement".to_string(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Type + default mapping
// ---------------------------------------------------------------------------

/// Map a Fitz `TypeExpr` to the corresponding Rust type string used
/// inside `RefCell<...>` for state fields.
///
/// Phase 11.5.d extended this from Int-only (11.4.b) to the four
/// primitive scalars + `Nullable<T>` of a primitive. K-3 sums
/// `List<T>` (mapped to `Vec<Rust-T>`) for compound props. The set
/// matches `check::coerce_child_prop_raw_value` so any type that
/// flows through `<Child prop="v" />` composition can also be
/// declared as a state field on the child.
///
/// `Map<K, V>` and nominal types still deferred — they need cell
/// layout decisions that overlap with the reflow story (Phase
/// 11.6+).
fn type_expr_to_rust(ty: &TypeExpr, nominals: &NominalRegistry) -> EmitResult<String> {
    match ty {
        TypeExpr::Named(name) => match name.as_str() {
            "Int" => Ok("i64".to_string()),
            "Float" => Ok("f64".to_string()),
            "Bool" => Ok("bool".to_string()),
            "Str" => Ok("String".to_string()),
            // CW.9 (1c) — fitz-liveviews's `Html` newtype maps to the
            // per-bundle `__FlvHtml` shim (a struct over the raw markup
            // string), so a helper returning `Html` (e.g. `icon`) transpiles
            // and `.raw` field access yields the underlying String.
            "Html" => Ok("__FlvHtml".to_string()),
            // Phase 11.7 R3 — an imported classic nominal maps to the
            // Rust struct the emitter synthesises for it (same name).
            // Requires the `from foo import Foo` sibling to have been
            // loaded into the registry (`load_imported_nominals`).
            other if nominals.contains(other) => Ok(other.to_string()),
            other => Err(EmitError {
                message: format!(
                    "state field type `{other}` — only `Int`/`Float`/`Bool`/`Str` \
                     (and their `Nullable<T>` / `List<...>` / `Map<...>` wrappers) \
                     supported for primitives; nominal types must be imported from a \
                     sibling `.fitz` (e.g. `from card import {other}`) so the WASM \
                     emitter can load their fields (Phase 11.7 R3)"
                ),
                context: "type".to_string(),
            }),
        },
        TypeExpr::Nullable(inner) => {
            let inner_rust = type_expr_to_rust(inner, nominals)?;
            Ok(format!("Option<{inner_rust}>"))
        }
        TypeExpr::Generic { name, args } => {
            if name == "List" && args.len() == 1 {
                // K-3: List<primitive> for WASM state fields, emitted as
                // `Vec<Rust>`. Recurses so `List<Nullable<Int>>` works
                // symmetrically with the SSR + check helpers. R3:
                // `List<Card>` resolves via the nominal registry.
                let inner_rust = type_expr_to_rust(&args[0], nominals)?;
                Ok(format!("Vec<{inner_rust}>"))
            } else if name == "Map" && args.len() == 2 {
                // S.2 (2026-07-17): Map<K, V> for WASM state fields
                // emitted as `Vec<(K_rust, V_rust)>` — mirrors Fitz's
                // Rc<RefCell<Vec<(K, V)>>> representation. Static prop
                // coercion (check.rs) is restricted to Map<Str, Str>;
                // richer maps land via interpolation (K-3 remainder).
                let k_rust = type_expr_to_rust(&args[0], nominals)?;
                let v_rust = type_expr_to_rust(&args[1], nominals)?;
                Ok(format!("Vec<({k_rust}, {v_rust})>"))
            } else if name == "Result" && args.len() == 1 {
                // CW.9 (1a) — `Result<T>` as an imported helper-fn return
                // type maps to Rust `Result<T, String>` (Err pinned to
                // String, matching classic Fitz + the @rpc stub). This lets
                // a helper propagate failures with `?` and a caller `match`
                // its Ok/Err arms. A `Result` STATE field stays unusable
                // (no default lowering), so this is effectively
                // return-type-only.
                let inner_rust = type_expr_to_rust(&args[0], nominals)?;
                Ok(format!("Result<{inner_rust}, String>"))
            } else {
                Err(EmitError {
                    message: format!(
                        "state field type `{name}<...>` — other compound types \
                         deferred to Phase 11.7+"
                    ),
                    context: "type".to_string(),
                })
            }
        }
        TypeExpr::Tuple(_) | TypeExpr::Function { .. } => Err(EmitError {
            message: "tuple / function state field type — deferred to Phase 11.6+".to_string(),
            context: "type".to_string(),
        }),
    }
}

/// Lower a default expression to Rust source code, cross-checked
/// against the declared type of the field so we can emit the right
/// suffix (`0i64` for Int). POC only accepts Int defaults.
/// Emit the Rust literal for a state field's `default` expression,
/// checked against the field's declared type. Phase 11.5.d extended
/// beyond `Int` to the four primitive scalars + `Nullable<T>`, so
/// that any child-component composition target (`<Child /`>) can
/// declare its state with the same primitives that flow through
/// props.
///
/// - `Int` field ← `Expr::Int(n)`     → `<n>i64`
/// - `Float` field ← `Expr::Float(f)` → `<f>f64`
/// - `Bool` field ← `Expr::Bool(b)`   → `true`/`false`
/// - `Str` field ← `Expr::Str(s)`     → `"...".to_string()`
/// - `Nullable<T>` field ← `Expr::Null` → `None`
/// - `Nullable<T>` field ← non-null   → `Some(<inner literal>)`
/// - `Nullable<T>` field ← `Expr::Ident("null")` (legacy view sugar)
///   → `None`. The view parser today emits `Expr::Null` for the
///   `null` keyword, but we accept the ident form defensively so a
///   pre-lang-refresh checkpoint doesn't regress.
/// - `List<T>` field ← `Expr::List(items)` → empty list yields
///   `Vec::new()`; otherwise each item recurses via the same
///   helper so `default = [1, 2]` on a `List<Int>` field emits
///   `vec![1i64, 2i64]` (K-3, MVP: item must itself be a literal
///   of the inner primitive type).
///
/// Non-literal defaults (function calls, arithmetic, etc.) still
/// error — they need the classic-Fitz expression lowering which
/// belongs to a future emitter refactor.
fn default_expr_to_rust(default: &Expr, ty: &TypeExpr) -> EmitResult<String> {
    match (default, ty) {
        (Expr::Int(n, _), TypeExpr::Named(name)) if name == "Int" => Ok(format!("{n}i64")),
        (Expr::Float(f, _), TypeExpr::Named(name)) if name == "Float" => Ok(format!("{f}f64")),
        (Expr::Bool(b, _), TypeExpr::Named(name)) if name == "Bool" => Ok(b.to_string()),
        (Expr::Str(s, _), TypeExpr::Named(name)) if name == "Str" => {
            Ok(format!("{s:?}.to_string()"))
        }
        // Negative numeric default: `-1` / `-3.5` parse as a `UnaryOp{Neg, …}`
        // over an Int/Float literal, not a bare literal. Emit the negated Rust
        // literal (e.g. `Spinner`'s `progress: Int = -1`).
        (
            Expr::UnaryOp {
                op: crate::ast::UnaryOpKind::Neg,
                operand,
                ..
            },
            TypeExpr::Named(name),
        ) => match (operand.as_ref(), name.as_str()) {
            (Expr::Int(n, _), "Int") => Ok(format!("-{n}i64")),
            (Expr::Float(f, _), "Float") => Ok(format!("-{f}f64")),
            _ => Err(EmitError {
                message: format!(
                    "negated default is only supported for `Int` / `Float` literals \
                     (field type `{name}`)"
                ),
                context: "state field default".to_string(),
            }),
        },
        // Nullable dispatch: `null` → None, else recurse into inner.
        (Expr::Null(_), TypeExpr::Nullable(_)) => Ok("None".to_string()),
        (_, TypeExpr::Nullable(inner)) => {
            let inner_rust = default_expr_to_rust(default, inner)?;
            Ok(format!("Some({inner_rust})"))
        }
        // K-3: List<primitive> defaults. Accepts `Expr::List(items, _)`
        // against `List<T>` where T is a supported primitive (or
        // `Nullable<primitive>`). Empty list emits `Vec::new()`; non-empty
        // recurses per-item so `default = [1, 2, 3]` on a `List<Int>`
        // field emits `vec![1i64, 2i64, 3i64]`.
        (Expr::List(items, _), TypeExpr::Generic { name, args })
            if name == "List" && args.len() == 1 =>
        {
            if items.is_empty() {
                return Ok("Vec::new()".to_string());
            }
            let inner_ty = &args[0];
            let mut lits: Vec<String> = Vec::with_capacity(items.len());
            for item in items {
                lits.push(default_expr_to_rust(item, inner_ty)?);
            }
            Ok(format!("vec![{}]", lits.join(", ")))
        }
        // S.2 (2026-07-17): Map<K, V> defaults. Accepts `Expr::Map(entries, _)`
        // against `Map<K, V>` where K + V are supported primitives.
        // Empty map emits `Vec::new()` (matches the `Vec<(K, V)>` state
        // shape); non-empty recurses per-pair.
        (Expr::Map(entries, _), TypeExpr::Generic { name, args })
            if name == "Map" && args.len() == 2 =>
        {
            if entries.is_empty() {
                return Ok("Vec::new()".to_string());
            }
            let (k_ty, v_ty) = (&args[0], &args[1]);
            let mut lits: Vec<String> = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                let k_lit = default_expr_to_rust(k, k_ty)?;
                let v_lit = default_expr_to_rust(v, v_ty)?;
                lits.push(format!("({k_lit}, {v_lit})"));
            }
            Ok(format!("vec![{}]", lits.join(", ")))
        }
        // Everything else: not a literal, or a mismatch. The classic
        // checker catches literal-vs-type mismatch already; here we
        // reject with a generic message pointing at the composition
        // story.
        _ => Err(EmitError {
            message: "default expression for state field — must be a literal of the \
                     declared primitive type (`Int`/`Float`/`Bool`/`Str`) or `null` \
                     for a `Nullable<T>` field. Non-literal defaults deferred to \
                     Phase 11.6+."
                .to_string(),
            context: "default".to_string(),
        }),
    }
}

// ---------------------------------------------------------------------------
// String helpers
// ---------------------------------------------------------------------------

/// Emit a Rust string literal for the given `&str`. Uses `{:?}`
/// formatting which produces a valid Rust string literal with the
/// necessary escapes (quotes, backslashes, control chars). Good
/// enough for the POC (CSS bodies, static attr values, text nodes,
/// tag names — none of which are user-controlled at the browser
/// runtime level).
fn rust_string_literal(s: &str) -> String {
    format!("{:?}", s)
}

/// Escape a literal string so it can be embedded verbatim inside the format
/// string of a `format!("…")` call: backslash / double-quote for the Rust
/// string literal, and `{` / `}` doubled so `format!` treats them as literal
/// braces. Used by the mixed attribute interpolation emit (`style="width:
/// {pct}%"` → `format!("width: {}%", …)`).
fn escape_for_rust_format(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '{' => out.push_str("{{"),
            '}' => out.push_str("}}"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

/// Sanitize a name so it becomes a valid Rust identifier. Replaces
/// hyphens with underscores (the only non-ident char that appears
/// in `scope_class` post-kebab-case) and drops everything else that
/// isn't alphanumeric or underscore.
fn sanitize_ident(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else if ch == '-' {
            out.push('_');
        }
        // else: drop silently — happens for e.g. `.` if any appears
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{expand, parse, ExpandedStateField};

    /// Helper: parse + expand a `.fitzv` source into an
    /// `ExpandedViewFile`, panicking with the error's Display if
    /// either step fails. Keeps the test bodies focused on the
    /// emit output being validated.
    fn parse_expand(src: &str) -> ExpandedViewFile {
        let raw = parse(src).expect("parse succeeded");
        expand(&raw).expect("expand succeeded")
    }

    /// Simplified counter-shape source used for tests that only need
    /// the pipeline to produce a component with state Int + event
    /// handlers + template with @click + scoped style. Event bodies
    /// assign Int literals (not arithmetic) because the view lexer
    /// does not yet tokenise `+`/`-`/`*`/`/`/`%` (deuda documented in
    /// §9.m of `docs/fase-11-plan.md` — must close before 11.4.c
    /// browser smoke). Tests that specifically validate arithmetic
    /// lowering (e.g. `arithmetic_body_lowering_binop_add`) build
    /// the `ExpandedComponent` directly to bypass this lexer gap.
    fn counter_shape_src() -> &'static str {
        r#"component Counter {
  state { count: Int = 0 }
  event increment() { count = 42 }
  event decrement() { count = 0 }

  <template>
    <div class="counter">
      <button @click="decrement">reset</button>
      <span>{count}</span>
      <button @click="increment">bump</button>
    </div>
  </template>

  <style scoped>
    .counter { display: flex; gap: 8px; }
  </style>
}"#
    }

    // ---- Struct + new -------------------------------------------

    #[test]
    fn emit_struct_has_pub_struct_and_state_field() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("pub struct Counter {"),
            "struct decl:\n{}",
            out
        );
        assert!(
            out.contains("count: RefCell<i64>,"),
            "state field:\n{}",
            out
        );
        assert!(
            out.contains("root: RefCell<Option<HtmlElement>>,"),
            "root field:\n{}",
            out
        );
    }

    #[test]
    fn emit_new_returns_rc_self_with_defaults() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("pub fn new() -> Rc<Self> {"),
            "new() signature:\n{}",
            out
        );
        assert!(out.contains("Rc::new(Counter {"), "Rc::new call:\n{}", out);
        assert!(
            out.contains("count: RefCell::new(0i64),"),
            "default:\n{}",
            out
        );
        assert!(
            out.contains("root: RefCell::new(None),"),
            "root default:\n{}",
            out
        );
    }

    // ---- Event handlers -----------------------------------------

    #[test]
    fn emit_event_handler_lowers_assign_and_calls_render() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("fn increment(self: &Rc<Self>) {"),
            "handler signature:\n{}",
            out
        );
        assert!(
            out.contains("let __rhs = 42i64;"),
            "RHS lowering (literal Int):\n{}",
            out
        );
        assert!(
            out.contains("*self.count.borrow_mut() = __rhs;"),
            "assign:\n{}",
            out
        );
        assert!(out.contains("self.render();"), "auto-render call:\n{}", out);
    }

    #[test]
    fn emit_event_handler_decrement_uses_literal_zero() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("fn decrement(self: &Rc<Self>) {"),
            "decrement signature:\n{}",
            out
        );
        assert!(
            out.contains("let __rhs = 0i64;"),
            "reset lowering (literal Int 0):\n{}",
            out
        );
    }

    /// Arithmetic body (`count = count + 1`) is exactly what the
    /// counter demo of 11.4.c will need. The view lexer does NOT
    /// yet tokenise `+`/`-`/`*`/`/`/`%` so we cannot exercise it
    /// through parse→expand today. Build the `ExpandedComponent`
    /// directly to validate the emitter's arithmetic lowering
    /// path — the deuda that gates 11.4.c is separate (view
    /// lexer), documented in §9.m of `docs/fase-11-plan.md`.
    #[test]
    fn arithmetic_body_lowering_binop_add_direct() {
        use crate::ast::{Span, Stmt};
        use crate::view::ast::Loc;
        let loc = Loc { line: 1, column: 1 };
        let component = ExpandedComponent {
            name: "Counter".to_string(),
            loc,
            hydrate: false,
            state: vec![ExpandedStateField {
                name: "count".to_string(),
                type_expr: TypeExpr::Named("Int".to_string()),
                default: Expr::Int(0, Span::default()),
                loc,
            }],
            derived: Vec::new(),
            events: vec![ExpandedEventHandler {
                name: "increment".to_string(),
                params: vec![],
                body: vec![Stmt::Assign {
                    target: AssignTarget::Ident("count".to_string(), Span::default()),
                    type_: None,
                    value: Expr::BinOp {
                        op: BinOpKind::Add,
                        left: Box::new(Expr::Ident("count".to_string(), Span::default())),
                        right: Box::new(Expr::Int(1, Span::default())),
                        span: Span::default(),
                    },
                    is_let: true,
                    span: Span::default(),
                }],
                loc,
            }],
            template: None,
            style: None,
        };
        let out = emit_component(&component).unwrap();
        assert!(
            out.contains("let __rhs = ((*self.count.borrow()) + 1i64);"),
            "arithmetic lowering (Ident + Int):\n{}",
            out
        );
        assert!(
            out.contains("*self.count.borrow_mut() = __rhs;"),
            "assign target:\n{}",
            out
        );
        assert!(out.contains("self.render();"), "auto-render:\n{}", out);
    }

    /// End-to-end pipeline test: after the view-lexer arithmetic
    /// follow-up (§9.n of `docs/fase-11-plan.md`), a full counter
    /// source with `count = count + 1` parses, expands, AND emits
    /// the expected WASM lowering. Distinct from the direct-AST
    /// tests above — this proves the whole path is unblocked, not
    /// just the emitter in isolation.
    #[test]
    fn arithmetic_body_lowering_end_to_end_via_parse_expand() {
        let src = r#"component Counter {
  state { count: Int = 0 }
  event increment() { count = count + 1 }
  event decrement() { count = count - 1 }

  <template>
    <div><span>{count}</span></div>
  </template>
}"#;
        let expanded = parse_expand(src);
        let out = emit_component(&expanded.components[0]).unwrap();
        // The full pipeline should produce the exact same lowering
        // as the direct-AST arithmetic tests.
        assert!(
            out.contains("let __rhs = ((*self.count.borrow()) + 1i64);"),
            "increment arithmetic lowering via full pipeline:\n{}",
            out
        );
        assert!(
            out.contains("let __rhs = ((*self.count.borrow()) - 1i64);"),
            "decrement arithmetic lowering via full pipeline:\n{}",
            out
        );
        assert!(
            out.contains(r#"= format!("{}", (*self.count.borrow()));"#),
            "state interpolation in template:\n{}",
            out
        );
    }

    #[test]
    fn arithmetic_body_lowering_binop_sub_direct() {
        use crate::ast::{Span, Stmt};
        use crate::view::ast::Loc;
        let loc = Loc { line: 1, column: 1 };
        let component = ExpandedComponent {
            name: "Counter".to_string(),
            loc,
            hydrate: false,
            state: vec![ExpandedStateField {
                name: "count".to_string(),
                type_expr: TypeExpr::Named("Int".to_string()),
                default: Expr::Int(0, Span::default()),
                loc,
            }],
            derived: Vec::new(),
            events: vec![ExpandedEventHandler {
                name: "decrement".to_string(),
                params: vec![],
                body: vec![Stmt::Assign {
                    target: AssignTarget::Ident("count".to_string(), Span::default()),
                    type_: None,
                    value: Expr::BinOp {
                        op: BinOpKind::Sub,
                        left: Box::new(Expr::Ident("count".to_string(), Span::default())),
                        right: Box::new(Expr::Int(1, Span::default())),
                        span: Span::default(),
                    },
                    is_let: true,
                    span: Span::default(),
                }],
                loc,
            }],
            template: None,
            style: None,
        };
        let out = emit_component(&component).unwrap();
        assert!(
            out.contains("let __rhs = ((*self.count.borrow()) - 1i64);"),
            "subtract lowering:\n{}",
            out
        );
    }

    // ---- Mount + render -----------------------------------------

    #[test]
    fn emit_mount_signature_and_style_helper_call() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("pub fn mount(self: &Rc<Self>, selector: &str) -> Result<(), JsValue> {"),
            "mount signature:\n{}",
            out
        );
        // Scoped style → helper name contains `_counter_c_` (kebab + hex)
        assert!(
            out.contains("__inject_style_Counter_counter_c_"),
            "style helper call:\n{}",
            out
        );
        assert!(
            out.contains(".dyn_into::<HtmlElement>()?;"),
            "dyn_into cast:\n{}",
            out
        );
    }

    #[test]
    fn emit_render_clears_and_rebuilds() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("fn render(self: &Rc<Self>) {"),
            "render signature:\n{}",
            out
        );
        assert!(
            out.contains("while let Some(child) = root.first_child() {"),
            "clear loop:\n{}",
            out
        );
        assert!(
            out.contains("let _ = root.remove_child(&child);"),
            "remove_child:\n{}",
            out
        );
    }

    #[test]
    fn emit_template_element_creates_div_with_scoped_class() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains(r#"document.create_element("div").unwrap();"#),
            "div creation:\n{}",
            out
        );
        // 11.3.c already suffixed the class attribute — the emit
        // just copies the suffixed value. Actual shape is
        // `class="<original> <original>-<scope_class>"`, and the
        // scope_class from 11.3.c is `counter-c-<8hex>` (kebab of
        // "Counter" + `-c-` + FNV-1a hash), so the full attr reads
        // `class="counter counter-counter-c-<hex>"` (original class
        // `counter` + scope suffix `counter-c-<hex>`).
        assert!(
            out.contains(r#".set_attribute("class", "counter counter-counter-c-"#),
            "scoped class attr:\n{}",
            out
        );
    }

    #[test]
    fn emit_interpolation_uses_format_and_state_borrow() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        // The exact var counter (`__interp0` vs `__interp4`) depends
        // on how many earlier elements/texts the render walker
        // allocated. Assert the format! + borrow shape without
        // pinning the counter — it changes if the template grows.
        assert!(
            out.contains(r#"= format!("{}", (*self.count.borrow()));"#),
            "interpolation format! call:\n{}",
            out
        );
    }

    #[test]
    fn cw6_flv_is_identity_passthrough_in_text_interpolation() {
        // A component authored in the SSR companion style — `{flv(label)}`
        // + `from fitz_liveviews import flv` — must emit byte-identically to
        // plain `{label}` on the client-WASM target: a DOM text node escapes
        // intrinsically, so `flv` (HTML escape) is the identity here. This is
        // what lets an SSR component compile to `--target wasm-client`
        // unchanged (CW.6 dual-target).
        let flv_src = "component tag {\n  \
             state {\n    label: Str = \"\"\n  }\n  \
             <template><span>{flv(label)}</span></template>\n}\n";
        let plain_src = "component tag {\n  \
             state {\n    label: Str = \"\"\n  }\n  \
             <template><span>{label}</span></template>\n}\n";
        let flv_out = emit_component(&parse_expand(flv_src).components[0]).unwrap();
        let plain_out = emit_component(&parse_expand(plain_src).components[0]).unwrap();
        assert_eq!(
            flv_out, plain_out,
            "`{{flv(label)}}` must emit byte-identically to `{{label}}` on wasm"
        );
        // No Rust call to a `flv` fn survives (it would fail to link — the
        // helper lives in the SSR lib, not the wasm bundle).
        assert!(
            !flv_out.contains("flv("),
            "no `flv(...)` Rust call should survive:\n{flv_out}"
        );
    }

    #[test]
    fn cw6_raw_html_helpers_hard_error_as_ssr_only() {
        // The List<Html>-folding framework helpers have no client-WASM
        // equivalent (no single-string form) — identity would fail to
        // type-check. They must hard-error, naming themselves SSR-only,
        // rather than emitting a broken call (CW.6 dual-target). NOTE:
        // `raw_html`/`html` used to be in this list, but CW.9 (1b) turns
        // them into a `set_inner_html` sink in interpolation position — see
        // `cw9_1b_raw_html_sink_emits_set_inner_html`.
        for helper in ["h_join", "h_when", "h_either"] {
            let src = format!(
                "component tag {{\n  state {{\n    label: Str = \"\"\n  }}\n  \
                 <template><span>{{{helper}(label)}}</span></template>\n}}\n"
            );
            let err = emit_component(&parse_expand(&src).components[0])
                .expect_err("List<Html> helper must reject on the wasm target");
            assert!(
                err.message.contains("SSR-only"),
                "error for `{helper}` should name it SSR-only: {}",
                err.message
            );
        }
    }

    #[test]
    fn cw9_1b_raw_html_sink_emits_set_inner_html() {
        // CW.9 (1b) — `{raw_html(x)}` / `{html(x)}` as an element child
        // injects unescaped markup via `set_inner_html` on the parent,
        // instead of an escaping text node. This unblocks SSR companion
        // components whose helpers build markup strings.
        for helper in ["raw_html", "html"] {
            let src = format!(
                "component tag {{\n  state {{\n    markup: Str = \"\"\n  }}\n  \
                 <template><span>{{{helper}(markup)}}</span></template>\n}}\n"
            );
            let out = emit_component(&parse_expand(&src).components[0])
                .unwrap_or_else(|e| panic!("`{helper}` sink must emit, not reject: {e}"));
            assert!(
                out.contains(".set_inner_html(&format!(\"{}\", (*self.markup.borrow())))"),
                "`{helper}(markup)` must lower to set_inner_html on the parent:\n{out}"
            );
            assert!(
                !out.contains("create_text_node"),
                "the raw-HTML sink must NOT emit an escaping text node:\n{out}"
            );
        }
    }

    #[test]
    fn cw9_1c_html_returning_fn_transpiles_with_shim() {
        // CW.9 (1c) — a real markup helper returns `Html` (e.g. `icon`) and
        // builds it via `html(...)`. The `Html` shim lets it transpile:
        // `Html` -> `__FlvHtml`, `html(x)` -> the shim ctor, `.raw` field
        // access -> the String, rendered as DOM by the raw-HTML sink.
        let file = single_component_file(
            r#"component App {
  state { name: Str = "" }
  <template><span>{raw_html(mk_svg(name).raw)}</span></template>
}"#,
        );
        let fns = fns_from_classic(
            "fn mk_svg(n: Str) -> Html { return html(\"<svg>\" + n + \"</svg>\") }",
        );
        let out = emit_module_with_imports(&file, &NominalRegistry::new(), &fns).unwrap();
        assert!(
            out.contains("struct __FlvHtml {"),
            "Html shim struct:\n{out}"
        );
        assert!(
            out.contains("fn html(__s: String) -> __FlvHtml"),
            "html constructor:\n{out}"
        );
        assert!(
            out.contains("fn mk_svg(n: String) -> __FlvHtml {"),
            "Html return type maps to __FlvHtml:\n{out}"
        );
        assert!(
            out.contains("html(format!"),
            "html() call lowers in the body:\n{out}"
        );
        assert!(
            out.contains("set_inner_html(&format!(\"{}\", mk_svg((*self.name.borrow()).clone()).raw.clone()))"),
            "the sink renders `.raw` of the Html value:\n{out}"
        );
    }

    #[test]
    fn cw9_1c_no_html_shim_when_html_unused() {
        // Byte-compat: a bundle whose imported fns don't touch `Html` never
        // emits the shim.
        let file = single_component_file(
            r#"component App {
  state { x: Int = 0 }
  <template><span>{x}</span></template>
}"#,
        );
        let fns = fns_from_classic("fn double(n: Int) -> Int { return n * 2 }");
        let out = emit_module_with_imports(&file, &NominalRegistry::new(), &fns).unwrap();
        assert!(
            !out.contains("__FlvHtml"),
            "no Html shim when no fn uses Html:\n{out}"
        );
    }

    #[test]
    fn cw9_bool_field_access_in_if_condition_lowers() {
        // CW.9 — `{#if c.done}` (a Bool field on a `{#for}` loop var of
        // nominal type) lowers as a condition. Unblocks the option-list
        // components (`Select` / `RadioGroup`, which test `{#if o.on}`).
        let expanded = parse_expand(
            "from card import Card\n\ncomponent Board {\n  state { cards: List<Card> = [] }\n  <template><ul>{#for c in cards}<li>{#if c.done}<b>x</b>{#else}<i>y</i>{/if}</li>{/for}</ul></template>\n}",
        );
        let out = emit_module_with_nominals(&expanded, &card_registry()).unwrap();
        assert!(
            out.contains("c.done.clone()"),
            "the bool field access lowers in the condition:\n{out}"
        );
    }

    #[test]
    fn cw9_list_for_in_helper_body_lowers() {
        // CW.9 — `for x in <list>` in a helper body (e.g. `bar_scale`'s max
        // scan) iterates the list by cloned value, so the list stays
        // available afterwards.
        let file = single_component_file(
            r#"component App {
  state { x: Int = 0 }
  <template><span>{x}</span></template>
}"#,
        );
        let fns = fns_from_classic(
            "fn max_of(xs: List<Int>) -> Int {\n  let m = 0\n  for x in xs {\n    if (x > m) { m = x }\n  }\n  return m\n}",
        );
        let out = emit_module_with_imports(&file, &NominalRegistry::new(), &fns).unwrap();
        assert!(
            out.contains("fn max_of(xs: Vec<i64>) -> i64 {"),
            "List<Int> param maps to Vec<i64>:\n{out}"
        );
        assert!(
            out.contains("for x in (xs).iter().cloned() {"),
            "the list `for` iterates by cloned value:\n{out}"
        );
    }

    #[test]
    fn emit_event_attr_wires_click_via_closure() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("let __closure = Closure::wrap(Box::new(move |_evt: Event| {"),
            "closure wrap:\n{}",
            out
        );
        assert!(
            out.contains("Counter::decrement(&__self_clone);"),
            "handler call:\n{}",
            out
        );
        assert!(
            out.contains(
                r#".add_event_listener_with_callback("click", __closure.as_ref().unchecked_ref()).unwrap();"#
            ),
            "add_event_listener:\n{}",
            out
        );
        assert!(
            out.contains("__closure.forget();"),
            "closure forget:\n{}",
            out
        );
    }

    // ---- Style helper -------------------------------------------

    #[test]
    fn emit_scoped_style_helper_dedups_with_atomic_bool() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("fn __inject_style_Counter_counter_c_"),
            "helper signature:\n{}",
            out
        );
        assert!(
            out.contains("static INJECTED: AtomicBool = AtomicBool::new(false);"),
            "dedup atomic:\n{}",
            out
        );
        assert!(
            out.contains("if INJECTED.swap(true, Ordering::SeqCst) { return; }"),
            "swap check:\n{}",
            out
        );
        // CSS body should be scoped: the `.counter` selector gets
        // suffixed to `.counter-<scope_class>` by 11.3.b, and the
        // scope_class is `counter-c-<8hex>`, so the full selector
        // reads `.counter-counter-c-<hex>` (double `counter-`).
        assert!(
            out.contains("counter-counter-c-"),
            "scoped CSS body has double-prefix (11.3.b apply_scope + 11.3.c scope class):\n{}",
            out
        );
    }

    #[test]
    fn emit_global_style_helper_named_global() {
        let src = r#"component Foo {
  state { x: Int = 0 }

  <template>
    <div></div>
  </template>

  <style global>
    body { margin: 0; }
  </style>
}"#;
        let expanded = parse_expand(src);
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("fn __inject_style_Foo_global()"),
            "global helper:\n{}",
            out
        );
        assert!(
            out.contains("body { margin: 0; }"),
            "global CSS body:\n{}",
            out
        );
    }

    #[test]
    fn emit_no_style_no_helper() {
        let src = r#"component Bare {
  state { x: Int = 0 }

  <template>
    <div></div>
  </template>
}"#;
        let expanded = parse_expand(src);
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            !out.contains("__inject_style"),
            "no style helper expected:\n{}",
            out
        );
        // mount() also should NOT call any injector
        assert!(
            !out.contains("__inject_style_Bare"),
            "no injector call in mount:\n{}",
            out
        );
    }

    // ---- Errors for the deferred subset -------------------------

    #[test]
    fn emit_now_accepts_str_state_field_since_11_5_d() {
        // Regression / behaviour-change guard: Phase 11.5.d extended
        // the emitter's accepted state-field primitive set to
        // include `Str` (plus `Float`/`Bool`/`Nullable<T>` — see the
        // 11.5.d unit tests). A `Str` state field with a literal
        // default now emits successfully instead of erroring.
        let src = r#"component Foo {
  state { name: Str = "hi" }

  <template>
    <div>{name}</div>
  </template>
}"#;
        let expanded = parse_expand(src);
        let out = emit_component(&expanded.components[0])
            .expect("Str state fields must emit successfully since 11.5.d");
        assert!(
            out.contains("name: RefCell<String>,"),
            "Str field should map to RefCell<String>:\n{out}"
        );
        assert!(
            out.contains(r#"name: RefCell::new("hi".to_string())"#),
            "default `\"hi\"` should coerce to a Rust String literal:\n{out}"
        );
    }

    #[test]
    fn emit_rejects_unregistered_nominal_state_field_citing_r3_import() {
        // A nominal state field with NO entry in the registry (the
        // `emit_component` path uses an empty registry) still rejects,
        // now pointing the user at importing the type from a sibling
        // `.fitz` so R3 can load its fields.
        let src = r#"component Foo {
  state { user: User? = null }

  <template>
    <div>hi</div>
  </template>
}"#;
        let expanded = parse_expand(src);
        let err = emit_component(&expanded.components[0]).unwrap_err();
        assert!(
            err.message.contains("User") && err.message.contains("from card import"),
            "unregistered nominal rejection should suggest importing it (R3):\n{err}"
        );
    }

    // ---------------------------------------------------------------------
    // Phase 11.7 R3 — nominal types on the client-WASM target
    // ---------------------------------------------------------------------

    /// A `Card` nominal registry mirroring the `card.fitz` sibling of
    /// the `examples/view/nominal-list/` demo, for the R3 unit tests.
    fn card_registry() -> NominalRegistry {
        let mut r = NominalRegistry::new();
        r.insert(
            "Card".to_string(),
            vec![
                ("id".to_string(), TypeExpr::Named("Int".to_string())),
                ("title".to_string(), TypeExpr::Named("Str".to_string())),
                ("done".to_string(), TypeExpr::Named("Bool".to_string())),
            ],
        );
        r
    }

    /// A compact `.fitzv` exercising the whole R3 nominal path: a
    /// `List<Card>` state field, a `.push(Card { ... })` event body, a
    /// `{#for c in cards}` loop with `{c.*}` field access, and a keyed
    /// `<Row />` whose primitive props come from nominal fields.
    fn nominal_list_src() -> &'static str {
        r#"from card import Card

component Board {
  state {
    cards: List<Card> = []
    next_id: Int = 1
  }

  event add() {
    cards.push(Card { id: next_id, title: "x", done: false })
    next_id = next_id + 1
  }

  <template>
    <ul>
      {#for c in cards}
        <Row key="{c.id}" n="{c.id}" label="{c.title}" />
      {/for}
    </ul>
  </template>
}

component Row {
  state {
    n: Int = 0
    label: Str = ""
  }
  <template><li>{label}</li></template>
}"#
    }

    #[test]
    fn phase_11_7_r3_nominal_struct_emitted_from_registry() {
        let expanded = parse_expand(nominal_list_src());
        let out = emit_module_with_nominals(&expanded, &card_registry()).unwrap();
        assert!(
            out.contains("#[derive(Clone)]\npub struct Card {"),
            "imported nominal must be emitted as a Clone struct:\n{out}"
        );
        assert!(out.contains("    id: i64,"), "Card.id -> i64:\n{out}");
        assert!(
            out.contains("    title: String,"),
            "Card.title -> String:\n{out}"
        );
        assert!(out.contains("    done: bool,"), "Card.done -> bool:\n{out}");
    }

    #[test]
    fn phase_11_7_r3_list_nominal_state_maps_to_vec_of_struct() {
        let expanded = parse_expand(nominal_list_src());
        let out = emit_module_with_nominals(&expanded, &card_registry()).unwrap();
        assert!(
            out.contains("cards: RefCell<Vec<Card>>,"),
            "List<Card> state must map to Vec<Card>:\n{out}"
        );
        assert!(
            out.contains("cards: RefCell::new(Vec::new()),"),
            "empty List<Card> default must be Vec::new():\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_for_over_list_nominal_snapshots_and_binds_loop_var() {
        let expanded = parse_expand(nominal_list_src());
        let out = emit_module_with_nominals(&expanded, &card_registry()).unwrap();
        assert!(
            out.contains("(*self.cards.borrow()).clone();"),
            "the {{#for}} must snapshot the nominal Vec:\n{out}"
        );
        assert!(
            out.contains("for c in __for") && out.contains(".iter().cloned() {"),
            "the loop var must bind the nominal by value:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_field_access_interpolation_lowers_to_clone() {
        let expanded = parse_expand(nominal_list_src());
        let out = emit_module_with_nominals(&expanded, &card_registry()).unwrap();
        // Key from field access on the loop var.
        assert!(
            out.contains("let __key = format!(\"{}\", c.id.clone());"),
            "keyed child key must lower from `c.id` field access:\n{out}"
        );
        // Primitive props fanned out from nominal fields.
        assert!(
            out.contains(".n.borrow_mut() = c.id.clone();"),
            "int prop must come from `c.id`:\n{out}"
        );
        assert!(
            out.contains(".label.borrow_mut() = c.title.clone();"),
            "str prop must come from `c.title`:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_struct_literal_and_push_in_event_body() {
        let expanded = parse_expand(nominal_list_src());
        let out = emit_module_with_nominals(&expanded, &card_registry()).unwrap();
        assert!(
            out.contains(
                "self.cards.borrow_mut().push(Card { id: (*self.next_id.borrow()), \
                 title: \"x\".to_string(), done: false });"
            ),
            "event body must construct + push a Card:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_keyed_dynamic_child_over_nominal_list() {
        let expanded = parse_expand(nominal_list_src());
        let out = emit_module_with_nominals(&expanded, &card_registry()).unwrap();
        assert!(
            out.contains("__child_map_0: RefCell<std::collections::HashMap<String, Rc<Row>>>,"),
            "dynamic child over a nominal list still gets a keyed cache:\n{out}"
        );
        assert!(
            out.contains(
                "self.__child_map_0.borrow_mut().retain(|__k, _| __seen_0.contains(__k));"
            ),
            "reconciliation retain must still be emitted:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_unregistered_nominal_list_rejects_citing_import() {
        // Empty registry — `Card` is unknown, so the `List<Card>` state
        // field rejects at struct emission with the R3 import pointer.
        let expanded = parse_expand(nominal_list_src());
        let err = emit_module_with_nominals(&expanded, &NominalRegistry::new()).unwrap_err();
        assert!(
            err.message.contains("Card") && err.message.contains("from card import"),
            "unregistered nominal list must point at importing the type:\n{err}"
        );
    }

    #[test]
    fn phase_11_7_r3_empty_registry_emits_no_struct() {
        // A primitive-only component with an empty registry must not
        // emit any nominal struct — preserving the pre-R3 output.
        let expanded = parse_expand(counter_shape_src());
        let out = emit_module_with_nominals(&expanded, &NominalRegistry::new()).unwrap();
        assert!(
            !out.contains("#[derive(Clone)]"),
            "no nominal struct should be emitted for a primitive-only module:\n{out}"
        );
    }

    // ---------------------------------------------------------------------
    // Phase 11.7.b — `{#if}` / `{#for}` in the WASM emitter
    // ---------------------------------------------------------------------

    #[test]
    fn phase_11_7_b_if_directive_emits_rust_if_with_comparison() {
        let src = r#"component Foo {
  state { x: Int = 0 }

  <template>
    <div>{#if x > 0}<span>positive</span>{#else}<span>zero</span>{/if}</div>
  </template>
}"#;
        let expanded = parse_expand(src);
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("if ((*self.x.borrow()) > 0i64) {"),
            "if condition must lower to a Rust comparison:\n{out}"
        );
        assert!(
            out.contains("} else {"),
            "the {{#else}} branch must emit a Rust else:\n{out}"
        );
    }

    #[test]
    fn str_comparison_if_condition_lowers_to_string_eq() {
        // Str comparison in a `{#if}` is NOT a gap — both sides lower via
        // `lower_expr`, so `variant == "error"` compiles as `String == String`.
        // (Regression guard: it had been mis-documented as unsupported.)
        let src = r#"component Foo {
  state { variant: Str = "error" }

  <template>
    <div>{#if variant == "error"}<span>e</span>{#else}<span>o</span>{/if}</div>
  </template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains(r#"if ((*self.variant.borrow()) == "error".to_string()) {"#),
            "Str `{{#if}}` must lower to a String == String test:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_b_if_bool_state_field_used_directly() {
        let src = r#"component Foo {
  state { visible: Bool = false }

  <template>
    <div>{#if visible}<span>shown</span>{/if}</div>
  </template>
}"#;
        let expanded = parse_expand(src);
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("if (*self.visible.borrow()) {"),
            "a bool state field must be usable directly as a condition:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_b_for_over_list_str_emits_snapshot_and_loop() {
        let src = r#"component Foo {
  state { tags: List<Str> = [] }

  <template>
    <ul>{#for tag in tags}<li>{tag}</li>{/for}</ul>
  </template>
}"#;
        let expanded = parse_expand(src);
        let out = emit_component(&expanded.components[0]).unwrap();
        assert!(
            out.contains("= (*self.tags.borrow()).clone();"),
            "for must snapshot the state Vec:\n{out}"
        );
        assert!(
            out.contains("for tag in ") && out.contains(".iter().cloned() {"),
            "for must iterate the snapshot binding the loop var:\n{out}"
        );
        assert!(
            out.contains(r#"format!("{}", tag)"#),
            "the loop var must be usable in an interpolation:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_a1_for_unlowerable_iterable_still_rejects() {
        // Phase 11.7 R3.5a.1 opened `{#for}` to general iterables that
        // lower (method calls like `.filter(...)`), but an iterable the
        // expr lowerer can't handle — here a `Range` — still rejects,
        // now via the general lowering path rather than a blanket
        // "must be a state-field identifier" gate.
        let src = r#"component Foo {
  state { x: Int = 0 }
  event bump() { x = 1 }

  <template>
    <div>{#for n in 0..3}<span>hi</span>{/for}</div>
  </template>
}"#;
        let expanded = parse_expand(src);
        let err = emit_component(&expanded.components[0]).unwrap_err();
        assert!(
            err.message.contains("11.7"),
            "an unlowerable `{{#for}}` iterable must still reject:\n{}",
            err
        );
    }

    #[test]
    fn phase_11_7_d_emit_slot_renders_callback_or_fallback() {
        // Phase 11.7.d — `<slot />` now emits the parent-content callback
        // with a fallback branch (it no longer rejects).
        let src = r#"component Foo {
  state { x: Int = 0 }

  <template>
    <div><slot><span>fallback</span></slot></div>
  </template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("__slot: RefCell<Option<Rc<dyn Fn(&web_sys::Node)>>>,"),
            "a component with a <slot /> gets a __slot field:\n{out}"
        );
        assert!(
            out.contains("if let Some(__cb) = self.__slot.borrow().as_ref() {")
                && out.contains("} else {"),
            "the slot emits a callback-or-fallback branch:\n{out}"
        );
    }

    #[test]
    fn named_slots_emit_named_field_and_render_branch() {
        let src = r#"component Foo {
  state { x: Int = 0 }
  <template><div><slot name="header" /></div></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("__slot_header: RefCell<Option<Rc<dyn Fn(&web_sys::Node)>>>,"),
            "a named slot gets a __slot_<name> field:\n{out}"
        );
        assert!(
            !out.contains("    __slot: RefCell<Option<Rc<dyn Fn(&web_sys::Node)>>>,"),
            "a named-only component has no default __slot field:\n{out}"
        );
        assert!(
            out.contains("if let Some(__cb) = self.__slot_header.borrow().as_ref() {"),
            "the named slot renders from its own backing field:\n{out}"
        );
    }

    #[test]
    fn emit_rejects_unsupported_event() {
        // CW.9 wired `@input` / `@change`; a genuinely unsupported event
        // (e.g. `@mouseover`) still rejects with a clear message.
        let src = r#"component Foo {
  state { x: Int = 0 }
  event bump() { x = 1 }

  <template>
    <div @mouseover="bump"></div>
  </template>
}"#;
        let expanded = parse_expand(src);
        let err = emit_component(&expanded.components[0]).unwrap_err();
        assert!(
            err.message.contains("@mouseover"),
            "event kind error:\n{}",
            err
        );
        assert!(
            err.message.contains("@click"),
            "should name the supported events:\n{}",
            err
        );
    }

    #[test]
    fn cw9_input_event_reads_value_into_payload() {
        // CW.9 — `@input` on a text input wires an `input` listener that reads
        // the target's live value into `payload["value"]` and calls the
        // handler (which reads it back).
        let src = r#"component Form {
  state { name: Str = "" }
  event on_type() { name = payload["value"] }

  <template>
    <input @input="on_type" value="{name}" />
    <p>{name}</p>
  </template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("move |__evt: Event|"),
            "input closure names the event:\n{out}"
        );
        assert!(
            out.contains("__evt.target()")
                && out.contains("dyn_ref::<web_sys::HtmlInputElement>()")
                && out.contains("__el.value()"),
            "input listener reads the target value:\n{out}"
        );
        assert!(
            out.contains(r#"__payload.insert("value".to_string()"#),
            "value goes into the payload under \"value\":\n{out}"
        );
        assert!(
            out.contains("Form::on_type(&__self_clone, &__payload)"),
            "handler called with payload:\n{out}"
        );
        assert!(
            out.contains(r#".add_event_listener_with_callback("input""#),
            "listener bound on the `input` event:\n{out}"
        );
    }

    #[test]
    fn cw9_change_event_covers_select_and_textarea() {
        // `@change` on a <select> wires a `change` listener; the emitted
        // closure covers select + textarea + input casts.
        let src = r#"component Picker {
  state { choice: Str = "a" }
  event on_pick() { choice = payload["value"] }

  <template>
    <select @change="on_pick"><option>a</option><option>b</option></select>
  </template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains(r#".add_event_listener_with_callback("change""#),
            "listener bound on the `change` event:\n{out}"
        );
        assert!(
            out.contains("dyn_ref::<web_sys::HtmlSelectElement>()")
                && out.contains("dyn_ref::<web_sys::HtmlTextAreaElement>()"),
            "select + textarea casts present:\n{out}"
        );
    }

    #[test]
    fn cw9_input_handler_must_read_payload() {
        // An `@input` handler that ignores the value is a mistake — reject it
        // with a pointer to `payload["value"]`.
        let src = r#"component Bad {
  state { n: Int = 0 }
  event bump() { n = 1 }

  <template>
    <input @input="bump" />
  </template>
}"#;
        let err = emit_component(&parse_expand(src).components[0]).unwrap_err();
        assert!(
            err.message.contains("payload[\"value\"]"),
            "must point at payload[\"value\"]:\n{}",
            err
        );
    }

    #[test]
    fn cw9_input_adds_html_input_web_sys_features() {
        let src = r#"component Form {
  state { name: Str = "" }
  event on_type() { name = payload["value"] }
  <template><input @input="on_type" /></template>
}"#;
        let file = parse_expand(src);
        let feats = wasm_extra_web_sys_features(&file);
        for f in [
            "HtmlInputElement",
            "HtmlSelectElement",
            "HtmlTextAreaElement",
        ] {
            assert!(feats.contains(&f), "missing web-sys feature {f}: {feats:?}");
        }
    }

    #[test]
    fn cw9_click_only_component_keeps_no_input_features() {
        // A component with only `@click` must NOT pull the input features —
        // byte-for-byte with pre-CW.9 crates.
        let src = r#"component Tap {
  state { n: Int = 0 }
  event bump() { n = n + 1 }
  <template><button @click="bump">{n}</button></template>
}"#;
        let file = parse_expand(src);
        let feats = wasm_extra_web_sys_features(&file);
        assert!(
            feats.is_empty(),
            "click-only component needs no extra web-sys features: {feats:?}"
        );
    }

    // -------------------------------------------------------------------
    // Phase 11.12 — SSR → client hydration (adopt the server-painted DOM)
    // -------------------------------------------------------------------

    fn hydratable_src() -> &'static str {
        // Sole-child interpolations (`<span>{name}</span>`) so the server-
        // painted text nodes map 1:1 onto the adopt walk (slice-1 constraint:
        // no mixed static+interpolated text).
        r#"component Greeter {
  state { name: Str = "world" }
  event on_name() { name = payload["value"] }
  <template>
    <div class="greeter">
      <p><span class="nm">{name}</span></p>
      <input @input="on_name" value="{name}" />
    </div>
  </template>
}"#
    }

    #[test]
    fn phase_11_12_hydratable_component_emits_hydrate_and_apply_state() {
        let out = emit_component(&parse_expand(hydratable_src()).components[0]).unwrap();
        assert!(
            out.contains(
                "pub fn hydrate(self: &Rc<Self>, root: HtmlElement) -> Result<(), JsValue>"
            ),
            "hydrate method:\n{out}"
        );
        assert!(
            out.contains("fn __apply_state_json(self: &Rc<Self>, __json: &str)"),
            "apply-state method:\n{out}"
        );
        // Restores the serialized state from the `<script>` before adopting.
        assert!(
            out.contains("get_element_by_id(\"__flv_state_Greeter\")"),
            "reads the state script by id:\n{out}"
        );
        assert!(
            out.contains("if let Some(__x) = __v.get(\"name\").and_then(|__j| __j.as_str()) { *self.name.borrow_mut() = __x.to_string(); }"),
            "restores the Str state field:\n{out}"
        );
        // Marks built so later state changes patch in place, not rebuild.
        assert!(
            out.contains("*self.__built.borrow_mut() = true;"),
            "hydrate marks the component built:\n{out}"
        );
    }

    #[test]
    fn phase_11_12_hydrate_adopts_not_creates() {
        let out = emit_component(&parse_expand(hydratable_src()).components[0]).unwrap();
        // Slice the hydrate() body and assert it ADOPTS (cursor helpers) and
        // never CREATES nodes — that is the whole point.
        let start = out.find("pub fn hydrate(").expect("hydrate present");
        let body = &out[start..];
        let end = body.find("*self.__built.borrow_mut() = true;").unwrap();
        let hydrate_body = &body[..end];
        assert!(
            hydrate_body.contains("__flv_next_element(&mut")
                && hydrate_body.contains("__flv_next_text(&mut"),
            "hydrate adopts via cursor helpers:\n{hydrate_body}"
        );
        assert!(
            !hydrate_body.contains("create_element") && !hydrate_body.contains("create_text_node"),
            "hydrate must NOT create nodes:\n{hydrate_body}"
        );
        // The adopted nodes are stashed into the same keep-node handles the
        // build walk declared (indices assigned in DFS order).
        assert!(
            hydrate_body.contains(".borrow_mut() = Some(__hn);"),
            "hydrate stashes an adopted text handle:\n{hydrate_body}"
        );
        assert!(
            hydrate_body.contains("*self.__kattr_") && hydrate_body.contains("*self.__ktext_"),
            "hydrate stashes text + attr handles:\n{hydrate_body}"
        );
        // The live `@input` listener is wired onto the adopted element.
        assert!(
            hydrate_body.contains(".add_event_listener_with_callback(\"input\","),
            "hydrate wires the @input listener:\n{hydrate_body}"
        );
    }

    #[test]
    fn phase_11_12_module_emits_cursor_helpers_when_hydratable() {
        let out = emit_module(&parse_expand(hydratable_src())).unwrap();
        assert!(
            out.contains(
                "fn __flv_next_element(__cursor: &mut Option<web_sys::Node>) -> Option<web_sys::Element>"
            ) && out.contains(
                "fn __flv_next_text(__cursor: &mut Option<web_sys::Node>) -> Option<web_sys::Text>"
            ),
            "cursor helpers present:\n{out}"
        );
    }

    #[test]
    fn phase_11_12_state_apply_maps_each_primitive() {
        // Str/Int/Float/Bool restore via the right serde_json accessor; the
        // component is value-input (keeps it on the keep-node/hydrate path).
        let src = r#"component Multi {
  state {
    s: Str = ""
    n: Int = 0
    f: Float = 0.0
    b: Bool = false
  }
  event on_type() { s = payload["value"] }
  <template>
    <input @input="on_type" value="{s}" />
    <p><span>{n}</span></p>
    <p><span>{f}</span></p>
    <p><span>{b}</span></p>
  </template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(out.contains("__v.get(\"s\").and_then(|__j| __j.as_str()) { *self.s.borrow_mut() = __x.to_string(); }"), "Str:\n{out}");
        assert!(
            out.contains(
                "__v.get(\"n\").and_then(|__j| __j.as_i64()) { *self.n.borrow_mut() = __x; }"
            ),
            "Int:\n{out}"
        );
        assert!(
            out.contains(
                "__v.get(\"f\").and_then(|__j| __j.as_f64()) { *self.f.borrow_mut() = __x; }"
            ),
            "Float:\n{out}"
        );
        assert!(
            out.contains(
                "__v.get(\"b\").and_then(|__j| __j.as_bool()) { *self.b.borrow_mut() = __x; }"
            ),
            "Bool:\n{out}"
        );
    }

    #[test]
    fn phase_11_13_emit_dev_state_methods_snapshots_and_applies_primitives() {
        // The dev-only state-preservation methods snapshot primitive state to
        // JSON and re-apply it, mirroring `json_state_accessor`.
        let src = "component App {\n  state {\n    count: Int = 0\n    label: Str = \"x\"\n  }\n  event inc() { count = count + 1 }\n  <template><div><span>{label}</span><span>{count}</span></div></template>\n}\n";
        let comp = &parse_expand(src).components[0];
        let mut out = String::new();
        emit_dev_state_methods(comp, &NominalRegistry::new(), &mut out).unwrap();
        assert!(
            out.contains("pub fn __fitz_dev_snapshot(self: &Rc<Self>) -> String"),
            "snapshot method:\n{out}"
        );
        assert!(
            out.contains("pub fn __fitz_dev_apply(self: &Rc<Self>, __json: &str)"),
            "apply method:\n{out}"
        );
        // snapshot: Int via `*borrow`, Str via `.clone()`.
        assert!(
            out.contains("serde_json::json!(*self.count.borrow())"),
            "int snapshot:\n{out}"
        );
        assert!(
            out.contains("serde_json::json!(self.label.borrow().clone())"),
            "str snapshot:\n{out}"
        );
        // apply: byte-identical to the scalar branch of `__apply_state_json`.
        assert!(
            out.contains("if let Some(__x) = __v.get(\"count\").and_then(|__j| __j.as_i64()) { *self.count.borrow_mut() = __x; }"),
            "int apply:\n{out}"
        );
        assert!(
            out.contains("if let Some(__x) = __v.get(\"label\").and_then(|__j| __j.as_str()) { *self.label.borrow_mut() = __x.to_string(); }"),
            "str apply:\n{out}"
        );
        assert!(
            out.contains("self.render();"),
            "re-render after apply:\n{out}"
        );
    }

    #[test]
    fn phase_11_13_slice3_composite_state_dump_and_apply() {
        // A `List<Str>` state field now round-trips through the dev snapshot:
        // the snapshot serializes it (json_dump_value → Value::Array) and the
        // apply restores it (json_restore_value → as_array filter_map).
        let src = "component App {\n  state {\n    items: List<Str> = []\n  }\n  event add() { items.push(\"x\") }\n  <template><div>{items.len()}</div></template>\n}\n";
        let comp = &parse_expand(src).components[0];
        let mut out = String::new();
        emit_dev_state_methods(comp, &NominalRegistry::new(), &mut out).unwrap();
        // snapshot: builds a JSON array from the Vec.
        assert!(
            out.contains("serde_json::Value::Array((&*self.items.borrow()).iter().map(|__le|"),
            "composite snapshot:\n{out}"
        );
        // apply: restores the composite behind the get() guard, then assigns.
        assert!(
            out.contains("if let Some(__fv) = __v.get(\"items\") {"),
            "composite apply guard:\n{out}"
        );
        assert!(
            out.contains("as_array().map(|__arr|"),
            "composite apply restore:\n{out}"
        );
        assert!(
            out.contains("*self.items.borrow_mut() = __restored;"),
            "assign:\n{out}"
        );
    }

    #[test]
    fn phase_11_12_slice3_list_str_restores_from_payload() {
        // A hydratable component with a `List<Str>` state field now restores
        // it from the payload (slice 1/2 kept the default). The scalar sibling
        // keeps the byte-identical accessor form.
        let src = r#"component Names {
  state {
    q: Str = ""
    items: List<Str> = ["a", "b"]
  }
  event on_type() { q = payload["value"] }
  <template>
    <input @input="on_type" value="{q}" />
    <ul>{#for it in items}<li>{it}</li>{/for}</ul>
  </template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        // Scalar unchanged.
        assert!(
            out.contains(
                "__v.get(\"q\").and_then(|__j| __j.as_str()) { *self.q.borrow_mut() = __x.to_string(); }"
            ),
            "scalar keeps accessor form:\n{out}"
        );
        // Composite restore for the list.
        assert!(
            out.contains("if let Some(__fv) = __v.get(\"items\") {"),
            "list field guarded by get(\"items\"):\n{out}"
        );
        assert!(
            out.contains(
                "__fv.as_array().map(|__arr| __arr.iter().filter_map(|__le| __le.as_str().map(|__s| __s.to_string())).collect::<Vec<String>>())"
            ),
            "list<Str> restore shape:\n{out}"
        );
    }

    #[test]
    fn phase_11_12_slice3_scalar_only_component_has_no_composite_guard() {
        // Byte-compat guard: a component whose state is all scalars must not
        // grow the composite `if let Some(__fv)` restore form.
        let src = r#"component Lone {
  state { s: Str = "" }
  event on_type() { s = payload["value"] }
  <template><input @input="on_type" value="{s}" /></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            !out.contains("if let Some(__fv) = __v.get("),
            "scalar-only component keeps no composite restore:\n{out}"
        );
    }

    #[test]
    fn phase_11_12_slice3_json_restore_nullable_and_map() {
        let empty = NominalRegistry::new();
        // Nullable<Int> — null restores to Some(None), else the inner.
        let nul = json_restore_value(
            &TypeExpr::Nullable(Box::new(TypeExpr::Named("Int".to_string()))),
            &empty,
            "__fv",
        )
        .expect("Nullable<Int> restorable");
        assert_eq!(
            nul,
            "if __fv.is_null() { Some(None) } else { (__fv.as_i64()).map(Some) }"
        );
        // Map<Str, Int> — object → Vec<(String, i64)>.
        let m = json_restore_value(
            &TypeExpr::Generic {
                name: "Map".to_string(),
                args: vec![
                    TypeExpr::Named("Str".to_string()),
                    TypeExpr::Named("Int".to_string()),
                ],
            },
            &empty,
            "__fv",
        )
        .expect("Map<Str, Int> restorable");
        assert!(
            m.contains("as_object()")
                && m.contains("__mk.clone()")
                && m.contains("collect::<Vec<(String, i64)>>()"),
            "Map<Str, Int> restore shape: {m}"
        );
        // Map<Int, Str> — a non-Str key can't round-trip through a JSON object.
        assert!(
            json_restore_value(
                &TypeExpr::Generic {
                    name: "Map".to_string(),
                    args: vec![
                        TypeExpr::Named("Int".to_string()),
                        TypeExpr::Named("Str".to_string()),
                    ],
                },
                &empty,
                "__fv",
            )
            .is_none(),
            "Map<Int, _> is not restorable"
        );
    }

    #[test]
    fn phase_11_12_slice3_json_restore_nominal_and_list_nominal() {
        let mut reg = NominalRegistry::new();
        reg.insert(
            "Card".to_string(),
            vec![
                ("id".to_string(), TypeExpr::Named("Int".to_string())),
                ("title".to_string(), TypeExpr::Named("Str".to_string())),
                ("done".to_string(), TypeExpr::Named("Bool".to_string())),
            ],
        );
        // Nominal: a closure that builds the struct, every field via `?`.
        let card = json_restore_value(&TypeExpr::Named("Card".to_string()), &reg, "__fv")
            .expect("nominal");
        assert!(
            card.starts_with("(|__no: &serde_json::Value| -> Option<Card> { Some(Card {"),
            "nominal closure header: {card}"
        );
        assert!(
            card.contains("id: { let __nf = __no.get(\"id\")?; (__nf.as_i64())? }")
                && card.contains(
                    "title: { let __nf = __no.get(\"title\")?; (__nf.as_str().map(|__s| __s.to_string()))? }"
                )
                && card.contains("done: { let __nf = __no.get(\"done\")?; (__nf.as_bool())? }")
                && card.ends_with("})(__fv)"),
            "nominal field restore: {card}"
        );
        // List<Card>: each element restored via the nominal closure.
        let list = json_restore_value(
            &TypeExpr::Generic {
                name: "List".to_string(),
                args: vec![TypeExpr::Named("Card".to_string())],
            },
            &reg,
            "__fv",
        )
        .expect("List<Card>");
        assert!(
            list.starts_with("__fv.as_array().map(|__arr| __arr.iter().filter_map(|__le|")
                && list.contains("-> Option<Card>")
                && list.contains(")(__le))")
                && list.ends_with(".collect::<Vec<Card>>())"),
            "List<Card> restore shape: {list}"
        );
        // Unknown nominal → not restorable (keeps default).
        assert!(
            json_restore_value(&TypeExpr::Named("Ghost".to_string()), &reg, "__fv").is_none(),
            "unknown nominal is not restorable"
        );
    }

    #[test]
    fn phase_11_12_slice3_json_restore_unsupported_is_none() {
        let empty = NominalRegistry::new();
        assert!(
            json_restore_value(&TypeExpr::Tuple(vec![]), &empty, "__fv").is_none(),
            "tuple not restorable"
        );
        assert!(
            json_restore_value(
                &TypeExpr::Function {
                    params: vec![],
                    ret: Box::new(TypeExpr::Named("Int".to_string())),
                },
                &empty,
                "__fv",
            )
            .is_none(),
            "function not restorable"
        );
    }

    #[test]
    fn phase_11_12_non_hydratable_component_has_no_hydrate() {
        // A naive (non-value-input) component keeps the byte-identical naive
        // path: no hydrate method, no cursor helpers.
        let out = emit_module(&parse_expand(counter_shape_src())).unwrap();
        assert!(!out.contains("fn hydrate("), "no hydrate method:\n{out}");
        assert!(
            !out.contains("__flv_next_element"),
            "no cursor helpers:\n{out}"
        );
    }

    #[test]
    fn phase_11_12_regions_are_hydratable() {
        // Phase 11.12 slice 2 — a value-input component WITH a `{#if}` region is
        // keep-node AND hydratable: it emits a `hydrate()` whose body adopts the
        // region's server anchors (`<!--fr-->` / `<!--/fr-->`) instead of
        // rebuilding, while keeping the `__patch_region_0` method for later
        // state changes.
        let src = r#"component Toggler {
  state {
    name: Str = ""
    on: Bool = false
  }
  event on_type() { name = payload["value"] }
  <template>
    <input @input="on_type" value="{name}" />
    {#if on}<p><span>{name}</span></p>{/if}
  </template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("fn __patch(self: &Rc<Self>)"),
            "still keep-node:\n{out}"
        );
        assert!(
            out.contains("pub fn hydrate("),
            "regions now hydrate:\n{out}"
        );
        assert!(
            out.contains("fn __patch_region_0(self: &Rc<Self>)"),
            "region patch method retained:\n{out}"
        );
        // The hydrate body adopts the region anchors by tagged comment.
        let hstart = out.find("pub fn hydrate(").expect("hydrate present");
        let htail = out[hstart..]
            .find("*self.__built.borrow_mut() = true;")
            .expect("hydrate tail present");
        let hbody = &out[hstart..hstart + htail];
        assert!(
            hbody.contains("__flv_next_comment(&mut __cur_root, \"fr\")")
                && hbody.contains("\"/fr\""),
            "hydrate adopts region anchors by tagged comment:\n{hbody}"
        );
        assert!(
            hbody.contains("*self.__astart_0.borrow_mut() = Some(__rs);")
                && hbody.contains("*self.__aend_0.borrow_mut() = Some(__re);"),
            "hydrate stashes into the same region handle fields:\n{hbody}"
        );
        // Adopt only — the region content is server-painted, so `hydrate()`
        // must not create nodes or mount the region.
        assert!(
            !hbody.contains("create_element")
                && !hbody.contains("create_comment")
                && !hbody.contains("__mount_region_0()"),
            "hydrate must adopt, not build the region:\n{hbody}"
        );
    }

    #[test]
    fn phase_11_12_component_is_hydratable_predicate() {
        let empty = std::collections::BTreeSet::new();
        let greeter = parse_expand(hydratable_src()).components[0].clone();
        assert!(
            component_is_hydratable(&greeter, &empty),
            "value-input static"
        );

        let counter = parse_expand(counter_shape_src()).components[0].clone();
        assert!(
            !component_is_hydratable(&counter, &empty),
            "non-value-input is not hydratable"
        );

        // Slice 2 — a value-input component with a `{#if}` region IS hydratable.
        let region_src = r#"component Toggler {
  state {
    name: Str = ""
    on: Bool = false
  }
  event on_type() { name = payload["value"] }
  <template>
    <input @input="on_type" value="{name}" />
    {#if on}<p>{name}</p>{/if}
  </template>
}"#;
        let toggler = parse_expand(region_src).components[0].clone();
        assert!(
            component_is_hydratable(&toggler, &empty),
            "value-input with region is hydratable in slice 2"
        );
    }

    // -----------------------------------------------------------------------
    // Phase 11.12 slice 4 — naive composition hydration (child + slots)
    // -----------------------------------------------------------------------

    /// A root `App` marked `hydrate` composing a `Card` with a `<slot>`.
    const SLICE4_COMPOSITION_SRC: &str = r#"component App hydrate {
  state { title: Str = "d" }
  event bump() { title = "x" }
  <template>
    <div class="page">
      <Card>
        <p class="lead">from parent</p>
        <button class="b" @click="bump">bump</button>
      </Card>
    </div>
  </template>
}

component Card {
  state { taps: Int = 0 }
  event tap() { taps = taps + 1 }
  <template>
    <section class="card">
      <div class="body"><slot><em>fallback</em></slot></div>
      <button class="t" @click="tap">tap</button>
    </section>
  </template>
}"#;

    #[test]
    fn phase_11_12_slice4_marker_makes_naive_composition_hydratable() {
        let out = emit_module(&parse_expand(SLICE4_COMPOSITION_SRC)).unwrap();
        // Both App and Card emit a hydrate() (whole tree opted in via the root).
        assert_eq!(
            out.matches("pub fn hydrate(").count(),
            2,
            "App + Card both emit hydrate():\n{out}"
        );
        // The slot-declaring child (Card) carries the __hslot adopt field.
        assert!(
            out.contains("__hslot: RefCell<Option<Rc<dyn Fn(&mut Option<web_sys::Node>)>>>,"),
            "Card carries __hslot field:\n{out}"
        );
        // The parent (App) synthesises a __hydrate_slot_0 adopt method taking a
        // cursor, and reborrows the param place (`&mut (*__cursor)`).
        assert!(
            out.contains(
                "fn __hydrate_slot_0(self: &Rc<Self>, __cursor: &mut Option<web_sys::Node>) {"
            ),
            "App emits __hydrate_slot_0:\n{out}"
        );
        assert!(
            out.contains("__flv_next_element(&mut (*__cursor))"),
            "slot adopt method reborrows the cursor param:\n{out}"
        );
        // App wires Card's __hslot (adopt) AND keeps __slot (rebuild) wiring.
        assert!(
            out.contains(".__hslot.borrow_mut() = Some(Rc::new(move |__c: &mut Option<web_sys::Node>| __parent.__hydrate_slot_0(__c)));"),
            "App wires __hslot -> __hydrate_slot_0:\n{out}"
        );
        assert!(
            out.contains(".__slot.borrow_mut() = Some(Rc::new(move |__t: &web_sys::Node| __parent.__render_slot_0(__t)));"),
            "App keeps __slot -> __render_slot_0 for later rebuild:\n{out}"
        );
        // Card.hydrate adopts its <slot> via the __hslot callback.
        assert!(
            out.contains("let __hcb = self.__hslot.borrow().clone();"),
            "Card.hydrate adopts <slot> via __hslot:\n{out}"
        );
        // The adopted child is hydrated, not mount_into'd, in App's adopt walk.
        assert!(
            out.contains(".hydrate(") && out.contains("_html).unwrap();"),
            "child is hydrated in the adopt walk:\n{out}"
        );
    }

    #[test]
    fn phase_11_12_slice4_without_marker_stays_fresh_mount() {
        // Same tree WITHOUT the `hydrate` marker → no hydration surface at all
        // (composition still fresh-mounts via mount_into, byte-identical to the
        // pre-11.12 path).
        let no_marker =
            SLICE4_COMPOSITION_SRC.replace("component App hydrate {", "component App {");
        let out = emit_module(&parse_expand(&no_marker)).unwrap();
        assert!(
            !out.contains("pub fn hydrate("),
            "no marker -> no hydrate() emitted:\n{out}"
        );
        assert!(
            !out.contains("__hslot"),
            "no marker -> no __hslot field:\n{out}"
        );
        assert!(
            !out.contains("__hydrate_slot_"),
            "no marker -> no __hydrate_slot method:\n{out}"
        );
        // Composition still works via the naive build path.
        assert!(
            out.contains(".mount_into(") && out.contains("__render_slot_0"),
            "composition still fresh-mounts:\n{out}"
        );
    }

    #[test]
    fn phase_11_12_slice4_gate_and_propagation() {
        let empty = std::collections::BTreeSet::new();
        let mut file = parse_expand(SLICE4_COMPOSITION_SRC);
        // Before propagation: only the root App carries the marker.
        assert!(file.components[0].hydrate, "root App has the marker");
        assert!(!file.components[1].hydrate, "Card has no marker of its own");
        // The gate treats a naive component as hydratable iff its `hydrate` flag
        // is set (Card is not keep-node — it composes a `<slot>`).
        assert!(
            component_is_hydratable(&file.components[0], &empty),
            "App (marker) is hydratable"
        );
        assert!(
            !component_is_hydratable(&file.components[1], &empty),
            "Card is not hydratable until propagation"
        );
        // Propagation lifts the whole tree.
        propagate_root_hydrate(&mut file);
        assert!(
            file.components.iter().all(|c| c.hydrate),
            "propagation sets hydrate on every component"
        );
        assert!(
            component_is_hydratable(&file.components[1], &empty),
            "Card hydratable after propagation"
        );
    }

    #[test]
    fn phase_11_12_slice4_naive_region_hydration_is_rejected() {
        // A naive component (no value-input → not keep-node) marked `hydrate`
        // with a `{#if}` region: the naive hydration path does not adopt regions
        // in this slice, so emit errors with a clear message.
        let src = r#"component App hydrate {
  state { on: Bool = true }
  event flip() { on = false }
  <template>
    <div>
      <button @click="flip">flip</button>
      {#if on}<p>shown</p>{/if}
    </div>
  </template>
}"#;
        let err = emit_module(&parse_expand(src)).unwrap_err();
        assert!(
            err.message.contains("NAIVE") && err.message.contains("region"),
            "region-in-naive-hydrate rejected with a clear message: {}",
            err.message
        );
    }

    #[test]
    fn phase_11_12_mixed_text_adopts_static_runs_and_the_interp_node() {
        // Phase 11.12 slice 2 — mixed static+interpolated text (`Hi, {name}!`)
        // hydrates: the static runs advance the cursor (discarded), the dynamic
        // run adopts its text node into the keep handle. The server separates
        // the runs with comment markers (`<!--fi-->` … `<!--/fi-->`), which the
        // skip-based `__flv_next_text` steps over, so the walk lines up without
        // a sole-child wrapper.
        let src = r#"component Greet {
  state { name: Str = "" }
  event on_type() { name = payload["value"] }
  <template>
    <input @input="on_type" value="{name}" />
    <p>Hi, {name}!</p>
  </template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        let hstart = out.find("pub fn hydrate(").expect("hydrate present");
        let htail = out[hstart..]
            .find("*self.__built.borrow_mut() = true;")
            .expect("hydrate tail present");
        let hbody = &out[hstart..hstart + htail];
        // Two static runs ("Hi, " and "!") advance the cursor and discard.
        assert_eq!(
            hbody.matches("let _ = __flv_next_text(&mut").count(),
            2,
            "two static text runs advance the cursor:\n{hbody}"
        );
        // The dynamic run adopts its text node into a keep handle.
        assert!(
            hbody.contains("if let Some(__hn) = __flv_next_text(&mut")
                && hbody.contains(".borrow_mut() = Some(__hn);"),
            "the interpolation adopts its text node:\n{hbody}"
        );
        // Adopt only — no node creation for the mixed text.
        assert!(
            !hbody.contains("create_text_node"),
            "mixed text adopts, never creates:\n{hbody}"
        );
    }

    // -------------------------------------------------------------------
    // Phase 11.10 slice 1 — keep-node reconciliation
    // -------------------------------------------------------------------

    #[test]
    fn phase_11_10_keepnode_gate_fires_for_static_value_input() {
        // A live `@input` over a static template compiles to the keep-node
        // model: a `__built` flag, `render()` dispatching build/patch, and a
        // `__patch()` that updates stashed handles in place.
        let src = r#"component LiveName {
  state { name: Str = "" }
  event on_type() { name = payload["value"] }
  <template>
    <input @input="on_type" value="{name}" />
    <p>Hello, {name}!</p>
  </template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(out.contains("__built: RefCell<bool>"), "build flag:\n{out}");
        assert!(
            out.contains("fn __build(self: &Rc<Self>)")
                && out.contains("fn __patch(self: &Rc<Self>)"),
            "build + patch fns:\n{out}"
        );
        assert!(
            out.contains("if *self.__built.borrow() {") && out.contains("self.__patch();"),
            "render dispatches to patch once built:\n{out}"
        );
        // Text interpolation `{name}` patches via set_data; the `value` attr
        // patches via set_attribute — both over stashed handles.
        assert!(
            out.contains("__ktext_")
                && out.contains(".set_data(&format!(\"{}\", (*self.name.borrow())))"),
            "text node patched in place:\n{out}"
        );
        assert!(
            out.contains("__kattr_")
                && out
                    .contains(".set_attribute(\"value\", &format!(\"{}\", (*self.name.borrow())))"),
            "value attr patched in place:\n{out}"
        );
        // The input element is built once (no wipe loop in render itself —
        // it lives in __build), so it is never re-created on a keystroke.
        assert!(
            out.contains("*self.__built.borrow_mut() = false;"),
            "mount_into resets the build flag:\n{out}"
        );
    }

    #[test]
    fn phase_11_10_keepnode_region_for_control_flow() {
        // Slice 3 — a value-input component with `{#if}`/`{#for}` now stays on
        // keep-node: each control-flow directive becomes an anchored dynamic
        // region (comment anchors + a DocumentFragment rebuild), while the
        // live `<input>` is still patched in place.
        let src = r#"component Guarded {
  state { name: Str = ""
          xs: List<Str> = [] }
  event on_type() { name = payload["value"] }
  <template>
    <input @input="on_type" value="{name}" />
    {#if name == ""}<p>empty</p>{#else}<p>{name}</p>{/if}
    <ul>{#for x in xs}<li>{x}</li>{/for}</ul>
  </template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        // Still keep-node (build/patch dispatch, input patched in place).
        assert!(
            out.contains("fn __build(") && out.contains("fn __patch(") && out.contains("__built"),
            "value-input with control flow stays keep-node:\n{out}"
        );
        // Two regions: the `{#if}` and the `{#for}`, each with anchors +
        // mount/patch methods + a fragment, and both invoked from __patch.
        for needle in [
            "create_comment(\"\").into();",
            "fn __mount_region_0(",
            "fn __patch_region_0(",
            "fn __mount_region_1(",
            "fn __patch_region_1(",
            "create_document_fragment()",
            "self.__patch_region_0();",
            "self.__patch_region_1();",
            "insert_before(&__frag, Some(__e))",
        ] {
            assert!(
                out.contains(needle),
                "region scaffolding `{needle}`:\n{out}"
            );
        }
    }

    #[test]
    fn phase_11_10_derived_emits_cached_cells_and_recompute() {
        // Slice 4 — a `derived` block emits a cached `RefCell` field per value
        // + a `__recompute_derived` method (in declaration order, so a derived
        // reads an earlier one's fresh cell), called from render before the
        // build/patch reads. `{full}` resolves like state.
        let src = r#"component Greeter {
  state { first: Str = "Ada"
          last: Str = "L" }
  derived { full: Str = "{first} {last}"
            greeting: Str = "Hi {full}" }
  event on_first() { first = payload["value"] }
  <template>
    <input @input="on_first" value="{first}" />
    <p>{full}</p>
    <p>{greeting}</p>
  </template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("full: RefCell<String>") && out.contains("greeting: RefCell<String>"),
            "derived cached cells:\n{out}"
        );
        // Zero-init + recompute in declaration order.
        assert!(
            out.contains("full: RefCell::new(String::new())"),
            "derived zero-init:\n{out}"
        );
        let recompute = &out[out
            .find("fn __recompute_derived")
            .expect("recompute method")..];
        let full_at = recompute.find("*self.full.borrow_mut() =").unwrap();
        let greeting_at = recompute.find("*self.greeting.borrow_mut() =").unwrap();
        assert!(
            full_at < greeting_at,
            "full recomputed before greeting (derived-of-derived order):\n{recompute}"
        );
        assert!(
            recompute[greeting_at..].contains("(*self.full.borrow())"),
            "greeting reads the full cell:\n{recompute}"
        );
        // render refreshes derived; {full} reads the cell like state.
        assert!(
            out.contains("self.__recompute_derived();"),
            "render calls recompute:\n{out}"
        );
        assert!(
            out.contains("(*self.full.borrow())"),
            "{{full}} resolves to the cached cell:\n{out}"
        );
    }

    #[test]
    fn phase_11_10_derived_rejects_compound_type() {
        // Only primitive derived (Str/Int/Float/Bool) are supported this slice.
        let src = r#"component X {
  state { xs: List<Int> = [] }
  derived { copy: List<Int> = xs }
  <template><p>done</p></template>
}"#;
        let err = emit_component(&parse_expand(src).components[0]).unwrap_err();
        assert!(
            err.message.contains("compound derived") || err.message.contains("only primitive"),
            "compound derived is rejected with a clear message: {}",
            err.message
        );
    }

    #[test]
    fn phase_11_10_keepnode_composition_stays_naive() {
        // A value-input component that composes (`<slot />` here) still uses
        // the naive re-render — the keep-node path does not drive the child
        // instance cache / slot callbacks.
        let src = r#"component Wrap {
  state { name: Str = "" }
  event on_type() { name = payload["value"] }
  <template>
    <input @input="on_type" value="{name}" />
    <slot />
  </template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            !out.contains("fn __patch(") && !out.contains("__built"),
            "composing value-input stays naive:\n{out}"
        );
    }

    #[test]
    fn phase_11_10_keepnode_not_used_for_non_value_input() {
        // A `@click`-only component (no live input) has no caret to preserve,
        // so it keeps the byte-identical naive re-render.
        let src = r#"component Tap {
  state { n: Int = 0 }
  event bump() { n = n + 1 }
  <template><button @click="bump">{n}</button></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            !out.contains("fn __patch(") && !out.contains("__built"),
            "click-only component stays naive:\n{out}"
        );
    }

    #[test]
    fn phase_11_10_async_worker_renders_before_await_after_state_write() {
        // A state write before an `.await` (the loading pattern) makes the
        // async worker flush a render right before it suspends, so the
        // loading state paints while the request is in flight.
        let src = r#"component Fetcher {
  state { loading: Bool = false
          msg: Str = "" }
  event go() {
    loading = true
    let m = fetch_it().await?
    msg = m
    loading = false
  }
  <template><p>{msg}</p></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        // The worker sets loading then renders BEFORE awaiting fetch_it.
        let worker = async_worker_body(&out, "__go_async");
        let set_loading = worker.find("*self.loading.borrow_mut() = __rhs;").unwrap();
        let mid_render = worker.find("self.render();").unwrap();
        let await_call = worker.find("fetch_it(").unwrap();
        assert!(
            set_loading < mid_render && mid_render < await_call,
            "render() must sit between the state write and the await:\n{worker}"
        );
    }

    #[test]
    fn phase_11_10_async_worker_no_extra_render_without_pre_await_write() {
        // A handler that awaits first (no state write before the await) emits
        // byte-identically: exactly one render, at the end. Guards the
        // byte-compat of the existing rpc example.
        let src = r#"component Plain {
  state { msg: Str = "" }
  event go() {
    let m = fetch_it().await?
    msg = m
  }
  <template><p>{msg}</p></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        let worker = async_worker_body(&out, "__go_async");
        assert_eq!(
            worker.matches("self.render();").count(),
            1,
            "no mid-flight render when nothing is written before the await:\n{worker}"
        );
    }

    /// Slice the emitted `async fn <suffix>(...)` worker body out of a full
    /// module string, bounded to the next method, so render-count assertions
    /// don't spill into `mount_into`/`render`.
    fn async_worker_body<'a>(out: &'a str, suffix: &str) -> &'a str {
        let def = out
            .find(&format!("async fn {}", suffix))
            .expect("async worker definition present");
        let after = &out[def..];
        let end = after.find("\n    pub fn ").unwrap_or(after.len());
        &after[..end]
    }

    #[test]
    fn emit_rejects_handler_with_params() {
        // Direct construction — the surface parser rejects `event
        // f(x) { ... }` syntax before we can test it via source, but
        // the emitter's guard against handler params is worth
        // asserting independently as a defense in depth.
        use crate::ast::{Param, Span};
        use crate::view::ast::Loc;
        let component = ExpandedComponent {
            name: "Foo".to_string(),
            loc: Loc { line: 1, column: 1 },
            hydrate: false,
            state: vec![],
            derived: Vec::new(),
            events: vec![ExpandedEventHandler {
                name: "handle".to_string(),
                params: vec![Param {
                    name: "x".to_string(),
                    type_: Some(TypeExpr::Named("Int".to_string())),
                    default: None,
                    varargs: false,
                    name_span: Span::default(),
                }],
                body: vec![],
                loc: Loc { line: 1, column: 1 },
            }],
            template: None,
            style: None,
        };
        let err = emit_component(&component).unwrap_err();
        assert!(
            err.message.contains("11.5"),
            "handler param error:\n{}",
            err
        );
        assert!(
            err.context.contains("handler `handle`"),
            "context:\n{}",
            err
        );
    }

    // ---- emit_module --------------------------------------------

    #[test]
    fn emit_module_includes_preamble_and_all_components() {
        let expanded = parse_expand(counter_shape_src());
        let out = emit_module(&expanded).unwrap();
        assert!(
            out.starts_with("// Generated by fitz view WASM emitter"),
            "preamble header:\n{}",
            &out[..200.min(out.len())]
        );
        assert!(
            out.contains("use wasm_bindgen::prelude::*;"),
            "wasm_bindgen import:\n{}",
            &out[..500.min(out.len())]
        );
        assert!(
            out.contains("use web_sys::{Event, HtmlElement};"),
            "web_sys import:\n{}",
            &out[..500.min(out.len())]
        );
        // And the component still lands after the preamble
        assert!(
            out.contains("pub struct Counter {"),
            "component decl after preamble present"
        );
    }

    // ---- sanitize_ident + rust_string_literal -------------------

    #[test]
    fn sanitize_ident_replaces_hyphens_with_underscores() {
        assert_eq!(sanitize_ident("counter-c-abc12345"), "counter_c_abc12345");
        assert_eq!(sanitize_ident("Foo"), "Foo");
        assert_eq!(sanitize_ident("with_underscore"), "with_underscore");
    }

    #[test]
    fn rust_string_literal_escapes_quotes_and_newlines() {
        assert_eq!(rust_string_literal("hola"), "\"hola\"");
        assert_eq!(
            rust_string_literal("with \"quote\""),
            "\"with \\\"quote\\\"\""
        );
        assert_eq!(rust_string_literal("line1\nline2"), "\"line1\\nline2\"");
    }

    // ---- Phase 11.5.d — mount_into split + emit_child_component ----

    #[test]
    fn phase_11_5_d_emit_mount_delegates_to_mount_into() {
        let file = parse_expand(counter_shape_src());
        let out = emit_module(&file).unwrap();
        assert!(
            out.contains("pub fn mount(self: &Rc<Self>, selector: &str) -> Result<(), JsValue> {"),
            "public mount(selector) missing:\n{out}"
        );
        assert!(
            out.contains(
                "pub fn mount_into(self: &Rc<Self>, root: HtmlElement) -> Result<(), JsValue> {"
            ),
            "public mount_into(root) missing:\n{out}"
        );
        assert!(
            out.contains("self.mount_into(root)"),
            "mount(selector) must delegate to mount_into:\n{out}"
        );
    }

    #[test]
    fn phase_11_5_d_emit_child_component_creates_wrapper_and_mounts_into() {
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
}"#;
        let file = parse_expand(src);
        let out = emit_module(&file).unwrap();
        // Wrapper div created + classed with `__fitz-child-<Name>`.
        assert!(
            out.contains(r#"document.create_element("div").unwrap();"#),
            "wrapper div creation missing"
        );
        assert!(
            out.contains(r#"set_attribute("class", "__fitz-child-Card")"#),
            "wrapper class must include `__fitz-child-Card`:\n{out}"
        );
        // Phase 11.7.e — child instantiated via the instance-cache
        // slot (get-or-create) instead of a fresh `Card::new()` each
        // render, so its state persists across parent re-renders.
        assert!(
            out.contains("__child_slot_0: RefCell<Option<Rc<Card>>>,"),
            "parent struct must carry an instance-cache slot for the Card site:\n{out}"
        );
        assert!(
            out.contains("let mut __slot = self.__child_slot_0.borrow_mut();")
                && out.contains("if __slot.is_none() { *__slot = Some(Card::new()); }"),
            "child must be instantiated via get-or-create from the cache slot:\n{out}"
        );
        assert!(
            out.contains(r#"*__child1.title.borrow_mut() = "Hello".to_string();"#)
                || out.contains(r#"*__child1.title.borrow_mut() = "Hello""#),
            "title prop must be coerced to a Rust String literal:\n{out}"
        );
        assert!(
            out.contains("*__child1.count.borrow_mut() = 3i64;"),
            "count prop must be coerced to i64:\n{out}"
        );
        assert!(
            out.contains(".mount_into(__el0_html).unwrap();"),
            "child must be mounted via mount_into on the wrapper:\n{out}"
        );
    }

    #[test]
    fn phase_11_5_d_emit_child_component_nullable_null_produces_none() {
        let src = r#"component Parent {
  state {}
  <template><Card n="null" /></template>
}
component Card {
  state { n: Int? = null }
  <template><span>hi</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module(&file).unwrap();
        assert!(
            out.contains("*__child1.n.borrow_mut() = None;"),
            "nullable null must emit None:\n{out}"
        );
    }

    #[test]
    fn phase_11_5_d_emit_child_component_bool_produces_bare_literal() {
        let src = r#"component Parent {
  state {}
  <template><Card active="true" /></template>
}
component Card {
  state { active: Bool = false }
  <template><span>hi</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module(&file).unwrap();
        assert!(
            out.contains("*__child1.active.borrow_mut() = true;"),
            "bool prop must emit bare `true`/`false`:\n{out}"
        );
    }

    // ---------------------------------------------------------------------
    // K-3 — WASM type_expr_to_rust + default_expr_to_rust for List<T>
    // ---------------------------------------------------------------------
    //
    // The view lexer/parser cannot express `List<Str>` in state
    // fields today (the raw `<`/`>` chars land inside `<template>`
    // territory), so these unit tests exercise the type + default
    // lowering helpers directly. The child-prop path in
    // `emit_child_component` already delegates to
    // `check::coerce_child_prop_raw_value`, which the K-3 check
    // tests cover end-to-end (Rust `vec![...]` literal). The WASM
    // helpers below own the STATE-side story: what Rust type the
    // struct field gets and what `RefCell::new(...)` initializer
    // wraps a `List` default expression.

    #[test]
    fn k3_wasm_type_expr_to_rust_list_str_maps_to_vec_string() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::Named("Str".into())],
        };
        assert_eq!(
            type_expr_to_rust(&ty, &NominalRegistry::new()).unwrap(),
            "Vec<String>"
        );
    }

    #[test]
    fn k3_wasm_type_expr_to_rust_list_int_maps_to_vec_i64() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::Named("Int".into())],
        };
        assert_eq!(
            type_expr_to_rust(&ty, &NominalRegistry::new()).unwrap(),
            "Vec<i64>"
        );
    }

    #[test]
    fn k3_wasm_type_expr_to_rust_list_nullable_int_recurses() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::Nullable(Box::new(TypeExpr::Named("Int".into())))],
        };
        assert_eq!(
            type_expr_to_rust(&ty, &NominalRegistry::new()).unwrap(),
            "Vec<Option<i64>>"
        );
    }

    #[test]
    fn s2_wasm_type_expr_to_rust_map_str_int_maps_to_vec_of_tuples() {
        // Was `k3_wasm_type_expr_to_rust_map_still_rejected` pre-S.2.
        // S.2 (2026-07-17) extended `type_expr_to_rust` to accept
        // `Map<K, V>` where K + V are supported primitives — emits
        // `Vec<(K_rust, V_rust)>` matching Fitz's underlying shape.
        // The STATIC PROP COERCION (`check::coerce_child_prop_raw_value`)
        // still restricts to `Map<Str, Str>` because a raw HTML attr
        // can't disambiguate Int vs Str for the value side, but
        // WASM state fields (declared explicitly with a type
        // annotation) work for any primitive-parameterised Map.
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::Named("Str".into()), TypeExpr::Named("Int".into())],
        };
        assert_eq!(
            type_expr_to_rust(&ty, &NominalRegistry::new()).unwrap(),
            "Vec<(String, i64)>"
        );
    }

    #[test]
    fn s2_wasm_type_expr_to_rust_map_str_str_maps_to_vec_of_string_pairs() {
        use crate::ast::TypeExpr;
        let ty = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::Named("Str".into()), TypeExpr::Named("Str".into())],
        };
        assert_eq!(
            type_expr_to_rust(&ty, &NominalRegistry::new()).unwrap(),
            "Vec<(String, String)>"
        );
    }

    #[test]
    fn s2_wasm_default_expr_to_rust_empty_map_produces_vec_new() {
        use crate::ast::{Expr, Span, TypeExpr};
        let ty = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::Named("Str".into()), TypeExpr::Named("Str".into())],
        };
        let default = Expr::Map(Vec::new(), Span::new(1, 1));
        assert_eq!(default_expr_to_rust(&default, &ty).unwrap(), "Vec::new()");
    }

    #[test]
    fn s2_wasm_default_expr_to_rust_non_empty_map_produces_vec_of_tuples() {
        use crate::ast::{Expr, Span, TypeExpr};
        let ty = TypeExpr::Generic {
            name: "Map".into(),
            args: vec![TypeExpr::Named("Str".into()), TypeExpr::Named("Int".into())],
        };
        // Map { "a": 1, "b": 2 } → vec![(..., 1i64), (..., 2i64)]
        let default = Expr::Map(
            vec![
                (
                    Expr::Str("a".to_string(), Span::new(1, 1)),
                    Expr::Int(1, Span::new(1, 1)),
                ),
                (
                    Expr::Str("b".to_string(), Span::new(1, 1)),
                    Expr::Int(2, Span::new(1, 1)),
                ),
            ],
            Span::new(1, 1),
        );
        let lit = default_expr_to_rust(&default, &ty).unwrap();
        assert!(lit.starts_with("vec!["), "got: {lit}");
        assert!(lit.contains("\"a\".to_string()"), "got: {lit}");
        assert!(lit.contains("1i64"), "got: {lit}");
    }

    #[test]
    fn k3_wasm_default_expr_to_rust_empty_list_produces_vec_new() {
        use crate::ast::{Expr, Span, TypeExpr};
        let ty = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::Named("Str".into())],
        };
        let default = Expr::List(Vec::new(), Span::new(1, 1));
        assert_eq!(default_expr_to_rust(&default, &ty).unwrap(), "Vec::new()");
    }

    #[test]
    fn negative_numeric_default_emits_negated_literal() {
        // `-1` / `-0.5` parse as `UnaryOp{Neg, lit}`, not a bare literal.
        let src = r#"component App {
  state {
    progress: Int = -1
    ratio: Float = -0.5
  }
  <template><span>{progress}</span></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("RefCell::new(-1i64)"),
            "negative Int default must emit the negated literal:\n{out}"
        );
        assert!(
            out.contains("RefCell::new(-0.5f64)"),
            "negative Float default must emit the negated literal:\n{out}"
        );
    }

    #[test]
    fn k3_wasm_default_expr_to_rust_list_int_literal_produces_vec_macro() {
        use crate::ast::{Expr, Span, TypeExpr};
        let ty = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::Named("Int".into())],
        };
        let default = Expr::List(
            vec![
                Expr::Int(1, Span::new(1, 1)),
                Expr::Int(2, Span::new(1, 1)),
                Expr::Int(3, Span::new(1, 1)),
            ],
            Span::new(1, 1),
        );
        assert_eq!(
            default_expr_to_rust(&default, &ty).unwrap(),
            "vec![1i64, 2i64, 3i64]"
        );
    }

    #[test]
    fn k3_wasm_default_expr_to_rust_list_str_literal_produces_vec_of_strings() {
        use crate::ast::{Expr, Span, TypeExpr};
        let ty = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::Named("Str".into())],
        };
        let default = Expr::List(
            vec![
                Expr::Str("a".to_string(), Span::new(1, 1)),
                Expr::Str("b".to_string(), Span::new(1, 1)),
            ],
            Span::new(1, 1),
        );
        let lit = default_expr_to_rust(&default, &ty).unwrap();
        assert!(lit.starts_with("vec!["), "got: {lit}");
        assert!(lit.contains("\"a\".to_string()"), "got: {lit}");
        assert!(lit.contains("\"b\".to_string()"), "got: {lit}");
    }

    #[test]
    fn k3_wasm_default_expr_to_rust_list_nullable_int_recurses_via_some_none() {
        use crate::ast::{Expr, Span, TypeExpr};
        let ty = TypeExpr::Generic {
            name: "List".into(),
            args: vec![TypeExpr::Nullable(Box::new(TypeExpr::Named("Int".into())))],
        };
        let default = Expr::List(
            vec![
                Expr::Int(1, Span::new(1, 1)),
                Expr::Null(Span::new(1, 1)),
                Expr::Int(3, Span::new(1, 1)),
            ],
            Span::new(1, 1),
        );
        assert_eq!(
            default_expr_to_rust(&default, &ty).unwrap(),
            "vec![Some(1i64), None, Some(3i64)]"
        );
    }

    // ---------------------------------------------------------------------
    // Phase 11.7.a — WASM accepts interpolated child props (simple case)
    // ---------------------------------------------------------------------
    //
    // The SSR path already accepts `<Child prop={expr} />` (K-3, parent
    // state rewrite via scoping helper). Phase 11.7.a brings the "simple
    // case" to the client-WASM target: a bare parent state field or
    // numeric arithmetic over parent state, into a primitive child
    // field. Under the dirty-flag reactivity model the value is
    // recomputed on every parent re-render, so propagation is reactive
    // for free. Richer targets (nullable / nominal / list) and richer
    // shapes (Str concat, method calls, imports) still defer.

    #[test]
    fn phase_11_7_a_wasm_interpolated_prop_bare_state_field_str() {
        let src = r#"component Parent {
  state { title: Str = "hi" }
  <template><Card label="{title}" /></template>
}
component Card {
  state { label: Str = "" }
  <template><span>hi</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module(&file).expect("simple interpolated Str prop must emit");
        assert!(
            out.contains("(*self.title.borrow()).clone()"),
            "interpolated bare state field must read the parent's RefCell + clone:\n{out}"
        );
        assert!(
            out.contains(".label.borrow_mut() = (*self.title.borrow()).clone();"),
            "the computed value must be assigned into the child's `label` field:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_a_wasm_interpolated_prop_arithmetic_over_state_int() {
        // Uses the arithmetic-lexer-enabled counter helper shape so the
        // view lexer tokenises `+`. `parse_expand` handles it because
        // the arithmetic tokens are enabled since §9.m.
        let src = r#"component Parent {
  state { n: Int = 0 }
  <template><Card count="{n + 1}" /></template>
}
component Card {
  state { count: Int = 0 }
  <template><span>hi</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module(&file).expect("arithmetic interpolated Int prop must emit");
        assert!(
            out.contains(".count.borrow_mut() = ((*self.n.borrow()) + 1i64);"),
            "arithmetic prop must lower to the parent state read + literal:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_a_wasm_interpolated_prop_nullable_target_rejects() {
        let src = r#"component Parent {
  state { title: Str = "hi" }
  <template><Card label="{title}" /></template>
}
component Card {
  state { label: Str? = null }
  <template><span>hi</span></template>
}"#;
        let file = parse_expand(src);
        let err = emit_module(&file)
            .expect_err("nullable child target must defer on the WASM path in 11.7.a");
        assert!(
            err.message.contains("non-primitive / nullable") && err.message.contains("11.7"),
            "message must cite the nullable/non-primitive deferral + 11.7: {}",
            err.message
        );
    }

    #[test]
    fn phase_11_7_a_wasm_interpolated_prop_non_state_ident_rejects() {
        // `mystery` is neither a parent state field nor an imported
        // name — free-var resolution is not part of 11.7.a on the WASM
        // path.
        let src = r#"component Parent {
  state { title: Str = "hi" }
  <template><Card label="{mystery}" /></template>
}
component Card {
  state { label: Str = "" }
  <template><span>hi</span></template>
}"#;
        let file = parse_expand(src);
        let err =
            emit_module(&file).expect_err("non-state ident prop must reject on the WASM path");
        assert!(
            err.message.contains("11.7"),
            "message must point at a later 11.7 slice / SSR: {}",
            err.message
        );
    }

    // ---------------------------------------------------------------------
    // Phase 11.7.e — persistent child state via keyed instance cache
    // ---------------------------------------------------------------------

    #[test]
    fn phase_11_7_e_two_child_sites_get_two_aligned_cache_slots() {
        // Two static <Child /> sites → two typed slots. The slot
        // index the render reads must match the struct field, in DFS
        // order (A before B).
        let src = r#"component Parent {
  state {}
  <template>
    <div>
      <Alpha label="a" />
      <Beta label="b" />
    </div>
  </template>
}
component Alpha {
  state { label: Str = "" }
  <template><span>a</span></template>
}
component Beta {
  state { label: Str = "" }
  <template><span>b</span></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module(&file).expect("two child sites must emit");
        assert!(
            out.contains("__child_slot_0: RefCell<Option<Rc<Alpha>>>,"),
            "site 0 slot must be typed to Alpha:\n{out}"
        );
        assert!(
            out.contains("__child_slot_1: RefCell<Option<Rc<Beta>>>,"),
            "site 1 slot must be typed to Beta:\n{out}"
        );
        assert!(
            out.contains("*__slot = Some(Alpha::new());")
                && out.contains("*__slot = Some(Beta::new());"),
            "both children must be get-or-created from their slot:\n{out}"
        );
        // new() initialises both slots to None. Total `RefCell::new(None)`:
        // Parent = 2 child slots + 1 root; Alpha = 1 root; Beta = 1 root → 5.
        assert_eq!(
            out.matches("RefCell::new(None),").count(),
            5,
            "each component's new() inits its child slots + root to None:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_e_no_child_sites_emits_no_slots() {
        // Regression: a component with no <Child /> must not gain any
        // `__child_slot_*` field (counter stays at 0).
        let src = r#"component Solo {
  state { n: Int = 0 }
  event tick() { n = n + 1 }
  <template><button @click="tick">{n}</button></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module(&file).expect("no-child component must emit");
        assert!(
            !out.contains("__child_slot_"),
            "a component with no <Child /> must emit no cache slots:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_a_wasm_static_prop_path_unchanged() {
        // Regression: the static coerce-to-literal path is untouched.
        let src_static = r#"component Parent {
  state {}
  <template><Card label="hi" /></template>
}
component Card {
  state { label: Str = "" }
  <template><span>hi</span></template>
}"#;
        let file_static = parse_expand(src_static);
        let out = emit_module(&file_static).expect("static path must still work");
        assert!(
            out.contains(r#".label.borrow_mut() = "hi".to_string();"#),
            "static prop must still coerce to a Rust literal:\n{out}"
        );
    }

    // ---------------------------------------------------------------------
    // Phase 11.7.b R2b — keyed `<Child />` composition inside `{#for}`
    // ---------------------------------------------------------------------

    /// Canonical R2b shape: a `<Child key="{x}" prop="{x}" />` inside a
    /// `{#for x in items}` over `List<Str>` emits a keyed instance
    /// cache (`__child_map_0`), a per-render `__seen_0` set, the
    /// get-or-create via the map's `entry(...)` API, and a
    /// reconciliation `retain` after the loop.
    fn keyed_for_src() -> &'static str {
        r#"component Board {
  state { tags: List<Str> = ["a", "b", "c"] }
  <template>
    <ul>
      {#for x in tags}
        <Card key="{x}" label="{x}" />
      {/for}
    </ul>
  </template>
}
component Card {
  state {
    label: Str = ""
    taps: Int = 0
  }
  event tap() { taps = taps + 1 }
  <template><li>{label} ({taps})</li></template>
}"#
    }

    #[test]
    fn phase_11_7_b_r2b_dynamic_child_emits_keyed_map_field() {
        let file = parse_expand(keyed_for_src());
        let out = emit_module(&file).expect("keyed for must emit");
        assert!(
            out.contains("__child_map_0: RefCell<std::collections::HashMap<String, Rc<Card>>>,"),
            "a `<Child />` inside `{{#for}}` must get a keyed instance cache field:\n{out}"
        );
        assert!(
            out.contains("__child_map_0: RefCell::new(std::collections::HashMap::new()),"),
            "new() must init the keyed map empty:\n{out}"
        );
        // A dynamic site must NOT also get a static slot.
        assert!(
            !out.contains("__child_slot_"),
            "a dynamic site must not emit a static slot:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_b_r2b_dynamic_child_emits_seen_set_and_retain() {
        let file = parse_expand(keyed_for_src());
        let out = emit_module(&file).expect("keyed for must emit");
        assert!(
            out.contains("let mut __seen_0 = std::collections::HashSet::<String>::new();"),
            "the enclosing for must declare a per-render seen set:\n{out}"
        );
        assert!(
            out.contains(r#"let __key = format!("{}", x);"#),
            "the key must lower to `format!(\"{{}}\", x)` from the loop var:\n{out}"
        );
        assert!(
            out.contains("__seen_0.insert(__key.clone());"),
            "each rendered child must record its key as seen:\n{out}"
        );
        assert!(
            out.contains("__map.entry(__key.clone()).or_insert_with(|| Card::new()).clone()"),
            "the child must be get-or-created through the keyed map:\n{out}"
        );
        assert!(
            out.contains(
                "self.__child_map_0.borrow_mut().retain(|__k, _| __seen_0.contains(__k));"
            ),
            "after the loop, vanished keys must be evicted (reconciliation):\n{out}"
        );
    }

    #[test]
    fn phase_11_7_b_r2b_dynamic_child_without_key_rejects() {
        let src = r#"component Board {
  state { tags: List<Str> = ["a"] }
  <template>
    <ul>
      {#for x in tags}
        <Card label="{x}" />
      {/for}
    </ul>
  </template>
}
component Card {
  state { label: Str = "" }
  <template><li>{label}</li></template>
}"#;
        let file = parse_expand(src);
        let err = emit_module(&file).expect_err("a keyless dynamic child must reject");
        assert!(
            err.message.contains("key=") && err.message.contains("{#for}"),
            "the error must ask for a `key` inside `{{#for}}`:\n{}",
            err.message
        );
    }

    #[test]
    fn phase_11_7_b_r2b_static_key_attr_rejects_at_expand() {
        // `key="literal"` (not interpolated) is a mistake — the key is
        // meant to be the loop variable. Rejected at expand time.
        let src = r#"component Board {
  state { tags: List<Str> = ["a"] }
  <template>
    <ul>
      {#for x in tags}
        <Card key="static" label="{x}" />
      {/for}
    </ul>
  </template>
}
component Card {
  state { label: Str = "" }
  <template><li>{label}</li></template>
}"#;
        let raw = parse(src).expect("parse ok");
        let err = expand(&raw).expect_err("static key must reject at expand");
        assert!(
            err.to_string().contains("key="),
            "expand must reject a static key attribute:\n{err}"
        );
    }

    #[test]
    fn phase_11_7_b_r2b_static_and_dynamic_sites_get_aligned_indices() {
        // A static `<Header />` (slot 0) followed by a `{#for}` with a
        // dynamic `<Card />` (map 0): the two index spaces are
        // independent and each field is typed to its own child.
        let src = r#"component Board {
  state { tags: List<Str> = ["a"] }
  <template>
    <div>
      <Header label="hi" />
      <ul>
        {#for x in tags}
          <Card key="{x}" label="{x}" />
        {/for}
      </ul>
    </div>
  </template>
}
component Header {
  state { label: Str = "" }
  <template><h1>{label}</h1></template>
}
component Card {
  state { label: Str = "" }
  <template><li>{label}</li></template>
}"#;
        let file = parse_expand(src);
        let out = emit_module(&file).expect("mixed sites must emit");
        assert!(
            out.contains("__child_slot_0: RefCell<Option<Rc<Header>>>,"),
            "the static site must be slot 0 typed to Header:\n{out}"
        );
        assert!(
            out.contains("__child_map_0: RefCell<std::collections::HashMap<String, Rc<Card>>>,"),
            "the dynamic site must be map 0 typed to Card:\n{out}"
        );
        assert!(
            out.contains("if __slot.is_none() {") && out.contains("Header::new()"),
            "the static child must still get-or-create from its slot:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_b_r2b_for_without_child_emits_no_seen_set() {
        // Regression: a `{#for}` over `List<primitive>` with no child
        // composition (the 11.7.b control-flow shape) must NOT gain a
        // seen set / map / retain.
        let src = r#"component App {
  state { labels: List<Str> = ["a", "b"] }
  <template>
    <ul>
      {#for label in labels}
        <li>{label}</li>
      {/for}
    </ul>
  </template>
}"#;
        let file = parse_expand(src);
        let out = emit_module(&file).expect("plain for must emit");
        assert!(
            !out.contains("__seen_") && !out.contains("__child_map_"),
            "a for without child composition must not reconcile:\n{out}"
        );
    }

    // ---- Phase 11.7 R3.5a.1 — richer expr/stmt lowerer -------------

    #[test]
    fn phase_11_7_r3_5_a1_map_closure_lowers_to_iterator_chain() {
        let src = r#"component Nums {
  state { nums: List<Int> = [1, 2, 3] }
  event double_all() { nums = nums.map(fn(n) => n * 2) }
  <template><ul>{#for n in nums}<li>{n}</li>{/for}</ul></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains(".clone().into_iter().map(|n| (n * 2i64)).collect::<Vec<_>>()"),
            "`.map` closure must lower to an iterator chain:\n{out}"
        );
        // The reassignment writes the collected Vec back into the state
        // RefCell via the `__rhs` snapshot (no double-borrow).
        assert!(
            out.contains("let __rhs =") && out.contains("*self.nums.borrow_mut() = __rhs;"),
            "list reassignment must go through __rhs then borrow_mut:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_a1_filter_closure_clones_ref_into_owned_param() {
        let src = r#"component Nums {
  state { nums: List<Int> = [1, 2, 3, 4] }
  event keep_big() { nums = nums.filter(fn(n) => n > 1) }
  <template><ul>{#for n in nums}<li>{n}</li>{/for}</ul></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains(
                ".clone().into_iter().filter(|__it| { let n = __it.clone(); (n > 1i64) }).collect::<Vec<_>>()"
            ),
            "`.filter` must clone the &T param into an owned binding:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_a1_len_lowers_to_usize_cast_i64() {
        // `.len()` in an interpolation — the classic Fitz `.len()` returns
        // Int, so the WASM lowering casts `usize` → `i64`.
        let src = r#"component Nums {
  state { nums: List<Int> = [1, 2, 3] }
  event noop() { nums = nums }
  <template><p>{nums.len()}</p></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("((*self.nums.borrow())).len() as i64"),
            "`.len()` must cast to i64:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_a1_list_literal_reassignment_emits_vec_macro() {
        let src = r#"component Nums {
  state { nums: List<Int> = [9] }
  event reset() { nums = [1, 2, 3] }
  <template><ul>{#for n in nums}<li>{n}</li>{/for}</ul></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("let __rhs = vec![1i64, 2i64, 3i64];"),
            "list-literal reassignment must emit a `vec!` macro:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_a1_let_binding_is_in_scope_for_a_later_closure() {
        let src = r#"component Nums {
  state { nums: List<Int> = [1, 2] }
  event bump_by() {
    let k = 10
    nums = nums.map(fn(n) => n + k)
  }
  <template><ul>{#for n in nums}<li>{n}</li>{/for}</ul></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("let k = 10i64;"),
            "a non-state assign must lower to a Rust `let` binding:\n{out}"
        );
        assert!(
            out.contains(".map(|n| (n + k))"),
            "the closure must capture the local `k` introduced earlier:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_a1_for_over_method_call_uses_into_iter() {
        // `{#for}` over a general (non-state-field) iterable takes the
        // lower-then-`.into_iter()` path.
        let src = r#"component Nums {
  state { nums: List<Int> = [1, 2, 3, 4] }
  event noop() { nums = nums }
  <template><ul>{#for big in nums.filter(fn(n) => n > 2)}<li>{big}</li>{/for}</ul></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains(
                ".filter(|__it| { let n = __it.clone(); (n > 2i64) }).collect::<Vec<_>>();"
            ),
            "the for-over-call iterable must lower the method chain:\n{out}"
        );
        assert!(
            out.contains(".into_iter() {"),
            "a general `{{#for}}` iterable must iterate with `.into_iter()`:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_a1_if_as_expression_lowers_to_rust_if() {
        // `let x = if (cond) { a } else { b }` — if used as a value.
        let src = r#"component Toggle {
  state { count: Int = 0 }
  event step() {
    let next = if (count > 0) { 0 } else { 1 }
    count = next
  }
  <template><span>{count}</span></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("let next = (if ((*self.count.borrow()) > 0i64) { 0i64 } else { 1i64 });"),
            "if-as-expression must lower to a parenthesised Rust if:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_a1_statement_if_guard_lowers_to_rust_if() {
        let src = r#"component Guard {
  state { count: Int = 0 }
  event maybe_reset() {
    if (count > 5) {
      count = 0
    }
  }
  <template><span>{count}</span></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("if ((*self.count.borrow()) > 5i64) {"),
            "a statement-position `if` must lower to a Rust `if` block:\n{out}"
        );
    }

    // ---- Phase 11.7 R3.5a.2 — imported classic-fn transpilation -----

    /// Build an `ImportedFnRegistry` from a classic-Fitz source snippet,
    /// mirroring `wasm_build::load_imported_fns` but from a string (no
    /// disk). Used by the emit-side unit tests.
    fn fns_from_classic(src: &str) -> ImportedFnRegistry {
        let tokens = crate::lexer::tokenize(src).expect("tokenize classic snippet");
        let program = crate::parser::parse(tokens).expect("parse classic snippet");
        let mut reg = ImportedFnRegistry::new();
        for stmt in &program {
            if let Stmt::FnDef {
                name,
                params,
                return_type,
                body,
                decorators,
                ..
            } = stmt
            {
                let params = params
                    .iter()
                    .map(|p| (p.name.clone(), p.type_.clone()))
                    .collect();
                let is_rpc = decorators.iter().any(|d| d.name == "rpc");
                reg.insert(
                    name.clone(),
                    params,
                    return_type.clone(),
                    body.clone(),
                    is_rpc,
                );
            }
        }
        reg
    }

    fn single_component_file(src: &str) -> ExpandedViewFile {
        parse_expand(src)
    }

    #[test]
    fn phase_11_7_r3_5_a2_imported_fn_emitted_as_rust_fn() {
        let file = single_component_file(
            r#"component App {
  state { x: Int = 0 }
  <template><span>{x}</span></template>
}"#,
        );
        let fns = fns_from_classic("fn double(n: Int) -> Int { return n * 2 }");
        let out = emit_module_with_imports(&file, &NominalRegistry::new(), &fns).unwrap();
        assert!(
            out.contains("#[allow(dead_code)]\nfn double(n: i64) -> i64 {"),
            "the imported fn must emit with mapped param/return types:\n{out}"
        );
        assert!(
            out.contains("return (n * 2i64);"),
            "the imported fn body must lower:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_a2_imported_fn_missing_param_type_rejects() {
        let file = single_component_file(
            r#"component App {
  state { x: Int = 0 }
  <template><span>{x}</span></template>
}"#,
        );
        let fns = fns_from_classic("fn f(n) -> Int { return 1 }");
        let err = emit_module_with_imports(&file, &NominalRegistry::new(), &fns).unwrap_err();
        assert!(
            err.message.contains("param `n` needs a type annotation"),
            "an un-annotated param must reject:\n{err}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_a2_imported_fn_missing_return_type_rejects() {
        let file = single_component_file(
            r#"component App {
  state { x: Int = 0 }
  <template><span>{x}</span></template>
}"#,
        );
        let fns = fns_from_classic("fn f(n: Int) { return n }");
        let err = emit_module_with_imports(&file, &NominalRegistry::new(), &fns).unwrap_err();
        assert!(
            err.message.contains("needs a return-type annotation"),
            "a missing return type must reject:\n{err}"
        );
    }

    #[test]
    fn cw9_1a_imported_fn_result_return_with_try_and_ok() {
        // CW.9 (1a) — a helper with a `Result<Int>` return type, `?`
        // propagation on a `Result` param, and an `Ok(...)` constructor.
        // `Result<T>` maps to `Result<T, String>`; `?` and `Ok` lower.
        let file = single_component_file(
            r#"component App {
  state { x: Int = 0 }
  <template><span>{x}</span></template>
}"#,
        );
        let fns = fns_from_classic(
            "fn checked_double(inner: Result<Int>) -> Result<Int> {\n  let n = inner?\n  return Ok(n * 2)\n}",
        );
        let out = emit_module_with_imports(&file, &NominalRegistry::new(), &fns).unwrap();
        assert!(
            out.contains("fn checked_double(inner: Result<i64, String>) -> Result<i64, String> {"),
            "Result<T> must map to Result<T, String> in param + return:\n{out}"
        );
        assert!(
            out.contains("let n = inner?;"),
            "`?` must propagate:\n{out}"
        );
        assert!(
            out.contains("return Ok((n * 2i64));"),
            "`Ok(...)` constructor must lower:\n{out}"
        );
    }

    #[test]
    fn cw9_1a_imported_fn_match_ok_err_arms_bind_inner() {
        // CW.9 (1a) — a `match` on a `Result` param whose arms bind the
        // inner (`Ok(v)` / `Err(e)`) AND reference the binding in the arm
        // body. The binding must be threaded into the arm-body scope.
        let file = single_component_file(
            r#"component App {
  state { x: Int = 0 }
  <template><span>{x}</span></template>
}"#,
        );
        let fns = fns_from_classic(
            "fn describe(r: Result<Int>) -> Str { return match r { Ok(v) => \"value {v}\", Err(e) => e } }",
        );
        let out = emit_module_with_imports(&file, &NominalRegistry::new(), &fns).unwrap();
        assert!(
            out.contains("Ok(v) => format!(\"value {}\", v)"),
            "Ok(v) binding must be visible in the arm body:\n{out}"
        );
        assert!(
            out.contains("Err(e) => e"),
            "Err(e) binding must be visible in the arm body:\n{out}"
        );
    }

    #[test]
    fn cw9_1a_result_return_with_err_constructor_in_match_arm() {
        // CW.9 (1a) — a guard helper that returns `Err("...")` in one arm
        // and `Ok(...)` in the other. Exercises the `Err` constructor with
        // a Str literal (owned `String`, matching the pinned error type).
        let file = single_component_file(
            r#"component App {
  state { x: Int = 0 }
  <template><span>{x}</span></template>
}"#,
        );
        let fns = fns_from_classic(
            "fn guard(n: Int) -> Result<Int> { return match n == 0 { true => Err(\"zero\"), false => Ok(n) } }",
        );
        let out = emit_module_with_imports(&file, &NominalRegistry::new(), &fns).unwrap();
        assert!(
            out.contains("-> Result<i64, String> {"),
            "Result<Int> return maps to Result<i64, String>:\n{out}"
        );
        assert!(
            out.contains("true => Err(\"zero\".to_string())"),
            "Err(\"...\") lowers to an owned String:\n{out}"
        );
        assert!(
            out.contains("false => Ok(n)"),
            "Ok(n) lowers in the match arm:\n{out}"
        );
    }

    // ---- Phase 11.11.c — `@rpc` client stubs + async handlers -------

    #[test]
    fn phase_11_11_rpc_fn_emits_async_fetch_stub_and_helper() {
        let file = single_component_file(
            r#"component App {
  state { x: Int = 0 }
  <template><span>{x}</span></template>
}"#,
        );
        let fns = fns_from_classic(
            "@rpc\nasync fn greet(name: Str) -> Result<Str> { return Ok(\"hi\") }",
        );
        let out = emit_module_with_imports(&file, &NominalRegistry::new(), &fns).unwrap();
        // The shared fetch runtime is emitted once.
        assert_eq!(
            out.matches("async fn __fitz_fetch_post(").count(),
            1,
            "the fetch helper must be emitted exactly once:\n{out}"
        );
        // The stub is an async fn returning Result<T, String>, NOT a
        // transpiled body.
        assert!(
            out.contains("async fn greet(name: String) -> Result<String, String> {"),
            "the rpc stub signature:\n{out}"
        );
        assert!(
            out.contains("__fitz_fetch_post(\"/__rpc/greet\", &__body).await?"),
            "the stub must POST to /__rpc/greet:\n{out}"
        );
        assert!(
            out.contains("serde_json::from_str::<String>(&__text)"),
            "the 200 branch must deserialize T:\n{out}"
        );
    }

    #[test]
    fn phase_11_11_non_rpc_fn_still_transpiles_and_no_fetch_helper() {
        let file = single_component_file(
            r#"component App {
  state { x: Int = 0 }
  <template><span>{x}</span></template>
}"#,
        );
        let fns = fns_from_classic("fn double(n: Int) -> Int { return n * 2 }");
        let out = emit_module_with_imports(&file, &NominalRegistry::new(), &fns).unwrap();
        assert!(
            out.contains("fn double(n: i64) -> i64 {") && out.contains("return (n * 2i64);"),
            "a non-rpc fn must still transpile its body:\n{out}"
        );
        assert!(
            !out.contains("__fitz_fetch_post"),
            "no rpc → no fetch helper:\n{out}"
        );
    }

    #[test]
    fn phase_11_11_async_event_handler_wraps_in_spawn_local() {
        let src = r#"component App {
  state { who: Str = "world", msg: Str = "" }
  event go() {
    let m = greet(who).await?
    msg = m
  }
  <template><span>{msg}</span><button @click="go">go</button></template>
}"#;
        let file = single_component_file(src);
        let fns = fns_from_classic(
            "@rpc\nasync fn greet(name: Str) -> Result<Str> { return Ok(\"hi\") }",
        );
        let out = emit_module_with_imports(&file, &NominalRegistry::new(), &fns).unwrap();
        // Sync wrapper spawns the async worker.
        assert!(
            out.contains("wasm_bindgen_futures::spawn_local(async move {"),
            "an awaiting handler must spawn_local:\n{out}"
        );
        assert!(
            out.contains("async fn __go_async(self: Rc<Self>) -> Result<(), String> {"),
            "the async worker takes an owned Rc<Self> and returns Result:\n{out}"
        );
        assert!(
            out.contains(".await?"),
            "the awaited call keeps its .await?:\n{out}"
        );
    }

    #[test]
    fn phase_11_11_nominal_gets_serde_derives_when_rpc() {
        let file = single_component_file(
            r#"component App {
  state { x: Int = 0 }
  <template><span>{x}</span></template>
}"#,
        );
        let mut nominals = NominalRegistry::new();
        nominals.insert(
            "User".to_string(),
            vec![
                (
                    "id".to_string(),
                    crate::ast::TypeExpr::Named("Int".to_string()),
                ),
                (
                    "name".to_string(),
                    crate::ast::TypeExpr::Named("Str".to_string()),
                ),
            ],
        );
        let fns = fns_from_classic(
            "@rpc\nasync fn get_user(id: Int) -> Result<User> { return Err(\"x\") }",
        );
        let out = emit_module_with_imports(&file, &nominals, &fns).unwrap();
        assert!(
            out.contains("#[derive(Clone, serde::Serialize, serde::Deserialize)]"),
            "nominals cross the wire under rpc, so they get serde derives:\n{out}"
        );
    }

    #[test]
    fn phase_11_11_nominal_no_serde_derive_without_rpc() {
        let file = single_component_file(
            r#"component App {
  state { x: Int = 0 }
  <template><span>{x}</span></template>
}"#,
        );
        let mut nominals = NominalRegistry::new();
        nominals.insert(
            "User".to_string(),
            vec![(
                "id".to_string(),
                crate::ast::TypeExpr::Named("Int".to_string()),
            )],
        );
        let out = emit_module_with_nominals(&file, &nominals).unwrap();
        assert!(
            out.contains("#[derive(Clone)]") && !out.contains("serde::Serialize"),
            "without rpc the nominal stays a plain Clone struct:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_a2_free_fn_call_clones_ident_args_in_map() {
        // `nums = nums.map(fn(n) => bump(n))` — the free-fn arg (a bare
        // ident) is `.clone()`d so a captured value survives the FnMut.
        let src = r#"component App {
  state { nums: List<Int> = [1, 2] }
  event go() { nums = nums.map(fn(n) => bump(n)) }
  <template><ul>{#for n in nums}<li>{n}</li>{/for}</ul></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains(".map(|n| bump(n.clone())).collect::<Vec<_>>()"),
            "a free-fn call inside .map must clone its ident argument:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_a2_free_fn_call_in_push_arg() {
        let src = r#"component App {
  state {
    nums: List<Int> = []
    next: Int = 1
  }
  event add() { nums.push(seed(next)) }
  <template><ul>{#for n in nums}<li>{n}</li>{/for}</ul></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("self.nums.borrow_mut().push(seed((*self.next.borrow()).clone()));"),
            "a free-fn call as a push arg must clone the state-field ident:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_a2_for_over_imported_call() {
        // `{#for n in make_nums()}` — a `{#for}` over a free-fn call
        // result takes the general `.into_iter()` path.
        let src = r#"component App {
  state { nums: List<Int> = [] }
  event noop() { nums = nums }
  <template><ul>{#for n in make_nums()}<li>{n}</li>{/for}</ul></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("= make_nums();") && out.contains(".into_iter() {"),
            "a {{#for}} over a free-fn call must snapshot + .into_iter():\n{out}"
        );
    }

    #[test]
    fn keyed_for_emits_data_flv_key_set_attribute_for_parity() {
        // `{#for r in rows key=r}` — the injected `data-flv-key="{r}"`
        // interpolation attr rides through expand into the WASM target and
        // emits a `set_attribute("data-flv-key", ...)` on the list item DOM
        // node (parity with the SSR target, harmless for WASM).
        let src = r#"component App {
  state { rows: List<Str> = [] }
  event noop() { rows = rows }
  <template><ul>{#for r in rows key=r}<li>{r}</li>{/for}</ul></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("set_attribute(\"data-flv-key\""),
            "keyed for must set data-flv-key on the item element:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_a2_empty_registry_emits_no_fns() {
        // Byte-identical guard — with no imported fns, no `fn` is emitted.
        let file = single_component_file(
            r#"component App {
  state { x: Int = 0 }
  <template><span>{x}</span></template>
}"#,
        );
        let with_empty =
            emit_module_with_imports(&file, &NominalRegistry::new(), &ImportedFnRegistry::new())
                .unwrap();
        let with_nominals = emit_module_with_nominals(&file, &NominalRegistry::new()).unwrap();
        assert_eq!(
            with_empty, with_nominals,
            "an empty fn registry must match the nominals-only path byte-for-byte"
        );
    }

    // ---- Phase 11.7 R3.5b.1 — click payload ------------------------

    #[test]
    fn phase_11_7_r3_5_b1_payload_handler_gets_param() {
        let src = r#"component App {
  state { last: Str = "none" }
  event pick() { last = payload["val"] }
  <template><span>{last}</span></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains(
                "fn pick(self: &Rc<Self>, payload: &std::collections::HashMap<String, String>) {"
            ),
            "a payload-using handler must take a payload param:\n{out}"
        );
        assert!(
            out.contains("payload.get(&(\"val\".to_string())).cloned().unwrap_or_default()"),
            "payload[key] must lower to a safe get:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_b1_non_payload_handler_keeps_zero_arg() {
        let src = r#"component App {
  state { n: Int = 0 }
  event bump() { n = n + 1 }
  <template><button @click="bump">{n}</button></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("fn bump(self: &Rc<Self>) {"),
            "a handler that doesn't read payload keeps the zero-arg signature:\n{out}"
        );
        assert!(
            !out.contains("fn bump(self: &Rc<Self>, payload:"),
            "no payload param should be added:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_b1_payload_has_lowers_in_guard() {
        let src = r#"component App {
  state { last: Str = "none" }
  event pick() {
    if (payload.has("val")) {
      last = payload["val"]
    }
  }
  <template><span>{last}</span></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("if payload.contains_key(&(\"val\".to_string())) {"),
            "payload.has must lower to contains_key in the guard condition:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_b1_interpolated_attr_is_set() {
        let src = r#"component App {
  state { nums: List<Int> = [1, 2] }
  event noop() { nums = nums }
  <template><ul>{#for n in nums}<li data-x="{n}">{n}</li>{/for}</ul></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains(".set_attribute(\"data-x\", &format!(\"{}\", n)).unwrap();"),
            "an interpolated attr must be set from the lowered expr:\n{out}"
        );
    }

    #[test]
    fn mixed_attr_interpolation_lowers_to_format_set_attribute() {
        // CW.9 — `style="width: {pct}%"` (literal + {expr} segments) lowers to a
        // `format!` interleaving the literals with `{}` for the interpolated expr.
        let src = r#"component App {
  state { pct: Int = 40 }
  <template><div class="bar"><div class="fill" style="width: {pct}%"></div></div></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains(
                r#".set_attribute("style", &format!("width: {}%", (*self.pct.borrow()))).unwrap();"#
            ),
            "mixed attr interp must lower to a format! set_attribute:\n{out}"
        );
    }

    #[test]
    fn wasm_fn_body_for_match_reassign_and_string_concat() {
        // CW.9 — a helper-style body using a range `for`, a `match` expression,
        // a reassigned local accumulator, and string concatenation. All four
        // constructs lower on the client-WASM target (exercised via an event
        // body, which shares the same `lower_stmt`/`lower_expr` path).
        let src = r#"component App {
  state { out: Str = "" }
  event build() {
    let acc = ""
    for n in 1..4 {
      let label = match n == 2 {
        true => "two",
        false => "x",
      }
      acc = acc + "{label}"
    }
    out = acc
  }
  <template><button @click="build">{out}</button></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        // range `for`
        assert!(
            out.contains("for n in 1i64..4i64 {"),
            "for-range loop:\n{out}"
        );
        // accumulator declared `let mut` because it is reassigned later
        assert!(
            out.contains("let mut acc = "),
            "reassigned local is let mut:\n{out}"
        );
        // reassignment (not a shadowing `let`) with a format! string concat
        assert!(
            out.contains("acc = format!(\"{}{}\", acc,"),
            "string concat reassignment:\n{out}"
        );
        // match expression as a value
        assert!(
            out.contains("match (n == 2i64) { true => "),
            "match expression:\n{out}"
        );
    }

    #[test]
    fn wasm_str_methods_lower() {
        // Str methods (parity with classic Fitz / SSR) lower on the wasm
        // target: upper/lower/trim → String, contains/starts_with/ends_with →
        // bool, replace → String.
        let src = r#"component S {
  state { name: Str = "ada" }
  <template>
    <p>{name.upper()}</p>
    <p>{name.lower()}</p>
    <p>{name.trim()}</p>
    <p>{name.replace("a", "b")}</p>
  </template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(out.contains(".to_uppercase()"), "upper:\n{out}");
        assert!(out.contains(".to_lowercase()"), "lower:\n{out}");
        assert!(out.contains(".trim().to_string()"), "trim:\n{out}");
        assert!(out.contains(".replace("), "replace:\n{out}");
    }

    #[test]
    fn wasm_case_insensitive_filter_lowers() {
        // The previously-blocked case: a `.filter` closure with `.lower()`,
        // `.contains()`, and a logical `or` — now compiles on the wasm target.
        let src = r#"component FilterList {
  state {
    names: List<Str> = ["Ada"]
    q: Str = ""
  }
  event on_filter() { q = payload["value"] }
  <template>
    <input @input="on_filter" value="{q}" />
    <ul>
      {#for it in names.filter(fn(x) => q == "" or x.lower().contains(q.lower()))}
        <li>{it}</li>
      {/for}
    </ul>
  </template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        // `.lower()` on both sides + `.contains(...)` + `or` → `||`.
        assert!(out.contains(".to_lowercase()"), "lower in filter:\n{out}");
        assert!(out.contains(".contains("), "contains in filter:\n{out}");
        assert!(out.contains(" || "), "logical or lowers to ||:\n{out}");
    }

    #[test]
    fn phase_11_7_r3_5_b1_data_flv_click_wires_listener_and_reads_value() {
        let src = r#"component App {
  state {
    nums: List<Int> = [1, 2]
    last: Str = "none"
  }
  event pick() { last = payload["val"] }
  <template><ul>{#for n in nums}<li><button data-flv-click="pick" data-flv-value-val="{n}">{n}</button></li>{/for}</ul></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("let __evt_el =") && out.contains(
                "__payload.insert(\"val\".to_string(), __evt_el.get_attribute(\"data-flv-value-val\").unwrap_or_default());"
            ),
            "the click listener must read the value attr into the payload:\n{out}"
        );
        assert!(
            out.contains("App::pick(&__self_clone, &__payload);"),
            "the listener must call the handler with the payload:\n{out}"
        );
        // The `data-flv-click` directive itself is NOT set as a DOM attr.
        assert!(
            !out.contains(".set_attribute(\"data-flv-click\""),
            "data-flv-click is a directive, not a DOM attribute:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_b1_data_flv_click_unknown_handler_rejects() {
        let src = r#"component App {
  state { x: Int = 0 }
  event real() { x = 1 }
  <template><button data-flv-click="typo">go</button></template>
}"#;
        let err = emit_component(&parse_expand(src).components[0]).unwrap_err();
        assert!(
            err.message.contains("typo") && err.message.contains("not an `event`"),
            "a data-flv-click to an unknown handler must reject:\n{err}"
        );
    }

    // ---- Phase 11.7 R3.5b.2 — form-submit payload ------------------

    #[test]
    fn phase_11_7_r3_5_b2_form_submit_wires_listener_reads_field_and_clears() {
        let src = r#"component App {
  state { items: List<Str> = [] }
  event add() { items.push(payload["text"]) }
  <template>
    <form data-flv-submit="add">
      <input name="text" data-flv-clear />
      <button>Add</button>
    </form>
  </template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("let __form_el =") && out.contains("__evt.prevent_default();"),
            "the submit listener must capture the form and preventDefault:\n{out}"
        );
        assert!(
            out.contains("__form_el.query_selector(\"[name=\\\"text\\\"]\").ok().flatten()")
                && out.contains("__payload.insert(\"text\".to_string(), __inp.value());"),
            "the field value must be read into the payload:\n{out}"
        );
        assert!(
            out.contains("App::add(&__self_clone, &__payload);"),
            "the handler must be called with the payload:\n{out}"
        );
        assert!(
            out.contains("__inp.set_value(\"\");"),
            "a data-flv-clear input must be reset after submit:\n{out}"
        );
        assert!(
            out.contains("add_event_listener_with_callback(\"submit\""),
            "the listener must attach to the submit event:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_b2_form_without_clear_does_not_reset() {
        let src = r#"component App {
  state { items: List<Str> = [] }
  event add() { items.push(payload["text"]) }
  <template>
    <form data-flv-submit="add">
      <input name="text" />
      <button>Add</button>
    </form>
  </template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            !out.contains("set_value(\"\")"),
            "an input without data-flv-clear must not be reset:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_b2_extra_web_sys_feature_only_when_form_present() {
        let form = parse_expand(
            r#"component App {
  state { items: List<Str> = [] }
  event add() { items.push(payload["text"]) }
  <template><form data-flv-submit="add"><input name="text" /></form></template>
}"#,
        );
        assert_eq!(
            wasm_extra_web_sys_features(&form),
            vec!["HtmlInputElement"],
            "a form component needs the HtmlInputElement feature"
        );

        let no_form = parse_expand(
            r#"component App {
  state { n: Int = 0 }
  <template><span>{n}</span></template>
}"#,
        );
        assert!(
            wasm_extra_web_sys_features(&no_form).is_empty(),
            "a form-free component needs no extra features"
        );
    }

    // ---- CW.9 gap #4 — file input (`data-flv-file`) ----------------

    #[test]
    fn data_flv_file_wires_change_listener_and_filereader() {
        let src = r#"component App {
  state {
    img: Str = ""
    has: Bool = false
  }
  event on_file() {
    img = payload["data"]
    has = true
  }
  <template>
    <input type="file" data-flv-file="on_file" />
  </template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("web_sys::FileReader::new()") && out.contains("read_as_data_url(&__file)"),
            "must create a FileReader and read the file as a data URL:\n{out}"
        );
        assert!(
            out.contains("__payload.insert(\"data\".to_string()")
                && out.contains("__payload.insert(\"name\".to_string()")
                && out.contains("__payload.insert(\"type\".to_string()"),
            "the payload must carry data / name / type:\n{out}"
        );
        assert!(
            out.contains("App::on_file(&__self2, &__payload);"),
            "the handler must be called with the payload:\n{out}"
        );
        assert!(
            out.contains("add_event_listener_with_callback(\"change\""),
            "the listener must attach to the change event:\n{out}"
        );
        // `data-flv-file` is a directive, not a DOM attribute.
        assert!(
            !out.contains(".set_attribute(\"data-flv-file\""),
            "data-flv-file must not be emitted as a DOM attribute:\n{out}"
        );
    }

    #[test]
    fn data_flv_file_adds_filereader_web_sys_features() {
        let file = parse_expand(
            r#"component App {
  state { img: Str = "" }
  event on_file() { img = payload["data"] }
  <template><input type="file" data-flv-file="on_file" /></template>
}"#,
        );
        let feats = wasm_extra_web_sys_features(&file);
        for f in ["HtmlInputElement", "FileReader", "File", "FileList", "Blob"] {
            assert!(feats.contains(&f), "missing feature `{f}`: {feats:?}");
        }
    }

    // ---- Phase 11.7 R3.5c — string interpolation -------------------

    #[test]
    fn phase_11_7_r3_5_c_str_interp_lowers_to_format() {
        let src = r#"component App {
  state {
    n: Int = 0
    label: Str = ""
  }
  event set_label() { label = "n is {n}" }
  <template><span>{label}</span></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("format!(\"n is {}\", (*self.n.borrow()))"),
            "string interpolation must lower to a format!:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_r3_5_c_str_interp_single_expr() {
        // The kanban's `let id_str = "{next_id}"` shape.
        let src = r#"component App {
  state {
    next_id: Int = 1
    last: Str = ""
  }
  event stamp() { last = "{next_id}" }
  <template><span>{last}</span></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("format!(\"{}\", (*self.next_id.borrow()))"),
            "a single-expr interpolation must lower to `format!(\"{{}}\", ...)`:\n{out}"
        );
    }

    // ---- Phase 11.7.c — event bubbling -----------------------------

    #[test]
    fn phase_11_7_c_child_event_bubbles_to_parent() {
        // Parent handler `on_hit` does NOT read `payload`, so the bubble
        // closure drops the payload (`|_|`) and calls the parent with the
        // zero-payload arity — but the slot still carries a payload type,
        // and the child (whose event IS bubbled) forwards `payload`.
        let src = r#"component App {
  state { hits: Int = 0 }
  event on_hit() { hits = hits + 1 }
  <template><div><Kid @ping="on_hit" /></div></template>
}
component Kid {
  state { n: Int = 0 }
  event ping() {}
  <template><button @click="ping">{n}</button></template>
}"#;
        let out = emit_module(&parse_expand(src)).unwrap();
        assert!(
            out.contains(
                "__on_ping: RefCell<Option<Box<dyn Fn(&std::collections::HashMap<String, String>)>>>,"
            ),
            "the child's bubble callback slot carries a payload:\n{out}"
        );
        // Kid's `ping` is bubbled → its signature takes a payload param
        // even though its body never reads it, so it can forward it.
        assert!(
            out.contains(
                "fn ping(self: &Rc<Self>, payload: &std::collections::HashMap<String, String>) {"
            ),
            "a bubbled handler takes a payload param to forward:\n{out}"
        );
        assert!(
            out.contains("if let Some(__cb) = self.__on_ping.borrow().as_ref() { __cb(payload); }"),
            "the child's ping handler forwards its payload to the callback:\n{out}"
        );
        assert!(
            out.contains(
                ".__on_ping.borrow_mut() = Some(Box::new(move |_: &std::collections::HashMap<String, String>| {"
            ) && out.contains("App::on_hit(&__parent);"),
            "a payload-ignoring parent handler drops the bubbled payload:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_payload_bubbles_to_parent_handler() {
        // Parent `on_pick` READS the payload, and the child sources it
        // from a `data-flv-value-*` attribute. The bubbled payload reaches
        // the parent handler intact.
        let src = r#"component App {
  state { last: Str = "none" }
  event on_pick() { if (payload.has("id")) { last = payload["id"] } }
  <template><div><Kid @choose="on_pick" /></div></template>
}
component Kid {
  state { id: Str = "" }
  event choose() {}
  <template><button data-flv-click="choose" data-flv-value-id="{id}">{id}</button></template>
}"#;
        let out = emit_module(&parse_expand(src)).unwrap();
        // The child's click listener builds the payload from the DOM attr.
        assert!(
            out.contains("__payload.insert(\"id\".to_string(), __evt_el.get_attribute(")
                && out.contains("Kid::choose(&__self_clone, &__payload);"),
            "the child sources its payload from data-flv-value-id:\n{out}"
        );
        // The child forwards that payload up.
        assert!(
            out.contains(
                "if let Some(__cb) = self.__on_choose.borrow().as_ref() { __cb(payload); }"
            ),
            "the child forwards the payload to the bubble callback:\n{out}"
        );
        // The parent's payload-reading handler receives it.
        assert!(
            out.contains(
                ".__on_choose.borrow_mut() = Some(Box::new(move |__pl: &std::collections::HashMap<String, String>| {"
            ) && out.contains("App::on_pick(&__parent, __pl);"),
            "the parent handler receives the bubbled payload:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_non_bubbled_component_has_no_payload_slot() {
        // No parent binds `@choose`, so Kid gains no callback slot and its
        // handler keeps its zero-arg (no-payload) signature.
        let src = r#"component App {
  state { x: Int = 0 }
  event noop() { x = 0 }
  <template><div><Kid /></div></template>
}
component Kid {
  state { n: Int = 0 }
  event choose() { n = n + 1 }
  <template><button @click="choose">{n}</button></template>
}"#;
        let out = emit_module(&parse_expand(src)).unwrap();
        assert!(!out.contains("__on_choose"), "no callback slot:\n{out}");
        assert!(
            out.contains("fn choose(self: &Rc<Self>) {"),
            "a non-bubbled handler that ignores payload stays zero-arg:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_c_unbound_child_event_gets_no_slot() {
        // Kid's `ping` event is NOT bound by any parent — no callback slot.
        let src = r#"component App {
  state { x: Int = 0 }
  event noop() { x = 0 }
  <template><div><Kid /></div></template>
}
component Kid {
  state { n: Int = 0 }
  event ping() { n = n + 1 }
  <template><button @click="ping">{n}</button></template>
}"#;
        let out = emit_module(&parse_expand(src)).unwrap();
        assert!(
            !out.contains("__on_ping"),
            "an unbound child event must not gain a callback slot:\n{out}"
        );
    }

    // ---- Phase 11.7.d — slots ---------------------------------------

    #[test]
    fn phase_11_7_d_slot_fill_emits_render_method_and_wiring() {
        let src = r#"component App {
  state { title: Str = "hi" }
  <template><div><Panel>{title}</Panel></div></template>
}
component Panel {
  state {}
  <template><section><slot /></section></template>
}"#;
        let out = emit_module(&parse_expand(src)).unwrap();
        assert!(
            out.contains("fn __render_slot_0(self: &Rc<Self>, __target: &web_sys::Node) {"),
            "the parent emits a slot-content renderer:\n{out}"
        );
        assert!(
            out.contains(
                ".__slot.borrow_mut() = Some(Rc::new(move |__t: &web_sys::Node| __parent.__render_slot_0(__t)));"
            ),
            "the parent wires the child's __slot to the renderer:\n{out}"
        );
        assert!(
            out.contains("__slot: RefCell<Option<Rc<dyn Fn(&web_sys::Node)>>>,"),
            "Panel gets a __slot field:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_d_self_closing_child_no_slot_wiring() {
        // A self-closing `<Panel />` provides no content → no renderer.
        let src = r#"component App {
  state {}
  <template><div><Panel /></div></template>
}
component Panel {
  state {}
  <template><section><slot /></section></template>
}"#;
        let out = emit_module(&parse_expand(src)).unwrap();
        assert!(
            !out.contains("__render_slot_0"),
            "a self-closing child must not add a slot renderer:\n{out}"
        );
    }

    #[test]
    fn phase_11_7_d_nested_child_in_slot_content_rejects() {
        let src = r#"component App {
  state {}
  <template><div><Panel><Inner /></Panel></div></template>
}
component Panel {
  state {}
  <template><section><slot /></section></template>
}
component Inner {
  state {}
  <template><span>x</span></template>
}"#;
        let err = emit_module(&parse_expand(src)).unwrap_err();
        assert!(
            err.message.contains("nested inside") && err.message.contains("Inner"),
            "a nested <Child /> in slot content must reject:\n{err}"
        );
    }

    // ---- v0.24.0 — named slots -------------------------------------

    #[test]
    fn named_slots_parent_routes_content_to_named_and_default_fields() {
        let src = r#"component App {
  state { title: Str = "hi" }
  <template>
    <div>
      <Panel>
        <h2 slot="header">Header {title}</h2>
        <p>default body</p>
        <div slot="footer">footer bits</div>
      </Panel>
    </div>
  </template>
}
component Panel {
  state {}
  <template>
    <section>
      <header><slot name="header" /></header>
      <div class="body"><slot /></div>
      <footer><slot name="footer" /></footer>
    </section>
  </template>
}"#;
        let out = emit_module(&parse_expand(src)).unwrap();
        // Child gains a distinct field per slot.
        assert!(
            out.contains("__slot_header: RefCell<Option<Rc<dyn Fn(&web_sys::Node)>>>,")
                && out.contains("__slot_footer: RefCell<Option<Rc<dyn Fn(&web_sys::Node)>>>,")
                && out.contains("    __slot: RefCell<Option<Rc<dyn Fn(&web_sys::Node)>>>,"),
            "Panel gets __slot_header, __slot_footer, and __slot fields:\n{out}"
        );
        // Parent wires each field to its own renderer method.
        assert!(
            out.contains(".__slot.borrow_mut() = Some(Rc::new(move |__t: &web_sys::Node| __parent.__render_slot_0(__t)));")
                && out.contains(".__slot_header.borrow_mut() = Some(Rc::new(move |__t: &web_sys::Node| __parent.__render_slot_1(__t)));")
                && out.contains(".__slot_footer.borrow_mut() = Some(Rc::new(move |__t: &web_sys::Node| __parent.__render_slot_2(__t)));"),
            "the parent wires default + both named fields to distinct renderers:\n{out}"
        );
        // Three synthesised renderer methods.
        assert!(
            out.contains("fn __render_slot_0(self: &Rc<Self>, __target: &web_sys::Node) {")
                && out.contains("fn __render_slot_1(self: &Rc<Self>, __target: &web_sys::Node) {")
                && out.contains("fn __render_slot_2(self: &Rc<Self>, __target: &web_sys::Node) {"),
            "three slot renderers are emitted:\n{out}"
        );
        // The `slot="..."` routing attribute is stripped from the
        // rendered element (not emitted as a literal DOM attribute).
        assert!(
            !out.contains("set_attribute(\"slot\""),
            "the routing `slot` attribute must be stripped from output:\n{out}"
        );
    }

    #[test]
    fn named_slot_unknown_target_rejects() {
        let src = r#"component App {
  state {}
  <template><div><Panel><span slot="footer">x</span></Panel></div></template>
}
component Panel {
  state {}
  <template><section><slot name="header" /></section></template>
}"#;
        let err = emit_module(&parse_expand(src)).unwrap_err();
        assert!(
            err.message.contains("slot=\"footer\"")
                && err.message.contains("no `<slot name=\"footer\" />`")
                && err.message.contains("`header`"),
            "targeting an undeclared named slot must reject and list the declared ones:\n{err}"
        );
    }

    #[test]
    fn named_slot_default_content_without_default_slot_rejects() {
        // Parent gives unslotted (default) real content but the child
        // only declares a named slot — no default `<slot />`.
        let src = r#"component App {
  state {}
  <template><div><Panel><h2 slot="header">ok</h2><p>orphan body</p></Panel></div></template>
}
component Panel {
  state {}
  <template><section><slot name="header" /></section></template>
}"#;
        let err = emit_module(&parse_expand(src)).unwrap_err();
        assert!(
            err.message.contains("default (unslotted) content")
                && err.message.contains("no default `<slot />`"),
            "unslotted content with no default slot must reject:\n{err}"
        );
    }

    #[test]
    fn slot_content_without_any_slot_rejects() {
        // The byte-for-byte default path: content but no `<slot />` at
        // all in the child.
        let src = r#"component App {
  state {}
  <template><div><Panel>content</Panel></div></template>
}
component Panel {
  state {}
  <template><section>no slot here</section></template>
}"#;
        let err = emit_module(&parse_expand(src)).unwrap_err();
        assert!(
            err.message.contains("no `<slot />` to fill"),
            "filling a slotless child must reject:\n{err}"
        );
    }

    #[test]
    fn named_slot_hyphen_folds_to_underscore() {
        let src = r#"component Foo {
  state {}
  <template><div><slot name="side-bar" /></div></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        assert!(
            out.contains("__slot_side_bar: RefCell<Option<Rc<dyn Fn(&web_sys::Node)>>>,")
                && out.contains("if let Some(__cb) = self.__slot_side_bar.borrow().as_ref() {"),
            "a hyphenated slot name folds to `__slot_side_bar`:\n{out}"
        );
    }

    #[test]
    fn named_slot_field_collision_rejects() {
        // `side-bar` and `side_bar` both fold to `__slot_side_bar`.
        let src = r#"component Foo {
  state {}
  <template><div><slot name="side-bar" /><slot name="side_bar" /></div></template>
}"#;
        let err = emit_component(&parse_expand(src).components[0]).unwrap_err();
        assert!(
            err.message.contains("same backing field") && err.message.contains("__slot_side_bar"),
            "colliding named slots must reject:\n{err}"
        );
    }

    #[test]
    fn default_only_component_keeps_bare_slot_field() {
        // Regression guard: a default-only component must emit exactly
        // `__slot` (no `__slot_` prefix) — byte-for-byte with 11.7.d.
        let src = r#"component Panel {
  state {}
  <template><section><slot /></section></template>
}"#;
        let out = emit_component(&parse_expand(src).components[0]).unwrap();
        // Exactly one slot-callback field (`__slot`) — no `__slot_<name>`.
        let field_count = out
            .matches("RefCell<Option<Rc<dyn Fn(&web_sys::Node)>>>,")
            .count();
        assert!(
            out.contains("    __slot: RefCell<Option<Rc<dyn Fn(&web_sys::Node)>>>,")
                && field_count == 1,
            "a default-only component keeps exactly the bare __slot field (found {field_count}):\n{out}"
        );
    }

    #[test]
    fn named_slot_self_closing_child_no_wiring() {
        // `<Panel />` self-closing fills nothing → the named slot renders
        // its fallback, no renderer method is synthesised.
        let src = r#"component App {
  state {}
  <template><div><Panel /></div></template>
}
component Panel {
  state {}
  <template><section><slot name="header"><em>fallback</em></slot></section></template>
}"#;
        let out = emit_module(&parse_expand(src)).unwrap();
        assert!(
            !out.contains("__render_slot_0"),
            "a self-closing child must not add a slot renderer:\n{out}"
        );
        assert!(
            out.contains("if let Some(__cb) = self.__slot_header.borrow().as_ref() {"),
            "the named slot still emits its callback-or-fallback branch:\n{out}"
        );
    }

    // ---- Phase 11.7 — cross-file `<Child />` composition -----------

    /// Helper: build an `ImportedComponentRegistry` from one or more
    /// `.fitzv` sources, as if their components were loaded from sibling
    /// files. Mirrors what `load_imported_components` does at build time.
    fn imported_registry(srcs: &[&str]) -> ImportedComponentRegistry {
        let mut reg = ImportedComponentRegistry::new();
        for src in srcs {
            for comp in parse_expand(src).components {
                reg.insert(comp);
            }
        }
        reg
    }

    #[test]
    fn cross_file_registry_first_registration_wins_on_name_collision() {
        let mut reg = ImportedComponentRegistry::new();
        let a = parse_expand(
            "component Card {\n  state { n: Int = 1 }\n  <template><span>{n}</span></template>\n}",
        )
        .components
        .remove(0);
        let b = parse_expand(
            "component Card {\n  state { n: Int = 2 }\n  <template><span>{n}</span></template>\n}",
        )
        .components
        .remove(0);
        reg.insert(a);
        reg.insert(b);
        // The second `Card` is dropped — first-registration wins.
        assert_eq!(reg.components().len(), 1);
        assert!(matches!(
            reg.get("Card").unwrap().state[0].default,
            crate::ast::Expr::Int(1, _)
        ));
    }

    #[test]
    fn merge_imported_components_empty_registry_is_structural_clone() {
        // The byte-a-byte invariant: with no imported components the
        // merge returns the file unchanged, so same-file examples emit
        // identically.
        let file = parse_expand(
            "component App {\n  state { n: Int = 0 }\n  <template><span>{n}</span></template>\n}",
        );
        let merged = merge_imported_components(&file, &ImportedComponentRegistry::new());
        assert_eq!(merged, file);
    }

    #[test]
    fn merge_imported_components_only_pulls_reachable_children() {
        // Local `App` composes `<Card />`; the registry also holds an
        // unused `Ghost`. Only `Card` (reachable) is merged in.
        let file = parse_expand(
            "component App {\n  state {}\n  <template><div><Card /></div></template>\n}",
        );
        let reg = imported_registry(&[
            "component Card {\n  state {}\n  <template><article>card</article></template>\n}",
            "component Ghost {\n  state {}\n  <template><aside>ghost</aside></template>\n}",
        ]);
        let merged = merge_imported_components(&file, &reg);
        let names: Vec<&str> = merged.components.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"Card"), "reachable Card merged: {names:?}");
        assert!(names.contains(&"App"), "local App preserved: {names:?}");
        assert!(
            !names.contains(&"Ghost"),
            "unreachable Ghost NOT merged: {names:?}"
        );
    }

    #[test]
    fn merge_imported_components_transitively_reaches_grandchildren() {
        // `App` -> `<Card />`; `Card` -> `<Badge />`. Both imported
        // components are reachable and merged.
        let file = parse_expand(
            "component App {\n  state {}\n  <template><div><Card /></div></template>\n}",
        );
        let reg = imported_registry(&[
            "component Card {\n  state {}\n  <template><article><Badge /></article></template>\n}",
            "component Badge {\n  state {}\n  <template><b>!</b></template>\n}",
        ]);
        let merged = merge_imported_components(&file, &reg);
        let names: Vec<&str> = merged.components.iter().map(|c| c.name.as_str()).collect();
        assert!(
            names.contains(&"Card") && names.contains(&"Badge"),
            "{names:?}"
        );
    }

    #[test]
    fn merge_imported_components_local_wins_on_name_collision() {
        // A local `Card` shadows an imported `Card` of the same name.
        let file = parse_expand(
            "component App {\n  state {}\n  <template><div><Card /></div></template>\n}\ncomponent Card {\n  state { local: Int = 7 }\n  <template><span>{local}</span></template>\n}",
        );
        let reg = imported_registry(&[
            "component Card {\n  state { imported: Int = 9 }\n  <template><span>{imported}</span></template>\n}",
        ]);
        let merged = merge_imported_components(&file, &reg);
        let card = merged
            .components
            .iter()
            .find(|c| c.name == "Card")
            .expect("a Card is present");
        assert_eq!(
            card.state[0].name, "local",
            "the LOCAL Card wins — the imported one is not merged"
        );
        // Exactly one Card in the merge (the imported one is skipped).
        assert_eq!(
            merged
                .components
                .iter()
                .filter(|c| c.name == "Card")
                .count(),
            1
        );
    }

    #[test]
    fn cross_file_emit_inlines_imported_component_struct() {
        // End-to-end: the parent's `<Card />` resolves to an imported
        // component; the emitter inlines the child's struct + wires the
        // parent's cache slot to it.
        let file = parse_expand(
            "component App {\n  state {}\n  <template><div><Card title=\"hi\" /></div></template>\n}",
        );
        let reg = imported_registry(&[
            "component Card {\n  state { title: Str = \"\" }\n  <template><article>{title}</article></template>\n}",
        ]);
        let out = emit_module_with_components(
            &file,
            &NominalRegistry::new(),
            &ImportedFnRegistry::new(),
            &reg,
        )
        .unwrap();
        assert!(out.contains("pub struct Card {"), "Card inlined:\n{out}");
        assert!(
            out.contains("__child_slot_0: RefCell<Option<Rc<Card>>>,"),
            "parent caches the cross-file Card:\n{out}"
        );
        assert!(
            out.contains(".title.borrow_mut() = \"hi\".to_string();"),
            "static prop fanned into the cross-file child:\n{out}"
        );
    }

    #[test]
    fn cross_file_emit_empty_registry_matches_same_file_path() {
        // `emit_module_with_components` with an empty registry is
        // byte-for-byte identical to `emit_module_with_imports`.
        let file = parse_expand(
            "component App {\n  state { n: Int = 0 }\n  event tap() { n = 1 }\n  <template><button @click=\"tap\">{n}</button></template>\n}",
        );
        let via_components = emit_module_with_components(
            &file,
            &NominalRegistry::new(),
            &ImportedFnRegistry::new(),
            &ImportedComponentRegistry::new(),
        )
        .unwrap();
        let via_imports =
            emit_module_with_imports(&file, &NominalRegistry::new(), &ImportedFnRegistry::new())
                .unwrap();
        assert_eq!(via_components, via_imports);
    }
}
