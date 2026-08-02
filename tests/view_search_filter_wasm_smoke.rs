//! Phase 11.10 slice 3 — keep-node dynamic regions ({#if}/{#for}) on the
//! client-WASM target, for `examples/view/search-filter/App.fitzv`.
//!
//! A value-input component with control flow now stays on keep-node: the
//! live `<input>` is patched in place (caret preserved) while each
//! `{#if}`/`{#for}` is rebuilt as an anchored region.
//!
//! - [`regenerate_search_filter_lib_rs`] (always runs) — regenerates
//!   `wasm-crate/src/lib.rs` + `Cargo.toml` and asserts the emitted Rust
//!   carries the keep-node region scaffolding (`__build`/`__patch`, comment
//!   anchors, `__mount_region_*`/`__patch_region_*`, `DocumentFragment`) plus
//!   the `Comment`/`DocumentFragment` web-sys features.
//! - [`build_search_filter_wasm`] (`#[ignore]`) — regeneration +
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
        .join("search-filter")
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
        "search-filter",
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
fn regenerate_search_filter_lib_rs() {
    let expanded = expanded_from_fitzv();
    let lib_rs = generate_lib_rs(&expanded);

    let checks: &[(&str, &str)] = &[
        (
            "fn __build(self: &Rc<Self>)",
            "keep-node: builds the DOM once",
        ),
        ("fn __patch(self: &Rc<Self>)", "keep-node: patches in place"),
        (
            "if *self.__built.borrow() {",
            "render dispatches build vs patch",
        ),
        (
            "let _ = __el.set_attribute(\"value\", &format!(\"{}\", (*self.query.borrow())))",
            "the live input value is patched in place",
        ),
        (
            "create_comment(\"\").into();",
            "each dynamic region is bounded by comment anchors",
        ),
        (
            "fn __mount_region_0(self: &Rc<Self>)",
            "the first region has a mount method",
        ),
        (
            "fn __patch_region_0(self: &Rc<Self>)",
            "the first region has a patch method",
        ),
        (
            "create_document_fragment()",
            "region content is built into a DocumentFragment",
        ),
        ("self.__patch_region_0();", "patch re-runs the region"),
        (
            "insert_before(&__frag, Some(__e))",
            "the region fragment is inserted before the end anchor",
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

    let cargo = generate_cargo_toml(&expanded);
    for f in ["\"Comment\",", "\"DocumentFragment\","] {
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
fn build_search_filter_wasm() {
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

    let wasm_path = crate_dir.join("pkg").join("search_filter_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- slice 3 search-filter bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
