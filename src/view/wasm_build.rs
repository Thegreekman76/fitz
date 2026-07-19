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

use super::codegen_wasm::{
    emit_module_with_imports, EmitError, EmitResult, ImportedFnRegistry, NominalRegistry,
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
    let root = match expanded.components.first() {
        Some(c) => c,
        None => {
            return Err(EmitError {
                message: NO_COMPONENTS_MSG.to_string(),
                context: "compose_lib_rs (Phase 11.5.c)".to_string(),
            });
        }
    };

    let emitted = emit_module_with_imports(expanded, nominals, fns)?;

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
    out.push_str(&compose_entry_wrapper(&root.name, mount_selector));
    Ok(out)
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
    use crate::ast::Stmt;

    let mut registry = NominalRegistry::new();
    for imp in imports {
        // MVP: single-segment sibling modules only.
        if imp.path.len() != 1 {
            continue;
        }
        let stem = &imp.path[0];
        let sibling = base_dir.join(format!("{stem}.fitz"));
        if !sibling.is_file() {
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
    use crate::ast::Stmt;

    let mut registry = ImportedFnRegistry::new();
    let mut scanned: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for imp in imports {
        if imp.path.len() != 1 {
            continue;
        }
        let stem = imp.path[0].clone();
        // Scan each sibling module at most once.
        if !scanned.insert(stem.clone()) {
            continue;
        }
        let sibling = base_dir.join(format!("{stem}.fitz"));
        if !sibling.is_file() {
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
                ..
            } = stmt
            {
                let param_list: Vec<(String, Option<crate::ast::TypeExpr>)> = params
                    .iter()
                    .map(|p| (p.name.clone(), p.type_.clone()))
                    .collect();
                registry.insert(name.clone(), param_list, return_type.clone(), body.clone());
            }
        }
    }
    Ok(registry)
}

/// The `#[wasm_bindgen(start)]` wrapper appended to the emitter
/// output. Instantiates `<Root>::new()` and calls
/// `mount("<selector>")`.
fn compose_entry_wrapper(root_component: &str, mount_selector: &str) -> String {
    // Escape backslashes / quotes for the Rust string literal. In
    // practice `mount` is a CSS selector like `"#app"` — no such
    // chars appear — but doing the escape keeps the emitter honest
    // if someone declares `mount = "[data-app='x']"` later.
    let escaped_selector = mount_selector.replace('\\', "\\\\").replace('"', "\\\"");

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
         \x20\x20\x20\x20Ok(())\n\
         }}\n",
        root = root_component,
        selector = escaped_selector,
    )
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
    compose_cargo_toml_with_features(package_name, &[])
}

/// Like [`compose_cargo_toml`], but appends `extra_features` to the
/// `web-sys` feature list (Phase 11.7 R3.5b.2). A `data-flv-submit` form
/// reads inputs via `HtmlInputElement`, which needs the matching feature.
/// With `extra_features` empty this is byte-for-byte identical to
/// [`compose_cargo_toml`], so form-free examples' `Cargo.toml` is
/// unchanged. Callers derive the extra set via
/// [`super::codegen_wasm::wasm_extra_web_sys_features`].
pub fn compose_cargo_toml_with_features(package_name: &str, extra_features: &[&str]) -> String {
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
    let mut feature_lines = String::new();
    for f in BASE_FEATURES.iter().chain(extra_features.iter()) {
        feature_lines.push_str(&format!("    \"{f}\",\n"));
    }
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

# `web-sys` con el subset EXACTO que el emitter de Fase 11.4.b usa
# (ver `src/view/codegen_wasm.rs::emit_module_header` + los emit_*
# helpers). Cualquier feature que sobre suma bytes al bundle final.
[dependencies.web-sys]
version = "0.3"
features = [
{feature_lines}]
"##,
        name = package_name,
        feature_lines = feature_lines,
    )
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
/// - `bin_name` — the sanitised crate name. Used as
///   `package.name` in the emitted `Cargo.toml`.
/// - `mount_selector` — the CSS selector for the composed
///   `start()` wrapper. Comes from the `[[bin]] mount = "..."`
///   field.
/// - `source_label` — an identifier for the `.fitzv` source
///   embedded in the `lib.rs` header comment (typically the
///   bin's `main` path relative to the manifest).
pub fn write_wasm_crate_scaffold(
    crate_dir: &Path,
    expanded: &ExpandedViewFile,
    nominals: &NominalRegistry,
    fns: &ImportedFnRegistry,
    bin_name: &str,
    mount_selector: &str,
    source_label: Option<&str>,
) -> Result<ScaffoldResult, ScaffoldError> {
    // Phase 11.7 R3.5b.2 — a `data-flv-submit` form needs the
    // `HtmlInputElement` web-sys feature; form-free crates keep the base
    // feature set (byte-identical Cargo.toml).
    let extra_features = super::codegen_wasm::wasm_extra_web_sys_features(expanded);
    let cargo_toml_text = compose_cargo_toml_with_features(bin_name, &extra_features);
    let lib_rs_text =
        compose_lib_rs_with_imports(expanded, nominals, fns, mount_selector, source_label)?;

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
            "web",
            "#app",
            None,
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
            "web",
            "#app",
            None,
        )
        .unwrap();
        // Second write with identical inputs — must not error.
        let result = write_wasm_crate_scaffold(
            &crate_dir,
            &expanded,
            &NominalRegistry::new(),
            &ImportedFnRegistry::new(),
            "web",
            "#app",
            None,
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
}
