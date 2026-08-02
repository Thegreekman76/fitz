# search-filter — keep-node dynamic regions (Phase 11.10 slice 3)

A `.fitzv` single-file component compiled to WebAssembly where a live text
`<input>` filters a `{#for}` list **as you type**, without the search box
losing its caret.

## What it exercises (new in slice 3)

Slice 1 gave keep-node reconciliation (patch in place, caret preserved) but
only for **static** templates. A value-input component with `{#if}`/`{#for}`
fell back to the naive re-render — so a search box next to a live list would
have its caret jump on every keystroke.

Slice 3 lifts that limitation: under keep-node, each `{#if}`/`{#for}` becomes
an **anchored dynamic region**. On a state change:

- the `<input>` (a static sibling) is **patched in place** — the element is
  never re-created, so the caret stays where you put it;
- each `{#if}`/`{#for}` region is **rebuilt wholesale** between two comment
  anchors: the emitter clears the nodes between the anchors and re-runs the
  naive `{#if}`/`{#for}` emit into a `DocumentFragment`, inserted back before
  the end anchor.

Typing `banana` hides the `banana` item (`{#for it in items}{#if it != query}`)
and updates the `{#if query != ""}` hint — while the caret in the search box
never moves.

## Files

| File | Role |
|------|------|
| `App.fitzv` | The `Filter` component: a live `<input>` + a `{#if}` hint + a `{#for}` list that hides the item matching the query. |
| `index.html` | Mount shim — loads the WASM bundle and mounts `Filter` into `#app`. |
| `wasm-crate/` | The generated Rust crate. `Cargo.toml` + `src/lib.rs` are produced by `fitz build --target wasm-client` (and by the smoke test); `pkg/` is the `wasm-pack` output. |

## Limitation

Nodes **inside** a region are re-created on each change — a region has no
per-item state or caret to preserve (its content is fully rebuilt). Keeping
identity for nodes inside a `{#for}` (keyed reconciliation) is a later, finer
slice. The caret guarantee here is for the live `<input>` that sits *outside*
the regions, which is the common search/filter-as-you-type case.

## Build + view

```bash
cd wasm-crate
wasm-pack build --release --target web
cd ..
python -m http.server 8000   # then open http://localhost:8000/
```
