# mini-board — Phase 11.7 R3.5a.2 (imported classic helper fns on WASM)

A `.fitzv` single-file component compiled to WebAssembly that calls
**imported classic helper functions** — transpiled into the WASM bundle
from a sibling classic module. This is the R3.5a.2 slice: the emitter now
lowers a sibling `.fitz`'s `fn`s to real Rust `fn`s, so the SFC's template
and event bodies can call them.

It's the kanban's **shape** minus per-card targeting: moving/deleting a
*specific* card needs a `payload` carrying the target id (R3.5b). Here
every button acts on the whole board (add / advance-all / reset).

## Files

| File | Role |
|------|------|
| `card.fitz` | Sibling classic module: `type Card { id, title, column }`. The WASM emitter synthesises its Rust struct inline (R3). |
| `board_helpers.fitz` | Pure helpers — `cards_in` (per-column filter), `next_column` (state machine), `advance`, `make_card`. Transpiled into the bundle (R3.5a.2). Equality (`c.column == col`) lives here, in the classic parser. |
| `App.fitzv` | The `Board` component: `List<Card>` state + add/advance-all/reset events + a 3-column template. |
| `index.html` | Mount shim — loads the WASM bundle and mounts `Board` into `#app`. |
| `wasm-crate/` | The generated Rust crate. `Cargo.toml` + `src/lib.rs` are produced by `fitz build --target wasm-client` (and by the smoke test); `pkg/` is the `wasm-pack` output. |

## What it exercises (new in R3.5a.2)

- **Imported-fn transpilation** — `board_helpers.fitz`'s `fn`s become
  Rust `fn`s in the bundle. `cards_in(all: List<Card>, col: Str)` →
  `fn cards_in(all: Vec<Card>, col: String) -> Vec<Card>`.
- **Transitive helpers** — `advance` calls `next_column`, which the SFC
  never imports; the emitter transpiles it anyway (it's reachable
  through an imported helper).
- **Free-function calls with argument cloning** — `advance(c)` inside a
  `.map` closure emits `advance(c.clone())`, and `make_card(next_id, ...)`
  emits `make_card((*self.next_id.borrow()).clone(), ...)`. Cloning bare
  arguments is what makes a String/nominal captured by a `.map`/`.filter`
  closure survive the `FnMut` borrow.
- **`{#for c in cards_in(cards, "todo")}`** — a `{#for}` over the RESULT
  of an imported fn (each column is a filtered view).
- **`.len()` on an imported-fn result** — `{cards_in(cards, "done").len()}`.

## Build + run

```sh
# Regenerate the committed src/lib.rs via the smoke test:
cargo test --test view_mini_board_wasm_smoke regenerate_mini_board_lib_rs

# Build the crate to real WASM (needs the wasm toolchain):
cd examples/view/mini-board/wasm-crate
wasm-pack build --release --target web

# Serve the demo (ES modules need an HTTP origin):
cd ..
python -m http.server 8000
# open http://localhost:8000/
```

`rustup target add wasm32-unknown-unknown` + `cargo install wasm-pack`
are prerequisites for the real WASM build.

## What's still deferred (toward the FULL kanban)

R3.5a.2 lands imported-fn transpilation. The full kanban additionally
needs:

- **`payload` + `data-flv-*` event wiring** — per-card move/delete reads
  a target id from the clicked element (`data-flv-value-card_id="{c.id}"`)
  or from form fields on submit. That is **R3.5b**.

Notes / MVP limits:
- Every `fn` in an imported sibling module is transpiled (to pick up
  internal helpers like `next_column`); a sibling `fn` using an
  unsupported construct (`match`, loops, `?`) rejects at emit even if
  unused. Split such helpers into their own module.
- Imported fns need param + return-type annotations (no inference).
- Inline `==`/`!=` isn't accepted in a `.fitzv` event body (only in
  classic helpers); `>`/`<` and arithmetic work.
