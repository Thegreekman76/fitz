# `hydrate-mixed` — SSR → client hydration of mixed text (Phase 11.12 SSR-2)

A `.fitzv` component whose greeting interpolates in a **mixed run**
(`Hello, {name}!`) rather than the sole-child wrapper the slice-1 `hydrate`
example used (`Hello, <span class="nm">{name}</span>`). The server renders it
hydration-ready and the client-WASM runtime **adopts** the server-painted DOM.

## Why mixed text needs markers

The browser coalesces adjacent text runs into a **single** text node, but the
client adopt walk expects one text node per significant run — it calls
`__flv_next_text` once for `Hello, `, once for `{name}` (adopting it into the
keep handle), and once for `!`. So the SSR emitter separates the dynamic run
with comment markers:

```html
<p class="greeting">Hello, <!--fi-->Ada<!--/fi-->!</p>
```

The comments break the browser's coalescing → three distinct text nodes. The
skip-based `__flv_next_text` steps over the `<!--fi-->` / `<!--/fi-->` markers
and maps 1:1 onto the build/adopt walk. No hand-authored markers: the whole
`index.html` `#app` block is the verbatim SSR render output.

## Build

```sh
fitz build --bin hydrate-mixed --target wasm-client   # from a project, or:
cd wasm-crate && wasm-pack build --release --target web
```

Then serve the directory and open `index.html`.

## What to observe

- The greeting reads **"Hello, Ada!"** on first paint (server HTML) and **stays
  "Ada"** after the wasm boots — state restored from the `<script>` payload, not
  reset to the component default `"world"`.
- `index.html` tags the greeting `<p>` with a JS property *before* calling
  `init()`. After hydration the property is still there → the node (and its
  mixed-text children) was **adopted, not recreated**.
- Typing in the input updates the greeting live and keeps the caret (keep-node
  patch over the adopted text node), and **reset** restores `"world"`.

## Where the server HTML comes from (SSR-2)

Everything inside `<div id="app">` is the exact output of the SSR emitter for
`App.fitzv`, i.e. what a Fitz HTTP server (or a build-time prerender) emits as
`<div id="app">{App_render(state).raw}</div>`. The `<!--fi-->` markers around the
`{name}` run and the trailing `<script>` state payload both come straight from
the emitter — both gated by the `hydrate` marker
(`component App hydrate { ... }`), which is SSR-side only (the component already
auto-hydrates on the wasm target because it is keep-node, region-free).

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

## What SSR-2 adds over slice 1

Slice 1 (`hydrate`) required dynamic interpolations to be the **sole child** of
their element. SSR-2 lifts that: mixed static+interpolated text renders with
`<!--fi-->` markers so it hydrates directly. `{#if}`/`{#for}` regions (SSR-3) and
`<Child />` composition (SSR-4) are later slices; interpolations inside a region
are adopted opaquely (between the region's `<!--fr-->` anchors) and therefore
carry no `fi` markers.
