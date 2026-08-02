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

## Reactivity — keep-node reconciliation (Phase 11.10)

A component with a live form control (`@input`/`@change`) over a **static
template** builds its DOM once and then **patches in place** on a state
change: the emitter stashes a handle per interpolation point and, on a
keystroke, updates only those nodes (`set_data` on a text node,
`set_attribute` on an element). The `<input>` element itself is never
re-created, so the caret stays where you put it mid-string — verified in
Chrome (caret after `He`, typing `XY` yields `HeXYllo`, not `HeXlloY`).

Every other component keeps the byte-identical naive re-render; keep-node is
gated to the live-input + static-structure case it fixes. Reconciling
control flow (`{#if}`/`{#for}`) and derived values (`memo`) are later
slices of Phase 11.10.

## Build + view

```bash
fitz build --bin app --target wasm-client   # (from a manifest with this as a bin)
# or regenerate + build via the smoke:
cargo test --test view_live_input_wasm_smoke build_live_input_wasm -- --ignored
python -m http.server 8000                  # serve (ES modules need HTTP)
# open http://localhost:8000/examples/view/live-input/
```
