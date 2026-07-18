# Keyed `<Child />` composition — Phase 11.7.b R2b

The last "rendering core" slice of **Phase 11.7** (client-side
dynamic capabilities). A parent mounts a child inside a `{#for}`
loop, giving each child a **stable identity** with a `key`
attribute so the child — and its local state — survives parent
re-renders.

```fitzv
{#for name in columns}
  <Column key="{name}" title="{name}" />
{/for}
```

Before R2b, a `<Child />` inside a `{#for}` had no way to keep a
stable instance: the parent re-render rebuilt the DOM and the child
would be reset. R2b adds a **keyed instance cache** — one child
instance per key (`HashMap<String, Rc<Column>>`) — plus a
reconciliation sweep that evicts any key that vanished from the list.

## How the keyed cache works

Fitz's `.fitzv` → WASM backend uses a **dirty-flag** reactivity
model (§9.m D1 of `docs/fase-11-plan.md`): a state mutation
re-renders the whole component subtree from the current state.

The `key` attribute is what makes that survivable for dynamic
children. On each render of the loop:

1. The key expression is evaluated (`format!("{}", name)`), giving
   a stable `String` identity per item.
2. The child is fetched from the site's keyed cache with
   `map.entry(key).or_insert_with(|| Column::new())` — a new key
   creates an instance, an existing key **reuses** it (state
   intact).
3. Every touched key is recorded in a per-render `__seen` set.
4. After the loop, a `retain` evicts every cached child whose key
   was **not** seen this render — so items removed from the list
   release their instance (reconciliation).

Because the same key always maps to the same cached `Column`, each
column's `taps` counter survives a parent re-render.

## Try it in the browser

1. Tap a column's *taps* button a few times — its own counter goes
   up. Each `<Column />` holds its **own** `taps` state.
2. Click *re-render parent* — the parent's `bumps` changes, so
   `App` re-renders and rebuilds the `{#for}` loop.
3. Every column's `taps` is **still there** — the keyed cache reused
   each instance instead of resetting it.

## Known R2b limitation → R3

The list here is **constant** (`["To Do", "In Progress", "Done"]`):
event bodies that push/remove list items are a separate 11.4.c debt,
so reconciliation runs but never evicts live. And `{#for}` over a
`List<nominal>` (e.g. `List<Card>`, what the kanban really needs) is
blocked on nominal-type support in the WASM target — that is the
**R3** prereq. See `docs/deudas-post-5b.md`.

## Structure

```
examples/view/keyed-composition/
├── App.fitzv               # parent App + child Column
├── index.html              # mount point + <script type="module">
├── README.md               # this file
└── wasm-crate/             # Rust crate that wraps the emitted code
    ├── Cargo.toml          # AUTO-GENERATED (compose_cargo_toml)
    ├── .gitignore          # target/, pkg/, Cargo.lock
    └── src/
        └── lib.rs          # AUTO-GENERATED — do NOT edit by hand
```

`wasm-crate/src/lib.rs` + `Cargo.toml` are regenerated from
`App.fitzv` by
`tests/view_keyed_composition_wasm_smoke.rs::regenerate_keyed_composition_lib_rs`,
which runs on every `cargo test`. They are committed in-tree so a
fresh clone can `wasm-pack build` without running the smoke first.

## Building + running

Prerequisites (install once per machine):

```
rustup target add wasm32-unknown-unknown
cargo install wasm-pack
```

Then from this directory:

```
cd wasm-crate
wasm-pack build --release --target web
cd ..
python -m http.server 8000
```

Open <http://localhost:8000/>.

## Building the bundle via the test harness

```
cargo test --test view_keyed_composition_wasm_smoke build_keyed_composition_wasm -- --ignored
```

Runs the regeneration + `wasm-pack build` and prints the bundle
size. Marked `#[ignore]` because it needs `wasm-pack` +
`wasm32-unknown-unknown` on the runner.
