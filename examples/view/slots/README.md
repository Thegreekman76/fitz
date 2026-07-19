# slots — Phase 11.7.d (`<slot />` with fallback on WASM)

A `.fitzv` compiled to WebAssembly where a child component exposes a
`<slot />` hole that the parent fills with content — or the child's
fallback shows when the parent provides none. This is the other half of
`<Child />` composition (props/events go one way; slot content the other).

## Files

| File | Role |
|------|------|
| `App.fitzv` | `App` (parent, fills `<Panel>content</Panel>` and leaves `<Panel />` empty) + `Panel` (child, `<slot>fallback</slot>`). |
| `index.html` | Mount shim. |
| `wasm-crate/` | The generated Rust crate (`Cargo.toml` + `src/lib.rs` produced by `fitz build --target wasm-client` / the smoke test; `pkg/` is the `wasm-pack` output). |

## How it works

- A component with a `<slot />` gains a `__slot: RefCell<Option<Rc<dyn
  Fn(&Node)>>>` callback field.
- `<Panel>content</Panel>` (non-self-closing) fills the slot: the parent
  synthesises a `__render_slot_<n>` method that renders the content **in
  the parent's scope** (parent state + event handlers) into a target node
  the child hands over at its `<slot />`. Wired via
  `child.__slot = Some(Rc::new(move |t| parent.__render_slot_<n>(t)))`.
- At `<slot />`, the child calls the callback if set, else renders its own
  fallback children (`<slot>fallback</slot>`).
- The renderer runs on every render, so parent-state slot content stays
  reactive (and event listeners are fresh).

Components without a `<slot />` are unchanged (no `__slot` field), so the
other view examples regenerate byte-for-byte.

## Build + run

```sh
cargo test --test view_slots_wasm_smoke regenerate_slots_lib_rs
cd examples/view/slots/wasm-crate
wasm-pack build --release --target web
cd ..
python -m http.server 8000   # open http://localhost:8000/
```

## MVP limits

- **Default slot only** — `<slot name="X" />` (named slots) rejects.
- **No `<Child />` inside slot content** — the parent's slot renderer has
  no instance cache for a nested component; rejects with a clear message.
- Slot fill is a client-WASM capability; the SSR target rejects both
  `<slot />` and `<Child>content</Child>`.
