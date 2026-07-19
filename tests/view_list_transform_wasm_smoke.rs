//! Phase 11.7 R3.5a.1 — closures, `.map`/`.filter`, list reassignment,
//! and `{#for}` over a method-call result on the client-WASM target.
//!
//! Parallel to `tests/view_nominal_list_wasm_smoke.rs` but for
//! `examples/view/list-transform/App.fitzv`, which transforms a
//! `List<Int>` with `.map`/`.filter` closures reassigned back into the
//! state field, and iterates the RESULT of a `.filter(...)` call inside
//! a `{#for}`. No imported nominals — the pure primitive-list path.
//!
//! - [`regenerate_list_transform_lib_rs`] (always runs) — regenerates
//!   `wasm-crate/src/lib.rs` + `Cargo.toml` and asserts the emitted Rust
//!   carries the closure iterator chains, the reassignment snapshot, the
//!   `{#for}` over the filter result, and the `.len()` cast.
//! - [`build_list_transform_wasm`] (`#[ignore]`) — regeneration +
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
        .join("list-transform")
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

    // No imported nominals — pure `List<Int>`.
    fitz::view::compose_lib_rs(&expanded, "#app", Some("App.fitzv"))
        .unwrap_or_else(|e| panic!("view::compose_lib_rs failed:\n{}", e))
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
fn regenerate_list_transform_lib_rs() {
    let lib_rs = generate_lib_rs_from_fitzv();

    let checks: &[(&str, &str)] = &[
        ("pub struct App {", "App struct present"),
        (
            "nums: RefCell<Vec<i64>>,",
            "the List<Int> state field maps to Vec<i64>",
        ),
        (
            ".clone().into_iter().map(|n| (n * 2i64)).collect::<Vec<_>>()",
            "the .map closure lowers to an iterator chain",
        ),
        (
            ".clone().into_iter().filter(|__it| { let n = __it.clone(); (n > 5i64) }).collect::<Vec<_>>()",
            "the .filter closure clones the &T param into an owned binding",
        ),
        (
            "*self.nums.borrow_mut() = __rhs;",
            "the transform is reassigned into the state field via a snapshot",
        ),
        (
            "self.nums.borrow_mut().push((*self.next.borrow()));",
            "live .push mutation with a value from another field",
        ),
        (
            "let __rhs = vec![1i64, 2i64, 3i64, 4i64, 5i64];",
            "reset reassigns a list literal via a vec! macro",
        ),
        (
            ".clone().into_iter().filter(|__it| { let n = __it.clone(); (n > 4i64) }).collect::<Vec<_>>();",
            "the {#for} iterable is a method call snapshotted into a local",
        ),
        (
            ".into_iter() {",
            "the {#for} over a call iterates with .into_iter()",
        ),
        (
            "((*self.nums.borrow())).len() as i64",
            ".len() in an interpolation casts usize to i64",
        ),
    ];
    for (needle, why) in checks {
        assert!(
            lib_rs.contains(needle),
            "structural invariant broken — {}\nExpected substring: `{}`\n\nFull emitted lib.rs (truncated):\n{}",
            why,
            needle,
            &lib_rs.chars().take(8000).collect::<String>()
        );
    }

    let dst = lib_rs_path();
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    write_if_changed(&dst, &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml("list-transform"),
    );
}

#[test]
#[ignore]
fn build_list_transform_wasm() {
    let lib_rs = generate_lib_rs_from_fitzv();
    write_if_changed(&lib_rs_path(), &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml("list-transform"),
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

    let wasm_path = crate_dir.join("pkg").join("list_transform_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- Phase 11.7 R3.5a.1 list-transform bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
