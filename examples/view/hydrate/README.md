# `hydrate` — SSR → client hydration (Phase 11.12)

A `.fitzv` component that renders on the server (first paint, SEO, works
without JS) and whose client-WASM runtime then **adopts** the server-painted
DOM instead of re-creating it.

## What hydration does here

`App.fitzv` compiles to a wasm crate whose generated `start()`:

1. Resolves the mount root (`#app`).
2. If the root **already has server-painted DOM** → calls `App::hydrate(root)`:
   - restores the serialized state from
     `<script type="application/json" id="__flv_state_App">`,
   - walks the existing DOM mapping each dynamic point onto the keep-node
     handles (`__ktext_*` / `__kattr_*`) via the `__flv_next_element` /
     `__flv_next_text` cursor helpers — **no wipe, no `create_element`**,
   - wires the `@input` / `@click` listeners onto the adopted elements,
   - marks the component built so later state changes patch in place.
3. If the root is **empty** → falls back to a fresh client `mount()` (the
   pre-11.12 behaviour), so the same bundle still works as a standalone SPA.

## Build

```sh
fitz build --bin hydrate --target wasm-client   # from a project, or:
cd wasm-crate && wasm-pack build --release --target web
```

Then serve the directory and open `index.html`. The page ships the
server-painted DOM (with `name = "Ada"`, not the component default `"world"`)
plus the state `<script>`; the wasm hydrates it.

## What to observe

- The greeting reads **"Hello, Ada"** on first paint (server HTML) and **stays
  "Ada"** after the wasm boots — the state was restored from the script, not
  reset to the default `"world"`.
- `index.html` tags the greeting `<span>` with a JS property *before* calling
  `init()`. After hydration the property is still there → the node was
  **adopted, not recreated**.
- Typing in the input updates the greeting live and keeps the caret (keep-node
  patch over the adopted `<input>`), and **reset** restores `"world"`.

## Slice-1 constraints (Phase 11.12)

- Only **keep-node, region-free** components hydrate (a live `@input`/`@change`
  over a static template). `{#if}`/`{#for}` regions and `<Child />`
  composition are later slices; such a component keeps fresh-mount-only
  behaviour.
- Dynamic text interpolations must be the **sole child** of their element
  (`<span>{name}</span>`). Mixed static+interpolated text (`Hello, {name}!`)
  needs comment markers to split the server's merged text node — a later
  slice. That is why the greeting wraps the name in its own `<span>`.
