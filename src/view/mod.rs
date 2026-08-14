// view/ — Phase 11 POC: Single-File Components (`.fitzv`).
//
// **Isolated module.** The classic `.fitz` parser does not touch
// this code, and vice versa. A bug here CANNOT break the classic
// pipeline (Invariant 4 of `docs/stack.md`).
//
// Current status: **parser POC**. Recognises the shape of a
// component and produces its own AST in `view::ast`. Does NOT
// evaluate, does NOT type-check, does NOT emit code. The full
// Phase 11 plan (why the extension is `.fitzv`, how the module
// connects with the checker + codegen, how it evolves toward
// SSR/WASM) lives in `docs/fase-11-plan.md`.
//
// `mod view` is declared in `src/lib.rs` as `pub mod view` plain
// (no feature gate) because:
//   - It adds zero new deps to `Cargo.toml`.
//   - The `fitz` binary does not dispatch to this module today —
//     only tests + external tooling can call `view::parse(...)`.
//   - A feature gate would add friction to the smoke without any
//     upside at this POC stage.
//
// Sub-modules:
//   - `ast`    — SFC AST types
//   - `lexer`  — dedicated tokenizer (`.fitzv` is its own dialect)
//   - `parser` — recursive parser + HTML sub-parser for `<template>`

pub mod ast;
pub mod check;
pub mod codegen_ssr;
pub mod codegen_wasm;
pub mod css_parser;
pub mod expand;
pub mod lexer;
pub mod parser;
pub mod wasm_build;

pub use check::{check, check_with_imported_components, CheckError};
pub use codegen_ssr::{emit_component_ssr, emit_module_ssr, SsrEmitError, SsrEmitResult};
pub use codegen_wasm::{
    component_is_hydratable, emit_component, emit_module, emit_module_with_components,
    emit_module_with_imports, emit_module_with_nominals, file_uses_hydration,
    merge_imported_components, wasm_extra_web_sys_features, EmitError, EmitResult,
    ImportedComponentRegistry, ImportedFnRegistry, NominalRegistry,
};
pub use css_parser::{apply_scope, CssParseError};
pub use expand::{
    expand, AttrValueSegment, ExpandError, ExpandResult, ExpandedAttr, ExpandedComponent,
    ExpandedEventHandler, ExpandedStateField, ExpandedStyle, ExpandedTemplate,
    ExpandedTemplateNode, ExpandedViewFile,
};
pub use parser::{parse, ViewParseError, ViewParseResult};
pub use wasm_build::{
    collect_transitive_view_imports, collect_transitive_view_imports_with_deps, compose_cargo_toml,
    compose_cargo_toml_with_features, compose_lib_rs, compose_lib_rs_with_components,
    compose_lib_rs_with_imports, compose_lib_rs_with_nominals, load_imported_components,
    load_imported_components_with_deps, load_imported_fns, load_imported_fns_with_deps,
    load_imported_nominals, load_imported_nominals_with_deps, sanitise_wasm_pkg_name,
    write_wasm_crate_scaffold, ScaffoldError, ScaffoldResult,
};

// ---------------------------------------------------------------------------
// Phase 11.6.d — Module loader bridge
// ---------------------------------------------------------------------------

/// Lower a `.fitzv` source into the classic Fitz source it represents.
///
/// The classic module loader calls this whenever an `import` /
/// `from ... import` resolves to a `.fitzv` file on disk. The
/// returned string is what the loader then feeds to `crate::lexer::
/// tokenize` + `crate::parser::parse` — as if the user had written
/// that classic Fitz source directly.
///
/// Pipeline: `view::parse` → `view::expand` → `view::check` →
/// `view::emit_module_ssr`.
///
/// Any failure at any stage collapses into a single
/// [`crate::error::FitzError`] with `ErrorKind::InvalidSyntax`, a
/// message that names the offending `.fitzv` file plus the stage
/// that failed and its inner message, and best-effort line/column
/// info from the underlying view-side error (0,0 when the stage
/// doesn't carry a location — the check pipeline collects errors
/// with their own `Loc`, we use the first error's location).
///
/// The `file_path_for_errors` parameter is used purely for the
/// error message so the user knows WHICH `.fitzv` broke —
/// resolution and I/O are the caller's responsibility.
///
/// This is the same lowering used by
/// [`crate::view::wasm_build`] but through the SSR emitter
/// instead of the WASM emitter, so a `.fitzv` used as a
/// classic-Fitz import always lowers via SSR (targeting
/// fitz-liveviews). The `wasm-client` target continues to use
/// [`emit_module`] and lives on the `fitz build --target
/// wasm-client` path (Phase 11.5.c).
pub fn transform_fitzv_source(
    source: &str,
    file_path_for_errors: &std::path::Path,
) -> Result<String, crate::error::FitzError> {
    use crate::error::{ErrorKind, FitzError};

    let path_display = file_path_for_errors.display();

    let raw = parse(source).map_err(|e| {
        FitzError::new(
            ErrorKind::InvalidSyntax,
            e.line,
            e.column,
            format!("view parse error in `{path_display}`: {}", e.message),
        )
    })?;

    let expanded = expand(&raw).map_err(|e| {
        FitzError::new(
            ErrorKind::InvalidSyntax,
            e.loc.line,
            e.loc.column,
            format!(
                "view expand error in `{path_display}`: {} ({})",
                e.message, e.context
            ),
        )
    })?;

    let check_errs = check(&expanded);
    if !check_errs.is_empty() {
        // Only the first error's location is carried in the
        // FitzError — the message concatenates every check
        // error so the user sees all of them at once (mirrors
        // how `fitz check` reports type errors).
        let (first_line, first_col) = check_errs
            .first()
            .map(|e| (e.loc.line, e.loc.column))
            .unwrap_or((0, 0));
        let joined = check_errs
            .iter()
            .map(|e| format!("- {} ({})", e.message, e.context))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(FitzError::new(
            ErrorKind::InvalidSyntax,
            first_line,
            first_col,
            format!(
                "view check errors in `{path_display}` ({} error(s)):\n{joined}",
                check_errs.len()
            ),
        ));
    }

    emit_module_ssr(&expanded).map_err(|e| {
        FitzError::new(
            ErrorKind::InvalidSyntax,
            0,
            0,
            format!(
                "view emit_ssr error in `{path_display}`: {} ({})",
                e.message, e.context
            ),
        )
    })
}

/// Phase 11 (gotcha #7) — type-check a `.fitzv` source and return
/// every view error found, WITHOUT lowering to classic Fitz or
/// emitting code. This is what `fitz check` runs when the entry is a
/// `.fitzv`, so a view type error surfaces at `check` time instead of
/// only in `run`/`build`.
///
/// Pipeline: `view::parse` → `view::expand` →
/// `view::check_with_imported_components`. Parse and expand each
/// short-circuit into a single `CheckError` (you can't type-check
/// what you can't parse); the type-check pass accumulates every
/// error. All three stages fold into one uniform `Vec<CheckError>`
/// (via [`CheckError::syntax`] for the two syntax stages) so the
/// caller reports them the same way.
///
/// Cross-file `<Child />` composition is resolved *dep-aware* — the
/// same transitive + `DepRegistry` loaders the `fitz build
/// --target wasm-client` path uses ([`build_wasm_client`] in
/// `src/main.rs`) — so a `check` result matches what a build would
/// see (`from fitz_liveviews.ui.Badge import Badge` resolves through
/// the dependency, not just a flat sibling). Component loading is
/// best-effort: a broken import degrades to an empty registry (the
/// unknown-component error then surfaces from the checker itself)
/// rather than aborting the whole check.
///
/// `base_dir` anchors sibling / dep resolution (the entry `.fitzv`'s
/// directory); `dep_registry` maps `dep-name → lib_entry` from the
/// manifest (empty in single-file mode → sibling-only resolution).
///
/// This does NOT load imported nominals or helper `fn`s — those are
/// needed only by the WASM emitter, not by the checker.
pub fn check_view_source(
    source: &str,
    base_dir: &std::path::Path,
    dep_registry: &crate::manifest::DepRegistry,
) -> Vec<CheckError> {
    let raw = match parse(source) {
        Ok(r) => r,
        Err(e) => {
            return vec![CheckError::syntax(
                format!("view parse error: {}", e.message),
                e.line,
                e.column,
                "view parse",
            )];
        }
    };

    let expanded = match expand(&raw) {
        Ok(x) => x,
        Err(e) => {
            return vec![CheckError::syntax(
                format!("view expand error: {} ({})", e.message, e.context),
                e.loc.line,
                e.loc.column,
                "view expand",
            )];
        }
    };

    // Cross-file `<Child />` — mirror the build path: walk the `.fitzv`
    // import graph (dep-aware) and load every reachable imported
    // component surface so composition validates against the real
    // shape. Best-effort: a load failure degrades to an empty registry.
    let all_imports =
        collect_transitive_view_imports_with_deps(&expanded.imports, base_dir, dep_registry);
    let imported = load_imported_components_with_deps(&all_imports, base_dir, dep_registry)
        .unwrap_or_default();

    check_with_imported_components(&expanded, imported.components())
}

/// True when `path` ends in a case-insensitive `.fitzv`
/// extension. Used by the loader entry points to decide whether
/// to route the source through [`transform_fitzv_source`] before
/// lexing.
pub fn is_fitzv_extension(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("fitzv"))
        .unwrap_or(false)
}

/// Try both `.fitz` and `.fitzv` extensions for the given
/// `<parent_dir>/<stem>` prefix. `.fitz` wins if both exist
/// (backward-compat with pre-11.6.d resolution). Returns
/// `None` when neither exists.
///
/// Consumed by the loader entry points in `src/main.rs`,
/// `src/evaluator.rs`, and `src/codegen.rs` — the resolution
/// logic centralises here so the two-extension behaviour is
/// bit-for-bit consistent across every loader path.
pub fn resolve_module_file_candidates(
    parent_dir: &std::path::Path,
    stem: &str,
) -> Option<std::path::PathBuf> {
    let classic = parent_dir.join(format!("{stem}.fitz"));
    if classic.exists() {
        return Some(classic);
    }
    let view = parent_dir.join(format!("{stem}.fitzv"));
    if view.exists() {
        return Some(view);
    }
    None
}

/// Resolve a dotted sub-path import into a dependency, e.g.
/// `from fitz_liveviews.ui.Pager import X`. `dep_lib_entry` is the path
/// to the dependency's lib entry file (from the `DepRegistry`);
/// `sub_segments` is everything AFTER the dep name (`["ui", "Pager"]`).
/// The remaining segments resolve as a path under the dependency's root
/// directory (the directory containing its lib entry), trying `.fitz`
/// then `.fitzv` on the last segment via
/// [`resolve_module_file_candidates`]. Returns `None` when the target
/// file does not exist under the dependency.
///
/// Shared by every loader path (interpreter / codegen / checker) so the
/// dotted-dep resolution stays bit-for-bit consistent.
pub fn resolve_dep_subpath_file(
    dep_lib_entry: &std::path::Path,
    sub_segments: &[String],
) -> Option<std::path::PathBuf> {
    let (last, dirs) = sub_segments.split_last()?;
    let mut dir = dep_lib_entry.parent()?.to_path_buf();
    for seg in dirs {
        dir.push(seg);
    }
    resolve_module_file_candidates(&dir, last)
}

// ---------------------------------------------------------------------------
// Phase 11.6.d — Loader bridge tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod loader_bridge_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn is_fitzv_extension_matches_lowercase_and_uppercase() {
        assert!(is_fitzv_extension(&PathBuf::from("Comp.fitzv")));
        assert!(is_fitzv_extension(&PathBuf::from("Comp.FITZV")));
        assert!(is_fitzv_extension(&PathBuf::from("path/to/Comp.fItZv")));
        assert!(!is_fitzv_extension(&PathBuf::from("Comp.fitz")));
        assert!(!is_fitzv_extension(&PathBuf::from("Comp")));
        assert!(!is_fitzv_extension(&PathBuf::from("Comp.txt")));
    }

    #[test]
    fn resolve_module_file_candidates_prefers_classic_when_both_exist() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("Comp.fitz"), "// classic").unwrap();
        std::fs::write(tmp.path().join("Comp.fitzv"), "// view").unwrap();
        let hit = resolve_module_file_candidates(tmp.path(), "Comp").unwrap();
        assert!(
            hit.extension().and_then(|s| s.to_str()) == Some("fitz"),
            "when both exist, `.fitz` wins (backward-compat): {hit:?}"
        );
    }

    #[test]
    fn resolve_module_file_candidates_falls_back_to_fitzv_when_only_view_exists() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("Comp.fitzv"), "// view").unwrap();
        let hit = resolve_module_file_candidates(tmp.path(), "Comp").unwrap();
        assert!(
            hit.extension().and_then(|s| s.to_str()) == Some("fitzv"),
            "should fall back to `.fitzv` when `.fitz` is missing: {hit:?}"
        );
    }

    #[test]
    fn resolve_module_file_candidates_returns_none_when_neither_exists() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(resolve_module_file_candidates(tmp.path(), "Missing").is_none());
    }

    #[test]
    fn resolve_dep_subpath_file_resolves_under_dep_root() {
        // A dependency laid out as `<pkg>/src/{lib.fitz, ui/Comp.fitzv}`.
        // `from pkg.ui.Comp import X` → sub_segments `["ui", "Comp"]`
        // resolve under the dep root (the lib entry's parent = `src/`).
        let tmp = tempfile::TempDir::new().unwrap();
        let src = tmp.path().join("pkg").join("src");
        std::fs::create_dir_all(src.join("ui")).unwrap();
        let lib_entry = src.join("lib.fitz");
        std::fs::write(&lib_entry, "// lib").unwrap();
        std::fs::write(src.join("ui").join("Comp.fitzv"), "// view").unwrap();

        let seg = ["ui".to_string(), "Comp".to_string()];
        let hit = resolve_dep_subpath_file(&lib_entry, &seg).expect("resolves under dep root");
        assert_eq!(hit, src.join("ui").join("Comp.fitzv"));

        // `.fitz` wins when both exist (same precedence as the base helper).
        std::fs::write(src.join("ui").join("Comp.fitz"), "// classic").unwrap();
        let hit2 = resolve_dep_subpath_file(&lib_entry, &seg).unwrap();
        assert_eq!(hit2.extension().and_then(|s| s.to_str()), Some("fitz"));

        // Missing sub-path → None (caller emits "module not found").
        let missing = ["ui".to_string(), "Nope".to_string()];
        assert!(resolve_dep_subpath_file(&lib_entry, &missing).is_none());

        // Empty sub-segments → None (defensive; never called that way).
        assert!(resolve_dep_subpath_file(&lib_entry, &[]).is_none());
    }

    #[test]
    fn transform_fitzv_source_emits_classic_fitz_from_a_simple_component() {
        // Baseline: a single-component `.fitzv` lowers to
        // classic Fitz source with the expected shape (import
        // + @live_component + @render_for).
        let src = r#"component Counter {
  state { count: Int = 0 }
  <template><span>{count}</span></template>
}"#;
        let out = transform_fitzv_source(src, std::path::Path::new("Counter.fitzv"))
            .expect("well-formed component transforms");
        assert!(
            out.contains("from fitz_liveviews import Html, html"),
            "missing lib import:\n{out}"
        );
        assert!(
            out.contains("@live_component(\"Counter\")"),
            "missing @live_component decorator:\n{out}"
        );
        assert!(
            out.contains("fn Counter_render(state: Counter) -> Html {"),
            "missing render fn signature:\n{out}"
        );
    }

    #[test]
    fn transform_fitzv_source_wraps_view_parse_error_with_path() {
        let broken = r#"component Counter { NOT_A_VALID_KEYWORD }"#;
        let err = transform_fitzv_source(broken, std::path::Path::new("Broken.fitzv")).unwrap_err();
        assert!(
            err.message.contains("Broken.fitzv"),
            "error must cite the file path:\n{}",
            err.message
        );
        assert!(
            err.message.contains("view parse error") || err.message.contains("view expand error"),
            "error must name the offending stage:\n{}",
            err.message
        );
    }

    #[test]
    fn transform_fitzv_source_wraps_downstream_errors_with_the_path() {
        // Any downstream failure — parse / expand / check /
        // emit — must include the file path in the message so
        // the user knows which `.fitzv` is broken. This is
        // the load-bearing invariant; the exact stage name is
        // secondary (it changes with the stage that catches
        // the error, which depends on the fixture).
        //
        // Fixture uses `event NAME(param) { body }` — events
        // don't accept parameters (the fitz-liveviews `@on`
        // contract is `(state, payload)`), so the emit stage
        // rejects with a 11.7+ pointer.
        let src = r#"component Widget {
  state { title: Str = "" }
  event set_title(next: Str) { title = next }
  <template><span>{title}</span></template>
}"#;
        let err = transform_fitzv_source(src, std::path::Path::new("Widget.fitzv")).unwrap_err();
        assert!(
            err.message.contains("Widget.fitzv"),
            "error must cite the file path:\n{}",
            err.message
        );
        assert!(
            err.message.contains("view parse error")
                || err.message.contains("view expand error")
                || err.message.contains("view check errors")
                || err.message.contains("view emit_ssr error"),
            "error must name at least one of the four stages:\n{}",
            err.message
        );
    }

    #[test]
    fn check_view_source_clean_component_returns_no_errors() {
        // gotcha #7 — a well-formed `.fitzv` produces zero view
        // errors (what `fitz check` reports as "no type errors").
        let src = r#"component Counter {
  state { count: Int = 0 }
  event bump() { count = count + 1 }
  <template><div><span>{count}</span><button @click="bump">+</button></div></template>
}"#;
        let deps = crate::manifest::DepRegistry::new();
        let errs = check_view_source(src, std::path::Path::new("."), &deps);
        assert!(errs.is_empty(), "expected clean check, got: {errs:?}");
    }

    #[test]
    fn check_view_source_parse_error_is_a_single_view_parse_check_error() {
        // An unterminated interpolation short-circuits into ONE
        // CheckError attributed to the "view parse" stage — proving
        // `fitz check` reports it as a view error, not a classic
        // lexer explosion.
        let src = "component Broken {\n  state { x: Int = 0 }\n  <template><div>{x</template>\n}\n";
        let deps = crate::manifest::DepRegistry::new();
        let errs = check_view_source(src, std::path::Path::new("."), &deps);
        assert_eq!(errs.len(), 1, "parse error should be a single error");
        assert_eq!(errs[0].context, "view parse");
        assert!(
            errs[0].message.contains("view parse error"),
            "message: {}",
            errs[0].message
        );
    }

    #[test]
    fn check_view_source_type_error_surfaces_the_checker_message() {
        // A state field default of the wrong type flows through the
        // checker (not the syntax short-circuit), so `context` names
        // the component/field, not a syntax stage.
        let src =
            "component Bad {\n  state { count: Int = \"nope\" }\n  <template><span>{count}</span></template>\n}\n";
        let deps = crate::manifest::DepRegistry::new();
        let errs = check_view_source(src, std::path::Path::new("."), &deps);
        assert!(!errs.is_empty(), "type error must surface");
        assert!(
            errs[0].context.contains("component 'Bad'"),
            "context should name the component/field: {}",
            errs[0].context
        );
        assert!(
            errs[0].message.contains("Int"),
            "message should mention the declared type: {}",
            errs[0].message
        );
    }

    #[test]
    fn transform_fitzv_source_produces_source_that_classic_fitz_lexes_and_parses() {
        // The whole point of the bridge: the emitted string
        // is valid classic Fitz. Run it through the classic
        // lexer + parser and assert both stages succeed.
        //
        // The event body uses the mutation shape the current
        // emitter accepts (no `event` parameters — the
        // fitz-liveviews contract is `(state, payload)`).
        let src = r#"component Widget {
  state { title: Str = "" }
  event set_title() { title = "next" }
  <template>
    <div><h1>{title}</h1><button @click="set_title">tap</button></div>
  </template>
}"#;
        let out = transform_fitzv_source(src, std::path::Path::new("Widget.fitzv"))
            .expect("transform succeeds");
        let tokens = crate::lexer::tokenize(&out)
            .unwrap_or_else(|e| panic!("classic lex must succeed on emitted source: {e}\n\n{out}"));
        crate::parser::parse(tokens)
            .unwrap_or_else(|e| panic!("classic parse must succeed: {e}\n\n{out}"));
    }
}
