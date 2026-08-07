//! Phase 11.7.c — child→parent event bubbling on the client-WASM target.
//!
//! `examples/view/event-bubbling/App.fitzv` binds `<Item @choose="on_pick"
//! />`: the child's `choose` event fires the parent App's `on_pick`
//! handler. Asserts the emitted Rust carries the child callback slot, the
//! bubble call in the child handler, and the parent-side wiring.
//!
//! - [`regenerate_event_bubbling_lib_rs`] (always runs).
//! - [`build_event_bubbling_wasm`] (`#[ignore]`) — real `wasm-pack build`.

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
        .join("event-bubbling")
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
fn regenerate_event_bubbling_lib_rs() {
    let lib_rs = generate_lib_rs_from_fitzv();

    let checks: &[(&str, &str)] = &[
        (
            "__on_choose: RefCell<Option<Box<dyn Fn(&std::collections::HashMap<String, String>)>>>,",
            "the child's bubble callback slot carries a payload",
        ),
        (
            "__on_choose: RefCell::new(None),",
            "the child inits the callback slot to None",
        ),
        (
            "if let Some(__cb) = self.__on_choose.borrow().as_ref() { __cb(payload); }",
            "the child's choose handler forwards its payload to the bubble callback",
        ),
        (
            ".__on_choose.borrow_mut() = Some(Box::new(move |__pl: &std::collections::HashMap<String, String>| {",
            "the parent registers a payload-carrying callback on the child instance",
        ),
        (
            "App::on_pick(&__parent, __pl);",
            "the callback passes the bubbled payload to the parent handler",
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
    // The non-bubbled Item state (label) must NOT gain a callback slot
    // beyond `choose`.
    assert!(
        !lib_rs.contains("__on_on_pick"),
        "the parent handler name must not become a child callback slot:\n{lib_rs}"
    );

    let dst = lib_rs_path();
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    write_if_changed(&dst, &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml_with_features("event-bubbling", &[], false, true),
    );
}

#[test]
#[ignore]
fn build_event_bubbling_wasm() {
    let lib_rs = generate_lib_rs_from_fitzv();
    write_if_changed(&lib_rs_path(), &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml_with_features("event-bubbling", &[], false, true),
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

    let wasm_path = crate_dir.join("pkg").join("event_bubbling_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- Phase 11.7.c event-bubbling bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
