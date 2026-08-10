// lib.rs — Fitz crate library.
//
// The `fitz` crate ships as a library + binaries (`fitz`, `fitz-lsp`).
// The modules live here; the bins (in `src/main.rs` and `src/bin/*.rs`)
// consume them via `use fitz::...`.
//
// Refactor introduced in Phase 9.x.1.b: until then `fitz` was bin-only
// and `src/main.rs` declared the modules directly. So the new bin
// `fitz-lsp` can reuse `lexer`/`parser`/`types` without recompiling
// the whole crate twice, the modules move into the lib and the bins
// migrate to `use fitz::...`.

// Phase 10.3.b1 — `EvalSignal` is exactly 128 bytes because of the
// shape of `Break(Value, Option<String>)` (Value weighs 104 B).
// Clippy treats anything >= 128 bytes as "Err-variant very large"
// and fires across hundreds of `Result<_, EvalSignal>` signatures.
// Boxing the Value inside Break does not help because
// `Error(FitzError)` already weighs 112 B and keeps the enum size
// near the threshold. A crate-level allow is the standard form in
// Rust projects with large error types that are intentional by
// language design.
#![allow(clippy::result_large_err)]

pub mod ast; // Phase 2.2 — AST definition
pub mod asyncapi; // Phase 9.w.2.d — AsyncAPI 3.0 generator (WebSockets)
pub mod background_jobs; // v0.37.7 — registry + persistence for `@background` jobs
pub mod cli; // Phase 13 (v0.11.0) — native CLI builder (`@command`)
pub mod codegen; // Phase 5b.1 — transpile AST → Rust → binary
pub mod cron_jobs; // Phase 9.w.3 — registry + scheduler for `@cron` jobs
pub mod db; // Phase 10.1 — pure Postgres driver (wire protocol + SCRAM-SHA-256 + OID types)
pub mod deploy; // Phase 12.6 — `fitz deploy <docker|compose>` orchestrator
pub mod docker; // Phase 12.4 — `fitz docker init` (Dockerfile + .dockerignore + docker-compose.yml)
pub mod env; // Phase 2.4 — environments / scopes
pub mod error; // compiler error handling
pub mod evaluator; // Phase 2.4 — execution
pub mod fmt; // Phase 9.z.1 — `fitz fmt` formatter (AST pretty-printer)
pub mod format; // Mini-batch Fm — runtime FormatSpec application
pub mod git_dep; // Phase 9.y.3.c — git deps + local cache
pub mod http; // Phase 4 — native HTTP (registry + runtime)
pub mod http_client; // Mini-fase HTTP client (2026-06-18) — outbound `http.get/...` builtin
pub mod launcher_template; // Phase 8.b.3 — launcher template for `--bundle-python`
pub mod lexer; // Phase 2.1 — tokenisation
pub mod lint;
pub mod lockfile; // Phase 9.y.3.a — `fitz.lock` lockfile (path deps)
pub mod logging; // Phase 12.3.a.2 — built-in structured logging (JSON + pretty + tracing)
pub mod manifest; // Phase 9.y.1 — `fitz.toml` package-manager manifest
pub mod migrations; // Phase 10.6 — automatic ORM migrations (introspect + diff + emit)
pub mod observability; // Phase 12.3.c.1 — OTLP exporter for HTTP spans
pub mod openapi; // Phase 7.1 — OpenAPI 3.1 generator
pub mod parser; // Phase 2.3 — AST construction
pub mod pbs; // Phase 8.b.1 — python-build-standalone download + cache for `--bundle-python`
pub mod pyi_loader; // 8-pyi.B (v0.9.57) — auto pickup of adjacent .pyi stubs
pub mod pyi_stub; // pyi-stubs (v0.9.39) — .pyi parser (PEP 484/561 subset)
pub mod smtp; // Mini-tanda SMTP builtin (2026-06-19) — outbound `smtp.send(opts)`
pub mod templates; // Phase 4 Y-B, Session 1.c — `fitz new --template <name>` scaffolding
pub mod testing; // Phase 9.z.2 — built-in testing (registry + @test + asserts)
pub mod types; // Phase 5.2 — resolved type system + base checker
pub mod value; // Phase 2.4 — runtime values // Phase 9.z.5 — `fitz lint` (4 lints + suppression)
pub mod view; // Phase 11 POC — Single-File Component parser for `.fitzv` files (isolated from classic pipeline)

// Phase 8.1.2 — Python interop via PyO3 (opt-in `python` feature).
// The module wraps `Python::with_gil` + `py.import(...)` and produces
// `Value::PyObject` so the evaluator routes `from python import X` to
// the embedded CPython runtime. Without the feature the module does
// not exist; `evaluator::load_module` emits a clear error citing the
// build flag.
#[cfg(feature = "python")]
pub mod py_interop;

// Phase 8.5 — `fitz py-types`: introspects SQLAlchemy models in a
// Python file and emits the matching Fitz `type`s to stdout or a
// file. Shares the CPython runtime with `py_interop` (in-process,
// not a subprocess) to reuse the GIL + the PyO3 dep already available.
#[cfg(feature = "python")]
pub mod py_types;

// Phase 9.x.1.b — LSP server logic (LSP-style pipeline + FitzError →
// Diagnostic helper). Lives in the lib (not in `src/bin/fitz-lsp.rs`)
// so it can be unit-tested. Feature-gated in parallel to `tower-lsp`.
#[cfg(feature = "lsp")]
pub mod lsp;
