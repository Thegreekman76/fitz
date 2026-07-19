# click-payload — Phase 11.7 R3.5b.1 (click payload on WASM)

A `.fitzv` single-file component compiled to WebAssembly that carries
per-item data on a **click event** and reads it back through
**`payload`** in the handler. This is the R3.5b.1 slice: the last piece
the kanban needs on top of R3.5a (nominal types + list transforms +
imported helper fns).

The event wiring uses the same `data-flv-*` convention the SSR runtime
reads, so the SAME `.fitzv` compiles to both the SSR target
(fitz-liveviews) and the WASM target.

## Files

| File | Role |
|------|------|
| `App.fitzv` | The `Picker` component: `List<Int>` + a `Str`, one payload-reading event. No siblings — pure primitives + payload. |
| `index.html` | Mount shim — loads the WASM bundle and mounts `Picker` into `#app`. |
| `wasm-crate/` | The generated Rust crate. `Cargo.toml` + `src/lib.rs` are produced by `fitz build --target wasm-client` (and by the smoke test); `pkg/` is the `wasm-pack` output. |

## What it exercises (new in R3.5b.1)

- **`payload` handler param** — a handler that reads `payload` gains a
  `payload: &HashMap<String, String>` parameter. Handlers that don't
  reference payload keep their zero-arg signature (byte-identical to the
  pre-R3.5b examples).
- **`payload.has("key")` / `payload["key"]`** — lower to
  `payload.contains_key(...)` and
  `payload.get(...).cloned().unwrap_or_default()` (a `Map<Str, Str>`
  lookup; a missing key yields `""`).
- **Interpolated attributes** — `data-flv-value-val="{n}"` sets the attr
  from the loop var via `set_attribute(name, &format!("{}", n))`.
- **`data-flv-click` binding** — wires a click listener that reads the
  element's `data-flv-value-*` attributes back into a payload map, then
  calls the target handler.

## Build + run

```sh
# Regenerate the committed src/lib.rs via the smoke test:
cargo test --test view_click_payload_wasm_smoke regenerate_click_payload_lib_rs

# Build the crate to real WASM (needs the wasm toolchain):
cd examples/view/click-payload/wasm-crate
wasm-pack build --release --target web

# Serve the demo (ES modules need an HTTP origin):
cd ..
python -m http.server 8000
# open http://localhost:8000/
```

`rustup target add wasm32-unknown-unknown` + `cargo install wasm-pack`
are prerequisites for the real WASM build.

## What's still deferred (toward the FULL kanban)

- **`data-flv-submit` (form-field payload)** — creating an item from a
  `<form>`'s named inputs (and clearing them via `data-flv-clear`) is
  **R3.5b.2**. This demo only covers click payloads.

With R3.5b.1 + R3.5b.2, the full kanban (per-card move/delete via click
value attrs + card creation via a form) compiles to a WASM SPA (R3.5c).
