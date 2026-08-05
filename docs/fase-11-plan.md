# Phase 11 — Native frontend in Fitz core

**Status**: sub-phases 11.1 + 11.2.a/b/c + view-lexer §7 + 11.3.a/b/c
CLOSED 2026-07-14. Sub-phases 11.4.a/b/c/d CLOSED 2026-07-14 →
2026-07-15 (browser smoke manual on the counter demo validated
the WASM emitter end-to-end within the 40 KB gzipped gate).
**Sub-phases 11.5.a/b/c/d/e CLOSED 2026-07-15 — Phase 11.5 shipped
entirely.** `fitz build --bin <web>` on a `.fitzv` with `target =
"wasm-client"` produces a browser bundle end-to-end; single-
component AND multi-component fixtures work, with the composition
subset that 11.5.d anchored (static props, self-closing children,
no event bubbling). **Sub-phase 11.6.a CLOSED 2026-07-15** —
research + decision doc pinning SSR emitter + fitz-liveviews
migration as the 11.6 deliverable set, with client-side dynamic
capabilities re-scoped to 11.7 (see §9.u). See `docs/stack.md`
for the architectural constitution this plan implements.

Sub-phases still open: **11.6.e (partial as of §9.bb
2026-07-16 — §9.z shipped SSR emitter `payload` in event-
body scope + `fitz_liveviews` missing-dep hint + counter
migration draft uncommitted; §9.aa shipped event-body
widening for `let` + nested `if` guards + `Expr::If`/
`Expr::StructLit` on RHS, unblocking kanban's
`card_editor_save` + chat's `send_message` `.fitzv`
migrations; §9.bb ships cross-module `@live_component`
auto-inject — extends v0.20.1's implicit `flv_register(...)`
to types declared in imported `.fitzv`/`.fitz` sibling
modules via `TypeEnv.imported_live_components` +
`pre_scan_imported_live_components` paralelo W12/B10;
remaining: cross-file `<Child />` composition (low
priority, §9.y debt), migration commits in
fitz-liveviews deferred until Fitz v0.21.0 release)**,
11.7 (client-side dynamic capabilities + kanban SPA
port), 11.8 (LSP support), 11.9 (pedagogic docs).

**Update 2026-07-20**: 11.7 CLOSED entirely at v0.24.0, plus
the cross-file composition refinements (v0.25.0/v0.26.0). The
client-WASM composition surface is complete. The next Phase 11
iteration is **fine-grained reactivity + fullstack (11.10–11.13)**
— signals, `@server`, SSR→client hydration, template hot reload.
See §11 at the bottom of this doc + the "próxima iteración" block
in `docs/roadmap.md`.

**11.6.b + 11.6.c + 11.6.d CLOSED ENTIRELY 2026-07-15** —
`view::emit_module_ssr` emits classic Fitz source with the
FULL template grammar (Text, Interpolation, Element, static
+ interpolated + event attrs, `{#if}` + `{#else}`, `{#for}`,
`<style>` inline, same-file `<Child prop="v" />`
composition) + full expression grammar (BinOp, UnaryOp,
Call, Field, Index, StrInterp, List, Map, Range, Ok, Err,
arrow FnExpr) with state-field rewriting + closure-param
local-scope tracking. View lexer accepts `.` in event body
context. Round-trip tests validate that emitted source
lexes + parses through classic Fitz. The classic module
loader — evaluator (`fitz run`), codegen loader
(`fitz build`), CLI pre-scan paths (`fitz check`), and LSP
cross-module resolvers — routes `.fitzv` files through the
view pipeline transparently: `.fitz` wins if both siblings
exist (backward-compat), `.fitzv` is fallback.

This document captures the decisions and the sub-phases shipped
so far, plus the shape of the ones still open. Its purpose is to
make the next sub-phase concrete enough that it can be picked up
in a fresh session without re-litigating shape.

---

## 1. Extension: `.fitzv`

The Phase 11 file format is `.fitzv` — read as "fitz view". One
component per file idiomatically; the parser accepts multiple
top-level `component` blocks in the same file but the plan defers
multi-component wiring to 11.2+.

**Why not `.flv`?** Collides with the FLV video container format;
would confuse tooling and search results.

**Why not `.fitzc`?** Reads as "fitz C" more than "fitz component";
also collides with the compilation-artefact convention (`.pyc`,
`.class`).

**Why not `.view.fitz` (Nuxt-style suffix)?** The classic `.fitz`
parser has no reason to look at those files; splitting them off
into `.fitzv` keeps Invariant 4 of `docs/stack.md` (the classic
parser CANNOT touch view code) trivial to enforce — the CLI
routes by extension, not by content sniffing.

**Why `.fitzv`?** Short (five chars), reads naturally, stays in the
`.fitz*` family, no known collision.

---

## 2. What lives in a `.fitzv` file

The shape recognised by the POC parser (`src/view/parser.rs`):

```
component ComponentName {
  state {
    field_name: TypeExpr = default_expr
    // ... more fields
  }

  event handler_name(params) {
    // body — same statement grammar as classic Fitz fns
  }

  <template>
    <!-- HTML with {expr} interpolation and @click="handler" -->
  </template>

  <style scoped>
    /* opaque CSS (parsed in 11.3+) */
  </style>
}
```

Rules the POC enforces today:

- Exactly one component per top-level block; multi-component per
  file is allowed but the plan defers "how does the codegen emit
  N components in one artefact?" to 11.2.
- `state`/`event`/`<template>`/`<style scoped>` are all optional
  and can appear in any order.
- Only one `<template>` and one `<style scoped>` per component
  (duplicates error clearly).
- All `state` fields require a type annotation and a default
  (parallel to `@live_component` types today, so the compiler can
  synthesise `TypeName {}` at boot without arguments).
- `<style scoped>` is the only style form accepted; unscoped
  styles are deferred to 11.3+ pending a decision on the
  global-style story.
- Attribute values are fully static (`class="card"`) or fully
  interpolated (`value="{title}"`). Mixed values
  (`class="btn btn-{kind}"`) land in 11.2+.
- No control flow inside `<template>` yet — `{#if}` / `{#for}` /
  `<slot>` are all deferred to 11.2+.

---

## 3. Isolation strategy (Invariant 4 of docs/stack.md)

The Phase 11 parser lives in `src/view/`, a dedicated module with
its own lexer, parser, and AST. Concretely:

- `src/view/mod.rs` — exports `parse(source: &str) -> ViewParseResult<ViewFile>`.
- `src/view/ast.rs` — SFC AST (`ViewFile`, `Component`,
  `StateField`, `EventHandler`, `Template`, `TemplateNode`,
  `Attr`, `Style`, `Loc`).
- `src/view/lexer.rs` — char-by-char tokenizer. Recognises the
  keywords `component`/`state`/`event`, identifiers, string
  literals, delimiters, and — critically — the raw blocks
  `<template>...</template>` and `<style scoped>...</style>`
  which it captures as single tokens.
- `src/view/parser.rs` — recursive-descent parser over the tokens
  plus an embedded HTML sub-parser that walks the `TemplateRaw`
  blob.

**Reuse from the classic pipeline**: **zero**, deliberately. The
view lexer does NOT invoke `crate::lexer::tokenize`. The view
parser does NOT invoke `crate::parser::parse`. The AST does NOT
share types with `crate::ast`. This is the price for Invariant 4:
if a bug happens in the view module, it CANNOT reach the classic
pipeline.

**When 11.2 lands** (parsing state defaults, event bodies, and
template interpolations as `crate::ast::Expr`/`Stmt`), we will
reuse `crate::lexer::tokenize` on the raw string blobs stored in
the view AST — but the entry point remains dispatch-by-extension,
and the view parser stays the front door.

Wiring into `src/lib.rs`: plain `pub mod view;`. No feature gate,
because the module adds zero new deps and today no code path in
the `fitz` binary dispatches to it — only tests and external
tooling can call `view::parse(...)`. A feature gate would add
friction to the smoke without upside.

---

## 4. Contract with the checker

**Today (POC)**: the view AST captures state field types,
defaults, event handler params, event bodies, and template
interpolations as **raw source strings**. No type-checking happens
against this AST.

**Phase 11.2 will type-check**:
- Every `state` field type — reuses `crate::types::resolve_type_expr`
  against a fresh `TypeEnv` seeded with the built-in nominals.
- Every `state` field default — same rules as `type Foo { field: T
  = expr }` today: the default must resolve to a value compatible
  with the declared type.
- Every `event` handler body — same rules as an `async fn` inside
  classic Fitz today: statements are checked in an env seeded with
  the component's state fields as `let`-bindings, plus the params
  declared on the handler.
- Every template `{expr}` interpolation — checked in the same env
  as the event handler bodies (state fields visible as
  identifiers). The result type must be `Str`-friendly (`Str`,
  `Int`, `Float`, `Bool`, or a nominal with a `Display` impl).
- Every `@event="handler"` attr — validates that `handler` is one
  of the declared `event ...` handlers in the same component.

**What stays opaque to the checker in 11.2**:
- Attribute names (`class`, `disabled`, `data-flv-value-foo`) —
  not validated against an HTML spec; the SFC targets any HTML
  attribute the user names.
- Tag names (`div`, `custom-el`) — not validated; the target
  might be a custom element registered client-side.
- Inline styles — the `style` attr value is a plain string.
- CSS inside `<style scoped>` — the checker does not parse CSS in
  the MVP.

---

## 5. Contract between codegen SSR and codegen client

Two targets Phase 11 will support, both derived from the same
view AST:

**Target A — SSR (Rust)**: emit a `pub async fn render_ComponentName(state: ComponentNameState) -> String` alongside the component's data struct. Server code composes components by calling these render fns and concatenating strings. `<style scoped>` blocks are hashed into a per-component class prefix and injected into the HTML alongside the markup.

**Target B — Client (WASM or vanilla JS)**: emit a per-component module that owns the state, wires DOM events to the handler fns, and re-renders on state change. Two candidate lowering strategies (decision deferred to 11.4):
- **WASM-first**: compile the component's Rust to WASM, ship a small JS shim that mounts the component into a DOM node. Bigger bundle but zero-language mismatch.
- **JS-vanilla**: emit hand-written JS from the view AST — the SFC becomes a small vanilla JS module with a state store and event bindings. Smaller bundle, but the compiler grows a JS emitter.

**What SSR and client SHARE**:
- The AST after Phase 11.1's parser.
- The state field parsing (defaults resolved at compile time to
  values that both targets serialise the same way).
- The event handler parsing to `Vec<Stmt>`.
- The template AST post-11.2 (with `{#if}`/`{#for}`/`<slot>`
  parsed).
- The `<style scoped>` hashing + class-prefix strategy.

**What SSR and client DO NOT share**:
- The output format (Rust code vs WASM/JS module).
- The event dispatch mechanism (server calls the handler on the
  next request/websocket message; client attaches DOM
  listeners).
- The reactivity model (SSR is pure functional recompute; client
  needs a diffing / VDOM / signals mechanism).

**Compiler flag** (11.5+): `fitz build --target ssr` (default when
the manifest has `[[bin]]`), `fitz build --target wasm`, `fitz
build --target js`. Emitting BOTH from the same source is the
Phase 11 promise.

---

## 6. Sub-phase plan

| Sub-phase | Scope | Cierre criterion |
|---|---|---|
| **11.1** | POC parser: recognise the SFC shape, capture bodies as raw. Isolated `src/view/` module. **CLOSED 2026-07-14** — this doc + `src/view/` + 19 unit tests. | Card component test parses cleanly; classic pipeline untouched (Invariants 1-5 verified). |
| **11.2** | Parse state defaults / event bodies / template interpolations as `crate::ast::Expr`/`Stmt`. Add `{#if}` / `{#for}` / `<slot>` to the template AST. Type-check every expression in `state` + `event` + `{...}` interpolations. | `fitz check my.fitzv` reports type errors for state field mismatches and template interpolation type errors, with source-accurate line/column pointing inside the `.fitzv` file. |
| **11.2.a** | *Sub-step of 11.2.* Parse state defaults / event bodies / template interpolations / event params / attr interpolations as classic Fitz AST. Introduce `crate::view::expand` bridging the raw view AST back through the classic lexer + parser via 4 new pub entry points in `crate::parser`. **CLOSED 2026-07-14** — `src/view/expand.rs` (~660 LoC) + 4 pub fns `parse_expression_from_source` / `parse_type_expression_from_source` / `parse_statements_from_source` / `parse_parameters_from_source` in `src/parser.rs` + 16 unit tests. No checker yet — that lands in 11.2.b. Positions inside spans are shifted approximately (blob-local + best-effort base); precise offset tracking still deferred. | Card component `expand()`s end-to-end producing `Expr::Str` / `Expr::Bool` state defaults, `Vec<Stmt::Assign>` event bodies, `Vec<Param>` event params, and `Expr::Ident` template interpolations. Two error cases (bad default, bad body) carry the naming context so users can find the wrong blob. |
| **11.2.b** | *Sub-step of 11.2.* Type-check every parsed AST from 11.2.a. Split into three mini-commits, ALL **CLOSED 2026-07-14**: **(1)** state field defaults compatible with declared type (`src/view/check.rs` ~325 LoC + 16 unit tests). **(2)** event handler bodies checked in an env seeded with state fields as let-bindings + params; template `{expr}` interpolations checked in the state env with the additional Str-friendly rule (rejected: Function, Result, Future, WsConn, DbConn/DbRow, QueryBuilder, Aggregated, Secret) (`src/view/check.rs` ~640 LoC total + 23 new unit tests). **(3)** `@event="handler"` attrs cross-check that `handler` names a declared event handler in the same component; broken references get a "did you mean ...?" hint via Levenshtein distance ≤ 3, or the full available handler list when the declared set is small (≤ 5) (`src/view/check.rs` ~825 LoC total + 11 new unit tests). **Closes 11.2.b entirely — 50 view::check tests all green.** | `.fitzv` files with mismatched types (e.g. `count: Int = "hi"`) or broken `@click="handler"` references surface at the correct field/blob with a friendly suggestion. |
| **11.2.c** | *Sub-step of 11.2.* Extend the template AST with `{#if cond}`, `{#for x in xs}`, `{#else}`, and `<slot name="X" />`. Update the HTML sub-parser + expand + checker to handle them. Split into three mini-commits, ALL **CLOSED 2026-07-14**. **Mini-commit 1**: `{#if cond}...{/if}` end-to-end (raw AST + HTML sub-parser directive dispatch + expand parses cond as classic Expr + checker validates cond is Bool-compatible and recurses walkers into If children). ~450 LoC + 24 new unit tests (9 parser + 4 expand + 11 checker). See §9.f. **Mini-commit 2**: `{#for x in xs}...{/for}` end-to-end (raw AST + HTML sub-parser dispatch on `for` + expand parses iter as classic Expr + `check_template_for_iters` pass delegating to the classic `Stmt::For` checker for iterable typing + refactor of the 3 collectors to track a `for_scope` chain so interps and `{#if}` conds nested inside a for see the binding, wrapped in the corresponding `Stmt::For` chain when synthesising). ~570 LoC + 25 new unit tests (10 parser + 4 expand + 11 checker). Binding restricted to a single bare identifier — compound patterns `(k, v)` for Map and index bindings `(x, i)` deferred. See §9.g. **Mini-commit 3**: `{#if cond}...{#else}...{/if}` (extend `TemplateNode::If` with `else_children: Option<...>` + refactor `parse_nodes` with `accept_else: bool` returning `(nodes, terminated_by_else)` + `{#else}` sentinel intercepted by `parse_nodes` when accepting, dispatched as targeted error by `parse_directive_open` otherwise) and `<slot />` / `<slot name="X" />` (new `TemplateNode::Slot { name: Option<String>, loc }` variant intercepted in `parse_element`'s self-closing branch; open-close form `<slot>...</slot>` rejected with a 11.5-pointer message; only the `name` attribute accepted, everything else rejected with targeted messages). All 4 checker collectors (`collect_if_conds` / `collect_for_iters` / `collect_interpolations` / `collect_event_attrs`) recurse through `else_children` when present; `Slot` is a leaf that every collector skips. ~740 LoC + 26 new unit tests (14 parser + 5 expand + 7 checker). **Closes 11.2.c entirely** — the template dialect now has the full set of directives promised for 11.2, and 11.2 itself is done. See §9.h. | Nested control flow inside `<template>` parses, expands, type-checks; `<slot>` markers survive the pipeline so 11.5 composition can consume them. |
| **11.3** | CSS scoping. Parse `<style scoped>` into a small AST, apply per-component class prefix, emit scoped CSS in the SSR output. Decide unscoped style story (`<style global>` or a separate directive). Split into three mini-commits, ALL **CLOSED 2026-07-14**: **11.3.a** — `<style global>` as a first-class sibling of `<style scoped>` in the lexer + parser + AST, plus new `StyleKind { Scoped, Global }` discriminant; bare `<style>` is rejected at lex time with a targeted error naming both accepted forms (see §9.i). **11.3.b** — standalone CSS mini-parser + `apply_scope(css_raw, scope) -> Result<String, CssParseError>` in `src/view/css_parser.rs` (~900 LoC + 45 unit tests + 1 doctest); walks the CSS char-by-char, suffixes every class selector `.<ident>` with `-<scope>`, recurses into `@media`/`@supports`/`@container`, keeps other at-rules opaque, handles strings + comments + attribute selectors + selector-arg pseudos (`:not(.foo)` correctly scopes the inner class) (see §9.j). **11.3.c** — wire scoping end-to-end in `expand`: new `enum ExpandedStyle { Scoped { css_scoped, scope_class, loc }, Global { css, loc } }` replaces the raw-passthrough `Option<RawStyle>`; scope class synthesised via FNV-1a of `<component>::<css_raw>` truncated to 8 hex, shape `<component-kebab>-c-<8hex>`; `apply_scope(...)` runs on the CSS body; every Element with a static `class` attribute gets suffixed variants added (`class="card"` → `class="card card-<scope>"`) via a recursive walker that descends into If then/else branches + For bodies; interpolated `class` attributes left alone as a documented limitation. Global styles are pure passthrough — no CSS transform, no template rewrite. Includes 21 new unit tests + `src/view/expand.rs` grew from ~830 LoC to ~1650 LoC (see §9.k). **Closes 11.3 entire** — Phase 11.3 shipped end-to-end. | A component with `<style scoped>` styling produces HTML + CSS where the styles apply only to that component's markup, verified against `.fitzv` fixtures. |
| **11.4** | Client target decision (WASM vs JS-vanilla). Prototype whichever wins on a two-page counter demo. Confirm bundle size is acceptable. Split into four sub-commits: **11.4.a** — research + decision (docs-only, no code). **CLOSED 2026-07-14** — decision recorded is **WASM-first, hand-rolled `wasm-bindgen` + `web-sys` directly under opt-in feature `client-wasm`** (Approach A2 in §9.l). Rationale: aligns with `docs/stack.md` v1 "WASM primero, JS/vanilla secundario"; preserves the "Fitz por sí solo" promise (no external framework, no `npm`); preserves Invariant 4 (emitter isolated in `src/view/codegen_wasm.rs`, `cargo build` default unchanged); bundle 15-25 KB gzipped acceptable for 90% of cases. Gate for 11.4.b/c: if the counter POC exceeds 40 KB gzipped, PIVOT to JS-vanilla and refresh `docs/stack.md`. See §9.l. **11.4.b** — POC emitter of the chosen target on the counter subset (state Int + eventless params + template Text/Element/Interpolation/Event/Static-class). **CLOSED 2026-07-14** — `src/view/codegen_wasm.rs` (~1500 LoC + 23 unit tests) emits Rust source for one component (struct + `new()` + event fns + `mount()` + `render()` + scoped/global style helper) via `pub fn emit_component(&ExpandedComponent) -> EmitResult<String>` and a whole `.rs` module via `pub fn emit_module(&ExpandedViewFile) -> EmitResult<String>`. Naive re-render on state mutation (D1), two-fn public API (D2), strictly conservative subset (D3 — Int state + `@click`-only + literal-Int + BinOp arithmetic; If/For/Slot/non-click/Str/Bool/Nominal/handler-params reject with `EmitError` citing 11.4.c/11.5), string-grep unit tests only (D4). `Cargo.toml` gains opt-in feature `client-wasm` with `wasm-bindgen`/`web-sys`/`console_error_panic_hook`; `cargo build` default dep tree unchanged. Verification: 3510 lib tests green (3487 baseline + 23 new), fmt + clippy `-D warnings` clean in both default and `--features client-wasm` modes. See §9.m. Deuda derivada VISIBLE that gated 11.4.c (now CLOSED via §9.n follow-up 2026-07-14): view lexer arithmetic gap. **11.4.c** — counter demo runnable (`examples/view/counter/`) + browser smoke + bundle-size measurement recorded against the 40 KB gate. **CLOSED 2026-07-15** — `examples/view/counter/{Counter.fitzv, index.html, README.md, wasm-crate/}` shipped end-to-end. Harness lives in `tests/view_counter_wasm_smoke.rs`: `regenerate_counter_lib_rs` (always runs on `cargo test`, keeps `wasm-crate/src/lib.rs` in sync with the emitter output + validates structural invariants) + `build_counter_wasm_and_measure` (`#[ignore]`, opt-in via `-- --ignored`, runs `wasm-pack build --release --target web` and measures gzipped size against the 40 KB gate). `flate2 = "1"` added as `dev-dependency` for the size measurement. Composed entry point (`#[wasm_bindgen(start)] pub fn start()`) lives at the tail of the harness, NOT in `view::emit_module()` — same posture Phase 11.5 CLI will inherit. Verification: 3517 lib tests green (unchanged baseline, no regressions), fmt + clippy default + clippy `--features client-wasm` all clean. See §9.o. **MEASUREMENT CLOSED 2026-07-15** — measured on Windows 11 with `rustc + rust-std wasm32-unknown-unknown + wasm-pack 0.15.0`: raw `.wasm` 26.1 KB / **gzipped 11.4 KB** (28.6 KB headroom under the 40 KB gate). A2 (hand-rolled `wasm-bindgen`) validated as the primary WASM target; no pivot to JS-vanilla needed, `docs/stack.md` lines 99-101 remain accurate. `wasm-pack` requires `wasm-opt = ['-O', '--enable-bulk-memory']` metadata on the wasm-crate to accept modern rustc output — recorded in `wasm-crate/Cargo.toml` with the reason inline. Two cosmetic emitter warnings surfaced (`unused_parens` in BinOp assignment RHS + `non_snake_case` in style-injection helpers) and are documented as deuda derivada in §9.o Debt residual — not correctness bugs; deferred to the 11.5 emitter cleanup pass. **11.4.d** — cierre formal + roadmap refresh. **CLOSED 2026-07-15** — this doc's row + §9.o Result subsection + `docs/roadmap.md` Fase 11 section refreshed to reflect the gate outcome. Browser smoke validated manually on Windows 11 / Chrome (2026-07-15): counter renders in `#app` with initial `0`, `+`/`-`/`reset` mutate the value and re-render is scoped to the component's subtree (no full-body redraw, per §9.m D1 naive-render policy). Two cosmetic emitter warnings (`unused_parens` + `non_snake_case`) intentionally deferred to the 11.5 CLI cleanup pass (see §9.o Debt residual). **Closes 11.4 entirely** — A2 (hand-rolled `wasm-bindgen` + `web-sys` under opt-in feature `client-wasm`) confirmed as the primary WASM target for Fitz, no pivot to JS-vanilla needed. | `fitz build --target <chosen>` produces a working browser demo of the counter component with state persisting across events. |
| **11.5** | CLI integration — `fitz build` routes `.fitzv` files based on `[[bin]] target` / `--target` flag. Multi-component composition (parent embeds child via `<Child prop="v" />`). Split into five sub-commits: **11.5.a** — research + decision (docs-only, no code). **CLOSED 2026-07-15** — decision recorded is **hybrid manifest + flag** (`[[bin]]` multi-bin closes debt 9.y.8+ as a side-effect; `target = "native"` \| `"wasm-client"` \| `"ssr"` with `ssr` reserved for 11.6+; `mount = "#app"` required for `wasm-client`; `--bin <name>` selector + `--target <t>` override; legacy `[bin]` singular auto-migrates). See §9.p. **11.5.b** — manifest extension: `[[bin]]` array-of-tables + `name`/`target`/`mount` fields + `--bin`/`--target` flags + auto-migration of legacy `[bin]` singular. Rejects `target = "ssr"` with targeted 11.6+ message. **CLOSED 2026-07-15** — `src/manifest.rs` now models `Manifest.bins: Vec<ManifestBin>` (was `Option<Bin>`) with new fields `name`/`target`/`mount`, new `Target` enum (kebab-case serde: `Native`/`WasmClient`/`Ssr`), new `ManifestWarning::SsrTargetReserved` surface for the CLI to display. Custom `Deserialize` via `RawManifest` + untagged `RawBinField { Single | Multiple }` auto-migrates legacy `[bin]` singular (fills `name` from `package.name` when omitted). Cross-field validation eagerly rejects `.fitzv` + `target = "native"` (both explicit and default) and `wasm-client` without `mount`. Custom `Serialize` preserves the visual `[bin]` singular shape for the common scaffolded case. CLI: `Commands::Build` gains `--bin <name>` + `--target <t>` (clap); new `resolve_entry_with_bin` propagates the selection + override into `ResolvedEntry.target_override` and `ManifestCtx.selected_bin`; helper `enforce_build_target_supported` rejects `wasm-client` citing 11.5.c and `ssr` citing 11.6+ before touching disk. 23 new unit tests in `manifest::tests` (legacy migration, multi-bin parse, target enum roundtrip, mount validation, `.fitzv` + native rejection, SSR warning, `select_bin`) + 7 new `cli_e2e` tests (`--bin` selector on multi-bin, wasm-client + 11.5.c rejection, ssr + 11.6+ rejection, unknown target value, legacy `[bin]` regression, `.fitzv` + native rejection). **Closes debt 9.y.8+ (multi-bin `[[bin]]`)** — noted in `docs/deudas-post-5b.md`. See §9.q. **11.5.c** — single-component `wasm-client` build. **CLOSED 2026-07-15** — new module `src/view/wasm_build.rs` (~430 LoC + 15 unit tests) owns the composition helpers: `compose_lib_rs(expanded, mount_selector, source_label)` runs `emit_module` and appends the `#[wasm_bindgen(start)]` wrapper (first-declared component = root, per §9.p); `compose_cargo_toml(pkg_name)` renders the wasm-crate Cargo.toml with the crucial `[package.metadata.wasm-pack.profile.release] wasm-opt = ['-O', '--enable-bulk-memory']` metadata knob (§9.o gotcha) + the exact `web-sys` feature subset the emitter uses; `sanitise_wasm_pkg_name` converts kebab-case bin names to snake_case Rust crate names; `write_wasm_crate_scaffold(dst, expanded, bin_name, mount, source_label)` materialises the scaffold on disk. `Manifest.warnings()` + the cross-field validation from 11.5.b keep the wasm-client path fail-fast. CLI: `Commands::Build` now dispatches `WasmClient` to `build_wasm_client_cmd(&resolved)` (in `src/main.rs`) which loads the `.fitzv`, runs the view pipeline (parse → expand → check), writes the scaffold under `<manifest_dir>/target/wasm-build/<bin_name>/`, shells `wasm-pack build --release --target web` inside it, and recursively copies `pkg/` to `<manifest_dir>/target/wasm/<bin_name>/`. Classic `.fitz` sources with `target = "wasm-client"` are rejected with a targeted 11.5.d pointer (composition case). Missing `wasm-pack` on the runner emits an actionable install pointer. The smoke harness `tests/view_counter_wasm_smoke.rs` now routes through `view::compose_lib_rs` — same helper the CLI uses — so the committed `examples/view/counter/wasm-crate/src/lib.rs` baseline is bit-for-bit what `fitz build --bin counter --target wasm-client` would produce. Added 3 new cli_e2e tests: (a) scaffold shape verification WITHOUT `wasm-pack` (checks `Cargo.toml` + `src/lib.rs` present with the composed wrapper and the wasm-opt knob); (b) `.fitz` + wasm-client rejected citing 11.5.d; (c) empty `.fitzv` (zero components) rejected with a targeted "no component to mount" error. The `[[bin]] name = "web"` + `mount = "#app"` path is now the canonical way to emit a browser WASM bundle from a `.fitzv`. See §9.r. **11.5.d** — multi-component composition. **CLOSED 2026-07-15** — capitalised template tags (`<Card />`) now expand to a dedicated `ExpandedTemplateNode::ChildComponent { name, props: Vec<ChildComponentProp>, loc }` variant. **Parser**: unchanged (`read_tag_name` already accepted `A-Z...`). **`expand.rs`**: `expand_template_node` dispatches on `starts_with_ascii_uppercase(tag)` to a new `expand_child_component` helper that enforces the composition shape — self-closing only (fallback children rejected citing 11.6+), static-value attrs only (dynamic `prop={expr}` rejected citing 11.6+), no event attrs (`@click="..."` on children rejected citing 11.6+, points at defining the handler inside the child's own `event ...` block). **`check.rs`**: new `check_child_components` pass builds a `HashMap<&str, &ExpandedComponent>` snapshot of the file and validates every `<Child />` site — child name exists (typo hint via Levenshtein ≤ 3, else full-list fallback), self-reference rejected with a "cannot mount itself" message, each prop's `field_name` matches a declared state field (typo hint), no duplicate props, and each `raw_value` coerces via the new `pub(crate) fn coerce_child_prop_raw_value(raw, type_expr) -> Result<String, String>` helper. Coercion supported for `Str`/`Int`/`Float`/`Bool` + `Nullable<T>` of those; compound types (`List`/`Map`/`Nominal`) rejected with 11.6+ pointers. **`codegen_wasm.rs`**: (a) `emit_mount_and_render` split — public `mount(selector)` now delegates to public `mount_into(root: HtmlElement)`, so composition sites hand pre-created elements directly; (b) new `emit_child_component` creates a wrapper `<div class="__fitz-child-<Name>">`, instantiates `Child::new()`, writes each coerced prop into the corresponding `RefCell<T>` state field, then calls `mount_into` on the wrapper; (c) `type_expr_to_rust` + `default_expr_to_rust` extended beyond `Int`-only to support the four primitive scalars plus `Nullable<T>` (matches the coercion helper — any type that flows through props is also usable as a state field). **Root convention**: first-declared component of the `.fitzv` is the WASM root; parent components mount children via inline `<Child />` sites. **RenderCtx** now carries `&'a ExpandedViewFile` so the emitter can look up the child's declared state field types when coercing. **Smoke harness** (`tests/view_counter_wasm_smoke.rs`) updated with 3 new structural invariants (both `mount(selector)` and `mount_into(root)` present, delegation call). **`emit_component`** wraps its single input in a synthetic `ExpandedViewFile` so the pre-existing single-component tests keep working. Counter baseline regenerated with the new `mount`/`mount_into` split — functionally identical output. **Tests**: 6 new expand tests + 16 new check tests + 4 new codegen tests + 1 new smoke test in `wasm_build.rs` + 2 updated pre-existing (Str state fields now emit successfully; nominal rejection message cites 11.6+ instead of 11.4.c). 321 view tests total, all green. See §9.s. Debt residual: **fallback children via `<slot>` + dynamic props + event bubbling on children**, all with targeted 11.6+ pointers in the rejection messages. Also: **child state resets on parent re-render** (documented consequence of naive-render §9.m D1 — persistent child state across parent renders needs a position-keyed component-instance cache, 11.6+). **11.5.e** — cierre formal. **CLOSED 2026-07-15** — three coordinated pieces landed together: (a) **cosmetic emitter warnings fixed** — crate-level `#![allow(non_snake_case, unused_parens)]` prepended by `emit_module_header` kills both warnings (§9.o Debt residual) uniformly across the emitted crate; chosen over per-fn attributes because the emitter's naming shape (`__inject_style_<PascalCase>_...`) is intentional and BinOp parens are correct for nested precedence, only redundant at the outermost RHS. (b) **Multi-component showcase fixture** `examples/view/showcase/` with `Dashboard.fitzv` (a `Board` root composing three `<MetricCard title="X" value="N" trend="Y" />` children with static Str + Int props), `wasm-crate/` scaffold (Cargo.toml + generated src/lib.rs), `index.html` mount shim, and README documenting build/serve recipe + honest limits (dynamic props / event bubbling / `<slot>` fill-in / cross-file composition / persistent child state all cited as 11.6+ debt). This is NOT the kanban port the original criterion asked for — that criterion was scope-drifted when §9.p moved SSR to 11.6+ and 11.5.d confirmed dynamic props stay deferred. The showcase is the LARGEST fixture the 11.5.d subset permits and exercises multi-component composition + prop coercion + per-child state end-to-end via `fitz::view::compose_lib_rs`. Counter baseline regenerated to include the new `#![allow]` header (functionally identical). (c) **New smoke test** `tests/view_showcase_wasm_smoke.rs` with `regenerate_showcase_lib_rs` (always runs; keeps the committed baseline in sync + validates 3 `<MetricCard />` composition sites + 5 total `mount_into` calls — 3 from composition + 2 from `mount(selector)` delegations) and `build_showcase_wasm` (`#[ignore]`, shells `wasm-pack` when the toolchain is present). No hard bundle-size assertion — the 40 KB gate was per-component-count-1; re-baselining is documented in §9.t. **Closes Phase 11.5 entirely.** See §9.t for the full closure narrative + the re-scoped kanban plan for 11.6+. | Multi-component showcase fixture (`examples/view/showcase/`) compiles + validates the end-to-end 11.5.d subset. Kanban port itself re-scoped to Phase 11.6+ (needs dynamic props + event bubbling + persistent child state — see §9.t rationale). Compile times of the fixture: view pipeline <100ms, `wasm-pack build --release` ~30s cold (`cargo build --release` + LTO + wasm-opt) — acceptable, no hard gate. |
| **11.6** | SSR emitter + migration of `fitz-liveviews` examples to `.fitzv`. New `view::emit_ssr` backend emits classic Fitz source (state `type` + `@render_for` fn returning `html("""<...>""")` + one `@on` fn per event block, all consumed by the existing fitz-liveviews framework runtime). Module loader detects `.fitzv` files transparently and runs the view pipeline before handing to classic lexer+parser. Kept isolated: the SSR emitter targets the fitz-liveviews API contract; client-side dynamic capabilities stay in 11.7. Split into five sub-commits: **11.6.a** — Research + decision (docs-only, no code). **CLOSED 2026-07-15** — SSR emitter approach pinned in §9.u (reconciles §9.t drift; original §6 row 11.6 intent restored: server-side `.fitzv` targeting fitz-liveviews now, client-side SPA capabilities re-scoped to 11.7). **11.6.b** — Skeleton `view::emit_ssr` on a single-component fixture. **CLOSED 2026-07-15** — new module `src/view/codegen_ssr.rs` (~700 LoC + 20 unit tests) with `pub fn emit_module_ssr(&ExpandedViewFile) -> SsrEmitResult<String>` + `pub fn emit_component_ssr(&ExpandedComponent)` entry points. Emits classic Fitz source text targeting the `fitz-liveviews` framework contract: `from fitz_liveviews import Html, html` + `@live_component("<Name>") type <Name> { <fields> }` + `@render_for("<Name>") fn <Name>_render(state: <Name>) -> Html { return html("""<html>""") }` + one `@on("<Name>", "<event>") fn <Name>_<event>(state: <Name>, payload: Map<Str, Str>) -> <Name>` per declared event. Two source-to-emit transformations pinned per §9.u: (1) `@click="handler"` in the template lowers to `data-flv-click="handler"` in the emitted HTML (fitz-liveviews's client runtime binds `data-flv-<event>` to WebSocket frames); (2) `{field}` template interpolation lowers to `{state.field}` inside the emitted `html("""...""")` string. Event body lowering: mutations accumulate into a single struct-literal return that carries EVERY declared state field — mutated fields take the assigned RHS, untouched fields carry over from `state.<field>`. Bare-ident RHS naming a state field rewrites as `state.<field>` (e.g., `a = b` → `a: state.b,`). MVP scope guards (all with 11.6.c/d/7+ pointers in the rejection messages): non-literal RHS (BinOp, function calls) deferred to 11.6.c; multi-statement bodies with non-assignment statements deferred to 11.6.c; `{#if}`/`{#for}` template directives deferred to 11.6.c; `<style scoped>` / `<style global>` deferred to 11.6.c; `<Child />` composition deferred to 11.6.d; `<slot />` deferred to 11.7+; non-state-field ident in template interpolation deferred to 11.6.c (needs richer expression lowering). Acceptance criterion: the emitted Fitz source round-trips cleanly through classic `crate::lexer::tokenize` + `crate::parser::parse` — validated by `emit_output_round_trips_through_classic_fitz_lexer_and_parser`. See §9.v. **11.6.c** — Full event body lowering (multi-mutation → struct literal build), `{#if}`/`{#for}` template lowering to string concat, `<style scoped>` inlined as `<style>` tag inside HTML output. **11.6.d** — Module loader integration + same-file `<Child />` composition. **CLOSED 2026-07-15** — See §9.y. Loader entry points (`src/evaluator.rs::resolve_module_path` + `src/codegen.rs::ModuleLoader::resolve_path` + `src/codegen.rs::resolve_loader_import_file_path` + `src/main.rs::resolve_import_file_path` + `src/lsp.rs::resolve_import_file_path_lsp`) now try `.fitz` first and `.fitzv` as fallback (backward-compat: `.fitz` wins if both siblings exist), driven by two new pub helpers in `src/view/mod.rs`: `is_fitzv_extension(&Path) -> bool` and `resolve_module_file_candidates(&Path, &str) -> Option<PathBuf>`. When a `.fitzv` is resolved, the loader calls the new pub `transform_fitzv_source(source, path) -> Result<String, FitzError>` bridge which runs the view pipeline (parse → expand → check → emit_module_ssr) and returns classic Fitz source — the classic lexer + parser + checker + evaluator never see a view-side token. Any pipeline stage error wraps into a single `FitzError` naming the offending `.fitzv` path plus the stage that failed. Same-file `<Child prop="v" />` composition lands in the SSR emitter: the composition site lowers to `Expr("<Child>_render(<Child> { <props> }).raw")` and splices the child's rendered HTML into the parent's chain-form html body. Prop coercion follows the 11.5.d subset (Str / Int / Float / Bool / `Nullable<T>` of a primitive) via the new `coerce_child_prop_raw_value_to_fitz_literal` helper — a Fitz-literal-returning parallel of 11.5.d's Rust-literal `coerce_child_prop_raw_value`. Same-file constraint enforced by resolving the child name against the parent's `siblings: &[ExpandedComponent]` slice; cross-file `<Child />` (child in a sibling `.fitzv` imported into main.fitz) errors with a targeted 11.6.e pointer — that path needs the loader's expanded-file cache threaded through the checker, which is 11.6.e's scope. `<slot />` still 11.7+. **Auto-inject `fitz-liveviews` dep is DEFERRED** to 11.6.e — the user must declare it in `fitz.toml` (`fitz_liveviews = { path = "..." }`); the emit still lists `from fitz_liveviews import Html, html`, and a missing dep surfaces as the normal classic-loader "module not found" error at the emitted-source stage. The 2-file E2E (main.fitz + Card.fitzv sibling with a broken variant + a shadowing-priority variant) validates the loader routing via `fitz run`. 7 SSR unit tests + 8 loader-bridge unit tests + 3 cli_e2e tests. **11.6.e** — Migrate the 4 fitz-liveviews examples (counter → dashboard → chat → kanban) from raw-string HTML to `.fitzv`. Sub-step **§9.z (2026-07-16) PARTIAL**: SSR emitter `payload` in event-body local scope + `fitz_liveviews` missing-dep hint + counter migration draft applied to sibling repo uncommitted. Sub-step **§9.aa (2026-07-16) PARTIAL**: event-body widening — SSR emit_event_fn dispatches trivial vs widened bodies; wide path primes shadow locals + lowers `let x = ...` (assign to non-state ident) + `if(cond){body}` guards at stmt level + nested arms with scope truncation on exit; walker accepts `Expr::If` (single-expr arms) + `Expr::StructLit`. Unlocks kanban's `card_editor_save` (`let new_text = if(payload.has("text")){payload["text"]} else {text}`) and chat's `send_message` (nested `if(payload.has("author")){if(payload.has("text")){...}}`) migrations. Trivial-path regression zero (~10 pre-existing tests keep the compact struct-literal shape). +13 SSR unit tests. Remaining: cross-module `@live_component` auto-inject (removes manual `flv_register(...)` boilerplate for counter/dashboard/chat/kanban when component types live in imported `.fitzv` siblings, paralleling W12/B10 pre-scan pattern), cross-file `<Child />` composition (§9.y debt, low priority since 4 examples use runtime `component(name, id)`), migration commits deferred to Fitz v0.21.0 release. Cierre formal. | fitz-liveviews's 4 examples (counter/chat/dashboard/kanban) rewritten as `.fitzv` SFCs; migration is transparent to the framework runtime; each example's server-side behaviour is bit-for-bit equivalent to the pre-migration handwritten version (or documented intentional divergence). |
| **11.7** | Client-side dynamic capabilities on top of the 11.5 WASM emit surface — dynamic child props (`<Card title={expr} />`), event bubbling from children (`<Card @select="handler" />`), cross-file `.fitzv` composition, `<slot>` fallback children, persistent child state across parent re-renders (position-keyed component-instance cache), and the client-side kanban port as the concrete fixture that validates the whole surface. Scope re-set from §6 original (originally LSP support) — LSP moves to 11.8. Deferred here because SSR-first (11.6) delivers more real user value now; client-side SPA capabilities are a Phase 11.7+ story once demand appears. All rejection messages in `src/view/` currently pointing at `11.6+` will be updated to point at `11.7+` when 11.6 closes. | Kanban port from `fitz-liveviews/examples/kanban/` rewritten as a client-side SPA `.fitzv` (`target = "wasm-client"`) with drag-drop, dynamic card lists, per-column event bubbling. Compiles under a re-baselined bundle size gate (measured on the kanban itself). |
| **11.8** | LSP support inside `.fitzv` — hover over `{expr}` shows the type, autocomplete inside `state { }` and `event ...` bodies, template-attr completion knows about the declared event handlers of the enclosing component. Was originally 11.7; moved to 11.8 by the 11.6.a re-scoping in §9.u. | VSCode extension bumped with the new grammar + LSP config; typing `{sta` inside a template completes to `state field name` when the component has that field. |
| **11.9** | Pedagogic docs — chapter in `docs/guide.md` covering `.fitzv` from scratch. Chapter in `docs/curso/` (new module M9?) mirroring the pedagogic style of M1-M8. Update `docs/architecture.md` with the two-parser split. Was originally 11.8; renumbered by the 11.6.a re-scoping in §9.u. | Chapter runs someone from zero to a working counter component. Course module has runnable examples. Neither confuses the reader about what's classic Fitz vs what's `.fitzv`. |

Sub-phases are deliberately spaced. 11.2 alone is a multi-session
job because it wires the type-checker end-to-end. 11.4 (client
target) is the biggest unknown — the WASM vs JS decision is a
research task that might turn into a two-week detour.

---

## 7. What the POC learned

Running the POC surfaced a few things worth naming:

- **Raw-block capture at the lexer level is the right layout**.
  Emitting `Token::TemplateRaw(String)` and `Token::StyleScopedRaw(String)`
  keeps the shell parser small and lets each sub-parser (HTML, CSS)
  work on a clean blob. Trying to weave the HTML parser inline
  with the shell parser would have coupled the two and made error
  positions ambiguous.
- **The HTML sub-parser is char-by-char, not token-based**. HTML
  syntax has too many edge cases (attribute values with `=` in
  them, self-closing vs open tags, quoting rules) to reuse the
  Fitz token stream. A dedicated pass is smaller, faster, and
  simpler to reason about.
- **Capturing bodies as raw source strings is not free**. Preserving
  the user's exact formatting for later re-lexing means the parser
  keeps token gaps intact. The POC uses a `append_token_source`
  helper that reconstructs whitespace approximately from the token
  stream — good enough for the POC's opaque blobs, but 11.2 will
  need to preserve exact source ranges (probably by tracking char
  offsets alongside tokens).
- **Attribute value classification (`Static` vs `Interpolation` vs
  `Event`) at parse time is cheap and clean**. Value shape decides
  everything downstream — the checker only cares about
  interpolations and events, and the codegen emits totally
  different code for each. Doing the classification at parse time
  removes an entire visitor pass later.
- **Duplicate-block detection is worth having early**. Users will
  try `<template>` twice (once for main, once for a "loading"
  state), and the current error catches it clearly. 11.2+ may
  want to accept multiple templates keyed by name (`<template
  name="loading">`) as an ergonomic escape hatch.
- **State field type annotations no longer ASCII-limited** —
  **CLOSED 2026-07-14** as a view-lexer follow-up right after
  11.2.b mini-commit 3. The view lexer now emits `Lt`, `Gt`,
  `Question`, `LBracket`, `RBracket` when they're not opening a
  `<template>` / `<style scoped>` block; the shell parser's
  `capture_raw_until` gained bracket-depth awareness (`{}`, `()`,
  `[]` count as nesting pairs) so a `{}` map literal default no
  longer looks like the closing brace of the `state { }` block.
  Consequence: `List<Str> = []`, `Map<K, V> = {}`, `List<Map<K, V>> = []`,
  and `Str? = null` now round-trip through `.fitzv` source
  end-to-end. The direct-construction tests from mini-commits 1+2
  stay as coverage of the checker's internal paths; matching
  source-level tests were added alongside so the parse→expand→check
  pipeline is proved for each shape (see §9.e). The shell parser's
  "expected `state`, `event`, `<template>` or `<style scoped>`"
  error path still catches `<foo/>` and other stray `<` at the
  top level of a component — just from the parser now, not the
  lexer.

---

## 8. What the POC does NOT prove

- **That the checker will integrate cleanly**. Feeding raw source
  blobs back through `crate::lexer::tokenize` inside a `.fitzv`
  context needs source position remapping so that errors point
  inside the `.fitzv` file at the correct offset (not offset 0 of
  the re-lex). 11.2 will hit this problem and might need to
  extend `crate::lexer::TokenWithPos` with a base offset.
- **That the codegen will emit two targets from one AST**. The AST
  is small enough that this looks feasible, but the SSR / client
  contract is speculation until we write one.
- **That the LSP will work inside `.fitzv`**. Hover / go-to-def
  currently assume classic Fitz syntax. `.fitzv` needs its own
  hover positions and its own scope model.
- **That existing fitz-liveviews users will migrate happily**. The
  refactor of `kanban/`/`chat/`/etc. is at least a few days of
  work. If the migration hurts, we may want to keep the classic
  `html("""...""")` shim alive indefinitely.

---

## 9. Files added by 11.1

- `src/view/mod.rs` — module declaration + re-exports
- `src/view/ast.rs` — SFC AST types
- `src/view/lexer.rs` — dedicated tokenizer
- `src/view/parser.rs` — recursive parser + HTML sub-parser +
  hardcoded Card SFC + 12 unit tests
- `docs/fase-11-plan.md` (this file)

Delta on `src/lib.rs`: one line, `pub mod view;`.

## 9.a Files added by 11.2.a

- `src/view/expand.rs` (~660 LoC) — lowering from the raw view AST
  to `ExpandedViewFile` with classic Fitz `TypeExpr` / `Expr` /
  `Stmt` / `Param` produced by the classic parser via 4 new
  pub entry points. 16 unit tests.
- Delta on `src/parser.rs`: 4 new `pub fn` wrappers
  (`parse_expression_from_source`,
  `parse_type_expression_from_source`,
  `parse_statements_from_source`,
  `parse_parameters_from_source`).
- Delta on `src/view/mod.rs`: two lines to export `expand::*`.

## 9.b Files added by 11.2.b mini-commit 1

- `src/view/check.rs` (~325 LoC) — type-checker for state field
  defaults. Synth-and-delegate: builds a synthetic `Stmt::Assign`
  per state field and pipes it through `crate::types::check_program`;
  remaps every `FitzError` back to a `CheckError` with the correct
  `Loc` + `context` label naming the component and field. 16 unit
  tests, including direct-construction cases for `List<T>` /
  `Map<K, V>` / `Str?` (deferred to a follow-up view-lexer
  extension — see §7).
- Delta on `src/view/mod.rs`: two lines to export `check::*`.

## 9.c Files touched by 11.2.b mini-commit 2

`src/view/check.rs` grew from ~325 LoC to ~640 LoC (23 new unit
tests, total 39 in `view::check::tests`). No new files; no changes
outside `src/view/`.

**New shape of `check()`**:

- Every state field default is still checked in isolation
  (mini-commit 1 behaviour, preserved).
- If any state field in a component errors, the component's
  handler + interpolation checks are **skipped** (cascade
  avoidance — see the file's doc-comment). This keeps the output
  focused on the actual bug instead of piling up consequential
  errors on every downstream reference.
- Each event handler body is checked in a synth program built by
  `build_env_program(component, Some(&handler.name), None)`. The
  helper emits state fields as annotated `let`s and every OTHER
  handler as an empty-body `async fn` (signatures only) so
  handler-to-handler calls resolve. The handler being checked is
  emitted with its full body.
- Every template interpolation (text nodes AND HTML attribute
  values) walks through `collect_interpolations` and gets its own
  synth program via `build_env_program(component, None,
  Some(<interp assign>))`. Every handler is a signature (empty
  body); the interp expr is bound to a distinct `__view_interp_check_N`
  local so `check_program` populates the returned `TypeInfo` for
  the interp span. If `TypeInfo::type_at(interp.span())` returns
  a `Type` that fails the `is_str_friendly` allow-list, the check
  emits a dedicated CheckError citing the unfriendly type.

**Additions to the public surface**:

- No new pub API. `view::check` and `view::CheckError` re-export
  paths from 9.b remain unchanged.

**Design decisions worth naming**:

- Handlers as signatures inside interpolation env: matches
  Vue/React template scope. `{go}` where `go` is a handler
  resolves to a Function value; the Str-friendly rule then
  produces a clear error naming Function as unrenderable. If
  handlers were absent, users would see "unknown variable `go`"
  which is misleading — the handler IS declared, just not
  displayable.
- Emit body-less fn signatures for every non-focused handler
  when checking a specific handler's body. Alternative
  (pre-registering signatures into a `TypeEnv` and calling
  `check_with_env`) is more surgical but the empty-body approach
  is simpler and produces identical results. Empty bodies with
  `-> Null` inference type-check cleanly.
- The Str-friendly allow-list is intentionally generous
  (accepts List/Map/Tuple/Nominal/Nullable-of-friendly-inner)
  because the codegen will emit `format!("{}", value)` at the
  interpolation site, and every listed type has an auto-Display
  impl. The block-list (Function, Result, Future, WsConn,
  DbConn/DbRow, QueryBuilder, Aggregated, Secret) captures the
  cases where the display would be wrong or the type is
  deliberately opaque (`Secret<T>` redacts).

**Verification** (delta at 11.2.b mini-commit 2 over mini-commit
1's baseline of 3273 unit + 39 `view::check`): `cargo test --lib`
green (3312 total, 39 in `view::check::tests`), `cargo test --lib
--features lsp` green (3448 total), `cargo test --test cli_e2e
--release` (101/101), `cargo test --test openapi_e2e --release`
(3/3), `cargo fmt --all --check`, `cargo clippy --lib --tests
--bins -- -D warnings`. The `GUIDE_EXAMPLES_COMPILE` smoke
(~290 ejemplos guía+curso+TaskHub, ~7 min) remains green — no
regression outside `src/view/`.

No other files touched by 11.1 / 11.2.a / 11.2.b mini-commits 1/2.

---

## 9.d Files touched by 11.2.b mini-commit 3 — closes 11.2.b entirely

`src/view/check.rs` grew from ~640 LoC to ~825 LoC (11 new unit
tests, total 50 in `view::check::tests`). No new files; no
changes outside `src/view/`; no changes to the classic pipeline.

**New shape of `check()`**:

- State field defaults still checked in isolation (mini-commit 1
  behaviour, preserved).
- Every `@event="handler"` attr in the template walks through
  `collect_event_attrs` (parallel to `collect_interpolations`)
  and gets compared against the component's declared handler
  set. Broken references produce a `CheckError` with a context
  like `"component 'Card': template event attr '@click'"`. The
  cross-ref pass runs **independently of state validity** — it
  doesn't route through the classic checker and doesn't depend
  on state field types, so a component with a broken state
  field AND a broken `@click="undeclared"` surfaces both errors
  together instead of hiding the second behind cascade
  avoidance. Cascade avoidance still applies to handler bodies +
  interpolations for the same reason it did in mini-commit 2.

**Additions to the public surface**:

- No new pub API. `view::check` and `view::CheckError` re-export
  paths from 9.b remain unchanged.

**Design decisions worth naming**:

- **Cross-refs run even when state has errors.** Reasoning:
  the event-attr check is a pure structural comparison — no
  `check_program` call, no type resolution. State errors don't
  contaminate it and hiding the second category would confuse
  users iterating on a template.
- **"Did you mean ...?" hints via Levenshtein distance ≤ 3.**
  Threshold chosen to catch realistic typos (`stat` ↔ `start`,
  `saev` ↔ `save`, `strat` ↔ `start`) without suggesting
  unrelated matches on longer identifiers. When the declared
  set is small (≤ 5) and no near-miss exists, the error lists
  every available handler so users don't have to open another
  file.
- **Zero declared handlers is its own message.** Component with
  `@click="save"` but no `event ...` declarations at all gets
  a distinct message ("this component declares no `event ...`
  handlers") instead of an awkward empty available list.
- **Levenshtein inlined, not a crate.** Standard two-row DP,
  O(a·b) time, O(min(a,b)) space. Handler names are short (< 30
  chars typical) and per-template counts are small, so
  performance is trivially fine. Pulling in `strsim` (~30 KB
  compiled) would be overkill.

**Verification** (delta at 11.2.b mini-commit 3 over mini-commit
2's baseline of 3312 unit + 39 `view::check`): `cargo test --lib`
green (3323 total, 50 in `view::check::tests`), `cargo test --lib
--features lsp` green (3459 total), `cargo fmt --all --check`,
`cargo clippy --lib --tests --bins -- -D warnings`,
`cargo clippy --lib --tests --bins --features lsp -- -D warnings`.
Full `cargo test` suite from the "Fitz core — full test suite
tras cada batch" memory (cli_e2e, openapi_e2e, compile_e2e,
python feature when the local shim resolves) recommended before
tagging a release that includes this change; deferred here since
this is an internal-to-`src/view/` addition with no user-facing
surface yet (the `.fitzv` compilation entry point is still
gated behind Phase 11.5).

No other files touched by 11.1 / 11.2.a / 11.2.b mini-commits 1/2/3.

---

## 9.e Files touched by view-lexer §7 follow-up (closes the state-annotation ASCII debt)

Small follow-up mini-commit right after 11.2.b mini-commit 3. Its
only job is to destrabar `List<T>` / `Map<K, V>` / `Str?` /
`List<T>?` shapes at the source level — the checker already
handled them internally, but the raw-blob capture in the shell
parser stopped short because the lexer errored on the first `<`.

**Changes**:

- `src/view/lexer.rs` — 5 new `Token` variants (`Lt`, `Gt`,
  `Question`, `LBracket`, `RBracket`) with `Display` and dispatch.
  The `<template>` / `<style scoped>` block detection stays first
  in `run()`; a `<` that doesn't open one of those blocks falls
  back to `Token::Lt`. `>` / `?` / `[` / `]` get their own match
  branches. 4 new unit tests + the pre-existing
  `unknown_lt_at_top_level_is_error` was renamed and repurposed to
  assert the new behaviour (lexer accepts, parser rejects).
- `src/view/parser.rs` — `append_token_source` learned the 5 new
  tokens; `needs_space_before` puts `Lt`/`Gt`/`Question`/`LBracket`/`RBracket`
  in the "no space before" list so `List<Str>` and `xs[0]`
  reconstruct verbatim. `capture_raw_until` grew a bracket-depth
  counter (`{}`, `()`, `[]` count) so a `{}` map literal default
  no longer looks like the closing brace of the `state { }` block.
  `<`/`>` are NOT counted — a `<` in a state default context could
  be a comparison operator (`count < 5`), so tracking it would
  confuse those cases.
- `src/view/check.rs` — 7 new source-level unit tests mirroring
  the pre-existing direct-construction ones from mini-commits 1+2:
  `Str? = null`, `Str? = "hello"`, `List<Str> = []`, `List<Int> = ["nope"]`
  (mismatch), `Map<Str, Int> = {}`, nested `List<Map<Str, Int>> = []`,
  and `List<Str>? = null` (Nullable wrapping a generic — exercises
  `Question` right after `Gt`). Direct-construction variants stay
  as coverage of the checker's internal paths.

**Verification** (delta at view-lexer §7 over 11.2.b mini-commit
3's baseline of 3323 unit + 50 `view::check`): `cargo test --lib`
green (3334 total, 57 in `view::check::tests`, 11 in
`view::lexer::tests`), `cargo test --lib --features lsp` green
(3470 total), `cargo fmt --all --check`, `cargo clippy --lib
--tests --bins -- -D warnings`, `cargo clippy --lib --tests --bins
--features lsp -- -D warnings`.

**Deuda residual** (does NOT block 11.2.c):

- **`.` in state defaults**. The view lexer still rejects `.` at
  the top level of a state default (Float literals like `0.5`,
  field access chains like `env.NAME`, method calls). The
  `state_field_int_to_float_coerces_no_errors` test uses `= 0`
  (an Int coerced to Float) because `= 0.5` won't lex today.
  Fix scope for a next mini-commit if demand appears: emit
  `Token::Dot` and add it to the "no space" list in
  `needs_space_before`.
- **`/` in the top level of a component body**. Non-issue in
  practice — `/` only shows up inside `<template>` / `<style
  scoped>` raw blocks (where it never reaches the shell lexer)
  or inside default expressions as a division operator (where
  the classic parser handles it after `expand`). Documented
  because the retired lexer test `unknown_lt_at_top_level_is_error`
  used `<foo/>` and had to be rewritten to `<foo` to isolate
  the new `Lt` behaviour from the still-existing "unexpected
  `/`" error.
- **Struct literal defaults with explicit generic args** (e.g.
  `Result<Str, Str>::Ok("x")` if we ever add such syntax). The
  view lexer would tokenize it but `capture_raw_until`'s
  balance counter does NOT track `<`/`>` — a `>` outside any
  `{}`/`()`/`[]` at depth 0 still wouldn't confuse the current
  stop set, but if we grow syntax that needs generic tracking
  in defaults, revisit.

No other files touched by 11.1 / 11.2.a / 11.2.b mini-commits
1/2/3 or the §7 follow-up.

---

## 9.f Files touched by 11.2.c mini-commit 1 — `{#if cond}...{/if}`

First of the three mini-commits inside 11.2.c. Adds `{#if}` end-to-
end (raw AST + HTML sub-parser + expand + checker) without touching
mini-commits 2 (`{#for}`) or 3 (`<slot>`) — those will land
separately because they raise independent design questions
(`{#for}` needs a scoped binding for the loop variable; `<slot>`
is mostly a marker until 11.5 wires component composition).

**Files touched**:

- `src/view/ast.rs` — new `TemplateNode::If { cond_raw, children, loc }`
  variant. Doc-comment updated to name mini-commits 2/3 for the
  remaining directives.
- `src/view/parser.rs` — HTML sub-parser refactor:
  - `parse_nodes(&mut self, parent, directive_parent)` gained a
    second parent-tracking parameter (`directive_parent: Option<&str>`
    holds the name of the enclosing `{#...}` block, `if` today).
    Elements and directives nest orthogonally: an element inside
    an `{#if}` and vice versa both walk uniformly.
  - Dispatch inside `parse_nodes` when the next char is `{`:
    `#` → new `parse_directive_open` (parses cond raw + recurses
    for children up to `{/name}`); `/` → close directive (validate
    match against `directive_parent`); otherwise → the existing
    `parse_interpolation`.
  - `parse_directive_open` restricts to `if` today; `for` is
    rejected with a message pointing at 11.2.c mini-commit 2, so
    the user knows it's planned, not a bug.
  - `capture_directive_arg_raw` reads the cond up to the matching
    `}` with brace-depth counting so a struct or map literal
    inside the cond doesn't terminate early.
  - `read_directive_name`, `skip_ws_inline` — small char-by-char
    helpers matching the shape of the existing `read_tag_name`.
  - 9 new unit tests: happy-path capture, nested braces in cond,
    element inside If, If inside element, nested If inside If,
    unterminated opener, mismatched closer, closer without opener,
    unknown directive name.
- `src/view/expand.rs` — new `ExpandedTemplateNode::If { cond, children, loc }`
  variant. `expand_template_node` gains an arm that parses
  `cond_raw` via `parse_expr_at` with a `component 'X': template
  \`{#if}\` condition` context label, then recurses `children`.
  4 new unit tests: cond parses as `Expr::Ident`, cond parses as
  BinOp, bad cond syntax produces `ExpandError` with correct
  context, children expand recursively (interpolation inside If).
- `src/view/check.rs` — new pass `check_template_if_conds` +
  helper `collect_if_conds` + struct `IfCondRef`:
  - Each `{#if}` cond gets its own synth-and-delegate check
    parallel to interpolations: build the state-seeded env,
    bind the cond to `__view_if_cond_check_N`, run `check_program`,
    then check `TypeInfo::type_at(cond.span())` against
    `is_bool_compatible` (accepts `Bool`, `Any`, `PyAny`,
    `Nullable<Bool/Any/PyAny>`).
  - `collect_interpolations` and `collect_event_attrs` grow an
    `If { children, .. }` arm so interpolations + event attrs
    inside `{#if}` bodies get checked by their existing passes —
    no separate walk.
  - Runs inside the existing cascade-avoidance guard (skipped when
    state has errors, like handler bodies + interpolations).
  - 9 behavior tests (Bool state, BinOp Int > Int, non-Bool
    reports, undefined ident, interp inside body, event attr
    inside body, Bool? accepted, Int not accepted, nested If
    conds each checked) + 2 helper unit tests (`is_bool_compatible`
    accepts/rejects).

**Design decisions worth naming**:

- **Bool-compatibility instead of strict Bool**. Accept `Bool`,
  gradual escapes (`Any`, `PyAny`), and `Nullable<Bool/Any/PyAny>`.
  The classic `if` in the checker has the same posture — see
  `types.rs`. Rejecting Int explicitly means the user must write
  `count > 0`, avoiding JS-style truthiness surprises.
- **Two independent parent trackers in `parse_nodes`**. An enum
  (`ParseCtx::Root | Element(&str) | Directive(&str)`) would be
  slightly more idiomatic but the two axes are truly orthogonal
  and the pair of `Option<&str>` reads clearly. Preserving the
  original `parent: Option<&str>` shape also kept the diff to
  `parse_element` a single-line change.
- **Directive dispatch inside `parse_nodes`, not `parse_interpolation`**.
  `{#if}` looks like an interpolation opener but it isn't — it
  opens a new node type, not an expression. Rerouting at the
  `parse_nodes` layer keeps `parse_interpolation` focused on its
  original job (single `{expr}` node) and mirrors how HTML tags
  vs `<template>`/`<style>` are distinguished at the view-lexer
  layer.
- **Cascade avoidance still applies** to `{#if}` cond checks.
  Reason: cond checks route through the classic checker (like
  handler bodies + interpolations) and depend on state field
  types. When state is broken, running conds would flood the
  output with "undefined variable `X`" errors for every state
  field the cond references. Users fix state, then re-run — same
  ergonomics as mini-commits 2 and 3.

**Verification** (delta at 11.2.c mini-commit 1 over view-lexer
§7's baseline of 3334 unit + 96 view tests): `cargo test --lib`
green (3358 total, 68 in `view::check::tests`, 20 in
`view::expand::tests`, 21 in `view::parser::tests`), `cargo test
--lib --features lsp` green (3494 total), `cargo fmt --all
--check`, `cargo clippy --lib --tests --bins -- -D warnings`,
`cargo clippy --lib --tests --bins --features lsp -- -D warnings`.

**Deuda residual** (does NOT block 11.2.c mini-commit 3):

- **`{#else}` branch**. Not modelled today — a `{#if}` has one
  set of children, no else path. Re-scoped from mini-commit 2
  to mini-commit 3 when it became clear that adding `{#else}`
  independently of `{#for}` keeps commits bisectable. Mini-commit
  3 will fold `{#else}` alongside `<slot>` since both extend
  the template AST without changing scope semantics.
- **Truthiness on `Nullable<Bool>` is deliberately loose**. We
  accept `Bool?` as a cond but don't force the user to unwrap
  the null case explicitly. Same posture as classic `if`; may
  tighten later if lint feedback demands it (e.g. "always
  compare `Bool?` to `null` or `true`").
- **`is_bool_compatible` for `Result<Bool, _>`**. We reject it —
  the user must `match` or `?` first. Same rule as classic Fitz.

No other files touched by 11.1 / 11.2.a / 11.2.b mini-commits
1/2/3 or the §7 follow-up. `docs/fase-11-plan.md` gets the row
update in §5 and this section.

---

## 9.g Files touched by 11.2.c mini-commit 2 — `{#for x in xs}...{/for}`

Second of the three mini-commits inside 11.2.c. Adds `{#for}`
end-to-end (parser + expand + checker) parallel to `{#if}` in
mini-commit 1, plus the scope-tracking refactor of the checker
that mini-commit 1 promised. Same isolation posture as before —
`fitz check my.fitzv` remains the ONE public entry to the view
pipeline, and the classic `crate::parser::parse()` is untouched.

- **src/view/ast.rs**
  - `TemplateNode::For { var, iter_raw, children, loc }` — new
    variant paralelo a `If`. `var` is a bare String (single
    identifier — compound patterns and index bindings deferred).
    `iter_raw` is the trimmed raw source text between `in` and
    the matching `}`; expand reparses it as `fast::Expr`.
  - Doc comment on the `TemplateNode` enum updated: `{#else}`
    moves from "deferred to mini-commit 2" to "deferred to
    mini-commit 3", and `{#for}` moves from deferred to
    supported.

- **src/view/parser.rs**
  - `parse_directive_open` dispatches on the directive name:
    `if` → `parse_if_directive` (extracted from the previous
    inline body), `for` → `parse_for_directive` (new), other
    → error with mention of both supported directives + 11.2.c
    mini-commit 3 for `#else` and `<slot>`.
  - `parse_for_directive` reads a bare identifier, expects the
    literal keyword `in` (via new `consume_keyword_in` helper
    with a word-boundary check so `interior_var` doesn't match),
    then reuses `capture_directive_arg_raw` (brace-depth aware,
    same as `{#if}`'s cond capture) to grab the iter expression.
    Recurses `parse_nodes` with `directive_parent = Some("for")`
    for the body. Targeted error messages for: missing var
    identifier, missing `in`, empty iter, unterminated `{#for}`.
  - Added 10 unit tests (`template_for_block_*`): basic shape,
    complex iter with nested braces, nested inside `<ul>`,
    nested `<div>` inside for, nested `{#for}` + `{#if}` in
    various interleavings, unterminated for, mismatched close,
    missing var / missing `in` / empty iter, and the corner case
    `{#for in in xs}` (binding named `in` — legal but ugly).

- **src/view/expand.rs**
  - `ExpandedTemplateNode::For { var, iter, children, loc }` —
    new variant.
  - `expand_template_node` gains a match arm that reparses
    `iter_raw` as `fast::Expr` with context label `"template
    `{#for x in ...}` iter"`, then recurses children.
  - Added 4 unit tests (`expand_for_block_*`): iter parses as
    classic Expr, method-chain iter parses, malformed iter
    produces `ExpandError` with the correct context label,
    children expanded recursively.

- **src/view/check.rs**
  - New pass `check_template_for_iters` runs as the third
    template pass (after handler bodies + before interpolations
    and if conds), invoked from `check()`. Iterates every
    `{#for x in iter}`, synthesises `for <var> in <iter> { }`,
    and lets the classic `Stmt::For` checker resolve iterable
    types + reject non-iterables (`the `for` iterable must be
    List, Range or Map, received ...`). Errors are shifted to
    the block's `Loc` with context `"component 'X': template
    `{#for <var> in ...}` iter"`.
  - New helper types + refactor: `ForBinding<'a> { var, iter }`
    captures one enclosing `{#for}` binding; `wrap_stmt_in_for_scope`
    envuelve el `extra` stmt in a chain of `Stmt::For { ... }`
    outer-most to inner-most; `build_env_program` takes a
    fourth param `for_scope: &[ForBinding]` and applies the
    wrap. Reads: when an interp / if-cond / nested for-iter
    lives inside one or more `{#for}` blocks, the synth
    program mirrors the source's real scoping so the classic
    checker walks the fors, resolves each binding's type, and
    enters the innermost body with all bindings visible.
  - Refactor of the 3 collectors: `collect_interpolations`,
    `collect_if_conds`, `collect_for_iters` (new) all take
    `for_scope: &mut Vec<ForBinding<'a>>` and push/pop on
    enter/leave of `For` children. Each ref type
    (`InterpolationRef` / `IfCondRef` / `ForIterRef`) carries a
    `for_scope: Vec<ForBinding<'a>>` snapshot so the check-time
    synth sees the exact enclosing chain. `collect_event_attrs`
    recurses into For children without scope (event attrs
    cross-ref only names, don't need bindings).
  - Added 11 unit tests (`for_block_*`): List<Str> / List<Int>
    / Range binding types, non-iterable (`Str`) rejected with
    the classic message shifted to the iter context, var
    visible inside body interp (text and attr), var invisible
    after `{/for}`, `{#if}` cond sees the for binding, nested
    for iter references the outer binding via nominal field
    access (state-error cascade documented for the case where
    the nominal isn't declared), inner iter with a bad method
    on the outer binding's type errors at the iter context,
    Map iter binds Tuple which is Str-friendly, method chain
    iter typechecks.

**Design decisions taken at kick-off**:

- **Solo binding `x` bare** (sin `(x, i)` con index, sin
  `(k, v)` para Map). El clásico HOY no tiene index binding, y
  metering asymmetric syntax at the template level lo divergiría
  del lenguaje. `(k, v)` para Map con `Pattern::Tuple` es
  paralelo del clásico y refinable en un mini-commit dedicado
  si aparece demanda. El MVP acepta `Map<K, V>` bindeeando
  a `Tuple[K, V]` (heredado del clásico) — sigue siendo
  Str-friendly vía auto-Display, así que `{kv}` renderiza sin
  error, aunque awkward.
- **Delegación total al classic checker para tipar el binding**
  + rechazar no-iterables. Cero lógica de tipos nueva en
  `view/check.rs`. La única regla propia es "el iter tiene su
  propio contexto de error" — el mensaje se shifta al bloque
  `{#for}` correcto.
- **Scope tracking en los 3 collectors** — necesario para que
  interps y `{#if}` conds nested vean el binding. El costo es
  `.clone()` del `Vec<ForBinding>` en cada ref, insignificante
  en la práctica (templates casi nunca anidan más de 2-3 fors).
- **`{#else}` diferido a mini-commit 3** — mini-commit 1 dejó
  la promesa de "revisitar con mini-commit 2", pero mezclar
  `{#for}` + `{#else}` en el mismo commit rompe bisect. Mejor
  cerrar `{#for}` limpio y luego mini-commit 3 folds `{#else}`
  alongside `<slot>` (ambos extienden el template AST sin
  cambiar semántica de scoping).

**Verification** (delta at 11.2.c mini-commit 2 over mini-commit
1's baseline of 3358 unit + 3494 with `--features lsp`):
`cargo test --lib` green (3383 total, 79 in `view::check::tests`,
24 in `view::expand::tests`, 31 in `view::parser::tests`),
`cargo test --lib --features lsp` green (3519 total), `cargo fmt
--all --check`, `cargo clippy --lib --tests --bins -- -D warnings`,
`cargo clippy --lib --tests --bins --features lsp -- -D warnings`.

**Deuda residual** (does NOT block 11.2.c mini-commit 3):

- **`{#for (k, v) in m}` compound patterns**. No modelado hoy —
  el HTML sub-parser solo lee una identifier bare tras `#for`.
  El clásico ya soporta `Pattern::Tuple` para iterar Maps con
  `(k, v)`; llegar al template exige extender el sub-parser
  para leer un mini-Pattern. Refinable en un mini-commit
  dedicado si aparece demanda pedagógica real (hoy el
  workaround es `{#for kv in m}<li>{kv}</li>{/for}` que sale
  como `("clave", valor)` vía Tuple auto-Display).
- **`{#for (x, i) in xs}` index bindings**. El clásico HOY no
  tiene esto tampoco. Cualquier extensión al template debería
  aterrizar en paralelo con la del clásico para no divergir
  conceptualmente. Diferido sin fecha.
- **Codegen SSR/client del `{#for}`**. Todavía es solo
  parser + checker; el evaluator / codegen no consume
  `ExpandedTemplateNode::For` — llega en 11.4/11.5 con el
  client target decidido. El checker cierra el ciclo de "el
  usuario escribe `{#for}` y ve errores útiles si se equivoca",
  pero el template no renderiza aún.
- **Refinar el error del iter no-iterable con hint del
  `.iter()`/`.entries()` idioms**. Hoy sale el mensaje bruto
  del clásico. Aceptable — el mensaje es correcto y accionable.

No other files touched by 11.1 / 11.2.a / 11.2.b mini-commits
1/2/3 or the §7 follow-up or 11.2.c mini-commit 1.
`docs/fase-11-plan.md` gets the row update in §6 and this
section.

---

## 9.h Files touched by 11.2.c mini-commit 3 — `<slot />` + `{#else}`

Third and last mini-commit inside 11.2.c. **Closes 11.2.c entire**
— the template dialect now has the four directives promised for
Phase 11.2 (`{#if}`, `{#for}`, `{#else}`, `<slot>`) end-to-end,
and 11.2 itself is done. Two independent extensions folded into
one commit because they both add template-AST variants without
changing the scoping semantics that mini-commits 1 and 2 already
paid for.

- **`src/view/ast.rs`**
  - `TemplateNode::If` gained a new field
    `else_children: Option<Vec<TemplateNode>>`. `None` when the
    block is `{#if}...{/if}`; `Some(vec![...])` when the block
    has `{#else}` (with an empty vec when the else branch is
    empty). Doc-comment on the `If` variant updated to describe
    the extended shape.
  - New variant `TemplateNode::Slot { name: Option<String>, loc: Loc }`.
    `name` is `None` for the default (unnamed) slot,
    `Some("X")` for a named slot. Opaque marker — the tree
    carries it through expand + check but the semantic wiring
    (cross-check against child components' declared slots) lands
    in Phase 11.5.
  - The enum-level doc updated: `{#else}` and `<slot />` move
    from "deferred to mini-commit 3" to "supported"; the
    "Deferred to later mini-commits" list now names `<slot>...</slot>`
    fallback children and `{#elseif}` chains as the two follow-
    up refinements (neither blocks 11.3+).

- **`src/view/parser.rs`**
  - `parse_nodes` signature refactor: gains an `accept_else: bool`
    param and returns `(Vec<TemplateNode>, bool)` where the
    second bool is `true` when the walk terminated on `{#else}`
    (only possible when `accept_else` was `true`). All four
    callers updated — three ignore the bool via `let (nodes, _)`,
    one (`parse_if_directive`) matches on it.
  - `parse_if_directive` parses the then-body with
    `accept_else = true`; if the walk terminated on `{#else}`,
    it parses the else-body with a second call at
    `accept_else = false` (so a stray second `{#else}` inside
    the else branch is caught cleanly by `parse_directive_open`'s
    new "else" arm).
  - `parse_directive_open` grew an `"else"` arm that emits
    `"unexpected `{#else}` — must appear inside an `{#if}` body,
    and only one `{#else}` per `{#if}` is allowed"`. This covers
    both stray top-level `{#else}` and double `{#else}` inside
    an else branch, since neither case ever reaches
    `parse_nodes`' accept_else intercept.
  - Two new helpers on `HtmlParser`:
    `peek_directive_name(&self) -> Option<String>` reads the
    identifier immediately after `{#` WITHOUT consuming — used
    by `parse_nodes` to distinguish `{#else}` (an if-body
    terminator) from a regular directive opener;
    `consume_else_marker(&mut self) -> ViewParseResult<()>`
    consumes the six chars of `{#else}` and the closing `}`,
    validating brace balance with a clear error on the missing
    `}`.
  - `parse_element` gained two special cases for `<slot>`. In
    the self-closing branch (`.../>`), a `slot` tag hands off to
    a new free helper `build_slot(attrs, line, column)` which
    accepts zero or one `Static { name: "name", value }` attr
    and rejects everything else (extra static attrs,
    interpolated attrs, `@event` bindings, duplicate `name`)
    with targeted messages pointing at 11.5 for the richer
    slot APIs. In the open-close branch (`<slot>...</slot>`),
    the tag is rejected with the "fallback children not
    supported yet" message pointing at 11.5.
  - The mini-commit-2 "unknown directive" error message
    updated: it now names `{#if}`, `{#for}`, and `{#else}` as
    the supported directives rather than pointing at mini-commit 3
    (which no longer exists — this IS mini-commit 3).
  - 14 new unit tests: three `<slot>` shape tests (default slot,
    named slot, slot inside an element), four `<slot>` rejection
    tests (open-close form, extra static attr, event attr,
    interpolated `name`), one duplicate-`name` rejection, one
    basic `{#if...#else...}` shape test, one regression that
    `{#if}` without `{#else}` produces `else_children = None`,
    one legal `{#else}{/if}` (empty else body), one nested-if
    scoping test (each if scopes its own else, outer's else
    doesn't leak into inner and vice versa), two `{#else}`
    rejection tests (double else inside one if, stray top-level
    else), and one updated error-message test that names the
    supported directives.

- **`src/view/expand.rs`**
  - `ExpandedTemplateNode::If` mirrors the raw AST — gains
    `else_children: Option<Vec<ExpandedTemplateNode>>`.
  - New `ExpandedTemplateNode::Slot { name, loc }` variant — the
    expand pass copies the raw name verbatim (nothing to
    re-parse, the slot is opaque here).
  - `expand_template_node` gained a match arm that recursively
    expands `else_children` when present. Slot arm is a leaf.
  - 5 new unit tests: slot default expands with `name = None`;
    slot with name preserves it through expand; if/else with
    interpolations on both sides recurses through both
    branches; if without else preserves `else_children = None`;
    a bad interpolation inside the else branch reports the
    correct expand context (proves the recursion).

- **`src/view/check.rs`**
  - All four collectors (`collect_if_conds`,
    `collect_for_iters`, `collect_interpolations`,
    `collect_event_attrs`) grew a match arm for `Slot { .. }`
    (leaf — skipped) and extended the `If { ... }` arm to
    recurse through `else_children` when present. The scoping
    logic is unchanged: else branches see the same `for_scope`
    chain as then branches, because `{#else}` doesn't introduce
    new bindings (bindings come from `{#for}` and state fields).
  - 7 new unit tests: interpolation inside else branch is
    checked (proves `collect_interpolations` walks in); event
    attr inside else is cross-checked; nested `{#if}` cond
    inside else is checked; `{#for}` iter inside else is
    checked; regression that plain `{#if}` (no else) still
    checks the then branch; slot without other content produces
    zero errors; slot mixed with interpolations + events +
    if/else still produces zero errors when the rest is
    well-formed.

**Design decisions taken at kick-off**:

- **`name` on `<slot>` is optional**. `<slot />` maps to the
  default (unnamed) slot; `<slot name="X" />` to a named slot.
  Matches Vue/Svelte/Web Components convention. Making `name`
  mandatory would confuse users coming from those ecosystems
  and would require a synthetic name for the default slot
  ("default"?) that the compiler would have to invent —
  brittle.
- **Only self-closing `<slot />` today**. `<slot>...</slot>`
  with fallback children is deferred to 11.5 (composition
  wiring), where the actual semantics of "use the child's
  slotted content if any, else render this default" become
  meaningful. Today the slot is an opaque marker — accepting
  fallback children would look like it works when nothing
  consumes them. Cleaner to reject up front with a message
  that names 11.5.
- **`accept_else: bool` on `parse_nodes` vs a dedicated
  `parse_if_body`**. The bool + `(nodes, terminated_by_else)`
  return is ~30 LoC leaner than duplicating the parse_nodes
  loop for a specialised if-body helper. All four call sites
  update trivially (`let (nodes, _) = ...` for the callers
  that don't accept else; only `parse_if_directive` matches on
  the terminator). The alternative was cleaner conceptually
  but bulkier in code, and this file already does the same
  kind of orthogonal parent-tracking trick with `parent` and
  `directive_parent`.
- **`{#else}` sentinel intercepted in `parse_nodes` when
  accept_else is true, dispatched as targeted error by
  `parse_directive_open` otherwise**. Two paths reach a
  `{#else}` in the source: legitimately as an if-body
  terminator, or illegitimately (stray at top level, or
  second `{#else}` inside an else branch). The first path is
  handled by the intercept before it ever reaches
  `parse_directive_open`; the second path always ends up in
  `parse_directive_open` with the name "else" — so a
  dedicated "else" arm with a friendly message covers both
  illegitimate cases at once. `peek_directive_name` +
  `consume_else_marker` are two tiny helpers that keep the
  intercept clean.
- **Slot handling folded into `parse_element` rather than a
  separate top-level pass**. `<slot />` is syntactically an
  element — same tag+attrs+self-closing shape — so the
  cleanest place to intercept is right where `parse_element`
  decides whether to construct an `Element` variant. The
  conversion happens inside the self-closing branch (using
  the collected attrs) and the open-close branch rejects
  immediately. `build_slot` is a free helper (no self) so it
  reads clearly at file scope near `extract_full_interp`.
- **`else_children: Option<Vec<...>>` in the AST vs a
  flattened representation**. The `Option` shape matches the
  source: `{#if}...{/if}` has no else branch, `{#if}...{#else}...{/if}`
  does. Turning that into e.g. `arms: Vec<(cond, children)>`
  or `else_children: Vec<...>` (empty for the no-else case)
  would gloss over a meaningful semantic distinction.
  `{#elseif}` chains, when they come, would extend this to
  `Vec<(cond, children)>` + `else_children: Option<...>`
  (Svelte-style) — clean upgrade path.
- **All four collectors recurse into `else_children`**.
  Interpolations, `{#if}` conds, `{#for}` iters, and
  `@event="handler"` refs inside the else branch all get
  checked — no special treatment. The `for_scope` chain used
  by the interpolations / conds / iters walks unchanged into
  the else, because `{#else}` doesn't introduce new bindings.
  Slot is a leaf in every collector (no expressions to
  check, no cross-refs to make).

**Verification** (delta at 11.2.c mini-commit 3 over mini-commit
2's baseline of 3383 unit + 3519 with `--features lsp`): `cargo
test --lib` green (3411 total, 173 in `view::*` — 66 in parser,
25 in expand, 79 in check plus lexer + Loc tests), `cargo test
--lib --features lsp` green (3547 total), `cargo fmt --all
--check` clean, `cargo clippy --lib --tests --bins -- -D warnings`
clean, `cargo clippy --lib --tests --bins --features lsp -- -D
warnings` clean.

**Deuda residual** (does NOT block 11.3+):

- **`<slot>...</slot>` with fallback children**. Deferred with a
  targeted rejection message that names 11.5. When composition
  wiring lands, the fallback-children form becomes semantic:
  "render this default if the parent provides no slot content".
  Refinable then.
- **`<slot>` cross-reference against child components' declared
  slots**. Purely a marker today — no component knows what slots
  its callers reference or vice versa. 11.5 (composition
  wiring) is when the parent-child relation exists as an AST
  concept; cross-checking rides that.
- **`{#elseif cond}` chains**. Fitz stays at just `{#if}` and
  `{#else}` for MVP. `{#elseif}` is a shorthand for `{#else}{#if
  ...}{/if}` — nice to have, refinable behind demand. When it
  lands, the `TemplateNode::If` shape probably grows a
  `Vec<(cond, children)>` for chained branches + the existing
  `else_children` for the final catch-all.
- **Slot props / defaults / scoped bindings**. Vue and Svelte
  both let slots forward data down to their content
  (`<slot :prop="X" />`). Deferred to 11.5+ where the
  composition model is real. Today `<slot>` accepts only the
  `name` attribute and rejects everything else with a
  targeted 11.5 message.
- **Position mapping inside the raw blob for `{#else}` and
  `<slot>`**. Same debt as the rest of 11.2 — positions are
  approximate (blob-local + best-effort base). Precise offset
  tracking still deferred to a dedicated commit.

No other files touched by 11.1 / 11.2.a / 11.2.b mini-commits
1/2/3 or the §7 follow-up or 11.2.c mini-commits 1/2.
`docs/fase-11-plan.md` gets the row update in §6 and this
section.

---

## 9.i Files touched by 11.3.a — `<style global>` syntax + `StyleKind` enum

First of the three planned mini-commits inside 11.3. Opens the
unscoped-style story with the smallest possible reversible change:
a new sibling opener `<style global>` next to the existing
`<style scoped>`, plus a `StyleKind { Scoped, Global }`
discriminant on the AST + lexer token. Zero scoping is applied
yet — that lands in 11.3.b (CSS mini-parser) and 11.3.c
(expand wiring). The purpose of this mini-commit is to nail the
surface syntax first so the CSS parser + expand can be written
against a shape that won't shift.

**Files touched**:

- `src/view/ast.rs` — `Style` gains a `kind: StyleKind` field; new
  `enum StyleKind { Scoped, Global }` next to it. Doc-comment on
  `Style` rewritten to describe both shapes; doc-comment on
  `StyleKind` documents the semantics of each variant (Scoped
  gets class-prefix rewriting in 11.3.c, Global emits as-is). Both
  types stay `pub` inside `view::ast`; no re-export at
  `view::mod`'s `pub use` list because no external consumer needs
  them yet.
- `src/view/lexer.rs` — `Token::StyleScopedRaw(String)` refactored
  to `Token::StyleRaw { kind: StyleKind, body: String }`. The
  refactor was preferred over adding a parallel
  `Token::StyleGlobalRaw` variant because the two shapes share the
  same closer (`</style>`) and the same "opaque body" contract —
  distinguishing them as a payload field reads clearer than as two
  variants that would always be matched together. Detection in
  `run()` adds a `<style global>` branch parallel to the existing
  `<style scoped>` one; both delegate to the refactored
  `consume_style_block(kind, opener, ...)` helper that now takes
  the opener as a `&'static str` (so the "unterminated" error
  message reproduces the exact tag the user typed). Bare `<style>`
  or `<style anything-else>` is rejected at lex time with a
  targeted error naming both accepted forms — Vue defaults to
  global, Svelte to scoped, and Fitz refuses to pick a silent
  default. 5 new unit tests: `<style global>` captures body,
  `<style>` rejected, `<style foo>` rejected, unterminated
  `<style scoped>` names the scoped opener, unterminated
  `<style global>` names the global opener.
- `src/view/parser.rs` — the match on `Token::StyleScopedRaw`
  updated to `Token::StyleRaw { .. }` with a subsequent destructure
  to extract both `kind` and `body`. Duplicate detection widened:
  any second `<style>` block regardless of kind produces
  `duplicate `<style>` block — only one `<style scoped>` or
  `<style global>` per component`. The "expected `state`, `event`,
  `<template>` or `<style scoped>`" fallback error message also
  names both style forms now. `append_token_source` updated to
  ignore the new token variant (paralelo al viejo — the raw blob
  never appears inside `capture_*` calls). 5 new + 1 renamed unit
  tests: `duplicate_scoped_style_block_errors_clearly` (renamed
  from `duplicate_style_block_errors_clearly` — the message
  assertion widened accordingly), `duplicate_global_style_block_errors_clearly`,
  `scoped_and_global_style_together_rejected_as_duplicate`
  (documents the MVP "one style block per component" rule and
  points at the Vue/Svelte convention as a future refinement),
  `global_style_parses_with_kind_global`,
  `scoped_style_parses_with_kind_scoped`,
  `expected_shape_error_names_both_style_forms`.
- `src/view/expand.rs` — **no changes**. `ExpandedComponent.style`
  is still `Option<RawStyle>` and the new `kind` field on
  `RawStyle` rides through the `.clone()` transparently. When
  11.3.c lands, the expand output will grow a dedicated
  `ExpandedStyle { kind, css_scoped, scope_class }` variant that
  replaces this passthrough — but that requires the CSS parser
  (11.3.b) to exist first.
- `src/view/check.rs` — **no changes**. The checker doesn't touch
  CSS in the MVP (documented in §4). The `kind` field flows
  through the tree unchanged.

**Design decisions worth naming**:

- **Explicit opt-in over silent default**. Vue's `<style>` defaults
  to global, Svelte's defaults to scoped. Both work in their own
  ecosystems because users know the convention, but a user coming
  from the other framework hits surprising behavior on their first
  `<style>` block. Rejecting bare `<style>` at lex time trades one
  extra word (`scoped` or `global`) for zero ambiguity — the
  scoping intent is legible from the source without needing to
  know Fitz's default.
- **One style block per component (MVP cap)**. Vue and Svelte
  both allow "one scoped + one global" side by side. Fitz will
  probably get there once demand appears (splitting resets from
  component-specific rules is a real use case), but 11.3.a keeps
  the cap at one to avoid deciding ordering / merge semantics
  before the CSS parser + scoping wire land. If a user really
  needs both today, they factor the global rules into a dedicated
  global-only sibling component — clunky but workable, and the
  error message points at both accepted forms so the user knows
  the option exists in principle.
- **`Token::StyleRaw { kind, body }` refactor over
  `Token::StyleGlobalRaw` addition**. Additive would have been
  smaller diff but asymmetric — the two shapes share
  everything (closer, body semantics, downstream consumer) except
  the kind, so treating the kind as a payload field reads
  honester than treating it as a variant discriminator. The
  `TemplateRaw(String)` variant stays single-payload because
  templates have no "kind" concept and probably never will.
- **`consume_style_block(kind, opener, ...)` over
  hardcoded opener chars**. Passing the opener as
  `&'static str` (`"<style scoped>"` / `"<style global>"`)
  serves two purposes: (a) the "unterminated" error message
  reproduces the exact tag the user typed instead of a generic
  "unterminated `<style>` block" that would confuse users who
  only wrote one of the two forms; (b) the char-count arithmetic
  (`for _ in 0..opener.len()`) stays honest about what's being
  consumed, no magic 14.
- **No scoping applied yet**. 11.3.a is deliberately just the
  syntax opt-in — no per-component class prefix, no template
  attr rewrite, no CSS parsing. The next mini-commit (11.3.b)
  writes the CSS mini-parser + `apply_scope(css_raw, prefix)`
  helper as a pure standalone function, testable in isolation
  without touching expand. Then 11.3.c wires everything into the
  expand output. Splitting this way keeps each mini-commit
  bisectable — a bug in the CSS parser won't touch the
  syntax-opt-in shape and vice versa.

**Verification** (delta at 11.3.a over 11.2.c mini-commit 3's
baseline of 3411 unit + 3547 with `--features lsp`): `cargo test
--lib` green (3421 total, +10 new tests — 5 in
`view::lexer::tests`, 5 in `view::parser::tests`), `cargo test
--lib --features lsp` green (3557 total, +10 mirroring the same
new tests), `cargo fmt --all --check` clean, `cargo clippy --lib
--tests --bins -- -D warnings` clean, `cargo clippy --lib --tests
--bins --features lsp -- -D warnings` clean. Full `cargo test`
suite (cli_e2e, openapi_e2e, compile_e2e, python feature when the
local shim resolves) recommended before tagging a release that
includes this change; deferred here since 11.3.a is internal to
`src/view/` with no user-facing surface (the `.fitzv` compilation
entry point still lives behind Phase 11.5 per the plan).

**Deuda residual** (does NOT block 11.3.b):

- **"One scoped + one global" side by side**. MVP caps at one
  style block per component regardless of kind. Refinable if
  demand appears — the AST already carries `kind` per block, so
  extending `Component.style: Option<Style>` to
  `Component.styles: Vec<Style>` with a max-of-two-of-different-kinds
  invariant is straightforward. Ordering semantics (does scoped
  win over global on the same selector? or is it declaration
  order?) is the actual design question and is why the cap holds
  today.
- **`<style module>` for CSS Modules**. Not planned. If a third
  opt-in ever lands (e.g. per-file CSS module isolation with
  `import styles from './x.fitzv'` in a hypothetical companion
  file), the `starts_with` cascade in the lexer + a new
  `StyleKind` variant is the extension path. Nothing about
  11.3.a forecloses this.
- **`:global(...)` selector escape hatch inside `<style scoped>`**.
  Vue and Svelte both let a scoped block target parent/child
  elements via a `:global(...)` pseudo-class. Deferred to a
  follow-up on the CSS parser (probably a 11.3.b refinement or a
  11.3.d cosmetic pass) — the class-prefix strategy needs to
  learn about `:global(...)` to skip suffixing.

No other files touched by 11.1 / 11.2.a / 11.2.b mini-commits
1/2/3 or the §7 follow-up or 11.2.c mini-commits 1/2/3.
`docs/fase-11-plan.md` gets the row update in §6 (11.3 row now
carries a "mini-commit 11.3.a CLOSED 2026-07-14" annotation) and
this section.

---

## 9.j Files touched by 11.3.b — CSS mini-parser + `apply_scope(...)` helper

Second of the three planned mini-commits inside 11.3. Adds the
CSS mini-parser and the pure `apply_scope(css_raw, scope) ->
Result<String, CssParseError>` function that 11.3.c will call to
transform each component's `<style scoped>` body. Standalone — no
wiring into `expand` yet, no changes to the raw or expanded AST,
no changes to the shell / lexer / parser. The single new module
lives at `src/view/css_parser.rs` and is exposed as
`crate::view::apply_scope` + `crate::view::CssParseError` via
`view::mod`'s `pub use`.

**Files touched**:

- `src/view/css_parser.rs` (new, ~900 LoC + 45 unit tests + 1
  doctest). Contents: `pub fn apply_scope(css_raw: &str, scope:
  &str) -> Result<String, CssParseError>` (the only public API);
  `pub struct CssParseError { message, pos }` with `Display` +
  `std::error::Error`; `struct CssParser<'a>` private with
  char-by-char walk state; `enum AtRuleTerminator { Semicolon,
  Brace, Eof }` internal; helpers `transform_selector_list`,
  `transform_selector`, `capture_balanced_slice`,
  `capture_string_slice`, `capture_comment_slice`,
  `is_class_start`, `is_ident_cont`, `scan_ident_end`,
  `at_rule_nests_selectors`.
- `src/view/mod.rs` — one line adding `pub mod css_parser;` +
  one line adding `pub use css_parser::{apply_scope,
  CssParseError};`.

**Scoping strategy chosen — class-suffix**:

- Every class selector `.<ident>` in the CSS becomes
  `.<ident>-<scope>`. Everything else — type selectors, IDs,
  attribute selectors, pseudo-classes, pseudo-elements,
  combinators, declarations, comments, strings — passes through
  verbatim.
- Rationale: this is the smallest possible transformation that
  achieves per-component isolation for the common case of "target
  elements by class". Vue-style attribute-selector scoping
  (`.foo[data-c-XXXX]`) would need a more capable CSS parser (to
  understand where each compound selector ends before pseudo-
  classes / pseudo-elements) AND would mutate every element in
  the template with a bespoke attribute. Class-suffix keeps both
  the parser and the template rewrite dead simple.
- Trade-off (documented instead of solved): type selectors
  (`div`), IDs (`#foo`), and attribute selectors (`[data-x]`) are
  NOT scoped. Users who want per-component styling opt in by
  targeting classes — same posture as Svelte for the MVP. If
  demand appears, we can layer on Vue-style attribute selectors
  as a second strategy behind a flag on `<style scoped>` (e.g.
  `<style scoped=deep>`) without breaking the class-suffix
  default.

**Parser design**:

- Recursive-descent-ish, char-by-char (no regex, no crates).
  `parse_stylesheet` loops over rules; `parse_rule` dispatches on
  `@` vs other; `parse_qualified_rule` reads a selector prelude
  into a scratch `String`, transforms it via
  `transform_selector_list`, then copies the `{ ... }` body
  verbatim while tracking brace depth + strings + comments.
- `parse_at_rule` reads the at-rule name, copies the prelude
  (media queries / feature queries / URLs / keyframe names never
  need scoping), then dispatches on the name: `@media`,
  `@supports`, `@container` recurse into a nested stylesheet;
  every other at-rule (`@keyframes`, `@font-face`, `@page`,
  `@charset`, `@import`, `@namespace`) treats its body as opaque
  and copies verbatim.
- Selector transformer walks char-by-char. `.` followed by an
  ident-start char captures the ident and emits
  `.<ident>-<scope>`. `[...]` and strings and comments are
  skipped (copied verbatim without transformation). Parens are
  NOT skipped — selector-arg pseudos like `:not(.foo)` /
  `:is(.a, .b)` / `:has(.c)` want their inner class tokens
  scoped, and the naive walk handles them correctly by
  transparently transforming `.<ident>` regardless of whether
  we're inside parens. Non-selector-arg pseudos
  (`:nth-child(2n+1)`, `:lang(en)`) don't have `.` inside, so the
  walk doesn't touch them.
- `,` at the top level of a selector list splits the list;
  each selector is transformed independently and rejoined with
  `,`. Commas inside `(...)`, `[...]`, strings, and comments are
  respected via `capture_balanced_slice` / `capture_string_slice`
  / `capture_comment_slice` scratch helpers that track their own
  bounds.

**45 unit tests + 1 doctest** cover: empty input; whitespace only;
single class selector; compound class selectors (`.a.b`); class
names with hyphens and underscores and digits; multiple rules;
comma-separated selectors; each combinator (descendant, child,
adjacent sibling, general sibling); pseudo-class after class;
pseudo-element after class; `:not(.foo)` scoping the inner;
`:nth-child(2n+1)` passing through; `:is(.a, .b)` scoping each
inner independently; type selector NOT scoped; ID selector NOT
scoped; attribute selector body NOT transformed
(`[data-x="a.b"]`); class after attribute; declaration body
copied verbatim (`.x { content: ".foo"; ... }`); `url(foo.png)`
without quotes passing through in the declaration body; `@media`
recursing; `@supports` recursing; nested `@media` recursing all
the way; `@keyframes` opaque; `@font-face` opaque; `@import`
terminated by `;`; block comment before a rule preserved; block
comment inside a selector preserved; block comment inside a
declaration body preserved; unterminated rule body errors;
unterminated string in declaration errors; unterminated block
comment errors; unterminated media body errors; universal
selector passes through; descendant of type + class scopes only
the class; compound type + class scopes only the class; class
after ID gets scoped; multi-line rule preserves whitespace; dot
without ident-start preserved (`padding: .5em`); scope string
used verbatim.

**Design decisions worth naming**:

- **Pure standalone function, not a struct-based visitor**.
  The `apply_scope` entry point takes an `&str` and returns an
  owned `String`. No state escapes the call. Rationale: keeps
  the API surface minimal, makes it testable in complete
  isolation without setting up a `Component`, and lets 11.3.c
  wire it into `expand` however it wants (probably by calling
  `apply_scope(&component.style.css_raw, &scope_class)` and
  storing the result on the new `ExpandedStyle` variant).
- **Char-by-char, no regex, no crates**. Same posture as the HTML
  sub-parser in `view::parser`. CSS is small enough at this
  scope to hand-write, and the char-by-char walk gives precise
  error positions and predictable perf without dep bloat. If we
  ever need `:global(...)` handling or complex selector rewrites
  (Vue-style attribute selectors), we can either extend this
  parser or reach for `cssparser` (~40 KB, Servo project) as a
  targeted upgrade.
- **`@keyframes` treated as opaque**. Keyframe steps (`0%`,
  `100%`, `from`, `to`) look nothing like selectors and don't
  need scoping — a keyframe animation is a named entity that a
  component references by name, and the animation targets
  whatever element the CSS class was applied to. Recursing into
  the body would either mis-scope the steps as if they were
  class selectors (they're not) or need special-case handling
  for percentage tokens. Opaque is honest and correct.
- **Selector-arg pseudos handled by not special-casing parens**.
  The naive walk transforms `.<ident>` regardless of whether
  we're inside parens. For `:not(.foo)` / `:is(.a, .b)` /
  `:has(.c)` this is exactly the right behaviour — the inner
  classes should be scoped as part of the component's isolation.
  Non-selector-arg pseudos happen to not contain `.` so the walk
  leaves them alone. The corner case of `:nth-child(.foo)` (an
  invalid CSS construct we'd nonetheless transform) is
  acceptable — the CSS was broken before we touched it.
- **At-rule prelude captured with balanced brackets + strings**.
  `@media (min-width: 800px)`, `@supports (display: grid)`,
  `@container (min-width: 400px)`, `@import url("foo.css")`,
  `@charset "utf-8"` all need their parens / strings respected
  while scanning for the terminating `;` or `{`. The
  `copy_at_rule_prelude` helper handles this by delegating to
  `copy_balanced_body` and `copy_string_body`, which also handle
  nested comments.
- **`is_class_start` refuses digits and other punctuation**. A
  `.` followed by a digit (`.5em` inside a declaration, or
  `.5` at the start of a selector — the latter is invalid CSS)
  does NOT trigger class transformation. This keeps `padding:
  .5em` in a declaration body from producing `padding:
  .5em-<scope>` mid-value. (Declaration bodies are copied
  verbatim anyway, but the selector transformer applies the same
  rule for consistency.)

**Verification** (delta at 11.3.b over 11.3.a's baseline of 3421
unit + 3557 with `--features lsp`): `cargo test --lib` green
(3466 total, +45 new in `view::css_parser::tests`), `cargo test
--lib --features lsp` green (3602 total, same +45 mirrored),
`cargo test --doc view::` green (1 new doctest for the
`apply_scope` example), `cargo fmt --all --check` clean, `cargo
clippy --lib --tests --bins -- -D warnings` clean, `cargo clippy
--lib --tests --bins --features lsp -- -D warnings` clean. Full
`cargo test` suite recommended before tagging a release that
includes this change; deferred here since 11.3.b is internal to
`src/view/css_parser.rs` with no user-facing surface (still no
wiring into the `.fitzv` compilation entry point until 11.3.c +
11.5).

**Deuda residual** (does NOT block 11.3.c):

- **`:global(...)` escape hatch**. A user with a scoped block
  who wants one rule to leak outside (targeting descendants of a
  slotted element, cross-component theming, etc.) has no way
  today. Vue and Svelte both let a scoped block target beyond
  its component via `:global(...)`. Refinable in a follow-up:
  the walker learns "if we're inside `:global(...)`, skip the
  `.<ident>` transformation AND drop the `:global(...)` wrapper
  from the output". Small change to the selector transformer,
  no ripple to expand or the template rewrite.
- **Vue-style attribute-selector scoping as an alternative**. The
  class-suffix strategy trades type-selector scoping for
  simplicity. A future `<style scoped=deep>` opt-in could invoke
  a different transformer that adds `[data-c-XXXX]` to each
  compound. The `StyleKind` enum from 11.3.a would grow a third
  variant (or `Scoped` would gain a strategy field). Refinable
  behind demand.
- **CSS Modules-style class rewriting**. Distinct from scoping —
  the user writes `.card` and the compiler emits a fully
  synthetic class name (`_XXXX_card`) that the template
  references via a compile-time-generated JS object. Not in
  Fitz's scope today; the interop path with existing CSS
  Modules workflows would need its own design.
- **Precise position mapping of transforms back into the
  `.fitzv` source**. The `CssParseError.pos` is a char offset
  into the CSS blob, not a `Loc` in the `.fitzv` file. 11.3.c
  will map the base offset when wiring into `expand`. Full
  per-transform tracking (e.g. "the `.card` at line 5 col 10
  became `.card-c-XXXX`") stays deferred — no consumer needs
  it yet.
- **`@layer` at-rules recurse or opaque?** The MVP treats
  `@layer` as opaque (not in the recurse allowlist). If a user
  wraps their scoped rules inside `@layer components { ... }`,
  the inner rules will NOT be scoped. Refinable by adding
  `"layer"` to `at_rule_nests_selectors` if demand appears.

No other files touched by 11.1 / 11.2.a / 11.2.b mini-commits
1/2/3 or the §7 follow-up or 11.2.c mini-commits 1/2/3 or
11.3.a. `docs/fase-11-plan.md` gets the row update in §6 (11.3
row carries "mini-commit 11.3.b CLOSED 2026-07-14" annotation)
and this section.

---

## 9.k Files touched by 11.3.c — wire scoping in expand + template class-attr rewrite

Third and last mini-commit inside 11.3. **Closes 11.3 entire** —
`<style scoped>` blocks now flow through the pipeline as
scoped-and-rewritten CSS with a matching per-component class
prefix injected into every element's `class` attribute in the
template. `<style global>` blocks flow through as pure
passthrough (verbatim CSS, no template rewrite). The pipeline is
now `parse → expand (with scoping) → check`, with 11.4 / 11.5 to
come to actually render the result.

**Files touched**:

- `src/view/expand.rs` grew from ~830 LoC to ~1650 LoC (+21 unit
  tests, total 50 in `view::expand::tests`). Key changes:
  - `ExpandedComponent.style` type changes from
    `Option<RawStyle>` to `Option<ExpandedStyle>` — the raw
    passthrough is gone, replaced by the processed form.
  - New public `enum ExpandedStyle { Scoped { css_scoped,
    scope_class, loc }, Global { css, loc } }`. Doc comments
    document each variant's semantics + the naming rule for
    `scope_class`.
  - `expand_component` now grabs `template.as_mut()` and threads
    it through the new `process_style(...)` helper AFTER
    `expand_template` has run, so the class-attr rewrite walks
    the already-expanded tree.
  - New helper `process_style(raw, component_name, template) ->
    ExpandResult<Option<ExpandedStyle>>`: dispatches on
    `StyleKind`. For `Scoped`, synthesises the scope class, runs
    the CSS body through `apply_scope`, and (if a template is
    present) mutates it via `rewrite_class_attrs_in_template`.
    For `Global`, copies the CSS body verbatim; no template
    mutation.
  - New helper `synth_scope_class(component_name, css_raw) ->
    String`: shape `<component-kebab>-c-<8hex>`. Deterministic
    across runs — same name + same CSS → same class.
  - New helper `to_kebab_case(name) -> String`: CamelCase →
    kebab-case, `_` becomes `-`, non-alphanumerics dropped,
    empty result falls back to `"component"` so the class is
    always non-empty.
  - New helper `fnv1a_hash_8_hex(input) -> String`: FNV-1a 64-bit
    truncated to the low 32 bits, formatted as 8 hex chars.
    Zero deps.
  - New helper `rewrite_class_attrs_in_template(nodes,
    scope_class)`: recursive walker over the ExpandedTemplateNode
    tree. Descends into Element children, If then + else
    branches, and For bodies. For each Element with a static
    `class` attribute, transforms the value via
    `rewrite_class_value`.
  - New helper `rewrite_class_value(original, scope_class) ->
    String`: splits on ASCII whitespace, keeps the originals
    first, then appends each suffixed variant with a space
    separator. Empty / whitespace-only inputs pass through.
  - New helper `css_parse_error_to_expand(err, loc,
    component_name) -> ExpandError`: shifts a `CssParseError`
    into an `ExpandError` whose `context` names the component
    and the `<style scoped>` block. Precise offset mapping (turn
    `err.pos` into a `Loc` inside the CSS blob) stays deferred —
    same debt as the other blob-parsers in `view/`.
  - The 21 new unit tests cover: scoped style produces
    `ExpandedStyle::Scoped` with the correct `-<scope_class>`
    suffix inside the CSS; global style produces
    `ExpandedStyle::Global` with CSS verbatim; no style produces
    `None`; determinism (same input → same class); different CSS
    body → different class; different component name →
    different class; CamelCase name kebab-cases to
    `login-form-c-...`; all-lowercase name stays as-is; element
    with single class gets suffixed variant; element with
    multiple classes gets each suffixed independently; element
    without a class attribute stays unchanged; element with
    interpolated `class="{dyn}"` stays as an Interpolation attr
    (documented limitation); class rewrite recurses into
    Element children; class rewrite recurses into both If
    branches; class rewrite recurses into For bodies; Global
    style does NOT trigger template class rewrite; no style
    block leaves template untouched; malformed scoped CSS
    surfaces an ExpandError with the component context; scoped
    style with no template still expands the style (rewrite is
    a no-op with `None` template); the free
    `rewrite_class_value` helper normalises multi-space input;
    empty / whitespace-only inputs preserve.
- `src/view/mod.rs` — one line adding `ExpandedStyle` to the
  `pub use expand::{...}` list so external consumers (11.4 /
  11.5) can pattern-match on the scoped vs global shape.

**Design decisions worth naming**:

- **Class-suffix strategy over Vue-style attribute selector**.
  Decided at 11.3 kick-off — see §9.i and §9.j. The template
  rewrite in 11.3.c preserves original class names and appends
  suffixed variants, so external JS querying `.card` keeps
  working. CSS `.card { ... }` becomes `.card-<scope>`, and the
  template's `class="card"` becomes `class="card card-<scope>"`
  — the suffixed variant is what the scoped CSS actually matches.
- **Enum `ExpandedStyle` over struct with `Option<scope_class>`**.
  A struct would need `scope_class: Option<String>` to accommodate
  the Global variant, leaking the abstraction. The enum makes
  the invariant "Global has no scope class" a type-level fact.
  Small win, worth the extra pattern-match at each consumer.
- **`scope_class` synthesised from `<component_name>::<css_raw>`,
  not from a random UUID**. Determinism is load-bearing: the
  same source file rebuilt on a different machine produces the
  same scope class, so caches (rustc, incremental hot reload,
  eventual SSR output) don't invalidate spuriously. FNV-1a is
  fast, tiny, deterministic, and non-cryptographic — perfect
  fit here.
- **8 hex chars of the FNV-1a low 32 bits (not 16 of the full
  64)**. 4 billion permutations vs. typically <100 components
  per file — the birthday-paradox collision probability is
  vanishing. 8 chars keep the class name readable in DevTools
  and don't inflate the compiled HTML. If we ever hit a
  collision in practice, we can bump to 16 in a one-line change.
- **Template rewrite runs after `expand_template`, not
  interleaved**. The rewrite walks the already-expanded tree,
  which means it works uniformly for Elements, If children,
  For children, and future template constructs. Interleaving
  with the expand pass would spread the scoping logic across
  five match arms and make the ordering harder to reason about.
  The trade-off is one extra pass over the tree — negligible
  for the sizes we're targeting.
- **Rewrite preserves originals before appending suffixes**.
  `class="card title"` becomes `"card title card-<s> title-<s>"`,
  not `"card-<s> title-<s>"`. External CSS / JS querying `.card`
  keeps working — the compiler adds isolation without breaking
  the source-visible class names. Trade-off: bigger `class`
  attribute strings in the output HTML. Acceptable.
- **Interpolated `class="{expr}"` NOT rewritten**. The
  transformer only touches `Attr::Static` with name `"class"`.
  `Attr::Interpolation` passes through unchanged. Rationale:
  we don't know at compile time what the runtime string will
  be, so we can't append suffixes to the right tokens. A future
  refinement could emit a runtime helper that suffixes on the
  fly, but for MVP the user includes the suffix manually in
  the interpolation expression when they need scoped dynamic
  classes.
- **Global style is a pure passthrough — no CSS transform, no
  template rewrite**. That's the semantic promise of `global`:
  the user opts out of isolation. No hidden magic.
- **`css_parse_error_to_expand` remaps char-offset errors to
  the style block's `Loc`**. Users see the CSS error at the
  `<style scoped>` block's location in the `.fitzv` file, plus
  the char offset inside the CSS body for further diagnosis.
  Precise line/column mapping inside the CSS blob is the same
  debt as the rest of 11.2's blob parsers (blob-local + best-
  effort base offset) and stays deferred.

**Verification** (delta at 11.3.c over 11.3.b's baseline of 3466
unit + 3602 with `--features lsp`): `cargo test --lib` green
(3487 total, +21 new in `view::expand::tests`, so total
`view::expand::tests` = 50), `cargo test --lib --features lsp`
green (3623 total, same +21 mirrored), `cargo fmt --all --check`
clean, `cargo clippy --lib --tests --bins -- -D warnings` clean,
`cargo clippy --lib --tests --bins --features lsp -- -D warnings`
clean. Full `cargo test` suite (cli_e2e, openapi_e2e,
compile_e2e) recommended before tagging a release that includes
this change; deferred here since 11.3.c is internal to
`src/view/` with no user-facing surface — the `.fitzv`
compilation entry point still lives behind Phase 11.5 per the
plan.

**Deuda residual** (does NOT block 11.4 or 11.5):

- **Interpolated `class` attributes** stay as-is. Users who
  want dynamic scoped classes include the suffix manually
  today; a future refinement could emit a runtime helper. Not
  urgent — full interpolation is already an escape hatch.
- **`class` attributes with mixed static / interpolated parts**
  (`class="btn btn-{kind}"`) are already rejected at the raw
  parser level (see AST doc-comment on `Attr` — "POC allows
  only fully-static or fully-interpolated values"). Whenever
  the raw parser learns mixed parts, the rewriter here will
  need matching support.
- **Type / ID / attribute selectors are NOT scoped**. Documented
  MVP trade-off; refinable behind a `<style scoped=deep>`
  opt-in that would swap `apply_scope` for a Vue-style
  attribute-selector rewriter. Would ALSO need the template
  rewrite to inject a `data-c-XXXX` attribute on every element
  (matching change here). Sized as a dedicated mini-commit if
  demand appears.
- **`:global(...)` escape hatch** is not yet parsed by
  `apply_scope`, so `.card :global(.legacy) { ... }` inside a
  scoped block will get the `.legacy` scoped (incorrect). Same
  deuda as 11.3.b. When `apply_scope` grows the escape hatch,
  the template rewrite doesn't change — the template still
  doesn't know or care about `:global(...)`.
- **Precise position mapping of CSS errors into the `.fitzv`
  file**. `CssParseError.pos` is a char offset into the CSS
  blob. Today we surface the style block's `Loc` (which points
  at `<style scoped>` in the `.fitzv` file) plus the offset for
  further diagnosis. Turning the offset into a line + column
  inside the blob is the same debt as the other blob parsers
  in `view/`, tracked in §7.
- **No SSR / codegen consumer yet**. The `ExpandedStyle`
  variants are shipped but no code path in the `fitz` binary
  routes to them — that's the promise of 11.4 and 11.5. Tests
  cover the shape end-to-end; a downstream consumer (Rust SSR
  render fn, or a WASM / JS emitter) will read
  `ExpandedComponent.style` + walk the template and pick up
  the suffixed classes directly.

No other files touched by 11.1 / 11.2.a / 11.2.b mini-commits
1/2/3 or the §7 follow-up or 11.2.c mini-commits 1/2/3 or
11.3.a or 11.3.b. `docs/fase-11-plan.md` gets the row update in
§6 (11.3 row carries "CLOSES 11.3 entire" annotation) and this
section.

---

## 9.l Decision recorded by 11.4.a — client target: WASM-first, hand-rolled `wasm-bindgen` (feature `client-wasm`)

First sub-commit of 11.4. **Research + decision only — zero code
touched, zero tests added, docs-only commit.** The output IS this
§9.l section plus the refreshed row for 11.4 in §6. Confirms the
architectural direction already committed in `docs/stack.md` v1
(2026-07-14, lines 99-101), and picks the concrete Rust→WASM
approach among five candidates before 11.4.b starts writing the
emitter.

**Decision**: **Approach A2 — hand-roll `wasm-bindgen` + `web-sys`
directly**. Feature-gated as opt-in `client-wasm`; `cargo build`
default remains standalone with the current dep tree. The emitter
lives in `src/view/codegen_wasm.rs` (new, added in 11.4.b),
preserving Invariant 4.

**Files touched by 11.4.a**:

- `docs/fase-11-plan.md` — the row for 11.4 in §6 expands with the
  four sub-commit breakdown + the closed 11.4.a annotation. This
  section (§9.l) captures the analysis so the decision has a
  place to live and 11.4.b/c/d can reference it.

That's it. No `src/` changes, no `examples/` changes, no
`Cargo.toml` changes yet — the deps for `client-wasm` land in
11.4.b when the emitter starts calling them.

**Data collected during 11.4.a research**:

Bundle sizes for a hypothetical counter component, from public
benchmarks + framework documentation (all measurements are
gzipped transfer size unless noted):

| Path | Baseline | Counter estimate | Sources |
|---|---|---|---|
| `wasm-bindgen` hello world | ~15 KB WASM | ~15-25 KB gzipped after `opt-level=z` + `wasm-opt` | [sendilkumarn.com wasm-bindgen post](https://sendilkumarn.com/blog/wasm-bindgen); [rustwasm/wasm-bindgen#2856](https://github.com/rustwasm/wasm-bindgen/issues/2856); [Leptos book — Optimizing WASM Binary Size](https://book.leptos.dev/deployment/binary_size.html) |
| Solid.js runtime + counter | ~7 KB | ~7-10 KB gzipped | [pkgpulse.com Solid vs Svelte](https://www.pkgpulse.com/compare/solid-js-vs-svelte) |
| Svelte 5 compiled counter | ~2-3 KB | ~2-10 KB gzipped for full production build | [Svelte 5 bundle size discussion](https://github.com/sveltejs/svelte/discussions/11214); [Bundlephobia svelte](https://bundlephobia.com/package/svelte) |

Real ratio between WASM and JS-vanilla for the counter: **~2-5x**,
not the "~10-20x" often quoted in Reddit / HN threads. Optimized
WASM sits in the "acceptable" band for SPAs, admin panels,
dashboards, LiveViews-augmented sites — the 90% of real use
cases. The 10% where JS-vanilla still wins (edge functions with
< 1 MB budgets, ultra-low-power mobile) is exactly the nicho
that `docs/stack.md` reserves for the JS-vanilla secondary target.

**Approaches evaluated during 11.4.a**:

| # | Approach | Bundle base | Deps added | Invariant 4 | Verdict |
|---|---|---|---|---|---|
| **A1** | Delegate to Leptos/Sycamore/Dioxus/Yew | 30-100 KB gz | Framework crate + transitive tree | ⚠️ debatable — compiling to a framework's code | ❌ ecosystem lock-in; violates `docs/stack.md` L24 "Fitz por sí solo puede resolver el rango entero" |
| **A2** | Hand-roll `wasm-bindgen` + `web-sys` directly | 15-25 KB gz | `wasm-bindgen`, `web-sys`, `console_error_panic_hook` under opt-in feature `client-wasm` | ✅ 100% inside `src/view/codegen_wasm.rs` | ✅ **elegido** |
| **A3** | In-tree runtime crate (Svelte-style, shared signals + DOM patching) | ~10 KB gz base + N KB per component | Same as A2 plus a Fitz-published micro-crate | ✅ | 🟡 refinement of A2 — not the arranque; open a sub-phase later if `N` components bloat the bundle (measurable when kanban of 11.6 ports) |
| **B1** | Emit hand-written JS from the AST | 2-10 KB gz | Zero Cargo deps (emitter code + string constants) | ✅ | 🟡 queued for 11.9+ as the secondary target promised in `docs/stack.md`; NOT built in 11.4 |
| **B2** | Delegate to Svelte/Solid compiler | 2-10 KB gz | Requires `node.js` in the build machine | ❌ | ❌ same lock-in as A1 plus `npm` dep in the build toolchain |

**Rationale for picking A2** (in priority order):

1. **Respects `docs/stack.md` v1**. WASM-first was committed on
   2026-07-14 in the same doc where the architectural invariants
   for the stack were written. Re-litigating that commitment
   inside 11.4.a would ask for evidence 11.4.a explicitly does
   NOT have — real bundle measurements come from 11.4.c on the
   actual emitter, not from external framework benchmarks.
2. **Preserves the "Fitz por sí solo" philosophy**. The `fitz`
   binary compiles `.fitzv` to WASM without external
   frameworks, without `npm`, without `node`. A user with only
   `fitz` installed can build a WASM client bundle.
3. **Preserves Invariant 4**. The emitter lives entirely in
   `src/view/codegen_wasm.rs`. The feature `client-wasm` is
   opt-in — `cargo build` default keeps the current dep tree
   untouched, meaning the `~370` guide/course/TaskHub examples
   plus the 11 boilerplates keep compiling with zero changes.
4. **Bundle 15-25 KB gz is acceptable for the 90% of cases**.
   For the 10% of pathological cases (edge functions, ultra-
   low-power mobile), JS-vanilla via B1 remains queued for
   11.9+.
5. **A3 is a refinement, not a competitor**. Start with A2 (each
   component emits its own DOM binding code, redundant across
   components). If bundle sizes bloat when N components stack
   up (measurable when the kanban port lands in 11.6), refactor
   by extracting shared signals + DOM patching into an in-tree
   runtime crate. The refactor is transparent to user code — it
   changes the emitter internals only.
6. **Zero transitive coupling today**. `cargo tree -e normal`
   confirms neither `wasm-bindgen`, `web-sys`, nor `js-sys` are
   in the current dep tree — the feature `client-wasm` is 100%
   additive with no accidental exposure via existing crates.

**Measured gate for 11.4.b / 11.4.c**:

If the counter POC produced by A2 exceeds **40 KB gzipped** (2x
the projected ceiling), 11.4.b PIVOTs to B1 (JS-vanilla emitter)
and 11.4.c re-runs the demo. In that scenario, `docs/stack.md`
lines 99-101 get updated with the measured data. This is a real
gate, not a rubber-stamp — the number is measured, recorded in
§9.n at close of 11.4.c, and drives the pivot if triggered.

**Cargo.toml delta that 11.4.b will add** (for reference, not
edited in this commit):

```toml
# Fase 11.4.b — target client WASM. Feature `client-wasm` opt-in:
# the `fitz` binary default does NOT link wasm-bindgen; only
# `fitz build --target wasm` (Phase 11.5) or the WASM emitter
# tests activate these deps.
wasm-bindgen = { version = "0.2", optional = true }
web-sys = { version = "0.3", optional = true, features = [
    "Document", "Element", "HtmlElement", "Event", "EventTarget",
    "Node", "Text", "Window",
] }
console_error_panic_hook = { version = "0.1", optional = true }

[features]
client-wasm = ["dep:wasm-bindgen", "dep:web-sys", "dep:console_error_panic_hook"]
```

`web-sys` follows "pay-only-for-what-you-use" — only the DOM
classes the emitter actually touches get feature-gated in. The
initial set above covers the counter subset; 11.4.b / future
sub-phases expand it (add `MouseEvent`, `InputEvent`,
`HtmlInputElement`, etc.) as the template dialect demands.

**Deferred / queued items (NOT built in 11.4)**:

- **B1 — JS-vanilla emitter**. Queued for a later sub-phase
  (likely 11.9) when either real demand for <5 KB bundles
  surfaces OR the WASM path measurably fails the 40 KB gate on
  the counter. `ExpandedComponent` is target-agnostic —
  a second emitter in `src/view/codegen_js.rs` does NOT
  require refactoring the AST or `check`/`expand`.
- **A3 — in-tree runtime crate**. Refactor of A2, gated by
  measured bundle-bloat when `N` components stack up. Opens
  post-11.6 (kanban ported) when we have real component counts
  to measure against.
- **11.5 CLI wiring** (`fitz build --target wasm`). Stays in
  11.5. 11.4 assumes the emitter is invoked from a test / bin
  dedicated to `view::codegen_wasm`, NOT through `fitz build`.

**Design decisions worth naming**:

- **A1 (framework delegation) rejected as arranque**. Two
  reasons: (1) Leptos/Sycamore/Yew/Dioxus each have their own
  release cadence + breaking-change history that Fitz would
  inherit if it emitted to their component model. (2) Compiling
  to third-party framework code makes Invariant 4 harder to
  defend — a bug in the emitter that produces malformed Leptos
  code would first surface as a Leptos compiler error, not a
  Fitz error. The isolation story gets fuzzier.
- **B1 (JS-vanilla) NOT dropped, just queued**. The AST post-
  expand is target-agnostic; a JS emitter that reads
  `ExpandedComponent` and emits vanilla JS is a straightforward
  addition when demand justifies it. The reason it isn't
  arranque is that `docs/stack.md` already committed to WASM
  as primary — starting with JS would require reversing that
  commitment on evidence 11.4.a doesn't have.
- **A3 (in-tree runtime crate) NOT arranque either**. Starting
  with A3 pays the coordination cost of a second published
  crate before we know if the shared runtime pattern pays off
  for Fitz's specific reactivity model. A2 is refactorable to
  A3 later without user-facing breakage — the emitter internals
  just change from "each component embeds all bindings" to
  "each component references shared runtime helpers".
- **`console_error_panic_hook` included in the feature bundle**.
  Without it, WASM panics silently no-op or produce cryptic
  browser errors. Cost is ~2-3 KB gzipped, worth every byte for
  the DX during 11.4.b/c development. Users of the production
  build can toggle it off in a future refinement if bundle size
  becomes contested.
- **`opt-level = "z"` + `wasm-opt` policy for 11.4.b**. The
  emitter itself doesn't set these — they're `Cargo.toml`
  profile settings the user's build applies. 11.4.b documents
  the recommended profile settings in the emitter's rustdoc,
  and 11.4.c measures with them applied.
- **Zero code / zero tests in 11.4.a is deliberate**. This
  sub-commit is pure decision. Splitting the research from the
  POC keeps the git history readable: the future 11.4.b commit
  becomes "implement the decision recorded in §9.l" rather
  than "research + implement together, no clear record of why
  we chose A2".

**Verification pre-close of 11.4.a**:

- Row 11.4 in §6 refreshed with sub-commit breakdown; the
  markdown table still renders (no broken pipes, no orphan
  columns).
- §9.l inserted between §9.k and §10; heading level is
  consistent with the rest of §9.
- `grep -n "^## 9\." docs/fase-11-plan.md` shows the section
  index reads §9.a through §9.l in order.
- No `src/` files touched, no `Cargo.toml` touched, no
  `examples/` touched — verified by `git diff --stat` showing
  only `docs/fase-11-plan.md` in the changeset.
- Cero tests to run: this is a docs-only commit. Invariants 1-5
  hold trivially because no code changed. The classic `.fitz`
  pipeline, the `.fitzv` pipeline post-11.3.d, the boilerplates,
  and the smoke `GUIDE_EXAMPLES_COMPILE` are all unchanged by
  this sub-commit.

**Debt residual visible from 11.4.a** (opens deuda entries that
11.4.b/c must respect):

- **Scoped CSS injection strategy in WASM**. `ExpandedStyle::Scoped`
  carries pre-suffixed CSS from 11.3.c. The WASM emitter has to
  inject the `<style>` block into `document.head` at mount
  time, deduplicating by `scope_class` so N mounted instances of
  the same component share one `<style>` block. 11.4.b
  documents the canonical strategy inline.
- **`Value::Secret` in interpolations**. The checker of 11.2.b
  already blocks Secret at type-check time. The WASM emitter
  assumes that guarantee — zero defensive runtime handling of
  `Secret` in template rendering. If a Secret ever reaches the
  emitter it means the checker was bypassed, and the emitter's
  behavior in that case is intentionally undefined (the safe
  answer is to redact via the check, not the emit).
- **`<slot />` opaque handling in 11.4**. The emitter must emit
  a DOM marker (probable shape: a `<template
  data-fitz-slot="name">` element or a marker comment) without
  resolving the composition. Composition resolution lands in
  11.5 alongside `<Child prop="v" />` syntax. 11.4.b's test
  suite includes a smoke test that a component with a `<slot />`
  compiles cleanly (opaque marker present in output) without
  actually mounting a child.
- **Interpolated `class="{expr}"` unchanged from 11.3.c
  limitation**. Dynamic class values do NOT get the scope
  suffix appended. The WASM emitter preserves this limitation
  faithfully — dynamic class attributes are set as raw strings
  from the interpolation. Users who want scoped dynamic
  classes concatenate the suffix manually inside the expression
  (a workaround, escape hatch documented in 11.4.c).
- **`docs/stack.md` cross-link**. When 11.4.d closes the
  sub-phase, `docs/stack.md` gets a footnote crediting §9.l
  with the "hand-rolled A2" refinement of the WASM-first
  commitment. The footnote makes future readers of stack.md
  find the concrete decision without re-deriving it.

No other files touched by 11.4.a. The next sub-commit (11.4.b)
will add `src/view/codegen_wasm.rs`, the `client-wasm` feature
plus deps in `Cargo.toml`, and the initial suite of emitter unit
tests.

## 9.m Files touched by 11.4.b — POC WASM emitter (`src/view/codegen_wasm.rs`)

Second sub-commit of 11.4. Implements the decision recorded in
§9.l: hand-rolled `wasm-bindgen` + `web-sys` emitter for `.fitzv`
components, feature-gated behind opt-in `client-wasm`. Consumes an
`ExpandedComponent` post-11.3.c pipeline (parse → expand →
optional check) and produces Rust source code that, when built for
`wasm32-unknown-unknown` with the `client-wasm` feature active,
mounts the component into a DOM node.

**Files touched**:

- `src/view/codegen_wasm.rs` — **new module, ~1500 LoC + 23 unit
  tests** in `view::codegen_wasm::tests`. See below for the
  public API + subset covered + design decisions.
- `src/view/mod.rs` — two lines: `pub mod codegen_wasm;` and
  `pub use codegen_wasm::{emit_component, emit_module, EmitError,
  EmitResult};`. External consumers (11.4.c smoke, 11.5 CLI) can
  now call `view::emit_component(...)` directly.
- `Cargo.toml` — +14 lines: three new `optional = true` deps
  (`wasm-bindgen = "0.2"`, `web-sys = "0.3"` with features
  `Document`/`Element`/`Event`/`EventTarget`/`HtmlElement`/
  `HtmlHeadElement`/`Node`/`Text`/`Window`, and
  `console_error_panic_hook = "0.1"`) plus the feature
  declaration `client-wasm = ["dep:wasm-bindgen", "dep:web-sys",
  "dep:console_error_panic_hook"]`. Zero effect on the default
  `cargo build` — the dep tree base stays byte-for-byte identical
  to v0.20.1.

**Public API (D2 of 11.4.b)**:

```rust
pub struct EmitError { pub message: String, pub context: String }
pub type EmitResult<T> = Result<T, EmitError>;

/// Emit Rust source for ONE component (struct + impl + optional
/// style helper). Does NOT emit `use` imports.
pub fn emit_component(component: &ExpandedComponent) -> EmitResult<String>;

/// Emit a full Rust module ready for `wasm-pack build`: preludio
/// with imports + N components concatenated.
pub fn emit_module(file: &ExpandedViewFile) -> EmitResult<String>;
```

Both mirror the posture of `view::css_parser::apply_scope`:
stringly-typed input/output, pure functions, tests validate
substrings of the emit.

**What the emitter produces per component**:

- `pub struct <Name> { <state fields wrapped in RefCell>...,
  root: RefCell<Option<HtmlElement>> }`.
- `impl <Name>` with:
  - `pub fn new() -> Rc<Self>` — instantiates with the parsed
    defaults from `ExpandedStateField.default`.
  - One `fn <event>(self: &Rc<Self>)` per declared handler,
    body lowered from the classic Fitz `Vec<Stmt>` and tail-
    called by `self.render()` so any state mutation triggers a
    re-render.
  - `pub fn mount(self: &Rc<Self>, selector: &str) -> Result<(),
    JsValue>` — resolves the CSS selector, casts to
    `HtmlElement`, injects the style helper (when the component
    has one), and kicks off the first `render()`.
  - `fn render(self: &Rc<Self>)` — clears the mount root's
    children and rebuilds the DOM subtree from the expanded
    template using the current state.
- When the component has a `<style scoped>` or `<style global>`
  block, a free `fn __inject_style_<Component>_<sanitized>()`
  that appends a `<style>` element to `document.head` exactly
  once (dedup via `AtomicBool` for scoped; scoped rules only
  apply to the component thanks to 11.3.b/c already suffixing
  the CSS selectors + template class attributes).

**Scope of the POC (D3 of 11.4.b — strictly conservative)**:

Cubre exactamente lo necesario para el counter demo de 11.4.c:

- State fields: only `Int` primitives with `Expr::Int` literal
  defaults (`state { count: Int = 0 }`).
- Event handlers: sync, zero params, body = a single
  `Stmt::Assign` whose target is a state field ident and whose
  RHS is `Expr::Int` / `Expr::Ident` (referencing a state field)
  / `Expr::BinOp` with arithmetic ops (Add/Sub/Mul/Div/Mod).
- Template nodes: `Text`, `Element` (static tag), `Interpolation`
  with an `Expr::Ident` referencing a state field, `Static`
  attributes (`class="..."` etc), `Event` attributes with
  `@click` mapped to the DOM `click` event.
- Styles: `Scoped` (dedup helper) or `Global` (single-shot
  injection). No style = no helper emitted.

Rechazos explícitos con `EmitError` que cita la sub-fase donde se
cierra:

- `{#if cond}` / `{#for x in xs}` → "deferred to Phase 11.4.c".
- `<slot />` → "deferred to Phase 11.5".
- Interpolated attrs (`class="{expr}"`) → "deferred to 11.4.c"
  (paired with the 11.3.c documented limitation of no scope-suffix
  on dynamic classes).
- `@input` / `@change` / `@submit` and any non-`@click` event →
  "only `@click` supported in Phase 11.4.b (deferred to 11.4.c)".
- State field type `Str` / `Bool` / `Float` / `List<...>` /
  `Map<...,...>` / nominal / nullable → "deferred to Phase 11.4.c".
- Handler with parameters or `async` → "deferred to Phase 11.5".
- Non-arithmetic BinOps (comparisons, logical, bitwise) inside
  an event body → "deferred to Phase 11.4.c".
- `Stmt::Assign` where target is `AssignTarget::Field` /
  `AssignTarget::Index`, or any Stmt kind other than `Assign` →
  "deferred to Phase 11.4.c".

Every rejection carries a `context` label that names the exact
component + field / handler / element where the misuse lives.

**Reactivity model (D1 of 11.4.b)**:

Naive re-render on state mutation. Every emitted event handler
tail-calls `self.render()`, which:

1. Borrows the mount root, no-ops if not yet mounted.
2. Removes every child of the root.
3. Grabs `document` and rebuilds the entire template subtree
   from scratch, reading state via `(*self.<field>.borrow())`.

Ineffective for large / high-frequency updates (1000-item list
refreshing at 60 fps would repaint N nodes per tick). Acceptable
for the counter / forms / dashboards with occasional updates. The
signals-based refinement (Solid/Leptos-style fine-grained
reactivity) is queued as the A3 refactor from §9.l, gated by
measured bloat when many components stack up in 11.6's kanban
port.

**Testing strategy (D4 of 11.4.b) — 23 unit tests**:

All tests validate substrings of the emitted string (parallel
bit-for-bit to `view::css_parser::tests`). Split roughly as:

- 8 tests around the counter shape (struct decl, `new()` with
  defaults, event handler signature + assign lowering + auto-
  render call, mount signature + style helper call, render
  clears + rebuilds, template element with scoped class attr,
  interpolation via `format!` + state borrow, event attr wires
  click via Closure + `add_event_listener_with_callback`).
- 3 tests around the style helper (scoped with `AtomicBool`
  dedup, global helper naming convention, no-style skips the
  helper entirely).
- 6 rejection tests (Str state field / `{#if}` / `{#for}` /
  `<slot />` / `@input` / handler with params).
- 2 arithmetic lowering tests built directly from AST nodes
  because the view lexer does NOT yet tokenise `+`/`-` (see
  Deuda residual below).
- 1 test for `emit_module` covering the preamble + component
  concat.
- 3 helper tests (sanitize_ident, rust_string_literal, and
  the emit_no_style negative case).

Tests use `parse_expand(source)` (parse → expand pipeline) for
shapes the view lexer accepts, and direct `ExpandedComponent`
construction for the arithmetic subset that requires operators.

**Design decisions worth naming**:

- **`Closure::forget()` leaks the closure per-event-listener,
  per-render**. Cada `render()` rebuilds N event listeners; each
  one calls `.forget()` to keep the closure alive as long as the
  DOM element exists. Since `render()` removes all children first,
  the old element (and its click handler ref) becomes unreachable
  and JS's GC eventually reclaims them — but the Rust closure
  itself is `Box::leak`-ed via `forget()`, so the *Rust* side
  never drops. For a counter clicked 10× per session this leaks
  ~10 tiny closures; for a chat client that re-renders every
  second, over an hour that's ~3600 leaked closures. Acceptable
  for the POC; refinement lives in A3 (in-tree runtime crate
  with pooled closures per event kind) or by storing closures on
  the component struct and rebinding rather than re-creating.
- **`AsRef<EventTarget>` via Deref chain**. The emitter calls
  `element.add_event_listener_with_callback(...)` directly on
  the `Element` — wasm-bindgen models JS inheritance via `Deref`
  chains (`Element: Deref<Target = Node>`, `Node: Deref<Target =
  EventTarget>`), so the `EventTarget` methods are inherited.
  Simpler than explicit `.dyn_ref::<EventTarget>()` calls.
- **Var counter allocation** (`__el0`, `__t1`, `__interp2`,
  `__el3`, ...). The `RenderCtx` allocates unique names via a
  single monotonic counter across ALL node kinds; this reads
  slightly less prettily than per-kind counters (`__el0`, `__el1`,
  `__t0`, `__interp0` would be nicer to read in the emit output),
  but keeps the emitter code trivial. The test that asserted on
  a specific `__interp0` had to be relaxed to just check for
  `= format!("{}", (*self.count.borrow()));` (any var name).
- **Whitespace-only text nodes skipped**. `emit_text` drops nodes
  whose content is entirely whitespace — HTML collapses them
  anyway, and skipping keeps the emit smaller. Real Text nodes
  with content (e.g. `"reset"`, `"bump"`) still emit
  `create_text_node` + `append_child` as normal.
- **`rust_string_literal` uses `{:?}` formatting**. Produces a
  valid Rust string literal with the necessary escapes (quotes,
  backslashes, control chars). Good enough for the POC because
  the strings coming through the emitter (CSS bodies, static
  attr values, text nodes, tag names) are all authored by the
  Fitz user in the `.fitzv` source — no runtime interpolation
  from an untrusted source.
- **`sanitize_ident` replaces `-` with `_`, drops the rest**.
  The scope class from 11.3.c is `<component-kebab>-c-<8hex>`
  which contains hyphens; hyphens are not legal in Rust idents.
  Replace with underscore. Everything non-alphanumeric that
  isn't `-` gets silently dropped (defensive; should not happen
  in practice given the naming rules of 11.3.c).
- **`emit_component_impl` leaves the `impl <Name> {` block
  open** after emitting `new()`, and each subsequent helper
  (`emit_event_handlers`, `emit_mount_and_render`) writes its
  content into the same block, then `emit_mount_and_render`
  closes it with `}`. This flat write-in-order style avoids
  buffering + concatenation of separate strings; a small
  invariant that emit code stays lean.
- **Docs-only `#[cfg(test)]` cargo commands work in both
  feature modes**. The tests never call `wasm_bindgen` or
  `web_sys` types — they only inspect the emitted STRING. So
  `cargo test --lib` (default features) exercises all 23 tests
  without needing to link the WASM crates. `cargo build
  --features client-wasm` compiles the emitter itself under the
  same code path plus the WASM deps in scope, confirming the
  string the emitter produces AT LEAST refers to pinned versions
  of `wasm-bindgen`/`web-sys` that resolve in the crates.io
  index. Real end-to-end (`wasm-pack build` + browser mount)
  lives in 11.4.c.

**Verification pre-close of 11.4.b**:

- `cargo test --lib --release`: **3510 passed / 0 failed** in
  3.25s (3487 baseline pre-11.4.b + 23 new
  `view::codegen_wasm::tests::*`).
- `cargo build`: clean, dep tree byte-identical to v0.20.1.
- `cargo build --features client-wasm`: clean in 2m 40s cold
  (first run downloads `wasm-bindgen` 0.2, `web-sys` 0.3,
  `console_error_panic_hook` 0.1, `js-sys` 0.3 transitive).
- `cargo fmt --all --check`: clean.
- `cargo clippy --lib --tests -- -D warnings`: clean.
- `cargo clippy --lib --tests --features client-wasm -- -D
  warnings`: clean in 1m 42s.
- Invariants 1-5 (`docs/stack.md`) all hold: the 3487 pre-
  existing tests still green, boilerplates untouched, classic
  `.fitz` parser surface unchanged, emitter isolated in
  `src/view/`, verification suite unchanged.

**Debt residual visible from 11.4.b**:

- **View lexer arithmetic gap — CLOSED 2026-07-14 via §9.n
  follow-up**. Previously the view lexer did NOT tokenise
  `+`/`-`/`*`/`/`/`%`; any `.fitzv` source with an arithmetic
  operator inside an event body errored at lex time before the
  parser's `capture_balanced_body_raw` ran. Closed by adding the
  5 arithmetic Token variants to `src/view/lexer.rs` + serializer
  arms to `src/view/parser.rs::append_token_source` +
  `needs_space_before` audit (`=` also added for consistent
  spacing). See §9.n for the full breakdown. The 11.4.c counter
  demo can now `parse → expand → check → emit` end-to-end.
- **Closure leak via `.forget()`**. Documented in Design
  decisions above. Refinement path: A3 (in-tree runtime crate)
  with pooled closures per event kind, or reuse-closures-on-
  render by storing them on the component struct. Not urgent
  for POC.
- **Naive re-render is O(N) per state mutation**. Documented in
  Reactivity model above. Refinement queued for A3.
- **Var counter is shared across node kinds** (produces
  `__el0`, `__t1`, `__interp2`, `__el3`, ...). Cosmetic — the
  emit reads a bit less prettily than per-kind counters would.
  Refinable if any user complains about generated code
  readability.
- **`emit_module` preludio hardcoded**. The `use` list at the
  top of `emit_module`'s preludio (`RefCell`, `Rc`,
  `AtomicBool`, `wasm_bindgen::prelude::*`, `JsCast`, `Event`,
  `HtmlElement`) is a static string constant. If the emitter
  ever produces code that references types outside this fixed
  set (e.g. `MouseEvent` for the `@click` DOM event data,
  `HtmlInputElement` for `@input` handlers on `<input>`), the
  preludio needs to grow. 11.4.c will likely bump `web-sys`
  features + preludio as it wires the first non-`click` event.
- **No `#[wasm_bindgen(start)]` in `emit_module`**. The
  preludio + component + style helpers land, but the browser
  entry point (the fn that runs on page load, creates a
  `Counter::new()`, and calls `.mount("#app")`) is expected to
  be authored by the user in 11.4.c or emitted by the 11.5 CLI.
  Keeping it out of `emit_module` avoids forcing an opinion
  about how many components are mounted, in what order, into
  what selectors.
- **Cross-link to §9.l**. §9.l names the A3 (in-tree runtime
  crate) refinement path but does not schedule it. 11.4.b's
  measured emit output size (per-component redundancy) is the
  first real data point — TBD after 11.4.c bundle size
  measurement whether A3 is worth pulling forward.

No other files touched by 11.4.b. The next sub-commit (11.4.c)
must first close the view lexer arithmetic gap (dedicated pre-req
mini-commit) before it can build the runnable counter demo end-
to-end.

## 9.n Files touched by view-lexer arithmetic follow-up (closes the operator gap blocking 11.4.c)

Dedicated pre-req mini-commit between 11.4.b and 11.4.c, mirroring
the shape of §9.e (view-lexer §7 follow-up between 11.2.b.3 and
11.2.c.1). **Closes the debt residual documented in §9.m** —
event bodies with arithmetic operators (`count = count + 1`,
`n = n * 2`, etc.) now lex, round-trip through
`capture_balanced_body_raw`, and re-lex successfully via the
classic Fitz parser inside `expand::parse_statements_from_source`.
The 11.4.c counter demo can now be built end-to-end from a
`.fitzv` source without hitting a "unexpected character `+`"
error at lex time.

**Files touched**:

- `src/view/lexer.rs` — five new `Token` variants (`Plus`,
  `Minus`, `Star`, `Slash`, `Percent`) with matching `Display`
  impl arms and five new arms in the main match block of `run()`
  emitting them from the corresponding chars. The `/` arm sits
  AFTER `skip_ws_and_comments` so `//` line comments are still
  intercepted first — a single `/` reaches the main match only
  when it's not the start of a comment. Comments preserved
  entirely (line comment content NEVER appears as a `Token::Ident`
  in the emitted stream — regression test covers that). +3 unit
  tests in `view::lexer::tests`:
  - `arithmetic_ops_lex_as_dedicated_tokens` — one component
    with all 5 ops in one body, confirms each token appears in
    the stream.
  - `line_comment_still_works_after_slash_arm` — regression
    ensuring exactly one `Slash` is emitted for `a / b // hi`
    (the `//` is a comment) AND the comment content (`"ignored"`
    ident) does not leak into the token stream.
  - `counter_shape_with_arithmetic_body_lexes_clean` — the
    canonical counter source with `count = count + 1` /
    `count = count - 1` lexes without error.
- `src/view/parser.rs` — five new arms in `append_token_source`
  serialising each Token variant to its literal char, plus an
  extension to `needs_space_before`'s "space-triggers" set:
  `+`, `-`, `*`, `/`, `%`, and `=`. The `=` was implicit before
  the follow-up (event bodies never contained anything but
  simple literal assigns, so nobody noticed the missing space
  after `=`); once bodies gained arithmetic, `count =count + 1`
  looked jarring next to the tidy `count + 1` fragment. Adding
  `=` there produces `count = count + 1` — idiomatic + consistent
  with how the arithmetic ops space out. +3 unit tests in
  `view::parser::tests`:
  - `event_body_with_add_round_trips_verbatim` — asserts
    `body_raw` for `event increment() { count = count + 1 }`
    trims to exactly `"count = count + 1"`.
  - `event_body_with_all_arithmetic_ops_round_trips` — same
    with all 5 ops in one body.
  - `arithmetic_body_re_lexes_through_classic_parser` — takes
    the `body_raw` and runs it through
    `crate::parser::parse_statements_from_source`, asserting
    the resulting `Vec<Stmt>` matches the expected
    `Stmt::Assign { value: Expr::BinOp { op: Add, left:
    Ident("count"), right: Int(1), .. }, .. }` shape. Proves
    the round-trip is functionally faithful, not just
    string-equal.
- `src/view/codegen_wasm.rs` — +1 unit test in
  `view::codegen_wasm::tests`:
  - `arithmetic_body_lowering_end_to_end_via_parse_expand` —
    full pipeline (parse → expand → emit) on a counter source
    with `count = count + 1` and `count = count - 1`. Asserts
    the emitted Rust contains the same lowering as the
    direct-AST tests (`__rhs = ((*self.count.borrow()) + 1i64);`
    for increment, `- 1i64` for decrement, plus interpolation
    `format!("{}", ...)`). Distinct from the direct-AST tests
    that already existed in 11.4.b — this one proves the WHOLE
    path (view lexer + parser + expand + emitter) is unblocked,
    not just the emitter in isolation.
- `docs/fase-11-plan.md` — §9.m updated (the "View lexer
  arithmetic gap" bullet under Debt residual gets a "CLOSED
  2026-07-14 via §9.n follow-up" annotation with cross-link)
  + this §9.n section added.

**Design decisions worth naming**:

- **Only the 5 arithmetic ops + `=` — not comparisons, logical,
  bitwise, or unary `!`**. `==` / `!=` / `<=` / `>=` / `&&` /
  `||` / `!` / bitwise are not needed by any current
  `.fitzv` shape (event bodies of the counter demo, state
  defaults, template interpolations). Comparisons will land the
  moment `{#if cond}` stops rejecting at emit time in 11.4.c+ —
  a targeted follow-up when that need arrives, mirroring the
  posture of this one.
- **`/` guarded by the existing `skip_ws_and_comments`
  ordering**. The comment handler runs first in the tokenise
  loop; a bare `/` only reaches the main match when NOT
  followed by another `/`. Zero risk of `//` comments now
  being misparsed as two Slash tokens — regression test
  covers this explicitly. No new deuda opened.
- **`Token::Star` name (not `Asterisk` or `Times`)**. Matches
  the naming convention of the classic Fitz `crate::lexer`,
  which uses `Star` for `*` (see `crate::ast::BinOpKind::Mul`
  emitted from a `Star` token classic-side). Keeps the two
  lexers reading similarly for anyone context-switching between
  them.
- **`=` added to `needs_space_before`'s space-triggers set**.
  Discovered as a bystander deuda while writing the arithmetic
  tests — the first test's `assert_eq!(body.trim(), "count = count
  + 1")` failed because `count =count + 1` was produced instead.
  Root cause: `=` was never in the trigger set because pre-
  arithmetic bodies had shapes like `x = true` where the space
  before `=` (from the `x` alpha) plus the raw `=` char left the
  output as `x =true` — and no existing test asserted that shape.
  Adding `=` here is safe (both spacings lex-equivalent for the
  classic parser) and improves readability of error messages
  that quote raw blob content.
- **Tests split across three files instead of consolidated**.
  Each layer's test lives in its own module: the lexer test
  asserts the token stream shape, the parser test asserts the
  round-trip string + AST shape, the codegen_wasm test asserts
  the emitted Rust. Follows the isolation posture of `view/` —
  each layer testable in isolation, plus one end-to-end
  integration test in the top-most layer that proves they
  compose.

**Verification pre-close of §9.n**:

- `cargo test --lib --release`: **3517 passed / 0 failed** in
  2.99s (3510 baseline pre-arithmetic + 7 new: 3 lexer + 3
  parser + 1 codegen_wasm end-to-end).
- `cargo fmt --all --check`: clean.
- `cargo clippy --lib --tests -- -D warnings` (default): clean
  in 24.58s.
- `cargo clippy --lib --tests --features client-wasm -- -D
  warnings`: clean.
- Classic `.fitz` pipeline untouched (0 changes to `src/lexer.rs`,
  `src/parser.rs`, `src/ast.rs`, `src/types.rs`, `src/evaluator.rs`,
  `src/codegen.rs`). Invariants 1-5 all hold.

**Debt residual from §9.n**:

- **Comparison ops (`==` / `!=` / `<=` / `>=`) still error**.
  `{#if cond}` currently rejects at emit time, so this doesn't
  bite yet. Follow-up when 11.4.c wires If bodies into the
  emitter; probably ~30 LoC + a handful of tests, same posture
  as this one.
- **Logical `&&` / `||` unsupported**. Fitz uses `and` / `or` /
  `not` keywords (lex as `Ident` and re-lex correctly through
  the classic parser), so this only bites for users who write
  `&&` / `||` — a lesser priority since Fitz idiom prefers the
  keyword form. Deferred until real demand appears.
- **Float literals still tokenise wrongly**. `0.5` lexes as
  `Ident("0")` then hits `.` which is unhandled → error. State
  fields of type `Float` are already rejected by
  `codegen_wasm.rs` anyway, so no immediate bite. Fix would
  extend the digit-reader in the view lexer to accept `.` as a
  continuation when preceded by digits — small delta.

No other files touched by §9.n. 11.4.c is now unblocked: the
canonical counter shape parses, expands, checks, and emits
end-to-end.

---

## 9.o Files touched by 11.4.c — counter demo runnable + bundle-size measurement harness

11.4.c ships the first runnable end-to-end demo of the `.fitzv` →
WASM pipeline. Everything the emitter needed to prove — that it
produces valid, compilable Rust that a real browser can load and
interact with — is now instrumented. The **infrastructure** is
CLOSED; the **bundle-size gate measurement** against 40 KB
gzipped is a one-command opt-in that a contributor with
`wasm-pack` + `wasm32-unknown-unknown` installed can execute at
any time (see the "Measurement recipe" subsection below).

**Files added**:

- `examples/view/counter/Counter.fitzv` (~45 LoC counting
  comments) — the canonical counter source. Respects the POC
  subset of 11.4.b exactly (Int state + `@click`-only + literal
  or BinOp-arithmetic event bodies + Text/Element/Interpolation/
  Static-attr template + `<style scoped>`). Uses the arithmetic
  operators unblocked by §9.n (`count = count + 1` /
  `count = count - 1`) so the demo exercises the arithmetic
  lowering, not just literal reassignment.
- `examples/view/counter/index.html` (~55 LoC) — mount point
  `<div id="app">` + `<script type="module">` that imports
  `./wasm-crate/pkg/counter.js` and calls `init()`. Includes a
  tiny inline `<style>` for the surrounding page frame so the
  demo doesn't look raw in the browser, plus a helpful error
  message if the module fails to load (typically `file://` vs
  HTTP origin confusion).
- `examples/view/counter/README.md` (~130 LoC) — instructions
  covering: layout, source of truth (`Counter.fitzv`),
  regeneration workflow, `wasm-pack build` recipe, browser smoke
  recipe with `python -m http.server`, bundle-size measurement
  recipe, and the composition split (why
  `#[wasm_bindgen(start)]` lives in the harness, not in
  `view::emit_module()`).
- `examples/view/counter/wasm-crate/Cargo.toml` (~35 LoC) — the
  crate that wraps the generated `lib.rs`. `crate-type =
  ["cdylib"]`, deps limited to `wasm-bindgen`, `web-sys` (with
  the exact features list that mirrors the root `Cargo.toml`
  `client-wasm` feature — see §9.m), and
  `console_error_panic_hook`. `[profile.release]` set to
  `opt-level = "z"` + `lto = true` + `codegen-units = 1` + `strip
  = true` — the canonical wasm-bindgen size-tuning knobs. Not a
  `[workspace]` member (deliberately — see design decisions
  below).
- `examples/view/counter/wasm-crate/.gitignore` — excludes
  `target/`, `pkg/` (wasm-pack output), and `Cargo.lock` (the
  demo crate treats itself as a POC scaffold that anyone can
  blow away + rebuild; the top-level `fitz/Cargo.lock` is the
  canonical lockfile for reproducibility).
- `examples/view/counter/wasm-crate/src/lib.rs` (~140 LoC,
  **AUTO-GENERATED**) — the emitter's output for
  `Counter.fitzv` + the composed `#[wasm_bindgen(start)]` entry
  point at the tail. Committed in-tree so `wasm-pack build`
  works out of the box for a fresh clone. Kept in sync by the
  `regenerate_counter_lib_rs` test (see below); running the
  test after any `Counter.fitzv` edit or any change to
  `src/view/codegen_wasm.rs` will refresh this file.
- `tests/view_counter_wasm_smoke.rs` (~260 LoC + doc comments)
  — top-level integration test file with two tests:
  - `regenerate_counter_lib_rs` (always runs): loads
    `Counter.fitzv`, runs the full pipeline (`view::parse` →
    `view::expand` → `view::check` → `view::emit_module`),
    appends the composed entry point, validates 8 structural
    invariants (struct decl, state field type, each event
    handler, `#[wasm_bindgen(start)]`, mount selector, style
    helper referenced), and writes the result to `lib.rs` iff
    the content changed. Zero external dependencies — this
    runs on every `cargo test` and keeps the committed baseline
    fresh automatically.
  - `build_counter_wasm_and_measure` (`#[ignore]`): same
    regeneration + shells out to `wasm-pack build --release
    --target web` inside `wasm-crate/`, reads
    `pkg/counter_bg.wasm`, computes raw + gzipped sizes (gzip
    via `flate2::write::GzEncoder` with `Compression::best()`
    so the measurement matches what a CDN typically serves),
    prints a verdict, and fails the test if gzipped size
    exceeds `40 * 1024 = 40960` bytes. Panics with a targeted
    message pointing at §9.l for the pivot decision when the
    gate is breached.

**Files touched**:

- `Cargo.toml` — one line added under `[dev-dependencies]`:
  `flate2 = "1"`. Reasoning: `wasm-pack`/browsers serve gzipped
  by default and the 40 KB gate is defined against the gzipped
  size, so we need an in-process way to compute it. Alternative
  considered: shell out to `gzip -c`, which works on Linux/macOS
  but is not consistently in `PATH` on Windows. `flate2` is a
  small, universally-supported crate that keeps the smoke
  cross-platform. `dev-dependency` only — the `fitz` release
  binary does not link it.
- `docs/fase-11-plan.md` — this §9.o added + row 11.4 in §6
  refreshed to mark 11.4.c INFRA CLOSED with measurement
  pending. §9.m's "Debt residual" bullet about
  `#[wasm_bindgen(start)]` being unemitted is left as-is: it's
  a Phase 11.5 CLI concern that 11.4.c handles ad-hoc in the
  harness, not a debt that 11.4.c "closes" (the CLI still needs
  to be written).

**Design decisions worth naming**:

- **Two smoke tests, not one, with the always-on test being the
  regeneration**. The temptation was a single `#[ignore]` test
  that regenerates AND builds. Splitting them means: (a) every
  `cargo test` verifies the pipeline still produces valid,
  compilable-looking Rust and the committed `lib.rs` matches
  the current emitter, catching drift the moment a PR touches
  `src/view/codegen_wasm.rs`; (b) the expensive step
  (wasm-pack, ~30-60s cold, several deps download on first
  run) stays opt-in for contributors who have the toolchain.
  Trade-off: the always-on regeneration test WRITES to
  `wasm-crate/src/lib.rs`, which is a mildly surprising side
  effect for a test. Mitigated by `write_if_changed` (no-op if
  the content matches) and by the file being marked
  AUTO-GENERATED in its header + `.gitignore` covering the
  build artefacts around it.
- **`wasm-crate/` NOT a `[workspace]` member of the root
  `Cargo.toml`**. Adding it as a workspace member would let
  `cargo build --workspace` build the wasm crate as part of the
  normal build, which sounds convenient but is actually bad:
  (a) it forces every `cargo build` to require the
  `wasm32-unknown-unknown` target to be installed OR emit a
  cryptic linker error; (b) it wires the demo's Cargo.lock into
  the workspace lock, coupling the demo's deps to the release
  binary's dep versions — the demo should be free to bump its
  own `wasm-bindgen` version independently. Keeping it out-of-
  tree from the workspace makes it a genuinely standalone POC
  scaffold. `wasm-pack build` from inside `wasm-crate/`
  resolves deps against `wasm-crate/Cargo.toml` alone.
- **The composed entry point (`#[wasm_bindgen(start)]`) lives
  in the harness that generates `lib.rs`, NOT in
  `view::emit_module()`**. `emit_module()` remains agnostic
  about how a component wires into the browser lifecycle —
  which mount selector, whether N components share a single
  `start()`, whether the entry point does other init (routing,
  analytics, state hydration). Phase 11.5 CLI (`fitz build
  --target wasm --entry Counter --mount '#app'`) will emit the
  equivalent wrapper per invocation. Handling the composition
  in the harness for 11.4.c is a deliberate throw-away of the
  ergonomics: it wires the demo end-to-end without prejudging
  the shape of the CLI flag surface.
- **Structural invariants in `regenerate_counter_lib_rs`
  duplicate coverage from `codegen_wasm::tests`**. The
  unit tests over there already assert most of these substrings.
  The invariants here catch a different failure mode: `emit_module`
  starts producing valid Rust that DIFFERS from the committed
  `lib.rs` (e.g. a refactor changes an ident name or the order
  of impls). Without the invariants, that drift would only
  surface when someone next runs `wasm-pack build` — much
  later than we'd like. The list is deliberately small (8
  items) so it doesn't duplicate the ~23 unit tests point-by-
  point.
- **`counter_bg.wasm` as the measurement target, not the whole
  `pkg/` directory**. The 40 KB gate is defined against the
  WASM binary specifically, not the JS glue or `.d.ts` file.
  `counter.js` in `pkg/` is ~15-20 KB uncompressed but browsers
  cache it separately, and it's not the interesting variable —
  the interesting variable is "how much WASM does one Fitz
  component compile to". Measuring just `counter_bg.wasm`
  isolates that question.

**Verification pre-close of §9.o**:

- `cargo test --lib --release`: **3517 passed / 0 failed** in
  2.99s (unchanged from post-§9.n baseline — no new lib tests
  added; the smoke tests live in `tests/view_counter_wasm_smoke.rs`
  as an integration test file, not adding to the lib count).
- `cargo test --test view_counter_wasm_smoke regenerate_counter_lib_rs`:
  **1 passed / 0 failed** in <1s (compile time ~30s cold for
  the new `flate2` dep chain; incremental thereafter). The
  test regenerated `wasm-crate/src/lib.rs` from
  `Counter.fitzv`, ran all 8 structural invariants clean, and
  wrote the file exactly once.
- `cargo fmt --all --check`: clean (one line-length reflow in
  the smoke file applied automatically by `cargo fmt --all`
  during the first pass; the checked state is stable).
- `cargo clippy --lib --tests --bins -- -D warnings` (default):
  clean in 43.03s.
- `cargo clippy --lib --tests --bins --features client-wasm --
  -D warnings`: clean in 45.78s. Same warnings-free posture as
  §9.m and §9.n confirmed.
- Classic `.fitz` pipeline untouched. Invariants 1-5 all hold.
  The `flate2` dev-dep addition does NOT touch the release
  binary — `cargo build --release` links the same set of
  crates as before v0.20.1 baseline.

**Result (measurement executed 2026-07-15)**:

Ran `cargo test --test view_counter_wasm_smoke -- --ignored` on
Windows 11 with `rustc 1.85+` + `rust-std-wasm32-unknown-unknown`
+ `wasm-pack 0.15.0` freshly installed via `cargo install
wasm-pack` (~2m cold). The smoke printed:

```
--- Phase 11.4.c bundle size ---
  raw .wasm :   26763 B (26.1 KB)
  gzipped   :   11654 B (11.4 KB)
  gate      :   40960 B (40 KB gzipped)
  verdict   : OK (29306 B under the gate, 28.6 KB headroom)
test build_counter_wasm_and_measure ... ok
```

**A2 (hand-rolled `wasm-bindgen` + `web-sys`) validated as the
primary WASM target for Phase 11**. Bundle sits at ~28% of the
gate, leaving ~28.6 KB of headroom for the growth that 11.5
(multi-component composition) + 11.6 (fitz-liveviews migration)
will bring. No pivot to JS-vanilla needed; `docs/stack.md`
lines 99-101 remain accurate as shipped in 11.4.a.

**Wasm-opt workaround required to run the pipeline**: on first
attempt, `wasm-pack build --release` failed at the wasm-opt
step with `wasm-validator error ... Bulk memory operations
require bulk memory [--enable-bulk-memory]` on 6 functions. The
`binaryen` binary that `wasm-pack 0.15.0` bundles is older than
the rustc output it needs to optimize — modern rustc emits
`memory.copy` ops unconditionally. Fix: add
`[package.metadata.wasm-pack.profile.release]` with
`wasm-opt = ['-O', '--enable-bulk-memory']` to
`wasm-crate/Cargo.toml`. This preserves the size-optimization
pass (the alternative `wasm-opt = false` would skip it and
grow the bundle 20-40%). Recorded inline in the Cargo.toml with
the reason and the exact error signature for future
contributors. Not counted against the 40 KB gate — this is a
tooling misalignment, not a runtime concern.

**Emitter cosmetic warnings surfaced during the build** (NOT
blocking, categorized as deuda derivada below):

- `unused_parens` on `let __rhs = ((*self.count.borrow()) +
  1i64);` — `emit_event_body` wraps the RHS of the assignment
  in redundant parentheses when the RHS is a `BinOp`. Two hits
  in the counter demo (`+` and `-`).
- `non_snake_case` on `__inject_style_Counter_counter_c_8b71bce8`
  — the style-injection helper's name mixes the component
  name (PascalCase per convention) with a snake_case prefix,
  which triggers rustc's naming lint. One hit per component
  with a `<style scoped>` block.

None of them are correctness bugs; the produced `.wasm` runs
correctly. Refinement lands in a future emitter cleanup pass
(likely bundled with the 11.5 CLI work when the composed
`#[wasm_bindgen(start)]` moves out of the demo harness and into
the CLI).

**Verification on the measurement host**:

- `rustup target add wasm32-unknown-unknown`: succeeded (~15s
  download of rust-std for wasm32-unknown-unknown, freshly
  added target).
- `cargo install wasm-pack`: succeeded (~2m cold, one-time
  compilation of `wasm-pack 0.15.0` + its dep tree). Alternative
  path via `curl -sSf https://rustwasm.github.io/wasm-pack/installer/init.sh
  | sh` would have been faster on Linux/macOS; on Windows the
  `cargo install` path is the reliable one.
- `cargo test --test view_counter_wasm_smoke -- --ignored`:
  first pass FAILED at wasm-opt (wasm-validator error above).
  Second pass after adding the wasm-opt metadata knob:
  **1 passed / 0 failed** in 51s (includes wasm-pack full
  build cold + wasm-opt + gzip measurement).
- `regenerate_counter_lib_rs` (always-on test): still passes
  clean — the emitter output is stable across the measurement
  session.

**Measurement recipe (kept for future re-runs)**:

1. Install prereqs (one-time per machine):
   ```
   rustup target add wasm32-unknown-unknown
   cargo install wasm-pack
   ```
2. From the repo root:
   ```
   cargo test --test view_counter_wasm_smoke -- --ignored
   ```
3. Read the "Phase 11.4.c bundle size" section printed by the
   test. Three outcomes:
   - **Under 40 KB gzipped**: passes cleanly. Refresh the
     Result subsection above with the new number if it's
     drifting materially (>10% change would warrant a note).
   - **Over 40 KB gzipped**: PIVOT to JS-vanilla (approach B1
     in §9.l). Refresh `docs/stack.md` lines 99-101 with the
     evidence + open a §9.o.pivot section documenting the
     decision + the new approach. The `codegen_wasm.rs` module
     stays as reference / point of future comparison, not
     deleted.
   - **`wasm-pack` build error**: something in the emitted
     `lib.rs` doesn't compile (typo in an emitted helper,
     `web-sys` feature missing, wasm-bindgen API mismatch, or
     a fresh wasm-opt validation gap like the bulk-memory one
     recorded above). Fix the emitter in
     `src/view/codegen_wasm.rs` (or the wasm-opt metadata
     knob in `wasm-crate/Cargo.toml`), re-run the smoke to
     regenerate `lib.rs`, iterate. Add a targeted unit test
     in `codegen_wasm::tests` for the shape that broke to
     prevent regression.

**Debt residual from 11.4.c**:

- **Bundle-size measurement recorded 2026-07-15**: 11.4 KB
  gzipped over 40 KB gate. This bullet is kept here (rather
  than deleted) as historical context — the deuda entry it
  reflected is CLOSED. See the Result subsection above.
- **wasm-opt bundled with wasm-pack lags rustc's `memory.copy`
  ops**. The `[package.metadata.wasm-pack.profile.release]`
  `wasm-opt = ['-O', '--enable-bulk-memory']` knob in
  `wasm-crate/Cargo.toml` is a working workaround, but it's
  brittle: newer rustc versions may emit further post-MVP
  wasm features (`reference-types`, `sign-ext`, `nontrapping-
  fptoint`) that wasm-opt will reject one by one. The
  long-term fix is either (a) wasm-pack shipping a newer
  binaryen — tracked in the wasm-pack repo but not scheduled;
  (b) us documenting a `[dependencies.wasm-opt]` override to
  install a system binaryen and skip the bundled one; or (c)
  Phase 11.5 CLI generating the Cargo.toml with the full set
  of `--enable-*` flags baked in. Not blocking any Fitz
  work.
- **Emitter cosmetic warnings** (`unused_parens` in event body
  BinOp assignments + `non_snake_case` in
  `__inject_style_<Component>_<scope>` helpers). Two shapes
  documented in the Result section. Two-line fixes in
  `src/view/codegen_wasm.rs` — the parens are added by an
  over-cautious wrapping in `emit_event_body_assign_rhs` and
  the helper name should lowercase the component name
  before concatenating. Deferred to the 11.5 CLI cleanup
  pass because the current Rust still builds fine and the
  fix has to be threaded through the invariant tests in
  `regenerate_counter_lib_rs` (which would need to update
  their expected substrings).
- **Browser smoke not automated**. The manual browser flow
  (open `index.html` via `python -m http.server` and click the
  buttons) is documented in the README but not tested by CI or
  anything in-tree. Automating this would require headless
  browser instrumentation (Playwright / wasm-bindgen-test with
  `--target chromium`), which is a much larger investment for
  11.5 CLI/LSP-adjacent work, not for 11.4.c.
- **`Closure::wrap(...).forget()` leaks closures on each
  `render()`** (documented in §9.m Debt residual). Confirmed
  present in the emitted `lib.rs` from `Counter.fitzv` — every
  click adds three fresh closures to `document`'s event
  listener table. Acceptable for POC per §9.m; refinement to
  a per-instance `Closure` cache falls under approach A3 (in-
  tree runtime crate), not 11.4.c.
- **`wasm-crate/src/lib.rs` scope-class hash is emitter-derived
  and will drift if the FNV-1a input format changes**. Today
  the hash comes from `<Counter>::<css_raw>` (see §9.k). If
  §9.k's hash input changes, the committed baseline
  `lib.rs` becomes stale and `wasm-pack build` breaks until
  someone runs the regeneration smoke. The regeneration smoke
  is always-on precisely to prevent this from lasting more
  than one PR cycle — anyone running `cargo test` will
  regenerate; git will show the diff.

11.4.c fully CLOSED with the Result subsection above. 11.4.d
(cierre formal + roadmap refresh) also CLOSED 2026-07-15 — this
plan doc's row 11.4 + `docs/roadmap.md` Fase 11 section refreshed
to reflect the gate outcome, and the browser smoke was validated
manually on Windows 11 / Chrome the same day (counter renders in
`#app` with initial `0`, `+`/`-`/`reset` mutate state and the
re-render is scoped to the component's subtree per §9.m D1). With
that, **11.4 CLOSED entirely** — A2 confirmed as primary WASM
target, 11.5 (CLI wiring) unblocked as the next norte.

---

## 9.p Decision recorded by 11.5.a — CLI target routing: hybrid manifest + flag (multi-bin closes 9.y.8+ debt)

First sub-commit of 11.5. **Research + decision only — zero code
change** in this commit. Publishes the design that 11.5.b/c/d/e
will implement. Same posture as 11.4.a (which pinned A2 as the
WASM emitter approach).

**What 11.5 has to solve**

Today `fitz build` only produces native binaries from `.fitz`
sources. The `.fitzv` compilation pipeline (parse → expand →
check → `view::emit_module`) exists end-to-end but has NO public
CLI entry — it is only exercised from `tests/view_counter_wasm_smoke.rs`
via the harness that wraps `emit_module()` in a wasm-crate scaffold
and shells out to `wasm-pack`. That posture was intentional for
11.4 (the emitter had to work before the CLI could route to it),
but it makes the feature unusable for anyone who is not reading
the tests.

11.5 must answer three coupled questions:

1. **How does the CLI decide the target?** Given `fitz build`
   invoked in a project, what says "compile this as a native
   binary" vs "compile this to WASM and bundle it for the
   browser"?
2. **How does the CLI find the entry?** A `.fitzv` component is
   NOT itself a `bin` — a WASM bundle is a `.fitzv` (or a `.fitz`
   that composes several) plus a mount selector plus a
   `#[wasm_bindgen(start)]` wrapper. Where does that composition
   live?
3. **How does a full-stack project ship both?** A single repo
   with a Fitz backend AND a Fitz frontend needs one manifest to
   describe both bins, one command to build each.

**Approaches evaluated**

| # | Approach | Reproducibility | Explores fast | Full-stack in one repo | Cost |
|---|---|---|---|---|---|
| **A** | Extension-driven pure — `.fitz`→native, `.fitzv`→wasm | ✅ deterministic | ✅ zero config | 🟡 needs multi-bin anyway | ❌ ambiguous when a `.fitzv` should go SSR |
| **B** | Manifest-only (`[bin] target = "..."`) | ✅ manifest is truth | ❌ requires edit per experiment | ❌ two repos | ✅ minimal manifest surface |
| **C** | Flag-only (`--target <t>`) | ❌ depends on scripts / CI | ✅ zero config | 🟡 requires flag every time | ✅ zero manifest change |
| **D** | **Hybrid: manifest primary + `--target` override** | ✅ manifest is truth | ✅ flag for one-shots | ✅ multi-bin natively | 🟡 opens `[[bin]]` (was debt 9.y.8+) |

Approaches A/B/C were rejected in turn:

- **A** loses the SSR-vs-WASM distinction. A `.fitzv` is a
  component, not a target — the same component could be rendered
  server-side to HTML AND compiled client-side to WASM depending
  on where the app wants to use it. Extensions are the wrong axis
  to distinguish targets.
- **B** would keep the manifest surface small BUT would force
  full-stack projects into two separate repos (or into subfolders
  each with its own `fitz.toml`), which fights the "one language,
  one project" pitch that motivates Fase 11 in the first place.
- **C** matches how `fitz build --bundle-python` (Fase 8.b) is
  wired today, which is a real precedent, but leaves the target
  choice out of the repo. That's fine for one-shot experiments;
  it's not fine for a project that WANTS to declare "the `web/`
  bin ships as WASM" once and never think about it again.

**D (hybrid) picked**. Recorded 2026-07-15 by the current
session's `AskUserQuestion` prompt to the author.

**Manifest shape**

`[[bin]]` becomes a TOML array-of-tables. Each entry is a
`ManifestBin` struct with the fields below. The legacy `[bin]`
singular form stays valid — the manifest parser auto-migrates it
to a single-entry `[[bin]]` at parse time (backward compat, zero
breakage for the 40+ boilerplates + course examples that use
`[bin]` today).

```toml
[[bin]]
name = "server"          # required in multi-bin form; optional in
                         # legacy single-bin form (defaults to
                         # `package.name`)
main = "src/main.fitz"   # required
target = "native"        # optional, default "native"

[[bin]]
name = "web"
main = "src/counter.fitzv"
target = "wasm-client"
mount = "#app"           # required when target = "wasm-client"
```

Fields:

- `name` — bin identifier used by `fitz build --bin <name>`. In
  multi-bin projects it must be unique per manifest. In the
  legacy `[bin]` shape it is optional and defaults to
  `package.name`.
- `main` — path to the entry, relative to the manifest dir. Can
  be a `.fitz` (any target) or a `.fitzv` (only for
  `target = "wasm-client"` in the MVP).
- `target` — `"native"` (default), `"wasm-client"`, or `"ssr"`
  (reserved, rejected in 11.5 with a targeted "coming in 11.6+"
  message).
- `mount` — CSS selector where the WASM bundle mounts its root
  component. Required when `target = "wasm-client"`, ignored
  otherwise. Default suggestion in error messages: `"#app"`.

Fields deferred to later sub-phases (documented here so 11.5.b
knows what NOT to accept yet):

- `entry_component` — for multi-component `.fitzv` files where
  the user wants to explicitly pick which one mounts. In 11.5's
  MVP the emitter accepts single-component files only; parents
  compose children via `<Child />` (11.5.d), no ambiguity.
  Reserved for 11.6+.
- `output` — override for the wasm-crate output dir (default
  `<manifest_dir>/target/wasm/<bin_name>/`). Reserved for 11.5.e
  cleanup if presión real appears.
- `wasm_pack_profile` — pass-through to `wasm-pack --profile`.
  MVP uses `release` always. Reserved for 11.5.e.

**CLI shape**

`fitz build` gains two flags:

- `--bin <name>` — selects which `[[bin]]` to build when there
  are more than one. In single-bin projects (or when only one
  matches a filter), `--bin` is optional. Error if `--bin
  <name>` names a bin that does not exist, with the list of
  available bins.
- `--target <t>` — one-shot override of the bin's `target`.
  Useful for experimenting with a `.fitzv` as `wasm-client` when
  the manifest declares it as `ssr` (or vice-versa). Rejected
  when it doesn't match the bin's `main` extension in a sensible
  way (e.g. `--target wasm-client` on a `.fitz` bin that has no
  `.fitzv` in scope → error citing 11.5.d for the composition
  path).

Legacy single-file mode (`fitz build src/counter.fitzv`) still
works — no manifest required. In that mode the target is inferred
from the extension (`.fitz` → `native`, `.fitzv` → `wasm-client`)
and the mount defaults to `"#app"`, with `--target` and
`--mount` flags available to override.

**Vocabulary for `target`**

Three targets pinned in 11.5:

- **`native`** — default, matches today's `fitz build` behaviour.
  Rust source emitted via `codegen.rs` + `cargo build` → binary
  in `<manifest_dir>/target/release/`.
- **`wasm-client`** — SFC compiled to a WASM bundle intended for
  the browser. Emits a `wasm-crate/` scaffold + `wasm-pack build
  --release --target web` + copies `pkg/` to
  `<manifest_dir>/target/wasm/<bin_name>/`. The `#[wasm_bindgen(start)]`
  wrapper is auto-generated by the CLI (this closes the
  intentional gap left in 11.4.b — the emitter did NOT emit
  `start`, that composition belongs to the CLI).
- **`ssr`** — SFC rendered server-side to HTML strings. Reserved
  in the vocabulary but NOT implemented in 11.5 — accepted at
  parse, rejected at build with a targeted "coming in 11.6+"
  message. This future-proofs the naming so users writing SFCs
  today can declare intent without breaking when 11.6 lands.

Rejected alternatives:

- `wasm` (no suffix) — ambiguous when SSR HTML lands (both
  server-side render and client-side WASM emit "wasm-adjacent"
  artifacts under some strategies).
- `bin` — renames the default to match Cargo, but breaks the
  mental model of every existing Fitz user reading the manifest.

**Sub-phase plan for 11.5**

- **11.5.a** — Research + decision. **CLOSED 2026-07-15** (this
  §9.p section). Zero code.
- **11.5.b** — Manifest extension: `[[bin]]` array-of-tables +
  `name`/`target`/`mount` fields on `ManifestBin`. Legacy
  `[bin]` auto-migrates. `fitz build --bin <name>` selector +
  `fitz build --target <t>` override. **Closes debt 9.y.8+ as
  a side-effect** — that debt line in
  `docs/roadmap.md` gets a "CLOSED via 11.5.b" note. Rejects
  `target = "ssr"` with a targeted 11.6+ message. `.fitzv` entry
  with `target = "native"` rejected (targeted error naming
  `wasm-client`). Legacy `[bin]` singular remains valid — no
  breaking change for existing boilerplates / course examples.
  Unit tests on `manifest.rs`: multi-bin parse, legacy migration,
  target enum, mount validation, cross-checks.
- **11.5.c** — Single-component wasm-client build: given a bin
  with `target = "wasm-client"` + `main = "src/counter.fitzv"`
  + `mount = "#app"`, `fitz build --bin web` emits the
  `wasm-crate/` scaffold (Cargo.toml with the metadata knob for
  `wasm-opt` — see §9.o gotcha), the `src/lib.rs` from
  `view::emit_module()`, and the `#[wasm_bindgen(start)]`
  wrapper that calls `Counter::new().mount("#app")`. Shells out
  to `wasm-pack build --release --target web`, copies `pkg/` to
  `target/wasm/<bin_name>/`. E2E test compares the emitted
  `lib.rs` against the counter demo baseline (bit-for-bit,
  proving the CLI produces the same output the `tests/view_counter_wasm_smoke.rs`
  harness does today).
- **11.5.d** — Multi-component composition: `<Child prop="v" />`
  in a template mounts a nested component with static props. The
  shell parser already recognises the `<Child />` shape (from
  §9.h `<slot />` work in 11.2.c mini-commit 3); 11.5.d wires
  the checker to validate that `Child` is a declared component
  in the same file, coerces static attribute values to the
  child's `state` field types, and emits Rust that instantiates
  `Child::new()` with the props set. Reject compound cases with
  targeted 11.6 messages: dynamic props (`prop={expr}`),
  fallthrough attrs, `<slot>` fill-in.
- **11.5.e** — Cierre formal: run the kanban example
  (`fitz-liveviews/examples/kanban/` today lives in the LiveViews
  repo) as a `.fitzv` rewrite in `examples/view/kanban/`; measure
  compile time under 5s per §6 row 11.5 cierre criterion; refresh
  `docs/fase-11-plan.md` §6 row + `docs/roadmap.md` + memory.
  If the kanban rewrite reveals a scope hole (e.g. dynamic list
  bindings, event bubbling), record it as debt residual for
  11.6+ rather than blocking 11.5 close.

**Files touched by 11.5.a**

- `docs/fase-11-plan.md` — this §9.p section + refresh of §6
  row 11.5 with the sub-phase breakdown a→e (11.5.a marked
  CLOSED, b/c/d/e listed with concise scope).
- Memory `project_phase_11_frontend_view.md` — cross-link this
  §9.p and note the routing decision so 11.5.b starts from the
  right assumption.

Not touched: `src/main.rs`, `src/manifest.rs`, `src/view/*` —
all deferred to 11.5.b. Code lands one commit later.

**Debt / gotchas visible from 11.5.a**

- **Multi-bin closes 9.y.8+** — the line item "Multi-bin
  (`[[bin]]`) — 9.y.8+" in `docs/roadmap.md` gets a CLOSED note
  at the end of 11.5.b. The debt was small (parsing + iteration)
  precisely because 9.y kept `[bin]` singular for the MVP; the
  cost of closing it now is the CLI dispatch through `--bin
  <name>` rather than a heavier rewrite.
- **`ssr` target reserved, not implemented** — 11.6+ will land
  SSR. Reserving the name in 11.5.a avoids breaking manifests
  written between 11.5 and 11.6.
- **wasm-crate metadata drift** — 11.5.c has to emit the
  `[package.metadata.wasm-pack.profile.release] wasm-opt =
  ['-O', '--enable-bulk-memory']` knob in every generated
  `Cargo.toml` (see §9.o Debt residual). If rustc starts emitting
  more post-MVP wasm features (e.g. `reference-types`,
  `sign-ext`), the emitter has to grow the list, or 11.5.e opens
  a debt for a `[dependencies.wasm-opt]` override that installs
  a system binaryen and skips the bundled one.
- **Cosmetic emitter warnings** documented in §9.o Debt residual
  (`unused_parens` + `non_snake_case`). The 11.5 emitter cleanup
  pass — probably squeezed into 11.5.c or 11.5.e — should fix
  both while the invariant tests are already under active edit.
  Two-line diffs each in `src/view/codegen_wasm.rs`.
- **Composition ambiguity in multi-component files** — a `.fitzv`
  today can declare multiple `component X { ... }` blocks. If the
  bin's `main = "src/multi.fitzv"` has more than one component,
  which one is the root? 11.5's MVP resolves this by convention:
  the FIRST declared component is the root, and children are
  reached via `<Child />` composition. If presión real appears
  for explicit selection, an `entry_component = "X"` manifest
  field can be added — flagged in the manifest shape section
  above.
- **`.fitzv` → SSR pipeline nonexistent** — the `view::emit_module`
  emitter today only produces WASM Rust. SSR HTML rendering is a
  separate codepath that 11.6+ will build (probably a second
  emitter alongside `codegen_wasm.rs`, maybe `codegen_ssr.rs`).
  11.5.a's `ssr` reservation is only nominal.

11.5.a fully CLOSED with this §9.p section. 11.5.b (manifest
extension + `--bin`/`--target` flags) is the next commit — it's
where code touches disk.

---

## 9.q 11.5.b — manifest extension + `--bin` / `--target` flags (multi-bin closes 9.y.8+)

Second commit of 11.5. **Code lands on disk.** Implements the
design pinned in §9.p: `[[bin]]` array-of-tables becomes the
canonical shape, legacy `[bin]` auto-migrates, `Target` enum plus
`mount` field are wired end-to-end, and `fitz build` gains
`--bin`/`--target`. No emitter changes yet (that's 11.5.c) —
selecting `target = "wasm-client"` still errors out, but now
with a targeted "coming in 11.5.c" message pointing at the exact
sub-phase.

**Files touched**

- `src/manifest.rs` (~570 LoC net added):
  - `Manifest.bin: Option<Bin>` removed; replaced by
    `Manifest.bins: Vec<ManifestBin>`.
  - New struct `ManifestBin { name, main, target, mount }` with
    helper `effective_target(&self) -> Target` (defaults to
    `Native` when `target.is_none()`).
  - New enum `Target { Native, WasmClient, Ssr }` with
    `#[serde(rename_all = "kebab-case")]` and `Default = Native`;
    `Display`/`as_str()` for CLI messages.
  - New enum `ManifestWarning::SsrTargetReserved { bin_name }`
    surfaced via `Manifest::warnings() -> Vec<ManifestWarning>`.
    Consumed by `fitz build` (see `emit_manifest_warnings` in
    `main.rs`) to notify the user without failing the parse.
  - New error variants: `BinMissingName`, `BinDuplicateName`,
    `BinInvalidShape`, `BinNotFound`, `BinAmbiguous`. All with
    formatted `Display` messages that quote the offending name
    and list available bins when relevant.
  - Custom `Deserialize` via `RawManifest` + untagged
    `RawBinField { Single(RawManifestBin) | Multiple(Vec<RawManifestBin>) }`.
    `normalize_bins()` migrates legacy `[bin]` singular (fills
    `name` from `package.name`) and enforces uniqueness for
    `[[bin]]` array-of-tables (rejects missing name + duplicates
    with the index/name in the error).
  - `validate_bin_cross_fields()` runs at parse time:
    - `.fitzv` entry with `target = "native"` (explicit or the
      `None` default) → `BinInvalidShape` naming the mismatch.
    - `target = "wasm-client"` without `mount` → `BinInvalidShape`
      hinting `mount = "#app"`.
    - `target = "ssr"` — parse OK; the warning goes via
      `warnings()`.
  - Custom `Serialize` on `Manifest` via `ManifestWire` +
    `BinFieldOut { Legacy | Multi }` (both untagged). Preserves
    the visual `[bin]` singular shape for the common scaffolded
    case (one bin, `name == package.name`, no `target`, no
    `mount`); everything else emits as `[[bin]]` array-of-tables.
    Existing `fitz new` / `fitz init` scaffolds remain
    byte-identical.
  - New helpers on `Manifest`:
    - `warnings()` — non-fatal notices for the CLI.
    - `bin_by_name(name)` — direct lookup, returns `Option<&ManifestBin>`.
    - `select_bin(selector: Option<&str>)` — the routing logic
      used by `resolve_entry_with_bin` (name match, single-bin
      shortcut, ambiguous error when multi-bin without selector,
      `Ok(None)` when the manifest has no `[bin]`).
- `src/main.rs` (~180 LoC net added):
  - `Commands::Build` gains `--bin <NAME>` and `--target <TARGET>`
    (both `Option<String>`). Legacy calls (`fitz build`, `fitz
    build src/main.fitz`) continue to work.
  - `ResolvedEntry` gains `target_override: Option<Target>` and
    an `effective_target()` method that resolves precedence:
    `--target` flag > selected bin's `target` field > extension
    inference in single-file mode (`.fitzv` → `WasmClient`, else
    `Native`).
  - `ManifestCtx` gains `selected_bin: Option<ManifestBin>`.
  - New public API `resolve_entry_with_bin(file, bin, target)`;
    the old `resolve_entry(file)` is a thin wrapper that passes
    `None, None` (kept for `Run`/`Check`/`Test`/`Docker`/`Deploy`
    etc. — they inherit the "single-bin or explicit file"
    requirement).
  - Helpers `parse_target_flag(&str) -> Result<Target, String>`,
    `emit_manifest_warnings(&ResolvedEntry)`, and
    `enforce_build_target_supported(Target) -> Result<(), String>`
    — the last one is where `WasmClient` errors with the
    11.5.c pointer and `Ssr` errors with the 11.6+ pointer,
    BEFORE any long operation (Cargo, cargo build, etc.).
  - `resolve_db_entry` and `discover_test_sources_from_manifest`
    updated to consume `parsed_manifest.bins.first()` (they
    accept the first bin as a convenience for single-bin
    projects; multi-bin projects that want granular selection
    for `fitz db` / `fitz test` open a deferred refinement — a
    per-command `--bin` flag).
- `src/deploy.rs` — test helper `dummy_manifest` updated to the
  new shape (`bins: vec![ManifestBin { ... }]`).
- `tests/cli_e2e.rs` (~155 LoC net added):
  - `phase_11_5_b_build_multi_bin_without_flag_errors_listing_bins`
  - `phase_11_5_b_build_with_bin_missing_name_errors_listing_available`
  - `phase_11_5_b_build_with_bin_wasm_client_rejects_citing_11_5_c`
  - `phase_11_5_b_build_target_override_ssr_rejects_citing_11_6`
  - `phase_11_5_b_build_unknown_target_value_is_rejected`
  - `phase_11_5_b_legacy_bin_singular_still_works_no_flag_needed`
    (regression against the 40+ boilerplates that pre-date 11.5.b)
  - `phase_11_5_b_manifest_fitzv_native_rejected_at_parse`
    (covers cross-field validation surfacing via any subcommand)

**Debt closed by 11.5.b**

- `9.y.8+ — Multi-bin (`[[bin]]`)`. The line item in
  `docs/roadmap.md` (Fase 9.y next-step list) and in
  `docs/deudas-post-5b.md` (residual debt table) gets a
  "CLOSED via 11.5.b" note. The debt was small precisely
  because 9.y kept `[bin]` singular as an MVP shortcut; 11.5.b
  extends the shape without breaking migration.

**Debt / gotchas visible from 11.5.b**

- **Run/Check on multi-bin projects**: `--bin` was intentionally
  scoped to `Commands::Build` only (per the 11.5.a plan). Users
  who run `fitz run` / `fitz check` from a directory whose
  manifest declares more than one `[[bin]]` will see the
  `BinAmbiguous` error from `select_bin(None)` and must either
  pass an explicit file (`fitz run src/main.fitz`) or use
  `fitz build --bin <name>` instead. If pressure appears to
  make `--bin` universal, promote it to Run/Check/Test/etc. in
  a follow-up.
- **`fitz test --bin`** — the current implementation picks
  `parsed_manifest.bins.first()` for inline `@test` discovery
  when the manifest has no `[lib]`. Multi-bin projects that
  want per-bin `@test` selection open a visible refinement.
- **Serialization asymmetry**: the manifest `Serialize` impl
  preserves legacy `[bin]` singular in the visual output when
  the sole bin matches the "legacy shape". If a user
  hand-writes `[[bin]] ... [/bin]]` (array-of-tables) and lets
  Fitz round-trip it back, the output collapses to `[bin]` if
  the shape qualifies. Documented in the `to_toml_string`
  comment; harmless because both forms parse to the same
  semantic manifest.
- **`--target wasm-client` on `.fitz` composition**: accepted at
  parse (11.5.b) so users can declare intent, but the build
  still errors with the 11.5.c pointer. When 11.5.c lands the
  cross-field validation for the composition case may need
  refinement (today the rejection is single-message; the plan
  is per-target composition semantics).
- **No `--mount` CLI flag yet**: the plan lists it for
  single-file mode overrides. Deferred to 11.5.c when the
  emitter actually consumes `mount`. In manifest mode the
  `mount` field carries the value; in single-file mode we
  currently default to `"#app"` in the emitter (once it lands).

**Files NOT touched by 11.5.b**

- `src/view/*` — no change to the parser/checker/emitter. The
  WASM emit still lives behind the harness in
  `tests/view_counter_wasm_smoke.rs`. 11.5.c wires
  `Commands::Build` to actually call `view::emit_module()` + the
  wasm-crate scaffold.
- `src/codegen.rs` — no change; only classic Rust emit for
  `Native` bin builds routes through it, unchanged.

11.5.b fully CLOSED with this §9.q section, the corresponding
row refresh in §6, and the debt line update in
`docs/deudas-post-5b.md`. Next: 11.5.c (single-component
`wasm-client` build — emit `wasm-crate/` scaffold + shell out
to `wasm-pack`).

---

## 9.r 11.5.c — single-component `wasm-client` build (scaffold + wasm-pack + copy pkg)

Third commit of 11.5. **The CLI can now emit a browser WASM
bundle from a `.fitzv`.** `fitz build --bin <web>` on a bin
with `target = "wasm-client"` and `main = "src/App.fitzv"`
walks the view pipeline, scaffolds a temporary wasm-crate,
shells out to `wasm-pack`, and copies the produced `pkg/` to
`<manifest_dir>/target/wasm/<bin_name>/`. Bit-for-bit
equivalent to the smoke harness `tests/view_counter_wasm_smoke.rs`
in the counter demo.

**Design decisions**

- **Composition helpers live in `src/view/wasm_build.rs`, not
  in the CLI.** Both the smoke harness and the CLI orchestrator
  in `main.rs` route through the same `view::compose_lib_rs` +
  `view::compose_cargo_toml` + `view::write_wasm_crate_scaffold`
  helpers. This is the invariant that makes the counter baseline
  `examples/view/counter/wasm-crate/src/lib.rs` a valid E2E
  reference for the CLI (it is bit-for-bit what the CLI would
  emit for `[[bin]] name = "counter"` + `mount = "#app"`).
- **First-declared component is the root.** Per §9.p (the
  11.5.a decision), the composer takes `expanded.components[0]`
  and instantiates it in the `#[wasm_bindgen(start)]` wrapper.
  Explicit `entry_component = "X"` field on `[[bin]]` is
  reserved for 11.6+.
- **`.fitzv`-only in MVP.** Classic `.fitz` sources with
  `target = "wasm-client"` are rejected with a targeted
  11.5.d pointer (composition case). The 11.5.b manifest
  cross-field validation only rejects the other direction
  (`.fitzv` with `native`), so the extension check lives in
  the CLI orchestrator.
- **Output layout: `target/wasm-build/<bin>/` for the scaffold,
  `target/wasm/<bin>/` for the final `pkg/` copy.** Cargo-style
  (analogous to `target/release/`). The scaffold dir contains
  the emitted crate; the `pkg/` copy is what the browser
  consumes. Keeping the two separate lets `wasm-pack` overwrite
  its own intermediate output freely without touching the
  end-user-facing `target/wasm/`.
- **Sanitised package name.** Bin names allow `-` (kebab-case
  per `is_valid_package_name`), but wasm-pack maps `-` to `_`
  in the artefact filename which is confusing. The CLI
  sanitises upfront via `sanitise_wasm_pkg_name(bin.name)` and
  uses the result for both the scaffold dir name and the
  `[package].name` in the emitted `Cargo.toml`.
- **`wasm-pack` missing → clean install pointer.** The CLI
  invokes `std::process::Command::new("wasm-pack")` directly;
  on `NotFound` errors it emits an actionable message with the
  `cargo install wasm-pack` command + the docs URL. No
  auto-install: users control their tooling.
- **`--target web`, not `--target bundler`.** The `web`
  target produces a bundle loadable via `<script type="module">`
  without a bundler (webpack/vite/parcel), which matches the
  counter demo's HTML shim and keeps the "Fitz por sí solo"
  promise (docs/stack.md invariant 1).
- **Idempotent `pkg/` copy.** `wasm-pack` writes deterministic
  filenames (`<pkg>_bg.wasm`, `<pkg>_bg.js`, `<pkg>.js`,
  `package.json`, etc.), so a recursive copy that overwrites
  existing files is safe. Files under `target/wasm/<bin>/`
  that are NOT under `pkg/` are preserved (rare in practice,
  but avoids surprise deletions).

**Files touched**

- `src/view/wasm_build.rs` (~430 LoC + 15 unit tests):
  - `pub fn compose_lib_rs(expanded, mount_selector, source_label) -> EmitResult<String>`
    — runs `emit_module` and appends the composed
    `#[wasm_bindgen(start)]` wrapper. Header comment cites
    the source label. Errors early with a targeted message
    when the `.fitzv` declares zero components (no root to
    mount).
  - `pub fn compose_cargo_toml(package_name) -> String` — the
    canonical `Cargo.toml` for the wasm-crate. Includes the
    `[package.metadata.wasm-pack.profile.release] wasm-opt`
    knob + the exact `web-sys` feature subset the emitter
    uses. Test `compose_cargo_toml_bit_for_bit_matches_
    counter_baseline_when_named_counter` pins the shape
    against the committed baseline.
  - `pub fn sanitise_wasm_pkg_name(bin_name) -> String` —
    Rust crate name normalisation (lowercase, hyphens → `_`,
    non-alphanumeric stripped, fallback `"wasm_bundle"`).
  - `pub struct ScaffoldResult { crate_dir, lib_rs, cargo_toml }`
    — return shape of `write_wasm_crate_scaffold`. Callers
    that want to inspect the artefacts (E2E tests) use the
    absolute paths directly.
  - `pub enum ScaffoldError { Io(io::Error, PathBuf), Emit(EmitError) }`.
  - `pub fn write_wasm_crate_scaffold(crate_dir, expanded, bin_name, mount_selector, source_label) -> Result<ScaffoldResult, ScaffoldError>`
    — materialises the scaffold on disk. Idempotent (overwrites
    existing files).
- `src/view/mod.rs` — new `pub mod wasm_build;` + re-exports.
- `src/main.rs` (~250 LoC net):
  - New `fn build_wasm_client_cmd(&ResolvedEntry)` — the
    orchestrator. Loads the `.fitzv`, runs the view pipeline,
    materialises the scaffold, shells `wasm-pack build
    --release --target web`, copies `pkg/` to
    `target/wasm/<bin>/`. Aborts with clean messages on any
    failure. Requires manifest mode (single-file mode
    rejected — needs `mount` from the bin entry AND a stable
    output layout).
  - `fn copy_dir_tree_overwriting(src, dst)` — recursive
    copy helper. Overwrites existing files under `dst`;
    doesn't delete unrelated files.
  - `enforce_build_target_supported` now accepts `WasmClient`
    (the dispatch happens in `Commands::Build` above); only
    `Ssr` still errors with the 11.6+ pointer.
- `tests/view_counter_wasm_smoke.rs` — refactored to route
  through `fitz::view::compose_lib_rs` (removes duplicated
  entry-point composition). Structural invariants updated to
  match the composer's generic `let root = ...` binding.
- `tests/cli_e2e.rs` — 3 new tests:
  - `phase_11_5_c_build_wasm_client_scaffolds_before_invoking_wasm_pack`
    — verifies the scaffold artefacts (Cargo.toml + src/lib.rs)
    exist with the expected content, regardless of whether
    `wasm-pack` succeeds on the runner. Fixture uses
    `main = "src/App.fitzv"` with a component named `App`.
  - `phase_11_5_c_build_with_bin_wasm_client_and_fitz_main_rejects_citing_11_5_d`
    — replaces the pre-11.5.c test that expected an outright
    rejection; now checks the classic `.fitz` composition
    case still rejects with the 11.5.d pointer.
  - `phase_11_5_c_build_wasm_client_with_empty_fitzv_errors_before_wasm_pack`
    — verifies the "no component to mount" error path fires
    before any subprocess.
- `examples/view/counter/wasm-crate/Cargo.toml` — cosmetic
  comment tweak so the composer output matches bit-for-bit.
  (Content is functionally identical.)

**Debt / gotchas visible from 11.5.c**

- **Single-file mode (`fitz build src/App.fitzv --target
  wasm-client`) rejected.** The wasm-client path requires
  manifest mode because the mount selector lives in
  `[[bin]] mount = "..."`. Adding a `--mount` CLI flag for
  single-file mode is a natural refinement — deferred until
  demand appears.
- **`.fitz` composition rejected.** The 11.5.d work item
  covers composing multiple `.fitz`/`.fitzv` sources into a
  single wasm bundle. Today the CLI enforces "one `.fitzv`,
  one bundle".
- **CI without `wasm-pack`.** The 3 new cli_e2e tests do
  NOT require `wasm-pack` on the runner (they exercise
  scaffolding + validation paths that abort before subprocess
  invocation). The full end-to-end (scaffold → `wasm-pack`
  → `pkg/` → measured bundle size) still lives behind
  `tests/view_counter_wasm_smoke.rs::build_counter_wasm_and_measure`
  which is `#[ignore]`. If CI adds `wasm-pack`, opt-in that
  test via `-- --ignored`.
- **No incremental scaffold cache.** Every `fitz build --bin
  <web>` re-writes `Cargo.toml` + `src/lib.rs` even if
  neither changed. `wasm-pack` + `cargo` already handle
  their own incremental compilation, so the CLI overhead is
  negligible. If it becomes visible, the composer could
  `write_if_changed` (matching the smoke harness) to keep
  `cargo`'s incremental cache maximally warm.
- **`--emit-only` flag not implemented.** A "scaffold and
  stop" flag would help testing (skip `wasm-pack`, inspect
  the scaffold in isolation) — the 3 new cli_e2e tests
  already cover the scaffold shape by running the full CLI
  and ignoring the `wasm-pack` exit. Deferred until a real
  use case appears.
- **Multi-`.fitzv` root selection.** A `.fitzv` file with
  multiple `component X { ... }` blocks silently picks the
  first as the root. Per §9.p that's the convention; if the
  user wants explicit control, `entry_component` on `[[bin]]`
  lands with 11.6+ (or earlier if pressure appears).

**Files NOT touched by 11.5.c**

- `src/codegen.rs` — unchanged. The native `fitz build` path
  routes through the classic codegen; `wasm-client` bypasses
  it entirely.
- `src/manifest.rs` — unchanged from 11.5.b. The cross-field
  validation and `Target` enum already model the wasm-client
  shape.
- `src/view/codegen_wasm.rs` — unchanged. The emitter output
  is consumed by the new composer without modifications.

11.5.c fully CLOSED with this §9.r section, the corresponding
row refresh in §6, and the memoria update. Next: 11.5.d
(multi-component composition — `<Child prop="v" />` templates).

---

## 9.s 11.5.d — multi-component composition (`<Child prop="v" />`)

Fourth commit of 11.5. **Multi-component views now compile
end-to-end.** A parent template embedding `<Card title="Welcome"
count="3" />` type-checks the composition, coerces static prop
values to the child's declared state-field types, and emits
Rust that instantiates the child and mounts it into a wrapper
element inside the parent's DOM subtree.

**Design decisions**

- **Tag capitalisation → component reference.** A tag whose
  first char is ASCII uppercase (`<Card />`, `<UserProfile />`)
  is treated as a child-component composition site, matching
  the Vue/React convention. Lowercase tags (`<div>`, `<button>`)
  stay HTML elements. Simple, universal, zero disambiguation
  ambiguity — the parser already accepted both shapes; only the
  expand step needs to route.
- **Dedicated AST variant.** `ExpandedTemplateNode::ChildComponent
  { name, props: Vec<ChildComponentProp>, loc }` — a distinct
  variant from `Element`, so downstream walkers can dispatch
  on the composition case without pattern-matching a magic tag
  string. The variant is `pub` so tests + the emitter both
  consume it directly.
- **Composition validation in `check.rs`, not in `expand.rs`.**
  The expand step doesn't know about sibling components (it
  processes ONE component at a time; the `ExpandedViewFile` is
  assembled after). Deferring validation to `check.rs` — which
  ALREADY receives the whole `ExpandedViewFile` — is cleaner
  and lets the typo-hint pass reuse the file-wide component
  list.
- **Static-only prop values in MVP.** `<Child prop="v" />`
  accepts `RawAttr::Static` only. Dynamic props (`{expr}`) +
  events (`@click`) both reject at expand time with 11.6+
  pointers. The MVP scope is "compose components with fixed
  configuration at their mount site"; dynamic props require
  more emitter machinery (re-render triggers on parent state
  change propagating to child's state — reflow story overlaps
  with the naive-render policy §9.m D1).
- **Prop coercion helper is `pub(crate)`, shared with the emitter.**
  Both the checker AND the emitter route through
  `check::coerce_child_prop_raw_value(raw, type_expr) ->
  Result<String, String>`. The checker uses it to VALIDATE
  (discarding the `Ok(literal)`); the emitter uses it to
  produce the exact Rust literal it writes into
  `*child.field.borrow_mut() = <literal>;`. Guarantees the two
  paths agree bit-for-bit — no drift where the checker accepts
  a value the emitter cannot emit.
- **First-declared component = root.** Same convention as
  11.5.c (§9.p decision). Non-root components in the file are
  only reachable via composition sites in the root's template
  (or via composition sites in reachable descendants). No
  auto-mount of siblings.
- **Self-reference detected upfront.** `<Loop />` inside
  `component Loop { ... }` would recurse forever at render
  time. The checker rejects with a dedicated message
  suggesting the user extract shared UI to a separate
  component.
- **`mount(selector)` refactored into `mount_into(root)`.**
  `mount(selector)` becomes a thin wrapper: query the
  selector, dyn_into `HtmlElement`, delegate to `mount_into`.
  Composition sites need `mount_into` because they already
  have the parent element in hand. The public API keeps both
  entry points — `mount(selector)` for the top-level composed
  entry (WASM `start()`), `mount_into(root)` for
  parent→child mounting.
- **Wrapper `<div class="__fitz-child-<Name>">`.** Every
  composition site creates a stable wrapper div (classed for
  scoped CSS + dev-tools inspection), appends it to the
  parent, then mounts the child into it. The wrapper is
  intentional: children own their own root element separate
  from the parent's DOM, so future features (portal-style
  mounting, keyed lists, etc.) have a stable insertion point.
- **Child state resets on parent re-render.** Documented
  consequence of naive-render (§9.m D1) — when the parent
  clears its root and rebuilds, children are re-instantiated
  from scratch, so their state resets. For the MVP this is
  fine (composition typically wires shared config, not
  independent state). Persistent child state across parent
  renders would need a position-keyed component-instance cache
  — 11.6+ work.
- **Primitive scalars only in state + props.** `type_expr_to_rust`
  + `default_expr_to_rust` extended beyond Int-only to
  `Int`/`Float`/`Bool`/`Str` + `Nullable<T>` of those. Matches
  the coercion helper — anything that flows through a prop is
  also usable as a state field. Compound types (`List`/`Map`/
  Nominal) still deferred: they need cell layout decisions
  that overlap with reflow (Phase 11.6+).

**Files touched**

- `src/view/expand.rs` (~130 LoC net):
  - New enum variant `ExpandedTemplateNode::ChildComponent
    { name, props, loc }`.
  - New struct `ChildComponentProp { field_name, raw_value,
    loc }` (`pub` so downstream consumers pattern-match
    directly).
  - `expand_template_node` dispatches on
    `starts_with_ascii_uppercase(tag)` and routes to
    `expand_child_component`.
  - `expand_child_component` enforces the composition shape
    (self-closing only, static-value props only, no events)
    with 3 targeted rejection messages.
  - Existing class-attr rewriter walker updated to skip
    `ChildComponent` (children's classes are rewritten during
    the child's OWN expand).
  - 6 new unit tests (`phase_11_5_d_expand_*`).
- `src/view/check.rs` (~310 LoC net):
  - `check` orchestrator gains `check_child_components`
    invocation per component (before the state-cascade
    guard — composition validation surfaces even if the
    component's own state has errors).
  - `check_child_components` builds a component-map snapshot
    and walks the template collecting composition sites.
  - `validate_child_site` runs the 5 validations (self-
    reference, name lookup with typo hint, prop-name lookup
    with typo hint, duplicate-prop guard, type coercion).
  - `coerce_child_prop_raw_value(raw, type_expr) -> Result<String, String>`
    is the shared helper the emitter also uses. Supports
    `Str`/`Int`/`Float`/`Bool` + `Nullable<T>`; rejects
    compound types + nominal types + function/tuple types
    with 11.6+ pointers.
  - `format_component_list` + `format_field_list` + local
    `rust_str_literal` helpers.
  - 4 walker updates for the new variant (skip in
    if-cond/for-iter/interpolation/event-attr collectors).
  - 16 new unit tests (`phase_11_5_d_check_*`).
- `src/view/codegen_wasm.rs` (~180 LoC net):
  - `RenderCtx` gains `file: &'a ExpandedViewFile` so
    `emit_child_component` can look up the child's declared
    state field types.
  - `emit_component_impl` + `emit_mount_and_render` gain a
    `file` parameter; call sites updated.
  - `emit_component` (single-component API) wraps its input
    in a synthetic `ExpandedViewFile` to preserve its
    signature — the pre-existing single-component tests
    keep working.
  - `emit_mount_and_render` emits both `mount(selector)`
    (thin wrapper) and `mount_into(root: HtmlElement)`
    (actual work + style injection + render).
  - `emit_template_node` dispatch for `ChildComponent`
    routes to `emit_child_component` which creates the
    wrapper, instantiates the child, writes each prop, and
    calls `mount_into`.
  - `type_expr_to_rust` + `default_expr_to_rust` extended
    to the four primitive scalars + `Nullable<T>`.
  - 4 new unit tests (`phase_11_5_d_emit_*`) + 2 pre-existing
    tests updated (Str state fields now accepted; nominal
    rejection cites 11.6+).
- `src/view/wasm_build.rs` (+1 test):
  - New end-to-end smoke `phase_11_5_d_compose_lib_rs_end_to_end_with_child_component`
    that runs the FULL composer pipeline on a 2-component
    fixture and checks: both structs emit, root mounts on
    `#app`, wrapper class present, coerced prop assignments
    correct, both `mount` + `mount_into` present on both
    components.
- `tests/view_counter_wasm_smoke.rs` — 3 new structural
  invariants for the counter baseline: `mount(selector)`
  present, `mount_into(root)` present, `mount(selector)`
  delegates to `mount_into(root)`. Counter baseline
  regenerated via the shared helper (functionally identical
  output).

**Debt / gotchas visible from 11.5.d**

- **Fallback children (`<Card>...</Card>`) rejected.** Need
  `<slot>` fill-in wiring. Path is: parent `<Card><span>foo</span></Card>`
  → child's `<slot />` positions render the fallback nodes
  inside the child's DOM. Requires threading the fallback
  tree from the composition site through to the child's
  render(). 11.6+ work.
- **Dynamic props (`prop={expr}`) rejected.** Need reflow
  wiring: when the parent's state changes, the parent's
  render() rebuilds — which re-instantiates the child from
  scratch (since composition sites live inside render()) and
  passes the new prop value. Naive but correct. The real
  refinement (skip re-instantiation, only update mutated
  props) needs a position-keyed component-instance cache.
- **Event bubbling from children (`<Card @select="handler" />`)
  rejected.** Need a `pub fn on_<event>(callback: Rc<dyn
  Fn(...)>)` on the child, wired from the parent at
  composition time. 11.6+ work — the current handler
  emission assumes handlers are static per-component.
- **Compound-type + nominal-type props rejected.** The
  coercion helper only handles primitives + `Nullable<T>` of
  primitives. Nominal types (`user: User? = null`) reject
  with a targeted message — 11.6+ work.
- **Child state resets on parent re-render.** Naive-render
  policy inherited from §9.m D1. Persistent state across
  parent renders needs a component-instance cache keyed by
  position in the render tree (React's fiber approach).
- **Cross-file composition not supported.** The composer
  looks up child components in the SAME `ExpandedViewFile`.
  Importing a component from a sibling `.fitzv` — the
  natural next step for building a real UI library — needs
  the composer to accept multiple files. 11.6+ work.
- **Multi-component wasm bundle size not re-measured.** The
  40 KB gzipped gate from §9.l was measured on the
  single-component counter. Multi-component bundles ADD
  emitted Rust (per-component struct + impls + style helper)
  — probably fits in the 28.6 KB headroom for 2-3 typical
  components, but the gate should be re-measured on a real
  multi-component fixture (e.g. the kanban rewrite in
  11.5.e).

**Files NOT touched by 11.5.d**

- `src/view/parser.rs` — unchanged. The parser already
  accepts capitalised tags via `read_tag_name`.
- `src/view/lexer.rs` — unchanged.
- `src/view/css_parser.rs` — unchanged. Scoping still
  applies per-component; the wrapper div class
  (`__fitz-child-<Name>`) is NOT scoped (it's a stable
  identifier for tooling, not a style hook).
- `src/main.rs` — unchanged. The CLI orchestrator
  (`build_wasm_client_cmd` from 11.5.c) already routes the
  full pipeline; the emitter changes plug in transparently.

11.5.d fully CLOSED with this §9.s section, the corresponding
row refresh in §6, and the memoria update. Next: 11.5.e
(cierre formal — kanban example rewrite + cosmetic emitter
warnings from §9.o).

---

## 9.t 11.5.e — cierre formal (emitter warnings + multi-component showcase + Phase 11.5 close)

Fifth and last commit of Phase 11.5. **Phase 11.5 CLOSED
ENTIRELY** as of this commit. Three coordinated pieces landed
together to formally wrap the block:

**(a) Cosmetic emitter warnings closed (§9.o Debt residual)**

Two warnings tripped by the emitter output ever since 11.4.b:

- `non_snake_case` on `__inject_style_<Component>_<scope>`
  helpers — component names are PascalCase by convention
  (11.5.d confirmed), so the synthesised helper name reflects
  that.
- `unused_parens` on BinOp assignment RHS — BinOp lowering
  wraps sub-expressions in `(...)` to preserve precedence when
  nested (`(a + b) * c` needs the parens); when the BinOp is
  the ENTIRE RHS of `let __rhs = ...;`, the outer parens are
  redundant but harmless.

**Fix**: crate-level `#![allow(non_snake_case, unused_parens)]`
prepended by `emit_module_header`. One line each, killed
uniformly across the emitted crate. Alternatives evaluated:

- **Per-fn `#[allow]` on the style helper** — attempted first,
  reverted. Doesn't cover the `unused_parens` case (Rust
  doesn't accept attribute-on-statement), and gives a
  false-precision impression of "these two warnings only".
- **Rewrite the emitter to not produce them** — for
  `non_snake_case`, would require lowercasing the component
  name in the helper (breaks the invariant baseline where
  every downstream test asserts `__inject_style_Counter_...`).
  For `unused_parens`, would need ast-shape analysis at the
  assignment site (top-level BinOp RHS knows it's outermost;
  nested doesn't). Both refactors cost more LoC than the
  allow, and the emitter output is generated code where the
  "aesthetic" rustc lints aren't user-consumed anyway.

Counter baseline regenerated via the smoke harness — the
header block now carries the `#![allow]` line + explanation
comment. Content is otherwise byte-identical to pre-11.5.e.

**(b) Multi-component showcase fixture (`examples/view/showcase/`)**

The largest fixture the current Phase 11.5.d subset permits.
Structure:

- `Dashboard.fitzv` — `Board` root that composes three
  `<MetricCard title="X" value="N" trend="Y" />` children. Each
  MetricCard has its own `clicks: Int` state + internal `tap`
  event handler, so per-child interactivity works. Static Str
  + Int props on the children exercise the coercion helper.
  Scoped styles per component (both `Board` and `MetricCard`
  carry a `<style scoped>` block).
- `wasm-crate/` scaffold — Cargo.toml + generated src/lib.rs.
  Both are COMMITTED so a fresh clone can `wasm-pack build`
  without running the smoke first. The lib.rs is regenerated
  by the smoke on demand.
- `wasm-crate/.gitignore` — excludes `/pkg`, `/target`,
  `/Cargo.lock` per the counter demo pattern.
- `index.html` — mount shim on `#app`, identical shape to the
  counter demo.
- `README.md` — build/serve recipe + explicit "what this does
  NOT demonstrate" section listing 11.6+ debt (dynamic props,
  event bubbling from children, `<slot>` fallback children,
  cross-file composition, persistent child state across parent
  re-renders).

**Why not the kanban port the original §6 criterion asked for?**

The original criterion in §6 row 11.5 was "Kanban example
(currently in `fitz-liveviews/examples/kanban/`) rewritten to
`.fitzv`, compiles + runs bit-for-bit equivalent via SSR." Two
things happened during 11.5's execution that made that
criterion impossible to meet as literally stated:

1. **§9.p (11.5.a decision) moved `ssr` to a reserved
   vocabulary keyword implemented in 11.6+.** The SSR emitter
   (`view::emit_ssr`) doesn't exist yet, so "compiles + runs
   bit-for-bit equivalent via SSR" cannot be validated.
2. **11.5.d closed with static-only props.** A real kanban
   needs dynamic props (each card shows a different title,
   populated from a runtime list) + event bubbling (drag-drop
   emits events from child up to parent) + persistent child
   state across parent re-renders (card position doesn't
   reset when the board re-renders after a drop). All three
   are documented as 11.6+ debt in §9.s and the rejection
   messages cite them explicitly.

So a "real kanban" would exercise the entire 11.6+ surface
that 11.5 explicitly DOES NOT support. Attempting the port
today would either produce a static-cards fixture that isn't
really a kanban (all cards would have hard-coded titles) OR
require open coding around every rejected feature — neither
serves the pedagogic purpose the original criterion had.

**Pragmatic re-scoping (documented here so future §6 readers
have the trail)**: the multi-component showcase closes 11.5.e
with the largest fixture the current subset permits. The real
kanban port becomes a 11.6+ deliverable, gated on: dynamic
props, event bubbling, cross-file composition (so `Board`,
`Column`, and `Card` can each live in their own `.fitzv`),
and either an SSR emitter or a client-only bundle that still
tolerates the drag interactions. That's a coherent Phase 11.6
scope item, not a stretched 11.5.e.

**Multi-component bundle-size gate: NOT re-baselined here.**
The 40 KB gzipped gate documented in §9.l was measured on the
single-component counter. Multi-component fixtures naturally
add per-component struct + impls + style helper LoC — the
counter's 11.4 KB gzipped baseline probably fits the
Dashboard within the 28.6 KB headroom, but that's speculation
until the `#[ignore]` `build_showcase_wasm` test runs. The
smoke test intentionally does NOT enforce a hard size
assertion (would be flaky across dependency + rustc version
bumps). Re-baselining belongs in 11.6+ once the kanban port
lands and we have a real "many components" datapoint.

**(c) New smoke test `tests/view_showcase_wasm_smoke.rs`**

Parallel to `view_counter_wasm_smoke.rs`:

- `regenerate_showcase_lib_rs` (always runs) — runs the full
  view pipeline on `Dashboard.fitzv` via
  `fitz::view::compose_lib_rs`, writes the result to
  `wasm-crate/src/lib.rs` (idempotent write-if-changed), and
  ALSO writes `wasm-crate/Cargo.toml` via
  `fitz::view::compose_cargo_toml("showcase")`. Structural
  invariants asserted: both `Board` + `MetricCard` structs
  present, root instantiated as `Board` (first-declared), all
  three composition sites produce a `__fitz-child-MetricCard`
  wrapper (`assert_eq!(count, 3)`), and total `mount_into`
  call count is 5 (3 composition sites + 2 delegating
  `mount(selector)` wrappers).
- `build_showcase_wasm` (`#[ignore]`) — shells `wasm-pack
  build --release --target web` and reports raw bundle size.
  No hard gate assertion.

**Files touched**

- `src/view/codegen_wasm.rs` (`emit_module_header`) — +14 LoC
  for the `#![allow]` header + explanation comment.
- `examples/view/counter/wasm-crate/src/lib.rs` — regenerated
  (the `#![allow]` line propagates via the shared header).
- `examples/view/showcase/Dashboard.fitzv` — NEW.
- `examples/view/showcase/wasm-crate/Cargo.toml` — NEW
  (generated by `compose_cargo_toml("showcase")`).
- `examples/view/showcase/wasm-crate/src/lib.rs` — NEW
  (generated by the smoke).
- `examples/view/showcase/wasm-crate/.gitignore` — NEW.
- `examples/view/showcase/index.html` — NEW.
- `examples/view/showcase/README.md` — NEW.
- `tests/view_showcase_wasm_smoke.rs` — NEW smoke harness.
- `docs/fase-11-plan.md` — this §9.t + §6 row 11.5.e updated
  to CLOSED.
- `docs/roadmap.md` — Fase 11 sub-fase 11.5.e refreshed +
  Phase 11.5 marked closed at the top of its block.
- Memoria `project_phase_11_frontend_view.md` — Phase 11.5
  full close entry.

**Debt closed by 11.5.e**

- The two cosmetic emitter warnings from §9.o Debt residual
  (visible since 11.4.b). Marked closed in the §9.o listing.

**Debt / gotchas visible from 11.5.e**

- **Kanban port re-scoped to 11.6+.** Documented in (b)
  above. `docs/fase-11-plan.md` §6 row 11.6 should reference
  the kanban port as the concrete deliverable once dynamic
  props + event bubbling + cross-file composition land.
- **Bundle-size re-baselining.** The 40 KB gzipped gate from
  §9.l needs a fresh measurement on a real multi-component
  fixture. Deferred to 11.6+.
- **No cross-file composition in the fixture.** `Board` and
  `MetricCard` both live in `Dashboard.fitzv`. Splitting them
  into separate files needs the composer to walk multiple
  `ExpandedViewFile`s — 11.6+ work.

**Files NOT touched by 11.5.e**

- `src/view/expand.rs` — unchanged since 11.5.d.
- `src/view/check.rs` — unchanged since 11.5.d.
- `src/view/wasm_build.rs` — unchanged since 11.5.d.
- `src/main.rs` — unchanged since 11.5.c.
- `src/manifest.rs` — unchanged since 11.5.b.

**Phase 11.5 CLOSED ENTIRELY**. The `.fitzv` → wasm-client
pipeline works end-to-end for single-component AND
multi-component fixtures, with the composition subset that
11.5.d anchored (static props, self-closing children, no
event bubbling). Every rejection message points at the right
11.6+ sub-phase for extensions. Next block: Phase 11.6+ —
kanban port drives the concrete deliverable set (dynamic
props, event bubbling, cross-file composition, SSR emitter,
persistent child state).

> **Correction (2026-07-15, 11.6.a)**: the "next block" text
> above implied `11.6+` would prioritise client-side dynamic
> capabilities. That reading is superseded by 11.6.a's
> research (§9.u) — the original §6 row 11.6 promise (SSR
> emitter + fitz-liveviews migration) is prioritised. Client-
> side dynamic capabilities move to §6 row **11.7**. The
> rationale (real user value now vs speculative SPA
> capabilities) lives in §9.u.

---

## 9.u 11.6.a — Research + decision: SSR emitter + fitz-liveviews migration (client-side dynamic capabilities → 11.7)

First sub-commit of 11.6. **Docs-only research + decision —
zero code change** in this commit. Publishes the design that
11.6.b/c/d/e will implement. Same posture as 11.5.a which
pinned the hybrid manifest + `--target` decision.

**What 11.6 has to solve — RECONCILED with §9.t drift**

The plan doc's original §6 row 11.6 promised *"Migration of
`fitz-liveviews`. The library refactors its examples
(`counter`, `chat`, `kanban`, `dashboard`) and its API to
consume `.fitzv` SFCs."* — that is, an SSR emitter that
produces server-side render functions consumed by the existing
fitz-liveviews runtime.

§9.t (11.5.e closure) proto-scoped 11.6+ as client-side
dynamic capabilities (dynamic props + event bubbling +
cross-file composition + persistent child state). That was a
proto-scoping mistake, not a formal decision — it papered
over the ambiguity by saying "kanban port drives the
deliverable set" without asking whether "kanban" meant
server-side (fitz-liveviews) or client-side (SPA / WASM
drag-drop).

**11.6.a resolves the ambiguity in favour of the original §6
plan.** Rationale:

- **Real user value now vs speculative.** The
  `fitz-liveviews` repo has 4 working examples today
  (counter / chat / dashboard / kanban) with hand-written
  raw-string HTML — that IS the current pain point. Any
  `.fitzv` compilation that produces the render functions
  fitz-liveviews already expects delivers immediate DX wins
  to a real, running project. Client-side dynamic
  capabilities don't unblock anything currently in
  production; they are pure showcase.
- **Technically simpler / lower risk of stalling.** SSR emit
  = second backend paralleling `codegen_wasm.rs`, consuming
  the same `ExpandedViewFile`. Output shape is well-
  understood (functional render fn returning HTML string).
  Client-side dynamic capabilities have an open design
  question — which reactivity model? (React fibers /
  Solid signals / Vue tracking / Elm patches) — each choice
  is a permanent API commitment.
- **Coherent web-framework pitch.** SSR-first means Fitz
  ships as "backend + SSR frontend + WebSocket-driven
  updates, one language, one binary" — a defensible pitch
  competitive with Rails/Phoenix/Django. Comparable to
  Elixir + Phoenix LiveView but with static typing. Client-
  side SPA is a nice-to-have, not the killer differentiator.
- **Author's own projects benefit first.** `fitz-liveviews`
  is the author's own repo. `fitzwatch` (private SaaS)
  could eventually mount `.fitzv` server-side without
  architectural change. Neither project needs client-side
  SPA today.

Client-side dynamic capabilities (§9.s Debt residual: dynamic
props, event bubbling from children, cross-file composition,
persistent child state, kanban as client-side SPA drag-drop
showcase) become the concrete scope of **11.7**. Documented in
the §6 row 11.7 rewrite.

**Existing fitz-liveviews API surface — what the SSR emitter targets**

Concrete peek at
`d:/fitz-liveviews/examples/dashboard/src/main.fitz`
(read at 11.6.a docs time — this is real production shape,
not aspirational):

```fitz
from fitz_liveviews import Html, html, live_layout, html_response, ...

@live_component("metric_tile")
type MetricTile {
  count: Int = 0
}

@render_for("metric_tile")
fn metric_tile_render(state: MetricTile) {
  return html("""<div class="tile-body">
    <div class="tile-count">{state.count}</div>
    <button data-flv-click="bump">+1</button>
    ...
  </div>""")
}

@on("metric_tile", "bump")
fn metric_tile_bump(state: MetricTile, payload: Map<Str, Str>) -> MetricTile {
  return MetricTile { count: state.count + 1 }
}
```

The framework runtime (Fitz core v0.20.1 `flv_register`
implicit injection + `fitz-liveviews` lib runtime) already
knows how to route WebSocket frames to `@on` handlers, call
`@render_for` fn on state change, diff HTML, and send patches
to the browser. **The pain point is the `html("""<div>...</div>""")`
raw string** — no syntax highlighting, no HTML validation, no
scoped styles, no template interpolation checking.

**The .fitzv → fitz-liveviews mapping**

A `.fitzv` SFC:

```
@live_component("metric_tile")
component MetricTile {
  state { count: Int = 0 }

  event bump() { count = count + 1 }
  event reset() { count = 0 }

  <template>
    <div class="tile-body">
      <div class="tile-count">{count}</div>
      <div class="tile-buttons">
        <button data-flv-click="bump">+1</button>
        <button data-flv-click="reset">reset</button>
      </div>
    </div>
  </template>

  <style scoped>
    .tile-body { padding: 12px; }
    .tile-count { font-size: 2em; }
  </style>
}
```

lowers via the new `view::emit_ssr` to the classic Fitz code
above — modulo two transformations:

1. **Event body: mutation → new-state return.** `.fitzv` event
   `count = count + 1` becomes `@on` fn
   `return MetricTile { count: state.count + 1 }`. Every
   field the mutation touches gets set from `state.<field>`;
   untouched fields carry over verbatim. Multi-mutation event
   bodies build one final struct literal.
2. **Template interpolation: `{field}` → `{state.field}`.**
   Inside the emitted `html("""...""")`, bare `{count}` refers
   to the state field — the emitter rewrites them to
   `{state.count}` so classic Fitz's string interpolation
   resolves them correctly.

Everything else is 1:1: the `@live_component` decorator stays
on the state type, the `@render_for` fn returns
`html(<raw>)`, the `@on` fns take
`(state: T, payload: Map<Str,Str>) -> T`.

**Design decisions pinned in 11.6.a**

1. **Output format: Fitz source text, not AST.** The emitter
   writes classic `.fitz` source that classic Fitz's lexer +
   parser consumes. Alternatives considered: emit `Program`
   AST directly and inject into the module cache (faster, no
   round-trip). Text-based wins because it's inspectable /
   debuggable / diffable in tests (parallel to the counter
   demo's committed `lib.rs` baseline pattern that 11.5.c/e
   used).
2. **Module resolution: transparent.** The Fitz module loader
   detects `.fitzv` extensions during `from ./Comp import X`
   resolution. When `.fitzv` is found instead of `.fitz`, it
   runs the view pipeline (parse → expand → check →
   `emit_ssr`), then feeds the emitted text through the
   classic lexer+parser as if it were a hand-authored
   `.fitz`. Zero manual `fitz view build` step. This is the
   MOST magical option — the alternative (explicit CLI step
   emitting a sibling `.fitz` file the user commits) is
   documented as fallback if the transparent path proves
   problematic in practice.
3. **Emit target: classic Fitz + `fitz-liveviews` framework
   contract.** The emitted code assumes `fitz-liveviews` is a
   declared dep in `fitz.toml` (`from fitz_liveviews import
   Html, html`). The user has to add the dep once; the
   emitter always emits the `from` import at the top of the
   emitted file. This is coupling from `.fitzv` to
   fitz-liveviews the framework, made explicit.
4. **Style handling: emit `<style>` inline in HTML.** The
   `<style scoped>` block's compiled CSS (with scope suffixes
   applied by 11.3.b's `apply_scope`) gets emitted INSIDE the
   render fn's HTML output, wrapped in a `<style>` tag.
   Simpler than trying to route styles to `<head>` (which
   would need framework-runtime cooperation). fitz-liveviews
   diffs HTML text — a stable `<style>` block at the start
   of the render output collapses to a no-op patch after the
   first render.
5. **Composition (`<Child />`): server-side inline render.**
   In SSR mode, a `<Child prop="v" />` composition site emits
   an inline call to the child's `render_<Child>` fn with a
   fresh `Child { prop: v, ... }` instance. NO
   `component("child_id", ...)` lookup — that's for
   per-instance state which the framework already provides
   via `@live_component` + WebSocket handshake. For 11.6.b
   MVP, static-prop composition is the same subset 11.5.d
   opened.
6. **The two 11.5.d rejection messages stay for SSR too.**
   Dynamic child props / event bubbling from children stay
   rejected with 11.7+ pointers. SSR doesn't change the
   composition subset — the emitter just targets a different
   output.

**Sub-phase plan for 11.6**

- **11.6.a** — Research + decision. **CLOSED 2026-07-15**
  (this §9.u section). Zero code.
- **11.6.b** — Skeleton `view::emit_ssr` module: consumes
  `ExpandedViewFile`, emits classic Fitz source text with
  the state type + `@render_for` fn + `@on` fns +
  `@live_component` decorator. Handles a single `.fitzv`
  file with the shape from `d:/fitz-liveviews/examples/
  counter/src/main.fitz` (single component, state Int,
  events with simple assignment bodies, no `<style>`, no
  `{#if}`/`{#for}`, no composition). Unit tests on the
  emitter output shape (grep-based invariants, matching the
  11.5 emitter test pattern). E2E test: view pipeline on a
  synthetic `.fitzv` produces classic Fitz text that
  lexer+parser accept.
- **11.6.c** — Full event body lowering + template
  interpolation richer + `<style>` inline + `{#if}` /
  `{#for}` template directive lowering. Split into two
  coordinated commits given size:
    - **11.6.c partial CLOSED 2026-07-15** (see §9.w) — RHS
      expression walker `format_fitz_expr_scoped` with
      state-field rewriting AND closure-param local-scope
      tracking (params shadow same-named state fields); the
      same walker powers event body RHS + template
      interpolation + state field defaults. Grammar: BinOp
      / UnaryOp / Call / Field / Index / StrInterp / List
      / Map / Range / Ok / Err / arrow FnExpr. `<style
      scoped>` and `<style global>` blocks now inline at the
      top of the render body's HTML string, with CSS-brace
      escaping (`{`/`}` → `\{`/`\}`) so the CSS syntax
      doesn't collide with classic Fitz's string
      interpolation. Visible view-lexer debt discovered
      during 11.6.c: the view lexer doesn't tokenise `.`
      inside event body context, blocking users from
      writing method calls / field access in `.fitzv`
      source (verified by unit tests that construct the
      `ExpandedComponent` AST directly). Fix belongs to a
      dedicated view-lexer cleanup or to 11.6.c
      continuation.
    - **11.6.c continuation CLOSED 2026-07-15** (see §9.x)
      — `{#if}` / `{#for}` template directives + view
      lexer `.` fix + `emit_template_node_to_html`
      refactored to `emit_template_node_to_pieces` (writes
      `Vec<TemplatePiece>` where `Text` pieces are literal
      HTML and `Expr` pieces are Fitz expressions yielding
      `Str`). Render fn's `html(...)` argument uses **pretty
      form** (triple-string) when all pieces are `Text` and
      **chain form** (`"txt" + (expr) + ...`) when any
      directive is present. `{#if cond}` → `if (cond) {
      <then as Str> } else { <else as Str, or ""> }`.
      `{#if}` / `{#else}` supported. `{#for x in xs}` →
      `__fitz_view_str_join(<iter>.map(fn(x) => <body as
      Str>))` with the `x` binding shadowing any same-named
      state field inside the body. The
      `__fitz_view_str_join(xs: List<Str>) -> Str` helper is
      emitted at module header unconditionally (dead-code
      when unused). View lexer gains `Token::Dot` so method
      calls (`state.count.upper()`) and field access work in
      event body raw-capture; two pre-existing tests that
      needed AST-construction now use natural `.fitzv`
      source. 5 new + 4 inverted-from-rejection tests. See
      §9.x.
- **11.6.d** — Module loader integration: `from ./Comp
  import X` triggers view pipeline when `Comp.fitzv` exists.
  Auto-add `fitz-liveviews` dep resolution when a `.fitzv`
  is present. `<Child />` composition (static props only,
  the 11.5.d subset). E2E test using a two-file project
  (main.fitz + Comp.fitzv) that compiles + runs.
- **11.6.e** — Migrate the 4 fitz-liveviews examples
  (counter → dashboard → chat → kanban) from raw-string HTML
  to `.fitzv`. Each migration is bit-for-bit output diff
  against the current handwritten baseline (or a documented
  intentional divergence). Cierre formal: this §9.u section
  + §6 rows + roadmap + memoria refreshed.

**Debt / gotchas visible from 11.6.a**

- **Event body lowering complexity.** Multi-mutation bodies
  need shape analysis: `count = count + 1; msg = "bumped"`
  becomes `return MetricTile { count: state.count + 1, msg:
  "bumped".to_string() }`. The emitter needs to build a
  fresh struct literal from ALL declared state fields,
  substituting mutated ones. Simple case is trivial; the
  interaction with `{#if}` bodies inside `event` is more
  work.
- **Template `{#for c in cards}` in SSR.** The template
  directive lowers to a Fitz `for` loop that concatenates
  strings. Works, but the generated code becomes verbose
  and less pretty than the source. Acceptable — the
  emitted `.fitz` is generated code, not for human reading.
- **Cross-file `<Child />` composition with SSR.** 11.6.d
  needs the `Comp.fitzv` to be loaded + expanded BEFORE the
  main `.fitz` tries to compose `<Comp />`. The module
  loader has to run in dependency order — same problem
  Rust's build system solves via `mod` declarations.
- **`fitz-liveviews` dep discovery.** The emitter emits
  `from fitz_liveviews import ...` unconditionally when
  emitting a `.fitzv`. If the user's `fitz.toml` doesn't
  declare `fitz-liveviews` as a dep, the compile fails at
  the classic Fitz stage with "unknown module `fitz_liveviews`".
  Fix: 11.6.d has the loader inject the dep automatically
  when `.fitzv` is detected, OR emit a clean error at the
  view pipeline stage pointing at `fitz.toml`.
- **Diffing baseline for migration.** Bit-for-bit output diff
  is aspirational — the migrated versions may deliberately
  clean up the raw HTML strings (whitespace normalisation,
  etc.). 11.6.e will document any intentional divergences.
- **Client-side dynamic capabilities are still 11.7.** The
  §9.s rejection messages still cite 11.7+ (not 11.6+) for
  dynamic props / event bubbling from children / cross-file
  composition of client-side components. `<Child prop={expr}
  />` on a `.fitzv` targeting `wasm-client` remains an error;
  the same shape on a `.fitzv` targeting fitz-liveviews SSR
  also remains an error (same subset, same rejections).

**Files touched by 11.6.a**

- `docs/fase-11-plan.md` — this §9.u section + refresh of §6
  row 11.6 with the sub-phase breakdown a→e (11.6.a marked
  CLOSED, b/c/d/e listed with concise scope) + §6 row 11.7
  rewritten as "client-side dynamic capabilities" (docs-
  only).
- Memoria `project_phase_11_frontend_view.md` — cross-link
  this §9.u and note the split so 11.6.b starts from the
  right assumption.

Not touched: `src/view/*`, `src/main.rs`, `src/manifest.rs`,
`fitz-liveviews/*` — all deferred to 11.6.b+. Code lands one
commit later.

11.6.a fully CLOSED with this §9.u section. 11.6.b (skeleton
`view::emit_ssr` on a single-component fixture) is the next
commit — it's where code touches disk.

---

## 9.v 11.6.b — Skeleton `view::emit_ssr` (single-component, literal-only event bodies)

Second commit of 11.6. **Code lands on disk.** Implements the
design pinned in §9.u for the MVP subset: a `.fitzv` file
with one component + Int/Str/Bool/etc. state + literal-only
event body assignments + a template using Text /
Interpolation / Element / static attrs / interpolated attrs
/ `@event` attrs. `<style>` / `{#if}` / `{#for}` / `<Child />`
/ `<slot />` all reject with a targeted 11.6.c/d/7+ pointer.

**Files touched**

- `src/view/codegen_ssr.rs` (~700 LoC + 20 unit tests):
  - `pub struct SsrEmitError { message, context }` +
    `pub type SsrEmitResult<T>`.
  - `pub fn emit_module_ssr(&ExpandedViewFile) -> SsrEmitResult<String>`
    — the entry point the module loader (11.6.d) will
    consume.
  - `pub fn emit_component_ssr(&ExpandedComponent) -> SsrEmitResult<String>`
    — convenience wrapper for tests + single-component
    consumers.
  - Private helpers:
    - `emit_module_header` — writes the `from
      fitz_liveviews import Html, html` line.
    - `emit_component_ssr_into` — orchestrator; rejects
      `<style>` per MVP scope, otherwise calls the three
      section emitters in order.
    - `emit_state_type` — writes
      `@live_component("<Name>") type <Name> { ... }`.
    - `emit_render_fn` — writes
      `@render_for("<Name>") fn <Name>_render(state: <Name>) -> Html { ... }`
      whose body is `return html("""<template as HTML>""")`.
    - `emit_event_fn` — writes
      `@on("<Name>", "<event>") fn <Name>_<event>(state: <Name>, payload: Map<Str, Str>) -> <Name>`
      whose body accumulates mutations from the event's
      `body: Vec<Stmt>` and returns a fresh struct literal
      carrying every declared state field (mutated fields
      take the assigned RHS; untouched fields carry over
      from `state.<field>`).
    - `format_event_rhs` — accepts `Expr::Int/Float/Bool/Str/Null`
      + `Expr::Ident(<state_field>)`; everything else
      rejects with the 11.6.c pointer.
    - `emit_template_node_to_html` — walks the template AST,
      recursively appending HTML text into a buffer. Text
      and Interpolation nodes emit inline; Element nodes
      emit `<tag attrs>children</tag>` (or self-closing);
      If/For/Slot/ChildComponent all reject with 11.6.c/d/7+
      pointers.
    - `emit_attr_to_html` — dispatches on the three
      `ExpandedAttr` shapes: Static verbatim (with `"`
      escaped), Interpolation as `name="{state.<field>}"`,
      Event as `data-flv-<event>="<handler>"`.
    - `format_template_interpolation` — accepts only bare
      state-field identifiers (rewritten as
      `state.<field>`); everything else rejects with the
      11.6.c pointer.
    - `format_type_expr_source` — delegates to
      `TypeExpr::display_name` (kept local for future
      emitter-specific tweaks).
    - `format_expr_source` — restricted to literals for
      state field defaults; rejects everything else with
      the 11.6.c pointer.
- `src/view/mod.rs` — new `pub mod codegen_ssr;` +
  re-exports of `emit_module_ssr`, `emit_component_ssr`,
  `SsrEmitError`, `SsrEmitResult`.

**Tests (20 unit tests in `codegen_ssr::tests`)**

Grouped by responsibility, matching the emitter walker
structure:

- Header — 1 test: `emit_module_header_imports_html_and_html_ctor`.
- State type — 2 tests: single-field + multi-primitive fields.
- Render fn — 4 tests: fn signature shape, `{field}` →
  `{state.field}` rewrite, `@click` → `data-flv-click`
  translation, static attrs carried verbatim.
- Event fn — 4 tests: signature shape, fresh struct literal
  carrying all fields (mutated + untouched), multi-mutation
  bodies (`a = 1; b = 2` both emit), bare-ident RHS rewrite.
- Rejections — 7 tests: BinOp RHS / scoped style / `{#if}` /
  `{#for}` / `<Child />` / `<slot />` / non-state-field
  ident in interpolation — each asserts the message cites
  the correct future sub-phase (11.6.c / 11.6.d / 11.7+).
- Round-trip — 1 test: the acceptance criterion. Feeds a
  small `.fitzv` through parse → expand → emit_ssr, then
  runs the emitted text through `crate::lexer::tokenize` +
  `crate::parser::parse`. Both stages must succeed.
- Convenience — 1 test: `emit_component_ssr` (single) matches
  `emit_module_ssr(file_with_one_component)` bit-for-bit.

**Debt / gotchas visible from 11.6.b**

- **Type checker validation is NOT part of the round-trip
  acceptance criterion.** The emitted source references
  `fitz-liveviews` types (`Html`, `html`) that are only
  defined when the `fitz-liveviews` dep is loaded. The
  round-trip test validates lex + parse only; full checker
  validation lands in 11.6.d when the module loader wires
  up the dep resolution.
- **HTML escaping in Text nodes is not applied.** The
  template's Text nodes get emitted verbatim into the HTML
  string. If a template author writes `<span>&nbsp;</span>`,
  that survives unchanged (which is correct). If they write
  `<span>3 < 5</span>`, the raw `<` renders as invalid HTML
  in the browser. Same behaviour classic HTML sub-parsers
  have — real fix is escaping which we defer until we see a
  real pattern.
- **`{state.field}` interpolation uses classic Fitz's built-in
  string interpolation** — which converts values to their
  Display representation. For `Str` fields the output is
  raw (unescaped) — an XSS vector if state comes from user
  input. fitz-liveviews's runtime is expected to escape at
  the diff step (real check TBD in 11.6.d).
- **Multi-statement event body with non-assignment stmts.**
  The MVP rejects mixed bodies (assign + print, assign + if,
  etc.) — event `bump()` can hold N `Stmt::Assign` but not
  a mixed `if x { count = 1 } else { count = 0 }`. 11.6.c
  will lower `if` bodies to `if x { <fields> } else { <fields> }`
  branch structure or similar.
- **Component name IS used verbatim for the type name +
  function names.** No case conversion (`counter_render` vs
  `Counter_render`). Matches convention when the .fitzv
  component is PascalCase (the vast majority of hand-written
  fitz-liveviews components use PascalCase already —
  `MetricTile`, `CardEditor`, etc.). Users who write
  `component counter { ... }` (lowercase) will get
  `counter_render` — legal but a style outlier. No warning
  emitted; the classic checker will handle any downstream
  naming issues.
- **`<style scoped>` blocks reject the WHOLE component**
  (fail-fast) rather than emitting the state type + render
  + events and just dropping the style. Chosen to keep the
  MVP scope guardrails loud — a `.fitzv` with `<style
  scoped>` is a request the emitter cannot honour, so failing
  clean is better than silently emitting a component that
  looks styled but isn't.

**Files NOT touched by 11.6.b**

- `src/main.rs` — no CLI wiring. The emitter is invoked
  ONLY by tests today. Module loader integration lands in
  11.6.d.
- `src/manifest.rs` — no changes. `.fitzv` files continue
  to be a `wasm-client` entry (per 11.5.b's `mount = "#app"`
  requirement) or unused by the classic `fitz build`. SSR
  target detection is a 11.6.d concern.
- `src/view/expand.rs` / `src/view/check.rs` — the SSR
  emitter consumes the same `ExpandedViewFile` shape the
  WASM emitter does. No changes to the pipeline.

11.6.b fully CLOSED with this §9.v section, the corresponding
row refresh in §6, and the memoria update. Next: 11.6.c
(full event body lowering + `{#if}`/`{#for}` template
lowering + `<style scoped>` inline emission).

---

## 9.w 11.6.c (partial) — Full RHS lowering + `<style>` inline

Third commit of 11.6, closing PART of the 11.6.c work
promised in §9.u's sub-phase plan. The RHS expression walker
+ style inline emission ship together as one cohesive
extension of the SSR emitter's shape. Template directives
(`{#if}` / `{#for}`) stay rejected in this commit — they
need a `Vec<TemplatePiece>` refactor of the emit path that
belongs in its own commit ("11.6.c continuation").

**Design decisions**

- **One walker for every expression context.** The new
  `format_fitz_expr_scoped` handles event body RHS,
  template `{expr}` interpolations, AND state-field
  defaults. Same grammar, same rewriting rules, same
  rejection surface. The three wrappers
  (`format_event_rhs`, `format_template_interpolation`,
  `format_expr_source`) become thin adapters.
- **State-field rewrite + local-scope tracking.** The
  walker takes two identifier slices:
    - `state_field_names` — declared state fields; matching
      `Ident(name)` rewrites as `state.<name>`.
    - `local_scope` — identifiers introduced by enclosing
      `FnExpr`s (closure params). Matching `Ident(name)`
      emits verbatim, taking precedence over
      state-field rewrite (params shadow same-named state
      fields).
  Anything else (free var) rejects with a 11.7+ pointer.
- **BinOp / UnaryOp always wrapped in outer parens.** Fitz
  and Rust have similar precedence but not identical; the
  safe choice is to parenthesise every non-leaf sub-
  expression. `count + 1` becomes `(state.count + 1)`,
  `-x` becomes `(-x)`. Slightly noisy but correctness-
  preserving.
- **StrInterp preserves `{expr}` semantics via recursive
  walk.** `"count is {count}"` in an event RHS becomes
  `"count is {state.count}"` in the emitted source
  (bare-ident state-field rewrite happens inside the
  interpolated segment).
- **FnExpr arrow form only.** `fn(x) => x + 1` works;
  multi-statement bodies (`fn(x) { let y = x; return y }`)
  reject with a 11.7+ pointer. Real event body statement
  lowering — `if x { count = 1 } else { count = 0 }` —
  belongs in the 11.6.c continuation or 11.7+.
- **Async closures rejected.** `async fn(...) => ...`
  makes no sense inside a synchronous render / event body;
  reject with a 11.7+ pointer.
- **Slice / ListComp / MapComp / Match / If-as-expression
  / StructLit / Await / Try / NamedArg / Bytes / Tuple**
  all reject with 11.7+ pointers (11.6.d for StructLit
  specifically — needed for the state-construction case).
  This closes off the walker's grammar with clear future-
  sub-phase pointers on every rejection.
- **`<style>` inline at the top of the render body's
  HTML.** The compiled CSS (with 11.3.b's `apply_scope`
  applied for `scoped`, verbatim for `global`) prepends
  the template output as `<style>...</style>`. Braces get
  escaped (`{` → `\{`, `}` → `\}`) so the CSS syntax
  doesn't collide with classic Fitz's string
  interpolation `{expr}` inside `html("""...""")`.
- **`<style>` inline: fail-fast rejection removed.**
  11.6.b failed the entire component when a `<style>`
  block was declared. 11.6.c partial inlines it.
- **Visible view-lexer debt: `.` in event body context.**
  Discovered during 11.6.c testing: `event bump() { count
  = state.count + 1 }` fails at `view::parse` because the
  view lexer refuses to tokenise `.` outside template
  contexts. The SSR walker itself supports method calls +
  field access — verified by unit tests that construct the
  `ExpandedComponent` directly. The lexer fix is a
  dedicated view-lexer cleanup (add `.` to the accepted
  character set inside event body raw-capture) and lands
  alongside the 11.6.c continuation or as a separate
  view-lexer follow-up. Documented in the module doc
  comment and in this section.

**Files touched**

- `src/view/codegen_ssr.rs` (~250 LoC net):
  - New `format_fitz_expr(&Expr, &[&str], &str, &str) ->
    SsrEmitResult<String>` — public thin wrapper.
  - New `format_fitz_expr_scoped(&Expr, &[&str], &[&str],
    &str, &str) -> SsrEmitResult<String>` — inner walker
    with `local_scope` for closure params.
  - Old `format_event_rhs` and `format_template_interpolation`
    become thin delegates over the walker.
  - `format_expr_source` (state field defaults) also
    delegates, passing empty `state_field_names` (defaults
    can't reference state fields — that's circular).
  - `emit_component_ssr_into` no longer fails on `<style>`;
    the fail-fast rejection is removed.
  - `emit_render_fn` prepends `<style>` inline into the
    HTML body when `component.style` is `Some`.
  - New helper `escape_css_braces_for_fitz_interp` for the
    `{`/`}` → `\{`/`\}` rewrite on the CSS body.
  - New helpers `format_binop_source`, `format_unaryop_source`,
    `expr_kind_label` for the walker.
  - Old rejection tests for BinOp / scoped style inverted
    to positive `phase_11_6_c_emit_accepts_*` tests.
  - 10 new unit tests under `phase_11_6_c_*`:
    - `emit_accepts_binop_rhs_in_event_body` — precedence
      parens verified.
    - `emit_accepts_arithmetic_rhs_multiple_ops` — nested
      BinOp with correct grouping.
    - `emit_accepts_str_interp_rhs_with_field_rewrite` —
      `"{count}"` → `"{state.count}"` inside interpolation.
    - `emit_accepts_method_call_via_direct_expr_construction`
      — AST-constructed fixture that bypasses the view
      lexer's `.` limitation.
    - `emit_accepts_arrow_closure_via_direct_expr_construction`
      — same, verifying local-scope tracking on closure
      params.
    - `emit_accepts_richer_template_interpolation` —
      `{count + 1}` in template → `{(state.count + 1)}`
      inside emitted HTML.
    - `emit_accepts_field_access_in_template_interp` —
      `{name.upper()}` works in template context (view
      lexer accepts `.` inside `{...}`).
    - `emit_inlines_scoped_style_at_top_of_render_body`
      — verifies `<style>` tag + ordering vs template
      content.
    - `emit_inlines_global_style_with_escaped_braces` —
      global CSS inline with `\{`/`\}` escapes.
    - `still_rejects_if_directive_pending_continuation` +
      `still_rejects_for_directive_pending_continuation`
      — regression guards for the deferred directives.
    - `round_trip_end_to_end_with_widened_grammar` —
      counter fixture with BinOp arithmetic + template
      interpolation of two state fields + scoped style;
      the emitted source must lex + parse cleanly through
      classic Fitz.

**Debt / gotchas visible from 11.6.c (partial)**

- **View lexer doesn't tokenise `.` in event body raw
  context.** Blocks `.fitzv` authors from writing method
  calls / field access in event bodies. Real fix: extend
  the view lexer's event-body char set. Not part of 11.6.c
  scope; deferred to a dedicated view-lexer cleanup or
  folded into 11.6.c continuation.
- **Template directives `{#if}` / `{#for}` still reject.**
  Belongs to 11.6.c continuation — needs Vec<TemplatePiece>
  refactor + emitted helper fn (`__fitz_view_str_join`) at
  module header for `{#for}` since classic Fitz's
  `List<Str>` methods don't include `.join()`.
- **Multi-statement event bodies with non-assignment
  statements still reject.** `if x { count = 1 } else {
  count = 0 }` inside an event body would need statement
  lowering to a struct literal with an if-as-expression on
  each mutated field. Deferred to 11.6.c continuation
  alongside `{#if}` (same infrastructure).
- **StrInterp `FormatSpec` (e.g. `{x:0.2f}`) not
  preserved.** The walker drops the format spec and emits
  bare `{state.<field>}`. Format specs on state-field
  interpolations are rare in .fitzv real-world usage; the
  fix is a `format_format_spec` helper if pressure
  appears.
- **StrInterp Lit segment escapes: only `"` and `\`.** No
  `\n` / `\t` escaping — a Lit segment containing a
  newline gets emitted verbatim, which classic Fitz's
  single-quoted string might reject. Deferred until a real
  fixture reveals the case.

**Files NOT touched**

- `src/view/expand.rs` / `src/view/check.rs` — the pipeline
  shape is unchanged. 11.6.c widens the emitter's
  ACCEPTED grammar, not the checker's or the expander's.
- `src/main.rs` — no CLI wiring; SSR emitter is still
  invoked only by tests. Module loader integration is
  11.6.d.
- `fitz-liveviews/*` — no migration yet. Migration lands
  in 11.6.e once the emitter closes 11.6.c continuation +
  11.6.d.

11.6.c (partial) CLOSED with this §9.w section, the
corresponding row refresh in §6, and the memoria update.
Next: 11.6.c continuation (`{#if}`/`{#for}` template
directives via `Vec<TemplatePiece>` refactor).

---

## 9.x 11.6.c continuation — `{#if}` / `{#for}` template directives + view lexer `.` fix

Fourth commit of Phase 11.6, closing the SECOND half of
11.6.c. Template directive support lands together with the
view lexer's `.` fix — packaged in one commit because the
lexer fix destraba pre-existing tests that had to
AST-construct their fixtures.

**Design decisions**

- **`Vec<TemplatePiece>` model for render body.** The
  render fn's HTML string is built as a list of pieces
  where each piece is either literal `Text(String)` (may
  contain Fitz `{state.<field>}` interpolation syntax and
  backslash-escaped `\{` / `\}` from CSS pre-escape) or
  `Expr(String)` holding a Fitz expression that evaluates
  to `Str` (produced by the `{#if}` / `{#for}` lowerings).
  `push_text` merges consecutive Text pieces so the pretty
  form emits as one contiguous string when possible.
- **Two serialisation forms.**
    - **Pretty form (triple-string)** — when every piece
      is `Text` (no directives), emit as `html("""<full
      HTML>""")`. Preserves the readable shape that 11.6.b
      and 11.6.c partial produced.
    - **Chain form** — when any directive is present, emit
      as `"txt1" + (expr1) + "txt2" + (expr2) + ...`. Each
      `Text` piece becomes a single-quoted Fitz string
      literal (only `"`, `\n`, `\r` escaped —
      backslash-prefixed sequences like `\{` / `\}` from
      CSS pre-escape pass through verbatim). Each `Expr`
      piece wraps in parens for precedence safety.
- **`{#if cond}` lowering.** Emits as `Expr("if (<cond>)
  { <then as Str> } else { <else as Str, or ""> }")`. Both
  branches recursively emit pieces + serialise via
  `serialize_pieces_as_html_arg` so nested directives work
  naturally. `{#if}` without `{#else}` yields an empty
  string in the else branch.
- **`{#for x in xs}` lowering.** Emits as `Expr(
  "__fitz_view_str_join(<iter>.map(fn(x) => <body as
  Str>))")`. The `x` binding is pushed to the walker's
  `local_scope` when walking the body so `Ident("x")`
  emits verbatim rather than being rewritten as
  `state.x` (which would be wrong).
- **`__fitz_view_str_join(xs: List<Str>) -> Str` helper.**
  Classic Fitz's `List<T>` built-in methods do not
  include `.join()`, so `{#for}` lowering needs a helper.
  Emitted at module header unconditionally (dead code
  when the module doesn't use `{#for}`, but the cost is
  negligible and the unconditional emit keeps the
  pipeline simple).
- **View lexer gains `Token::Dot`.** The view lexer's
  top-level char set now includes `.`. Emitted as a bare
  `Token::Dot` and re-serialised by
  `capture_balanced_body_raw` via `append_token_source`.
  Tight-binding rule in `needs_space_before` keeps
  `state.count.upper()` reconstructing as one token
  sequence rather than `state . count . upper ()`. Two
  pre-existing tests (`emit_accepts_method_call_via_
  direct_expr_construction` + `emit_accepts_arrow_
  closure_via_direct_expr_construction`) drop the
  AST-construction dance in favour of natural `.fitzv`
  source — the tests survive as `emit_accepts_method_
  call_rhs` and `emit_accepts_arrow_closure_rhs`.

**Files touched**

- `src/view/lexer.rs`:
  - New `Token::Dot` variant.
  - Match arm in the top-level lexer that emits
    `Token::Dot` on `.`.
- `src/view/parser.rs`:
  - `append_token_source` handles `Token::Dot` by pushing
    `.`.
  - `needs_space_before` adds `Token::Dot` to the
    tight-binding set so `state.count` reconstructs
    without stray whitespace.
- `src/view/codegen_ssr.rs`:
  - `emit_template_node_to_html` → `emit_template_node_
    to_pieces` (writes `Vec<TemplatePiece>` instead of
    `&mut String`). New `local_scope: &[&str]` parameter
    for `{#for}` bindings.
  - `emit_attr_to_html` → `emit_attr_to_pieces` (parallel
    signature change; passes `local_scope` through).
  - `emit_render_fn` accumulates pieces + calls
    `serialize_pieces_as_html_arg` at the end.
  - `emit_module_header` emits the `__fitz_view_str_join`
    helper.
  - New types + helpers: `TemplatePiece` enum, `push_text`,
    `serialize_pieces_as_html_arg`,
    `fitz_str_literal_for_chain_form`.
  - `format_template_interpolation` removed (dead code
    after the direct call to `format_fitz_expr_scoped` in
    the Interpolation case).
  - Module doc comment updated to reflect the new
    supported grammar + emit shapes.
- `src/view/codegen_ssr.rs` tests:
  - Two pre-existing tests simplified to natural `.fitzv`
    source now that the view lexer accepts `.` in event
    body context:
    - `phase_11_6_c_emit_accepts_method_call_rhs`
    - `phase_11_6_c_emit_accepts_arrow_closure_rhs`
  - Two pre-existing rejection tests inverted to positive
    checks (`emit_rejects_if_directive_citing_11_6_c` +
    `emit_rejects_for_directive_citing_11_6_c` were 11.6.b
    tests):
    - `phase_11_6_c_cont_emit_accepts_if_directive`
    - `phase_11_6_c_cont_emit_accepts_for_directive`
  - Two pre-existing "still rejects" tests from 11.6.c
    partial folded into positive checks:
    - `phase_11_6_c_cont_module_header_emits_str_join_helper`
      (verifies the helper is present in every emit).
  - 5 new tests:
    - `phase_11_6_c_cont_emit_if_with_else_branch`
    - `phase_11_6_c_cont_emit_if_without_else_uses_empty_string`
    - `phase_11_6_c_cont_for_body_uses_bare_var_not_state`
      — regression guard for the `{#for x in xs}` local-
      scope tracking.
    - `phase_11_6_c_cont_directive_bearing_module_round_
      trips_through_classic_fitz` — end-to-end fixture with
      `{#if}` + `{#for}` + BinOp arithmetic + template
      interp + scoped style.
    - `phase_11_6_c_cont_all_text_template_still_uses_
      pretty_triple_string` — regression guard for the
      pretty form.

**Total test delta**: net +5 SSR tests (34 → 34; 2 old
rejection tests removed, 4 inverted to positive, 5 new
positive tests added). Full view test suite: **356 → 356**
(no regressions; 22 net new positive assertions across the
rename/invert/add).

**Debt / gotchas visible from 11.6.c continuation**

- **`__fitz_view_str_join` emitted unconditionally.**
  Modules without `{#for}` carry the helper as dead code.
  Fix: scan all templates for `{#for}` presence before
  emitting the header, conditionally include. Skipped in
  this commit because the negligible cost doesn't justify
  the extra scan pass.
- **StrInterp `FormatSpec` still dropped by walker.**
  Same as 11.6.c partial. Format specs on state-field
  interpolations remain rare in fitz-liveviews real usage;
  refinement lands if pressure appears.
- **`{#if cond}` with non-bare-Bool cond emits `if
  (<expr>)` — classic Fitz may reject if `<expr>` doesn't
  evaluate to Bool.** The SSR walker doesn't insert type
  coercion; user is responsible for supplying a Bool-typed
  condition. Same rule the classic Fitz checker enforces.
- **`{#for}` body with statements (not just expression
  content).** The `<body as Str>` branch handles template
  nodes fine; anything more complex (a `<let>` inside a
  directive?) isn't in the current template AST anyway.
- **Multi-statement event bodies still reject.** Event
  bodies with `if x { count = 1 } else { count = 0 }`
  inside would need statement lowering to a struct
  literal with an if-as-expression per mutated field.
  Deferred to Phase 11.7+ alongside richer event
  semantics.

**Files NOT touched**

- `src/view/expand.rs` / `src/view/check.rs` — the
  pipeline shape is unchanged.
- `src/main.rs` — no CLI wiring; SSR emitter is still
  invoked only by tests. Module loader integration is
  11.6.d.
- `fitz-liveviews/*` — no migration yet.

**Phase 11.6.c CLOSED ENTIRELY** with this §9.x section +
the §6 row refresh (partial + continuation both marked
CLOSED). The SSR emitter now handles the FULL template
grammar (Text, Interpolation, Element, static + interpolated
+ event attrs, `{#if}` + `{#else}`, `{#for}`, `<style>`
inline) + the full expression grammar (BinOp, UnaryOp, Call,
Field, Index, StrInterp, List, Map, Range, Ok, Err, arrow
FnExpr) with proper state-field rewriting + closure-param
local-scope tracking. Next: 11.6.d — module loader
integration + `<Child />` composition + `.fitzv` transparent
handling in `from ./Comp import X`.

---

## 9.y 11.6.d — Module loader integration + same-file `<Child />` composition

Sub-step of 11.6. **Closes 11.6.d entirely.** Wires the view
pipeline into the classic module loader so a `.fitzv` sibling
is a drop-in classic Fitz module (parse → expand → check →
emit_module_ssr, all internal). Also lands same-file
`<Child />` composition in the SSR emitter — the last node
type the emitter rejected.

### Loader integration

The classic module loader has five entry points that resolve
an import path to a file on disk. Every one of them now tries
`.fitz` FIRST and `.fitzv` as fallback:

1. **`src/evaluator.rs::resolve_module_path`** — used by `fitz
   run`. Now builds `Vec<PathBuf>` candidates via a
   `build_paths` closure that emits both extensions per
   base directory; the existing "relative to `base_dir`,
   then relative to `import_root`" logic is preserved. Order:
   `<base>/<seg>.fitz` → `<base>/<seg>.fitzv` →
   `<import_root>/<seg>.fitz` → `<import_root>/<seg>.fitzv`.
2. **`src/codegen.rs::ModuleLoader::resolve_path`** — used by
   `fitz build`. Delegates to the new
   `crate::view::resolve_module_file_candidates(&Path, &str)`
   helper which does the two-extension check with existence
   probes; falls back to the classic `.fitz` path when
   neither exists so the downstream `canonicalize` raises
   the historical `module not found` error message.
3. **`src/codegen.rs::resolve_loader_import_file_path`** —
   used by the codegen pre-scan paths
   (`pre_scan_imported_auth_provider_for_loader`,
   `pre_scan_imported_background_fns_for_loader`,
   `pre_scan_imported_middleware_fns_for_loader`). Same
   delegation to `resolve_module_file_candidates`.
4. **`src/main.rs::resolve_import_file_path`** — used by
   the CLI pre-scan paths (`pre_scan_imported_auth_provider`,
   `pre_scan_imported_background_fns`). Same delegation via
   `fitz::view::resolve_module_file_candidates`.
5. **`src/lsp.rs::resolve_import_file_path_lsp`** — used by
   the LSP pre-scan (`pre_scan_imported_auth_provider_lsp`,
   `pre_scan_imported_background_fns_lsp`) and the
   cross-module resolvers (`resolve_cross_module_definition`,
   `from_import_completions`). Same delegation.

**Priority rule**: `.fitz` wins when both extensions exist in
the same directory. Backward-compat is the whole point —
projects with an existing `Card.fitz` continue to use it, even
if the author drops in an experimental `Card.fitzv` alongside.

Once a `.fitzv` file is resolved, every entry point routes the
source through the new
`crate::view::transform_fitzv_source(source, path) ->
Result<String, FitzError>` bridge before feeding to
`crate::lexer::tokenize`. The bridge runs the four-stage view
pipeline:

```rust
parse(source) → expand(&raw) → check(&expanded) → emit_module_ssr(&expanded)
```

Any stage error wraps into a single `FitzError` with
`ErrorKind::InvalidSyntax`, best-effort line/column, and a
message naming the file path plus the stage that failed
(`view parse error` / `view expand error` / `view check
errors` / `view emit_ssr error`). Check errors accumulate —
the message lists every error with its context label.

Pre-scan paths (silent-fallback policy heritage) still skip
gracefully when the transform errors — the runtime loader
(`evaluator::load_module` or codegen `ModuleLoader::load_module`)
will surface the same error later. This preserves the
pre-existing "the pre-scan never errors, only the loader
does" invariant.

### `<Child />` composition (same-file MVP)

The `<Child />` template node was the last one still
rejecting. It now lowers to a Fitz `Expr` piece that calls
the child's render fn on a struct literal built from the
supplied static props:

```text
Parent.fitzv:
  component Parent {
    <template><Card title="hello" count="42" /></template>
  }
  component Card {
    state { title: Str = "", count: Int = 0 }
    <template>{title}: {count}</template>
  }

Emitted Fitz (parent render body):
  return html("" + (Card_render(Card { title: "hello", count: 42 }).raw) + "")
```

Key mechanics:

- **`.raw` unwrap** — `Card_render(...)` returns
  `Html = { raw: Str }` (fitz-liveviews's canonical type).
  Accessing `.raw` yields a bare `Str` we splice into the
  parent's chain-form html body.
- **Chain form forced** — a template with `<Child />` is
  never all-Text, so the `serialize_pieces_as_html_arg`
  function emits chain form even if no `{#if}` / `{#for}`
  directive appears. Text pieces around the `<Child />`
  quote as Fitz string literals.
- **Prop coercion** — the new pub(crate) helper
  `coerce_child_prop_raw_value_to_fitz_literal(raw, type_expr,
  child_name, field_name) -> SsrEmitResult<String>` parallels
  11.5.d's `coerce_child_prop_raw_value` (which returns a
  Rust literal for the WASM path) but produces classic Fitz
  source. Same accepted subset — `Str` / `Int` / `Float` /
  `Bool` / `Nullable<T>` of a primitive. Nominals + generics
  + function types + tuples all reject with 11.7+ pointers.
- **Defaults kick in for omitted props** — the emitted
  struct literal supplies only mentioned props, so classic
  Fitz's default-application rule (Phase 3.2) fills the
  rest from the child type's declared `default:` clauses.
  This matches the intuition "if you don't say `count="7"`,
  you get the child's declared default".
- **Same-file constraint** — the emitter looks up the child
  in a `siblings: &[ExpandedComponent]` slice threaded from
  `emit_module_ssr` (which sees `file.components`) through
  `emit_component_ssr_into` → `emit_render_fn` →
  `emit_template_node_to_pieces`. `emit_component_ssr`
  (single-component test convenience) passes
  `std::slice::from_ref(component)` — so a `<Child />`
  reference from a single-component fixture resolves to
  ONLY the parent itself → not found → clear cross-file
  error pointing at 11.6.e ("cross-file `<Child />`
  composition is deferred to Phase 11.6.e; for now, declare
  both parent and child in the same `.fitzv` module").

### Bridge shape

New pub API in `src/view/mod.rs`:

```rust
/// True when `path` ends in `.fitzv` (case-insensitive).
pub fn is_fitzv_extension(path: &std::path::Path) -> bool;

/// Try both `.fitz` and `.fitzv` for `<parent_dir>/<stem>`.
/// `.fitz` wins if both exist (backward-compat).
pub fn resolve_module_file_candidates(
    parent_dir: &std::path::Path,
    stem: &str,
) -> Option<std::path::PathBuf>;

/// Run `parse → expand → check → emit_module_ssr`, wrapping any
/// error into a FitzError that names the offending `.fitzv`.
pub fn transform_fitzv_source(
    source: &str,
    file_path_for_errors: &std::path::Path,
) -> Result<String, crate::error::FitzError>;
```

Consumers pick two of the three: `resolve_module_file_candidates`
for the file-search step and `is_fitzv_extension` + `transform_fitzv_source`
for the read step. The three-helper split is intentional —
extension check is a hot-path predicate (used by every module
load, not just view sources); resolution is filesystem-touching
(so a helper); transform is heavyweight (four pipeline stages).

### Tests

- **7 SSR unit tests** in `codegen_ssr::tests::phase_11_6_d_*`:
  `emit_composes_same_file_child_with_no_props` (baseline
  Expr piece + child render fn present) /
  `emit_composes_same_file_child_with_primitive_props` (all
  four primitives coerce) / `emit_omits_undeclared_props_so_defaults_apply`
  (only supplied props appear in the struct literal) /
  `emit_rejects_child_declared_in_a_different_file` (cross-
  file cites 11.6.e) / `emit_rejects_nullable_str_null_literal_and_string_prop`
  (Nullable primitives coerce with `null` bare and value quoted) /
  `composition_forces_chain_form_html_arg` (a text template
  with `<Child />` uses chain form) / `module_with_composition_round_trips_through_classic_fitz`
  (emitted source lexes + parses cleanly through classic Fitz).
- **8 loader-bridge unit tests** in
  `view::loader_bridge_tests::*`:
  `is_fitzv_extension_matches_lowercase_and_uppercase` /
  `resolve_module_file_candidates_prefers_classic_when_both_exist` /
  `resolve_module_file_candidates_falls_back_to_fitzv_when_only_view_exists` /
  `resolve_module_file_candidates_returns_none_when_neither_exists` /
  `transform_fitzv_source_emits_classic_fitz_from_a_simple_component` /
  `transform_fitzv_source_wraps_view_parse_error_with_path` /
  `transform_fitzv_source_wraps_downstream_errors_with_the_path` /
  `transform_fitzv_source_produces_source_that_classic_fitz_lexes_and_parses`.
- **3 cli_e2e tests** in `phase_11_6_d_*`:
  `import_of_fitzv_sibling_is_transformed_through_view_pipeline`
  (main.fitz + Card.fitzv, `fitz run` fails at the
  `fitz_liveviews` dep lookup — proves the transform
  happened) / `import_prefers_dot_fitz_over_dot_fitzv_when_both_exist`
  (both siblings present, classic wins, `fitz run` succeeds
  without touching `fitz_liveviews`) / `broken_fitzv_import_surfaces_view_pipeline_error_with_path`
  (malformed `.fitzv` sibling surfaces its view-pipeline
  error with the file path — not a generic "module not
  found").

### Debt residual (11.6.e scope)

- **Cross-file `<Child />`** — a parent component declared in
  `main.fitz` (or another `.fitzv`) cannot compose a child
  declared in a sibling `.fitzv`. The blocker is that the
  parent's expand + check step doesn't see the sibling's
  `ExpandedComponent` (the view pipeline is per-file). Fix:
  thread the loader's expanded-file cache into the checker
  (roughly parallel to how `TypeEnv` threads nominal
  registration across modules).
- **Auto-inject `fitz-liveviews` dep** — the emitter emits
  `from fitz_liveviews import Html, html` unconditionally.
  If the user's `fitz.toml` doesn't declare the dep, the
  compile fails at the classic Fitz stage with "unknown
  module `fitz_liveviews`". Auto-injection at loader time
  is 11.6.e work; the honest error path is documented in
  the 11.6.d E2E tests.
- **LSP pre-scan silent-fallback** — the LSP pre-scan
  paths silence view-transform errors (consistent with
  read/lex silent-fallback policy). The main loader
  surfaces the error on next `fitz run` / `fitz build`
  / `fitz check`. Refinement (surface as LSP diagnostic
  from the `.fitzv` file) is a nice-to-have.
- **`fitz check` doesn't surface loader errors either**
  — because `check_program_with_pyi_stubs_and_deps` is a
  static walk that doesn't attempt to LOAD imported
  modules. The pre-scan reads them but silence-falls-back.
  `fitz run` and `fitz build` are the paths that surface
  errors today. This is pre-existing pipeline behaviour,
  not a 11.6.d regression.

### Files touched by 11.6.d

- `src/view/mod.rs` — three new pub helpers +
  `loader_bridge_tests` module with 8 tests.
- `src/view/codegen_ssr.rs` — `siblings` threaded through
  `emit_component_ssr_into` → `emit_render_fn` →
  `emit_template_node_to_pieces`, the `<Child />` reject
  arm replaced with real emit logic +
  `format_child_composition` +
  `coerce_child_prop_raw_value_to_fitz_literal` +
  updated module doc comment + 7 new tests.
- `src/evaluator.rs::resolve_module_path` +
  `load_module_inner` — dual-extension candidate build +
  view transform on `.fitzv` sources.
- `src/codegen.rs::ModuleLoader::resolve_path` +
  `load_module_inner` + `resolve_loader_import_file_path`
  + the three pre-scan callsites — dual-extension delegation
  + view transform on `.fitzv` sources.
- `src/main.rs::resolve_import_file_path` + two pre-scan
  callsites — same.
- `src/lsp.rs::resolve_import_file_path_lsp` + two pre-scan
  callsites + two cross-module resolvers — same.
- `tests/cli_e2e.rs` — 3 new tests exercising the 2-file
  E2E from the CLI.
- `docs/fase-11-plan.md` (this file) — §9.y + §6 row 11.6.d
  CLOSED + top-of-file status refresh.

**Next norte after 11.6.d**: **11.6.e** — migration of the
4 fitz-liveviews examples (counter → dashboard → chat →
kanban) from raw-string HTML to `.fitzv` SFCs, plus
cross-file `<Child />` composition (which the migration
will surface as a real need on the kanban port).

---

## 9.z 11.6.e — Migration prep (SSR emitter payload access + fitz_liveviews dep hint + counter draft) — PARTIAL

Sub-step of 11.6. **Does NOT close 11.6.e entirely**. Ships
the two Fitz-core enablers that surfaced when I actually
tried to migrate the 4 fitz-liveviews examples to `.fitzv`,
plus the counter migration draft applied to
`d:/fitz-liveviews/examples/counter/` (uncommitted in the
sibling repo, ready to land when Fitz v0.21.0 ships). The
remaining migrations (dashboard, chat, kanban) plus
cross-file `<Child />` composition remain scope for a
follow-up mini-fase.

### Empirical scoping

Before touching code I walked every one of the 4 examples in
`d:/fitz-liveviews/examples/{counter,dashboard,chat,kanban}/`.
The two decisions the migration forced (aligned with the
author 2026-07-16 at start of session):

- **Cross-file `<Child />` composition is NOT required for
  the migration.** All 4 examples that already have child
  components (dashboard's `MetricTile`, kanban's
  `CardEditor`) compose them with fitz-liveviews's runtime
  `component(name, id)` API, not template syntax. That API
  works cross-file naturally (registration + lookup at
  runtime). Cross-file `<Child />` template syntax would
  also need dynamic props (`title={expr}` — currently only
  static in 11.5.d), which is 11.7+ material. Documented
  as remaining scope for the 11.6.e continuation or 11.7.
- **Counter + chat need conversion to `@live_component`
  style.** The pre-11.6.e counter and chat kept mutation
  logic inside the WebSocket handler loop (`state =
  CounterState { count: state.count + 1 }`) instead of in
  `@on` handlers. Converting them to the Phase-4 style
  (matching dashboard + kanban) is documented as
  intentional divergence vs the pre-migration baseline —
  the whole point of the SFC is that state + events + template
  live in the component.

### The 2 SSR emitter blockers that surfaced

Walking the empirical migration shape uncovered two
event-body walker limits that block chat and kanban:

- **`payload` rejected as a free identifier in event body
  RHS.** The emitted `@on` fn signature is
  `fn <Name>_<event>(state: <Name>, payload: Map<Str, Str>) -> <Name>`,
  so `payload["author"]` and `payload.has("text")` are
  natural in the event body — but the pre-11.6.e emitter
  only passed `state_field_names` to
  `format_fitz_expr_scoped` and empty `local_scope`. Any
  bare `payload` reference tripped "identifier `payload` is
  not a declared state field". Fix in this §9.z:
  `format_event_rhs` now passes `&["payload"]` as
  `local_scope`, so `payload[key]` / `payload.has(...)` /
  `payload.get(...)` walk through the existing Index /
  Field / Call arms unchanged.
- **`if` statements + `let` bindings in event bodies
  reject.** Chat's `send_message` and kanban's
  `card_editor_save` both use nested `if
  (payload.has(...)) { ... }` guards + `let seed = if
  (...) { ... } else { ... }` expressions, but the emitter
  only accepts linear `Stmt::Assign` sequences with a
  hard reject on any other stmt kind (chat + kanban `.fitzv`
  drafts would break at emit-time). **Widening the emitter
  to full stmt bodies (`if` / `for` / `let` / expression
  stmts) is not part of this §9.z scope** — it's real
  11.6.e continuation work paralleling the `{#if}` /
  `{#for}` widening from 11.6.c but on the event-body
  side. Documented as debt below.

### What ships in this §9.z

1. **`src/view/codegen_ssr.rs`**: `format_event_rhs` now
   passes `&["payload"]` to `format_fitz_expr_scoped` (was
   `&[]`). Two new unit tests
   (`phase_11_6_e_emit_accepts_payload_index_access_in_event_body_rhs`
   +
   `phase_11_6_e_emit_accepts_payload_method_call_in_event_body_rhs`)
   validate the fixture shape chat/kanban would want.
2. **`src/evaluator.rs` + `src/codegen.rs`**: the classic
   loader's "module not found" error now enriches when the
   missing module is exactly `fitz_liveviews` — a targeted
   `hint:` block with the canonical
   `[dependencies] fitz_liveviews = { git = "…", tag =
   "v0.4.2" }` snippet users can paste into `fitz.toml`.
   Fires in both `fitz run` (evaluator path) and `fitz
   build` (codegen path). Two new unit tests in
   `evaluator::tests::phase_11_6_e_missing_fitz_liveviews_dep_*`
   assert the hint fires only for the targeted module name.
3. **`d:/fitz-liveviews/examples/counter/` (sibling repo)**:
   `Counter.fitzv` + rewritten `src/main.fitz` + updated
   `README.md`. Uncommitted local changes in the fitz-
   liveviews repo — they land as a real commit **only when
   Fitz v0.21.0 (with 11.6.d loader integration) ships**,
   since the system `fitz` binary users have installed is
   still pre-11.6. Smoke validated end-to-end against my
   local `d:/fitz/target/release/fitz.exe` (fitz 0.20.1 HEAD
   post-11.6.d + this §9.z): `fitz check` passes; `fitz
   run` boots the server on `:3000`; `curl /` returns the
   `<div data-flv-component-name="Counter"
   data-flv-value-instance_id="root">` wrapper with the
   `Count: 0` initial state and 3 `data-flv-click` buttons.
   The `flv_register(...)` call in `main.fitz` is manual
   because v0.20.1's implicit auto-inject only scans the
   TOP-LEVEL program for `@live_component` types — imported
   ones (like `Counter` from `Counter.fitzv`) aren't seen
   by `env.live_components`. Cross-module auto-inject is
   documented as debt below.

### Debt / gotchas visible after §9.z

Ordered by priority for the 11.6.e continuation:

- **Cross-module `@live_component` auto-inject.**
  `inject_live_component_registrations` in `src/types.rs`
  consults `env.live_components` populated by
  `resolve_program` walking the top-level program AST only.
  When `Counter` lives in `Counter.fitzv` imported via
  `from Counter import Counter`, the main program's `env`
  has no entry for `Counter`, so the auto-inject skips it
  and users must write the `flv_register(...)` call by
  hand. Parallel to imported-auth-provider (W12) /
  imported-background-fns (B10) — both closed with a
  dedicated pre-scan that threads metadata from imported
  modules into the main env. Same pattern applies here.
  ~200 LoC + tests.
- **Event body widening: `if` + `let` + non-assign stmts.**
  Blocks chat + kanban migrations. Chat's `send_message`
  uses `if (payload.has("author")) { if (payload.has("text"))
  { ... } }`; kanban's `card_editor_save` uses `let new_text
  = if (...) { ... } else { ... }`. The emitter needs to
  accept a linear-ish subset that lowers into a single
  return of the fresh struct literal. Parallel to how
  `{#if}` / `{#for}` in templates lower to Fitz `if
  (...) { ... }` / `.map(...)` expressions — the event
  body needs a matching lowering. Real 11.6.e continuation
  work, ~200-300 LoC + tests.
- **Cross-file `<Child />` composition.** The debt from
  §9.y's list. Not actually needed for any of the 4
  fitz-liveviews examples (they all use runtime
  `component(name, id)`), so lower priority than the two
  above. Still worth landing eventually because template-
  syntax composition is nicer DX than runtime lookup.
  Requires threading the loader's expanded-file cache
  through the checker + emitter.
- **Migration commits deferred until v0.21.0.**
  `d:/fitz-liveviews/examples/counter/` has 3 uncommitted
  changes (2 modified + 1 new). They land in fitz-liveviews
  as a real commit only when v0.21.0 releases (which needs
  a bump of `Cargo.toml` in fitz core plus the CHANGELOG
  entry pointing at 11.6.d/e/etc.). Dashboard's migration
  should be similar-shaped (extract `MetricTile.fitzv` +
  update main.fitz + README); chat + kanban wait for the
  event-body widening.
- **`fitz-liveviews`'s minimum fitz version.** With 11.6.d
  landed in fitz core, examples using `.fitzv` files
  require fitz binary v0.21.0+. Once the migrations
  actually commit, the fitz-liveviews README should
  document the minimum version requirement (paralleling
  how e.g. `fitz-liveviews` v0.4.x was pinned to specific
  fitz features in memoria
  `project_liveviews_phase4_plan`).

### Files touched by 11.6.e §9.z

- `src/view/codegen_ssr.rs` — `format_event_rhs` passes
  `&["payload"]` to `format_fitz_expr_scoped`; 2 new
  `phase_11_6_e_emit_accepts_payload_*` unit tests +
  updated doc comment on `format_event_rhs`.
- `src/evaluator.rs` — enriched "module not found" branch
  for `fitz_liveviews`; 2 new
  `phase_11_6_e_missing_fitz_liveviews_dep_*` unit tests.
- `src/codegen.rs` — parallel enrichment in
  `ModuleLoader::load_module`.
- `docs/fase-11-plan.md` (this file) — this §9.z section +
  top-of-file status refresh.
- `d:/fitz-liveviews/examples/counter/src/Counter.fitzv`
  (new, sibling repo, uncommitted).
- `d:/fitz-liveviews/examples/counter/src/main.fitz`
  (modified, sibling repo, uncommitted).
- `d:/fitz-liveviews/examples/counter/README.md`
  (modified, sibling repo, uncommitted).

**Next norte after §9.z**: **11.6.e continuation** — either
event-body widening (opens chat + kanban migrations) or
cross-module `@live_component` auto-inject (removes the
manual `flv_register(...)` boilerplate from the counter
migration + every future `.fitzv` project). Both are pure
Fitz-core work; migration commits in `fitz-liveviews` land
when v0.21.0 ships.

---

## §9.aa 11.6.e continuation — Event body widening (`if` guards + `let` bindings + `Expr::If`/`Expr::StructLit` on RHS)

Second §9 pass on 11.6.e. Closes the largest debt from
§9.z's list — the SSR emitter can now lower kanban's
`card_editor_save` (`let new_text = if(payload.has("text"))
{ payload["text"] } else { text }` + state-field mutations)
and chat's `send_message` (nested `if(payload.has("author"))
{ if(payload.has("text")) { ... } }`) event bodies without
tripping "non-assignment statement" / "if-as-expression
deferred" errors.

### The gap §9.z left open

Both the chat and kanban `.fitzv` migrations need event
bodies with shapes richer than the linear `state_field =
<expr>` sequence the emitter accepted through §9.z:

- **Kanban's `card_editor_save`** builds an intermediate
  `let new_text = if (payload.has("text")) { payload["text"]
  } else { text }` and then mutates two state fields
  (`is_editing = false`, `text = new_text`). Rejected in
  §9.z on two grounds: the emitter refused any stmt other
  than `Stmt::Assign` (so a `let x = ...` on a non-state
  ident fell through the wildcard reject), and the RHS
  walker refused `Expr::If`.
- **Chat's `send_message`** uses two nested `if
  (payload.has(...)) { ... }` guards to skip processing
  frames missing required fields. Also rejected — `Stmt::If`
  doesn't exist in the classic Fitz AST (it's a
  `Stmt::Expr(Expr::If, _)`), and even if it did, the
  emitter didn't recurse into nested bodies.

Both patterns are idiomatic. Neither requires new syntax on
the `.fitzv` side — they'd already been written by
fitz-liveviews authors on the pre-`.fitzv` `main.fitz`
handlers.

### Design pinned

**Two emit shapes, dispatched by body shape.**

- **Trivial body** — every stmt is `Stmt::Assign { target:
  Ident(name), type_: None }` where `name` is a declared
  state field. The pre-`.fitzv`-migration common case:
  `event bump() { count = count + 1 }`. Emit shape stays
  compact (`return X { <field>: <rhs>, ... }` where each
  RHS is either the assigned value or `state.<field>` if
  untouched). All ~10 pre-existing tests continue to pass
  bit-for-bit.
- **Widened body** — anything else triggers the shadow-
  local shape:
  1. **Prime**: `let <field> = state.<field>` for each
     state field at the top of the fn. Each state field is
     now a reassignable Fitz local (not `state.<field>`
     which is read-only).
  2. **Walk body verbatim**: `Stmt::Assign { target:
     Ident(name), value }` where `name` is already in the
     tracked scope emits as `<name> = <lowered rhs>` (a
     reassignment); otherwise as `let <name> = <lowered
     rhs>` (a new local) and the name pushes onto the
     scope. `Stmt::Expr(Expr::If { condition, then,
     else_ }, _)` emits as `if (<cond>) { <recurse then>
     } else { <recurse else> }` (or without the else
     clause); each arm gets a child scope that truncates
     back on close so an arm-local `let` doesn't leak.
     Everything else (`Stmt::Expr(non-If, _)`, Field/
     Index-target assign, `Return`, `Break`, `Continue`,
     `For`, `While`, `Loop`, `TypeDef`, `FnDef`, `Import`,
     `FromImport`, `Destructure`, `ReturnStatus`,
     `Error`) rejects with a 11.7+ pointer.
  3. **Return**: `<Name> { <field>: <field>, ... }` — the
     mutable shadows now carry the current values.

The trivial path is preserved for readability of the
emitted output on the common case — the wide path's
prime + return dance is uniform but noisier. Since both
paths call the same walker for RHS lowering, walker
widening (see below) benefits both.

**Walker widening**. `format_fitz_expr_scoped` grows two
new arms:

- **`Expr::If`** — `if (<cond>) { <arm> } else { <arm> }`
  where each arm body is a single `Stmt::Expr(<value>,
  _)`. Extracted into `format_if_arm_value`; multi-stmt
  arms defer to Phase 11.7+ with a clear pointer.
  Without-else falls back to `null` so the arm always
  has a value. Kanban's canonical `let seed = if (...)
  { ... } else { ... }` walks through here.
- **`Expr::StructLit`** — `TypeName { field: <expr>, ...
  }`. Chat's inline `Message { author: payload["a"],
  text: payload["t"] }` construction walks through
  here. The emitter does not validate the type name —
  classic Fitz's type checker handles that on the round-
  trip.

Both new arms live in the same walker so template
interpolations get them for free too (`{if(cond){a}
else{b}}` in a `{state.field}` interpolation would work,
if a use case appears).

**Scope model** — `local_scope` extends what §9.z did
for `payload`. In the wide path, the top-of-fn primes
push every state field name plus `payload`. Every `let x
= ...` (assign to an ident NOT in `local_scope`) pushes
`x`. Every `if` arm enters a child scope: `saved =
local_scope.len()`, recurse, `local_scope.truncate(saved)`.
Arm-local bindings can't be referenced after the arm
closes — matches Fitz's own semantics.

**Ident classification** in the walker is unchanged:

1. **In `local_scope`** — emit verbatim. Now covers state
   fields (in the wide path they're mutable shadows,
   emitted bare), `payload` (fn param), closure params
   (as of 11.6.c), and event-body-introduced locals
   (this pass).
2. **In `state_field_names` but NOT in `local_scope`** —
   rewrite as `state.<field>`. This is the render-fn path
   (which passes empty `local_scope`) and any other
   walker consumer that doesn't shadow.
3. **Neither** — reject with the "not a declared state
   field" pointer that's been stable since 11.6.b.

### What ships in this §9.aa

1. **`src/view/codegen_ssr.rs`** — `emit_event_fn`
   refactored into a dispatcher on `is_trivial_event_body`;
   trivial path kept verbatim from §9.z (renamed
   `emit_event_fn_trivial`); new
   `emit_event_fn_widened` handles the shadow-local
   shape. New helpers: `is_trivial_event_body`,
   `lower_event_body_stmts` (recursive body walker with
   scope-truncation on `if` arms), `format_if_arm_value`
   (single-expr-stmt arm reader). Walker
   `format_fitz_expr_scoped` gains `Expr::If` and
   `Expr::StructLit` arms; both are removed from the
   rejection block. Module-level doc comment refreshed
   with the "Phase 11.6.e widens further" section.
2. **13 new SSR unit tests** under `phase_11_6_e_*`:
   - `widened_body_primes_shadow_locals_from_state` —
     shadow prime + let + mutation + return-via-shadow
     all present.
   - `widened_body_lowers_if_guard_at_stmt_level` —
     `if (payload.has("t")) { title = payload["t"] }`
     canonical kanban shape.
   - `widened_body_lowers_nested_if_guards` — chat
     `send_message` shape (simplified without
     StructLit).
   - `widened_body_lowers_if_as_expression_in_let_rhs` —
     kanban's `let new_text = if (...) { ... } else {
     ... }` full shape.
   - `walker_accepts_struct_literal_in_wide_body_let` —
     chat `let m = Message { ... }` inline.
   - `walker_accepts_struct_literal_in_trivial_body_rhs` —
     StructLit RHS in a trivial state-field mutation
     stays on the compact shape.
   - `trivial_body_still_uses_compact_shape` —
     regression against accidental wide-path routing.
   - `if_arm_scope_does_not_leak_after_arm_closes` —
     arm-local `let x = 5` scoped properly + post-arm
     mutation via shadow works.
   - `widened_body_rejects_bare_expression_stmt` —
     `payload.has("k")` as a side-effect stmt trips
     the 11.7+ pointer.
   - `widened_body_rejects_index_target_assign` —
     `xs[0] = n` trips the 11.7+ pointer.
   - `widened_body_round_trips_through_classic_fitz` —
     kanban's full `card_editor_save` + `start` +
     `cancel` shape lexes + parses through classic Fitz
     after the SSR emitter runs.
   - Plus the 2 payload-access tests already added by
     §9.z (`widened_body_lowers_if_guard_at_stmt_level`
     replays their scope guarantees under the wide
     path).
3. **`docs/fase-11-plan.md`** — this §9.aa section +
   top-of-file / row-11.6 status refresh + memoria
   `project_phase_11_frontend_view` bumped.

Total: 53 view SSR unit tests, all green. Full lib test
suite green. Trivial-path regression zero.

### Debt / gotchas visible after §9.aa

Ordered by priority for the 11.6.e continuation:

- **Cross-module `@live_component` auto-inject** — same
  status as after §9.z. Chat + kanban migrations still
  need this (their `.fitzv` files export components that
  main.fitz imports). Independent from event-body
  widening; probable next task.
- **Cross-file `<Child />` composition** — same status.
  Not needed by any of the 4 fitz-liveviews examples
  (they all use runtime `component(name, id)`). Low
  priority.
- **Multi-stmt if arms in RHS** — kanban's canonical
  bodies all fit into single-expr arm bodies, so
  deferred. If a real use case appears, extend
  `format_if_arm_value` to walk multi-stmt arms
  (probably by re-using `lower_event_body_stmts` with
  a sink-tail that emits the final value).
- **`Expr::Match` in walker** — deferred. Match with
  `Result<T>` scrutinees would let event bodies do
  `match payload.get("k") { Ok(v) => ..., Err(_) =>
  ... }` — nicer than `if(payload.has("k")){...}`
  because it also gives access to the extracted value.
  Not needed by the 4 examples so far.
- **Nested field/index-target mutations** — `obj.field =
  value` and `xs[i] = value` require the shadow-local
  model to break down (`obj` or `xs` is itself a
  shadow, and a partial mutation to its interior needs
  either a deep clone or a proper mutable reference).
  Kanban's kanban_socket handler does `board.cards =
  board.cards.map(fn(c) => ...)` — a full-list rebind,
  not a nested mutation — so §9.aa's rejection is fine
  for the migrations at hand. Real nested mutation
  support lands as a 11.7+ item.
- **`for` loops in event bodies** — for-loops over state
  lists (`for c in cards { ... }`) rejected. No canonical
  use case in the 4 examples; if one appears, the
  lowering is `xs.iter().for_each(...)`-shaped.

### Files touched by 11.6.e §9.aa

- `src/view/codegen_ssr.rs` — `emit_event_fn` refactored
  into trivial/wide dispatch; new `emit_event_fn_trivial`
  + `emit_event_fn_widened` + `lower_event_body_stmts`
  + `is_trivial_event_body` + `format_if_arm_value`;
  walker widened for `Expr::If` + `Expr::StructLit`;
  module doc comment + `format_fitz_expr` doc comment
  refreshed. +13 unit tests.
- `docs/fase-11-plan.md` (this file) — this §9.aa
  section + status refresh at top.

**Next norte after §9.aa**: **cross-module
`@live_component` auto-inject** — the pattern §9.z
identified as removing the last piece of manual
`flv_register(...)` boilerplate from every `.fitzv`
project that lives in a sibling module. Chat + kanban
migrations both need it; counter's migration draft
already does (it manually writes the register call).
Paralleling W12 (`pre_scan_imported_auth_provider`) +
B10 (`pre_scan_imported_background_fns`) is a clean
shape.

---

## §9.bb 11.6.e continuation — Cross-module `@live_component`
## auto-inject

**Sub-step of Phase 11.6.e (2026-07-16)**. Extends v0.20.1's
implicit `flv_register(...)` injection (which only scanned the
top-level program AST) to components declared in **imported**
`.fitzv`/`.fitz` sibling modules. Parallels W12
(`pre_scan_imported_auth_provider`) and B10
(`pre_scan_imported_background_fns`).

### What changes

Before §9.bb, a project like the counter migration in
`d:/fitz-liveviews/examples/counter/` required a manual boot
registration in `main.fitz`:

```fitz
from fitz_liveviews import Html, html, flv_register
from Counter import Counter, Counter_render, Counter_increment,
                    Counter_decrement, Counter_reset

// Manual — v0.20.1's implicit inject only scanned top-level
// program, so imported components had to be registered by hand.
flv_register("Counter", Counter { }, Counter_render, {
  "increment": Counter_increment,
  "decrement": Counter_decrement,
  "reset": Counter_reset,
})

@get("/") fn counter_page() -> Response { ... }
```

Post-§9.bb, the manual call goes away — the compiler synthesises
it during check-time by extracting `@live_component` +
`@render_for` + `@on` metadata from every imported module:

```fitz
from fitz_liveviews import Html, html, flv_register
from Counter import Counter, Counter_render, Counter_increment,
                    Counter_decrement, Counter_reset

// Auto-injected. No manual flv_register(...) needed.

@get("/") fn counter_page() -> Response { ... }
```

The user still writes the `from Counter import ...` line — the
injected call uses bare `Ident` refs for the type + render fn +
event handlers, matching the local case shape.

### Files touched by §9.bb

- `src/types.rs`:
  - New `pub struct ImportedLiveComponent { component_name,
    type_name, module_name, render_fn, events }` — populated per
    imported component with the fully-resolved render + event fn
    names.
  - New field `TypeEnv.imported_live_components:
    Vec<ImportedLiveComponent>` + `add_imported_live_components`
    setter + `imported_live_components()` getter.
  - New public `pub fn extract_live_components_from_program(
    program: &Program, module_name: &str) ->
    Vec<ImportedLiveComponent>` — walks a parsed module's AST
    collecting `@live_component` types plus their sibling
    `@render_for` + `@on` fns. Silently drops components without a
    matching `@render_for` (the imported module's own checker will
    surface the error when it's loaded through the classic
    pipeline). Deterministic order: sorted by component name,
    events sorted by event name.
  - Extended `inject_live_component_registrations`: after
    processing local components (as before), iterates over
    `env.imported_live_components()` sorted by component name.
    For each imported entry:
    - Skips if the user wrote a manual `flv_register("<name>",
      ...)` call (same as local).
    - Skips if a local `@live_component` with the same name
      exists (**local wins over imported** — silent skip).
    - Validates that `type_name`, `render_fn`, and every event
      handler fn name are in scope via `from` imports OR local
      FnDef/TypeDef/Assign stmts. Missing names produce a clean
      error listing every missing name plus an actionable `Add
      \`from <module> import <TypeName>, <TypeName>_render,
      <TypeName>_<event>...\`` hint.
    - Emits the same `Expr::Call { flv_register, [Str(comp),
      StructLit { type_name, fields: [] }, Ident(render_fn),
      Map({event: fn_name})] }` shape as the local case.
  - New private helper `collect_names_in_scope(program) ->
    HashSet<String>` used by the imported branch — collects
    identifiers brought into top-level scope by
    `Stmt::FromImport { names, .. }` (respecting aliases),
    `Stmt::FnDef`, `Stmt::TypeDef`, and `Stmt::Assign` with
    `AssignTarget::Ident`.
  - +9 unit tests `types::tests::extract_live_components_*_9bb`
    (4 extractor tests) and `types::tests::inject_*_9bb` (5
    injector tests) covering: extractor basic + no-components
    no-op + skip-without-render-for + multi-component alphabetical
    order + inject basic + missing-imports-hint + manual-call
    precedence + local-wins-over-imported + missing-flv_register-
    import.
- `src/main.rs`:
  - New helper `pre_scan_imported_live_components(program,
    base_dir, dep_registry) -> Vec<types::ImportedLiveComponent>`
    paralleling `pre_scan_imported_background_fns`. Walks each
    `Stmt::Import` / `Stmt::FromImport`, resolves the file
    (`.fitz` first, `.fitzv` fallback via
    `fitz::view::resolve_module_file_candidates`, transformed
    through `fitz::view::transform_fitzv_source`), tokenizes +
    parses, and feeds the result to
    `types::extract_live_components_from_program`. Silent-fallback
    on read/lex/parse/view-transform errors (paralelo bit-a-bit al
    W12/B10 pattern). Module binding name derived from the last
    segment of the import path (`from Counter import ...` →
    `"Counter"`).
  - Wired into `check_program_with_pyi_stubs_and_deps` right
    after the B10 pre-scan and before `check_with_env`. All 3
    sites that call `inject_live_component_registrations`
    (`run_file`, `build_file`, `build_file_with_bundle`) benefit
    automatically because they all read from the enriched
    `TypeEnv` produced by the checker.
- `tests/cli_e2e.rs`:
  - Two new E2E tests (`phase_11_6_e_bb_cross_module_*`) using a
    stub `fitz_liveviews.fitz` sibling providing `Html`,
    `html(s) -> Html`, and `flv_register(name, initial_state,
    render_fn, events) -> Null` no-ops so the classic loader can
    resolve `from fitz_liveviews import ...` without the real
    library. Canonical `Counter.fitzv` sibling declares one
    `@live_component` + `@render_for` + `@on`.
    - `phase_11_6_e_bb_cross_module_live_component_auto_injects_flv_register`
      — happy path. main.fitz omits the manual `flv_register(...)`
      call and reaches `print("boot OK")` via `fitz run`.
    - `phase_11_6_e_bb_cross_module_missing_imports_errors_with_hint`
      — negative case. main.fitz forgets `Counter_render` +
      `Counter_increment` in its `from` import; `fitz run`
      fails citing every missing name plus the actionable fix.

### Design decisions

- **Local wins over imported** (silent skip): if the same
  component name is registered both locally and via import, the
  imported entry is skipped. Matches how W12 handles cross-module
  `@auth_provider` overlap.
- **Bare `Ident` refs in the injected call**: the injected shape
  is identical to the local case (`flv_register("Counter",
  Counter { }, Counter_render, {...})`) — the user brings the
  names into scope via `from <module> import <TypeName>,
  <TypeName>_render, <TypeName>_<event>...`. Reasons:
  - Struct literals require a bare `TypeName` in scope (parser
    doesn't accept `mod.TypeName { }`); forcing `from` for the
    type carries the render + event fn names in the same import
    line naturally.
  - Emitting `<module>.<name>` field access for the fn refs would
    require dual paths + branching per imported entry.
  - Missing-names errors are actionable: we surface the exact
    `from <module> import ...` line the user needs.
- **Silent drop of components without `@render_for`** at
  extraction time — the imported module's own checker reports
  the missing renderer when loaded through the classic pipeline;
  we don't want to double-report from the importer.
- **Field-default validation skipped for imported types** — the
  extractor never inspects the imported type's fields. If a
  field lacks a default, the resulting `<TypeName> { }` fails at
  eval-time with the standard "missing field" error, same UX as
  writing it manually.
- **`fitz check` does NOT run the injector** — same as v0.20.1's
  local case (`fitz check` is a diagnostic-only pass). Missing-
  imports errors from cross-module auto-inject surface via `fitz
  run` and `fitz build` only. Refinable if presión real appears
  from LSP or CI-only workflows.

### Debt / gotchas visible after §9.bb

Ordered by priority for the 11.6.e continuation:

- **Cross-file `<Child />` composition** — still open from §9.y.
  Low priority because none of the 4 fitz-liveviews examples need
  it (they all use runtime `component(name, id)`). Requires
  threading the loader's expanded-file cache through the checker
  + emitter.
- **`fitz check` inject-time errors** — checker doesn't run
  inject, so missing-imports errors from cross-module auto-inject
  don't fire during `fitz check`. UX-visible only via `fitz run`
  or `fitz build`. Refinable if LSP or CI-only flows demand it.
- **Migration commits in fitz-liveviews** — counter draft
  uncommitted from §9.z; dashboard should follow the same shape
  (extract `MetricTile.fitzv`); chat + kanban now unblocked by
  §9.aa (event-body widening) + §9.bb (cross-module auto-inject).
  Commits land when Fitz v0.21.0 ships.
- **Transitive imports** — like W12/B10, only direct imports are
  scanned. A component in a module imported by another imported
  module is invisible to the auto-inject.

### Files touched by 11.6.e §9.bb

- `src/types.rs` — `ImportedLiveComponent` struct + `TypeEnv`
  field + accessors + `extract_live_components_from_program`
  extractor + `inject_live_component_registrations` extension +
  `collect_names_in_scope` helper. +9 unit tests.
- `src/main.rs` — `pre_scan_imported_live_components` helper +
  wired into `check_program_with_pyi_stubs_and_deps`.
- `tests/cli_e2e.rs` — 2 new E2E tests with stub
  `fitz_liveviews.fitz` + canonical `Counter.fitzv`.
- `docs/fase-11-plan.md` (this file) — this §9.bb section +
  status refresh at top.

**Next norte after §9.bb**: **§9.y debt (cross-file `<Child />`
composition)** if a demand appears (none of the 4 fitz-liveviews
examples need it), OR **land the counter/dashboard/chat/kanban
migrations** in the sibling repo when Fitz v0.21.0 ships. With
§9.bb closed, chat + kanban `.fitzv` versions can drop their
manual `flv_register(...)` boot boilerplate.

---

## 10. Naming decision recap

**Extension**: `.fitzv`.

**Module path**: `crate::view`.

**Keywords added to the view dialect**: `component`, `state`,
`event`. Not added to the classic dialect (`crate::lexer` never
sees them).

**AST prefixes**: view AST types stay in `crate::view::ast` — no
name collision with `crate::ast`.

**Public API surface (POC)**: only `crate::view::parse(source)
-> ViewParseResult<ViewFile>`. Everything else is `pub` inside
`view::` for tests but not re-exported at the crate root.

If the extension name needs to change later (e.g. community
feedback points to `.fzv` or `.fitzc`), the change is trivial:
this doc, the CLI dispatch in 11.5, and any editor grammar files
that hard-code `.fitzv`. The parser code does not care about the
extension.

---

## 11. Next iteration — fine-grained reactivity + fullstack (planned)

The client-WASM composition surface (props, event bubbling,
slots, cross-file `<Child />`) is complete as of v0.26.0 under
the **dirty-flag + naive re-render** model. The next Phase 11
iteration lands the four jumps that turn a `.fitzv` into a
first-class fullstack app. Numbered **11.10–11.13** in
`docs/roadmap.md` (see the "próxima iteración" block there for
the user-facing summary). Ordered by cost/benefit, not strict
dependency:

- **11.10 — Fine-grained reactivity.** Replace the top-down
  naive re-render with a reactive graph of **local
  subscriptions**: mutating a `state` field only recomputes the
  template nodes that *read* it. No new user-facing API in the
  base case — the dev still writes `{count}` bare and the
  compiler wires the subscription when lowering the `.fitzv` to
  WASM. Adds **derived values** (a `memo` that recomputes only
  when a dependency changes) and **async derived values**. Closes
  the 11.7.a/R1 limit ("the child is recreated on each parent
  re-render → loses local state"): with subscriptions the child
  survives because it subscribes to what it reads. Touches
  `src/view/codegen_wasm.rs` (emitted runtime: reactive cells +
  subscription graph instead of the top-down re-render).

- **11.11 — Server functions (`@server`).** New decorator on an
  `async fn` making it **callable directly from the client-WASM
  `.fitzv`** as if local. The compiler emits both halves of a
  typed contract in one place: (a) the server-side HTTP handler
  (reusing the existing native HTTP runtime + auth + `Result` +
  JSON marshaling) and (b) the client stub that serializes args →
  `http.request` (built-in since v0.17.0) → deserializes the
  return into the declared type. **Low cost, high value** — the
  pieces already exist; only the compiler macro that joins them
  is missing. This is the feature that gives the view its "whole
  stack together, no plumbing" moment.

- **11.12 — SSR → client hydration.** A single `.fitzv` renders
  **SSR** (first paint, SEO, works without JS) and the
  client-WASM runtime **takes over the existing DOM** instead of
  re-creating it: restore the state the server serialized
  (initial state + `@server` results) and map template nodes to
  the real DOM nodes. Unifies the two emitters (SSR + WASM),
  today separate worlds sharing one check pass. The `data-flv-*`
  IDs the SSR emitter already emits are the basis of the mapping.
  The big vision that joins the two frontend halves.

- **11.13 — Template hot reload.** `fitz dev` re-parses a
  `.fitzv`'s `<template>` and applies the diff without
  recompiling the whole WASM crate (today `fitz dev` recompiles
  everything). A DX jump; the most ambitious technically (needs a
  client-side template runtime that accepts diffs).

**Suggested order:** 11.11 (server fns — pieces ready) → 11.10
(signals — unblocks child state) → 11.12 (hydration — unifies
backends) → 11.13 (hot reload — DX). Each is a serious session.

---

## §11.13 — Template hot reload: research, decision, slice-1 (2026-08-04)

Research mapped two seam sets (Explore agents over `src/main.rs`
and `src/view/`), then chose **Approach C** and shipped slice-1.

### Seams

- **`fitz dev` today** is kill+respawn of `fitz run` (interpreter,
  SSR path). The watcher (`path_is_relevant`, `main.rs`) matched
  `ext == "fitz"` **exact** → it ignored `.fitzv` entirely and
  never invoked `wasm-pack`. So the client-WASM `.fitzv` path had
  **no dev loop at all** (the roadmap's "recompiles everything" was
  inaccurate). No browser channel existed.
- **View pipeline** already holds the template as a rich data tree
  (`ExpandedTemplateNode`; all types derive `PartialEq`, so
  "only the template changed" is a trivial `==`). The blocker is the
  **back-end**: `codegen_wasm` unrolls that tree into imperative
  hard-coded Rust per node, with no serialized template artifact at
  runtime. Interpolations lower arbitrary Fitz expressions over
  state via `lower_expr`. The slow rebuild is
  `wasm-pack build --release` (cargo + `wasm-opt`) inside the stable
  scaffold `target/wasm-build/<bin>/`.

### The wall (why the literal spec is huge)

A data-driven runtime that swaps a new template tree must **evaluate
the interpolation expressions in WASM** — i.e. ship a Fitz
expression interpreter in the browser (a 2nd back-end). That's what
makes the literal spec ("diff without recompiling") the single
largest thing in Fase 11.

### Approaches

- **A** — full data-driven runtime + expression VM in WASM. Meets
  the letter; months-scale; permanent dual-codegen; bundle bloat.
- **B** — structural-only data-driven, expressions stay compiled by
  stable ID; new/changed expression degrades to recompile. Covers
  80% visual; still a rewrite with dual-mode emit; partial spec.
- **C (chosen)** — no template runtime. Watch `.fitzv`; rebuild
  incrementally (`wasm-pack --dev`, no `wasm-opt`, stable scaffold →
  warm cargo cache); serve the project root + a live-reload WS;
  push a browser reload on rebuild. State preservation (via the
  v0.31.0 hydration payload) is slice-2. Recompiles incrementally
  (~seconds), so it does **not** meet the literal "no recompile" —
  reframed honestly. Zero codegen rewrite, zero byte-compat risk.
- **D** — hot-patch the WASM (Dioxus `subsecond`: recompile changed
  fns + jump-patch the live module). Frontier; out of scale for a
  solo author. Descartado.

**Decision:** C for the MVP; A/B (the true data-driven runtime)
left as a separate large future norte gated by the expression-VM
sub-problem (deuda 🟡 in `docs/deudas-post-5b.md`).

### Slice-1 (shipped)

- **Seam A** — `path_is_relevant` accepts `.fitzv`.
- Extracted `build_wasm_client(resolved, release) -> Result<
  WasmBuildOutput, String>` from the exit-on-error
  `build_wasm_client_cmd` (thin wrapper kept for the CLI). `release`
  toggles `--release` vs `--dev`.
- New bin-local `src/dev_server.rs`: axum static server (root =
  manifest dir, `Cache-Control: no-store`, `application/wasm` MIME,
  missing `favicon.ico` → 204) + `/__fitz_dev_ws` broadcast WS +
  reload-snippet injection into the served `index.html` (or a
  generated fallback with the `[[bin]].mount` element).
- `fitz dev` detects a wasm-client default bin (`is_wasm_client_dev`)
  and runs `run_wasm_dev_loop`: initial build → serve → watch →
  rebuild + `signal_reload()` on each save. New `--port` flag
  (default `1234`).
- **Validated in real Chrome (puppeteer):** initial render + click
  interactivity, then editing the `<template>` auto-reloads the
  browser to the new content; state resets on reload (slice-1, no
  preservation); zero page errors. `cargo test --lib` green, view
  smokes byte-identical (`git status examples/view/` clean), fmt +
  clippy (default + lsp) clean.

### Slice-2 (shipped) — state preservation across reload

Dev-flag gated (`fitz dev` sets it via `write_wasm_crate_scaffold(...,
dev_mode = !release)`; `fitz build` passes `false` → prod `lib.rs` +
`Cargo.toml` byte-identical). Pieces:

- **`emit_dev_state_methods`** (`codegen_wasm.rs`) — emits `impl <Root> {
  __fitz_dev_snapshot() -> String; __fitz_dev_apply(&str) }` for the
  **primitive** state fields (mirrors `json_state_accessor`; apply ends
  in `self.render()`). Composite state (`List`/`Map`/nominal) is skipped
  → resets on reload (follow-up: a `json_dump_value` mirror of
  `json_restore_value`).
- **Dev entry wrapper** (`compose_entry_wrapper(..., dev_mode)`) — after
  `root.mount(...)`, restores `sessionStorage["__fitz_dev_state"]` (then
  clears it) and installs a `beforeunload` `Closure` that snapshots the
  live state back. Reuses the exact `Closure`/`add_event_listener` idiom
  the `@click` emit already uses (no `js_sys` direct dep).
- **`dev_augment_cargo_toml`** — post-processes the dev `Cargo.toml` to
  add `serde_json` + the `Storage` web-sys feature (idempotent), leaving
  `compose_cargo_toml_with_features` and its ~10 smoke call sites
  untouched.

`compose_lib_rs_with_components` was refactored into
`compose_lib_rs_inner(..., dev_mode)` (public fn delegates with
`false`); `write_wasm_crate_scaffold` gained a `dev_mode` param.
**Validated in Chrome:** editing a static template text reloads the DOM
while a bumped `count` survives the reload; view smokes byte-identical
(`git status examples/view/` clean); +2 unit tests
(`phase_11_13_emit_dev_state_methods_*`,
`phase_11_13_dev_augment_cargo_toml_*`).

### Slice-3 (shipped) — composite state in the snapshot

New `json_dump_value` (`codegen_wasm.rs`) — the recursive inverse of
`json_restore_value` — serializes `List<T>` / `Map<Str, V>` /
`Nullable<T>` / imported nominals into a `serde_json::Value`. Each
recursion level threads a fresh `&T` binding (`__le`/`__mv`/`__iv`/
`__fv`) so scalar leaves serialize via `serde_json::to_value(<ref>)`
with no `*&` deref (clippy-clean). `__fitz_dev_apply` gains the
composite branch (reuses `json_restore_value`, mirroring
`emit_apply_state_json`). The top-level snapshot ref is parenthesized
(`(&*self.<field>.borrow())`) so a trailing `.iter()`/`match` binds to
the `&T`, not the `Ref`. Types that can't round-trip through JSON (a
`Map` with a non-`Str` key, tuples, functions) are omitted from the
snapshot and reset to their default — symmetric with the restore side.
**Closes 11.13 Approach C.** Validated in Chrome: a `List<Str>` with 2
items survives the reload; the composite-state wasm crate compiles
clean. +1 unit test (`phase_11_13_slice3_composite_state_dump_and_apply`).

Remaining follow-ups: multi-bin wasm dev (`--bin`), live `fitz.toml`
re-resolution. See the deuda entry.
