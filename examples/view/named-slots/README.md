# named-slots — v0.24.0 (`<slot name="X" />` on WASM)

A `.fitzv` compiled to WebAssembly where a child component exposes
**multiple named holes** (`<slot name="title" />`, a default `<slot />`,
`<slot name="actions" />`) and the parent fills each one independently.
This rounds out `<Child />` composition: v0.22.0 shipped the single
default slot; this adds named slots.

## Files

| File | Role |
|------|------|
| `App.fitzv` | `App` (parent — fills a `Card`'s title/body/actions, plus a second `Card` that fills only the title) + `Card` (child — three slots, each with fallback). |
| `index.html` | Mount shim. |
| `wasm-crate/` | The generated Rust crate (`Cargo.toml` + `src/lib.rs` produced by `fitz build --target wasm-client` / the smoke test; `pkg/` is the `wasm-pack` output). |

## How it works

- A component gains one callback field per slot: `__slot` for the default
  `<slot />` and `__slot_<name>` for each `<slot name="X" />` (hyphens fold
  to `_`).
- The parent fills a named slot by tagging a top-level element inside
  `<Card>...</Card>` with `slot="X"` — the native Web Components
  convention. Content **without** a `slot=` attribute fills the default
  slot.
- For each filled slot the parent synthesises a `__render_slot_<n>` method
  that renders the content **in the parent's scope** (parent state + event
  handlers) into a target node the child hands over at its matching
  `<slot />`. Wired via
  `child.__slot_<name> = Some(Rc::new(move |t| parent.__render_slot_<n>(t)))`.
- A slot the parent doesn't fill renders the child's own fallback children
  (`<slot name="title"><em>untitled</em></slot>`).
- Renderers run on every render, so parent-state slot content stays
  reactive — click the first card's **like** button and watch its footer
  slot update.

Components without a `<slot />` are unchanged (no `__slot*` fields), so the
other view examples regenerate byte-for-byte.

## Build + run

```sh
cargo test --test view_named_slots_wasm_smoke regenerate_named_slots_lib_rs
cd examples/view/named-slots/wasm-crate
wasm-pack build --release --target web
cd ..
python -m http.server 8000   # open http://localhost:8000/
```

## MVP limits

- **No `<Child />` inside slot content** — the parent's slot renderer has
  no instance cache for a nested component; rejects with a clear message.
- Slot fill is a client-WASM capability; the SSR target rejects both
  `<slot />` and `<Child>content</Child>`.
- A `slot="X"` that targets no `<slot name="X" />` in the child, or
  unslotted content when the child has no default `<slot />`, is rejected
  at compile with a clear pointer.
