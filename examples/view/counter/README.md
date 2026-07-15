# Counter demo — Phase 11.4.c

This is the canonical Phase 11.4.c POC of the `.fitzv` → WebAssembly
pipeline. It validates that a `Counter.fitzv` source parses,
expands, type-checks, and emits Rust that compiles to a working
browser demo through the Phase 11.4.b WASM emitter (`src/view/codegen_wasm.rs`).

## Structure

```
examples/view/counter/
├── Counter.fitzv           # the source component
├── index.html              # mount point + <script type="module">
├── README.md               # this file
└── wasm-crate/             # Rust crate that wraps the emitted code
    ├── Cargo.toml          # deps: wasm-bindgen, web-sys, console_error_panic_hook
    ├── .gitignore          # target/, pkg/, Cargo.lock
    └── src/
        └── lib.rs          # AUTO-GENERATED — do NOT edit by hand
```

## What lives where

- **`Counter.fitzv`** is the source of truth. Edit this to change
  the component's shape.
- **`wasm-crate/src/lib.rs`** is generated from `Counter.fitzv` by
  `tests/view_counter_wasm_smoke.rs::regenerate_counter_lib_rs`.
  Every `cargo test` in this repo re-runs the pipeline and
  overwrites `lib.rs` if the emitter's output changed. The file
  is committed in-tree so a fresh clone can `wasm-pack build`
  without running the smoke first.
- **`index.html`** loads the ES-module wrapper produced by
  `wasm-pack` (`wasm-crate/pkg/counter.js`) and calls `init()`.
  The generated JS is out of tree (see `.gitignore`).

## Building the WASM bundle

Prerequisites (install once per machine):

```
rustup target add wasm32-unknown-unknown
cargo install wasm-pack
```

Then from this directory:

```
cd wasm-crate
wasm-pack build --release --target web
```

That produces `wasm-crate/pkg/` with:

- `counter_bg.wasm` — the WASM bundle (what the bundle-size gate
  measures)
- `counter.js` — the ES-module glue that `index.html` imports
- `counter.d.ts` — TypeScript type declarations (informational)

## Running the browser smoke

ES modules will not load over `file://`, so serve the folder over
HTTP. From this directory:

```
python -m http.server 8000
```

then open <http://localhost:8000/> in a browser. You should see:

1. The counter widget rendered inside `#app` with initial value `0`.
2. Clicking `+` increments the value.
3. Clicking `-` decrements.
4. Clicking `reset` sets it back to `0`.

Each click re-renders the entire counter subtree (naive re-render
on state mutation, per §9.m D1 of `docs/fase-11-plan.md`). No
signals, no VDOM diffing — the whole DOM tree of the component
gets torn down and rebuilt from the current state. Acceptable for
POC, refinable via approach A3 (in-tree runtime crate) if bloat
with N components becomes measurable.

## Bundle-size gate (40 KB gzipped)

`docs/stack.md` v1 commits to WASM-first "when it fits in ~40 KB
gzipped". Phase 11.4 uses that as the pivot gate: if the counter's
`counter_bg.wasm` exceeds 40 KB gzipped, we PIVOT to JS-vanilla
(approach B1 in §9.l of the plan doc) and refresh `docs/stack.md`
with the evidence.

The measurement is automated:

```
cd <repo root>
cargo test --test view_counter_wasm_smoke -- --ignored
```

This runs `build_counter_wasm_and_measure`, which:

1. Regenerates `lib.rs` from `Counter.fitzv`.
2. Shells out to `wasm-pack build --release --target web`.
3. Reads `pkg/counter_bg.wasm`, computes raw + gzipped sizes.
4. Compares gzipped size against the 40 KB gate.
5. Prints a verdict; fails the test if over the gate.

The test is `#[ignore]` because it requires `wasm-pack` +
`wasm32-unknown-unknown` on the runner. The default `cargo test`
run does NOT force this dependency on contributors.

## Where the composition happens

Phase 11.4.b's `view::emit_module()` deliberately does NOT emit
`#[wasm_bindgen(start)]`. That composition wires a specific
component (or a tree of components) into a specific mount point
selector — the emitter is agnostic about which. In this demo the
composition lives at the tail of `wasm-crate/src/lib.rs` (added
by the harness in `tests/view_counter_wasm_smoke.rs`):

```rust
#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    let counter = Counter::new();
    counter.mount("#app")?;
    Ok(())
}
```

Phase 11.5's CLI will emit an equivalent wrapper per target
(`fitz build --target wasm --entry Counter --mount '#app'`),
replacing this ad-hoc composition.

## Regenerating `lib.rs` by hand

Normally `cargo test` handles this. To force a regeneration
without running the full test suite:

```
cargo test --test view_counter_wasm_smoke regenerate_counter_lib_rs
```

If you want to see what would change without writing to disk,
inspect the smoke test — the `write_if_changed()` helper is a
no-op when the emitted content is identical.
