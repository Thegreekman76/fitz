# live-input — CW.9 (live value binding on WASM: `@input` / `@change`)

A `.fitzv` single-file component compiled to WebAssembly that reads a form
control's **live value** into the handler's `payload` under the `"value"`
key — `@input` on every keystroke, `@change` on selection/blur.

The event wiring matches the SSR emitter, which lowers any `@event` to
`data-flv-<event>`, so the SAME `.fitzv` targets both the SSR runtime
(fitz-liveviews) and the WASM target.

## Files

| File | Role |
|------|------|
| `App.fitzv` | The `LiveInput` component: a `<select>` (`@change`) + a text `<input>` (`@input`), both writing `payload["value"]` to state. |
| `index.html` | Mount shim — loads the WASM bundle and mounts `LiveInput` into `#app`. |
| `wasm-crate/` | The generated Rust crate. `Cargo.toml` + `src/lib.rs` are produced by `fitz build --target wasm-client` (and by the smoke test); `pkg/` is the `wasm-pack` output. |

## What it exercises (new in CW.9)

- **`@input` / `@change` value binding** — the emitter wires a DOM
  listener that reads the event target's current value and calls the
  handler with a payload carrying it under `"value"`. The handler reads
  `payload["value"]` and writes it to state.
- **Covers `<input>`, `<select>`, `<textarea>`** — the listener casts the
  target to each concrete element type, so the same wiring serves a text
  field, a dropdown, and a multi-line area.
- **Conditional web-sys features** — the emitter adds
  `HtmlInputElement` / `HtmlSelectElement` / `HtmlTextAreaElement` to the
  generated `Cargo.toml` **only when a component uses `@input`/`@change`**
  (value-free crates keep the base feature set).

## Caveat — naive re-render + live text inputs

A state change rebuilds the whole component DOM (the current model is
dirty-flag + naive re-render). For a `<select> @change` this is invisible
(there's no caret to lose). For a live text `<input> @input`, each
keystroke re-mounts the field: the value is bound back via `value="{name}"`,
but the caret jumps to the end. Fine-grained reactivity (patching in place)
is the ROADMAP's next iteration; the CW.9 capability here is the value
reliably flowing to the handler.

## Build + view

```bash
fitz build --bin app --target wasm-client   # (from a manifest with this as a bin)
# or regenerate + build via the smoke:
cargo test --test view_live_input_wasm_smoke build_live_input_wasm -- --ignored
python -m http.server 8000                  # serve (ES modules need HTTP)
# open http://localhost:8000/examples/view/live-input/
```
