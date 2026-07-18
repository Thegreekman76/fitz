# Reactive interpolated props — Phase 11.7.a

The first client-WASM slice of **Phase 11.7** (client-side dynamic
capabilities). A parent component passes its own state fields down
to a child as **interpolated** props, and those props track the
parent's state live.

```fitzv
<Badge heading="{title}" count="{clicks + 1}" />
```

Before 11.7.a the WASM emitter rejected interpolated props with a
"Phase 11.7+" pointer (only static string props were allowed —
see `examples/view/showcase/`). Now the WASM path accepts them for
the **simple case**: a bare parent state field or numeric
arithmetic over parent state, into a primitive child field.

## How the reactivity works

Fitz's `.fitzv` → WASM backend uses a **dirty-flag** reactivity
model (§9.m D1 of `docs/fase-11-plan.md`): a state mutation
re-renders the whole component subtree from the current state.
When you click *bump the parent*:

1. `App::bump()` mutates `clicks`.
2. `App::render()` fires — it recomputes the child's props from the
   parent's current state (`count = clicks + 1`) and re-mounts the
   `<Badge />`.
3. The child re-renders with the fresh `count`.

Reactive propagation falls out of the naive re-render for free —
no signals, no VDOM diffing.

## Known R1 limitation → R2

Because the parent re-render **recreates** the child (there is no
keyed instance cache yet), a child with its own local state would
see that state reset on every parent re-render. That is why
`Badge` here is a pure display component with no local state.

Persistent child state across parent re-render — the keyed
instance cache — is the **R2** work (Phase 11.7.e). See
`docs/deudas-post-5b.md` for the tracking.

## Structure

```
examples/view/reactive-props/
├── App.fitzv               # parent App + child Badge
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
`tests/view_reactive_props_wasm_smoke.rs::regenerate_reactive_props_lib_rs`,
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

Open <http://localhost:8000/>. You should see:

1. The `App` widget with a heading and a `<Badge />` reading
   `parent bumps + 1 = 1`.
2. Clicking *bump the parent* increments the badge's count live.

## Building the bundle via the test harness

```
cargo test --test view_reactive_props_wasm_smoke build_reactive_props_wasm -- --ignored
```

Runs the regeneration + `wasm-pack build` and prints the bundle
size. Marked `#[ignore]` because it needs `wasm-pack` +
`wasm32-unknown-unknown` on the runner.
