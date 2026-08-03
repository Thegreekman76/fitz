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

Then serve the directory and open `index.html`. The `#app` markup is the
**verbatim SSR output** for `App.fitzv` (with `name = "Ada"`, not the component
default `"world"`) plus the state `<script>`; the wasm hydrates it. See
[Where the server HTML comes from](#where-the-server-html-comes-from-phase-1112-ssr-1).

## What to observe

- The greeting reads **"Hello, Ada"** on first paint (server HTML) and **stays
  "Ada"** after the wasm boots — the state was restored from the script, not
  reset to the default `"world"`.
- `index.html` tags the greeting `<span>` with a JS property *before* calling
  `init()`. After hydration the property is still there → the node was
  **adopted, not recreated**.
- Typing in the input updates the greeting live and keeps the caret (keep-node
  patch over the adopted `<input>`), and **reset** restores `"world"`.

## Where the server HTML comes from (Phase 11.12 SSR-1)

Everything inside `<div id="app">` in `index.html` is **not hand-authored** — it
is the exact output of the SSR emitter for `App.fitzv`, i.e. what a Fitz HTTP
server (or a build-time prerender) emits as
`<div id="app">{App_render(state).raw}</div>`. That includes the `data-flv-*`
attributes (which fitz-liveviews' WS-takeover binds to; they are **inert** to
the wasm adopt walk) and the trailing `<script>` state payload.

The state `<script>` is emitted because the component opts in with the `hydrate`
marker (`component App hydrate { ... }`). The marker is SSR-side only: this
component already auto-hydrates on the wasm target (it is keep-node,
region-free), so the marker adds nothing to the wasm output — its job is to tell
the SSR emitter to append the `<script type="application/json"
id="__flv_state_App">{to_json(state)}</script>` payload. It is opt-in so
components SSR-rendered for the WS-takeover (whose HTML diff forbids `<script>`
in the LiveView root) stay byte-identical.

Reproduce the server HTML with a tiny program that imports `App.fitzv` through
`fitz-liveviews` and prints the render:

```fitz
// src/main.fitz  (with fitz_liveviews as a path dependency)
from fitz_liveviews import flv_register
from App import App, App_render, App_on_name, App_reset

let state = App { name: "Ada" }
print(App_render(state).raw)
```

```sh
fitz run   # prints the exact <div class="demo">…</div> + state <script>
```

## Slice-1 constraints (Phase 11.12)

- Only **keep-node, region-free** components hydrate (a live `@input`/`@change`
  over a static template). `{#if}`/`{#for}` regions and `<Child />`
  composition are later slices; such a component keeps fresh-mount-only
  behaviour.
- Dynamic text interpolations must be the **sole child** of their element
  (`<span>{name}</span>`). Mixed static+interpolated text (`Hello, {name}!`)
  needs comment markers to split the server's merged text node — a later
  slice. That is why the greeting wraps the name in its own `<span>`.
