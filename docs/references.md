# Referencias y Recursos

## Para aprender Rust

| Recurso | URL | Notas |
|---------|-----|-------|
| The Book (español) | https://book.rustlang-es.org | Empezar acá |
| Rustlings | https://rustlings.cool | Ejercicios prácticos |
| Rust by Example (español) | https://doc.rust-lang.org/rust-by-example | Cuando querés código directo |
| RustLangES comunidad | https://rustlang-es.org | Roadmap, recursos, Discord |
| Curso de Google (Comprehensive Rust) | https://google.github.io/comprehensive-rust | Para developers con experiencia |

## Para construir el lenguaje

### Libros
- **"Writing An Interpreter In Go"** — Thorsten Ball. El mejor recurso para
  aprender a construir un intérprete. Existe versión Rust de la comunidad.
- **"Crafting Interpreters"** — Robert Nystrom. Disponible gratis en
  https://craftinginterpreters.com. Muy completo, va desde intérprete a VM.

### Crates de Rust útiles

| Crate | Para qué | URL |
|-------|----------|-----|
| `logos` | Lexer generator | https://crates.io/crates/logos |
| `pest` | Parser generator (PEG) | https://crates.io/crates/pest |
| `nom` | Parser combinator | https://crates.io/crates/nom |
| `axum` | HTTP server (Fase 4, en uso) | https://crates.io/crates/axum |
| `tokio` | Async runtime de axum (Fase 4, en uso) | https://crates.io/crates/tokio |
| `serde` + `serde_json` | Serialización JSON (en uso) | https://crates.io/crates/serde |
| `miette` | Errores con diagnósticos bonitos (en uso) | https://crates.io/crates/miette |
| `clap` | CLI argument parsing (en uso) | https://crates.io/crates/clap |
| `inkwell` | LLVM bindings — referencia, **no se usa**: Fitz 5b transpila a Rust en vez de bajar a LLVM directo | https://crates.io/crates/inkwell |
| `cranelift` | Codegen alternativa — referencia, no se usa por la misma razón | https://crates.io/crates/cranelift |
| `syn` + `quote` | AST de Rust (dev-dep: tests del codegen) | https://crates.io/crates/syn |
| `tower` + `http-body-util` | Tests E2E de HTTP sin abrir socket | https://crates.io/crates/tower |
| `tempfile` | Fixtures temporales en tests del loader de módulos | https://crates.io/crates/tempfile |

## Lenguajes para estudiar como referencia

| Lenguaje | Qué tomar | Repo |
|----------|-----------|------|
| **Gleam** | Sintaxis limpia, tipos, mensajes de error | https://github.com/gleam-lang/gleam |
| **Rhai** | Scripting embebible en Rust | https://github.com/rhaiscript/rhai |
| **Mun** | Lenguaje con hot reload, escrito en Rust | https://github.com/mun-lang/mun |
| **Inko** | Concurrencia y tipos, escrito en Rust | https://github.com/inko-lang/inko |
| **Ante** | Tipado gradual, ergonomía | https://github.com/jfecher/ante |

## Comunidades

- **r/ProgrammingLanguages** — comunidad sobre diseño de lenguajes
- **Discord de RustLangES** — comunidad Rust en español
- **Crafting Interpreters Discord** — comunidad del libro
- **Programming Language Discord** — https://discord.gg/4Kjt3ZE

## Inspiraciones de diseño

| Feature de Fitz | Inspirado en |
|----------------|--------------|
| Sintaxis general | Python |
| Tipado gradual | TypeScript |
| `fn`, `match`, `Result` | Rust |
| Decoradores HTTP | FastAPI (Python) |
| Nulabilidad con `?` | Kotlin |
| Compilado, binario único | Go |
| Punto y coma opcional | Go, Kotlin, Swift |
| Sin clases, solo tipos | Rust, Go |
