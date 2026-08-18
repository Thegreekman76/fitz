//! v0.48.0 — SSR -> client hydration of DYNAMIC keyed composition
//! (`<Child key=... />` inside `{#for}`).
//!
//! `examples/view/hydrate-keyed-composition/App.fitzv`: a root `App` marked
//! `hydrate` composes a `Column` INSIDE a `{#for}` with a `key`. On boot the
//! client-WASM adopts the server-painted DOM — the `<!--fr-->`/`<!--/fr-->`
//! region anchors plus one `<div class="__fitz-child-Column">` wrapper per list
//! item — reconciling each through the keyed instance cache (`__child_map_0`)
//! instead of leaving the loop dead until the first re-render.
//!
//! - [`regenerate_hydrate_keyed_composition_lib_rs`] (always runs) — regenerates
//!   `lib.rs` + `Cargo.toml` from the `.fitzv` and asserts the structural
//!   invariants of the dynamic adopt: the `{#for}` region descends (not a bare
//!   skip), consumes the `fr`/`/fr` anchors inside `hydrate()`, adopts one child
//!   wrapper per item with `__flv_next_element` (never `create_element`/
//!   `mount_into`), reconciles through `__child_map_0` / `__seen_0` / `retain`,
//!   and calls `Column.hydrate(...)`.
//! - [`build_hydrate_keyed_composition_wasm`] (`#[ignore]`) — real `wasm-pack build`.

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
        .join("hydrate-keyed-composition")
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
        "hydrate_keyed_composition",
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

/// Slice App's `hydrate()` body out of the emitted `lib.rs` so needle checks
/// don't accidentally match Column's method or the build-walk `render()`.
fn app_hydrate_body(lib_rs: &str) -> String {
    let hstart = lib_rs
        .find("pub fn hydrate(")
        .expect("App emits a hydrate() method");
    let hbody = &lib_rs[hstart..];
    let hend = hbody.find("\n    }\n").unwrap_or(hbody.len());
    hbody[..hend].to_string()
}

#[test]
fn regenerate_hydrate_keyed_composition_lib_rs() {
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

    // Both App and Column emit a hydrate() method (whole tree opted in).
    assert_eq!(
        lib_rs.matches("pub fn hydrate(").count(),
        2,
        "expected exactly two hydrate() methods (App + Column):\n{lib_rs}"
    );

    let hbody = app_hydrate_body(&lib_rs);

    // The `{#for}` region descends in adopt mode: it consumes BOTH anchors
    // INSIDE App.hydrate() (a bare static-region skip would too, so the anchors
    // alone aren't the discriminator — the keyed needles below are).
    assert!(
        hbody.contains("__flv_next_comment(&mut")
            && hbody.contains("\"fr\"")
            && hbody.contains("\"/fr\""),
        "App.hydrate() must consume the region's fr//fr anchors:\n{hbody}"
    );

    // Per-item wrapper adoption: the dynamic child wrapper is ACQUIRED from the
    // cursor, never created; the child is hydrated, not mounted.
    assert!(
        hbody.contains("__flv_next_element(&mut"),
        "App.hydrate() adopts each child wrapper via __flv_next_element:\n{hbody}"
    );
    assert!(
        !hbody.contains("create_element("),
        "App.hydrate() must adopt the child wrappers, never create them:\n{hbody}"
    );
    assert!(
        !hbody.contains("mount_into("),
        "App.hydrate() hydrates children (not mount_into):\n{hbody}"
    );

    // Keyed reconciliation machinery reused inside the adopt walk.
    let keyed_needles: &[(&str, &str)] = &[
        (
            "self.__child_map_0.borrow_mut()",
            "the adopt walk reconciles through the keyed instance cache",
        ),
        (
            "__seen_0.insert(__key.clone());",
            "each adopted item records its key in the per-render __seen set",
        ),
        (
            ".retain(|__k, _| __seen_0.contains(__k));",
            "the post-loop reconciliation sweep runs in the adopt walk too",
        ),
        (
            ".hydrate(",
            "the adopted child is hydrated over its server wrapper",
        ),
    ];
    for (needle, why) in keyed_needles {
        assert!(
            hbody.contains(needle),
            "structural invariant broken — {}\nExpected substring in App.hydrate(): `{}`\n\nApp.hydrate() body:\n{}",
            why,
            needle,
            hbody
        );
    }

    // Index alignment: the SAME keyed cache (`__child_map_0`) is referenced by
    // both the build walk (render) and the adopt walk (hydrate), proving the
    // dynamic-site counter advances identically in both.
    assert!(
        lib_rs.matches("self.__child_map_0.borrow_mut()").count() >= 2,
        "`__child_map_0` must appear in both render() and hydrate():\n{lib_rs}"
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
fn build_hydrate_keyed_composition_wasm() {
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

    let wasm_path = crate_dir
        .join("pkg")
        .join("hydrate_keyed_composition_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- v0.48.0 hydrate-keyed-composition bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
