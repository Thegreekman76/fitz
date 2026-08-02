//! Phase 11.12 slice 2 — SSR → client hydration of `{#if}`/`{#for}` regions +
//! mixed static/interpolated text, for `examples/view/hydrate-regions/App.fitzv`.
//!
//! A keep-node component with regions is now HYDRATABLE: the generated
//! `hydrate()` adopts the server-painted DOM when the mount root already has
//! content — including the region anchors (`<!--fr-->` / `<!--/fr-->`, matched
//! by the `__flv_next_comment` cursor) and the mixed-text nodes — instead of
//! wiping + rebuilding.
//!
//! Phase 11.12 slice 3 also asserts that the component's `items` `List<Str>`
//! state field is restored from the `<script>` payload (composite state
//! restore), not left at the source default.
//!
//! - [`regenerate_hydrate_regions_lib_rs`] (always runs) — regenerates
//!   `wasm-crate/src/lib.rs` + `Cargo.toml` and asserts the emitted Rust
//!   carries the slice-2 hydration surface: the comment cursor helper, a
//!   `hydrate()` that adopts region anchors + mixed-text runs (never
//!   `create_*`), and the region patch methods retained for later state
//!   changes.
//! - [`build_hydrate_regions_wasm`] (`#[ignore]`) — regeneration +
//!   `wasm-pack build --release --target web` (needs the wasm toolchain).

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
        .join("hydrate-regions")
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
        "hydrate_regions",
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
fn regenerate_hydrate_regions_lib_rs() {
    let expanded = expanded_from_fitzv();
    let lib_rs = generate_lib_rs(&expanded);

    let checks: &[(&str, &str)] = &[
        (
            "fn __flv_next_comment(__cursor: &mut Option<web_sys::Node>, __data: &str) -> Option<web_sys::Node>",
            "the region-anchor comment cursor helper is emitted",
        ),
        (
            "pub fn hydrate(self: &Rc<Self>, root: HtmlElement) -> Result<(), JsValue>",
            "the hydrate() method is emitted",
        ),
        (
            "get_element_by_id(\"__flv_state_App\")",
            "hydrate reads the serialized state script by id",
        ),
        (
            "fn __patch_region_0(self: &Rc<Self>)",
            "the {#if} region patch method is retained for later state changes",
        ),
        (
            "fn __patch_region_1(self: &Rc<Self>)",
            "the {#for} region patch method is retained",
        ),
        (
            "return root.hydrate(__el);",
            "the entry wrapper branches to hydrate when the root has DOM",
        ),
        (
            "if let Some(__fv) = __v.get(\"items\") {",
            "slice 3 — the List<Str> state field restores from the payload",
        ),
        (
            "__fv.as_array().map(|__arr| __arr.iter().filter_map(|__le| __le.as_str().map(|__s| __s.to_string())).collect::<Vec<String>>())",
            "slice 3 — List<Str> restore deserializes the JSON array",
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

    // The hydrate() body must ADOPT, never build — no `create_*`, no region
    // mount — between the method header and its `__built = true` tail.
    let hstart = lib_rs.find("pub fn hydrate(").expect("hydrate present");
    let htail = lib_rs[hstart..]
        .find("*self.__built.borrow_mut() = true;")
        .expect("hydrate tail present");
    let hbody = &lib_rs[hstart..hstart + htail];
    assert!(
        !hbody.contains("create_element")
            && !hbody.contains("create_text_node")
            && !hbody.contains("create_comment"),
        "hydrate() must adopt, not create nodes:\n{hbody}"
    );
    assert!(
        !hbody.contains("__mount_region_0()") && !hbody.contains("__mount_region_1()"),
        "hydrate() must not (re)mount region content — it is server-painted:\n{hbody}"
    );
    // The hydrate body adopts both regions' anchors by tagged comment.
    assert!(
        hbody.contains("__flv_next_comment(&mut")
            && hbody.contains("\"fr\"")
            && hbody.contains("\"/fr\""),
        "hydrate() adopts region anchors by tagged comment:\n{hbody}"
    );
    assert!(
        hbody.contains("*self.__astart_0.borrow_mut()")
            && hbody.contains("*self.__aend_0.borrow_mut()")
            && hbody.contains("*self.__astart_1.borrow_mut()")
            && hbody.contains("*self.__aend_1.borrow_mut()"),
        "hydrate() stashes both regions into their handle fields:\n{hbody}"
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
fn build_hydrate_regions_wasm() {
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

    let wasm_path = crate_dir.join("pkg").join("hydrate_regions_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- Phase 11.12 slice 2 hydrate-regions bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
}
