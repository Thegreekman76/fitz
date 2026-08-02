//! Phase 11.11.c — `@rpc` server functions on the client-WASM target.
//!
//! Mirrors `tests/view_nominal_list_wasm_smoke.rs` but for
//! `examples/view/rpc/App.fitzv`, which imports two `@rpc async fn`s
//! from the sibling `api.fitz` and calls them with `.await?` inside
//! event handlers. The WASM emitter turns each `@rpc` fn into an async
//! `fetch` stub (NOT a transpiled body) and wraps the awaiting handlers
//! in `spawn_local`.
//!
//! - [`regenerate_rpc_lib_rs`] (always runs) — loads the imported
//!   nominals + fns, regenerates `wasm-crate/src/lib.rs` + `Cargo.toml`,
//!   and asserts the emitted Rust carries: the shared `__fitz_fetch_post`
//!   helper, the async stubs POSTing to `/__rpc/*`, the `serde` derives
//!   on the shared `User` struct, and the `spawn_local` + async-worker
//!   wrapping of the awaiting handlers.
//! - [`build_rpc_wasm`] (`#[ignore]`) — regeneration +
//!   `wasm-pack build --release --target web` (needs the wasm
//!   toolchain).

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn example_dir() -> PathBuf {
    repo_root().join("examples").join("view").join("rpc")
}

fn lib_rs_path() -> PathBuf {
    example_dir().join("wasm-crate").join("src").join("lib.rs")
}

fn cargo_toml_path() -> PathBuf {
    example_dir().join("wasm-crate").join("Cargo.toml")
}

fn generate_lib_rs_and_cargo() -> (String, String) {
    let src_path = example_dir().join("App.fitzv");
    let src = fs::read_to_string(&src_path)
        .unwrap_or_else(|e| panic!("failed to read `{}`: {}", src_path.display(), e));

    let raw = fitz::view::parse(&src)
        .unwrap_or_else(|e| panic!("view::parse failed on App.fitzv:\n{}", e));
    let expanded = fitz::view::expand(&raw)
        .unwrap_or_else(|e| panic!("view::expand failed on App.fitzv:\n{}", e));

    // Load the sibling `api.fitz`'s `type User` + `@rpc` fns — the same
    // helpers `fitz build --target wasm-client` runs (`main.rs`). The
    // fns registry flags each `@rpc` fn so the emitter produces a fetch
    // stub instead of transpiling its (server-side) body.
    let nominals = fitz::view::load_imported_nominals(&expanded.imports, &example_dir())
        .unwrap_or_else(|e| panic!("view::load_imported_nominals failed:\n{}", e));
    let fns = fitz::view::load_imported_fns(&expanded.imports, &example_dir())
        .unwrap_or_else(|e| panic!("view::load_imported_fns failed:\n{}", e));

    let check_errs = fitz::view::check(&expanded);
    if !check_errs.is_empty() {
        let joined: Vec<String> = check_errs.iter().map(|e| e.to_string()).collect();
        panic!(
            "view::check reported {} error(s) on App.fitzv:\n{}",
            joined.len(),
            joined.join("\n")
        );
    }

    let extra = fitz::view::wasm_extra_web_sys_features(&expanded);
    let cargo = fitz::view::compose_cargo_toml_with_features(
        "rpc-demo",
        &extra,
        fns.has_rpc(),
        fitz::view::file_uses_hydration(&expanded),
    );

    let lib_rs = fitz::view::compose_lib_rs_with_components(
        &expanded,
        &nominals,
        &fns,
        &fitz::view::ImportedComponentRegistry::new(),
        "#app",
        Some("App.fitzv"),
    )
    .unwrap_or_else(|e| panic!("view::compose_lib_rs_with_components failed:\n{}", e));
    (lib_rs, cargo)
}

fn write_if_changed(path: &Path, new_content: &str) {
    let existing = fs::read_to_string(path).ok();
    match existing {
        Some(cur) if cur == new_content => {}
        _ => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            let mut f = fs::File::create(path)
                .unwrap_or_else(|e| panic!("failed to open `{}` for write: {}", path.display(), e));
            f.write_all(new_content.as_bytes())
                .unwrap_or_else(|e| panic!("failed to write `{}`: {}", path.display(), e));
        }
    }
}

#[test]
fn regenerate_rpc_lib_rs() {
    let (lib_rs, cargo) = generate_lib_rs_and_cargo();

    let checks: &[(&str, &str)] = &[
        (
            "async fn __fitz_fetch_post(url: &str, body: &str) -> Result<(u16, String), String> {",
            "the shared fetch runtime is emitted",
        ),
        (
            "#[derive(Clone, serde::Serialize, serde::Deserialize)]\npub struct User {",
            "the wire-crossing nominal gets serde derives",
        ),
        (
            "async fn greet(name: String) -> Result<String, String> {",
            "the greet stub is an async fetch fn (not a transpiled body)",
        ),
        (
            "__fitz_fetch_post(\"/__rpc/greet\", &__body).await?",
            "greet POSTs to /__rpc/greet",
        ),
        (
            "async fn get_user(id: i64) -> Result<User, String> {",
            "the get_user stub returns the nominal",
        ),
        (
            "serde_json::from_str::<User>(&__text)",
            "the 200 branch deserializes the nominal reply",
        ),
        (
            "wasm_bindgen_futures::spawn_local(async move {",
            "an awaiting handler spawns its async worker",
        ),
        (
            "async fn __load_greeting_async(self: Rc<Self>) -> Result<(), String> {",
            "the async worker takes an owned Rc<Self>",
        ),
        (
            "async fn __load_user_async(self: Rc<Self>) -> Result<(), String> {",
            "the second awaiting handler also splits into a worker",
        ),
    ];
    for (needle, why) in checks {
        assert!(
            lib_rs.contains(needle),
            "structural invariant broken — {}\nExpected substring: `{}`\n\nFull emitted lib.rs (truncated):\n{}",
            why,
            needle,
            &lib_rs.chars().take(8000).collect::<String>()
        );
    }
    // Cargo.toml gains the rpc runtime deps.
    for dep in &["wasm-bindgen-futures", "js-sys", "serde", "serde_json"] {
        assert!(
            cargo.contains(dep),
            "Cargo.toml missing rpc dep `{dep}`:\n{cargo}"
        );
    }

    write_if_changed(&lib_rs_path(), &lib_rs);
    write_if_changed(&cargo_toml_path(), &cargo);
}

#[test]
#[ignore]
fn build_rpc_wasm() {
    let (lib_rs, cargo) = generate_lib_rs_and_cargo();
    write_if_changed(&lib_rs_path(), &lib_rs);
    write_if_changed(&cargo_toml_path(), &cargo);

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
