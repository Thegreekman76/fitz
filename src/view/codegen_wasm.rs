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
    ) {
        self.fns.insert(
            name.clone(),
            ImportedFn {
                name,
                params,
                ret,
                body,
            },
        );
    }

    /// True when no functions are registered — no `fn`s are emitted, so
    /// the output stays byte-identical to the pre-R3.5a.2 path.
    pub fn is_empty(&self) -> bool {
        self.fns.is_empty()
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
    emit_nominal_structs(nominals, &mut out)?;
    emit_imported_fns(fns, nominals, &mut out)?;
    let merged = merge_imported_components(file, components);
    let bubbled = collect_bubbled_events(&merged);
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
fn emit_nominal_structs(nominals: &NominalRegistry, out: &mut String) -> EmitResult<()> {
    if nominals.is_empty() {
        return Ok(());
    }
    for (name, fields) in nominals.iter() {
        writeln!(out, "#[allow(dead_code)]").unwrap();
        writeln!(out, "#[derive(Clone)]").unwrap();
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
fn emit_imported_fns(
    fns: &ImportedFnRegistry,
    nominals: &NominalRegistry,
    out: &mut String,
) -> EmitResult<()> {
    if fns.is_empty() {
        return Ok(());
    }
    for f in fns.iter() {
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
        for stmt in &f.body {
            lower_stmt(stmt, &[], &mut local_scope, "    ", out).map_err(|mut e| {
                e.context = format!("imported fn `{}` (body)", f.name);
                e
            })?;
        }
        writeln!(out, "}}\n").unwrap();
    }
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
    let state_names: Vec<String> = component.state.iter().map(|f| f.name.clone()).collect();

    // Phase 11.7.c — this component's event names that some parent binds
    // via `<ThisComponent @event="..." />`. Each gets a callback slot +
    // a bubble call in its handler. Empty for non-bubbled components.
    let empty = std::collections::BTreeSet::new();
    let this_bubbled = bubbled.get(&component.name).unwrap_or(&empty);

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

    emit_struct_and_new(
        component,
        &child_sites,
        nominals,
        this_bubbled,
        &slot_set,
        out,
    )?;
    emit_event_handlers(component, &state_names, this_bubbled, out)?;
    emit_mount_and_render(component, &state_names, file, nominals, this_bubbled, out)?;
    if let Some(style) = &component.style {
        emit_style_helper(&component.name, style, out);
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
    for stmt in &handler.body {
        lower_stmt(stmt, state_names, &mut locals, "        ", out).map_err(|mut e| {
            e.context = format!(
                "event handler `{}` of component `{}` (body)",
                handler.name, component_name
            );
            e
        })?;
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
        }
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
        } => emit_if(
            cond,
            children,
            else_children.as_deref(),
            parent_var,
            ctx,
            out,
        ),
        ExpandedTemplateNode::For {
            var,
            iter,
            children,
            ..
        } => emit_for(var, iter, children, parent_var, ctx, out),
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

fn emit_interpolation(
    expr: &Expr,
    parent_var: &str,
    ctx: &mut RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
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
/// bool literals, bool state fields / loop vars used directly, numeric
/// comparisons (`==`/`!=`/`<`/`<=`/`>`/`>=`), and `&&` / `||` / `!`
/// over those. Str comparisons + method-call conditions defer to a
/// later 11.7 slice or the SSR target.
fn lower_cond_expr(expr: &Expr, state_names: &[String], locals: &[String]) -> EmitResult<String> {
    match expr {
        Expr::Bool(b, _) => Ok(b.to_string()),
        // A bool state field / loop var used directly as a condition.
        // The checker guarantees it's Bool-typed.
        Expr::Ident(..) => lower_expr(expr, state_names, locals),
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
            message: "`{#if}` condition — supported: bool state field / loop var, \
                      numeric comparison, and &&/||/!. Str comparisons + method-call \
                      conditions defer to a later 11.7 slice or the SSR target"
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
            // Mixed attribute interpolation (`class="toast toast-{kind}"`)
            // is an SSR-target feature today. The client-WASM target
            // mutates attributes via `set_attribute` and would need each
            // `{expr}` segment lowered into a `format!` — deferred. Full
            // interpolation (`class="{cls}"`) works here.
            ExpandedAttr::MixedInterpolation { name, .. } => {
                return Err(EmitError {
                    message: format!(
                        "mixed attribute interpolation (`{name}=\"...{{expr}}...\"`) is not \
                         supported in the client-WASM target yet — use a full interpolation \
                         (`{name}=\"{{expr}}\"`) or the SSR target"
                    ),
                    context: format!(
                        "attribute `{}` of element `<{}>` in component `{}`",
                        name, tag, ctx.component_name
                    ),
                });
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

    // Phase 11.7.d + named slots — if the parent provided slot content
    // (`<Child>content</Child>`), register a renderer on the child that
    // fills its `<slot />` (or `<slot name="X" />`) with that content
    // (rendered in PARENT scope). `<Child />` nested inside slot content
    // is rejected (the parent has no child-cache field for it).
    if !slot_content.is_empty() {
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
        let child_slots = component_slot_set(child);
        let has_named_routing = slot_content.iter().any(|n| element_slot_attr(n).is_some());

        // Each entry: (backing field, content nodes) → one
        // `__render_slot_<n>` method wired to that field.
        let mut wirings: Vec<(String, Vec<ExpandedTemplateNode>)> = Vec::new();
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
                wirings.push(("__slot".to_string(), default_bucket));
            }
            for (slot_name, bucket) in named_buckets {
                wirings.push((slot_field_name(Some(&slot_name)), bucket));
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
            wirings.push(("__slot".to_string(), slot_content.to_vec()));
        }

        for (field, content) in wirings {
            let slot_idx = ctx.slot_methods.len();
            ctx.slot_methods.push(content);
            writeln!(out, "        {{").unwrap();
            writeln!(out, "            let __parent = self.clone();").unwrap();
            writeln!(
                out,
                "            *{}.{}.borrow_mut() = Some(Rc::new(move |__t: &web_sys::Node| __parent.__render_slot_{}(__t)));",
                child_var, field, slot_idx
            )
            .unwrap();
            writeln!(out, "        }}").unwrap();
        }
    }

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

fn emit_event_attr(
    event_name: &str,
    handler_name: &str,
    el_var: &str,
    ctx: &RenderCtx,
    out: &mut String,
) -> EmitResult<()> {
    let component_name = ctx.component_name;
    // POC only maps `@click` — other event names deferred to 11.4.c.
    let dom_event = match event_name {
        "click" => "click",
        other => {
            return Err(EmitError {
                message: format!(
                    "event `@{}` — only `@click` supported in Phase 11.4.b (deferred to 11.4.c)",
                    other
                ),
                context: format!(
                    "event attribute on element in template of component `{}`",
                    component_name
                ),
            });
        }
    };

    // Phase 11.7 R3.5b.1 — a `@click` handler that reads `payload` still
    // takes the param; with no `data-flv-value-*` attrs it receives an
    // empty map. Handlers that don't use payload keep the exact zero-arg
    // call, so the pre-R3.5b examples emit byte-for-byte unchanged.
    let takes_payload = handler_takes_payload(ctx, handler_name)?;

    writeln!(out, "        {{").unwrap();
    writeln!(out, "            let __self_clone = self.clone();").unwrap();
    writeln!(
        out,
        "            let __closure = Closure::wrap(Box::new(move |_evt: Event| {{"
    )
    .unwrap();
    if takes_payload {
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
            let op_str = lower_binop(op)?;
            let l = lower_expr(left, state_names, locals)?;
            let r = lower_expr(right, state_names, locals)?;
            Ok(format!("({} {} {})", l, op_str, r))
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
        BinOpKind::And
        | BinOpKind::Or
        | BinOpKind::Xor
        | BinOpKind::BitAnd
        | BinOpKind::BitOr
        | BinOpKind::BitXor
        | BinOpKind::Shl => Err(EmitError {
            message: "binary op — arithmetic (+/-/*//%) and comparisons \
                      (==/!=/</<=/>/>=) supported on the client-WASM target; \
                      logical (&&/||) belong in condition position (`{#if}` / \
                      `if`-expr) and bitwise ops are deferred"
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
                (other, n) => Err(EmitError {
                    message: format!(
                        "method `.{other}()` ({n} arg(s)) — the client-WASM target \
                         supports `.map`/`.filter`/`.len` on lists (Phase 11.7 \
                         R3.5a.1); other methods defer to a later 11.7 slice or the \
                         SSR target"
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
        // CW.6 (dual-target) — the raw-HTML / List<Html> framework helpers
        // have NO client-WASM equivalent: `html`/`raw_html` inject
        // deliberately-unescaped markup (a DOM text node cannot), and
        // `h_join`/`h_when`/`h_either` fold `Html` values. Treating them as
        // identity would silently render markup as escaped text (or fail to
        // type-check), so a component using them stays SSR-only. Hard-error
        // with a clear pointer rather than emitting a broken call.
        Expr::Ident(name, _)
            if matches!(
                name.as_str(),
                "html" | "raw_html" | "h_join" | "h_when" | "h_either"
            ) =>
        {
            Err(EmitError {
                message: format!(
                    "`{name}(...)` is an SSR-only fitz-liveviews helper (raw/unescaped \
                     HTML or List<Html> folding) with no client-WASM equivalent — a DOM \
                     text node escapes intrinsically and cannot inject raw markup. Use \
                     `{{expr}}` or `{{flv(expr)}}` for text content, or keep this \
                     component on the SSR target."
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
    out: &mut String,
) -> EmitResult<()> {
    match stmt {
        Stmt::Assign { target, value, .. } => match target {
            AssignTarget::Ident(name, _) => {
                if state_names.iter().any(|s| s == name) {
                    // Reassign a state field. The RHS is fully evaluated
                    // into `__rhs` first (dropping any read-borrow of the
                    // same field, e.g. `cards = cards.filter(...)`) before
                    // the `borrow_mut()`, so there is no double-borrow.
                    let rhs = lower_expr(value, state_names, locals)?;
                    writeln!(out, "{}let __rhs = {};", indent, rhs).unwrap();
                    writeln!(out, "{}*self.{}.borrow_mut() = __rhs;", indent, name).unwrap();
                } else {
                    // A local binding (`let target_id = ...`). Emit a Rust
                    // `let`; re-binding the same name later shadows, which
                    // matches Fitz's rebind semantics. Register the name so
                    // subsequent statements / closures see it as in-scope.
                    let rhs = lower_expr(value, state_names, locals)?;
                    writeln!(out, "{}let {} = {};", indent, name, rhs).unwrap();
                    if !locals.iter().any(|s| s == name) {
                        locals.push(name.clone());
                    }
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
        // Phase 11.7 R3.5a.1 — `return <expr>` (transpiled helper bodies
        // in R3.5a.2 use it; event bodies never return, so this arm is
        // dormant there).
        Stmt::Return(e, _) => {
            let rhs = lower_expr(e, state_names, locals)?;
            writeln!(out, "{}return {};", indent, rhs).unwrap();
            Ok(())
        }
        Stmt::Expr(inner, _) => lower_expr_stmt(inner, state_names, locals, indent, out),
        _ => Err(EmitError {
            message: "statement kind — supported on the client-WASM target: state-field \
                      reassignment, local `let` binding, `if` guard, `<state_list>.push/\
                      clear`, and `return`"
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
                lower_stmt(s, state_names, &mut then_locals, &inner_indent, out)?;
            }
            if let Some(else_stmts) = else_ {
                writeln!(out, "{}}} else {{", indent).unwrap();
                let mut else_locals = locals.to_vec();
                for s in else_stmts {
                    lower_stmt(s, state_names, &mut else_locals, &inner_indent, out)?;
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
            state: vec![ExpandedStateField {
                name: "count".to_string(),
                type_expr: TypeExpr::Named("Int".to_string()),
                default: Expr::Int(0, Span::default()),
                loc,
            }],
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
            state: vec![ExpandedStateField {
                name: "count".to_string(),
                type_expr: TypeExpr::Named("Int".to_string()),
                default: Expr::Int(0, Span::default()),
                loc,
            }],
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
        // The raw-HTML / List<Html> framework helpers have no client-WASM
        // equivalent — identity would silently render markup as escaped text
        // (or fail to type-check). They must hard-error, naming themselves
        // SSR-only, rather than emitting a broken call (CW.6 dual-target).
        for helper in ["html", "raw_html", "h_join", "h_when", "h_either"] {
            let src = format!(
                "component tag {{\n  state {{\n    label: Str = \"\"\n  }}\n  \
                 <template><span>{{{helper}(label)}}</span></template>\n}}\n"
            );
            let err = emit_component(&parse_expand(&src).components[0])
                .expect_err("raw-HTML helper must reject on the wasm target");
            assert!(
                err.message.contains("SSR-only"),
                "error for `{helper}` should name it SSR-only: {}",
                err.message
            );
        }
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
    fn emit_rejects_non_click_event() {
        let src = r#"component Foo {
  state { x: Int = 0 }
  event bump() { x = 1 }

  <template>
    <input @input="bump" />
  </template>
}"#;
        let expanded = parse_expand(src);
        let err = emit_component(&expanded.components[0]).unwrap_err();
        assert!(err.message.contains("@input"), "event kind error:\n{}", err);
        assert!(
            err.message.contains("11.4.c"),
            "deferred citation:\n{}",
            err
        );
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
            state: vec![],
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
                ..
            } = stmt
            {
                let params = params
                    .iter()
                    .map(|p| (p.name.clone(), p.type_.clone()))
                    .collect();
                reg.insert(name.clone(), params, return_type.clone(), body.clone());
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
