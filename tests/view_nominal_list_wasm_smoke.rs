//! Phase 11.7 R3 — nominal types on the client-WASM target.
//!
//! Parallel to `tests/view_keyed_composition_wasm_smoke.rs` but for
//! `examples/view/nominal-list/App.fitzv`, which iterates a
//! `List<Card>` (an imported classic `type`) with `{#for c in cards}`
//! and fans each nominal item's fields out into a keyed
//! `<CardRow key="{c.id}" title="{c.title}" />`. The `Card` struct is
//! synthesised inline in the WASM crate from the sibling `card.fitz`.
//!
//! - [`regenerate_nominal_list_lib_rs`] (always runs) — loads the
//!   imported nominals, regenerates `wasm-crate/src/lib.rs` +
//!   `Cargo.toml`, and asserts the emitted Rust carries: the `Card`
//!   struct, a `Vec<Card>` state field, nominal construction + live
//!   `.push`, the `{#for}` snapshot over the nominal list, field-access
//!   key + props, and the keyed instance cache + reconciliation.
//! - [`build_nominal_list_wasm`] (`#[ignore]`) — regeneration +
//!   `wasm-pack build --release --target web` (needs the wasm
//!   toolchain).

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
        .join("nominal-list")
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

    // Phase 11.7 R3 — load the sibling `card.fitz`'s `type Card` so the
    // emitter can synthesise its Rust struct. This is the same helper
    // `fitz build --target wasm-client` runs (`main.rs`).
    let nominals = fitz::view::load_imported_nominals(&expanded.imports, &example_dir())
        .unwrap_or_else(|e| panic!("view::load_imported_nominals failed:\n{}", e));

    fitz::view::compose_lib_rs_with_nominals(&expanded, &nominals, "#app", Some("App.fitzv"))
        .unwrap_or_else(|e| panic!("view::compose_lib_rs_with_nominals failed:\n{}", e))
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
fn regenerate_nominal_list_lib_rs() {
    let lib_rs = generate_lib_rs_from_fitzv();

    let checks: &[(&str, &str)] = &[
        (
            "#[derive(Clone)]\npub struct Card {",
            "the imported nominal `Card` is emitted as a Rust struct",
        ),
        ("    id: i64,", "Card.id maps to i64"),
        ("    title: String,", "Card.title maps to String"),
        ("    done: bool,", "Card.done maps to bool"),
        ("pub struct App {", "App struct present"),
        (
            "cards: RefCell<Vec<Card>>,",
            "the List<Card> state field maps to Vec<Card>",
        ),
        (
            "self.cards.borrow_mut().push(Card { id: (*self.next_id.borrow()), title: \"Task\".to_string(), done: false });",
            "nominal construction + live list mutation in the event body",
        ),
        (
            "(*self.cards.borrow()).clone();",
            "the {#for} snapshots the nominal Vec",
        ),
        (
            "for c in __for",
            "the loop var binds the nominal by value from the snapshot",
        ),
        (
            ".iter().cloned() {",
            "the nominal loop iterates the snapshot by value",
        ),
        (
            "__child_map_0: RefCell<std::collections::HashMap<String, Rc<CardRow>>>,",
            "the dynamic child site gets a keyed instance cache",
        ),
        (
            "let __key = format!(\"{}\", c.id.clone());",
            "the key lowers from field access on the nominal loop var",
        ),
        (
            "__map.entry(__key.clone()).or_insert_with(|| CardRow::new()).clone()",
            "the keyed child is get-or-created from the map",
        ),
        (
            ".title.borrow_mut() = c.title.clone();",
            "a primitive prop is fanned out from a nominal field",
        ),
        (
            "self.__child_map_0.borrow_mut().retain(|__k, _| __seen_0.contains(__k));",
            "reconciliation evicts vanished keys after the loop",
        ),
    ];
    for (needle, why) in checks {
        assert!(
            lib_rs.contains(needle),
            "structural invariant broken — {}\nExpected substring: `{}`\n\nFull emitted lib.rs (truncated):\n{}",
            why,
            needle,
            &lib_rs.chars().take(6000).collect::<String>()
        );
    }

    let dst = lib_rs_path();
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    write_if_changed(&dst, &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml("nominal-list"),
    );
}

#[test]
#[ignore]
fn build_nominal_list_wasm() {
    let lib_rs = generate_lib_rs_from_fitzv();
    write_if_changed(&lib_rs_path(), &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml("nominal-list"),
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

    let wasm_path = crate_dir.join("pkg").join("nominal_list_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- Phase 11.7 R3 nominal-list bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
