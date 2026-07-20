# cross-file-transitive — v0.26.0 (transitive + aliased cross-file `<Child />`)

A `.fitzv` compiled to WebAssembly that exercises the two refinements
v0.26.0 adds on top of v0.25.0's cross-file `<Child />`:

- **Aliasing** — `App.fitzv` imports the child with
  `from Card import Card as Row` and composes `<Row />`. The loader
  registers a renamed clone under the alias, so the parent's composition
  resolves against it (props down, `@like` event up), while the original
  `Card` name stays available for a component's own file-local siblings.
- **Transitivity** — `App` imports ONLY `Card`. `Card.fitzv` in turn
  imports and composes `<Badge />` from a THIRD file (`Badge.fitzv`). The
  transitive import walk follows `Card.fitzv`'s own imports, so `Badge`
  is discovered and inlined without `App` having to import it by hand.

## Files

| File | Role |
|------|------|
| `App.fitzv` | `App` (parent / WASM root) — `from Card import Card as Row`, composes two `<Row />`s with a static prop and a bubbled `@like`. |
| `Card.fitzv` | the middle component — imports+composes `<Badge />`, owns a `like` event, has its own scoped style. |
| `Badge.fitzv` | the transitively-reached grandchild — a leaf with a `label` prop. |
| `index.html` | Mount shim. |
| `wasm-crate/` | The generated Rust crate (`Cargo.toml` + `src/lib.rs`; `pkg/` is the `wasm-pack` output). |

## How it works

- `fitz build --target wasm-client` first walks the `.fitzv` import graph
  (`view::collect_transitive_view_imports`) to compute the transitive
  union of imports, then runs the three loaders over it — so the
  grandchild `Badge`, imported only by `Card`, is registered.
- `view::load_imported_components` honours the `as` alias: it registers a
  renamed clone under `Row` (so `<Row />` resolves) alongside the original
  `Card`. Only the components reachable from the parent's `<Child />` refs
  are emitted, so the unreached original `Card` is not double-emitted next
  to `Row`.
- The checker validates the parent's `<Row />` composition against the
  aliased child's real surface; the emitter inlines each reachable
  component's whole emit into the one generated crate.

## Build + run

```sh
cargo test --test view_cross_file_transitive_wasm_smoke regenerate_cross_file_transitive_lib_rs
cd examples/view/cross-file-transitive/wasm-crate
wasm-pack build --release --target web
cd ..
python -m http.server 8000   # open http://localhost:8000/
```

## MVP limits

- **One level of file nesting per resolution step** — the walk follows
  `.fitzv → .fitzv` imports transitively but resolves every file against a
  single flat directory (all view files in one dir, the layout these
  examples use).
- **Local wins on a name collision**; a cross-file dup (same name from two
  files) keeps the first import.
- **No aliasing collision handling** — importing the SAME component twice
  under two different aliases applies the first import's alias only.
- Cross-file composition is a client-WASM capability; the SSR target uses
  the runtime `component(...)` API instead.
