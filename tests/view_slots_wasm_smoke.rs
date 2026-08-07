//! Phase 11.7.d — `<slot />` with fallback on the client-WASM target.
//!
//! `examples/view/slots/App.fitzv`: child `Panel` has a
//! `<slot>fallback</slot>`; the parent fills the first `<Panel>content
//! </Panel>` and leaves the second `<Panel />` to render its fallback.
//! Asserts the child's `__slot` field + slot render, the parent's
//! `__render_slot_0` method, and the wiring.
//!
//! - [`regenerate_slots_lib_rs`] (always runs).
//! - [`build_slots_wasm`] (`#[ignore]`) — real `wasm-pack build`.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn example_dir() -> PathBuf {
    repo_root().join("examples").join("view").join("slots")
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
fn regenerate_slots_lib_rs() {
    let lib_rs = generate_lib_rs_from_fitzv();

    let checks: &[(&str, &str)] = &[
        (
            "__slot: RefCell<Option<Rc<dyn Fn(&web_sys::Node)>>>,",
            "the child (Panel) with a <slot /> gets a __slot field",
        ),
        (
            "if let Some(__cb) = self.__slot.borrow().as_ref() {",
            "the <slot /> renders the parent content if filled",
        ),
        (
            "let __slot_target: &web_sys::Node =",
            "the slot invokes the callback with the target node",
        ),
        (
            "} else {",
            "otherwise the <slot /> renders its fallback",
        ),
        (
            "fn __render_slot_0(self: &Rc<Self>, __target: &web_sys::Node) {",
            "the parent synthesises a slot-content renderer",
        ),
        (
            ".__slot.borrow_mut() = Some(Rc::new(move |__t: &web_sys::Node| __parent.__render_slot_0(__t)));",
            "the parent wires the child __slot to its renderer",
        ),
        (
            "App::greet(&__self_clone);",
            "the slot content's @click wires to the PARENT handler",
        ),
    ];
    for (needle, why) in checks {
        assert!(
            lib_rs.contains(needle),
            "structural invariant broken — {}\nExpected substring: `{}`\n\nFull emitted lib.rs (truncated):\n{}",
            why,
            needle,
            &lib_rs.chars().take(11000).collect::<String>()
        );
    }
    // Only the FIRST Panel is filled → exactly one slot renderer.
    assert!(
        !lib_rs.contains("__render_slot_1"),
        "the self-closing <Panel /> must not add a second slot renderer:\n{lib_rs}"
    );

    let dst = lib_rs_path();
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    write_if_changed(&dst, &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml_with_features("slots", &[], false, true),
    );
}

#[test]
#[ignore]
fn build_slots_wasm() {
    let lib_rs = generate_lib_rs_from_fitzv();
    write_if_changed(&lib_rs_path(), &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml_with_features("slots", &[], false, true),
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

    let wasm_path = crate_dir.join("pkg").join("slots_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- Phase 11.7.d slots bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
