# derived — computed values (`derived { ... }`) (Phase 11.10 slice 4)

A `.fitzv` single-file component with a `derived { ... }` block: read-only
values computed from state (and from earlier derived), referenced like state.

## What it exercises (new in slice 4)

```fitz
state {
  first: Str = "Ada"
  last: Str = "Lovelace"
}
derived {
  full: Str = "{first} {last}"      // reads state
  greeting: Str = "Hello, {full}!"  // derived-of-derived
}
```

- **`derived { name: T = expr }`** — a sibling block to `state`. Same
  `name: T = expr` shape, but read-only and recomputed from its inputs.
- **Referenced like state** — `{full}` in the template (and in event
  bodies) reads the derived value; no method-call syntax.
- **Derived-of-derived** — `greeting` reads `full`, declared above it.
  Derived are computed in declaration order.
- **Recomputed each render/patch** — the compiler caches each derived in a
  `RefCell` cell refreshed by `__recompute_derived()` before every render.
  Since this is a value-input component over a static template, it runs on
  keep-node: the inputs are patched in place (caret preserved) while the
  derived text updates.

## Limitations (this slice)

- **Primitive types only** — `Str` / `Int` / `Float` / `Bool` derived.
  Compound derived (`List<T>` / `Map` / nominal) defer to a later slice.
- **Read-only** — don't assign to a derived in an event handler.
- **Recomputed each render, not on-dependency-change** — the fine-grained
  "recompute only when a dep changes" + async derived are a later slice.
- **Client-WASM target** — the SSR emitter doesn't lower `derived` yet.

## Build + view

```bash
cd wasm-crate
wasm-pack build --release --target web
cd ..
python -m http.server 8000   # then open http://localhost:8000/
```
