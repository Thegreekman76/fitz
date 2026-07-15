//! Phase 11.5.e — multi-component showcase smoke.
//!
//! Parallel to `tests/view_counter_wasm_smoke.rs` but for
//! `examples/view/showcase/Dashboard.fitzv`, the largest fixture
//! the Phase 11.5.d composition subset permits (parent `Board`
//! composes three `<MetricCard title="X" value="N" trend="Y" />`
//! children).
//!
//! Two tests:
//!
//! - [`regenerate_showcase_lib_rs`] (always runs). Runs the full
//!   view pipeline (parse → expand → check → compose_lib_rs) on
//!   `Dashboard.fitzv`, writes the result to
//!   `examples/view/showcase/wasm-crate/src/lib.rs`. Keeps the
//!   committed baseline in sync with the emitter, so anyone
//!   cloning the repo can `wasm-pack build` the showcase without
//!   running the smoke first.
//!
//! - [`build_showcase_wasm`] (`#[ignore]`). Same regeneration +
//!   shells out to `wasm-pack build --release --target web`.
//!   Requires the wasm toolchain — opt-in via `-- --ignored`.
//!   Bundle-size measurement is NOT enforced here (the 40 KB
//!   gate is per-component-count, and the showcase adds a
//!   second component + its DOM tree — a re-baselining pass
//!   belongs to 11.5.e docs, not to a hard test assertion).
//!
//! Neither test opens a browser — the browser smoke is manual
//! (see `examples/view/showcase/README.md` for the recipe).

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn showcase_dir() -> PathBuf {
    repo_root().join("examples").join("view").join("showcase")
}

fn fitzv_source_path() -> PathBuf {
    showcase_dir().join("Dashboard.fitzv")
}

fn lib_rs_path() -> PathBuf {
    showcase_dir().join("wasm-crate").join("src").join("lib.rs")
}

fn cargo_toml_path() -> PathBuf {
    showcase_dir().join("wasm-crate").join("Cargo.toml")
}

/// Load `Dashboard.fitzv`, run parse → expand → check →
/// `fitz::view::compose_lib_rs` — the exact same helper
/// `fitz build --target wasm-client` uses. The composed source
/// is bit-for-bit what the CLI would emit for a bin named
/// `"showcase"` mounting `"#app"`.
fn generate_lib_rs_from_fitzv() -> String {
    let src_path = fitzv_source_path();
    let src = fs::read_to_string(&src_path).unwrap_or_else(|e| {
        panic!(
            "failed to read `{}`: {}\n(cwd = `{}`)",
            src_path.display(),
            e,
            std::env::current_dir().unwrap().display()
        )
    });

    let raw = fitz::view::parse(&src)
        .unwrap_or_else(|e| panic!("view::parse failed on Dashboard.fitzv:\n{}", e));

    let expanded = fitz::view::expand(&raw)
        .unwrap_or_else(|e| panic!("view::expand failed on Dashboard.fitzv:\n{}", e));

    let check_errs = fitz::view::check(&expanded);
    if !check_errs.is_empty() {
        let joined: Vec<String> = check_errs.iter().map(|e| e.to_string()).collect();
        panic!(
            "view::check reported {} error(s) on Dashboard.fitzv:\n{}",
            joined.len(),
            joined.join("\n")
        );
    }

    fitz::view::compose_lib_rs(&expanded, "#app", Some("Dashboard.fitzv"))
        .unwrap_or_else(|e| panic!("view::compose_lib_rs failed on Dashboard.fitzv:\n{}", e))
}

/// Same as [`fitz::view::compose_cargo_toml`] but wired to the
/// showcase's canonical package name (`showcase`). Kept out of
/// [`generate_lib_rs_from_fitzv`] so the regenerator can write
/// both files independently — updating `Cargo.toml` only when
/// its content changes matches how the counter's baseline is
/// committed today.
fn generate_cargo_toml() -> String {
    fitz::view::compose_cargo_toml("showcase")
}

/// Only overwrite the destination when the new content differs
/// from the existing file. Keeps `git status` clean when nothing
/// changed. Idempotent + tolerant of read failure (falls back to
/// write).
fn write_if_changed(path: &Path, new_content: &str) {
    let existing = fs::read_to_string(path).ok();
    match existing {
        Some(cur) if cur == new_content => {
            // no-op
        }
        _ => {
            let mut f = fs::File::create(path)
                .unwrap_or_else(|e| panic!("failed to open `{}` for write: {}", path.display(), e));
            f.write_all(new_content.as_bytes())
                .unwrap_or_else(|e| panic!("failed to write `{}`: {}", path.display(), e));
        }
    }
}

// ---------------------------------------------------------------
// Test 1: regenerate lib.rs + Cargo.toml (always runs).
// ---------------------------------------------------------------

/// Runs the pipeline and writes `wasm-crate/src/lib.rs` +
/// `wasm-crate/Cargo.toml`. Keeps the committed baselines in
/// sync with the emitter output.
///
/// Also validates structural invariants of the emitted Rust that
/// prove the 11.5.d composition wiring survived: both component
/// structs present, root instantiated as `Board`, three
/// `<MetricCard />` wrappers created (class `__fitz-child-MetricCard`),
/// coerced Int + Str prop assignments, `mount_into` calls on the
/// wrappers. These catch a regressing emitter refactor even if
/// lib.rs happens to still compile.
#[test]
fn regenerate_showcase_lib_rs() {
    let lib_rs = generate_lib_rs_from_fitzv();

    // Structural sanity — cheap grep-based invariants.
    let checks: &[(&str, &str)] = &[
        ("pub struct Board {", "Board struct present"),
        ("pub struct MetricCard {", "MetricCard struct present"),
        (
            "let root = Board::new();",
            "root instantiation is Board (first-declared)",
        ),
        ("root.mount(\"#app\")", "mount selector is `#app`"),
        (
            "pub fn mount_into(self: &Rc<Self>, root: HtmlElement) -> Result<(), JsValue> {",
            "mount_into API present on both components",
        ),
        // Three <MetricCard /> composition sites in the Board's
        // template. Each site emits a wrapper element + child
        // instantiation + prop assignments + `mount_into`.
        // Assert count so a regressing walker that drops nodes
        // fires here.
        // (Use string-count assertions via `matches` count.)
    ];
    for (needle, why) in checks {
        assert!(
            lib_rs.contains(needle),
            "structural invariant broken — {}\nExpected substring: `{}`\n\nFull emitted lib.rs (truncated):\n{}",
            why,
            needle,
            // Truncate to first 4000 chars in error output so
            // the failure message stays readable.
            &lib_rs.chars().take(4000).collect::<String>()
        );
    }

    let wrapper_count = lib_rs
        .matches(r#"set_attribute("class", "__fitz-child-MetricCard")"#)
        .count();
    assert_eq!(
        wrapper_count, 3,
        "expected 3 <MetricCard /> composition sites, saw {wrapper_count}"
    );
    let mount_into_calls = lib_rs.matches(".mount_into(").count();
    // 3 `mount_into` calls come from the composition sites; a
    // 4th appears inside `Board::mount(selector)` and a 5th
    // inside `MetricCard::mount(selector)` (they delegate to
    // `mount_into(root)` per the 11.5.d refactor). Total: 5.
    assert_eq!(
        mount_into_calls, 5,
        "expected 5 mount_into calls (3 composition sites + 2 delegating mount(selector) wrappers), saw {mount_into_calls}"
    );

    let dst = lib_rs_path();
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| {
            panic!("failed to create parent dir `{}`: {}", parent.display(), e)
        });
    }
    write_if_changed(&dst, &lib_rs);

    // Also write the Cargo.toml so `wasm-pack build` finds a
    // ready scaffold.
    let cargo_toml = generate_cargo_toml();
    write_if_changed(&cargo_toml_path(), &cargo_toml);
}

// ---------------------------------------------------------------
// Test 2: build the wasm bundle (opt-in, no size gate).
// ---------------------------------------------------------------

/// Shells out to `wasm-pack build --release --target web` inside
/// `examples/view/showcase/wasm-crate/`. Requires `wasm-pack` +
/// `wasm32-unknown-unknown` target installed. Marked `#[ignore]`
/// so the default `cargo test` run doesn't force this on
/// machines without the toolchain.
///
/// No hard bundle-size assertion here. The 40 KB gate documented
/// in §9.l was measured on the single-component counter;
/// multi-component fixtures add per-component struct + impls +
/// style helper LoC, and re-baselining that gate belongs in the
/// 11.5.e closure docs (see §9.t), not in a flaky size assertion.
#[test]
#[ignore]
fn build_showcase_wasm() {
    let lib_rs = generate_lib_rs_from_fitzv();
    write_if_changed(&lib_rs_path(), &lib_rs);
    write_if_changed(&cargo_toml_path(), &generate_cargo_toml());

    let crate_dir = showcase_dir().join("wasm-crate");
    let status = std::process::Command::new("wasm-pack")
        .args(["build", "--release", "--target", "web"])
        .current_dir(&crate_dir)
        .status()
        .unwrap_or_else(|e| {
            panic!(
                "failed to invoke `wasm-pack`: {}\n\
                 (do you have wasm-pack installed? `cargo install wasm-pack` \
                 or grab the installer from https://rustwasm.github.io/wasm-pack/)",
                e
            )
        });
    if !status.success() {
        panic!(
            "`wasm-pack build --release --target web` exited with {}. \
             Check the output above.",
            status
        );
    }

    let wasm_path = crate_dir.join("pkg").join("showcase_bg.wasm");
    let raw_bytes = fs::metadata(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to stat `{}`: {}", wasm_path.display(), e))
        .len();
    println!("--- Phase 11.5.e showcase bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
    println!(
        "  (no hard gate — see §9.t of `docs/fase-11-plan.md` for the \
         re-baselining rationale)"
    );
}
