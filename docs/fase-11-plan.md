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
| **11.2.b** | *Sub-step of 11.2.* Type-check every parsed AST from 11.2.a. Split into three mini-commits: **(1)** state field defaults compatible with declared type — **CLOSED 2026-07-14** (`src/view/check.rs` ~325 LoC + 16 unit tests). **(2)** event handler bodies checked in an env seeded with state fields as let-bindings + params. Template `{expr}` interpolations checked in the state env; must resolve to a `Str`-friendly type. **(3)** `@event="handler"` attrs cross-check that `handler` names a declared event handler in the same component. | `.fitzv` files with mismatched types (e.g. `count: Int = "hi"`) surface a type error at the correct field/blob. |
| **11.2.c** | *Sub-step of 11.2.* Extend the template AST with `{#if cond}`, `{#for x in xs}`, and `<slot name="X" />`. Update the HTML sub-parser + expand + checker to handle them. | Nested control flow inside `<template>` parses, expands, type-checks. |
| **11.3** | CSS scoping. Parse `<style scoped>` into a small AST, apply per-component class prefix, emit scoped CSS in the SSR output. Decide unscoped style story (`<style global>` or a separate directive). | A component with `<style scoped>` styling produces HTML + CSS where the styles apply only to that component's markup, verified against `.fitzv` fixtures. |
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
- **State field type annotations are ASCII-limited today**. The
  view lexer does not tokenize `<`, `>`, or `?` outside the
  `<template>` / `<style scoped>` block detector — `<` outside
  those two openers still hits the "unexpected `<`" error path.
  Consequence: `List<Str>`, `Map<K, V>`, `Result<T, Str>`, and
  `Str?` cannot come through source in a `.fitzv` file yet.
  Detected while writing 11.2.b mini-commit 1 (`src/view/check.rs`
  tests). The checker itself handles those shapes correctly — the
  tests for them construct the `ExpandedViewFile` directly. Fix
  scope for a follow-up view-lexer mini-commit: emit `Lt`, `Gt`,
  `Question` tokens (preserving `<template>` / `<style scoped>`
  detection order), extend `append_token_source` +
  `needs_space_before`, keep the shell parser's error unchanged
  for those tokens at top level.

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

No other files touched by 11.1 / 11.2.a / 11.2.b mini-commit 1.
Invariants 1-5 of `docs/stack.md` verified for each closure by
running `cargo test --lib`, `cargo test --test cli_e2e --release`
(101/101), `cargo test --test openapi_e2e --release` (3/3),
`cargo fmt --all --check`, and `cargo clippy --lib --tests --bins
-- -D warnings` — all green (delta at 11.2.b mini-commit 1: +16
`view::check::*` tests over 11.2.a's baseline of 3273 unit).

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
