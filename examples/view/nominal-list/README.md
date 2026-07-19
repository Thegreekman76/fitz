# nominal-list — Phase 11.7 R3 (nominal types on WASM)

A `.fitzv` single-file component compiled to WebAssembly that iterates
a **`List<Card>`** — a list of an imported classic `type` — and fans
each item's fields out into a keyed child. This is the R3 slice: the
first client-WASM demo where a **nominal** (user-defined `type`), not
just a primitive, flows through state, `{#for}`, field access, keyed
composition, and live list mutation.

R3 is the foundational prerequisite the kanban SPA port was blocked
on. See "What's still deferred" below for what the kanban needs on top
of R3.

## Files

| File | Role |
|------|------|
| `card.fitz` | Sibling classic module declaring `type Card { id, title, done }`. `.fitzv` files can't declare `type`s, so shared nominals live in a classic module (the canonical cross-file pattern). |
| `App.fitzv` | The `App` root (owns `List<Card>` state + the `add` event) and the `CardRow` child (per-row `taps` state). |
| `index.html` | Mount shim — loads the WASM bundle and mounts `App` into `#app`. |
| `wasm-crate/` | The generated Rust crate. `Cargo.toml` + `src/lib.rs` are produced by `fitz build --target wasm-client` (and by the smoke test); `pkg/` is the `wasm-pack` output. |

## What it exercises (all new in R3)

- **Imported nominal struct emission** — the emitter loads `card.fitz`'s
  `type Card` field list and synthesises
  `#[derive(Clone)] pub struct Card { id: i64, title: String, done: bool }`
  inline in the WASM crate.
- **`List<Card>` state** — `state { cards: List<Card> = [] }` →
  `RefCell<Vec<Card>>`.
- **Nominal construction + live list mutation** —
  `event add() { cards.push(Card { id: next_id, title: "Task", done: false }) }`.
  Keys appear at runtime, so reconciliation actually evicts/retains
  (the keyed-composition demo had a constant list).
- **`{#for c in cards}`** over the `List<Card>` — the loop var `c`
  binds as `Card`.
- **Field access** — `{c.title}` / `{c.id}` lower to `c.title.clone()`
  / `c.id.clone()`.
- **Keyed nominal → primitive props** —
  `<CardRow key="{c.id}" n="{c.id}" title="{c.title}" />` fans the
  nominal item's fields into the child's primitive props, and each
  card id keys a cached `CardRow` so its `taps` survives re-renders.

## Build + run

```sh
# From the repo root — build the WASM bundle.
fitz build --bin app --target wasm-client   # (manifest-driven; see below)

# Or regenerate the committed src/lib.rs via the smoke test:
cargo test --test view_nominal_list_wasm_smoke regenerate_nominal_list_lib_rs

# Build the crate to real WASM (needs the wasm toolchain):
cd examples/view/nominal-list/wasm-crate
wasm-pack build --release --target web

# Serve the demo (ES modules need an HTTP origin):
cd ..
python -m http.server 8000
# open http://localhost:8000/
```

`rustup target add wasm32-unknown-unknown` + `cargo install wasm-pack`
are prerequisites for the real WASM build.

## What's still deferred (toward the full kanban)

R3 lands nominal *types*. The kanban additionally needs:

- **`{#for}` over the result of a fn call** — the kanban iterates
  `{#for c in cards_in(cards, "todo")}`, not a bare state field.
- **`.map` / `.filter` + closures in event bodies** — the kanban's
  move/delete events reassign the list via `cards.map(fn(c) => ...)`.
- **Imported classic helper fns transpiled into the WASM crate** —
  `cards_in`, `move_one`, `keep_if_not`, `make_card`.

Those are the next slice (imported-fn support on the WASM target). The
SSR target (fitz-liveviews) supports all of the above today.
