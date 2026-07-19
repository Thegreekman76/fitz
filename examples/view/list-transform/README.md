# list-transform — Phase 11.7 R3.5a.1 (closures + map/filter on WASM)

A `.fitzv` single-file component compiled to WebAssembly that transforms
a **`List<Int>`** entirely client-side with inline closures. This is the
R3.5a.1 slice: the expression machinery — `.map`/`.filter`/`.len()`,
inline closures, list reassignment, and `{#for}` over a call RESULT —
that operates on the nominal *types* R3 (v0.21.8) landed.

It closes two of the three items the [`nominal-list`](../nominal-list/)
demo listed as "still deferred toward the kanban": `.map`/`.filter` +
closures, and `{#for}` over a method-call result. The last one —
calling an *imported* classic helper — is R3.5a.2.

## Files

| File | Role |
|------|------|
| `App.fitzv` | The `App` component: `List<Int>` state + four transform events + a template with two `{#for}` loops (one bare, one over a filter). No siblings — pure primitives. |
| `index.html` | Mount shim — loads the WASM bundle and mounts `App` into `#app`. |
| `wasm-crate/` | The generated Rust crate. `Cargo.toml` + `src/lib.rs` are produced by `fitz build --target wasm-client` (and by the smoke test); `pkg/` is the `wasm-pack` output. |

## What it exercises (new in R3.5a.1)

- **Inline closures in `.map`/`.filter`** —
  `nums = nums.map(fn(n) => n * 2)` and
  `nums = nums.filter(fn(n) => n > 5)` lower to Rust iterator chains.
  `.filter` clones the `&T` the closure receives into an owned binding
  so the predicate body type-checks.
- **List reassignment without a borrow conflict** — the transform reads
  `nums` into a snapshot (`let __rhs = ...`) BEFORE writing it back with
  `borrow_mut()`.
- **Live `.push` mutation** (from R3) — `add_next()` grows the list, so
  the transforms operate on a changing dataset.
- **`{#for m in nums.filter(fn(n) => n > 4)}`** — a `{#for}` over the
  RESULT of a method call, snapshotted into a local and iterated with
  `.into_iter()` (the bare `{#for n in nums}` keeps the state-field
  snapshot path).
- **`{nums.len()}`** — `.len()` in an interpolation, cast `usize` → `i64`
  (classic Fitz `.len()` returns `Int`).

## Build + run

```sh
# Regenerate the committed src/lib.rs via the smoke test:
cargo test --test view_list_transform_wasm_smoke regenerate_list_transform_lib_rs

# Build the crate to real WASM (needs the wasm toolchain):
cd examples/view/list-transform/wasm-crate
wasm-pack build --release --target web

# Serve the demo (ES modules need an HTTP origin):
cd ..
python -m http.server 8000
# open http://localhost:8000/
```

`rustup target add wasm32-unknown-unknown` + `cargo install wasm-pack`
are prerequisites for the real WASM build.

## What's still deferred (toward the full kanban)

R3.5a.1 lands the list-expression machinery. The kanban additionally
needs:

- **Imported classic helper fns transpiled into the WASM crate** — the
  kanban's closures call `move_one` / `keep_if_not` and its template
  iterates `cards_in(cards, "todo")`; those helpers live in a sibling
  classic module. That is **R3.5a.2** — a free-function call rejects
  today with a clear pointer.
- **`payload` + `data-flv-*` event wiring** — the kanban's events read a
  payload built from form fields / value attributes. That is **R3.5b**.

Inline `==` / `!=` in a `.fitzv` event body is a separate view-parser
limit (comparisons like `>` / `<` and arithmetic work); equality
belongs in imported classic helpers. The SSR target (fitz-liveviews)
supports all of the above today.
