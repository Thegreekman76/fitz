//! Phase 11.12 slice 4 — SSR → client hydration of COMPOSITION (child + slots).
//!
//! `examples/view/hydrate-composition/App.fitzv`: a root `App` marked `hydrate`
//! composes a `Card` that declares a `<slot>`. On boot the client-WASM adopts
//! the server-painted DOM — including the `<div class="__fitz-child-Card">`
//! wrapper and the parent-provided slot content — instead of fresh-mounting.
//!
//! - [`regenerate_hydrate_composition_lib_rs`] (always runs) — regenerates
//!   `lib.rs` + `Cargo.toml` from the `.fitzv` and asserts the slice-4
//!   structural invariants (naive `hydrate()`, `__hslot` fields, adopt of the
//!   child wrapper via `__flv_next_element`, `child.hydrate(...)`, and the
//!   `__hydrate_slot_0` adopt method).
//! - [`build_hydrate_composition_wasm`] (`#[ignore]`) — real `wasm-pack build`.

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
        .join("hydrate-composition")
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
        "hydrate_composition",
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
fn regenerate_hydrate_composition_lib_rs() {
    let expanded = expanded_from_fitzv();
    let lib_rs = generate_lib_rs(&expanded);

    // Write first so the generated output is on disk even if an assertion below
    // fails (aids debugging + the wasm build reuses it).
    {
        let dst = lib_rs_path();
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        write_if_changed(&dst, &lib_rs);
        write_if_changed(&cargo_toml_path(), &generate_cargo_toml(&expanded));
    }

    // Counter-independent structural needles (var names carry a per-walk index).
    let checks: &[(&str, &str)] = &[
        (
            "pub fn hydrate(self: &Rc<Self>, root: HtmlElement) -> Result<(), JsValue>",
            "both App and Card emit a hydrate() method (whole tree opted in)",
        ),
        (
            "__hslot: RefCell<Option<Rc<dyn Fn(&mut Option<web_sys::Node>)>>>,",
            "the slot-declaring child (Card) carries a __hslot adopt-callback field",
        ),
        (
            "fn __hydrate_slot_0(self: &Rc<Self>, __cursor: &mut Option<web_sys::Node>) {",
            "the parent (App) synthesises a __hydrate_slot_0 adopt method",
        ),
        (
            ".__hslot.borrow_mut() = Some(Rc::new(move |__c: &mut Option<web_sys::Node>| __parent.__hydrate_slot_0(__c)));",
            "App wires Card's __hslot to __hydrate_slot_0 during adopt",
        ),
        (
            ".__slot.borrow_mut() = Some(Rc::new(move |__t: &web_sys::Node| __parent.__render_slot_0(__t)));",
            "App also keeps the __render_slot_0 wiring so a later child rebuild fills the slot",
        ),
        (
            "let __hcb = self.__hslot.borrow().clone();",
            "Card.hydrate adopts its <slot> via the __hslot callback",
        ),
        (
            "__hcb(&mut ",
            "the slot adopt callback consumes the child's cursor",
        ),
        (
            "return root.hydrate(__el);",
            "the entry wrapper branches to hydrate (root App opted in)",
        ),
    ];
    for (needle, why) in checks {
        assert!(
            lib_rs.contains(needle),
            "structural invariant broken — {}\nExpected substring: `{}`\n\nFull emitted lib.rs (truncated):\n{}",
            why,
            needle,
            &lib_rs.chars().take(16000).collect::<String>()
        );
    }

    // Both App and Card emit a `hydrate()` method (whole tree opted in).
    assert!(
        lib_rs.matches("pub fn hydrate(").count() == 2,
        "expected exactly two hydrate() methods (App + Card):\n{lib_rs}"
    );
    // Callsites with a dot: the entry wrapper `root.hydrate(__el)` + at least one
    // child `.hydrate(...)` in App's adopt walk. The build path uses
    // `.mount_into(` instead — the adopt path replaces it with `.hydrate(`.
    assert!(
        lib_rs.matches(".hydrate(").count() >= 2,
        "expected the entry `root.hydrate(...)` + a child `.hydrate(...)` call:\n{lib_rs}"
    );

    // App.hydrate() must ADOPT the child wrapper, never create it: the naive
    // build path creates `<div class="__fitz-child-Card">`; the adopt path does
    // not. Scope the check to App's hydrate() body.
    let hstart = lib_rs.find("pub fn hydrate(").expect("hydrate present");
    let hbody = &lib_rs[hstart..];
    let hend = hbody.find("\n    }\n").unwrap_or(hbody.len());
    let hbody = &hbody[..hend];
    assert!(
        !hbody.contains("create_element(\"div\")"),
        "App.hydrate() must adopt the child wrapper, not create it:\n{hbody}"
    );

    // The crate needs serde_json for the state restore.
    let cargo = generate_cargo_toml(&expanded);
    assert!(
        cargo.contains("serde_json = \"1\""),
        "hydratable crate declares serde_json:\n{cargo}"
    );
}

#[test]
#[ignore]
fn build_hydrate_composition_wasm() {
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

    let wasm_path = crate_dir.join("pkg").join("hydrate_composition_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- Phase 11.12 slice 4 hydrate-composition bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
