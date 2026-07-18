# Control-flow directives — Phase 11.7.b

`{#if}` / `{#else}` and `{#for}` compiled to WebAssembly. Before
11.7.b the client-WASM target rejected both (deferred at 11.4.c,
never implemented — the counter/showcase demos never used them).

```fitzv
{#if count > 0}
  <p>clicked at least once</p>
{#else}
  <p>click to begin</p>
{/if}

<ul>
  {#for label in labels}
    <li>{label}</li>
  {/for}
</ul>
```

## What's supported (Phase 11.7.b subset)

- **`{#if cond}`** where `cond` is a bool state field / loop var, a
  numeric comparison (`>`/`<`/`==`/`!=`/`<=`/`>=`), or `&&` / `||` /
  `!` over those. `{#else}` optional. Evaluated at render time under
  the dirty-flag model.
- **`{#for x in <field>}`** where `<field>` is a state field of type
  `List<Int|Float|Str|Bool>`. The state `Vec` is snapshotted and
  iterated by value; `x` is a loop variable in scope for the
  children (usable in `{x}`).

## Deferred (clear pointers in the emitter)

- **`{#for}` over `List<nominal>`** (e.g. `List<Card>`, needed by the
  kanban) — waits on nominal-type support in the WASM target
  (Phase 11.7 R3 prereq).
- **`<Child />` composition inside `{#for}`** (keyed dynamic
  children) — R2b (Phase 11.7.b continuation) with an explicit
  `key` attribute for stable identity.
- **List mutation in event bodies** (`labels.push(...)`) — a
  separate 11.4.c debt (event bodies stay numeric today), so
  `labels` is constant in this demo.

## Structure

```
examples/view/control-flow/
├── App.fitzv               # the source component
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
`tests/view_control_flow_wasm_smoke.rs::regenerate_control_flow_lib_rs`,
which runs on every `cargo test`.

## Building + running

```
rustup target add wasm32-unknown-unknown   # once per machine
cargo install wasm-pack                     # once per machine

cd wasm-crate
wasm-pack build --release --target web
cd ..
python -m http.server 8000
```

Open <http://localhost:8000/>. Click the button — the conditional
message flips and the list renders from the loop.

## Building the bundle via the test harness

```
cargo test --test view_control_flow_wasm_smoke build_control_flow_wasm -- --ignored
```
