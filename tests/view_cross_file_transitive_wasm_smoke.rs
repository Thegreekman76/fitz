//! v0.26.0 — cross-file `<Child />` composition, TRANSITIVE + ALIAS, on
//! the client-WASM target.
//!
//! Parallel to `tests/view_cross_file_child_wasm_smoke.rs` but exercises
//! the two v0.26.0 refinements together on
//! `examples/view/cross-file-transitive/`:
//!
//!   - ALIAS — `App.fitzv` does `from Card import Card as Row` and
//!     composes `<Row />`; the loader registers a renamed clone under
//!     `Row`, so the parent's composition resolves against it.
//!   - TRANSITIVITY — App imports ONLY `Card`. `Card.fitzv` itself
//!     imports+composes `<Badge />` (a third file). The transitive import
//!     walk (`view::collect_transitive_view_imports`) discovers `Badge`
//!     so its emit is inlined too.
//!
//! - [`regenerate_cross_file_transitive_lib_rs`] (always runs) — computes
//!   the transitive import union, loads the imported components, runs the
//!   cross-file checker, regenerates `wasm-crate/src/lib.rs` + `Cargo.toml`,
//!   and asserts the emitted Rust carries the aliased `Row` struct, the
//!   transitively-reached `Badge` struct, and that the unreached original
//!   `Card` is NOT double-emitted.
//! - [`build_cross_file_transitive_wasm`] (`#[ignore]`) — regeneration +
//!   `wasm-pack build --release --target web` (needs the wasm toolchain).

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn example_dir() -> PathBuf {
    repo_root()
        .join("examples")
        .join("view")
        .join("cross-file-transitive")
}

fn lib_rs_path() -> PathBuf {
    example_dir().join("wasm-crate").join("src").join("lib.rs")
}

fn cargo_toml_path() -> PathBuf {
    example_dir().join("wasm-crate").join("Cargo.toml")
}

fn generate_lib_rs_from_fitzv() -> String {
    let src_path = example_dir().join("App.fitzv");
    let src = fs::read_to_string(&src_path)
        .unwrap_or_else(|e| panic!("failed to read `{}`: {}", src_path.display(), e));

    let raw = fitz::view::parse(&src)
        .unwrap_or_else(|e| panic!("view::parse failed on App.fitzv:\n{}", e));
    let expanded = fitz::view::expand(&raw)
        .unwrap_or_else(|e| panic!("view::expand failed on App.fitzv:\n{}", e));

    // v0.26.0 — walk the `.fitzv` import graph so the transitively-reached
    // `Badge` (imported by `Card.fitzv`, not by App) is discovered. This is
    // the same union `fitz build --target wasm-client` computes (`main.rs`).
    let all_imports =
        fitz::view::collect_transitive_view_imports(&expanded.imports, &example_dir());
    let components = fitz::view::load_imported_components(&all_imports, &example_dir())
        .unwrap_or_else(|e| panic!("view::load_imported_components failed:\n{}", e));

    let check_errs = fitz::view::check_with_imported_components(&expanded, components.components());
    if !check_errs.is_empty() {
        let joined: Vec<String> = check_errs.iter().map(|e| e.to_string()).collect();
        panic!(
            "view::check reported {} error(s) on App.fitzv:\n{}",
            joined.len(),
            joined.join("\n")
        );
    }

    fitz::view::compose_lib_rs_with_components(
        &expanded,
        &fitz::view::NominalRegistry::new(),
        &fitz::view::ImportedFnRegistry::new(),
        &components,
        "#app",
        Some("App.fitzv"),
    )
    .unwrap_or_else(|e| panic!("view::compose_lib_rs_with_components failed:\n{}", e))
}

fn write_if_changed(path: &Path, new_content: &str) {
    let existing = fs::read_to_string(path).ok();
    match existing {
        Some(cur) if cur == new_content => {}
        _ => {
            let mut f = fs::File::create(path)
                .unwrap_or_else(|e| panic!("failed to open `{}` for write: {}", path.display(), e));
            f.write_all(new_content.as_bytes())
                .unwrap_or_else(|e| panic!("failed to write `{}`: {}", path.display(), e));
        }
    }
}

#[test]
fn regenerate_cross_file_transitive_lib_rs() {
    let lib_rs = generate_lib_rs_from_fitzv();

    let checks: &[(&str, &str)] = &[
        (
            "pub struct App {",
            "the local parent component `App` is present",
        ),
        (
            "pub struct Row {",
            "the aliased import `Card as Row` is inlined under the alias name",
        ),
        (
            "pub struct Badge {",
            "the transitively-reached grandchild `Badge` is inlined",
        ),
        (
            "label: RefCell<String>,",
            "the transitive Badge's `label` state field is emitted",
        ),
        (
            "title: RefCell<String>,",
            "the aliased Row's `title` state field is emitted",
        ),
        (
            "App::bump(&__parent);",
            "the aliased child's `like` event bubbles up to the parent's `bump`",
        ),
    ];
    for (needle, why) in checks {
        assert!(
            lib_rs.contains(needle),
            "structural invariant broken — {}\nExpected substring: `{}`\n\nFull emitted lib.rs (truncated):\n{}",
            why,
            needle,
            &lib_rs.chars().take(9000).collect::<String>()
        );
    }

    // The original `Card` name is only reachable via the alias `Row`, so it
    // must NOT be double-emitted as a separate struct.
    assert!(
        !lib_rs.contains("pub struct Card {"),
        "the unreached original `Card` must not be double-emitted alongside the `Row` alias"
    );

    let dst = lib_rs_path();
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    write_if_changed(&dst, &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml("cross-file-transitive"),
    );
}

#[test]
#[ignore]
fn build_cross_file_transitive_wasm() {
    let lib_rs = generate_lib_rs_from_fitzv();
    write_if_changed(&lib_rs_path(), &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml("cross-file-transitive"),
    );

    let crate_dir = example_dir().join("wasm-crate");
    let status = std::process::Command::new("wasm-pack")
        .args(["build", "--release", "--target", "web"])
        .current_dir(&crate_dir)
        .status()
        .unwrap_or_else(|e| panic!("failed to invoke `wasm-pack`: {}", e));
    assert!(
        status.success(),
        "`wasm-pack build --release --target web` exited with {}",
        status
    );

    let wasm_path = crate_dir.join("pkg").join("cross_file_transitive_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- v0.26.0 cross-file-transitive bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
