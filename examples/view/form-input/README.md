# form-input — Phase 11.7 R3.5b.2 (form-submit payload on WASM)

A `.fitzv` single-file component compiled to WebAssembly that reads a
`<form>`'s named inputs into **`payload`** on submit, and clears fields
marked `data-flv-clear`. This is the R3.5b.2 slice — the second half of
`payload` (after click payloads in R3.5b.1).

The event wiring uses the same `data-flv-*` convention the SSR runtime
reads, so the SAME `.fitzv` compiles to both the SSR target
(fitz-liveviews) and the WASM target.

## Files

| File | Role |
|------|------|
| `App.fitzv` | The `TodoForm` component: `List<Str>` + a form-submit event. No siblings — pure primitives + payload. |
| `index.html` | Mount shim — loads the WASM bundle and mounts `TodoForm` into `#app`. |
| `wasm-crate/` | The generated Rust crate. `Cargo.toml` + `src/lib.rs` are produced by `fitz build --target wasm-client` (and by the smoke test); `pkg/` is the `wasm-pack` output. |

## What it exercises (new in R3.5b.2)

- **`data-flv-submit` binding** — a `<form data-flv-submit="add">` wires a
  `submit` listener that prevents default navigation, reads each named
  input into a payload map, calls the handler, then clears fields marked
  `data-flv-clear`.
- **Field read via `HtmlInputElement`** — each `<input name="text">` is
  located with `form.query_selector("[name=\"text\"]")` and read with
  `.value()`. This needs the `HtmlInputElement` web-sys feature, which the
  emitter adds to the generated `Cargo.toml` **only when a form is
  present** (form-free crates keep the base feature set).
- **`data-flv-clear`** — an input with this attribute is reset to `""`
  after the handler runs.

## Build + run

```sh
# Regenerate the committed src/lib.rs via the smoke test:
cargo test --test view_form_input_wasm_smoke regenerate_form_input_lib_rs

# Build the crate to real WASM (needs the wasm toolchain):
cd examples/view/form-input/wasm-crate
wasm-pack build --release --target web

# Serve the demo (ES modules need an HTTP origin):
cd ..
python -m http.server 8000
# open http://localhost:8000/
```

`rustup target add wasm32-unknown-unknown` + `cargo install wasm-pack`
are prerequisites for the real WASM build.

## Notes / MVP limits

- Form fields are read from `<input>` elements (`HtmlInputElement`).
  `<textarea>` / `<select>` are collected by name but read through the
  same `HtmlInputElement` cast; dedicated support can land if a demo
  needs it.
- With R3.5b.1 (click payload) + R3.5b.2 (form payload), `payload` is
  complete, so the full kanban — per-card move/delete via click value
  attributes + card creation via a form — compiles to a WASM SPA (R3.5c).
