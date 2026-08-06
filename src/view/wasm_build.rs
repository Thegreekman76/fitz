//! Phase 11.5.c — `.fitzv` → `wasm-client` build orchestration
//! (composition helpers).
//!
//! This module owns the shape of the scaffold that `fitz build
//! --bin <web>` writes to disk when the selected bin's target is
//! `wasm-client`. Concretely:
//!
//! 1. [`compose_lib_rs`] runs the emitter (`view::emit_module`)
//!    and appends the `#[wasm_bindgen(start)] pub fn start()`
//!    entry point that instantiates the root component and calls
//!    `mount("<selector>")`.
//! 2. [`compose_cargo_toml`] produces the `Cargo.toml` for the
//!    scaffolded wasm-crate. Includes the canonical release
//!    profile knobs (`opt-level = "z"`, `lto`, single codegen
//!    unit, strip) and the crucial
//!    `[package.metadata.wasm-pack.profile.release]
//!    wasm-opt = ['-O', '--enable-bulk-memory']` knob without
//!    which `wasm-pack build --release` fails on modern rustc
//!    (see `docs/fase-11-plan.md` §9.o Debt residual).
//! 3. [`write_wasm_crate_scaffold`] materialises the scaffold on
//!    disk under `<manifest_dir>/target/wasm-build/<bin_name>/`.
//!
//! The pipeline (parse → expand → check → emit_module) is NOT
//! duplicated here — this module consumes an
//! [`ExpandedViewFile`] that the caller already produced.
//!
//! ## Root-component convention
//!
//! When the `.fitzv` file declares multiple `component X { ... }`
//! blocks, the FIRST-declared component is the root that the
//! composed `start()` mounts. Per `docs/fase-11-plan.md` §9.p, an
//! explicit `entry_component = "X"` manifest field is reserved
//! for 11.6+.
//!
//! ## Reusable by both the CLI and the smoke harness
//!
//! `tests/view_counter_wasm_smoke.rs` calls the same helpers as
//! `main.rs::build_wasm_client_cmd`. That is deliberate: the
//! bit-for-bit E2E test that compares the CLI-emitted `lib.rs`
//! against the counter baseline works because both paths route
//! through this module.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::manifest::DepRegistry;

use super::codegen_wasm::{
    emit_module_with_components, merge_imported_components, EmitError, EmitResult,
    ImportedComponentRegistry, ImportedFnRegistry, NominalRegistry,
};
use super::expand::{ExpandedViewFile, ExpandedViewImport};

/// A `.fitzv` file has no components declared, so there is no
/// root to mount. Returned by [`compose_lib_rs`] before it even
/// invokes the emitter, so the caller gets an actionable error
/// early. Encoded as an [`EmitError`] to keep the return type
/// aligned with `emit_module`.
const NO_COMPONENTS_MSG: &str = "the `.fitzv` file declares no `component X { ... }` block. \
     `target = \"wasm-client\"` needs at least one component to \
     compose the WASM entry point.";

/// Runs `emit_module` on `expanded` and appends the composed
/// `#[wasm_bindgen(start)] pub fn start() -> Result<(), JsValue>`
/// wrapper. The wrapper instantiates the FIRST-declared component
/// and calls `mount("<mount_selector>")`.
///
/// The composed source lives at the tail of the emitted `lib.rs`.
/// Header comments identify the source of truth (the caller's
/// path, typically `<bin.main>`) so a reader who opens the
/// generated file knows where to look.
///
/// `mount_selector` is written verbatim into the composed
/// wrapper via Rust source string interpolation — the caller
/// (the manifest parser, in the manifest-mode CLI path) is
/// responsible for validating the shape (typically `"#app"`).
///
/// `header_source_label` is embedded in the file's top comment
/// so `git blame` / readers know what produced the file. Typical
/// value: `"src/counter.fitzv"` or `"Counter.fitzv"`. If `None`,
/// a generic label is used.
pub fn compose_lib_rs(
    expanded: &ExpandedViewFile,
    mount_selector: &str,
    header_source_label: Option<&str>,
) -> EmitResult<String> {
    compose_lib_rs_with_nominals(
        expanded,
        &NominalRegistry::new(),
        mount_selector,
        header_source_label,
    )
}

/// Like [`compose_lib_rs`], but with a [`NominalRegistry`] of imported
/// classic `type` definitions (Phase 11.7 R3) so the emitted `lib.rs`
/// carries a real Rust `struct` for each nominal used in `List<Card>`
/// state, `{#for c in cards}` loops, `{c.title}` field access, and
/// `Card { ... }` construction. Populate the registry with
/// [`load_imported_nominals`] before calling.
///
/// When the registry is empty this delegates to the same emit path as
/// [`compose_lib_rs`] — byte-for-byte identical output, so the four
/// pre-R3 examples regenerate unchanged.
pub fn compose_lib_rs_with_nominals(
    expanded: &ExpandedViewFile,
    nominals: &NominalRegistry,
    mount_selector: &str,
    header_source_label: Option<&str>,
) -> EmitResult<String> {
    compose_lib_rs_with_imports(
        expanded,
        nominals,
        &ImportedFnRegistry::new(),
        mount_selector,
        header_source_label,
    )
}

/// Like [`compose_lib_rs_with_nominals`], but also transpiles imported
/// classic `fn`s into the emitted `lib.rs` (Phase 11.7 R3.5a.2), so the
/// component code can call helpers like `cards_in(cards, "todo")` /
/// `move_one(...)`. Populate `fns` with [`load_imported_fns`] before
/// calling.
///
/// When `fns` is empty this is byte-for-byte identical to
/// [`compose_lib_rs_with_nominals`].
pub fn compose_lib_rs_with_imports(
    expanded: &ExpandedViewFile,
    nominals: &NominalRegistry,
    fns: &ImportedFnRegistry,
    mount_selector: &str,
    header_source_label: Option<&str>,
) -> EmitResult<String> {
    compose_lib_rs_with_components(
        expanded,
        nominals,
        fns,
        &ImportedComponentRegistry::new(),
        mount_selector,
        header_source_label,
    )
}

/// Like [`compose_lib_rs_with_imports`], but also inlines imported
/// `.fitzv` components so cross-file `<Child />` composition lowers to
/// real Rust (Phase 11.7 — cross-file). Populate `components` with
/// [`load_imported_components`] before calling.
///
/// When `components` is empty this is byte-for-byte identical to
/// [`compose_lib_rs_with_imports`], so the same-file examples regenerate
/// unchanged.
pub fn compose_lib_rs_with_components(
    expanded: &ExpandedViewFile,
    nominals: &NominalRegistry,
    fns: &ImportedFnRegistry,
    components: &ImportedComponentRegistry,
    mount_selector: &str,
    header_source_label: Option<&str>,
) -> EmitResult<String> {
    compose_lib_rs_inner(
        expanded,
        nominals,
        fns,
        components,
        mount_selector,
        header_source_label,
        /*dev_mode=*/ false,
    )
}

/// Phase 11.13 slice-2 — like [`compose_lib_rs_with_components`], but for
/// `fitz dev`'s dev-mode build: appends the dev-only state-snapshot methods
/// (`__fitz_dev_snapshot`/`__fitz_dev_apply`) on the root component and a dev
/// entry wrapper that stashes/restores state through `sessionStorage` so a hot
/// reload preserves live state. `fitz build` never calls this — the prod
/// `lib.rs` stays byte-identical.
fn compose_lib_rs_inner(
    expanded: &ExpandedViewFile,
    nominals: &NominalRegistry,
    fns: &ImportedFnRegistry,
    components: &ImportedComponentRegistry,
    mount_selector: &str,
    header_source_label: Option<&str>,
    dev_mode: bool,
) -> EmitResult<String> {
    let root = match expanded.components.first() {
        Some(c) => c,
        None => {
            return Err(EmitError {
                message: NO_COMPONENTS_MSG.to_string(),
                context: "compose_lib_rs (Phase 11.5.c)".to_string(),
            });
        }
    };

    let emitted = emit_module_with_components(expanded, nominals, fns, components)?;

    let source_label = header_source_label.unwrap_or("(unknown source)");
    let mut out = String::new();
    out.push_str("// AUTO-GENERATED by `fitz build --target wasm-client`.\n");
    out.push_str("// Do NOT edit by hand — regenerate from the source `.fitzv`.\n");
    out.push_str("//\n");
    out.push_str(&format!("// Source: {source_label}\n"));
    out.push_str("//\n");
    out.push_str("// Composed by `src/view/wasm_build.rs::compose_lib_rs` (Phase 11.5.c).\n");
    out.push_str("// The tail below is the `#[wasm_bindgen(start)]` wrapper the CLI\n");
    out.push_str("// appends to the emitter output; the emitter itself does NOT emit\n");
    out.push_str("// the entry point so future targets (SSR, hybrid) can compose\n");
    out.push_str("// their own.\n\n");
    out.push_str(&emitted);
    // Phase 11.13 slice-2 — dev-only state-preservation methods on the root.
    if dev_mode {
        out.push('\n');
        super::codegen_wasm::emit_dev_state_methods(root, nominals, &mut out)?;
    }
    // Phase 11.12 — a root component is never bubbled (nothing composes it),
    // so an empty bubbled set is the right context for its hydratability.
    let root_hydratable =
        super::codegen_wasm::component_is_hydratable(root, &std::collections::BTreeSet::new());
    out.push_str(&compose_entry_wrapper(
        &root.name,
        mount_selector,
        root_hydratable,
        dev_mode,
    ));
    Ok(out)
}

/// Framework helpers the WASM emitter special-cases in `lower_call`:
/// `flv` is the identity passthrough, and `html`/`raw_html`/`h_join`/
/// `h_when`/`h_either` hard-error as SSR-only. They are NOT loadable
/// modules, so a `from <framework> import flv` line must never trigger a
/// dep load — resolving it would pull the framework's whole lib entry
/// into the transpile and blow the wasm envelope.
fn is_framework_view_builtin(name: &str) -> bool {
    matches!(
        name,
        "flv" | "html" | "raw_html" | "h_join" | "h_when" | "h_either"
    )
}

/// Resolve a `.fitzv` view import to a file path — dep-aware (CW.8).
///
/// Resolution order, mirroring the classic loaders (`codegen.rs` /
/// `evaluator.rs`) bit-for-bit:
/// 1. **Framework-builtin-only imports** (`from fitz_liveviews import flv`)
///    → `None`. The emitter special-cases these; they are never a file, and
///    loading a dep for them would drag the framework's lib entry in.
/// 2. **First path segment names a dependency** in `dep_registry` → resolve
///    through the dep's lib entry: a single segment (`from dep import X`) →
///    the lib entry itself; a dotted path (`from dep.ui.Badge import Badge`)
///    → the shared [`crate::view::resolve_dep_subpath_file`] under the dep
///    root (tries `.fitz` then `.fitzv`).
/// 3. **Sibling fallback** (unchanged): a single-segment path joined onto
///    `base_dir` with `sibling_ext`, if the file exists.
/// 4. Anything else (multi-segment non-dep, missing sibling) → `None`.
///
/// Every loader treats `None` as a silent skip. `sibling_ext` selects the
/// classic (`"fitz"`) vs view (`"fitzv"`) extension for the sibling
/// fallback only — the dep sub-path resolver always tries both.
fn resolve_view_import(
    imp: &ExpandedViewImport,
    base_dir: &Path,
    dep_registry: &DepRegistry,
    sibling_ext: &str,
) -> Option<PathBuf> {
    // (1) Framework builtins are emitter special-cases, not modules.
    if !imp.names.is_empty() && imp.names.iter().all(|(n, _)| is_framework_view_builtin(n)) {
        return None;
    }
    // (2) Dep-aware: first segment names a dependency.
    if let Some(seg0) = imp.path.first() {
        if let Some(lib_entry) = dep_registry.get(seg0) {
            if imp.path.len() == 1 {
                return Some(lib_entry.clone());
            }
            return crate::view::resolve_dep_subpath_file(lib_entry, &imp.path[1..]);
        }
    }
    // (3) Sibling fallback: single-segment path in `base_dir`.
    if imp.path.len() != 1 {
        return None;
    }
    let sibling = base_dir.join(format!("{}.{}", imp.path[0], sibling_ext));
    sibling.is_file().then_some(sibling)
}

/// Load the classic `type Foo { ... }` definitions that a `.fitzv`
/// imports (`from foo import Foo`) into a [`NominalRegistry`], so the
/// WASM emitter can synthesise a Rust `struct` for each (Phase 11.7
/// R3).
///
/// Unlike the SSR path — which re-emits the `from foo import Foo` line
/// verbatim and lets the classic loader resolve `Foo` in a second
/// pass — the `wasm-client` target produces a standalone crate with no
/// downstream classic pass, so the nominal's field list must be loaded
/// here and lowered to real Rust.
///
/// **Best-effort + MVP scope**:
/// - Only single-segment import paths (`from card import Card`);
///   dotted paths (`from a.b import X`) are skipped (the nominal use
///   then rejects at emit with a clear pointer).
/// - Only sibling `.fitz` files are read (nominal `type` decls live in
///   classic Fitz). If the sibling is a `.fitzv`, missing, or fails to
///   read/parse, that import is skipped silently — it may be a
///   function-only helper module, and a genuinely-needed nominal that
///   ends up unregistered rejects at emit with an actionable message.
/// - Aliases are honoured: `from card import Card as Row` registers
///   the type's fields under `Row` (the name the `.fitzv` uses).
/// - Imported names that don't match a `type` in the sibling (e.g.
///   imported functions) are simply not registered.
pub fn load_imported_nominals(
    imports: &[ExpandedViewImport],
    base_dir: &Path,
) -> EmitResult<NominalRegistry> {
    load_imported_nominals_with_deps(imports, base_dir, &DepRegistry::new())
}

/// Dep-aware variant of [`load_imported_nominals`] (CW.8). Resolves each
/// import through [`resolve_view_import`], so an import whose first path
/// segment names a `fitz.toml` dependency (`from dep.ui.Types import Foo`)
/// finds the nominal under the dependency's root instead of only in a flat
/// sibling. The empty-registry wrapper above preserves the old two-arg
/// signature the view smoke tests call.
pub fn load_imported_nominals_with_deps(
    imports: &[ExpandedViewImport],
    base_dir: &Path,
    dep_registry: &DepRegistry,
) -> EmitResult<NominalRegistry> {
    use crate::ast::Stmt;

    let mut registry = NominalRegistry::new();
    for imp in imports {
        let sibling = match resolve_view_import(imp, base_dir, dep_registry, "fitz") {
            Some(f) => f,
            None => continue,
        };
        let source = match fs::read_to_string(&sibling) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let tokens = match crate::lexer::tokenize(&source) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let program = match crate::parser::parse(tokens) {
            Ok(p) => p,
            Err(_) => continue,
        };
        // Index the sibling's `type` decls by name.
        for stmt in &program {
            if let Stmt::TypeDef { name, fields, .. } = stmt {
                // Register only the names this import actually brings in,
                // under their local binding (alias when present).
                for (orig, alias) in &imp.names {
                    if orig == name {
                        let binding = alias.clone().unwrap_or_else(|| orig.clone());
                        let field_list: Vec<(String, crate::ast::TypeExpr)> = fields
                            .iter()
                            .map(|f| (f.name.clone(), f.type_.clone()))
                            .collect();
                        registry.insert(binding, field_list);
                    }
                }
            }
        }
    }
    Ok(registry)
}

/// Load the classic top-level `fn`s reachable through a `.fitzv`'s
/// imports into an [`ImportedFnRegistry`], so the WASM emitter can
/// transpile each to a Rust `fn` (Phase 11.7 R3.5a.2).
///
/// Unlike [`load_imported_nominals`] — which registers only the
/// explicitly-imported names — this registers **every** top-level `fn`
/// in each imported sibling module. An imported helper (`move_one`)
/// typically calls internal siblings (`next_column` / `prev_column`)
/// that the `.fitzv` never names, so all of them must be transpiled.
///
/// **Best-effort + MVP scope** (parallel to `load_imported_nominals`):
/// - Only single-segment sibling `.fitz` modules are scanned; each is
///   scanned once even if several names are imported from it.
/// - A sibling that is missing / a `.fitzv` / fails to read/parse is
///   skipped silently (it may be a nominal-only module). A genuinely
///   needed helper that ends up unregistered surfaces as a `cannot find
///   function` rustc error in the generated crate.
/// - Functions with an unsupported body (`match`, loops, `?`) or a
///   missing param / return annotation reject at EMIT (`emit_imported_fns`),
///   not here — so an unused complex sibling only bites if it is emitted.
pub fn load_imported_fns(
    imports: &[ExpandedViewImport],
    base_dir: &Path,
) -> EmitResult<ImportedFnRegistry> {
    load_imported_fns_with_deps(imports, base_dir, &DepRegistry::new())
}

/// Dep-aware variant of [`load_imported_fns`] (CW.8). Resolves each import
/// through [`resolve_view_import`] and de-dupes on the RESOLVED file path
/// (not `imp.path[0]`), so two dep imports that share a dep name but resolve
/// to different modules (`from dep.a import X` + `from dep.b import Y`) are
/// both scanned. The empty-registry wrapper above keeps the old two-arg
/// signature the view smoke tests call.
pub fn load_imported_fns_with_deps(
    imports: &[ExpandedViewImport],
    base_dir: &Path,
    dep_registry: &DepRegistry,
) -> EmitResult<ImportedFnRegistry> {
    use crate::ast::Stmt;

    let mut registry = ImportedFnRegistry::new();
    let mut scanned: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // CW.9 — fn aliases from the imports (`from icon import icon as
    // render_icon`). The "load whole file" scan registers each fn by its real
    // name; a template calls it by its local binding, so an aliased fn is ALSO
    // registered (and emitted) under the alias. Parallel to the alias handling
    // in `load_imported_nominals` / `load_imported_components` (CW.8).
    let fn_aliases: Vec<(String, String)> = imports
        .iter()
        .flat_map(|imp| imp.names.iter())
        .filter_map(|(orig, alias)| match alias {
            Some(a) if a != orig => Some((orig.clone(), a.clone())),
            _ => None,
        })
        .collect();
    for imp in imports {
        let sibling = match resolve_view_import(imp, base_dir, dep_registry, "fitz") {
            Some(f) => f,
            None => continue,
        };
        // Scan each resolved module at most once.
        if !scanned.insert(sibling.to_string_lossy().into_owned()) {
            continue;
        }
        let source = match fs::read_to_string(&sibling) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let tokens = match crate::lexer::tokenize(&source) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let program = match crate::parser::parse(tokens) {
            Ok(p) => p,
            Err(_) => continue,
        };
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
                let param_list: Vec<(String, Option<crate::ast::TypeExpr>)> = params
                    .iter()
                    .map(|p| (p.name.clone(), p.type_.clone()))
                    .collect();
                // Phase 11.11.c — an `@rpc` fn is a server function: its
                // body stays on the server; the wasm emitter produces a
                // `fetch` stub instead of transpiling it.
                let is_rpc = decorators.iter().any(|d| d.name == "rpc");
                registry.insert(
                    name.clone(),
                    param_list.clone(),
                    return_type.clone(),
                    body.clone(),
                    is_rpc,
                );
                // Also register under any import alias for this fn name, so a
                // template calling the aliased binding resolves (CW.9).
                for (orig, alias) in &fn_aliases {
                    if orig == name {
                        registry.insert(
                            alias.clone(),
                            param_list.clone(),
                            return_type.clone(),
                            body.clone(),
                            is_rpc,
                        );
                    }
                }
            }
        }
    }
    Ok(registry)
}

/// Load the `.fitzv` components a `.fitzv` imports (`from Card import
/// Card`) into an [`ImportedComponentRegistry`], so the WASM emitter can
/// inline each imported component's whole emit (struct + `new` + event
/// handlers + render + `<style scoped>`) into the standalone crate —
/// enabling cross-file `<Child />` composition (Phase 11.7).
///
/// Unlike [`load_imported_nominals`] / [`load_imported_fns`], which read
/// classic `.fitz` siblings (nominal `type`s + free `fn`s), a component
/// lives in a `.fitzv` — so this resolves `{stem}.fitzv` and runs the view
/// pipeline (`parse` → `expand`) on each sibling. Every component the
/// sibling declares is registered (not just the explicitly-imported name)
/// so an imported `Card` can compose its own file-local siblings — the
/// same "load the whole file" policy as [`load_imported_fns`].
///
/// **Best-effort + MVP scope** (parallel to the nominal / fn loaders):
/// - Only single-segment sibling `.fitzv` modules are scanned; each is
///   scanned once even if several names are imported from it.
/// - A sibling that is missing / a classic `.fitz` / fails to parse or
///   expand is skipped silently (it is most likely a nominal-or-fn-only
///   `.fitz` module, already handled by the other loaders). A genuinely
///   needed component that ends up unregistered surfaces as an "unknown
///   component" checker error at the parent's composition site.
/// - Transitivity: the loader itself reads only direct siblings, but the
///   caller passes the transitive union computed by
///   [`collect_transitive_view_imports`], so a grandchild component in a
///   file the ENTRY does not import is still registered. File-local sibling
///   components of an imported component are also available (whole file is
///   loaded).
/// - Aliasing is honoured (v0.26.0): `from Card import Card as Row`
///   registers a renamed clone under `Row` (so `<Row />` resolves) *and*
///   keeps the original `Card`, so an imported component's file-local
///   siblings that compose it by its real name still work.
pub fn load_imported_components(
    imports: &[ExpandedViewImport],
    base_dir: &Path,
) -> EmitResult<ImportedComponentRegistry> {
    load_imported_components_with_deps(imports, base_dir, &DepRegistry::new())
}

/// Dep-aware variant of [`load_imported_components`] (CW.8). Resolves each
/// import through [`resolve_view_import`], so an import whose first path
/// segment names a `fitz.toml` dependency (`from fitz_liveviews.ui.Badge
/// import Badge`) inlines the dep's component instead of only a flat
/// sibling — unblocking an external wasm app consuming the companion UI as
/// a library. De-dupes on the RESOLVED file path so several dep sub-path
/// imports sharing a dep name are each scanned. The empty-registry wrapper
/// above keeps the old two-arg signature the view smoke tests call.
pub fn load_imported_components_with_deps(
    imports: &[ExpandedViewImport],
    base_dir: &Path,
    dep_registry: &DepRegistry,
) -> EmitResult<ImportedComponentRegistry> {
    let mut registry = ImportedComponentRegistry::new();
    let mut scanned: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for imp in imports {
        let sibling = match resolve_view_import(imp, base_dir, dep_registry, "fitzv") {
            Some(f) => f,
            None => continue,
        };
        // Scan each resolved `.fitzv` at most once.
        if !scanned.insert(sibling.to_string_lossy().into_owned()) {
            continue;
        }
        let source = match fs::read_to_string(&sibling) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let raw = match super::parse(&source) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let expanded = match super::expand(&raw) {
            Ok(x) => x,
            Err(_) => continue,
        };
        for component in expanded.components {
            // Aliasing: if this import brings the component in under an alias
            // (`from Card import Card as Row`), register a renamed clone so
            // `<Row />` resolves. The original name is kept too, so the
            // imported component's file-local siblings that compose it by its
            // real name still work. Distinct names → both survive the
            // first-wins `insert`.
            let alias = imp.names.iter().find_map(|(orig, al)| {
                if *orig == component.name {
                    al.clone()
                } else {
                    None
                }
            });
            if let Some(alias_name) = alias {
                if alias_name != component.name {
                    let mut renamed = component.clone();
                    renamed.name = alias_name;
                    registry.insert(renamed);
                }
            }
            registry.insert(component);
        }
    }
    Ok(registry)
}

/// Compute the transitive closure of `.fitzv` view imports reachable from
/// `entry`, following each imported `.fitzv`'s OWN `from X import Y` one
/// file at a time. The MVP loaders ([`load_imported_components`] /
/// [`load_imported_nominals`] / [`load_imported_fns`]) each read only the
/// direct siblings; this walks the `.fitzv` import graph so a grandchild
/// component — or a nominal / helper `fn` an imported component needs — that
/// lives in a file the ENTRY does not import is still discovered.
///
/// Returns the union of `entry` plus every import found in a reachable
/// `.fitzv`, with entry imports first (so entry-level aliases take
/// precedence in the `scanned`-deduped loaders). Feed it verbatim to the
/// three loaders, which each de-dupe by module stem.
///
/// **Best-effort + MVP scope** (parallel to the loaders):
/// - Only single-segment `.fitzv` siblings are expanded to read their own
///   imports; classic `.fitz` imports (nominals / fns) reach the union but
///   are not recursed into (following `.fitz → .fitz` chains is a separate,
///   deeper feature).
/// - Resolved against a single flat `base_dir` (all view files in one
///   directory — the layout the examples use).
/// - Cycle-safe: each `.fitzv` file is expanded at most once.
/// - A sibling that is missing / fails to parse or expand is skipped
///   silently; the same import still reaches the loaders.
pub fn collect_transitive_view_imports(
    entry: &[ExpandedViewImport],
    base_dir: &Path,
) -> Vec<ExpandedViewImport> {
    collect_transitive_view_imports_with_deps(entry, base_dir, &DepRegistry::new())
}

/// Dep-aware variant of [`collect_transitive_view_imports`] (CW.8). Follows
/// dependency `.fitzv` components (resolved via [`resolve_view_import`]) in
/// addition to flat siblings, so a dep component's own imports (a sibling
/// dep component, a dep nominal/helper) reach the union fed to the loaders.
/// De-dupes expanded files on the RESOLVED path. The empty-registry wrapper
/// above keeps the old two-arg signature the view smoke tests call.
///
/// **MVP limit**: resolution uses a single flat `base_dir` for the sibling
/// fallback, so a dep component that composes a bare-name sibling (`from
/// Icon import Icon`, not `from dep.ui.Icon import Icon`) does not resolve
/// that sibling against the DEP's own directory. The dual-target companion
/// primitives are leaves, so this is documented residual debt, not a blocker.
pub fn collect_transitive_view_imports_with_deps(
    entry: &[ExpandedViewImport],
    base_dir: &Path,
    dep_registry: &DepRegistry,
) -> Vec<ExpandedViewImport> {
    use std::collections::{BTreeSet, VecDeque};

    let mut result: Vec<ExpandedViewImport> = Vec::new();
    let mut seen_entries: BTreeSet<String> = BTreeSet::new();
    let mut expanded_files: BTreeSet<String> = BTreeSet::new();
    let mut worklist: VecDeque<ExpandedViewImport> = entry.iter().cloned().collect();

    while let Some(imp) = worklist.pop_front() {
        // De-dupe exact import entries (same path + same names/aliases) so an
        // import reached at two levels is not fed to the loaders twice.
        let key = format!("{:?}|{:?}", imp.path, imp.names);
        if seen_entries.insert(key) {
            result.push(imp.clone());
        }
        // Follow a `.fitzv` component (sibling or dep) — those can compose
        // further children. Each expanded at most once (cycle-safe), keyed on
        // the RESOLVED path so dep sub-paths sharing a name are each followed.
        let sibling = match resolve_view_import(&imp, base_dir, dep_registry, "fitzv") {
            Some(f) => f,
            None => continue,
        };
        if !expanded_files.insert(sibling.to_string_lossy().into_owned()) {
            continue;
        }
        let source = match fs::read_to_string(&sibling) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let raw = match super::parse(&source) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let expanded = match super::expand(&raw) {
            Ok(x) => x,
            Err(_) => continue,
        };
        for child_imp in expanded.imports {
            worklist.push_back(child_imp);
        }
    }
    result
}

/// The `#[wasm_bindgen(start)]` wrapper appended to the emitter
/// output. Instantiates `<Root>::new()` and calls `mount("<selector>")`.
///
/// Phase 11.12 — when `root_hydratable` is `true`, the wrapper instead
/// checks whether the mount root already holds server-painted DOM: if so
/// it calls `root.hydrate(root_el)` (adopt the existing DOM, don't wipe);
/// otherwise it falls back to `mount` (fresh client render into an empty
/// root). A non-hydratable root emits the byte-identical `mount`-only
/// wrapper, so pre-11.12 crates regenerate unchanged.
fn compose_entry_wrapper(
    root_component: &str,
    mount_selector: &str,
    root_hydratable: bool,
    dev_mode: bool,
) -> String {
    // Escape backslashes / quotes for the Rust string literal. In
    // practice `mount` is a CSS selector like `"#app"` — no such
    // chars appear — but doing the escape keeps the emitter honest
    // if someone declares `mount = "[data-app='x']"` later.
    let escaped_selector = mount_selector.replace('\\', "\\\\").replace('"', "\\\"");

    // Phase 11.13 slice-2 — dev-only glue injected after `root.mount(...)`:
    // restore the state snapshot stashed by the previous run before a hot
    // reload, and register a `beforeunload` handler that snapshots the live
    // state back into `sessionStorage`. Empty string in prod builds → the
    // wrapper is byte-identical to the pre-11.13 output.
    let dev_glue = if dev_mode {
        dev_state_preservation_glue()
    } else {
        String::new()
    };

    if root_hydratable {
        return format!(
            "\n\n\
             // Composed entry point — NOT part of emit_module output.\n\
             // `fitz build --target wasm-client` appends this wrapper. Phase\n\
             // 11.12: hydrate the server-painted DOM when the mount root\n\
             // already has content, else fresh-mount into an empty root.\n\
             #[wasm_bindgen(start)]\n\
             pub fn start() -> Result<(), JsValue> {{\n\
             \x20\x20\x20\x20console_error_panic_hook::set_once();\n\
             \x20\x20\x20\x20let root = {root}::new();\n\
             \x20\x20\x20\x20let __document = web_sys::window().unwrap().document().unwrap();\n\
             \x20\x20\x20\x20if let Some(__mount) = __document.query_selector(\"{selector}\")? {{\n\
             \x20\x20\x20\x20\x20\x20\x20\x20if __mount.first_element_child().is_some() {{\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20let __el = __mount.dyn_into::<web_sys::HtmlElement>()?;\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20return root.hydrate(__el);\n\
             \x20\x20\x20\x20\x20\x20\x20\x20}}\n\
             \x20\x20\x20\x20}}\n\
             \x20\x20\x20\x20root.mount(\"{selector}\")?;\n\
             {dev_glue}\
             \x20\x20\x20\x20Ok(())\n\
             }}\n",
            root = root_component,
            selector = escaped_selector,
            dev_glue = dev_glue,
        );
    }

    format!(
        "\n\n\
         // Composed entry point — NOT part of emit_module output.\n\
         // `fitz build --target wasm-client` (Phase 11.5.c) appends this\n\
         // wrapper so the bundle mounts the root component when the\n\
         // browser calls the `start` export.\n\
         #[wasm_bindgen(start)]\n\
         pub fn start() -> Result<(), JsValue> {{\n\
         \x20\x20\x20\x20console_error_panic_hook::set_once();\n\
         \x20\x20\x20\x20let root = {root}::new();\n\
         \x20\x20\x20\x20root.mount(\"{selector}\")?;\n\
         {dev_glue}\
         \x20\x20\x20\x20Ok(())\n\
         }}\n",
        root = root_component,
        selector = escaped_selector,
        dev_glue = dev_glue,
    )
}

/// Phase 11.13 slice-2 — the dev-only `start()` body inserted between
/// `root.mount(...)` and `Ok(())`. Restores the state stashed before the
/// previous reload (then clears it) and installs a `beforeunload` snapshot,
/// both through `sessionStorage`. Reuses the exact `Closure` + event-listener
/// idiom the emitter already uses for `@click`/`@input` handlers, so it needs
/// no extra deps beyond the `Storage` web-sys feature the dev Cargo.toml adds.
fn dev_state_preservation_glue() -> String {
    "\x20\x20\x20\x20// Phase 11.13 — fitz dev hot-reload state preservation.\n\
     \x20\x20\x20\x20let __win = web_sys::window().unwrap();\n\
     \x20\x20\x20\x20if let Ok(Some(__ss)) = __win.session_storage() {\n\
     \x20\x20\x20\x20\x20\x20\x20\x20if let Ok(Some(__saved)) = __ss.get_item(\"__fitz_dev_state\") {\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20root.__fitz_dev_apply(&__saved);\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20let _ = __ss.remove_item(\"__fitz_dev_state\");\n\
     \x20\x20\x20\x20\x20\x20\x20\x20}\n\
     \x20\x20\x20\x20\x20\x20\x20\x20let __ss_bu = __ss.clone();\n\
     \x20\x20\x20\x20\x20\x20\x20\x20let __root_bu = root.clone();\n\
     \x20\x20\x20\x20\x20\x20\x20\x20let __closure = Closure::wrap(Box::new(move |_evt: Event| {\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20let _ = __ss_bu.set_item(\"__fitz_dev_state\", &__root_bu.__fitz_dev_snapshot());\n\
     \x20\x20\x20\x20\x20\x20\x20\x20}) as Box<dyn FnMut(Event)>);\n\
     \x20\x20\x20\x20\x20\x20\x20\x20__win.add_event_listener_with_callback(\"beforeunload\", __closure.as_ref().unchecked_ref()).unwrap();\n\
     \x20\x20\x20\x20\x20\x20\x20\x20__closure.forget();\n\
     \x20\x20\x20\x20}\n"
        .to_string()
}

/// Produces the `Cargo.toml` for the scaffolded wasm-crate. The
/// `package.name` is derived from the bin name (must already be
/// a valid Rust crate identifier — the caller sanitises via
/// [`sanitise_wasm_pkg_name`]).
///
/// Kept aligned bit-for-bit with `examples/view/counter/wasm-crate/
/// Cargo.toml` (the canonical baseline used by the smoke test) so
/// the E2E can compare hand-authored vs CLI-emitted outputs
/// without noise.
///
/// The `wasm-opt = ['-O', '--enable-bulk-memory']` metadata knob
/// is CRITICAL: without it, `wasm-pack build --release` fails at
/// the `wasm-opt` step because the bundled `binaryen` rejects the
/// bulk-memory ops modern rustc emits (see `docs/fase-11-plan.md`
/// §9.o Debt residual).
pub fn compose_cargo_toml(package_name: &str) -> String {
    compose_cargo_toml_with_features(package_name, &[], false, false)
}

/// Like [`compose_cargo_toml`], but appends `extra_features` to the
/// `web-sys` feature list (Phase 11.7 R3.5b.2). A `data-flv-submit` form
/// reads inputs via `HtmlInputElement`, which needs the matching feature.
/// With `extra_features` empty this is byte-for-byte identical to
/// [`compose_cargo_toml`], so form-free examples' `Cargo.toml` is
/// unchanged. Callers derive the extra set via
/// [`super::codegen_wasm::wasm_extra_web_sys_features`].
pub fn compose_cargo_toml_with_features(
    package_name: &str,
    extra_features: &[&str],
    uses_rpc: bool,
    uses_hydration: bool,
) -> String {
    // Base subset the emitter always uses (see codegen_wasm). Kept in the
    // same order as the committed baselines so the empty-extra case stays
    // byte-identical.
    const BASE_FEATURES: &[&str] = &[
        "Document",
        "Element",
        "Event",
        "EventTarget",
        "HtmlElement",
        "HtmlHeadElement",
        "Node",
        "Text",
        "Window",
    ];
    // Phase 11.11.c — `@rpc` needs the fetch APIs + a console for
    // error reporting. Appended after `extra_features` so non-rpc crates
    // stay byte-identical.
    const RPC_FEATURES: &[&str] = &[
        "Request",
        "RequestInit",
        "RequestMode",
        "Response",
        "Headers",
        "console",
    ];
    let mut feature_lines = String::new();
    let rpc_features: &[&str] = if uses_rpc { RPC_FEATURES } else { &[] };
    for f in BASE_FEATURES
        .iter()
        .chain(extra_features.iter())
        .chain(rpc_features.iter())
    {
        feature_lines.push_str(&format!("    \"{f}\",\n"));
    }
    // Phase 11.11.c — extra crate deps for the fetch stub + JSON
    // marshaling. Injected into `[dependencies]` only when the crate
    // uses `@rpc`.
    let rpc_deps = if uses_rpc {
        "wasm-bindgen-futures = \"0.4\"\n\
         js-sys = \"0.3\"\n\
         serde = { version = \"1\", features = [\"derive\"] }\n\
         serde_json = \"1\"\n"
    } else {
        ""
    };
    // Phase 11.12 — hydration restores serialized state via `serde_json`. Only
    // when the crate hydrates AND `@rpc` didn't already pull it (so the deps
    // stay byte-identical for non-hydration and rpc crates).
    let hydration_deps = if uses_hydration && !uses_rpc {
        "serde_json = \"1\"\n"
    } else {
        ""
    };
    format!(
        r##"[package]
name = "{name}"
version = "0.0.1"
edition = "2021"
publish = false

[lib]
crate-type = ["cdylib"]

# Sizing knobs — el gate de Phase 11.4 es 40 KB gzipped sobre el
# `.wasm` producido por `wasm-pack build --release`. `opt-level =
# "z"` + `lto = true` es la config canónica del ecosistema
# wasm-bindgen para minimizar el bundle. `wasm-pack` invoca
# `wasm-opt` automáticamente en modo release.
[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
strip = true

# wasm-opt (bundled with wasm-pack) uses an older binaryen that rejects
# bulk-memory ops emitted by recent rustc. Pass `--enable-bulk-memory`
# so the size-optimization pass still runs. Without this, `wasm-pack
# build --release` fails at the wasm-opt step with a `wasm-validator
# error ... Bulk memory operations require bulk memory` panic.
[package.metadata.wasm-pack.profile.release]
wasm-opt = ['-O', '--enable-bulk-memory']

[dependencies]
wasm-bindgen = "0.2"
console_error_panic_hook = "0.1"
{rpc_deps}{hydration_deps}
# `web-sys` con el subset EXACTO que el emitter de Fase 11.4.b usa
# (ver `src/view/codegen_wasm.rs::emit_module_header` + los emit_*
# helpers). Cualquier feature que sobre suma bytes al bundle final.
[dependencies.web-sys]
version = "0.3"
features = [
{feature_lines}]
"##,
        name = package_name,
        rpc_deps = rpc_deps,
        hydration_deps = hydration_deps,
        feature_lines = feature_lines,
    )
}

/// Phase 11.13 slice-2 — augments a composed `Cargo.toml` with the two extra
/// bits a `fitz dev` build needs: the `serde_json` dependency (state
/// snapshot/apply) and the `Storage` web-sys feature (`sessionStorage`). Both
/// are idempotent: if `serde_json` is already present (rpc/hydration crate) or
/// `Storage` is already in the feature list, that injection is skipped. Only
/// called on the dev path — the prod `Cargo.toml` is never touched.
fn dev_augment_cargo_toml(toml: String) -> String {
    let mut out = toml;
    // serde_json dep — insert after the always-present
    // `console_error_panic_hook = "0.1"` line.
    if !out.contains("serde_json = \"1\"") {
        let anchor = "console_error_panic_hook = \"0.1\"\n";
        if let Some(pos) = out.find(anchor) {
            let at = pos + anchor.len();
            out.insert_str(at, "serde_json = \"1\"\n");
        }
    }
    // `Storage` web-sys feature — insert after the always-present `"Window",`
    // feature line (BASE_FEATURES).
    if !out.contains("\"Storage\",") {
        let anchor = "    \"Window\",\n";
        if let Some(pos) = out.find(anchor) {
            let at = pos + anchor.len();
            out.insert_str(at, "    \"Storage\",\n");
        }
    }
    out
}

/// Sanitises a bin name into a valid Rust crate identifier. The
/// Fitz manifest allows bin names like `"web-client"` (kebab-case
/// per `is_valid_package_name`), but Rust crate names in
/// `Cargo.toml` map to identifiers with `-` accepted only in the
/// package name field (converted to `_` internally when used as a
/// module path). `wasm-pack` produces `<name>_bg.wasm` and
/// `<name>_bg.js` where `<name>` has `-` replaced by `_`, which
/// is confusing. Simpler: sanitise upfront by converting `-` to
/// `_`.
///
/// Also lowercases (defensive — `is_valid_package_name` already
/// enforces this) and strips any character that doesn't fit
/// `[a-z0-9_]`. If sanitisation produces an empty string, returns
/// `"wasm_bundle"` as a fallback (should never happen in practice
/// because `Manifest::parse` rejects invalid names earlier).
pub fn sanitise_wasm_pkg_name(bin_name: &str) -> String {
    let clean: String = bin_name
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .map(|c| if c == '-' { '_' } else { c })
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
        .collect();
    if clean.is_empty() {
        "wasm_bundle".to_string()
    } else {
        clean
    }
}

/// Result of scaffolding a wasm-crate on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct ScaffoldResult {
    /// Absolute path of the scaffolded crate directory (the one
    /// with `Cargo.toml` at its root). Caller shells `wasm-pack
    /// build` inside this dir.
    pub crate_dir: PathBuf,
    /// Absolute path of the emitted `src/lib.rs`. Useful for
    /// E2E tests that inspect the CLI-emitted source without
    /// invoking `wasm-pack`.
    pub lib_rs: PathBuf,
    /// Absolute path of the emitted `Cargo.toml`. Symmetric with
    /// `lib_rs` — useful for tests.
    pub cargo_toml: PathBuf,
}

/// Errors returned by [`write_wasm_crate_scaffold`]. Distinct
/// from [`EmitError`] because the failure surface is filesystem
/// I/O (which `EmitError` doesn't model) — the caller
/// (`main.rs`) wraps into `String` for `eprintln!`.
#[derive(Debug)]
pub enum ScaffoldError {
    /// Failure to create the destination dir or write files.
    Io(io::Error, PathBuf),
    /// The composed `lib.rs` could not be produced (upstream
    /// emitter or root-component-missing error).
    Emit(EmitError),
}

impl std::fmt::Display for ScaffoldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScaffoldError::Io(e, path) => {
                write!(
                    f,
                    "wasm-crate scaffold I/O error at `{}`: {e}",
                    path.display()
                )
            }
            ScaffoldError::Emit(e) => write!(f, "wasm-crate emitter error: {e}"),
        }
    }
}

impl std::error::Error for ScaffoldError {}

impl From<EmitError> for ScaffoldError {
    fn from(e: EmitError) -> Self {
        ScaffoldError::Emit(e)
    }
}

/// Materialises the scaffold at `crate_dir`. Creates
/// `crate_dir/`, `crate_dir/src/`, writes `Cargo.toml` and
/// `src/lib.rs`. Idempotent: overwrites whatever is there.
///
/// Callers pass:
/// - `crate_dir` — absolute path of the crate to create. The
///   `main.rs` CLI uses `<manifest_dir>/target/wasm-build/<bin>/`.
/// - `expanded` — the checked view file (result of `view::expand`
///   + `view::check`).
/// - `nominals` — imported classic `type` defs loaded via
///   [`load_imported_nominals`] (Phase 11.7 R3). Pass
///   `&NominalRegistry::new()` when the `.fitzv` imports no nominals.
/// - `fns` — imported classic `fn`s loaded via [`load_imported_fns`]
///   (Phase 11.7 R3.5a.2). Pass `&ImportedFnRegistry::new()` when the
///   `.fitzv` imports no helpers.
/// - `components` — imported `.fitzv` components loaded via
///   [`load_imported_components`] (Phase 11.7 — cross-file `<Child />`).
///   Pass `&ImportedComponentRegistry::new()` when the `.fitzv` composes
///   no cross-file children.
/// - `bin_name` — the sanitised crate name. Used as
///   `package.name` in the emitted `Cargo.toml`.
/// - `mount_selector` — the CSS selector for the composed
///   `start()` wrapper. Comes from the `[[bin]] mount = "..."`
///   field.
/// - `source_label` — an identifier for the `.fitzv` source
///   embedded in the `lib.rs` header comment (typically the
///   bin's `main` path relative to the manifest).
// The three import registries (nominals / fns / components) each carry
// a distinct kind of cross-file surface the standalone WASM crate needs
// inlined; folding them into a struct would only move the argument list
// one level down without clarifying anything.
#[allow(clippy::too_many_arguments)]
pub fn write_wasm_crate_scaffold(
    crate_dir: &Path,
    expanded: &ExpandedViewFile,
    nominals: &NominalRegistry,
    fns: &ImportedFnRegistry,
    components: &ImportedComponentRegistry,
    bin_name: &str,
    mount_selector: &str,
    source_label: Option<&str>,
    // Phase 11.13 slice-2 — `fitz dev` sets this so the scaffold carries the
    // dev-only hot-reload state-preservation glue (`sessionStorage` +
    // `serde_json` + the `Storage` web-sys feature). `fitz build` passes
    // `false` → the prod scaffold is byte-identical to pre-11.13.
    dev_mode: bool,
) -> Result<ScaffoldResult, ScaffoldError> {
    // Phase 11.7 R3.5b.2 — a `data-flv-submit` form needs the
    // `HtmlInputElement` web-sys feature; form-free crates keep the base
    // feature set (byte-identical Cargo.toml). Detect over the MERGED
    // file so a form declared inside a cross-file imported component is
    // counted too — when `components` is empty the merge is a structural
    // clone, so the Cargo.toml stays byte-identical for same-file crates.
    let feature_file = merge_imported_components(expanded, components);
    let extra_features = super::codegen_wasm::wasm_extra_web_sys_features(&feature_file);
    // Phase 11.12 — a crate that hydrates any component restores serialized
    // state via `serde_json`. Detect over the merged file so an imported
    // hydratable component counts too (empty `components` → structural clone →
    // byte-identical Cargo.toml for same-file crates).
    let uses_hydration = super::codegen_wasm::file_uses_hydration(&feature_file);
    // Phase 11.11.c — when any imported fn is `@rpc`, the crate gets the
    // fetch runtime deps + web-sys features (see
    // `compose_cargo_toml_with_features`).
    let cargo_toml_text =
        compose_cargo_toml_with_features(bin_name, &extra_features, fns.has_rpc(), uses_hydration);
    // Phase 11.13 slice-2 — dev builds need `serde_json` (state snapshot) + the
    // `Storage` web-sys feature (`sessionStorage`). Post-process instead of
    // threading a param through `compose_cargo_toml_with_features` so its ~10
    // smoke call sites stay untouched and the prod Cargo.toml is byte-identical.
    let cargo_toml_text = if dev_mode {
        dev_augment_cargo_toml(cargo_toml_text)
    } else {
        cargo_toml_text
    };
    let lib_rs_text = compose_lib_rs_inner(
        expanded,
        nominals,
        fns,
        components,
        mount_selector,
        source_label,
        dev_mode,
    )?;

    let src_dir = crate_dir.join("src");
    fs::create_dir_all(&src_dir).map_err(|e| ScaffoldError::Io(e, src_dir.clone()))?;

    let cargo_toml_path = crate_dir.join("Cargo.toml");
    fs::write(&cargo_toml_path, cargo_toml_text)
        .map_err(|e| ScaffoldError::Io(e, cargo_toml_path.clone()))?;

    let lib_rs_path = src_dir.join("lib.rs");
    fs::write(&lib_rs_path, lib_rs_text).map_err(|e| ScaffoldError::Io(e, lib_rs_path.clone()))?;

    Ok(ScaffoldResult {
        crate_dir: crate_dir.to_path_buf(),
        lib_rs: lib_rs_path,
        cargo_toml: cargo_toml_path,
    })
}

// ---- tests ----

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view;

    /// Parses + expands + checks a `.fitzv` source string and
    /// returns the [`ExpandedViewFile`]. Panics on any pipeline
    /// error (tests own the fixture, so bugs there are hard
    /// errors).
    fn expand_ok(src: &str) -> ExpandedViewFile {
        let raw = view::parse(src).expect("view::parse");
        let expanded = view::expand(&raw).expect("view::expand");
        let errs = view::check(&expanded);
        assert!(errs.is_empty(), "check errors: {errs:?}");
        expanded
    }

    #[test]
    fn compose_lib_rs_appends_wasm_bindgen_start_entry_point() {
        let src = r#"component Root {
  state { count: Int = 0 }
  event tap() { count = count + 1 }
  <template><button @click="tap">{count}</button></template>
}
"#;
        let expanded = expand_ok(src);
        let out = compose_lib_rs(&expanded, "#app", Some("Root.fitzv")).unwrap();

        // Emitter output present.
        assert!(out.contains("pub struct Root"), "emitter output missing");
        // Composed entry appended.
        assert!(
            out.contains("#[wasm_bindgen(start)]"),
            "wasm-bindgen start attribute missing"
        );
        assert!(
            out.contains("let root = Root::new();"),
            "root instantiation must use the first component's name"
        );
        assert!(
            out.contains("root.mount(\"#app\")"),
            "mount selector must be interpolated verbatim"
        );
        // Header cites the source label so a reader knows where to
        // look.
        assert!(
            out.contains("Source: Root.fitzv"),
            "source label must appear in the header comment"
        );
    }

    #[test]
    fn compose_lib_rs_picks_first_component_as_root() {
        // Multi-component file: the FIRST-declared component is
        // the root. Per §9.p, an explicit `entry_component`
        // manifest field is reserved for 11.6+.
        let src = r#"component Alpha {
  state { x: Int = 0 }
  <template><span>{x}</span></template>
}
component Beta {
  state { y: Int = 0 }
  <template><span>{y}</span></template>
}
"#;
        let expanded = expand_ok(src);
        let out = compose_lib_rs(&expanded, "#app", None).unwrap();
        assert!(
            out.contains("let root = Alpha::new();"),
            "Alpha (first-declared) must be the mounted root"
        );
        assert!(
            !out.contains("let root = Beta::new();"),
            "Beta (second) must NOT be the mounted root"
        );
        // Both components are still present in the emitter
        // output, ready to be mounted as children via `<Beta />`
        // composition (11.5.d).
        assert!(out.contains("pub struct Alpha"));
        assert!(out.contains("pub struct Beta"));
    }

    #[test]
    fn compose_lib_rs_escapes_mount_selector_quotes_and_backslashes() {
        // Not idiomatic CSS but we guard the codegen anyway.
        let src = r#"component X {
  state {}
  <template><div>hi</div></template>
}
"#;
        let expanded = expand_ok(src);
        let out = compose_lib_rs(&expanded, r#"[data-app="x"]"#, None).unwrap();
        assert!(
            out.contains(r#"root.mount("[data-app=\"x\"]")"#),
            "quotes must be escaped in the Rust string literal:\n{out}"
        );
    }

    #[test]
    fn compose_lib_rs_errors_when_no_components_declared() {
        // Empty file — the parser accepts it (0 top-level
        // components); the composer must reject before emitter
        // because there is no root to mount.
        let src = "";
        let expanded = expand_ok(src);
        let err = compose_lib_rs(&expanded, "#app", None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no `component X { ... }` block"), "msg: {msg}");
        assert!(msg.contains("wasm-client"), "msg: {msg}");
    }

    #[test]
    fn phase_11_13_dev_augment_cargo_toml_adds_serde_json_and_storage() {
        // A plain crate (no rpc, no hydration) has neither serde_json nor the
        // Storage feature. The dev augment adds both, idempotently.
        let plain = compose_cargo_toml("counter");
        assert!(!plain.contains("serde_json = \"1\""));
        assert!(!plain.contains("\"Storage\","));
        let dev = dev_augment_cargo_toml(plain);
        assert!(dev.contains("serde_json = \"1\""), "serde_json dep:\n{dev}");
        assert!(dev.contains("\"Storage\","), "Storage feature:\n{dev}");
        // `Storage` sits right after `Window` in the feature list.
        let win = dev.find("\"Window\",").unwrap();
        let sto = dev.find("\"Storage\",").unwrap();
        assert!(sto > win, "Storage after Window:\n{dev}");
        // Idempotent — augmenting again does not duplicate.
        let dev2 = dev_augment_cargo_toml(dev);
        assert_eq!(dev2.matches("serde_json = \"1\"").count(), 1);
        assert_eq!(dev2.matches("\"Storage\",").count(), 1);
    }

    #[test]
    fn compose_cargo_toml_contains_the_wasm_opt_metadata_knob() {
        let toml = compose_cargo_toml("counter");
        assert!(
            toml.contains("[package.metadata.wasm-pack.profile.release]"),
            "wasm-opt metadata section missing:\n{toml}"
        );
        assert!(
            toml.contains("wasm-opt = ['-O', '--enable-bulk-memory']"),
            "wasm-opt knob must include --enable-bulk-memory:\n{toml}"
        );
    }

    #[test]
    fn compose_cargo_toml_pins_web_sys_features_that_the_emitter_uses() {
        let toml = compose_cargo_toml("counter");
        // Subset used by the emitter (see codegen_wasm.rs).
        for feature in &[
            "Document",
            "Element",
            "Event",
            "EventTarget",
            "HtmlElement",
            "HtmlHeadElement",
            "Node",
            "Text",
            "Window",
        ] {
            let quoted = format!("\"{feature}\"");
            assert!(
                toml.contains(&quoted),
                "web-sys feature `{feature}` missing from Cargo.toml:\n{toml}"
            );
        }
    }

    #[test]
    fn compose_cargo_toml_sets_lib_crate_type_cdylib() {
        let toml = compose_cargo_toml("counter");
        assert!(
            toml.contains(r#"crate-type = ["cdylib"]"#),
            "crate-type = [\"cdylib\"] missing:\n{toml}"
        );
    }

    #[test]
    fn compose_cargo_toml_adds_rpc_deps_and_features_when_uses_rpc() {
        // Phase 11.11.c — an `@rpc`-using crate gets the fetch runtime.
        let toml = compose_cargo_toml_with_features("web", &[], true, false);
        for dep in &[
            "wasm-bindgen-futures = \"0.4\"",
            "js-sys = \"0.3\"",
            "serde = { version = \"1\", features = [\"derive\"] }",
            "serde_json = \"1\"",
        ] {
            assert!(toml.contains(dep), "rpc dep `{dep}` missing:\n{toml}");
        }
        for feat in &["Request", "RequestInit", "Response", "Headers", "console"] {
            assert!(
                toml.contains(&format!("\"{feat}\"")),
                "rpc web-sys feature `{feat}` missing:\n{toml}"
            );
        }
    }

    #[test]
    fn compose_cargo_toml_without_rpc_is_byte_identical_to_plain() {
        // The `uses_rpc = false` path must not perturb the non-rpc
        // baseline (the committed examples' Cargo.toml).
        let plain = compose_cargo_toml("counter");
        let via_flag = compose_cargo_toml_with_features("counter", &[], false, false);
        assert_eq!(plain, via_flag);
        assert!(!plain.contains("wasm-bindgen-futures"));
        assert!(!plain.contains("Request"));
    }

    #[test]
    fn phase_11_12_compose_cargo_toml_adds_serde_json_when_hydration() {
        // A hydrating crate (no `@rpc`) pulls only `serde_json` (state
        // restore), not the full fetch runtime.
        let toml = compose_cargo_toml_with_features("web", &[], false, true);
        assert!(
            toml.contains("serde_json = \"1\""),
            "serde_json dep:\n{toml}"
        );
        assert!(
            !toml.contains("wasm-bindgen-futures"),
            "hydration must not drag in the fetch runtime:\n{toml}"
        );
    }

    #[test]
    fn phase_11_12_compose_cargo_toml_hydration_no_dup_when_rpc() {
        // When `@rpc` already pulled serde_json, hydration must not duplicate
        // the dep line.
        let toml = compose_cargo_toml_with_features("web", &[], true, true);
        assert_eq!(
            toml.matches("serde_json = \"1\"").count(),
            1,
            "serde_json appears exactly once:\n{toml}"
        );
    }

    #[test]
    fn phase_11_12_compose_cargo_toml_without_hydration_is_byte_identical() {
        let plain = compose_cargo_toml("counter");
        let via_flag = compose_cargo_toml_with_features("counter", &[], false, false);
        assert_eq!(
            plain, via_flag,
            "non-hydration Cargo.toml stays byte-identical"
        );
    }

    #[test]
    fn phase_11_12_entry_wrapper_hydratable_branches_to_hydrate() {
        let wrapper = compose_entry_wrapper("Greeter", "#app", true, false);
        assert!(
            wrapper.contains("query_selector(\"#app\")?")
                && wrapper.contains("first_element_child().is_some()")
                && wrapper.contains("return root.hydrate(__el);"),
            "hydratable entry branches to hydrate:\n{wrapper}"
        );
        // Still falls back to a fresh mount into an empty root.
        assert!(
            wrapper.contains("root.mount(\"#app\")?;"),
            "hydratable entry keeps the fresh-mount fallback:\n{wrapper}"
        );
    }

    #[test]
    fn phase_11_12_entry_wrapper_non_hydratable_is_mount_only() {
        let wrapper = compose_entry_wrapper("Counter", "#app", false, false);
        assert!(
            wrapper.contains("root.mount(\"#app\")?;") && !wrapper.contains("hydrate"),
            "non-hydratable entry is mount-only (byte-identical to pre-11.12):\n{wrapper}"
        );
    }

    #[test]
    fn compose_cargo_toml_uses_supplied_package_name() {
        let toml = compose_cargo_toml("my_awesome_bin");
        assert!(
            toml.contains(r#"name = "my_awesome_bin""#),
            "package.name must reflect the supplied bin name:\n{toml}"
        );
    }

    #[test]
    fn compose_cargo_toml_bit_for_bit_matches_counter_baseline_when_named_counter() {
        // If this test fails, either the counter baseline drifted
        // or the composer drifted. When intentional, update
        // `examples/view/counter/wasm-crate/Cargo.toml` to match
        // (that file is committed baseline for the smoke test).
        let composed = compose_cargo_toml("counter");
        let baseline = include_str!("../../examples/view/counter/wasm-crate/Cargo.toml");
        // Normalise line endings to LF for the comparison (Windows
        // clones may check out CRLF).
        let composed_lf = composed.replace("\r\n", "\n");
        let baseline_lf = baseline.replace("\r\n", "\n");
        assert_eq!(
            composed_lf, baseline_lf,
            "composed Cargo.toml drifted from the counter baseline:\n\
             --- composed ---\n{composed_lf}\n\
             --- baseline ---\n{baseline_lf}"
        );
    }

    #[test]
    fn sanitise_wasm_pkg_name_converts_hyphens_to_underscores() {
        assert_eq!(sanitise_wasm_pkg_name("web-client"), "web_client");
        assert_eq!(sanitise_wasm_pkg_name("my-cool-bin"), "my_cool_bin");
    }

    #[test]
    fn sanitise_wasm_pkg_name_lowercases_defensively() {
        assert_eq!(sanitise_wasm_pkg_name("Counter"), "counter");
        assert_eq!(sanitise_wasm_pkg_name("Web-Client"), "web_client");
    }

    #[test]
    fn sanitise_wasm_pkg_name_falls_back_when_empty() {
        assert_eq!(sanitise_wasm_pkg_name(""), "wasm_bundle");
        // All chars stripped.
        assert_eq!(sanitise_wasm_pkg_name("!@#$"), "wasm_bundle");
    }

    #[test]
    fn sanitise_wasm_pkg_name_preserves_digits() {
        assert_eq!(sanitise_wasm_pkg_name("app2"), "app2");
    }

    #[test]
    fn write_wasm_crate_scaffold_creates_dir_and_files() {
        let src = r#"component X {
  state { n: Int = 0 }
  <template><span>{n}</span></template>
}
"#;
        let expanded = expand_ok(src);
        let tmp = tempfile::tempdir().unwrap();
        let crate_dir = tmp.path().join("web");

        let result = write_wasm_crate_scaffold(
            &crate_dir,
            &expanded,
            &NominalRegistry::new(),
            &ImportedFnRegistry::new(),
            &ImportedComponentRegistry::new(),
            "web",
            "#app",
            None,
            false,
        )
        .unwrap();
        assert!(result.cargo_toml.is_file(), "Cargo.toml not written");
        assert!(result.lib_rs.is_file(), "src/lib.rs not written");
        assert_eq!(result.crate_dir, crate_dir);

        let cargo_txt = fs::read_to_string(&result.cargo_toml).unwrap();
        assert!(cargo_txt.contains(r#"name = "web""#));

        let lib_txt = fs::read_to_string(&result.lib_rs).unwrap();
        assert!(lib_txt.contains("#[wasm_bindgen(start)]"));
        assert!(lib_txt.contains("root.mount(\"#app\")"));
    }

    #[test]
    fn write_wasm_crate_scaffold_overwrites_existing_files_idempotently() {
        let src = r#"component X {
  state {}
  <template><span>hi</span></template>
}
"#;
        let expanded = expand_ok(src);
        let tmp = tempfile::tempdir().unwrap();
        let crate_dir = tmp.path().join("web");

        // First write.
        write_wasm_crate_scaffold(
            &crate_dir,
            &expanded,
            &NominalRegistry::new(),
            &ImportedFnRegistry::new(),
            &ImportedComponentRegistry::new(),
            "web",
            "#app",
            None,
            false,
        )
        .unwrap();
        // Second write with identical inputs — must not error.
        let result = write_wasm_crate_scaffold(
            &crate_dir,
            &expanded,
            &NominalRegistry::new(),
            &ImportedFnRegistry::new(),
            &ImportedComponentRegistry::new(),
            "web",
            "#app",
            None,
            false,
        )
        .unwrap();
        assert!(result.cargo_toml.is_file());
        assert!(result.lib_rs.is_file());
    }

    // ---- Phase 11.5.d — multi-component composition smoke ---------

    #[test]
    fn phase_11_5_d_compose_lib_rs_end_to_end_with_child_component() {
        // Parent mounts a `<Card />` with two static props. The
        // composer runs the FULL view pipeline (parse → expand →
        // check → emit_module → compose entry) and the emitted
        // Rust must contain: both component impls, the composed
        // entry point mounting the FIRST-declared component
        // (Parent), the wrapper div with `__fitz-child-Card`
        // class, the coerced prop assignments (title as String,
        // count as i64), and the child's `mount_into` call. This
        // is the end-to-end smoke that proves the CLI produces
        // usable WASM output for multi-component projects.
        let src = r#"component Parent {
  state {}
  <template>
    <section>
      <Card title="Welcome" count="3" />
    </section>
  </template>
}
component Card {
  state {
    title: Str = "x"
    count: Int = 0
  }
  <template><div class="card">{title}</div></template>
}"#;
        let expanded = expand_ok(src);
        let out = compose_lib_rs(&expanded, "#app", Some("MultiCompose.fitzv")).unwrap();

        // Both components emit.
        assert!(out.contains("pub struct Parent {"), "Parent struct missing");
        assert!(out.contains("pub struct Card {"), "Card struct missing");
        // Composed entry mounts Parent (first declared) on `#app`.
        assert!(
            out.contains("let root = Parent::new();"),
            "root must be Parent (first declared)"
        );
        assert!(
            out.contains("root.mount(\"#app\")"),
            "root mount selector wrong"
        );
        // Parent's render body creates the Card wrapper.
        assert!(
            out.contains(r#"set_attribute("class", "__fitz-child-Card")"#),
            "wrapper class missing"
        );
        // Coerced props set on the child instance.
        assert!(
            out.contains(r#".title.borrow_mut() = "Welcome".to_string();"#),
            "title prop coercion missing"
        );
        assert!(
            out.contains(".count.borrow_mut() = 3i64;"),
            "count prop coercion missing"
        );
        // Child mounted via mount_into on the wrapper.
        assert!(
            out.contains(".mount_into("),
            "child must be mounted via mount_into"
        );
        // Both `mount(selector)` and `mount_into(root)` present on
        // BOTH components (Parent + Card).
        assert_eq!(
            out.matches("pub fn mount(self: &Rc<Self>, selector: &str)")
                .count(),
            2,
            "both components must have mount(selector)"
        );
        assert_eq!(
            out.matches("pub fn mount_into(self: &Rc<Self>, root: HtmlElement)")
                .count(),
            2,
            "both components must have mount_into(root)"
        );
    }

    // ---- Phase 11.7 R3 — load_imported_nominals -------------------

    use crate::view::ast::Loc;

    fn imp(path: &str, names: &[(&str, Option<&str>)]) -> ExpandedViewImport {
        ExpandedViewImport {
            path: vec![path.to_string()],
            names: names
                .iter()
                .map(|(o, a)| (o.to_string(), a.map(|s| s.to_string())))
                .collect(),
            loc: Loc { line: 1, column: 1 },
        }
    }

    #[test]
    fn load_imported_nominals_reads_sibling_type_fields() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("card.fitz"),
            "type Card {\n  id: Int\n  title: Str\n}\n",
        )
        .unwrap();
        let imports = vec![imp("card", &[("Card", None)])];
        let reg = load_imported_nominals(&imports, tmp.path()).unwrap();
        assert!(reg.contains("Card"), "Card must be registered");
        assert!(!reg.is_empty());
    }

    #[test]
    fn load_imported_nominals_honours_alias() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("card.fitz"), "type Card {\n  id: Int\n}\n").unwrap();
        let imports = vec![imp("card", &[("Card", Some("Row"))])];
        let reg = load_imported_nominals(&imports, tmp.path()).unwrap();
        assert!(
            reg.contains("Row"),
            "aliased import must register under the alias"
        );
        assert!(
            !reg.contains("Card"),
            "the original name is not the local binding under an alias"
        );
    }

    #[test]
    fn load_imported_nominals_skips_missing_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        // No card.fitz on disk.
        let imports = vec![imp("card", &[("Card", None)])];
        let reg = load_imported_nominals(&imports, tmp.path()).unwrap();
        assert!(
            reg.is_empty(),
            "missing sibling must yield an empty registry"
        );
    }

    #[test]
    fn load_imported_nominals_skips_fn_only_import() {
        let tmp = tempfile::tempdir().unwrap();
        // A helper module with no `type` decls — the imported name is a
        // function, so it must NOT be registered as a nominal.
        fs::write(
            tmp.path().join("helpers.fitz"),
            "fn double(n: Int) -> Int {\n  return n * 2\n}\n",
        )
        .unwrap();
        let imports = vec![imp("helpers", &[("double", None)])];
        let reg = load_imported_nominals(&imports, tmp.path()).unwrap();
        assert!(
            reg.is_empty(),
            "a function import must not register a nominal"
        );
    }

    #[test]
    fn load_imported_nominals_registers_only_imported_names() {
        let tmp = tempfile::tempdir().unwrap();
        // Sibling declares two types; only Card is imported.
        fs::write(
            tmp.path().join("card.fitz"),
            "type Card {\n  id: Int\n}\ntype Board {\n  next_id: Int\n}\n",
        )
        .unwrap();
        let imports = vec![imp("card", &[("Card", None)])];
        let reg = load_imported_nominals(&imports, tmp.path()).unwrap();
        assert!(reg.contains("Card"));
        assert!(
            !reg.contains("Board"),
            "only the imported name is registered, not every type in the sibling"
        );
    }

    // ---- Phase 11.7 R3.5a.2 — load_imported_fns -------------------

    #[test]
    fn load_imported_fns_registers_all_fns_including_non_imported() {
        let tmp = tempfile::tempdir().unwrap();
        // The SFC imports only `advance`, but `advance` calls the
        // internal `next_column` — so BOTH must be registered.
        fs::write(
            tmp.path().join("helpers.fitz"),
            "fn next_column(c: Str) -> Str { return c }\n\
             fn advance(c: Str) -> Str { return next_column(c) }\n",
        )
        .unwrap();
        let imports = vec![imp("helpers", &[("advance", None)])];
        let reg = load_imported_fns(&imports, tmp.path()).unwrap();
        assert!(
            reg.contains("advance"),
            "the imported fn must be registered"
        );
        assert!(
            reg.contains("next_column"),
            "an internal (non-imported) helper reachable through an imported fn \
             must be registered too"
        );
    }

    #[test]
    fn cw9_load_imported_fns_registers_alias_binding() {
        // CW.9 — `from icon import icon as render_icon` must register the fn
        // under its local binding (the alias) so a template calling
        // `render_icon(...)` resolves. The real name stays registered too (for
        // any internal sibling call).
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("icon.fitz"),
            "fn icon(n: Str) -> Str { return n }\n",
        )
        .unwrap();
        let imports = vec![imp("icon", &[("icon", Some("render_icon"))])];
        let reg = load_imported_fns(&imports, tmp.path()).unwrap();
        assert!(
            reg.contains("render_icon"),
            "the aliased binding must be registered"
        );
        assert!(
            reg.contains("icon"),
            "the real name stays registered for internal sibling calls"
        );
    }

    #[test]
    fn load_imported_fns_skips_missing_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        let imports = vec![imp("helpers", &[("advance", None)])];
        let reg = load_imported_fns(&imports, tmp.path()).unwrap();
        assert!(reg.is_empty(), "a missing sibling yields an empty registry");
    }

    #[test]
    fn load_imported_fns_nominal_only_sibling_registers_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        // A type-only module (no `fn`s) contributes no functions.
        fs::write(tmp.path().join("card.fitz"), "type Card {\n  id: Int\n}\n").unwrap();
        let imports = vec![imp("card", &[("Card", None)])];
        let reg = load_imported_fns(&imports, tmp.path()).unwrap();
        assert!(
            reg.is_empty(),
            "a nominal-only sibling registers no functions"
        );
    }

    #[test]
    fn load_imported_fns_scans_each_module_once() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("helpers.fitz"),
            "fn a(n: Int) -> Int { return n }\nfn b(n: Int) -> Int { return n }\n",
        )
        .unwrap();
        // Two imports from the same module — the module is scanned once,
        // and both fns are registered.
        let imports = vec![imp("helpers", &[("a", None), ("b", None)])];
        let reg = load_imported_fns(&imports, tmp.path()).unwrap();
        assert!(reg.contains("a") && reg.contains("b"));
    }

    // ---- Phase 11.7 — cross-file `<Child />` (load_imported_components)

    #[test]
    fn load_imported_components_reads_sibling_fitzv_component() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("Card.fitzv"),
            "component Card {\n  state { title: Str = \"\" }\n  <template><article>{title}</article></template>\n}\n",
        )
        .unwrap();
        let imports = vec![imp("Card", &[("Card", None)])];
        let reg = load_imported_components(&imports, tmp.path()).unwrap();
        assert!(reg.contains("Card"), "Card must be registered");
        assert_eq!(reg.get("Card").unwrap().state[0].name, "title");
    }

    #[test]
    fn load_imported_components_registers_all_components_in_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        // A file declaring two components: the imported `Card` plus its
        // file-local `Badge` sibling — both are loaded so `Card` can
        // compose `Badge` internally.
        fs::write(
            tmp.path().join("Card.fitzv"),
            "component Card {\n  state {}\n  <template><article><Badge /></article></template>\n}\ncomponent Badge {\n  state {}\n  <template><b>!</b></template>\n}\n",
        )
        .unwrap();
        let imports = vec![imp("Card", &[("Card", None)])];
        let reg = load_imported_components(&imports, tmp.path()).unwrap();
        assert!(
            reg.contains("Card") && reg.contains("Badge"),
            "both the imported component and its file-local sibling load"
        );
    }

    #[test]
    fn load_imported_components_skips_missing_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        let imports = vec![imp("Card", &[("Card", None)])];
        let reg = load_imported_components(&imports, tmp.path()).unwrap();
        assert!(reg.is_empty(), "a missing sibling yields an empty registry");
    }

    #[test]
    fn load_imported_components_skips_classic_fitz_only_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        // A `.fitz` (classic) sibling with no `.fitzv` → not a component
        // import (handled by the nominal / fn loaders instead).
        fs::write(tmp.path().join("card.fitz"), "type Card {\n  id: Int\n}\n").unwrap();
        let imports = vec![imp("card", &[("Card", None)])];
        let reg = load_imported_components(&imports, tmp.path()).unwrap();
        assert!(
            reg.is_empty(),
            "a classic-only sibling registers no components"
        );
    }

    #[test]
    fn load_imported_components_first_registration_wins_across_files() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("A.fitzv"),
            "component Card {\n  state { n: Int = 1 }\n  <template><span>{n}</span></template>\n}\n",
        )
        .unwrap();
        fs::write(
            tmp.path().join("B.fitzv"),
            "component Card {\n  state { n: Int = 2 }\n  <template><span>{n}</span></template>\n}\n",
        )
        .unwrap();
        let imports = vec![imp("A", &[("Card", None)]), imp("B", &[("Card", None)])];
        let reg = load_imported_components(&imports, tmp.path()).unwrap();
        assert_eq!(reg.components().len(), 1, "one Card survives the collision");
        assert!(
            matches!(
                reg.get("Card").unwrap().state[0].default,
                crate::ast::Expr::Int(1, _)
            ),
            "the first import (A) wins"
        );
    }

    // ---- v0.26.0 — aliasing (load_imported_components) ------------------

    #[test]
    fn load_imported_components_honours_alias_registers_both_names() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("Card.fitzv"),
            "component Card {\n  state { title: Str = \"\" }\n  <template><article>{title}</article></template>\n}\n",
        )
        .unwrap();
        // `from Card import Card as Row` → `<Row />` must resolve, and the
        // original `Card` stays available for a file-local sibling.
        let imports = vec![imp("Card", &[("Card", Some("Row"))])];
        let reg = load_imported_components(&imports, tmp.path()).unwrap();
        assert!(reg.contains("Row"), "the alias binding is registered");
        assert!(reg.contains("Card"), "the original name is kept too");
        assert_eq!(
            reg.get("Row").unwrap().state[0].name,
            "title",
            "the aliased clone carries the original component's surface"
        );
    }

    #[test]
    fn load_imported_components_alias_only_affects_imported_name() {
        let tmp = tempfile::tempdir().unwrap();
        // File declares Card + file-local Badge; only Card is aliased.
        fs::write(
            tmp.path().join("Card.fitzv"),
            "component Card {\n  state {}\n  <template><article><Badge /></article></template>\n}\ncomponent Badge {\n  state {}\n  <template><b>!</b></template>\n}\n",
        )
        .unwrap();
        let imports = vec![imp("Card", &[("Card", Some("Row"))])];
        let reg = load_imported_components(&imports, tmp.path()).unwrap();
        assert!(reg.contains("Row"), "Card aliased to Row");
        assert!(reg.contains("Card"), "original Card kept");
        assert!(
            reg.contains("Badge"),
            "the non-aliased sibling keeps its declared name"
        );
        assert_eq!(
            reg.components().len(),
            3,
            "Card + Row (alias clone) + Badge — no spurious extra"
        );
    }

    // ---- v0.26.0 — transitivity (collect_transitive_view_imports) -------

    #[test]
    fn collect_transitive_view_imports_reaches_grandchild_imports() {
        let tmp = tempfile::tempdir().unwrap();
        // App imports Card; Card.fitzv itself imports Badge.
        fs::write(
            tmp.path().join("Card.fitzv"),
            "from Badge import Badge\ncomponent Card {\n  state {}\n  <template><article><Badge /></article></template>\n}\n",
        )
        .unwrap();
        fs::write(
            tmp.path().join("Badge.fitzv"),
            "component Badge {\n  state {}\n  <template><b>!</b></template>\n}\n",
        )
        .unwrap();
        let entry = vec![imp("Card", &[("Card", None)])];
        let union = collect_transitive_view_imports(&entry, tmp.path());
        assert!(
            union.iter().any(|i| i.path == vec!["Card".to_string()]),
            "entry import is present"
        );
        assert!(
            union.iter().any(|i| i.path == vec!["Badge".to_string()]),
            "the grandchild's Badge import is discovered transitively"
        );
        // And loading over the union registers the grandchild component.
        let reg = load_imported_components(&union, tmp.path()).unwrap();
        assert!(
            reg.contains("Card") && reg.contains("Badge"),
            "the transitively-reached Badge is registered"
        );
    }

    #[test]
    fn collect_transitive_view_imports_is_cycle_safe() {
        let tmp = tempfile::tempdir().unwrap();
        // A imports B, B imports A — the walk must terminate.
        fs::write(
            tmp.path().join("A.fitzv"),
            "from B import B\ncomponent A {\n  state {}\n  <template><B /></template>\n}\n",
        )
        .unwrap();
        fs::write(
            tmp.path().join("B.fitzv"),
            "from A import A\ncomponent B {\n  state {}\n  <template><A /></template>\n}\n",
        )
        .unwrap();
        let entry = vec![imp("A", &[("A", None)])];
        let union = collect_transitive_view_imports(&entry, tmp.path());
        // Terminates; both files' imports are present, each once.
        assert!(union.iter().any(|i| i.path == vec!["A".to_string()]));
        assert!(union.iter().any(|i| i.path == vec!["B".to_string()]));
    }

    #[test]
    fn collect_transitive_view_imports_no_siblings_is_identity() {
        let tmp = tempfile::tempdir().unwrap();
        // No `.fitzv` siblings on disk — the union is just the entry imports.
        let entry = vec![imp("Card", &[("Card", None)]), imp("util", &[("f", None)])];
        let union = collect_transitive_view_imports(&entry, tmp.path());
        assert_eq!(union.len(), 2, "nothing transitive → union == entry");
    }

    // ---- CW.8: dep-aware view import resolution -------------------------

    /// Multi-segment import (`from dep.ui.Badge import Badge`).
    fn imp_path(segs: &[&str], names: &[(&str, Option<&str>)]) -> ExpandedViewImport {
        ExpandedViewImport {
            path: segs.iter().map(|s| s.to_string()).collect(),
            names: names
                .iter()
                .map(|(o, a)| (o.to_string(), a.map(|s| s.to_string())))
                .collect(),
            loc: Loc { line: 1, column: 1 },
        }
    }

    /// Writes a minimal dependency layout under `root`:
    /// `<root>/<dep>/src/{lib.fitz, ui/<Comp>.fitzv}`. Returns a
    /// `DepRegistry` mapping the dep name to its `lib.fitz` entry.
    fn write_dep(root: &Path, dep_name: &str, comps: &[(&str, &str)]) -> DepRegistry {
        let src = root.join(dep_name).join("src");
        std::fs::create_dir_all(src.join("ui")).unwrap();
        let lib_entry = src.join("lib.fitz");
        std::fs::write(&lib_entry, "// lib\n").unwrap();
        for (name, body) in comps {
            std::fs::write(src.join("ui").join(format!("{name}.fitzv")), body).unwrap();
        }
        let mut reg = DepRegistry::new();
        reg.insert(dep_name.to_string(), lib_entry);
        reg
    }

    const BADGE_FITZV: &str = "component Badge {\n  state { label: Str = \"\" }\n  \
         <template><span class=\"badge\">{label}</span></template>\n}\n";
    const CHIP_FITZV: &str = "component Chip {\n  state { text: Str = \"\" }\n  \
         <template><span class=\"chip\">{text}</span></template>\n}\n";

    #[test]
    fn cw8_resolve_view_import_dep_single_segment_returns_lib_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = write_dep(tmp.path(), "mylib", &[]);
        let hit = resolve_view_import(&imp("mylib", &[("X", None)]), tmp.path(), &reg, "fitz");
        assert_eq!(hit.unwrap().file_name().unwrap(), "lib.fitz");
    }

    #[test]
    fn cw8_resolve_view_import_dep_dotted_resolves_under_dep_root() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = write_dep(tmp.path(), "mylib", &[("Badge", BADGE_FITZV)]);
        let hit = resolve_view_import(
            &imp_path(&["mylib", "ui", "Badge"], &[("Badge", None)]),
            tmp.path(),
            &reg,
            "fitzv",
        )
        .expect("dep sub-path resolves");
        assert_eq!(hit.file_name().unwrap(), "Badge.fitzv");
    }

    #[test]
    fn cw8_resolve_view_import_framework_builtin_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        // Even with the dep registered, a `flv`-only import must never load a
        // file — the emitter special-cases it; loading would drag the whole
        // framework lib entry into the transpile.
        let reg = write_dep(tmp.path(), "fitz_liveviews", &[]);
        let hit = resolve_view_import(
            &imp("fitz_liveviews", &[("flv", None)]),
            tmp.path(),
            &reg,
            "fitz",
        );
        assert!(hit.is_none(), "framework builtin import must not resolve");
    }

    #[test]
    fn cw8_resolve_view_import_sibling_fallback_when_not_a_dep() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Card.fitzv"), BADGE_FITZV).unwrap();
        let reg = DepRegistry::new(); // empty → sibling model
        let hit = resolve_view_import(&imp("Card", &[("Badge", None)]), tmp.path(), &reg, "fitzv");
        assert_eq!(hit.unwrap().file_name().unwrap(), "Card.fitzv");
        // Multi-segment non-dep → None (unchanged skip).
        let none = resolve_view_import(
            &imp_path(&["a", "b"], &[("X", None)]),
            tmp.path(),
            &reg,
            "fitzv",
        );
        assert!(none.is_none());
    }

    #[test]
    fn cw8_load_imported_components_with_deps_resolves_dep_component() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = write_dep(tmp.path(), "mylib", &[("Badge", BADGE_FITZV)]);
        let imports = vec![imp_path(&["mylib", "ui", "Badge"], &[("Badge", None)])];
        let registry = load_imported_components_with_deps(&imports, tmp.path(), &reg).unwrap();
        assert!(
            registry.contains("Badge"),
            "dep component Badge must be registered"
        );
    }

    #[test]
    fn cw8_dedupe_on_resolved_path_scans_both_dep_subpaths() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = write_dep(
            tmp.path(),
            "mylib",
            &[("Badge", BADGE_FITZV), ("Chip", CHIP_FITZV)],
        );
        // Two imports sharing the dep name `mylib` but different sub-paths.
        // The old `path[0]` dedupe would drop the second; the resolved-path
        // dedupe scans both.
        let imports = vec![
            imp_path(&["mylib", "ui", "Badge"], &[("Badge", None)]),
            imp_path(&["mylib", "ui", "Chip"], &[("Chip", None)]),
        ];
        let registry = load_imported_components_with_deps(&imports, tmp.path(), &reg).unwrap();
        assert!(registry.contains("Badge"), "Badge registered");
        assert!(
            registry.contains("Chip"),
            "Chip registered (not deduped away by shared dep name)"
        );
    }

    #[test]
    fn cw8_dep_component_composes_into_lib_rs_end_to_end() {
        // A consumer `.fitzv` imports a component from a `fitz.toml`
        // dependency by dotted sub-path and composes `<Badge/>`. The full
        // pipeline (transitive collect → load components → check → compose)
        // must inline the dep component's struct into the standalone crate.
        let tmp = tempfile::tempdir().unwrap();
        let reg = write_dep(tmp.path(), "mylib", &[("Badge", BADGE_FITZV)]);
        let src = "from mylib.ui.Badge import Badge\n\ncomponent App {\n  \
             state { n: Int = 0 }\n  \
             <template><div><Badge label=\"hi\" /></div></template>\n}\n";
        let raw = view::parse(src).expect("parse consumer");
        let expanded = view::expand(&raw).expect("expand consumer");
        let all = collect_transitive_view_imports_with_deps(&expanded.imports, tmp.path(), &reg);
        let components = load_imported_components_with_deps(&all, tmp.path(), &reg).unwrap();
        let errs = view::check_with_imported_components(&expanded, components.components());
        assert!(errs.is_empty(), "check errors: {errs:?}");
        let nominals = load_imported_nominals_with_deps(&all, tmp.path(), &reg).unwrap();
        let fns = load_imported_fns_with_deps(&all, tmp.path(), &reg).unwrap();
        let out = compose_lib_rs_with_components(
            &expanded,
            &nominals,
            &fns,
            &components,
            "#app",
            Some("App.fitzv"),
        )
        .unwrap();
        assert!(
            out.contains("pub struct Badge"),
            "dep component Badge must be inlined into lib.rs:\n{out}"
        );
        assert!(out.contains("pub struct App"), "root App present:\n{out}");
    }

    #[test]
    fn cw8_empty_registry_is_byte_for_byte_sibling_only() {
        // With an empty dep registry the wrapper path must behave exactly like
        // the old sibling-only loader: a single-segment `.fitzv` sibling loads,
        // a dep-style multi-segment import resolves to nothing.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Card.fitzv"), BADGE_FITZV).unwrap();
        let reg = DepRegistry::new();
        let imports = vec![
            imp("Card", &[("Badge", None)]),
            imp_path(&["mylib", "ui", "Badge"], &[("Badge", None)]),
        ];
        let registry = load_imported_components_with_deps(&imports, tmp.path(), &reg).unwrap();
        assert!(registry.contains("Badge"), "sibling Card.fitzv loads");
    }
}
