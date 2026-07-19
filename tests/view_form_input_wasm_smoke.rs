//! Phase 11.7 R3.5b.2 — form-submit payload on the client-WASM target.
//!
//! Parallel to `tests/view_click_payload_wasm_smoke.rs` but for
//! `examples/view/form-input/App.fitzv`, which wires a `data-flv-submit`
//! form: the handler reads named inputs from the payload at submit time,
//! and `data-flv-clear` resets the field afterwards.
//!
//! - [`regenerate_form_input_lib_rs`] (always runs) — regenerates
//!   `wasm-crate/src/lib.rs` + `Cargo.toml` (with the conditional
//!   `HtmlInputElement` web-sys feature) and asserts the emitted Rust
//!   carries the submit listener, the field read, the handler call, and
//!   the `data-flv-clear` reset.
//! - [`build_form_input_wasm`] (`#[ignore]`) — regeneration +
//!   `wasm-pack build --release --target web` (needs the wasm toolchain).

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn example_dir() -> PathBuf {
    repo_root().join("examples").join("view").join("form-input")
}

fn lib_rs_path() -> PathBuf {
    example_dir().join("wasm-crate").join("src").join("lib.rs")
}

fn cargo_toml_path() -> PathBuf {
    example_dir().join("wasm-crate").join("Cargo.toml")
}

fn expanded_from_fitzv() -> fitz::view::ExpandedViewFile {
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
    expanded
}

fn generate_lib_rs(expanded: &fitz::view::ExpandedViewFile) -> String {
    fitz::view::compose_lib_rs(expanded, "#app", Some("App.fitzv"))
        .unwrap_or_else(|e| panic!("view::compose_lib_rs failed:\n{}", e))
}

fn generate_cargo_toml(expanded: &fitz::view::ExpandedViewFile) -> String {
    let extra = fitz::view::wasm_extra_web_sys_features(expanded);
    fitz::view::compose_cargo_toml_with_features("form-input", &extra)
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
fn regenerate_form_input_lib_rs() {
    let expanded = expanded_from_fitzv();
    let lib_rs = generate_lib_rs(&expanded);

    let checks: &[(&str, &str)] = &[
        (
            "fn add(self: &Rc<Self>, payload: &std::collections::HashMap<String, String>) {",
            "the submit handler takes a payload param",
        ),
        (
            "let __form_el =",
            "the submit listener captures the form element",
        ),
        (
            "__evt.prevent_default();",
            "the submit listener prevents default navigation",
        ),
        (
            "if let Some(__f) = __form_el.query_selector(\"[name=\\\"text\\\"]\").ok().flatten() {",
            "the listener queries the named input",
        ),
        (
            "__payload.insert(\"text\".to_string(), __inp.value());",
            "the field value is read into the payload",
        ),
        (
            "TodoForm::add(&__self_clone, &__payload);",
            "the handler is called with the built payload",
        ),
        (
            "__inp.set_value(\"\");",
            "the data-flv-clear input is reset after submit",
        ),
        (
            "add_event_listener_with_callback(\"submit\"",
            "the listener is attached to the submit event",
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

    // The conditional web-sys feature must be present for the form path.
    let cargo = generate_cargo_toml(&expanded);
    assert!(
        cargo.contains("\"HtmlInputElement\","),
        "the form crate must declare the HtmlInputElement web-sys feature:\n{cargo}"
    );

    let dst = lib_rs_path();
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    write_if_changed(&dst, &lib_rs);
    write_if_changed(&cargo_toml_path(), &cargo);
}

#[test]
#[ignore]
fn build_form_input_wasm() {
    let expanded = expanded_from_fitzv();
    write_if_changed(&lib_rs_path(), &generate_lib_rs(&expanded));
    write_if_changed(&cargo_toml_path(), &generate_cargo_toml(&expanded));

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

    let wasm_path = crate_dir.join("pkg").join("form_input_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- Phase 11.7 R3.5b.2 form-input bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
