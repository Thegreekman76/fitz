//! Phase 11.7 — cross-file `<Child />` composition on the client-WASM
//! target.
//!
//! Parallel to `tests/view_nominal_list_wasm_smoke.rs` but for
//! `examples/view/cross-file-child/App.fitzv`, whose `<Card />` is
//! declared in a SEPARATE `.fitzv` file (`Card.fitzv`) and imported with
//! `from Card import Card`. The emitter loads the sibling `.fitzv`
//! (`view::load_imported_components`), inlines the whole `Card` component
//! into the generated crate, and the parent's `<Card title="..."
//! @like="..." />` composition (props + event bubbling + named/default
//! slots) lowers to real Rust.
//!
//! - [`regenerate_cross_file_child_lib_rs`] (always runs) — loads the
//!   imported component, runs the cross-file checker, regenerates
//!   `wasm-crate/src/lib.rs` + `Cargo.toml`, and asserts the emitted Rust
//!   carries: the imported `Card` struct + its state, both a default and
//!   a named `badge` slot field, the cross-file child cache slots on the
//!   parent, the static prop fan-in, and the bubbled `like` event slot.
//! - [`build_cross_file_child_wasm`] (`#[ignore]`) — regeneration +
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
        .join("cross-file-child")
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

    // Phase 11.7 — load the sibling `Card.fitzv` component so the emitter
    // can inline it and the checker can validate the parent's `<Card />`
    // composition against the child's real surface. This is the same
    // helper `fitz build --target wasm-client` runs (`main.rs`).
    let components = fitz::view::load_imported_components(&expanded.imports, &example_dir())
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
fn regenerate_cross_file_child_lib_rs() {
    let lib_rs = generate_lib_rs_from_fitzv();

    let checks: &[(&str, &str)] = &[
        (
            "pub struct Card {",
            "the imported cross-file component `Card` is inlined as a struct",
        ),
        (
            "title: RefCell<String>,",
            "the imported Card's `title` state field is emitted",
        ),
        (
            "likes: RefCell<i64>,",
            "the imported Card's `likes` state field is emitted",
        ),
        (
            "pub struct App {",
            "the local parent component `App` is present",
        ),
        (
            "__slot: RefCell<Option<Rc<dyn Fn(&web_sys::Node)>>>,",
            "the imported Card declares a default slot field",
        ),
        (
            "__slot_badge: RefCell<Option<Rc<dyn Fn(&web_sys::Node)>>>,",
            "the imported Card declares a named `badge` slot field",
        ),
        (
            "__on_like: RefCell<Option<Box<dyn Fn(&std::collections::HashMap<String, String>)>>>,",
            "the imported Card gains a bubble slot for its `like` event bound by the parent",
        ),
        (
            "__child_slot_0: RefCell<Option<Rc<Card>>>,",
            "the parent caches the first cross-file Card instance",
        ),
        (
            "__child_slot_1: RefCell<Option<Rc<Card>>>,",
            "the parent caches the second cross-file Card instance",
        ),
        (
            ".title.borrow_mut() = \"Patagonia\".to_string();",
            "a static prop is fanned into the cross-file child",
        ),
        (
            "App::bump(&__parent);",
            "the child's `like` event bubbles up to the parent's `bump` handler",
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

    let dst = lib_rs_path();
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    write_if_changed(&dst, &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml_with_features("cross-file-child", &[], false, true),
    );
}

#[test]
#[ignore]
fn build_cross_file_child_wasm() {
    let lib_rs = generate_lib_rs_from_fitzv();
    write_if_changed(&lib_rs_path(), &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml_with_features("cross-file-child", &[], false, true),
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

    let wasm_path = crate_dir.join("pkg").join("cross_file_child_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- Phase 11.7 cross-file-child bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
