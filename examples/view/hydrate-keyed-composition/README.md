# `hydrate-keyed-composition` — SSR → client hydration of dynamic keyed composition (v0.48.0)

The last composition slice of hydration: a `<Child key="{...}" />` **inside a
`{#for}`** now **adopts** the server-painted DOM on boot, reconciling each item
through its keyed instance cache — instead of leaving the loop dead until the
first re-render.

Slice 4 (v0.30.4) hydrated **static** composition — a `<Child />` at a fixed
site (`__child_slot_<n>`). This lifts the last restriction: **dynamic** keyed
composition (`<Child key=... />` inside `{#for}`, backed by `__child_map_<n>`)
hydrates too.

## Opt-in: `component App hydrate`

Same opt-in as static composition — the `hydrate` marker on the root propagates
to the whole tree:

```
component App hydrate { ... }
```

Without it, the tree fresh-mounts exactly as before — every pre-v0.48.0
composition example (including the build-only `keyed-composition`) stays
byte-identical.

## What it demonstrates

- **Per-item wrapper adoption** — the server paints the loop as
  `<!--fr--><div class="__fitz-child-Column">…</div>×N<!--/fr-->`. `App.hydrate`
  consumes `<!--fr-->`, then adopts one wrapper per list item from the cursor
  (`__flv_next_element`, no `create_element`) and calls `Column.hydrate(wrapper)`
  instead of `mount_into`.
- **Keyed reconciliation, reused** — each adopted item flows through the exact
  build-walk machinery: `__key`, `__seen_0.insert`, get-or-create from
  `self.__child_map_0`, and the post-loop `retain` sweep. Item `i` ↔ wrapper `i`
  (same order the server serialized the list).
- **Persistent child state** — because each key maps to the same cached
  `Column`, a column's `taps` survives a later parent re-render (`bump`), even
  though the naive re-render rebuilds the whole root.
- **State restore** — `App.columns` is restored from the
  `<script type="application/json" id="__flv_state_App">` payload
  (`["To Do","In Progress","Done"]`), which differs from the component default
  (`["placeholder"]`) — so the 3 server columns must persist after the first
  `bump`, not collapse to the default.

## Naive model caveat

Naive composition has no in-place patch: any state change re-renders the whole
root. So the `__hydrationWitness` tagged on a server-painted node in `index.html`
survives the **first paint** (the adoption proof) but disappears after the first
`bump`. That's expected — the witness proves adoption; the persisted `taps`
prove the keyed instance cache. They're independent properties.

## Try it

```
cargo test --test view_hydrate_keyed_composition_wasm_smoke build_hydrate_keyed_composition_wasm -- --ignored --nocapture
python -m http.server -d examples/view/hydrate-keyed-composition
```

Open the served page, then in the console:

1. `document.querySelector('#app .columns > .__fitz-child-Column:nth-child(2) .tt').__hydrationWitness` → `"server-col-2"` (the dynamic wrapper was adopted, not recreated).
2. Tap column 1's button 3×, column 3's button 1× (each keeps its own `taps`).
3. Click **re-render parent** — the root rebuilds, but each column's `taps` persists (keyed instance cache).
