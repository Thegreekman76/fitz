# kanban — Phase 11.7 R3.5c (the full kanban as a WASM SPA)

**The headline of Phase 11.7.** The collaborative-kanban Board —
previously an SSR-only single-file component in
[fitz-liveviews](https://github.com/Thegreekman76/fitz-liveviews) —
compiled to a standalone **client-side WebAssembly SPA** from one
`.fitzv` (plus two sibling classic modules). Add cards, move them between
columns, delete them — entirely in the browser, no server, no WebSocket.

Every R3.5 slice converges here:

| Capability | Slice | Where it shows |
|------------|-------|----------------|
| Nominal type (`Card`) synthesised inline | R3 | `card.fitz` |
| Imported helper fns transpiled into the bundle | R3.5a.2 | `board_helpers.fitz` |
| `.map`/`.filter` closures calling imported helpers | R3.5a.1/.2 | `move_*` / `delete_card` events |
| `{#for c in cards_in(cards, "todo")}` + `.len()` | R3.5a.2 | the three columns |
| Click payload (`data-flv-value-card_id="{c.id}"`) | R3.5b.1 | per-card move/delete buttons |
| Form-submit payload + `data-flv-clear` | R3.5b.2 | the "Add Card" form |
| String interpolation (`let id_str = "{next_id}"`) | R3.5c | `create_card` |

The SAME `data-flv-*` conventions the SSR runtime reads drive the WASM
listeners, so **this exact `.fitzv` targets both backends**.

## Files

| File | Role |
|------|------|
| `card.fitz` | `type Card { id, title, author, column }` (id is `Str` to match the payload). |
| `board_helpers.fitz` | Pure helpers: `cards_in`, `keep_if_not`, `move_one`, `next_column`, `prev_column`, `make_card`. |
| `App.fitzv` | The `Board` component — state + create/move/delete events + the 3-column template. |
| `index.html` | Mount shim. |
| `wasm-crate/` | The generated Rust crate. `Cargo.toml` + `src/lib.rs` are produced by `fitz build --target wasm-client` (and by the smoke test); `pkg/` is the `wasm-pack` output. |

## Bundle size

The whole app — nominal struct, six transpiled helpers, click + form
payload wiring — is **~57 KB raw / ~21.5 KB gzipped**, under the 40 KB
gzipped gate.

## Build + run

```sh
# Regenerate the committed src/lib.rs via the smoke test:
cargo test --test view_kanban_wasm_smoke regenerate_kanban_lib_rs

# Build the crate to real WASM (needs the wasm toolchain):
cd examples/view/kanban/wasm-crate
wasm-pack build --release --target web

# Serve the demo (ES modules need an HTTP origin):
cd ..
python -m http.server 8000
# open http://localhost:8000/
```

`rustup target add wasm32-unknown-unknown` + `cargo install wasm-pack`
are prerequisites for the real WASM build.

## SSR vs WASM

The SSR version of this board (in fitz-liveviews) renders on the server
and syncs across clients over a WebSocket with compact HTML patches —
so two browser windows stay in sync. This WASM version runs entirely in
one browser tab (no shared server state). The `.fitzv` source is the
same shape; only the compile target differs.

## What's left in Phase 11.7 (not needed by the kanban)

- **Event bubbling** (`<Card @select="handler" />` child→parent) and
  **`<slot />`** fallback content — composition features that round out
  `<Child />` usage. The kanban renders its cards inline (no per-card
  child component), so it doesn't need them.
