# event-bubbling — Phase 11.7.c + payload bubbling (child→parent events on WASM)

A `.fitzv` compiled to WebAssembly where a child component notifies its
parent via `<Child @event="handler" />`, **carrying a payload** so the
parent knows which child fired. This rounds out `<Child />` composition:
**props go down, events bubble up (with data)**.

## Files

| File | Role |
|------|------|
| `App.fitzv` | `App` (parent, binds `<Item @choose="on_pick" />`) + `Item` (child, declares `event choose()` and exposes its `label` via `data-flv-value-label`). |
| `index.html` | Mount shim. |
| `wasm-crate/` | The generated Rust crate (`Cargo.toml` + `src/lib.rs` produced by `fitz build --target wasm-client` / the smoke test; `pkg/` is the `wasm-pack` output). |

## How it works

- The parent binds `<Item @choose="on_pick" />`. `on_pick` must be an
  `event` of the parent; `choose` must be an `event` of `Item` (both
  validated at check time).
- `Item`'s `data-flv-click="choose"` + `data-flv-value-label="{label}"`
  fires `choose` with a payload `{ "label": "Apple" }`. After `choose`
  runs + the child re-renders, the WASM emitter fires the registered
  parent callback → `App::on_pick(&parent, payload)` runs + the parent
  re-renders.
- `on_pick` reads `payload["label"]`, so it knows *which* item was picked.
- Under the hood: the child struct gains one `__on_choose:
  RefCell<Option<Box<dyn Fn(&HashMap<String, String>)>>>` slot per bubbled
  event; the parent sets it (capturing its own `Rc`) when it mounts the
  child; the child's handler invokes it, forwarding the payload it
  received.

The payload reuses the SAME `data-flv-value-*` machinery R3.5b uses for
plain click handlers — a bubbled event just forwards the payload it got
up to its parent. The child chooses what to expose by which
`data-flv-value-*` attributes it sets. Only events that some parent
actually binds get a callback slot, so components with no bubbled events
emit byte-for-byte unchanged.

## Build + run

```sh
cargo test --test view_event_bubbling_wasm_smoke regenerate_event_bubbling_lib_rs
cd examples/view/event-bubbling/wasm-crate
wasm-pack build --release --target web
cd ..
python -m http.server 8000   # open http://localhost:8000/
```

## Notes + limits

- The bubbled payload is a `Map<Str, Str>` (the `data-flv-value-*`
  convention), so numbers/bools arrive as strings — parse on the parent
  side if you need them typed.
- A parent handler that ignores the payload (`event on_pick() { ... }`
  with no `payload` reference) still works — the closure just drops it.
- Event bubbling is a client-WASM capability; the SSR target rejects
  `@event` on a child (it re-renders the whole tree per event over the
  wire).
