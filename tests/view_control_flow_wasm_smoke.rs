//! Phase 11.7.b — control-flow directives (`{#if}` / `{#for}`) smoke.
//!
//! Parallel to `tests/view_showcase_wasm_smoke.rs` but for
//! `examples/view/control-flow/App.fitzv`, which exercises `{#if}` /
//! `{#else}` and `{#for}` over a `List<primitive>` state field.
//!
//! - [`regenerate_control_flow_lib_rs`] (always runs) — regenerates
//!   `wasm-crate/src/lib.rs` + `Cargo.toml` and asserts the emitted
//!   Rust carries the lowered `if`/`else` + `for` loop.
//! - [`build_control_flow_wasm`] (`#[ignore]`) — regeneration +
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
        .join("control-flow")
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
fn regenerate_control_flow_lib_rs() {
    let lib_rs = generate_lib_rs_from_fitzv();

    let checks: &[(&str, &str)] = &[
        ("pub struct App {", "App struct present"),
        (
            "if ((*self.count.borrow()) > 0i64) {",
            "{#if} lowers to a Rust comparison",
        ),
        ("} else {", "{#else} lowers to a Rust else"),
        (
            "= (*self.labels.borrow()).clone();",
            "{#for} snapshots the state Vec",
        ),
        (".iter().cloned() {", "{#for} iterates the snapshot"),
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
        &fitz::view::compose_cargo_toml("control-flow"),
    );
}

#[test]
#[ignore]
fn build_control_flow_wasm() {
    let lib_rs = generate_lib_rs_from_fitzv();
    write_if_changed(&lib_rs_path(), &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml("control-flow"),
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

    let wasm_path = crate_dir.join("pkg").join("control_flow_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- Phase 11.7.b control-flow bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
