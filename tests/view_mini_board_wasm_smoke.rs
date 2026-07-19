//! Phase 11.7 R3.5a.2 — imported classic helper fns on the client-WASM
//! target.
//!
//! Parallel to `tests/view_nominal_list_wasm_smoke.rs` but for
//! `examples/view/mini-board/App.fitzv`, which imports helpers from the
//! sibling classic module `board_helpers.fitz` and calls them from the
//! template (`{#for c in cards_in(cards, "todo")}`) and event bodies
//! (`cards = cards.map(fn(c) => advance(c))`,
//! `cards.push(make_card(next_id, "Task"))`). The helper `fn`s are
//! transpiled into the WASM crate.
//!
//! - [`regenerate_mini_board_lib_rs`] (always runs) — loads the imported
//!   nominals + fns, regenerates `wasm-crate/src/lib.rs` + `Cargo.toml`,
//!   and asserts the emitted Rust carries the transpiled helper fns, the
//!   free-fn calls (with argument cloning), the `{#for}` over an
//!   imported-fn result, and the `.map`-with-imported-call event body.
//! - [`build_mini_board_wasm`] (`#[ignore]`) — regeneration +
//!   `wasm-pack build --release --target web` (needs the wasm toolchain).

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn example_dir() -> PathBuf {
    repo_root().join("examples").join("view").join("mini-board")
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

    // Phase 11.7 R3 + R3.5a.2 — load the sibling `card.fitz`'s `type Card`
    // (for the struct) AND `board_helpers.fitz`'s `fn`s (for the
    // transpiled helpers). This is the same pair of loads `fitz build
    // --target wasm-client` runs.
    let nominals = fitz::view::load_imported_nominals(&expanded.imports, &example_dir())
        .unwrap_or_else(|e| panic!("view::load_imported_nominals failed:\n{}", e));
    let fns = fitz::view::load_imported_fns(&expanded.imports, &example_dir())
        .unwrap_or_else(|e| panic!("view::load_imported_fns failed:\n{}", e));

    fitz::view::compose_lib_rs_with_imports(&expanded, &nominals, &fns, "#app", Some("App.fitzv"))
        .unwrap_or_else(|e| panic!("view::compose_lib_rs_with_imports failed:\n{}", e))
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
fn regenerate_mini_board_lib_rs() {
    let lib_rs = generate_lib_rs_from_fitzv();

    let checks: &[(&str, &str)] = &[
        (
            "#[derive(Clone)]\npub struct Card {",
            "the imported nominal `Card` is emitted as a Rust struct",
        ),
        (
            "fn cards_in(all: Vec<Card>, col: String) -> Vec<Card> {",
            "the imported `cards_in` helper is transpiled with mapped types",
        ),
        (
            ".into_iter().filter(|__it| { let c = __it.clone(); (c.column.clone() == col) })",
            "the helper's `.filter` closure lowers with a Str comparison",
        ),
        (
            "fn next_column(current: String) -> String {",
            "the internal `next_column` helper is transpiled even though it is not imported",
        ),
        (
            "if (current == \"todo\".to_string()) {",
            "the helper's statement-if condition lowers with a Str comparison",
        ),
        (
            "return \"in_progress\".to_string();",
            "the helper's return lowers a Str literal",
        ),
        (
            "fn advance(c: Card) -> Card {",
            "the `advance` helper is transpiled",
        ),
        (
            "let moved = next_column(c.column.clone());",
            "advance calls next_column with a cloned field argument",
        ),
        (
            "return Card { id: c.id.clone(), title: c.title.clone(), column: moved };",
            "advance returns a struct literal from field access + a local",
        ),
        (
            "self.cards.borrow_mut().push(make_card((*self.next_id.borrow()).clone(), \"Task\".to_string()));",
            "the add event pushes a make_card free-fn call",
        ),
        (
            ".clone().into_iter().map(|c| advance(c.clone())).collect::<Vec<_>>()",
            "advance_all maps the imported advance over the cards (arg cloned)",
        ),
        (
            "let __for",
            "the template {#for} snapshots the imported-fn result into a local",
        ),
        (
            "cards_in((*self.cards.borrow()).clone(), \"todo\".to_string())",
            "the {#for} / .len() call the imported cards_in with the state list cloned",
        ),
        (
            ".into_iter() {",
            "the {#for} over the imported-fn result iterates with .into_iter()",
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

    let dst = lib_rs_path();
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    write_if_changed(&dst, &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml("mini-board"),
    );
}

#[test]
#[ignore]
fn build_mini_board_wasm() {
    let lib_rs = generate_lib_rs_from_fitzv();
    write_if_changed(&lib_rs_path(), &lib_rs);
    write_if_changed(
        &cargo_toml_path(),
        &fitz::view::compose_cargo_toml("mini-board"),
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

    let wasm_path = crate_dir.join("pkg").join("mini_board_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- Phase 11.7 R3.5a.2 mini-board bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
