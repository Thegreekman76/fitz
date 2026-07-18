//! Phase 11.7.b R2b — keyed `<Child />` composition inside `{#for}`.
//!
//! Parallel to `tests/view_control_flow_wasm_smoke.rs` but for
//! `examples/view/keyed-composition/App.fitzv`, which mounts a
//! `<Column key="{name}" title="{name}" />` inside a `{#for name in
//! columns}` loop over a `List<Str>` state field. Each keyed child
//! keeps its own local `taps` state across parent re-renders via the
//! WASM emitter's keyed instance cache + reconciliation.
//!
//! - [`regenerate_keyed_composition_lib_rs`] (always runs) —
//!   regenerates `wasm-crate/src/lib.rs` + `Cargo.toml` and asserts
//!   the emitted Rust carries the keyed map field, the per-render
//!   seen set, the `entry(...)` get-or-create, and the reconciliation
//!   `retain`.
//! - [`build_keyed_composition_wasm`] (`#[ignore]`) — regeneration +
//!   `wasm-pack build --release --target web` (needs the wasm
//!   toolchain).

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
        .join("keyed-composition")
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
    let check_errs = fitz::view::check(&expanded);
    if !check_errs.is_empty() {
        let joined: Vec<String> = check_errs.iter().map(|e| e.to_string()).collect();
        panic!(
            "view::check reported {} error(s) on App.fitzv:\n{}",
            joined.len(),
            joined.join("\n")
        );
    }
    fitz::view::compose_lib_rs(&expanded, "#app", Some("App.fitzv"))
        .unwrap_or_else(|e| panic!("view::compose_lib_rs failed on App.fitzv:\n{}", e))
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
fn regenerate_keyed_composition_lib_rs() {
    let lib_rs = generate_lib_rs_from_fitzv();

    let checks: &[(&str, &str)] = &[
        ("pub struct App {", "App struct present"),
        (
            "__child_map_0: RefCell<std::collections::HashMap<String, Rc<Column>>>,",
            "dynamic child site gets a keyed instance cache",
        ),
        (
            "let mut __seen_0 = std::collections::HashSet::<String>::new();",
            "the for loop declares a per-render seen set",
        ),
        (
            "let __key = format!(\"{}\", name);",
            "the key lowers from the loop variable",
        ),
        (
            "__map.entry(__key.clone()).or_insert_with(|| Column::new()).clone()",
            "the keyed child is get-or-created from the map",
        ),
        (
            "self.__child_map_0.borrow_mut().retain(|__k, _| __seen_0.contains(__k));",
            "reconciliation evicts vanished keys after the loop",
        ),
    ];
    for (needle, why) in checks {
        assert!(
            lib_rs.contains(needle),
            "structural invariant broken — {}\nExpected substring: `{}`\n\nFull emitted lib.rs (truncated):\n{}",
            why,
            needle,
            &lib_rs.chars().take(4000).collect::<String>()
        );
    }

    let dst = lib_rs_path();
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    write_if_changed(&dst, &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml("keyed-composition"),
    );
}

#[test]
#[ignore]
fn build_keyed_composition_wasm() {
    let lib_rs = generate_lib_rs_from_fitzv();
    write_if_changed(&lib_rs_path(), &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml("keyed-composition"),
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

    let wasm_path = crate_dir.join("pkg").join("keyed_composition_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- Phase 11.7.b R2b keyed-composition bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
