// lib.rs — Crate library de Fitz.
//
// El crate `fitz` se publica como library + binarios (`fitz`, `fitz-lsp`).
// Los módulos viven acá; los bins (en `src/main.rs` y `src/bin/*.rs`)
// los consumen vía `use fitz::...`.
//
// Refactor introducido en Fase 9.x.1.b: hasta entonces `fitz` era
// bin-only y `src/main.rs` declaraba los módulos directamente. Para que
// el bin nuevo `fitz-lsp` pueda reusar `lexer`/`parser`/`types` sin
// re-compilar todo el crate dos veces, los módulos se mueven a la lib
// y los bins se migran a `use fitz::...`.

pub mod lexer; // Fase 2.1 — tokenización
pub mod ast; // Fase 2.2 — definición del AST
pub mod parser; // Fase 2.3 — construcción del AST
pub mod value; // Fase 2.4 — valores en runtime
pub mod env; // Fase 2.4 — entornos / scopes
pub mod evaluator; // Fase 2.4 — ejecución
pub mod format; // Mini-tanda Fm — aplicación de FormatSpec en runtime
pub mod error; // manejo de errores del compilador
pub mod http; // Fase 4 — HTTP nativo (registry + runtime)
pub mod types; // Fase 5.2 — sistema de tipos resuelto + checker base
pub mod codegen; // Fase 5b.1 — transpile AST → Rust → binario
pub mod openapi; // Fase 7.1 — generador OpenAPI 3.1
pub mod manifest; // Fase 9.y.1 — manifest `fitz.toml` del package manager
pub mod lockfile; // Fase 9.y.3.a — lockfile `fitz.lock` (path deps)
pub mod git_dep; // Fase 9.y.3.c — git deps + cache local
pub mod fmt; // Fase 9.z.1 — formatter `fitz fmt` (pretty-printer del AST)
pub mod testing; // Fase 9.z.2 — testing built-in (registry + @test + asserts)
pub mod lint; // Fase 9.z.5 — `fitz lint` (4 lints + suppression)

// Fase 8.1.2 — interop Python via PyO3 (feature opt-in `python`).
// El módulo envuelve `Python::with_gil` + `py.import(...)` y produce
// `Value::PyObject` para que el evaluator rutee `from python import X`
// al runtime CPython embebido. Sin la feature el módulo no existe;
// `evaluator::load_module` emite error claro citando el flag de build.
#[cfg(feature = "python")]
pub mod py_interop;

// Fase 8.5 — `fitz py-types`: introspecciona modelos SQLAlchemy en un
// archivo Python y emite los `type` Fitz correspondientes a stdout o
// archivo. Comparte el runtime CPython con `py_interop` (in-process,
// no subprocess) para reusar el GIL + dep PyO3 ya disponible.
#[cfg(feature = "python")]
pub mod py_types;

// Fase 9.x.1.b — Lógica del LSP server (pipeline LSP-style + helper
// FitzError → Diagnostic). Vive en la lib (no en `src/bin/fitz-lsp.rs`)
// para que sea unit-testeable. Feature-gated paralelo a `tower-lsp`.
#[cfg(feature = "lsp")]
pub mod lsp;
