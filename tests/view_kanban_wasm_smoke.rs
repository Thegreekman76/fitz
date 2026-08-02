//! Phase 11.7 R3.5c — the full kanban as a client-WASM SPA.
//!
//! The headline of Phase 11.7: `examples/view/kanban/App.fitzv` (the
//! collaborative-kanban Board, previously SSR-only in fitz-liveviews)
//! compiled to a standalone WASM single-page app. It converges every
//! R3.5 slice — nominal types (R3), imported helper fns (R3.5a.2),
//! `.map`/`.filter` closures (R3.5a.1), `{#for}` over an imported-fn
//! result, click payload (R3.5b.1), and form-submit payload (R3.5b.2).
//!
//! - [`regenerate_kanban_lib_rs`] (always runs) — loads the imported
//!   nominals + fns, regenerates `wasm-crate/src/lib.rs` + `Cargo.toml`
//!   (with the conditional `HtmlInputElement` feature), and asserts the
//!   emitted Rust wires the whole board.
//! - [`build_kanban_wasm`] (`#[ignore]`) — regeneration +
//!   `wasm-pack build --release --target web` (needs the wasm toolchain).

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn example_dir() -> PathBuf {
    repo_root().join("examples").join("view").join("kanban")
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
    let nominals = fitz::view::load_imported_nominals(&expanded.imports, &example_dir())
        .unwrap_or_else(|e| panic!("view::load_imported_nominals failed:\n{}", e));
    let fns = fitz::view::load_imported_fns(&expanded.imports, &example_dir())
        .unwrap_or_else(|e| panic!("view::load_imported_fns failed:\n{}", e));
    fitz::view::compose_lib_rs_with_imports(expanded, &nominals, &fns, "#app", Some("App.fitzv"))
        .unwrap_or_else(|e| panic!("view::compose_lib_rs_with_imports failed:\n{}", e))
}

fn generate_cargo_toml(expanded: &fitz::view::ExpandedViewFile) -> String {
    let extra = fitz::view::wasm_extra_web_sys_features(expanded);
    fitz::view::compose_cargo_toml_with_features(
        "kanban",
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
fn regenerate_kanban_lib_rs() {
    let expanded = expanded_from_fitzv();
    let lib_rs = generate_lib_rs(&expanded);

    let checks: &[(&str, &str)] = &[
        // Nominal + imported fns (R3 / R3.5a.2).
        ("pub struct Card {", "the Card nominal struct is emitted"),
        (
            "fn cards_in(all: Vec<Card>, col: String) -> Vec<Card> {",
            "cards_in is transpiled",
        ),
        (
            "fn move_one(target_id: String, direction: String, c: Card) -> Card {",
            "move_one is transpiled",
        ),
        (
            "fn next_column(current: String) -> String {",
            "the internal next_column helper is transpiled (reachable via move_one)",
        ),
        // Event bodies (R3.5a + R3.5b + R3.5c StrInterp).
        (
            "let id_str = format!(\"{}\", (*self.next_id.borrow()));",
            "the string-interpolated id lowers to a format!",
        ),
        (
            ".map(|c| move_one(target_id.clone(), \"right\".to_string(), c.clone()))",
            "move_right maps the imported move_one with cloned captured args",
        ),
        (
            ".filter(|__it| { let c = __it.clone(); keep_if_not(target_id.clone(), c.clone()) })",
            "delete_card filters via the imported keep_if_not",
        ),
        (
            "self.cards.borrow_mut().push(make_card(id_str.clone(), title.clone(), author.clone()));",
            "create_card pushes a make_card built from the payload",
        ),
        // Payload plumbing (R3.5b).
        (
            "fn create_card(self: &Rc<Self>, payload: &std::collections::HashMap<String, String>) {",
            "create_card takes the payload param",
        ),
        (
            "if payload.contains_key(&(\"title\".to_string())) {",
            "payload.has guards the create handler",
        ),
        (
            "let title = payload.get(&(\"title\".to_string())).cloned().unwrap_or_default();",
            "the form field is read from the payload",
        ),
        // Form submit + click wiring (R3.5b.1/.2).
        (
            "add_event_listener_with_callback(\"submit\"",
            "the form wires a submit listener",
        ),
        (
            "__inp.set_value(\"\");",
            "the data-flv-clear inputs reset after submit",
        ),
        (
            "__payload.insert(\"card_id\".to_string(), __evt_el.get_attribute(\"data-flv-value-card_id\").unwrap_or_default());",
            "the move/delete buttons read card_id from the value attr",
        ),
        (
            ".set_attribute(\"data-flv-value-card_id\", &format!(\"{}\", c.id.clone())).unwrap();",
            "the interpolated value attr carries the card id",
        ),
        // {#for} over an imported-fn result (R3.5a.2).
        (
            "cards_in((*self.cards.borrow()).clone(), \"todo\".to_string())",
            "each column iterates cards_in over the state list",
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
    assert!(
        cargo.contains("\"HtmlInputElement\","),
        "the kanban crate needs HtmlInputElement (form submit):\n{cargo}"
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
fn build_kanban_wasm() {
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

    let wasm_path = crate_dir.join("pkg").join("kanban_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- Phase 11.7 R3.5c kanban bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
