# `hydrate-regions` — SSR → client hydration of regions + mixed text (Phase 11.12 slice 2)

Slice 1 hydrated a **region-free** keep-node component whose interpolations were
each the **sole child** of their element. Slice 2 lifts both restrictions:

- **Mixed static + interpolated text** — `Hello, {name}!` (no sole-child
  wrapper needed).
- **`{#if}` / `{#for}` regions** — the client adopts the region's server-painted
  anchors instead of rebuilding.

## The server contract

Because a browser fuses adjacent text and gives no place to anchor a region,
the server-painted HTML uses two kinds of comment markers (see `index.html`):

| Shape | Server markup |
| --- | --- |
| Mixed text `Hello, {name}!` | `Hello, <!--fi-->Ada<!--/fi-->!` |
| Region `{#if}…{/if}` / `{#for}…{/for}` | `<!--fr-->…content…<!--/fr-->` |

- The `<!--fi-->` markers force the browser to keep the static (`Hello, ` / `!`)
  and dynamic (`Ada`) runs as **distinct text nodes**. The skip-based adopt
  cursor steps over the comments, so the walk maps 1:1.
- The `<!--fr-->` / `<!--/fr-->` anchors bound each region. On boot the client
  adopts them into the keep-node region handles (`__astart_<r>` / `__aend_<r>`)
  and **leaves the content in place**. A later state change rebuilds only the
  content between the adopted anchors (`__patch_region_<r>`), so the live
  `<input>` outside the region keeps its caret.

(There is no isomorphic SSR string renderer yet — the contract is validated with
hand-authored HTML, as the slice plan intended. A real Fitz SSR emitter that
writes these markers is a later slice.)

## Build

```sh
cd wasm-crate && wasm-pack build --release --target web
```

Then serve the directory and open `index.html`.

## What to observe

- The greeting reads **"Hello, Ada!"** on first paint (mixed text from the
  server) and **stays "Ada"** after the wasm boots (state restored from the
  `<script>`, not reset to the default `"world"`).
- The `{#if}` hint (**"Hiding items equal to: Ada"**) and the `{#for}` list
  (Grace / Hopper) are the server-painted content between the region anchors —
  adopted, not rebuilt, on boot.
- `index.html` tags the `.greeting` and `.items` elements with a JS property
  *before* `init()`. After hydration the properties are still there → the nodes
  were **adopted, not recreated**.
- Typing a name updates the greeting live (keep-node patch over the adopted text
  node — caret preserved) and rebuilds the `{#if}` hint + the `{#for}` list
  (which now hides the matching item). **reset** restores `"world"`.

## Composite state restore (slice 3)

Slice 1/2 restored only primitive scalars; a `List`/`Map`/nominal state field
kept its default. **Slice 3 restores composite state too.** Here the `items`
`List<Str>` server value (`["Ada","Grace","Hopper"]`) differs from the component
default (`["alpha","beta","gamma"]`), so after hydration the client holds the
**server** list. Type `Grace` into the input: the `{#for}` region re-filters the
restored server list → **Ada / Hopper** (never the default alpha/beta/gamma),
proving `items` came from the payload, not the source default.

The restore is recursive: `List<T>`, `Map<Str, V>`, `Nullable<T>`, and imported
nominals (and their nestings) all round-trip through the `<script>` payload. A
field whose JSON doesn't match its type — or a type that can't round-trip
through JSON at all (tuples, functions, a `Map` with a non-`Str` key) — keeps
its default.

## Limitations (Phase 11.12)

- Only **keep-node** components hydrate (a live `@input`/`@change`);
  composition (`<slot>` / `<Child />`) is a later slice.
- Adopt maps by DFS position, so keep the server runs **tight** (no whitespace
  between an element's open tag and its first significant text — see the greeting
  and `<ul>` in `index.html`).
