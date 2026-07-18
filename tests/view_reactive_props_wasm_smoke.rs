//! Phase 11.7.a — reactive interpolated child props smoke.
//!
//! Parallel to `tests/view_showcase_wasm_smoke.rs` but for
//! `examples/view/reactive-props/App.fitzv`, the first client-WASM
//! slice of Phase 11.7: a parent `App` passes two of its own state
//! fields down to a `<Badge />` child as INTERPOLATED props
//! (`heading="{title}"`, `count="{clicks + 1}"`).
//!
//! Two tests:
//!
//! - [`regenerate_reactive_props_lib_rs`] (always runs). Runs the
//!   full view pipeline (parse → expand → check → compose_lib_rs)
//!   and writes `wasm-crate/src/lib.rs` + `Cargo.toml`. Keeps the
//!   committed baseline in sync with the emitter so a fresh clone
//!   can `wasm-pack build` without running the smoke first. Also
//!   asserts the interpolated props lowered as expected (parent
//!   state read for the Str prop, arithmetic for the Int prop).
//!
//! - [`build_reactive_props_wasm`] (`#[ignore]`). Same regeneration
//!   plus `wasm-pack build --release --target web`. Requires the
//!   wasm toolchain — opt-in via `-- --ignored`.

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
        .join("reactive-props")
}

fn lib_rs_path() -> PathBuf {
    example_dir().join("wasm-crate").join("src").join("lib.rs")
}

fn cargo_toml_path() -> PathBuf {
    example_dir().join("wasm-crate").join("Cargo.toml")
}

/// Load `App.fitzv`, run parse → expand → check → `compose_lib_rs`
/// — the exact same helper `fitz build --target wasm-client` uses.
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
fn regenerate_reactive_props_lib_rs() {
    let lib_rs = generate_lib_rs_from_fitzv();

    let checks: &[(&str, &str)] = &[
        ("pub struct App {", "App struct present"),
        ("pub struct Badge {", "Badge struct present"),
        ("let root = App::new();", "root instantiation is App"),
        (
            r#"set_attribute("class", "__fitz-child-Badge")"#,
            "one <Badge /> composition site",
        ),
        // Phase 11.7.a — the interpolated props lowered against the
        // PARENT's state: bare Str field read + numeric arithmetic.
        (
            "(*self.title.borrow()).clone()",
            "interpolated Str prop reads parent state field",
        ),
        (
            "((*self.clicks.borrow()) + 1i64)",
            "interpolated arithmetic prop reads parent state field",
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
        &fitz::view::compose_cargo_toml("reactive-props"),
    );
}

#[test]
#[ignore]
fn build_reactive_props_wasm() {
    let lib_rs = generate_lib_rs_from_fitzv();
    write_if_changed(&lib_rs_path(), &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml("reactive-props"),
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

    let wasm_path = crate_dir.join("pkg").join("reactive_props_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- Phase 11.7.a reactive-props bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
