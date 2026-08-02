//! Phase 11.12 — SSR → client hydration on the client-WASM target, for
//! `examples/view/hydrate/App.fitzv`.
//!
//! A region-free keep-node component (`@input` over a static template) is
//! HYDRATABLE: the generated `start()` adopts the server-painted DOM when the
//! mount root already has content (restoring the serialized state from the
//! `<script type="application/json">`), instead of wiping + rebuilding.
//!
//! - [`regenerate_hydrate_lib_rs`] (always runs) — regenerates
//!   `wasm-crate/src/lib.rs` + `Cargo.toml` and asserts the emitted Rust
//!   carries the hydration surface: cursor helpers, a `hydrate()` method that
//!   adopts (never `create_element`) into the keep-node handles, a
//!   `__apply_state_json` state restore, a branching `start()`, and the
//!   `serde_json` dep.
//! - [`build_hydrate_wasm`] (`#[ignore]`) — regeneration +
//!   `wasm-pack build --release --target web` (needs the wasm toolchain).

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn example_dir() -> PathBuf {
    repo_root().join("examples").join("view").join("hydrate")
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
        "hydrate",
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
fn regenerate_hydrate_lib_rs() {
    let expanded = expanded_from_fitzv();
    let lib_rs = generate_lib_rs(&expanded);

    let checks: &[(&str, &str)] = &[
        (
            "fn __flv_next_element(__cursor: &mut Option<web_sys::Node>) -> Option<web_sys::Element>",
            "the element cursor helper is emitted",
        ),
        (
            "fn __flv_next_text(__cursor: &mut Option<web_sys::Node>) -> Option<web_sys::Text>",
            "the text cursor helper is emitted",
        ),
        (
            "pub fn hydrate(self: &Rc<Self>, root: HtmlElement) -> Result<(), JsValue>",
            "the hydrate() method is emitted",
        ),
        (
            "fn __apply_state_json(self: &Rc<Self>, __json: &str)",
            "the state-restore method is emitted",
        ),
        (
            "get_element_by_id(\"__flv_state_App\")",
            "hydrate reads the serialized state script by id",
        ),
        (
            "__v.get(\"name\").and_then(|__j| __j.as_str())",
            "the Str state field restores via as_str()",
        ),
        (
            "if let Some(__hn) = __flv_next_text(&mut",
            "the greeting text node is adopted, not created",
        ),
        (
            "return root.hydrate(__el);",
            "the entry wrapper branches to hydrate when the root has DOM",
        ),
        (
            "root.mount(\"#app\")?;",
            "the entry wrapper keeps the fresh-mount fallback",
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

    // The hydrate() body must ADOPT, never create — assert no create_* between
    // the method header and its `__built = true` tail.
    let hstart = lib_rs.find("pub fn hydrate(").expect("hydrate present");
    let htail = lib_rs[hstart..]
        .find("*self.__built.borrow_mut() = true;")
        .expect("hydrate tail present");
    let hbody = &lib_rs[hstart..hstart + htail];
    assert!(
        !hbody.contains("create_element") && !hbody.contains("create_text_node"),
        "hydrate() must adopt, not create nodes:\n{hbody}"
    );

    // The crate needs the serde_json dep for the state restore.
    let cargo = generate_cargo_toml(&expanded);
    assert!(
        cargo.contains("serde_json = \"1\""),
        "hydratable crate declares serde_json:\n{cargo}"
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
fn build_hydrate_wasm() {
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

    let wasm_path = crate_dir.join("pkg").join("hydrate_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- Phase 11.12 hydrate bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
