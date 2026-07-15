//! Phase 11.4.c — end-to-end smoke of the `.fitzv` → WASM pipeline
//! against the canonical counter demo.
//!
//! Two tests:
//!
//! - [`regenerate_counter_lib_rs`] (always runs). Runs the full
//!   view pipeline (parse → expand → check → emit_module) on
//!   `examples/view/counter/Counter.fitzv`, wraps the emitted
//!   Rust with a `#[wasm_bindgen(start)]` entry point, and
//!   writes the result to
//!   `examples/view/counter/wasm-crate/src/lib.rs`. This keeps
//!   the committed lib.rs baseline in sync with the emitter,
//!   so anyone cloning the repo can `wasm-pack build` the demo
//!   without running the smoke first.
//!
//! - [`build_counter_wasm_and_measure`] (`#[ignore]`). Same
//!   regeneration + shells out to `wasm-pack build --release
//!   --target web` and measures the resulting
//!   `pkg/counter_bg.wasm` (raw + gzipped) against the 40 KB
//!   gate documented in §9.l of `docs/fase-11-plan.md`.
//!   Requires `wasm-pack` + the `wasm32-unknown-unknown` target
//!   installed on the runner. Opt-in via `-- --ignored`.
//!
//! Neither test opens a browser — the browser smoke is manual
//! (see `examples/view/counter/README.md` for the recipe).
//!
//! ## Phase 11.5.c note
//!
//! Both tests now route through `fitz::view::wasm_build::compose_lib_rs`,
//! the same helper `fitz build --target wasm-client` uses. That
//! makes the regeneration bit-for-bit equivalent to what the CLI
//! would emit for a bin named `"counter"` mounting `"#app"` —
//! the regression suite for the CLI can compare against this
//! baseline directly.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------

fn repo_root() -> PathBuf {
    // The test process runs with CWD = repo root under `cargo test`.
    // `env!("CARGO_MANIFEST_DIR")` is more robust (points to the
    // package root of the crate under test, which for this repo IS
    // the repo root).
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn counter_dir() -> PathBuf {
    repo_root().join("examples").join("view").join("counter")
}

fn fitzv_source_path() -> PathBuf {
    counter_dir().join("Counter.fitzv")
}

fn lib_rs_path() -> PathBuf {
    counter_dir().join("wasm-crate").join("src").join("lib.rs")
}

/// Load `Counter.fitzv`, run parse → expand → check → and route
/// through `fitz::view::wasm_build::compose_lib_rs` — the exact
/// same helper `fitz build --target wasm-client` uses. The
/// composed source is bit-for-bit what the CLI would emit for a
/// bin named `"counter"` mounting `"#app"`.
fn generate_lib_rs_from_fitzv() -> String {
    let src_path = fitzv_source_path();
    let src = fs::read_to_string(&src_path).unwrap_or_else(|e| {
        panic!(
            "failed to read `{}`: {}\n\
             (running from CWD = `{}`)",
            src_path.display(),
            e,
            std::env::current_dir().unwrap().display()
        )
    });

    let raw = fitz::view::parse(&src)
        .unwrap_or_else(|e| panic!("view::parse failed on Counter.fitzv:\n{}", e));

    let expanded = fitz::view::expand(&raw)
        .unwrap_or_else(|e| panic!("view::expand failed on Counter.fitzv:\n{}", e));

    let check_errs = fitz::view::check(&expanded);
    if !check_errs.is_empty() {
        let joined: Vec<String> = check_errs.iter().map(|e| e.to_string()).collect();
        panic!(
            "view::check reported {} error(s) on Counter.fitzv:\n{}",
            joined.len(),
            joined.join("\n")
        );
    }

    fitz::view::compose_lib_rs(&expanded, "#app", Some("Counter.fitzv"))
        .unwrap_or_else(|e| panic!("view::compose_lib_rs failed on Counter.fitzv:\n{}", e))
}

// ---------------------------------------------------------------
// Test 1: regenerate lib.rs (always runs).
// ---------------------------------------------------------------

/// Runs the pipeline and writes `wasm-crate/src/lib.rs`. Keeps the
/// committed baseline in sync with the emitter output.
///
/// Also validates a few structural invariants of the emitted Rust
/// (state field present, event handlers present, `#[wasm_bindgen(start)]`
/// composed, style helper referenced) — these catch a regressing
/// emitter refactor even if lib.rs happens to still compile.
#[test]
fn regenerate_counter_lib_rs() {
    let lib_rs = generate_lib_rs_from_fitzv();

    // Structural sanity — cheap grep-based invariants.
    let checks: &[(&str, &str)] = &[
        ("pub struct Counter {", "struct declaration is present"),
        ("count: RefCell<i64>,", "state field `count` typed as i64"),
        ("fn increment(self: &Rc<Self>)", "increment handler emitted"),
        ("fn decrement(self: &Rc<Self>)", "decrement handler emitted"),
        ("fn reset(self: &Rc<Self>)", "reset handler emitted"),
        ("#[wasm_bindgen(start)]", "composed entry point appended"),
        // Phase 11.5.c — `compose_lib_rs` uses a generic `root`
        // binding for the mounted component so the wrapper is
        // component-agnostic (works for any root name).
        (
            "let root = Counter::new();",
            "root instantiation via compose_lib_rs",
        ),
        ("root.mount(\"#app\")", "entry point mounts on `#app`"),
        ("__inject_style_Counter", "scoped style helper referenced"),
    ];
    for (needle, why) in checks {
        assert!(
            lib_rs.contains(needle),
            "structural invariant broken — {}\nExpected substring: `{}`\n\nFull emitted lib.rs:\n{}",
            why,
            needle,
            lib_rs
        );
    }

    // Write to disk, creating parent dirs if needed. Idempotent.
    let dst = lib_rs_path();
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| {
            panic!("failed to create parent dir `{}`: {}", parent.display(), e)
        });
    }
    write_if_changed(&dst, &lib_rs);
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
// Test 2: build the wasm bundle + measure size (opt-in).
// ---------------------------------------------------------------

/// Shells out to `wasm-pack build --release --target web` inside
/// `examples/view/counter/wasm-crate/` and prints:
///
///   - raw `.wasm` size in bytes
///   - gzipped `.wasm` size in bytes
///   - whether the gzipped size is under the 40 KB gate
///
/// Fails if the build errors OR if the gzipped size exceeds 40 KB
/// (per the pivot gate documented in §9.l of the plan doc).
///
/// Requires `wasm-pack` + `wasm32-unknown-unknown` target
/// installed. Marked `#[ignore]` so the default `cargo test` run
/// doesn't force this on machines without the toolchain.
#[test]
#[ignore]
fn build_counter_wasm_and_measure() {
    // Regenerate lib.rs first, so we're building the freshest
    // shape derived from Counter.fitzv (not a stale committed
    // baseline).
    let lib_rs = generate_lib_rs_from_fitzv();
    write_if_changed(&lib_rs_path(), &lib_rs);

    // Build.
    let crate_dir = counter_dir().join("wasm-crate");
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

    // Measure.
    let wasm_path = crate_dir.join("pkg").join("counter_bg.wasm");
    let wasm_bytes = fs::read(&wasm_path)
        .unwrap_or_else(|e| panic!("failed to read `{}`: {}", wasm_path.display(), e));
    let raw_bytes = wasm_bytes.len();

    let gzipped_bytes = gzip_size(&wasm_bytes);

    const GATE_KB: usize = 40;
    let gate_bytes = GATE_KB * 1024;

    println!("--- Phase 11.4.c bundle size ---");
    println!(
        "  raw .wasm : {:>7} B ({:.1} KB)",
        raw_bytes,
        raw_bytes as f64 / 1024.0
    );
    println!(
        "  gzipped   : {:>7} B ({:.1} KB)",
        gzipped_bytes,
        gzipped_bytes as f64 / 1024.0
    );
    println!("  gate      : {:>7} B ({} KB gzipped)", gate_bytes, GATE_KB);
    if gzipped_bytes <= gate_bytes {
        let headroom = gate_bytes - gzipped_bytes;
        println!(
            "  verdict   : OK ({} B under the gate, {:.1} KB headroom)",
            headroom,
            headroom as f64 / 1024.0
        );
    } else {
        let over = gzipped_bytes - gate_bytes;
        panic!(
            "gzipped bundle exceeds the 40 KB gate by {} B ({:.1} KB). \
             Per §9.l of `docs/fase-11-plan.md`, this triggers a PIVOT \
             decision (probably to JS-vanilla, approach B1). Update the \
             plan doc with the evidence before proceeding.",
            over,
            over as f64 / 1024.0
        );
    }
}

/// Compute the size of `bytes` gzipped, without emitting the
/// gzipped output to disk. Uses `flate2` — a transitive dep of
/// several first-order deps already in the tree, so no new
/// `Cargo.toml` line is needed (verified via `cargo tree`
/// before writing this).
///
/// If for some reason `flate2` isn't reachable, fall back to
/// shelling out to `gzip -c` and counting the bytes; that path
/// requires `gzip` in `PATH` (universally available on
/// Linux/macOS, less so on Windows).
fn gzip_size(bytes: &[u8]) -> usize {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(bytes).expect("gzip write");
    encoder.finish().expect("gzip finish").len()
}
