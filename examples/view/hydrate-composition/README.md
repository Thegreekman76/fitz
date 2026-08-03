# `hydrate-composition` — SSR → client hydration of composition (Phase 11.12 slice 4)

The composition slice of hydration: a root `App` that composes a `Card` with a
`<slot>` now **adopts** the server-painted DOM on boot instead of fresh-mounting
it — across the parent/child boundary.

Slices 1–3 hydrated a single **keep-node** component (a live `@input` over a
static/region template, auto-hydrated). Slice 4 lifts the last restriction: a
**naive composition** tree (`<Child />` + `<slot>`) can hydrate too, behind an
explicit opt-in.

## Opt-in: `component App hydrate`

Hydration for composition is **opt-in** via the `hydrate` marker on the root
component:

```
component App hydrate { ... }
```

The marker propagates to the whole tree, so the composed `Card` hydrates
alongside `App`. Without it, a composition tree fresh-mounts exactly as before —
that keeps every pre-11.12 composition example byte-identical while keep-node
components keep auto-hydrating.

## What it demonstrates

- **Child wrapper adoption** — `App.hydrate` acquires the server-painted
  `<div class="__fitz-child-Card">` from the cursor (no `create_element`) and
  calls `Card.hydrate(wrapper)` instead of `mount_into`.
- **Slot adoption across the boundary** — `Card`'s `<slot>` invokes the parent's
  `__hydrate_slot_0` adopt callback (`__hslot`), which walks the parent-provided
  slot content **in the parent scope** (so `@click="like"` wires to `App`),
  advancing the child's cursor past it.
- **Persistent child state** — the child is get-or-created from `App`'s
  `__child_slot_0` cache, so `Card.taps` survives a later parent re-render.
- **State restore** — `App.title` is restored from the
  `<script type="application/json" id="__flv_state_App">` payload.

## Naive model caveat

Composition has **no in-place patch model**, so hydration here means: adopt +
wire on boot (no flash, server nodes preserved). The **first** state change
re-renders wholesale via the naive `render()` — that is when the restored state
becomes visible (e.g. clicking `like` re-renders and the title stays
`hydrated!`, not the default `default-title`). The child instance (and its
state) is reused from the cache across that rebuild.

## Run it

```
# from this directory
wasm-pack build --release --target web   # into ./wasm-crate
python -m http.server 8000               # serve over HTTP (ES modules need an origin)
# open http://localhost:8000/
```

The smoke test regenerates `wasm-crate/src/lib.rs` from `App.fitzv`
(`cargo test --test view_hydrate_composition_wasm_smoke`); the `#[ignore]` build
test drives a real `wasm-pack build`.

## Authoring the server DOM

`index.html` hand-authors the server-painted markup (there is no isomorphic SSR
render-to-string yet — a later 11.12 unification). It must match the client
build walk exactly: the `__fitz-child-Card` wrapper, the slot content inlined at
the child's `<slot />`, and significant text tight after each element's open tag
(whitespace between elements is skipped by the adopt cursor).
