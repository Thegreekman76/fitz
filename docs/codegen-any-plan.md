# Codegen `Any` support via `__FitzValue` — design plan

**Status**: **steps 1-6 IMPLEMENTED + validated (2026-07-20)**. The `Any`
codegen support is done and regression-clean (see §8). Step 7 (acceptance) is
partial: a fitz-liveviews app now gets *past* every `Any` error, but building
one to a full binary surfaced a chain of pre-existing WS/view build gaps —
four cleared, one (cross-module type identity for dependency-imported fn
return types) deferred as its own effort (§8).

**Goal**: make `fitz build` (codegen → native binary) support programs that
use the `Any` type in the specific shape that dynamic *registries* need —
so that **fitz-liveviews apps compile to native binaries** (they only run
under `fitz run` today). This is a general codegen capability, not coupled to
fitz-liveviews.

---

## 1. Why — and what is (and isn't) the problem

`fitz build` on any program whose inferred types include a bare `Type::Any`
fails with:

```
codegen 5b does not support the type `Any`
```

`Any` "just works" in the interpreter (the runtime `Value` enum is already a
dynamic union). Codegen has no bare-`Any` representation.

**Empirically re-scoped (2026-07-20).** Two things that looked like the hard
blockers already compile in v0.26.0:

| Micro-test | `fitz build` |
|---|---|
| Recursive struct (`type Node { children: List<Node> }`) | ✓ compiles |
| Higher-order with a *typed* fn value (`Map<Str, Function>` → retrieve → call) | ✓ compiles |
| Fn stored as **`Any`** (`render_fn: Any`) → retrieve → call | ✗ `Any` error |

So higher-order functions and recursive structs are **done**. The *only*
blocker is bare `Type::Any`.

**What the target actually needs from `Any`** (from an audit of
`fitz-liveviews/src/lib.fitz` — all `Any` is in ~140 lines of the component
registry/dispatch; the HTML parser + diff engine are 100% statically typed):

- **Easy** — pass-through: store an `Any`, return it, pass it as an argument,
  compare it to `null`.
- **Hard (two things)**:
  1. Hold an arbitrary **user struct** in `Any`, then pass it to a function
     that expects that struct's concrete type. (The per-instance component
     *state*: `Map<Str, Any>` store, `initial_state: Any`.)
  2. Hold a **function** in `Any` (`event_handlers: Map<Str, Any>`,
     `render_fn: Any`) and invoke it dynamically by runtime key
     (`render_fn(state)`, `handler(state, payload)`). Signatures are
     heterogeneous across components; all are top-level named `fn`s (no
     captured environment).

The simplest possible fitz-liveviews component (`MetricTile`, state
`{ count: Int }`) already exercises *all* of the above — there is no smaller
subset. The MVP is the full mechanism.

---

## 2. Current state (facts, `src/codegen.rs`)

- **`__FitzValue`** — the tagged runtime enum codegen already emits for ORM
  JSONB / heterogeneous literals (gated by `uses_fitz_value`). Variants:
  `Int, Float, Str, Bool, Null, Bytes, Nominal(String), List(Vec<_>),
  Map(Vec<(_,_)>)`. Has `Display`, `PartialEq`, `__fv_type_name`, and (under
  `has_http`) JSON impls. **`Nominal(String)` stores only the instance's
  Display string** — a struct in `__FitzValue` today is *not recoverable* to
  typed field access. No `Function` variant.
- **`rust_type_for(Type::Any)`** → error (catch-all ~35133). `Any` *nested*
  in a container is special-cased: `List<Any>` → `Arc<Mutex<Vec<__FitzValue>>>`,
  `Map<_,Any>` → `Arc<Mutex<Vec<(__FitzValue,__FitzValue)>>>`. No uniform
  bare-`Any` → `__FitzValue` mapping.
- **`coerce`** — no `Any` arms at all (silent passthrough → rustc mismatch).
- **`wrap_as_fitz_value(code, ty)`** (concrete → `__FitzValue`): Nominal →
  `Nominal(format!("{}", &*code.lock().unwrap()))` (Display only); Function /
  Tuple / Future → error.
- **Only methods callable on `Any`**: `as_int/as_float/as_str/as_bool/as_bytes/
  type_name` (dynamic downcast arms).
- **Function value repr**: `Type::Function{params,ret}` →
  `Arc<dyn Fn(P..) -> R + Send + Sync>` (concrete signature). Calling a fn
  value goes through `gen_call_with_sig` and **requires the concrete
  signature** — which is why a homogeneous `Map<Str, Function>` works but a
  heterogeneous `Map<Str, Any>` cannot.
- **Struct instance repr**: `type Foo = Arc<Mutex<FooData>>` (post-F17,
  Send+Sync, 'static).

---

## 3. Design

Represent bare `Type::Any` as `__FitzValue`, and extend `__FitzValue` with
the two things the registry pattern needs: a **recoverable struct** and a
**type-erased callable**.

### 3.1 New `__FitzValue` variants (additive)

```rust
enum __FitzValue {
    // ... existing variants unchanged ...
    // Recoverable instance: the actual Arc<Mutex<FooData>> boxed as `Any`,
    // plus its Display for formatting/equality. Additive — the existing
    // `Nominal(String)` stays for the ORM JSONB Display-only path.
    Instance(std::sync::Arc<dyn std::any::Any + Send + Sync>, String),
    // Type-erased callable. Args and return marshalled through __FitzValue.
    Function(std::sync::Arc<dyn Fn(Vec<__FitzValue>) -> __FitzValue + Send + Sync>),
}
```

- `Display`: `Instance(_, repr)` → `repr`; `Function(_)` → `"<function>"`.
- `PartialEq`: `Instance` compares by `repr` string (consistent with today's
  `Nominal` behavior); `Function` → always `false` (functions aren't
  comparable — parallels `field_eq_expr`).
- `__fv_type_name`: `Instance` → `"Instance"`, `Function` → `"Function"`.
- JSON impls (`has_http`): `Instance` serialises via its typed data is out of
  MVP scope (the framework never JSON-serialises an `Any`); emit a safe
  fallback (`Function`/`Instance` → `null` in `__ToFitzJson`) and document.

**Aliasing is preserved**: `Instance` boxes the `Arc<Mutex<FooData>>`, so the
inner shared cell survives the round-trip; a downcast recovers the *same*
`Arc<Mutex<FooData>>`.

### 3.2 `Type::Any` → `__FitzValue`

- `rust_type_for(Type::Any)` returns `"__FitzValue"`.
- Extend the `uses_fitz_value` detector (`program_uses_fitz_value`) to also
  fire on any `Any`-typed binding / param / return / struct field (today it
  only detects heterogeneous literals + `.group_by`). Without this the prelude
  isn't emitted.

### 3.3 `coerce` — the Any boundary

- `concrete → Any` (wrap): reuse/extend `wrap_as_fitz_value`:
  - primitives / List / Map: as today.
  - **Nominal(T)** → `__FitzValue::Instance(Arc::new(code.clone()) as Arc<dyn Any + Send + Sync>, format!("{}", &*code.lock().unwrap()))`.
  - **Function** → generate a marshalling adapter (§3.4).
- `Any → concrete` (unwrap):
  - primitive: the existing `as_int/...` downcast helpers.
  - **Nominal(T)** → downcast: `match v { __FitzValue::Instance(d, _) => d.downcast::<Arc<Mutex<TData>>>().expect(...).as_ref().clone(), _ => panic!(...) }` (helper fn emitted per needed target type, or a generic `__fv_as_instance::<T>()`).
  - `Any → Any`: identity.

### 3.4 Marshalling adapter (the crux)

At a `fn → Any` coercion site codegen **knows the fn's concrete signature**
`fn(P1..Pn) -> R`. Emit:

```rust
__FitzValue::Function(std::sync::Arc::new(|__a: Vec<__FitzValue>| -> __FitzValue {
    let __p0 = <coerce __a[0]: Any → P1>;
    // ...
    let __r = the_fn(__p0, ...);
    <coerce __r: R → Any>
}))
```

- Arg count is fixed per adapter (the fn's arity); a runtime length check
  guards misuse.
- Uses the same `coerce` Any↔concrete from §3.3, so the two pieces compose.
- **Sync only for MVP**: fitz-liveviews render fns return `Html` and event
  handlers return the state struct — both sync. `async` fn values inside
  `Any` are follow-up debt (documented).

### 3.5 Calling an `Any` value as a function

`f(a0, a1)` where `f: Any`:

```rust
match &f { __FitzValue::Function(__g) => __g(vec![<wrap a0>, <wrap a1>]), _ => panic!("value is not callable") }
```

returning `__FitzValue`, then `coerce`d to the call context's expected type.
Wire this into `gen_call` where the callee's static type is `Any` (today that
path errors in `rust_type_for`).

### 3.6 One narrow field-access-on-Any case → solve with an annotation

The framework does `let inner = render_fn(state); ... inner.raw` — `inner` is
`Any` (result of calling an `Any`). Rather than implement general
field-access-on-`Any`, annotate on the **framework** side:
`let inner: Html = render_fn(state)` → `coerce(Any → Html)` downcast, then
`.raw` is static. A 1-line change in `fitz-liveviews/src/lib.fitz`. General
field-access-on-`Any` stays out of scope (the framework doesn't otherwise
need it).

---

## 4. Sub-steps (incremental, each independently testable)

1. **`__FitzValue` variants + impls** — add `Instance`/`Function` + Display /
   PartialEq / type_name / JSON fallback. No behavior change; prelude only.
2. **`rust_type_for(Any) → __FitzValue`** + extend `program_uses_fitz_value`
   detector. Unblocks bare-`Any` signatures/bindings (pass-through).
3. **`coerce` Any↔primitive + `Any == null`** — the easy path.
4. **`coerce` Nominal↔Any** — wrap (Instance) + unwrap (downcast helper).
5. **Function→Any adapter** (§3.4) + **call-Any-as-fn** (§3.5) + `gen_call`
   wiring.
6. **fitz-liveviews annotations** (separate repo) — `let inner: Html = ...`
   and any sibling spot.
7. **Acceptance**: `examples/dashboard` (fitz-liveviews) `fitz build`
   succeeds + runs bit-for-bit vs `fitz run`; then the admin showcase builds
   + `docker compose up`.

Steps 1-5 are fitz-core; 6 is fitz-liveviews (minimal); 7 is validation.

## 5. Risks / open questions

- **`__FitzValue` is shared with the ORM/JSONB path.** Mitigation: additive
  variants only; `Nominal(String)` untouched. Full re-run of the DB / ORM
  test suite required.
- **Downcast bounds**: boxing `Arc<Mutex<FooData>>` as `Arc<dyn Any + Send +
  Sync>` requires `FooData: 'static + Send + Sync` — true post-F17. Verify no
  type carries a non-'static field.
- **`PartialEq` for `Instance` by `repr`** — matches current Nominal
  semantics but is Display-based, not structural. Acceptable; document.
- **Async fn values in `Any`** — not needed for the framework MVP; error
  clearly + document as debt.
- **Detector completeness** — if `program_uses_fitz_value` misses an
  `Any`-typed site, the prelude won't emit and rustc will fail with a missing
  `__FitzValue`. Needs thorough walker coverage (params, returns, fields,
  lets, generic args).
- **Interpreter parity** — `fitz run` already handles all of this; the bar is
  bit-for-bit identical output from the built binary on the acceptance
  examples.

## 6. Effort

Substantial but **bounded** — an extension of existing machinery
(`__FitzValue`, `coerce`, `wrap_as_fitz_value`, the function-value repr), not
a new dynamic runtime. Estimate: a focused multi-day fitz-core effort, its own
sub-phase with the full workflow (lib + db + compile_e2e suites, fmt/clippy,
guide/CHANGELOG/roadmap, VSCode extension unaffected). The design risk is
concentrated in steps 4-5 (typed-struct-in-Any + type-erased call with
marshalling under `Send + Sync`).

## 7. Alternatives considered (rejected)

- **Compile the LiveViews registry to static dispatch from `@live_component`
  decorators** — the compiler knows every component statically, so it could
  emit a static `match` instead of a runtime `Map<Str, Any>`. Rejected:
  couples fitz-core to fitz-liveviews internals; speculative.
- **Redesign fitz-liveviews to avoid `Any`** (stringly-typed / serialized
  state) — rejected: loses the typed-struct ergonomics of the `.fitzv` model.
- **Leave fitz-liveviews `fitz run`-only** — rejected by the author; the
  native-binary + Docker story is wanted.

---

## 8. Status (implemented 2026-07-20)

### Delivered in fitz-core (`src/codegen.rs`, `src/types.rs`)

Steps 1-5 landed exactly as designed:

1. **`__FitzValue::Instance` + `Function` variants** + `Debug` (manual, since
   `Arc<dyn Any>`/`Arc<dyn Fn>` aren't `derive(Debug)`) + `Display` /
   `PartialEq` / `__fv_type_name` arms + JSON/DB-helper fallbacks (`→ null`).
2. **`rust_type_for(Type::Any) → "__FitzValue"`**. The
   `program_uses_fitz_value` detector already fired on explicit `Any`
   annotations (`type_expr_has_any` over fields/params/returns/lets) — no
   change needed.
3. **`coerce` Any↔primitive** + prelude unwrap helpers
   `__fv_to_i64/f64/string/bool`. `Any → Nullable` composes via the existing
   Nullable arm; `Any == null` works through the wrap + derived `PartialEq`.
4. **`coerce` Nominal↔Any** via a new `wrap_any_value` (Nominal →
   `Instance` boxing the `Arc<Mutex<TData>>` unsized to `Arc<dyn Any>`;
   unwrap → `downcast::<Mutex<TData>>()`). Also `Any → List`/`Any → Map`
   compound unwrap (needed for event-handler payloads).
5. **`wrap_fn_as_any`** (the marshalling adapter) + **call-Any-as-fn** dynamic
   dispatch wired into `gen_call` (callee whose `lookup_var` type is `Any`).
   Literal/`Map<Str,Any>` value wrapping routes Functions through the adapter
   via `wrap_as_fitz_value_with_env` (kept Nominal→Display there so the F13.B
   heterogeneous-literal / JSONB behavior is unchanged) + a map-index key-wrap
   fix for heterogeneous storage.

**Validation**: `let a: Any = 42` round-trip, `type P; let a: Any = P{..};
p.count` (struct-in-Any recovered by downcast), fn-in-`Any` invoked, and the
full fitz-liveviews-style dispatch (`render_fn(state)` + `handler(state,
payload)` through a `Map<Str, Any>`, state as struct-in-Any, `Map<Str,Str>`
payload, unwrap of the returned state) all `fitz build` + run correctly.
`cargo test --lib` **3841 / 0** (ORM/JSONB untouched). fmt + clippy
(`--lib --tests --bins -D warnings`) clean.

Step 6 (framework annotation `let inner: Html = render_fn(state)`) applied to
`fitz-liveviews/src/lib.fitz`.

### Downstream WS/view build gaps (cleared — pre-existing, exposed because no
fitz-liveviews app had ever reached `fitz build`)

- **`@test` fns skipped in imported modules** (codegen only skipped them in
  the main program; `lib.fitz` ships `@test` suites).
- **`@render_for` accepts a nominal `Html`** return, not just `Str` (the
  checker comment already anticipated this).
- **`?` inside `@ws` handlers**: emit the handler `-> Result<(), String>` and
  push a Result frame on `ret_stack` so the native Rust `?` compiles — gated
  on `body_has_try` so `?`-free WS handlers keep their plain `()` return.

### Deferred (its own effort)

- **Cross-module type identity for dependency-imported fn return types.**
  `component()` from the `fitz_liveviews` dependency returns `Html`; its
  origin-module `TypeId` collides with a local type's id in the importer, so
  codegen mis-types the call result. `remap_imported_nominals` exists but the
  obvious call-site remap didn't take — dependency imports resolve through a
  different path than sibling-file imports. Correctly unifying cross-module
  type identity across all import paths is a focused follow-up; full
  fitz-liveviews binary builds wait on it. The showcase runs under `fitz run`
  meanwhile.
