# Phase 11 — Native frontend in Fitz core

**Status**: design + parser POC — 2026-07-14. No code paths in the
`fitz` binary route to this yet. See `docs/stack.md` for the
architectural constitution this plan implements.

This document captures the decisions and the parser POC that
kick off Phase 11. Its purpose is to make the next sub-phases
concrete enough that they can be picked up in a fresh session
without re-litigating shape.

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
| **11.3** | CSS scoping. Parse `<style scoped>` into a small AST, apply per-component class prefix, emit scoped CSS in the SSR output. Decide unscoped style story (`<style global>` or a separate directive). Split into three mini-commits: **11.3.a CLOSED 2026-07-14** — `<style global>` as a first-class sibling of `<style scoped>` in the lexer + parser + AST, plus new `StyleKind { Scoped, Global }` discriminant; bare `<style>` is rejected at lex time with a targeted error naming both accepted forms (see §9.i). Mini-commits 11.3.b (CSS mini-parser + `apply_scope(...)` helper) and 11.3.c (wire scoping into expand + template class-attr rewrite) still open. | A component with `<style scoped>` styling produces HTML + CSS where the styles apply only to that component's markup, verified against `.fitzv` fixtures. |
| **11.4** | Client target decision (WASM vs JS-vanilla). Prototype whichever wins on a two-page counter demo. Confirm bundle size is acceptable. | `fitz build --target <chosen>` produces a working browser demo of the counter component with state persisting across events. |
| **11.5** | CLI integration — `fitz build` routes `.fitzv` files based on `[[bin]] target` / `--target` flag. Multi-component composition (parent embeds child via `<Child prop="v" />`). | Kanban example (currently in `fitz-liveviews/examples/kanban/`) rewritten to `.fitzv`, compiles + runs bit-for-bit equivalent via SSR. Compile times acceptable (<5s for the kanban rewrite). |
| **11.6** | Migration of `fitz-liveviews`. The library refactors its examples (`counter`, `chat`, `kanban`, `dashboard`) and its API to consume `.fitzv` SFCs. Public API (`component()`, `dispatch_component_events()`, decorators `@live_component` / `@render_for` / `@on`) stays identical from the user's POV, just their bodies are now generated by the `.fitzv` compiler. | The existing 4 fitz-liveviews examples run against a `fitz-liveviews vX.Y.Z` that requires `Fitz core vZ+` (whatever version ships Phase 11.5). |
| **11.7** | LSP support inside `.fitzv` — hover over `{expr}` shows the type, autocomplete inside `state { }` and `event ...` bodies, template-attr completion knows about the declared event handlers of the enclosing component. | VSCode extension bumped with the new grammar + LSP config; typing `{sta` inside a template completes to `state field name` when the component has that field. |
| **11.8** | Pedagogic docs — chapter in `docs/guide.md` covering `.fitzv` from scratch. Chapter in `docs/curso/` (new module M9?) mirroring the pedagogic style of M1-M8. Update `docs/architecture.md` with the two-parser split. | Chapter runs someone from zero to a working counter component. Course module has runnable examples. Neither confuses the reader about what's classic Fitz vs what's `.fitzv`. |

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
