//! Phase 11.7 R3.5b.1 — click payload on the client-WASM target.
//!
//! Parallel to `tests/view_list_transform_wasm_smoke.rs` but for
//! `examples/view/click-payload/App.fitzv`, which wires a
//! `data-flv-click` button with an interpolated `data-flv-value-*`
//! attribute and reads the value back through `payload` in the handler.
//!
//! - [`regenerate_click_payload_lib_rs`] (always runs) — regenerates
//!   `wasm-crate/src/lib.rs` + `Cargo.toml` and asserts the emitted Rust
//!   carries the payload handler signature, the `payload.has` /
//!   `payload[...]` lowering, the interpolated attr, and the click
//!   listener that reads the value attr into the payload.
//! - [`build_click_payload_wasm`] (`#[ignore]`) — regeneration +
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
        .join("click-payload")
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

    // No imported nominals / fns — pure `List<Int>` + `Str` + payload.
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
fn regenerate_click_payload_lib_rs() {
    let lib_rs = generate_lib_rs_from_fitzv();

    let checks: &[(&str, &str)] = &[
        (
            "fn pick(self: &Rc<Self>, payload: &std::collections::HashMap<String, String>) {",
            "the payload-using handler takes a payload param",
        ),
        (
            "if payload.contains_key(&(\"val\".to_string())) {",
            "payload.has lowers to contains_key in the guard",
        ),
        (
            "let __rhs = payload.get(&(\"val\".to_string())).cloned().unwrap_or_default();",
            "payload[key] lowers to a get().cloned().unwrap_or_default()",
        ),
        (
            ".set_attribute(\"data-flv-value-val\", &format!(\"{}\", n)).unwrap();",
            "the interpolated value attr is set from the loop var",
        ),
        (
            "let __evt_el =",
            "the click listener captures the element to read its value attrs",
        ),
        (
            "__payload.insert(\"val\".to_string(), __evt_el.get_attribute(\"data-flv-value-val\").unwrap_or_default());",
            "the listener reads the value attr into the payload",
        ),
        (
            "Picker::pick(&__self_clone, &__payload);",
            "the click listener calls the handler with the built payload",
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
        &fitz::view::compose_cargo_toml("click-payload"),
    );
}

#[test]
#[ignore]
fn build_click_payload_wasm() {
    let lib_rs = generate_lib_rs_from_fitzv();
    write_if_changed(&lib_rs_path(), &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml("click-payload"),
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

    let wasm_path = crate_dir.join("pkg").join("click_payload_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- Phase 11.7 R3.5b.1 click-payload bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
