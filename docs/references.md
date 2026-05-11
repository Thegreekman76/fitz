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
| `axum` | HTTP server (para Fase 4) | https://crates.io/crates/axum |
| `serde` | Serialización JSON | https://crates.io/crates/serde |
| `inkwell` | LLVM bindings (para Fase 5) | https://crates.io/crates/inkwell |
| `cranelift` | Code generation alternativa | https://crates.io/crates/cranelift |
| `miette` | Errores con diagnósticos bonitos | https://crates.io/crates/miette |
| `clap` | CLI argument parsing | https://crates.io/crates/clap |

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
