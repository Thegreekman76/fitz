# Fitz view showcase — multi-component composition (Phase 11.5.e)

The largest `.fitzv` fixture the current Phase 11.5.d
composition subset permits: a `Board` root component that mounts
three `<MetricCard title="X" value="N" trend="Y" />` children
with static props. Demonstrates the pieces 11.5.d wired end-to-
end:

- **Multiple components in one file** — `Board` + `MetricCard`,
  each with its own state / event / template / scoped style.
- **`<Child />` composition** — the parent mounts child instances
  by writing capitalised tags in its template.
- **Static prop coercion** — each attribute value gets coerced to
  the child's declared state-field type (`Str` / `Int` here).
- **Per-child internal state** — every card has its own `clicks`
  counter that updates when the button is tapped, independent of
  the parent.
- **Scoped styles per component** — each component's
  `<style scoped>` block generates a hashed class suffix, so
  `.card` in `MetricCard` doesn't collide with `.card` elsewhere.

## Contract of the fixture

The `.fitzv` source (`Dashboard.fitzv`) respects the 11.5.d
subset:

- Every child prop is a static string value in the parent's
  template.
- Every prop type on the child is a primitive scalar (`Str` /
  `Int` / `Float` / `Bool`) or `Nullable<T>` of a primitive.
- `<MetricCard />` is always self-closing (no fallback children
  via `<slot>`).

Anything outside that subset is rejected at expand time with a
targeted 11.6+ pointer. Concretely: dynamic props
(`prop={expr}`), event attributes on children
(`<Card @click="handler" />`), and fallback children
(`<Card>...</Card>`) all reject with actionable messages.

## Build the WASM bundle

The `wasm-crate/` scaffold is committed alongside the fixture,
so a fresh clone can `wasm-pack build` without running the smoke
harness first. To regenerate the scaffold after editing
`Dashboard.fitzv`:

```
cargo test --test view_showcase_wasm_smoke regenerate_showcase_lib_rs
```

That rewrites `wasm-crate/src/lib.rs` and `wasm-crate/Cargo.toml`
from the current emitter output.

To build the bundle:

```
cd examples/view/showcase/wasm-crate
wasm-pack build --release --target web
```

Or run the opt-in smoke that does both in one shot:

```
cargo test --test view_showcase_wasm_smoke build_showcase_wasm -- --ignored
```

Requires `wasm-pack` + the `wasm32-unknown-unknown` rustc target
installed (see the counter demo's README for the setup recipe).

## Serve locally

`index.html` uses ES modules, so it needs an HTTP origin — a
plain double-click over `file://` won't work.

```
cd examples/view/showcase
python -m http.server 8000
# open http://localhost:8000/
```

You should see three metric cards laid out in a 3-column grid.
Click any card's "tapped N times" button; the number goes up
independently per card.

## Compile-time note (per `docs/fase-11-plan.md` §6 row 11.5.e)

The view pipeline (parse → expand → check → compose_lib_rs) is
trivial in wall-clock terms — well under 100ms for this fixture
on any modern machine. The wall-clock number a user cares about
is the `wasm-pack build --release --target web` step, which is
`cargo build --release` under the hood; that's typically 20-40s
on a cold cache (the release profile does LTO + wasm-opt). No
hard assertion in the smoke — see §9.t of the plan for the
re-baselining rationale.

## What this fixture does NOT demonstrate

All of these are Phase 11.6+ work (documented in `docs/fase-11-plan.md`
§9.s Debt residual):

- **Dynamic child props** (`<Card title={data.title} />`) —
  would let the fixture iterate over a runtime list of card
  configs, which is what a "real" dashboard would want.
- **Event bubbling from children** (`<Card @select="handler" />`) —
  would let the parent react to child interactions.
- **`{#for}` with children as body** — `{#for c in cards} <Card
  title={c.title} /> {/for}` needs dynamic props, so it hits
  the same block as the item above.
- **Fallback children via `<slot>`** (`<Card>fallback</Card>`) —
  needs slot fill-in wiring.
- **Cross-file composition** — importing a component from a
  sibling `.fitzv`. Today, all components live in the same file.
- **Persistent child state across parent re-renders** — naive
  render clears the root and rebuilds, which re-instantiates
  children. A position-keyed component-instance cache would
  preserve child state.

Once 11.6+ lands, the `fitz-liveviews/examples/kanban/` port
that §6 row 11.5 originally scoped becomes viable.
