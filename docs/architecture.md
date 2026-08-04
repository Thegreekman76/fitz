# Compiler architecture

This document describes how the code in [src/](../src/) is laid out and
what happens to a Fitz program from the moment it is text in a `.fitz`
file to the moment it produces output (a `print`, an HTTP response, a
native binary, or a reported test). It is the reference for understanding
the compiler from the inside; to learn the language from the outside,
see [docs/guide.md](guide.md).

> **Status**: this doc covers up to the close of **all of Phase 12 +
> Phase 8.b/8.c + TaskHub** (v0.15.x, June 2026). Refresh synchronized
> with the repo state. It includes the Phase 9.w MVP (auth + WS + cron)
> and its iteration 2 (RBAC + jobs persistence), all of Phase 10
> (pure Postgres driver + ORM + migrations), Phase 12 (healthz/readyz
> + Secret + OTel observability + Docker + deploy + @trace/@metric/
> @flag), Phase 8.b/8.c (embedded CPython + bundled pip packages),
> and the CLI builder (Phase 13). When we advance to Phase 13+ or
> further, this doc is updated alongside.

## Pipeline in one picture

```mermaid
flowchart TD
    Source[".fitz file<br/>(source text)"] --> Lexer["<b>lexer.rs</b><br/>tokenize"]
    Lexer --> Tokens["Vec&lt;Token&gt;"]
    Tokens --> Parser["<b>parser.rs</b><br/>parse with precedence<br/>(uses ast.rs)"]
    Parser --> AST["Program = Vec&lt;Stmt&gt;<br/>(ast.rs)"]
    AST --> Checker["<b>types.rs</b><br/>check_program<br/>(resolve + check)"]
    Checker -->|errors| Abort["✗ stderr + exit 1"]
    Checker -->|OK + TypeEnv + TypeInfo + DefinitionInfo| Fork{"fitz<br/>subcommand"}

    Fork -->|fitz check| OK["✓ no type errors"]

    Fork -->|fitz run<br/>fitz test<br/>fitz repl<br/>fitz dev| Eval["<b>evaluator.rs</b><br/>execute AST<br/>(env.rs + value.rs<br/>+ cron_jobs.rs)"]
    Eval -->|registered HTTP routes| Http["<b>http.rs</b><br/>axum + tokio<br/>multi-thread"]
    Eval -->|WS routes (@ws)| Ws["<b>http.rs + asyncapi.rs</b><br/>WsConn&lt;T&gt; + AsyncAPI 3.0"]
    Eval -->|@cron jobs| Cron["<b>cron_jobs.rs</b><br/>tokio::spawn scheduler<br/>+ opt-in DB persistence"]
    Eval -->|no routes| StdOut["stdout / output / test report"]
    Http -->|/openapi.json + /docs| Openapi["<b>openapi.rs</b><br/>OpenAPI 3.1 schema"]
    Http -->|/healthz + /readyz + /metrics| HealthMetrics["healthz/readyz<br/>+ Prometheus"]
    Http -->|spans + logs + metrics| Otel["<b>observability.rs + logging.rs</b><br/>OTLP exporter + structured JSON"]
    Http --> Server["server at host:port"]

    Fork -->|fitz build| Codegen["<b>codegen.rs</b><br/>AST + TypeEnv → Rust"]
    Codegen --> Project["Cargo project at<br/>target/fitz-build/&lt;stem&gt;/"]
    Project --> Cargo["cargo build --release"]
    Cargo --> Bin["native binary<br/>(next to the .fitz<br/>or in target/release/)"]
    Cargo -->|--bundle-python/--bundle-pip| Launcher["<b>launcher_template.rs<br/>+ pbs.rs</b><br/>embedded CPython + pip"]

    Fork -->|fitz openapi| OpenApiCmd["<b>openapi.rs</b><br/>standalone schema"]
    Fork -->|fitz fmt| Fmt["<b>fmt.rs</b><br/>pretty-printer"]
    Fork -->|fitz lint| Lint["<b>lint.rs</b><br/>pattern linter"]
    Fork -->|fitz test| Test["<b>testing.rs</b><br/>@test fn registry"]
    Fork -->|fitz db &lt;sub&gt;| Db["<b>migrations.rs</b><br/>schema diff + introspect"]
    Db -->|diff/migrate/inspect| Driver["<b>db.rs</b><br/>pure Postgres driver<br/>(wire v3.0 + TLS + pool)"]
    Driver --> Postgres[("real Postgres")]
    Db -->|new/squash/stamp| FileSystem[".sql/.fitz files<br/>in migrations/"]
    Fork -->|fitz docker init/build| Docker["<b>docker.rs</b><br/>auto-generated Dockerfile<br/>+ compose"]
    Fork -->|fitz deploy| Deploy["<b>deploy.rs</b><br/>thin wrapper over<br/>docker/compose"]
    Fork -->|fitz py-types<br/>fitz py-stubs| PyTools["<b>py_types.rs<br/>+ pyi_stub.rs</b><br/>SQLAlchemy → Fitz<br/>+ .pyi stubs"]
    Fork -->|fitz new<br/>fitz init<br/>fitz add/remove/update| Pm["<b>manifest.rs<br/>+ lockfile.rs<br/>+ git_dep.rs</b><br/>package manager"]
    Pm --> Toml["fitz.toml + fitz.lock"]

    classDef good fill:#dff5dd,stroke:#3a8a3a
    classDef bad fill:#fcdede,stroke:#a33
    classDef input fill:#e0e8ff,stroke:#446
    classDef external fill:#fff5dd,stroke:#aa8
    class Source input
    class OK,StdOut,Server,Bin,OpenApiCmd,Fmt,Lint,Test,FileSystem,Toml,Launcher,HealthMetrics,Otel,Docker,Deploy,Ws,Cron,PyTools good
    class Abort bad
    class Postgres external
```

ASCII fallback (same diagram, no colors, for terminals and editors
that do not render mermaid):

```
                                .fitz file
                                    │
                                    ▼
                              ┌──────────┐
                              │ lexer.rs │   tokenize
                              └────┬─────┘
                                   ▼
                              Vec<Token>
                                   │
                                   ▼
                              ┌───────────┐
                              │ parser.rs │  precedence + structure
                              │  (ast.rs) │  (Program = Vec<Stmt>)
                              └────┬──────┘
                                   ▼
                                Program
                                   │
                                   ▼
                              ┌──────────┐
                              │ types.rs │  resolve + check
                              └────┬─────┘
                                   │ (TypeEnv + TypeInfo + DefinitionInfo)
                                   │
   ┌───────┬──────┬───────┬───────┬┴──────┬───────┬─────────┬─────────┬──────┐
   ▼       ▼      ▼       ▼       ▼       ▼       ▼         ▼         ▼      ▼
 check   run/   build  openapi  fmt     lint    test    db <sub>   docker  deploy
        test/         openapi  fmt.rs  lint.rs testing  migrations docker  deploy
        dev/           .rs                      .rs     .rs        .rs     .rs
        repl                                                                │
          │              │                                                  │
          ▼              ▼                                                  ▼
   evaluator.rs       codegen.rs                                    wrapper
   env.rs+value       Cargo project                                docker/
   cron_jobs.rs       │                                            compose
   testing.rs         ▼
        │         cargo build --release
        │             │
   ┌────┴───────────┐ ▼
   ▼    ▼   ▼      ┌──────────┐
  CLI  http  cron  │  native  │ (+ launcher_template + pbs if
 stdout axum sched │ binary   │  --bundle-python/--bundle-pip)
       tokio  │   └──────────┘
       multi  │
       thread │
       │     │
       ▼     ▼
    server   jobs
   + /healthz/readyz/metrics
   + /openapi.json + /docs
   + /asyncapi.json (if @ws is present)
   + structured logs (logging.rs)
   + OTel spans (observability.rs)
       │
       ▼
   Postgres (db.rs pure driver: wire v3.0 + TLS + pool)
```

## The CLI flows

The CLI ([main.rs](../src/main.rs)) has **33 sub-commands**
(19 top-level + 10 `db ...` sub-commands + 2 `docker ...` + 2
`deploy ...`) grouped into **8 families**. They all share the
front-end (lexer → parser → checker) when they work with Fitz
code; they fork after that:

**Family 1 — Language core pipeline**:
- **`fitz run [file]`** — Type-checks in strict mode (flag
  `--no-typecheck` downgrades it to a warning) and executes the
  AST with the evaluator. Without args, it looks for `fitz.toml`
  and runs `[bin].main`. If the program registered HTTP / WS /
  `@cron` routes, it starts the server / scheduler.
- **`fitz build [file]`** — Strict check (no escape), generates a
  Cargo project, invokes `cargo build --release`, copies the
  binary. Without args, manifest mode + output to
  `target/release/<pkg-name>`. Phase 8.b/8.c flags:
  `--bundle-python` + `--bundle-pip <PACKAGE>` (repeatable) +
  `--bundle-pip-requirements <FILE>` (repeatable) to embed
  CPython 3.14.x via python-build-standalone and preinstalled pip
  packages (standalone binaries that do NOT require Python at the
  destination).
- **`fitz check [file]`** — Lex + parse + type-check. Reports
  errors and exits (useful for editors / CI).
- **`fitz openapi <file>`** — Emits an OpenAPI 3.1 schema to
  stdout (HTTP handlers discovered in the AST).

**Family 2 — Package manager (Phase 9.y)**:
- **`fitz new <name>`** — Creates a new project (`<name>/fitz.toml`
  + `src/main.fitz` + `git init`).
- **`fitz init`** — Turns the current cwd into a project.
- **`fitz add <name> --path/--git`** — Adds a dep to `fitz.toml`
  + syncs `fitz.lock`.
- **`fitz remove <name>`** — Removes a dep + syncs the lockfile.
- **`fitz update [name]`** — Re-resolves deps (re-clones git deps).

**Family 3 — DX (Phase 9.z)**:
- **`fitz fmt [files] [--check]`** — Zero-config pretty-printer
  (preserves the user's comments + blank lines).
- **`fitz test [filter] [--file]`** — Built-in test runner
  (discovers `@test` fns, cargo-style output).
- **`fitz dev [--file]`** — Hot reload with a file watcher
  (kill+respawn of the child on change).
- **`fitz repl`** — Interactive REPL with a shared env between
  lines.
- **`fitz lint [files] [--deny <name>]`** — Pattern linter
  (4 lints: unused_variable, unused_import, useless_match,
  string_concat).

**Family 4 — Python interop (Phase 8, feature `python`)**:
- **`fitz py-types <file.py>`** — Generates Fitz `type` from
  SQLAlchemy models (introspection via duck typing on
  `__table__.columns`).
- **`fitz py-stubs <file.py>`** — Generates `.pyi` stubs from Fitz.
- **Stub auto-pickup** (Phase 8-pyi.B, not a sub-command): when
  the program has `from python import foo`, the loader looks for
  an adjacent `<base_dir>/foo.pyi` and registers nominals declared
  there in the `TypeEnv` before the checker — the user can write
  `let u: User = requests.fetch(...)?` with `User` declared in
  `requests.pyi` and the checker resolves it.

**Family 5 — DB / migrations / ORM (Phase 10 + Tier S v0.10.28-29)**:
- **`fitz db diff [--file] [--out] [--check-destructive] [--allow-destructive]`**
  — Compares the declared schema to the real DB and emits DDL.
  Classifies each change as Safe/Risky/Destructive
  (v0.10.31 Tier A.1).
- **`fitz db migrate [--dry-run] [--sql]`** — Applies pending
  migrations to the DB. Idempotent tracking via the
  `_fitz_migrations` table.
- **`fitz db status`** — Lists applied vs pending migrations.
- **`fitz db new <name>`** — Creates a new `.sql`/`.fitz` file
  with a timestamp prefix.
- **`fitz db rollback [--count N]`** — Reverts the last N
  migrations (uses the `-- DOWN` block).
- **`fitz db check [--file]`** — Validates schema + migrations
  consistency without touching the DB.
- **`fitz db history`** — Lists applied migrations with timestamps.
- **`fitz db squash <from> <to>`** — Combines migrations into one.
- **`fitz db stamp <version> | --all`** — Marks migration(s) as
  applied WITHOUT executing SQL (adopt a legacy DB).
- **`fitz db inspect [--schema | --all-schemas | --table | --json]`**
  — Introspects the **real** schema with a text view +
  machine-readable JSON.

**Family 6 — Editor support (Phase 9.x, feature `lsp`)**:
- **`fitz-lsp`** (separate bin at `src/bin/fitz-lsp.rs`, not a
  sub-command) — Language server over tower-lsp. Consumed by the
  VSCode extension (`editors/vscode/`).

**Family 7 — Docker stack (Phase 12.4)**:
- **`fitz docker init [--force]`** — Generates a multi-stage
  `Dockerfile` + `.dockerignore` + smart `docker-compose.yml` in
  the manifest directory. AST-only detection of the entry point
  to decide `EXPOSE <port>` (if there is `@server(N)`) +
  `postgres:16-alpine` service with healthcheck (if there is
  `db.connect(...)`). Sub-step 12.4.b adds the
  `python:3.12-slim-bookworm` fallback when there is Python
  interop + `restart: unless-stopped` when there is `@cron`.
- **`fitz docker build [--tag <X>]`** — Thin wrapper over
  `docker build` that uses the Dockerfile from manifest_dir and
  tags with `<package.name>:latest` (override with `--tag`).

**Family 8 — Deploy orchestrator (Phase 12.6)**:
- **`fitz deploy docker [--tag <X>] [--no-push]`** — `docker build`
  + `docker push` with the tag derived from the manifest. MVP
  targets are only docker and compose; `fly`/`railway`/`k8s`
  remain as visible debt.
- **`fitz deploy compose [--no-detach] [--no-build]`** — Thin
  wrapper over `docker compose up -d --build`.

## Modules in src/

### main.rs — entry point and CLI

Parses arguments with [clap](https://docs.rs/clap), routes to the
right subcommand, orchestrates the pipeline steps. Each error ends
with `exit(1)` and a message to `stderr`. Deliberately thin: it
contains no language logic, only coordination. The `dev`, `repl`,
`lint`, `fmt`, `test`, `add`, `remove`, `update`, `new`, `init`
sub-commands have their `*_cmd` functions inside it.

### lib.rs — crate library

Exposes the modules as `pub mod` so the bins (`fitz` at
`src/main.rs`, `fitz-lsp` at `src/bin/fitz-lsp.rs`) consume them
via `use fitz::...`. Refactor introduced in Phase 9.x.1.b when the
LSP landed — before then Fitz was bin-only. The lib also makes
unit testing easier and supports future integration with other
tools (for instance, an external linter or formatter that reuses
`parser` and `types`).

### lexer.rs — tokenization

Converts source text into a `Vec<Token>`. Recognizes literals (Int,
Float, Str, Bool, Null), identifiers, keywords (`fn`, `if`,
`while`, `for`, `type`, `match`, `return`, `import`, `async`,
`await`, `as`, etc.), operators, delimiters, and decorators
(`@name`). Handles interpolated strings at the lexical level
(detects `{` inside the string); the `Expr::StrInterp` assembly is
finished by the parser. Annotates each token with its position
(line, column).

Phase 9.z.1.b added a **`Trivia` side-stream**: a second output
from the lexer with `Vec<Comment>` (line + block) and `Vec<usize>`
(blank lines). The `tokenize_with_trivia` function exposes it for
the formatter; the regular `tokenize` (parser/checker/eval)
ignores it — zero overhead for the rest.

### ast.rs — AST definition

Pure types, no logic. Defines `Program = Vec<Stmt>`, where `Stmt`
covers `FnDef`, `TypeDef`, `Assign`, `If`, `While`, `For`, `Loop`,
`Return`, `ReturnStatus` (custom HTTP status), `Import`,
`FromImport`, `Expr` (statement-expression), `Break`, `Continue`,
`Error` (recovery, Phase 9.0). `Expr` covers literals, `Ident`,
`BinOp`, `UnaryOp`, `Call`, `FnExpr`, `Field`, `Index`, `Match`,
`Try` (`expr?`), `Await`, `Range`, `List`, `Map`, `StructLit`,
`Ok`, `Err`, `StrInterp`, `Error`. Aside: `TypeExpr` (`Named`,
`Generic`, `Nullable`, `Function`) for annotations, `Pattern` for
`match`, and `Decorator`
(`@get`/`@server`/`@middleware`/`@header`/`@test`) with `args` +
`kwargs`. `Expr` and `Stmt` carry `span` for positioned errors
(S1.2 + S1.codegen, post-5b).

### parser.rs — AST construction

Recursive descent with a precedence climbing ladder. Takes
`Vec<Token>` and returns `Result<Program, FitzError>`. Handles
every language construct, including type annotations with nested
generics (`Map<Str, List<Int>>`), nullable suffix (`Str?`),
function type (`Fn(Int) -> Int`), `match` patterns, optional
parentheses on decorators (post-9.z.2.a, required for
`@test fn ...`), multi-line method chains (post-PreF8.2), aliases
in imports (`from foo import bar as b`).

It also has `parse_with_recovery` (Phase 9.0 / F15): an
error-tolerant variant that returns `(Program, Vec<FitzError>)` —
it synthesizes `Stmt::Error` / `Expr::Error` nodes where the parse
failed and continues with the next stmt. Used by the LSP so
`did_change` emits diagnostics over buffers under construction.

### value.rs — runtime values

The `Value` enum that lives during `fitz run`/`test`/`dev`/`repl`:
`Int`, `Float`, `Str`, `Bool`, `Null`, `List`, `Map`, `Instance`,
`Result`, `Function` (built-in or user), `Module`, `Type`, `Future`
(post-Phase 6), `HttpResponse`, `CorsConfig`, `PyObject` (feature
`python`).

`List`, `Map`, and `Instance` are wrapped in
`Arc<parking_lot::Mutex<...>>` (alias `Shared<T>`) to model
shared-reference semantics and to make `Value` `Send` — that
unlocked F17.5 (removal of the HTTP bridge) and the multi-thread
tokio runtime for the server. Includes an `impl Display` that
produces the canonical format (strings with double quotes inside
collections, `Float` with `.0`, etc.) that the codegen replicates
bit-for-bit.

### env.rs — environments / scopes

`Environment` with a stack of scopes (`Arc<Mutex<...>>` after F17).
Methods `define`, `get` (recursive lookup walking up to parents),
`assign` (overwrites in the scope where it was defined), `has`,
`local_names` (for the REPL's `:env`, no recursion). Closures
(`Value::Function { closure }`) capture a handle of the
definition's env; the evaluator adds a child for the params at
invocation time.

### types.rs — type system and static checker

Two responsibilities:

1. **Resolution**: converts `TypeExpr` (syntactic) into `Type`
   (resolved, with nominal identity). `Type` covers primitives,
   built-in generics (`List<T>`, `Map<K,V>`, `Result<T>`,
   `Nullable<T>`, `Future<T>`), `Nominal(TypeId)` for custom
   types, `Function { params, ret }`, `PyAny` (Phase 8.4), and
   `Any` as the gradual-typing escape hatch.
2. **Checker**: `check_program(&Program)` walks the AST with a
   `CheckCtx` (scopes, `return_stack` for `?` and `return`,
   `inferred_returns` for `FnExpr.ret`, `def_info` for the LSP's
   go-to-def). It synthesizes types, validates calls (arity +
   types), checks exhaustiveness of `match` over `Result`, and
   validates built-in methods with parametric templates. Returns
   `(TypeEnv, TypeInfo, DefinitionInfo, Vec<FitzError>)`.

`TypeInfo` (F16) records the synthesized type of each `Expr`
indexed by span — consumed by the LSP `textDocument/hover` and by
the REPL's `:type`. `DefinitionInfo` records
`(use_span, def_span)` for `textDocument/definition`.

### evaluator.rs — AST execution

The interpreter. Takes `Program` + `Environment` and produces
effects. Handles control flow (`return`, `break`, `continue`) via
internal signals. Resolves `import` by loading the canonicalized
file, parsing it recursively, with a per-path cache and cycle
detection via stack. Dispatches built-in methods (`xs.map`,
`m.get`, `s.upper`, etc.) by receiver type. HTTP decorators
(`@get`/`@post`/etc.) register routes in the active
`HttpRegistry`; `@test` registers in the active `TestRegistry`
(Phase 9.z.2); `@server` configures `ServerConfig`.

After Phase 6.4 (native async) the evaluator functions are
`async fn` and use `#[async_recursion]` so `eval_expr`/
`eval_stmt`/`eval_call` can recurse through `.await`. After F17
all signatures are `Send` — a single tokio runtime handles both
the evaluator and axum, and HTTP handlers run in parallel across
workers.

Public APIs consumed by the sub-commands:
- `eval_with_base_and_deps` for `fitz run` and `fitz test`.
- `eval_program_with_env` (Phase 9.z.4) for the REPL: shared env
  across invocations + returns the `Value` of the last stmt for
  Python-style pretty-print.
- `run_test_handler` (Phase 9.z.2) to invoke a `@test fn` (sync
  or async) and translate the result into ok/FAILED.
- `build_runtime` to build the tokio current_thread runtime
  shared by every async sub-command.

### http.rs — HTTP runtime + WebSockets + healthz/metrics

Powers the native HTTP layer. After F17.5 there is no more
`mpsc/oneshot` bridge or separate `std::thread` for tokio — a
single tokio `rt-multi-thread` runtime runs both the evaluator
and axum, and the axum handlers invoke the evaluator directly
over a shared `Arc<HttpRegistry>`. Real parallelism across
requests.

Components:
- `HttpRegistry`: route table (`method`, `path_template`,
  `handler`, `RouteMeta`, `middlewares`, `cors`, `required_roles`,
  `is_ws`, `ws_msg_type`). Thread-local + `parking_lot::Mutex`
  for the active registry. Includes
  `ws_broadcaster: Arc<WsBroadcaster>` for the WebSocket fan-out
  and `cron_registry: Arc<CronRegistry>` so that the lifecycle
  shared between the HTTP server and the cron scheduler reuses
  the same tokio runtime.
- `serve(registry, program, addr)`: builds `Arc<HttpRegistry>` +
  `axum::Router` + `TcpListener`, calls `axum::serve` with
  graceful shutdown (SIGTERM drain with 30s + readyz → 503).
  Pre-computes the OpenAPI + AsyncAPI schema eagerly and serves
  them at `/openapi.json` + Scalar UI at `/docs` +
  `/asyncapi.json`.
- `build_router(metas, registry, openapi_schema)`: assembles the
  `Router` with one closure per route + CORS preflight + middleware
  chain. Auto-mounts `/healthz` + `/readyz` (Phase 12.1) and
  `/metrics` Prometheus (Phase 12.3.iter2.Tier3) with override
  if the user declares handlers with the same names.
- **WebSockets** (Phase 9.w.2): `build_ws_method_router` with the
  `WebSocketUpgrade` extractor + pre-upgrade auth (401/403
  without opening the socket) + optional heartbeat ping/pong via
  `@server(ws_heartbeat_secs=N)`. `WsConn<T>` with
  `recv`/`send`/`broadcast`/`close` methods doing automatic JSON
  marshaling against the declared `type T`.
- **Native auth** (Phase 9.w.1): `dispatch_request` runs
  `@auth_provider` before body parsing if the route has
  `@authenticated`/`@admin`/`@requires("role")`. The provider
  returns `Result<User>` → 401 if Err, 403 if admin/role does not
  match.
- `MiddlewareKind::{Pre, Post}` + `MiddlewareSpec.kind`: mini-batch
  Mw.next classifies middlewares by arity. Pre (1 arg) runs before
  the handler with gate-only semantics; Post (2 args
  `(Request, Response)`) runs after the handler in reverse order.
- `parse_urlencoded_body`: parses an
  `application/x-www-form-urlencoded` body into a
  `Value::Map<Str, Str>`. Strict Content-Type 415.
- `value_to_json` / `json_to_value` / `json_to_instance`:
  translate between `Value` and `serde_json::Value`. With a
  schema (declared `type`), `json_to_instance` validates fields,
  applies defaults, rejects extras (→ 400). `Value::Secret` is
  redacted to `"***"`.
- `parse_path_template` / `coerce_path_param`: extract typed path
  params + query params (post-F7).
- `ServerConfig`: `@server(port, host, docs=Bool, api_version="X",
  observability=Bool, prometheus=Bool, ws_heartbeat_secs=N)`.
  Default `127.0.0.1:3000`.

### asyncapi.rs — AsyncAPI 3.0 generator (Phase 9.w.2.d)

Parallel to `openapi.rs` but for `@ws("/path")` handlers. Emits
channels + receive/send operations + securitySchemes.bearerAuth
when there is auth. `channels_from_registry` (runtime, consumed
by `serve`) and `pseudo_channels_from_ast` (build-time, consumed
by `codegen.rs` to embed the schema bit-for-bit in the binary).
`BTreeMap` for deterministic order. Served at `/asyncapi.json`
when there are `@ws` handlers.

### cron_jobs.rs — `@cron` + `@background` job scheduler (Phase 9.w.3)

`CronJob` (handler + parsed Schedule + tz + retry + catch_up +
optional store for persistence) + `CronRegistry` (parallel to
`HttpRegistry`, lives as `cron_registry: Arc<CronRegistry>`
inside). `spawn_cron_scheduler` starts one `tokio::spawn` per
job with a loop `sleep_until(next_tick) → invoke_with_retry`.

Persistence (Phase 9.w.3.iter2): when the job declares
`store=<db_binding>`, it creates the `fitz_cron_jobs` +
`fitz_cron_runs` tables automatically via
`CREATE TABLE IF NOT EXISTS` at scheduler boot; persists every
attempt with status `running`/`ok`/`retrying`/`failed`; opt-in
`catch_up=true` runs ONE immediate run at boot if there were
missed runs (not N — avoids spam). CREATE TABLE race condition
serialized via a global `tokio::sync::OnceCell` (`v0.15.13` fix
+ parallel in codegen v0.15.14).

`spawn(fn_call)` fire-and-forget integrated with the same model:
`tokio::spawn(invoke)` wraps the JoinHandle in `Value::Future`,
allows `await` or discard.

### codegen.rs — transpile to Rust

Generates a complete Cargo project from the AST + `TypeEnv` +
`TypeInfo` (mini-batch Hpx.2 — the codegen consults the side-table
to infer return types of unannotated fns).
`generate_project(path, program, type_env, type_info, dep_registry)`
returns `Cargo.toml` + `src/main.rs` + helper modules.

**Helpers for inference** (mini-batches Hpx.2 + 5b.1 + P2):
- `infer_return_type_from_body(body, type_info)`: walks
  `Stmt::Return(e)` in the body, looks up `e.span()` in `TypeInfo`,
  unifies with `lub`.
- `infer_param_type_from_call_sites(program, fn_name, idx, type_info)`:
  scans the program for calls to `fn_name`, extracts the type of
  the `idx`-th arg.
- `has_unannotated_fn_params(program)` +
  `fill_inferred_param_types(program, type_info)`: `build_file`
  uses these to mutate the AST in place after the first checker
  pass, filling inferred params. The checker is re-run to refine
  `TypeInfo` and support the 5b.1+Hpx.2 chain (both param and
  return inferred when the return depends on the param).

**Mw.next codegen** (mini-batches P1 + RP):
`HandlerSig.mw_user_fns_post` stores the names of post mws.
`CodegenCtx.middleware_post_fn_names` tracks fns marked as Post
(classified by arity in the post-scan of `pre_register_fns`).
`gen_top_fn` emits Post mws with return `__FitzResponse` (not
Option). `emit_handler_dispatch_and_response` builds an
intermediate `__FitzResponse`, runs the Post mws chain in
reverse, and converts it to an axum Response at the end. Covers
handlers with return plain T, returns_response (ReturnStatus),
and Result<T>.

Type mapping (post-F17.4b):
- `Int → i64`, `Float → f64`, `Str → String`, `Bool → bool`,
  `Null → ()`.
- `List<T> → Arc<Mutex<Vec<T>>>` (`std::sync::Mutex`, no extra
  deps in the generated `Cargo.toml`).
- `Map<K,V> → Arc<Mutex<Vec<(K,V)>>>`.
- `Result<T> → Result<T, String>` (Err pinned to String).
- `type Foo { ... } → struct FooData { ... } + type Foo =
  Arc<Mutex<FooData>>`. `PartialEq` emitted manually (Mutex does
  not impl PartialEq) with a recursive `field_eq_expr` helper.

Shared HTTP state goes from `thread_local!` (pre-F17) to
`static X: LazyLock<Arc<Mutex<T>>>` + `(*X).clone()`
materialization in each handler. Field access is emitted as a
scoped block `{ let __obj = ...; let __g = __obj.lock().unwrap();
__g.<f>.clone() }` to avoid deadlock by re-locking.

`fitz build` with HTTP emits `#[tokio::main]` default multi-thread
+ `axum::Router` + async handler wrappers. Conditional
Cargo.toml: `axum`/`tokio`/`serde` only if there are HTTP
decorators, `pyo3` only with feature `python`. Rule of thumb:
binary output is bit-for-bit identical to `fitz run` for programs
in the supported subset.

It has its own `ModuleLoader` that mirrors the evaluator's but
AOT, and a `check_no_python_imports` guard that aborts builds
with `from python import` unless the `python` feature is active
(Phase 8.7 = debt F19 closed — the binary linked with PyO3 now
supports interop).

### openapi.rs — OpenAPI 3.1 generator

`generate_openapi(routes, program)` produces a `Value::Map` with
the OpenAPI 3.1 structure (`openapi`, `info`, `paths`,
`components`). It receives routes from two sources:
- `routes_from_registry`: routes resolved at runtime (consumed by
  `fitz run`).
- `pseudo_routes_from_ast`: routes inferred from the AST without
  evaluation (consumed by standalone `fitz openapi` + by the
  codegen to embed the schema bit-for-bit in the binary).

Supports custom status codes (post-F7), query params, headers via
`@header(name="X")`, opt-out with `@server(docs=false)`,
configurable `api_version`. Renders the Scalar UI via
`templates/scalar.html` when serving `/docs`.

### manifest.rs — `fitz.toml` of the package manager (Phase 9.y.1)

Defines `Manifest { package, bin, lib, dependencies }` + `Package`
+ `Bin` + `Lib` + `Dependency` (Detailed with
`path`/`git`/`tag`/`rev`) + `DepRegistry = HashMap<String, PathBuf>`.
APIs:
- `find_manifest(start)`: cargo-style walk-up.
- `Manifest::parse(text)`: TOML → struct via `serde`.
- `resolve_dependencies(manifest, base_dir)`: resolves each dep
  to a `ResolvedDep` (absolute path to `lib.entry`). Path deps
  resolve immediately; git deps delegate to `git_dep.rs`.
- `build_dep_registry(resolved)`: from `Vec<ResolvedDep>` to the
  `DepRegistry` that the evaluator's loader consumes.
- `write_manifest_with_edit` (Phase 9.y.4): preserves comments
  via `toml_edit` when `fitz add`/`remove` edits the file.

Name validation: `^[a-z][a-z0-9_-]{0,63}$` (crates.io style).

### lockfile.rs — `fitz.lock` (Phase 9.y.3.a)

`Lockfile { version: 1, packages: Vec<LockedPackage> }`. Each
`LockedPackage` carries `name`, `version`, and an optional
`source` (for git deps: `git+<url>#<commit>`). Path deps have no
source.

`from_resolved(resolved)` builds the lockfile from the resolution
result; `write_lockfile_if_changed(path, lock)` short-circuits
byte-for-byte to avoid touching mtime when content has not
changed. `fitz run`/`build`/`check` sync it automatically on each
invocation.

### git_dep.rs — git deps + cache (Phase 9.y.3.c)

Handles `[dependencies] foo = { git = "<url>", tag = "v1" }`.
Clones to `<cache_dir>/git/<sanitized-url>@<ref>/` (default
`~/.fitz/cache/`, override via `FITZ_CACHE_DIR`). Split strategy:
`--depth 1 --branch <tag>` for tags, full clone + checkout for
revs. Detects the exact commit hash for the lockfile via
`git rev-parse HEAD`. Cache reuse with no automatic re-clone;
`fitz update <name>` invalidates the cache.

### templates.rs — project scaffolding (v0.20.0)

Powers `fitz new my-app --template <name>`. Registry with one entry
today (`liveviews` →
`https://github.com/Thegreekman76/fitz-liveviews` at `templates/basic`
on `main`); env var overrides per template
(`FITZ_TEMPLATE_LIVEVIEWS_URL/SUBPATH/REF`) change only known names
so tests + power users can retarget without opening the registry.

`scaffold_from_template(source, target_dir, project_name)` shallow
clones to an in-house self-cleaning `TempDir` (avoids promoting
`tempfile` from dev-dep to runtime dep), copies the requested subpath
(skipping any nested `.git/`), and substitutes `{{name}}` in every
UTF-8-decodable file with the project name (binaries copy verbatim).
Two-strategy clone parallels `git_dep::clone_fresh`:
`--depth 1 --branch <ref>` first, full clone + `git checkout` fallback.

### db.rs — pure Postgres driver + ORM (Phase 10, ~3000+ LoC)

The largest module in the project by LoC. Implements a
**complete Postgres driver in pure Fitz/Rust** (no
`tokio-postgres` / `sqlx` / `diesel` / `libpq`):

- **Wire protocol v3.0**: `Connection::connect` performs the
  handshake + SCRAM-SHA-256 auth (RFC 7677) + PBKDF2-HMAC-SHA-256
  + StartupMessage. Simple Query and Extended Query with
  Parse/Bind/Describe/Execute/Sync. ErrorResponse parsed
  structurally into `DbError::Server { severity, code, message }`
  with native SQLSTATE codes.
- **Strict TLS** (v0.10.23): `sslmode=require` / `verify-ca` /
  `verify-full` via `rustls` (system CA bundle or custom
  `sslrootcert=<path>` PEM). `disable` also supported for local
  dev.
- **Connection pool**: `DbPool` with `idle: Mutex<Vec<Connection>>`
  + `permits: Semaphore` (default 10, override
  `FITZ_DB_MAX_CONNS` v0.10.29 with clamp `[1, 200]`). Global
  cache `OnceLock<HashMap<URL, Arc<DbConnHandle>>>` — multiple
  `db.connect(url)` with the same URL return the SAME handle
  (avoids the "connection pool leak" where each call created a
  fresh pool). Background health-check task with `tokio::spawn`
  that closes stale idle conns every 30s.
- **PgValue types**: 11 core OIDs (BOOL/INT/FLOAT/TEXT/BYTEA/
  DATE/TIME/UUID/JSON/JSONB/VOID) + 12 array OIDs (`int4[]`,
  `text[]`, `jsonb[]`, etc.). Bidirectional marshaling Fitz
  `Value` ↔ `PgValue` respecting NULL inside arrays
  (`List<Int?>` ↔ `int8[]` with `{a,NULL,c}` format).
- **Observability** (v0.10.28-29): `FITZ_DB_LOG=1|verbose` env
  var opt-in. Mode `verbose` applies **secret redaction**
  (v0.10.29) on params via a contextual heuristic over the SQL
  (`password`/`secret`/`token`/`api_key`/etc. → `<redacted>`).
- **Enriched errors** (v0.10.29): `DbError::Server` Display now
  shows `<severity> [<SQLSTATE>]: <msg>`. Failed queries go
  through `enrich_db_error_with_context` which appends
  `[sql: <one-line truncated> params=[...]]` (with redaction).

The ORM (decorators `@table`/`@primary`/`@column`/`@belongs_to`/
`@has_many`/`@has_one`/`@unique`/`@check_constraint`/`@index`)
does NOT live in `db.rs` — the decorators are processed in
`types.rs` and populate `TableMetadata`. The SQL builder for the
read/write methods lives in `evaluator.rs`
(`translate_method_call_to_sql` for `.where(...)` closures +
related methods) and `codegen.rs` (parity for `fitz build`).

### migrations.rs — schema diff + introspect + DDL emit (Phase 10.6+)

Automatic migrations system parallel to Alembic / Flyway /
TypeORM CLI, but with no external deps — all Rust + the driver's
wire protocol.

- **`Schema`** = `Vec<Table>`; **`Table`** = `(name, columns,
  indexes, foreign_keys, composite_pk, check_constraints,
  schema, renamed_from)`. **`Index.using/where_clause`**
  + **`ForeignKey.references_schema`** (v0.10.29 — transparent
  cross-schema FK).
- **`schema_from_program(program, type_env)`** — Builds the
  "target" schema from the AST's `@table` types. Resolves
  TypeMetadata → Table with auto-naming (constraints
  `<table>_<col>_fkey`/`idx_<table>_<col>`/`chk_<table>_<idx>`),
  composite PK as a table-level `PRIMARY KEY (a, b)`,
  qualified cross-schema FK.
- **`introspect_schema(conn)`** — Reads the "current" schema from
  the DB with queries against `pg_catalog` (`pg_class`,
  `pg_attribute`, `pg_index`, `pg_constraint`, `pg_am`). Covers
  columns + types + defaults + NOT NULL, indexes with WHERE
  clauses and method (gin/gist/etc.), FKs with ON DELETE.
- **`diff_schemas(current, target)`** — Returns a deterministic
  `Vec<Change>`:
  - `CreateTable` / `DropTable` / `RenameTable`.
  - `AddColumn` / `DropColumn` / `RenameColumn` /
    `AlterColumnType` / `AlterColumnNullable` /
    `AlterColumnDefault`.
  - `CreateIndex` / `DropIndex` (v0.10.29: detects changes in
    `using`/`where_clause`/`unique`/`columns` when names match →
    `DROP + CREATE` to regenerate).
  - `AddForeignKey` / `DropForeignKey` (with the cross-schema
    qualifier).
- **`changes_to_sql(changes)`** — Emits DDL directly executable
  via `psql -f` or `db.exec`.
- **`format_inspection_text` / `format_inspection_json`**
  (v0.10.28) + **`format_inspection_*_all_schemas`** (v0.10.29) —
  Pretty-print of the schema for `fitz db inspect`.
- **Tracking via the `_fitz_migrations` table**:
  `applied_versions`, `record_applied`,
  `apply_pending_migrations`. Idempotent — re-running
  `fitz db migrate` skips the already-applied ones.

### testing.rs — built-in testing (Phase 9.z.2.a)

`TestRegistry { tests: Vec<TestSpec> }` with `TestSpec { name,
handler, is_async, span, source_file }`. Thread-local activated
by `with_active_test_registry` (sync/async) during discovery for
the runner.

`with_test_source` adds another level: the module loader sets it
with the filename of the imported module before evaluation, so
`@test`s declared in modules are labelled with their actual file
(not with the importer's). `current_test_source` is read by the
`@test` branch of `process_decorator` when building the
`TestSpec`.

### fmt.rs — formatter (Phase 9.z.1)

Zero-config pretty-printer over the AST. `format_source(text)`
tokenizes with `tokenize_with_trivia` (Phase 9.z.1.b), parses,
walks the AST emitting each node in its canonical form (4-space
indent, double quotes, mandatory parentheses on conditions,
multi-line type defs, etc.), and interleaves the comments from
the `Trivia` stream at their original position.

Blank-line rules: max 1 consecutive (multiple collapse), mandatory
between top-level `fn`/`type`. Comments are normalized
(`//foo` → `// foo`); trailing comments use 2 spaces of
separation. See [docs/fmt-style.md](fmt-style.md) for the full
reference.

Residual debt: known bug when a fn body ends with a trailing
comment followed by another block — it inserts a spurious blank
inside the second body. Variant of the edge case documented in
`fmt-style.md`. Tracked in `docs/deudas-post-5b.md`.

### lint.rs — linter (Phase 9.z.5)

Pattern linter beyond types. `lint_source(source, program) ->
Vec<LintFinding>` walks the AST collecting findings + applies
suppressions by reading the raw source (looks for
`// @allow(<lint>)` on the line immediately above).

4 implemented lints:
- `unused_variable`: `let x = ...` whose name does not appear in
  uses, skips the `_` prefix.
- `unused_import`: `import X` / `from X import Y` with an
  unreferenced binding.
- `useless_match`: `match expr { _ => body }` with a single
  catch-all arm.
- `string_concat`: `BinOp Add` with both operands as `Str`
  literals.

Walkers `collect_uses_in_*` and `walk_exprs_in_stmt` recurse
through stmts and exprs. Closed catalog (no plugins in the MVP).

### lsp.rs — Language Server (Phase 9.x, feature `lsp`)

Feature-gated behind `cargo build --features lsp`. Implements the
tower-lsp handlers:
- `textDocument/didOpen`/`didChange`/`didClose`: parses with
  `parse_with_recovery`, runs `check_program`, emits
  `Diagnostic`s.
- `textDocument/hover`: reads the type of the node under the
  cursor from `TypeInfo` (F16) and renders it as a markdown
  `fitz` block.
- `textDocument/definition`: reads `DefinitionInfo` and returns
  the `Location` of the use → def_span.
- `textDocument/completion`: enumerates scope-level symbols +
  fields/methods after-dot.

Pure-function pipeline in `check_source_with_types(text) ->
(TypeEnv, TypeInfo, DefinitionInfo, Vec<FitzError>)`. The bin
lives at `src/bin/fitz-lsp.rs` (also feature-gated). The VSCode
client at `editors/vscode/` invokes it as a subprocess.

### py_interop.rs / py_types.rs / pyi_loader.rs / pyi_stub.rs — Python interop (Phase 8, feature `python`)

Feature-gated behind `cargo build --features python`. Links
against CPython 3.10+ via [PyO3](https://pyo3.rs) (`abi3-py310`,
`auto-initialize`).

`py_interop.rs` (Phase 8.1-8.6) exposes `import_module`,
`get_attr`, `call`, `value_to_py`/`py_to_value` (Phase 8.2
bidirectional marshaling of composite types). Python exceptions
are wrapped in `Value::Result(Err)` with format
`"<ClassName>: <message>"` (Phase 8.3). Automatic coroutine
detection + tokio↔asyncio bridge via `tokio::spawn_blocking` +
`asyncio.run_until_complete` (Phase 8.6).

`py_types.rs` (Phase 8.5) implements `fitz py-types`:
introspection of SQLAlchemy models via duck typing over
`__table__.columns`, emits the corresponding Fitz `type`.

`pyi_stub.rs` (v0.9.39) — Parser for Python `.pyi` stubs
(PEP 484/561). MVP scope: top-level `def`/`class`/vars with
annotations, type expressions
`int|str|float|bool|list[T]|dict[K,V]|Optional[T]`,
`Union[T, None]`, etc.

`pyi_loader.rs` (Phase 8-pyi.B, v0.9.57) — Auto-pickup of `.pyi`
stubs adjacent to the root `.fitz`. When the program contains
`from python import foo`, the loader looks for
`<base_dir>/foo.pyi` and, if it exists, parses it with
`pyi_stub::parse_stub` and registers the declared nominals in
the `TypeEnv` BEFORE the checker. Silent fallback: if it does
not exist, the binding remains opaque as `Type::PyAny`.

### logging.rs — Structured logging built-in (Phase 12.3.a)

Implements `log.info`/`log.warn`/`log.error`/`log.debug` with
heterogeneous kwargs (Int/Float/Str/Bool/Null/Secret/List/Map).
Output is flat JSON to stderr (containers/CI/redirection) or
pretty with ANSI colors (TTY). Explicit override via
`FITZ_LOG_FORMAT=json|pretty`. Filtering via `RUST_LOG` (default
`info`). Recursive redaction of `Value::Secret` in List/Map.
`SpanContext` (trace_id 32 hex / span_id 16 hex) flowing through
`tokio::task_local!` for multi-thread correlation; logs inside
an HTTP handler automatically inherit the request's trace_id.

### observability.rs — OTLP exporter for HTTP spans (Phase 12.3.c)

When `OTEL_EXPORTER_OTLP_ENDPOINT` is set, it connects the spans
that `dispatch_request` (12.3.b) opens to a real OTel backend
(Jaeger, Tempo, Honeycomb, Datadog). Without that var,
`init_otel()` is a silent no-op — zero overhead. OTel-standard
env vars: `OTEL_SERVICE_NAME` (default `fitz-app`),
`OTEL_TRACES_SAMPLER_ARG` (ratio `0.0..1.0`, default `1.0`).
Iter2.a (v0.12.1) derived `trace_id`/`span_id` from the OTel span
when active for bit-for-bit Fitz↔OTel correlation: the same
trace_id appears in stderr logs and in the OTel backend.
Iter2.Tier3 (v0.12.2) added a `/metrics` Prometheus endpoint via
`metrics-exporter-prometheus` with dual gate
`@server(prometheus=true)` (compile-time) or `FITZ_PROMETHEUS=1`
(runtime). Iter2.Tier2 (OTel metrics bridge) is **blocked** by
the release of the `metrics-exporter-opentelemetry` crate
compatible with `opentelemetry_sdk 0.32` — Prometheus pull
covers the 90% case in the meantime.

### docker.rs — `fitz docker init` + `fitz docker build` (Phase 12.4)

The `fitz docker init` sub-command generates 3 files in the
manifest directory: multi-stage `Dockerfile` (builder with the
official image `ghcr.io/thegreekman76/fitz:<tag>` that runs
`fitz build`, runtime `gcr.io/distroless/cc-debian12` or
`python:3.12-slim-bookworm` if there is Python interop),
`.dockerignore`, and a smart `docker-compose.yml`. AST-only
detection of the entry point (`@server(N)` → `EXPOSE` + `ports:`,
`db.connect(...)` → service `postgres:16-alpine` + healthcheck,
`@cron` → `restart: unless-stopped`, `from python import` →
runtime swap). Skip-by-default + `--force` policy. Bonus
`fitz docker build [--tag X]` thin wrapper over
`docker build -t <tag> .`.

### deploy.rs — `fitz deploy <target>` orchestrator (Phase 12.6)

Thin wrapper over `docker build/push` and `docker compose up`
depending on the target. MVP targets: `docker` (build + push with
optional `--no-push`) and `compose` (local up with
`--no-detach`/`--no-build`). Aborts if Dockerfile/compose.yml is
missing (suggests `fitz docker init`). Targets
`fly`/`railway`/`k8s` remain as visible debt — for those, run
the CLIs directly. AST-only detection of the entry point to
validate that the project is ready. Does NOT touch codegen
(emits no Rust code).

### launcher_template.rs — Launcher for `--bundle-python` (Phase 8.b)

When `fitz build --bundle-python` is active, the output is NOT
the "real" binary directly, but a standalone Rust **launcher**
that embeds:
1. The PBS tarball (CPython 3.14.x install_only_stripped) via
   `include_bytes!`.
2. The "real binary" (standard transpile with feature `python`,
   links libpython).

On the first run, the launcher extracts everything to
`$TMPDIR/fitz-py-<hash>/`, sets `PYTHONHOME` +
`LD_LIBRARY_PATH`/`DYLD_FALLBACK_LIBRARY_PATH`/`PATH`, and
`exec`s/spawn-and-waits the real binary. A `.extracted` sentinel
marks completion; subsequent runs reuse the dir. Observed timing
Windows 11 SSD: cold ~5 s, warm ~50 ms.

Sub-step 8.c adds `--bundle-pip <PACKAGE>` (repeatable) +
`--bundle-pip-requirements <FILE>`: pip packages are
preinstalled into a temporary venv and packed into a secondary
tarball, also embedded. Cache key (FNV-1a hash over the
`bundle_pip` args + requirements file contents) with an
`inputs_hash` sidecar for reuse.

### pbs.rs — python-build-standalone downloader (Phase 8.b)

Downloads the `install_only_stripped` tarball from the pinned
release (constant `PBS_RELEASE`, manual bump every 3-6 months)
for the destination triple and saves it at
`<cache>/pbs/<tarball-name>` (global cache shared across builds,
default `~/.fitz/cache/`, override via `FITZ_CACHE_DIR`).
Reproducible builds (same policy as `Cargo.lock`). CPython 3.14.x
within the `abi3-py310` range.

### cli.rs — Native CLI builder (Phase 13)

`@command("name", desc="...")` on a fn declares a CLI command.
The binary produced by `fitz build` parses `std::env::args()`
and dispatches to the matching command. Param convention: no
default → required positional arg (`mybin <name>`); with default
→ optional flag (`--name <value>`); Bool with default `false` →
bool flag (`--loud`). Auto-generated help with `mybin --help` +
`mybin <cmd> --help`. The `fitz run` interpreter can also run
CLI programs.

### view/ — `.fitzv` single-file components (Phase 11)

Since v0.21.0, Fitz recognises `.fitzv` as a first-class file
extension for single-file components — a self-contained module
with `<template>`/`<style>`/state/event blocks that compiles to
either **SSR** (classic Fitz consumed by `fitz-liveviews`) or
**WASM client-side** (Rust with `wasm-bindgen`, opt-in feature
`client-wasm`).

The pipeline lives entirely under `src/view/`:

- **`view/lexer.rs`** — tokenizes the `.fitzv` source. Distinct
  from the classic Fitz lexer: recognises `component`, `state`,
  `event`, `from`, `import`, `as` (S.1, v0.21.2) as keywords;
  captures `<template>...</template>` and `<style scoped>...
  </style>` as `TemplateRaw`/`StyleRaw` tokens (the classic
  Fitz lexer would choke on the raw HTML/CSS).
- **`view/parser.rs`** — builds the raw view AST
  (`view/ast.rs`): `ViewFile { imports, components }`.
  Component blocks have state fields with type + default,
  events with body raw blob, template as raw string, and
  optional style. The parser is intentionally lightweight —
  raw blobs (template body, event body, state field type/
  default) are captured verbatim as strings and re-parsed
  later by the classic Fitz parser during the expand pass.
- **`view/expand.rs`** — converts the raw view AST to
  `ExpandedViewFile` by parsing each raw blob into a classic
  Fitz `Expr` / `Stmt` AST node. Applies template directives
  (`{#if}`, `{#for}`, `{#else}`) to nested nodes. Also runs the
  `<style scoped>` rewriting (CSS class selectors get suffixed
  with a per-component scope class) via
  `view/css_parser.rs`.
- **`view/check.rs`** — the view type checker. Validates state
  field defaults against their declared types, event body
  RHS against state field types, template interpolation
  identifiers against the enclosing scope (with the K-4
  imported-name resolution shipped v0.21.1). Errors are
  `CheckError { message, loc, context }` with the SFC's own
  line/column + a context label naming the offending block
  (e.g. `"component 'App': event 'save' body"`).
- **`view/codegen_ssr.rs`** — emits classic Fitz source for the
  SSR target: a `type <Component>` with the state fields, a
  `<Component>_render(state)` fn producing the HTML, one
  `<Component>_<event>(state, payload)` per event, and a
  `flv_register(...)` at boot (auto-injected via §9.bb). Since
  Phase 11.12 (v0.31.0) a component marked `hydrate` emits the
  **isomorphic render-to-string** the client-WASM hydration adopt
  walk expects: a `<script id="__flv_state_<Comp>">` state payload,
  `<!--fi-->` markers around mixed-context interpolations,
  `<!--fr-->` anchors around `{#if}`/`{#for}` regions, and — for
  composition — a `<div class="__fitz-child-<Name>">` wrapper with
  the parent-provided slot content inlined at the child's `<slot>`
  (threaded as the child render fn's `__slot: Str` arg). The
  `hydrate` marker propagates to the whole tree; a composed child
  suppresses its own state script (its state is re-derived from
  props on the client). Non-hydratable components stay
  byte-identical.
- **`view/codegen_wasm.rs`** — emits Rust source for the WASM
  target: a struct with `RefCell` fields, a `render()` method
  that mutates the DOM via `web-sys`, event handlers that
  update the state cells. Compiles with `wasm-pack`.
- **`view/wasm_build.rs`** — builds the WASM crate scaffolding
  (Cargo.toml + lib.rs) and invokes `wasm-pack build`. Also
  hosts the sibling loaders the standalone crate needs inlined:
  `load_imported_nominals` (classic `type`s), `load_imported_fns`
  (classic helpers), and — since v0.25.0 — `load_imported_components`
  (cross-file `.fitzv` `<Child />` components; parses + expands the
  sibling `.fitzv` so the emitter inlines it and the checker
  validates composition against its real surface). Since v0.26.0 the
  loader honours `as` aliases (registers a renamed clone), and
  `collect_transitive_view_imports` walks the `.fitzv` import graph
  (cycle-safe) so the three loaders run over the transitive union —
  a grandchild in a file the entry does not import directly is still
  discovered. The LSP reuses the same loaders (via
  `lsp::check_view_source_with_base_dir`) so editing a `.fitzv`
  resolves cross-file children too.

**Two emit branches, one check pass**: both the SSR and WASM
emitters consume the same `ExpandedViewFile` — the view checker
runs ONCE regardless of target. The SSR target sees a broader
type surface (compound props, nominal-type interpolated props);
the WASM target has stricter restrictions on prop shapes
(reactive prop propagation is Phase 11.7+ scope).

**Module loader integration** — the classic Fitz module loader
routes `.fitzv` transparent: `from Card import Card` resolves
first to `Card.fitz` (classic wins if both exist), then to
`Card.fitzv` (view path). This makes the migration from classic
to view opt-in and additive.

### format.rs — `FormatSpec` applied to a `Value` (mini-batch Fm)

Implements the full Python-style format-spec semantics over
`Value`: width, alignment, fill, sign, alternate form, grouping
(`,`/`_`), precision, type chars
(`b`/`c`/`d`/`e`/`E`/`f`/`F`/`g`/`G`/`o`/`s`/`x`/`X`/`%`). Entry
point `format_value_with_spec(value, spec) -> Result<String, String>`
with a readable message if the type and spec are incompatible.
Consumed by the `"{x:>10.2f}"` syntax in interpolated strings.

### error.rs — error handling

`FitzError` common to every phase, with `kind`, `line`, `column`,
`message`, optional `hint`. The `ErrorKind` enum covers
lexer/parser/evaluator/checker errors. `impl Display` formats
with the position when it is not `0:0`. Each compiler phase
returns `Result<T, FitzError>` and `main.rs` decides how to
display them based on the sub-command.

## Why this order and not another

**Separating lexer / parser / AST / checker / eval / codegen** is
the classic structure of a compiler, but there are project-specific
decisions worth pointing out:

- **The checker runs before eval and before codegen, not as an
  optional layer.** Strict mode by default in `fitz run` (and
  always in `fitz build`) catches errors early. The
  `--no-typecheck` flag exists to diagnose checker bugs, not for
  end users.

- **The evaluator uses the AST directly, without a typed IR.** It
  is simpler and fast enough for the use case (development
  server, scripts). The codegen also consumes the AST + TypeEnv
  directly. When typed info per node was needed (LSP hover, REPL
  `:type`), it was added as a **span-indexed side-table**
  (`TypeInfo`/`DefinitionInfo` from F16) instead of a formal IR —
  cheaper.

- **`http.rs` is separate from the evaluator** because the
  tokio/axum interaction is complex enough to live apart and so
  the evaluator does not depend on `tokio` for its sync paths.
  After F17 there is no longer a bridge between the interpreter
  and the server; everything runs in a single multi-thread tokio
  runtime. WebSockets (Phase 9.w.2) and cron jobs (Phase 9.w.3)
  live in the SAME registry + runtime, sharing the
  `Arc<HttpRegistry>` — a program with HTTP + WS + cron starts a
  single process with all the subsystems coordinated.

- **The codegen produces a Cargo project, not a single `.rs` +
  `rustc` invocation.** Cross-file imports need `mod`, HTTP
  decorators need external dependencies, and cargo caches
  incremental builds. Known trade-off: the first compilation
  costs ~1-2 s extra vs direct `rustc`.

- **Bit-for-bit parity `fitz run` ↔ `fitz build` is a language
  contract, not implementation detail.** Every feature (HTTP, WS,
  auth, ORM, cron, Python interop, observability) has a path in
  both `evaluator.rs` and `codegen.rs`, and the E2E tests
  (`compile_e2e`) validate that the output matches character by
  character. Divergences (e.g., `fitz build` does not support
  shared state pre-F11) are explicit codegen errors with a message
  citing the future sub-step, not silent fallbacks.

- **Optional features (`python`, `lsp`) are cargo features**, not
  separate binaries or separate crates. Reason: the body of the
  evaluator and codegen keep `#[cfg(feature = "python")]`
  branches to register/serialize `Value::PyObject`. Splitting
  into separate crates would mean re-exposing too much internal
  API. There IS a separate bin for `fitz-lsp`
  (`required-features = ["lsp"]`) so the default `cargo build`
  without features does not drag tower-lsp in.

- **`--bundle-python` produces a launcher, not the binary
  directly** ([launcher_template.rs](../src/launcher_template.rs)).
  Embedding CPython in a standalone binary that does not require
  Python at the destination forces extracting the runtime to a
  cache dir + setting env vars before `exec` — that lives in a
  separate thin Rust binary that wraps the "real" binary emitted
  by the standard codegen. The pattern is the same one Datasette
  uses to distribute Python applications as portable CLIs.

- **Observability is opt-in via env vars, not a language flag.**
  `OTEL_EXPORTER_OTLP_ENDPOINT` absent → `init_otel()` is a
  silent no-op. `RUST_LOG` absent → logs only at `info`+ level.
  `FITZ_PROMETHEUS=1` or `@server(prometheus=true)` enables
  `/metrics`. Trade-off: opting out of automatic HTTP access
  logs requires an explicit `@server(observability=false)`
  (OPEN residual debt: gating of emitted OTel deps when the
  program does not use them — today they are linked whenever
  there are HTTP handlers).

- **Package manager, DX, Docker stack, deploy, and CLI builder
  are sibling modules, not compiler layers**. `manifest.rs`,
  `lockfile.rs`, `git_dep.rs`, `fmt.rs`, `testing.rs`, `lint.rs`,
  `docker.rs`, `deploy.rs`, `cli.rs` operate on the filesystem
  and the AST, but they are not part of the core
  lexer→parser→checker→eval/codegen pipeline. `main.rs` invokes
  them by sub-command; there is no deep coupling between them.

- **The Postgres driver is pure Rust, without libpq.**
  [db.rs](../src/db.rs) implements wire protocol v3.0 +
  SCRAM-SHA-256 + TLS (rustls) + connection pool from scratch,
  without `tokio-postgres`/`sqlx`/`diesel`. Reason: full control
  over the Fitz `Value` ↔ `PgValue` marshaling (including arrays
  with NULLs inside, free-form JSONB vs concrete `Map<Str, T>`,
  Date/DateTime/Uuid as native types), and the promise of "zero
  external deps for intrinsic features". The declarative ORM
  (`@table`/`@primary`/`@column`/`@belongs_to`/`@has_many`) lives
  on top — decorators are processed in `types.rs` populating
  `TableMetadata`, the SQL builder in `evaluator.rs`
  (`.where` closures translated to parametrized SQL) +
  `codegen.rs` (parity). [migrations.rs](../src/migrations.rs)
  consumes the driver for introspect/diff/migrate.
