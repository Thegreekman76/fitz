# cross-file-child — v0.25.0 (cross-file `<Child />` on WASM)

A `.fitzv` compiled to WebAssembly whose `<Card />` lives in a
**separate `.fitzv` file** (`Card.fitzv`), imported with
`from Card import Card`. This closes the last open piece of the WASM
composition surface: before this slice, a `<Child />` had to be declared
in the SAME file as its parent (the workaround was the runtime
`component("Name", "id")` API of `fitz-liveviews`). Now props flow down,
events bubble up, and slots fill — all across a file boundary.

## Files

| File | Role |
|------|------|
| `App.fitzv` | `App` (parent) — composes two `<Card />`s with a static prop, a bubbled `@like`, and slot content. |
| `Card.fitzv` | `Card` (the imported child) — owns its `likes` state, a `like` event, a named `badge` slot + a default slot, and its own scoped style. |
| `index.html` | Mount shim. |
| `wasm-crate/` | The generated Rust crate (`Cargo.toml` + `src/lib.rs` produced by `fitz build --target wasm-client` / the smoke test; `pkg/` is the `wasm-pack` output). |

## How it works

- `fitz build --target wasm-client` loads every component declared in
  each imported sibling `.fitzv` (`view::load_imported_components`) and
  inlines its **whole** emit — struct + `new` + event handlers + render +
  `<style scoped>` — into the one generated crate.
- The emitter merges the *reachable* imported components (the transitive
  closure of `<Child />` refs from the local components) ahead of the
  local ones into a single synthetic file, so every existing pass —
  bubbled-event collection, per-component emit, and same-file child
  resolution — treats the cross-file child as if it were local.
- The checker validates the parent's `<Card />` composition (prop
  existence + type, `@event` binding, slot fill) against the imported
  child's **real surface** instead of reporting an unknown component.
- Each imported component brings its own scoped-style hash
  (`FNV-1a(name::css)`), baked at its own file's expand — so `App`'s and
  `Card`'s scoped styles never collide.

## Build + run

```sh
cargo test --test view_cross_file_child_wasm_smoke regenerate_cross_file_child_lib_rs
cd examples/view/cross-file-child/wasm-crate
wasm-pack build --release --target web
cd ..
python -m http.server 8000   # open http://localhost:8000/
```

## Since v0.26.0

The three MVP limits this example originally shipped with are now closed
(see `examples/view/cross-file-transitive/` for a demo):

- **Transitivity** — an imported `.fitzv`'s own `from X import Y` are now
  followed, so a grandchild component in a file the entry does not import
  is discovered and inlined.
- **Component aliasing** — `from Card import Card as Row` now registers a
  renamed clone, so `<Row />` resolves.
- **LSP cross-file** — diagnostics over a single `.fitzv` now resolve
  imported sibling components (the LSP derives the document's directory),
  so a cross-file `<Child />` is no longer flagged as unknown.

## Remaining limits

- **Local wins on a name collision** — a local component shadows an
  imported one of the same name; a cross-file dup (same name from two
  files) keeps the first import.
- Cross-file composition is a client-WASM capability; the SSR target uses
  the runtime `component(...)` API instead.
