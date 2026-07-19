# event-bubbling — Phase 11.7.c (child→parent event bubbling on WASM)

A `.fitzv` compiled to WebAssembly where a child component notifies its
parent via `<Child @event="handler" />`. This rounds out `<Child />`
composition: **props go down, events bubble up**.

## Files

| File | Role |
|------|------|
| `App.fitzv` | `App` (parent, binds `<Item @choose="on_pick" />`) + `Item` (child, declares `event choose()`). |
| `index.html` | Mount shim. |
| `wasm-crate/` | The generated Rust crate (`Cargo.toml` + `src/lib.rs` produced by `fitz build --target wasm-client` / the smoke test; `pkg/` is the `wasm-pack` output). |

## How it works

- The parent binds `<Item @choose="on_pick" />`. `on_pick` must be an
  `event` of the parent; `choose` must be an `event` of `Item` (both
  validated at check time).
- `Item`'s own `@click="choose"` fires `choose`. After `choose` runs +
  the child re-renders, the WASM emitter fires the registered parent
  callback → `App::on_pick` runs + the parent re-renders.
- Under the hood: the child struct gains one `__on_choose:
  RefCell<Option<Box<dyn Fn()>>>` slot per bubbled event; the parent sets
  it (capturing its own `Rc`) when it mounts the child; the child's
  handler invokes it.

Only events that some parent actually binds get a callback slot, so
components with no bubbled events emit byte-for-byte unchanged.

## Build + run

```sh
cargo test --test view_event_bubbling_wasm_smoke regenerate_event_bubbling_lib_rs
cd examples/view/event-bubbling/wasm-crate
wasm-pack build --release --target web
cd ..
python -m http.server 8000   # open http://localhost:8000/
```

## MVP limit

The bubble carries **no payload**, so `on_pick` can't tell which of the
three items fired — it just counts clicks. Passing the child's state up
as a payload (so the parent knows *which* item) is a later slice. Event
bubbling is a client-WASM capability; the SSR target rejects `@event` on
a child (it re-renders the whole tree per event over the wire).
