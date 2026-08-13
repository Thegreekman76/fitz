# bool-attr — Form B / gotcha #6 (conditional boolean attributes)

A `.fitzv` single-file component compiled to WebAssembly that uses
**conditional boolean attributes**: `disabled={expr}` / `checked={expr}`
(unquoted brace) make the attribute present in the DOM **iff `expr` is
truthy** — the HTML boolean-attribute model.

This is distinct from the QUOTED `checked="{expr}"`, which is always present
with a stringified value. The unquoted-brace form is the type-driven, Bool
version; the checker requires `expr` to be `Bool`.

The same syntax targets both backends: the SSR emitter lowers it to a Fitz
`if`-as-expression that yields the bare attribute name or `""`, and the WASM
emitter builds it with `set_attribute` and toggles it reactively with
`set_attribute` / `remove_attribute`.

## Files

| File | Role |
|------|------|
| `App.fitzv` | The `ToggleForm` component: a text `<input>` (`@input`) drives `name`; the Submit button is `disabled={name == ""}` and a checkbox is `checked={name == ""}`. |
| `index.html` | Mount shim — loads the WASM bundle and mounts `ToggleForm` into `#app`. |
| `wasm-crate/` | The generated Rust crate. `Cargo.toml` + `src/lib.rs` are produced by `fitz build --target wasm-client` (and by the smoke test); `pkg/` is the `wasm-pack` output. |

## What it exercises (Form B / gotcha #6)

- **`attr={boolExpr}`** — an unquoted brace after `=` binds a conditional
  boolean attribute. The attribute is present iff `boolExpr` is truthy.
  Covers `checked` / `disabled` / `selected` / `readonly` / `required` / … —
  any HTML boolean attribute — with no whitelist: the syntax + a `Bool`
  requirement in the checker is the gate.
- **Reactive toggle on the WASM target** — this is a keep-node component (it
  has a live `@input` form control over a static template), so a state change
  patches in place. The emitter stashes each bool-attr element and, on
  re-render, `set_attribute(name, "")` when the cond holds or
  `remove_attribute(name)` when it doesn't — the only new web-sys primitive
  the feature needs.
- **Present-iff-truthy on the SSR target** — the same `.fitzv` lowers to
  `if (state.name == "") { "disabled" } else { "" }` in the emitted
  classic-Fitz string.
- **Replaces the `{#if}` two-variant workaround** — before gotcha #6 the only
  way to conditionally set a boolean attribute was emitting both element
  variants: `{#if checked}<input checked/>{#else}<input/>{/if}`.

## Try it

From this directory:

```sh
# Build the WASM bundle (needs `wasm-pack` + the wasm32 target).
cd wasm-crate && wasm-pack build --release --target web && cd ..

# Serve over HTTP (ES modules need an origin) and open the page.
python -m http.server 8000
# → http://localhost:8000/
```

Type into the field: the Submit button enables and the checkbox unchecks as
soon as the field is non-empty; clear it and both come back — the attributes
toggle in place, no subtree rebuild.

The `regenerate_bool_attr_lib_rs` smoke test (in `tests/`) asserts the emit
shape on every `cargo test`; `build_bool_attr_wasm` (`#[ignore]`) does the
real `wasm-pack` build.
