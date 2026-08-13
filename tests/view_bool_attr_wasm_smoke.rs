//! Form B (gotcha #6, v0.38.0) — conditional boolean attributes
//! (`checked={expr}` / `disabled={expr}`) on the client-WASM target, for
//! `examples/view/bool-attr/App.fitzv`.
//!
//! The attribute is present in the DOM iff `expr` is truthy (the HTML
//! boolean-attribute model). This is a keep-node component (it has a live
//! `@input` form control), so a state change PATCHES in place: the emitter
//! stashes each bool-attr element and toggles presence with `set_attribute` /
//! `remove_attribute` on re-render.
//!
//! - [`regenerate_bool_attr_lib_rs`] (always runs) — regenerates
//!   `wasm-crate/src/lib.rs` + `Cargo.toml` and asserts the emitted Rust
//!   carries the build-time `set_attribute(name, "")` guarded by the cond and
//!   the keep-node patch that toggles `set_attribute` / `remove_attribute`.
//! - [`build_bool_attr_wasm`] (`#[ignore]`) — regeneration + `wasm-pack build
//!   --release --target web` (needs the wasm toolchain). Proves the emitted
//!   Rust actually compiles to WebAssembly.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn example_dir() -> PathBuf {
    repo_root().join("examples").join("view").join("bool-attr")
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
    fitz::view::compose_cargo_toml_with_features(
        "bool-attr",
        &extra,
        false,
        fitz::view::file_uses_hydration(expanded),
    )
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
fn regenerate_bool_attr_lib_rs() {
    let expanded = expanded_from_fitzv();
    let lib_rs = generate_lib_rs(&expanded);

    let checks: &[(&str, &str)] = &[
        (
            r#".set_attribute("disabled", "").unwrap();"#,
            "the button's `disabled` bool attr build-sets the attribute",
        ),
        (
            r#".set_attribute("checked", "").unwrap();"#,
            "the checkbox's `checked` bool attr build-sets the attribute",
        ),
        (
            r#"let _ = __el.set_attribute("disabled", "");"#,
            "the keep-node patch re-sets `disabled` when truthy",
        ),
        (
            r#"let _ = __el.remove_attribute("disabled");"#,
            "the keep-node patch removes `disabled` when falsy",
        ),
        (
            r#"let _ = __el.remove_attribute("checked");"#,
            "the keep-node patch removes `checked` when falsy",
        ),
    ];
    for (needle, why) in checks {
        assert!(
            lib_rs.contains(needle),
            "structural invariant broken — {}\nExpected substring: `{}`\n\nFull emitted lib.rs (truncated):\n{}",
            why,
            needle,
            &lib_rs.chars().take(12000).collect::<String>()
        );
    }

    let dst = lib_rs_path();
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    write_if_changed(&dst, &lib_rs);
    write_if_changed(&cargo_toml_path(), &generate_cargo_toml(&expanded));
}

#[test]
#[ignore]
fn build_bool_attr_wasm() {
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
}
