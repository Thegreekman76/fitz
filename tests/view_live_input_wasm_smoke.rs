//! CW.9 — live value binding (`@input` / `@change`) on the client-WASM
//! target, for `examples/view/live-input/App.fitzv`.
//!
//! `@input` / `@change` wire a DOM listener that reads the target element's
//! live value into `payload["value"]` and calls the handler (parallel to the
//! SSR emitter's `data-flv-<event>` lowering). This covers `<input>`,
//! `<select>`, and `<textarea>`.
//!
//! - [`regenerate_live_input_lib_rs`] (always runs) — regenerates
//!   `wasm-crate/src/lib.rs` + `Cargo.toml` (with the conditional
//!   `HtmlInputElement` / `HtmlSelectElement` / `HtmlTextAreaElement` web-sys
//!   features) and asserts the emitted Rust carries the value-reading
//!   listeners for both the `input` and `change` events.
//! - [`build_live_input_wasm`] (`#[ignore]`) — regeneration +
//!   `wasm-pack build --release --target web` (needs the wasm toolchain).

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn example_dir() -> PathBuf {
    repo_root().join("examples").join("view").join("live-input")
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
    fitz::view::compose_cargo_toml_with_features("live-input", &extra)
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
fn regenerate_live_input_lib_rs() {
    let expanded = expanded_from_fitzv();
    let lib_rs = generate_lib_rs(&expanded);

    let checks: &[(&str, &str)] = &[
        (
            "move |__evt: Event|",
            "the value-reading listener names its event",
        ),
        ("__evt.target()", "the listener reads the event target"),
        (
            "dyn_ref::<web_sys::HtmlInputElement>()",
            "the target is cast to an input element",
        ),
        (
            "dyn_ref::<web_sys::HtmlSelectElement>()",
            "the target is cast to a select element",
        ),
        (
            "dyn_ref::<web_sys::HtmlTextAreaElement>()",
            "the target is cast to a textarea element",
        ),
        (
            "__payload.insert(\"value\".to_string(), __el.value());",
            "the value goes into the payload under \"value\"",
        ),
        (
            "LiveInput::on_name(&__self_clone, &__payload);",
            "the input handler is called with the payload",
        ),
        (
            "LiveInput::on_color(&__self_clone, &__payload);",
            "the change handler is called with the payload",
        ),
        (
            "add_event_listener_with_callback(\"input\"",
            "an `input` listener is attached",
        ),
        (
            "add_event_listener_with_callback(\"change\"",
            "a `change` listener is attached",
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

    // The conditional web-sys features must be present for the value path.
    let cargo = generate_cargo_toml(&expanded);
    for f in [
        "\"HtmlInputElement\",",
        "\"HtmlSelectElement\",",
        "\"HtmlTextAreaElement\",",
    ] {
        assert!(
            cargo.contains(f),
            "the crate must declare the {f} web-sys feature:\n{cargo}"
        );
    }

    let dst = lib_rs_path();
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    write_if_changed(&dst, &lib_rs);
    write_if_changed(&cargo_toml_path(), &cargo);
}

#[test]
#[ignore]
fn build_live_input_wasm() {
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

    let wasm_path = crate_dir.join("pkg").join("live_input_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- CW.9 live-input bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
