//! v0.24.0 — named slots (`<slot name="X" />`) on the client-WASM target.
//!
//! `examples/view/named-slots/App.fitzv`: child `Card` declares
//! `<slot name="title" />`, a default `<slot />`, and
//! `<slot name="actions" />`, each with fallback. The parent's first
//! `<Card>` fills all three (routing by `slot="..."` attributes); the
//! second `<Card>` fills only the title, so body + actions render their
//! fallback. Asserts the per-slot fields, the routed renderers + wiring,
//! and that the routing `slot` attribute is stripped from output.
//!
//! - [`regenerate_named_slots_lib_rs`] (always runs).
//! - [`build_named_slots_wasm`] (`#[ignore]`) — real `wasm-pack build`.

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
        .join("named-slots")
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
fn regenerate_named_slots_lib_rs() {
    let lib_rs = generate_lib_rs_from_fitzv();

    let checks: &[(&str, &str)] = &[
        (
            "__slot_title: RefCell<Option<Rc<dyn Fn(&web_sys::Node)>>>,",
            "the child (Card) gets a __slot_title field",
        ),
        (
            "__slot: RefCell<Option<Rc<dyn Fn(&web_sys::Node)>>>,",
            "the child gets a default __slot field",
        ),
        (
            "__slot_actions: RefCell<Option<Rc<dyn Fn(&web_sys::Node)>>>,",
            "the child gets a __slot_actions field",
        ),
        (
            "if let Some(__cb) = self.__slot_title.borrow().as_ref() {",
            "the named title slot renders from its own backing field",
        ),
        (
            "if let Some(__cb) = self.__slot_actions.borrow().as_ref() {",
            "the named actions slot renders from its own backing field",
        ),
        (
            "fn __render_slot_0(self: &Rc<Self>, __target: &web_sys::Node) {",
            "the parent synthesises a slot-content renderer",
        ),
        (
            ".__slot_title.borrow_mut() = Some(Rc::new(move |__t: &web_sys::Node| __parent.__render_slot_",
            "the parent wires the title slot to a renderer",
        ),
        (
            ".__slot_actions.borrow_mut() = Some(Rc::new(move |__t: &web_sys::Node| __parent.__render_slot_",
            "the parent wires the actions slot to a renderer",
        ),
        (
            "App::like(&__self_clone);",
            "the actions slot content's @click wires to the PARENT handler",
        ),
    ];
    for (needle, why) in checks {
        assert!(
            lib_rs.contains(needle),
            "structural invariant broken — {}\nExpected substring: `{}`\n\nFull emitted lib.rs (truncated):\n{}",
            why,
            needle,
            &lib_rs.chars().take(14000).collect::<String>()
        );
    }
    // The routing `slot="..."` attribute must not leak into the DOM.
    assert!(
        !lib_rs.contains("set_attribute(\"slot\""),
        "the routing `slot` attribute must be stripped from output:\n{lib_rs}"
    );
    // The first Card fills three slots; the second fills only the title →
    // four renderer methods total (3 + 1).
    assert!(
        lib_rs.contains("fn __render_slot_3(self: &Rc<Self>, __target: &web_sys::Node) {")
            && !lib_rs.contains("fn __render_slot_4("),
        "exactly four slot renderers expected (3 for the full card + 1 for the title-only card):\n{lib_rs}"
    );

    let dst = lib_rs_path();
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    write_if_changed(&dst, &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml_with_features("named-slots", &[], false, true),
    );
}

#[test]
#[ignore]
fn build_named_slots_wasm() {
    let lib_rs = generate_lib_rs_from_fitzv();
    write_if_changed(&lib_rs_path(), &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml_with_features("named-slots", &[], false, true),
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

    let wasm_path = crate_dir.join("pkg").join("named_slots_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- v0.24.0 named-slots bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
