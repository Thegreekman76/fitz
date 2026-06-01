<!--
Gracias por mandar este PR. Antes de pedir review, asegurate
de que los checks de abajo estén en verde. CI bloquea PRs que
no pasen fmt / clippy / tests.
-->

## Qué cambia

<!-- Resumen en 1-3 oraciones. Qué hacés y por qué. -->

## Issue relacionado

<!-- "Fixes #N" o "Refs #N" si aplica. Si es bug fix sin issue,
explicá brevemente cómo se reproduce. -->

## Tipo de cambio

- [ ] Bug fix (cambio no-breaking que arregla un issue)
- [ ] Feature nueva (cambio no-breaking que agrega funcionalidad)
- [ ] Breaking change (cambia comportamiento existente del lenguaje)
- [ ] Docs only (sin cambios de código)
- [ ] Refactor / chore / test (sin cambio de comportamiento)

## Áreas tocadas

<!-- Marcá las que apliquen — ayuda al review. -->

- [ ] `lexer.rs` / `parser.rs` (sintaxis)
- [ ] `types.rs` (checker)
- [ ] `evaluator.rs` (intérprete `fitz run`)
- [ ] `codegen.rs` (binario nativo `fitz build`)
- [ ] `http.rs` / `openapi.rs` (HTTP + docs)
- [ ] `db.rs` / `migrations.rs` (Postgres + ORM)
- [ ] `lsp.rs` / `bin/fitz-lsp.rs` (Language Server)
- [ ] `fmt.rs` / `lint.rs` / `testing.rs` (DX)
- [ ] `manifest.rs` / `lockfile.rs` / `git_dep.rs` (package manager)
- [ ] `py_interop.rs` / `py_types.rs` (Interop Python)
- [ ] CI / workflows / `.github/`
- [ ] Docs (`docs/`, `README.md`, `CHANGELOG.md`)
- [ ] Boilerplates / ejemplos

## Paridad `fitz run` ↔ `fitz build`

<!-- Si el cambio toca features user-facing del lenguaje, este
chequeo no es opcional. Si NO toca features del lenguaje (CI,
docs, package manager, etc.), podés saltarlo. -->

- [ ] El feature corre igual con `fitz run` que con `fitz build`
- [ ] Hay test E2E en `tests/compile_e2e.rs` validando paridad
- [ ] No aplica (cambio no afecta el output del programa)

## Checks locales

- [ ] `cargo fmt --all --check` pasa
- [ ] `cargo clippy --all-targets -- -D warnings` pasa
- [ ] `cargo test` pasa
- [ ] `cargo test --features python` pasa (si tocaste interop Python)
- [ ] `cargo test --features lsp` pasa (si tocaste el LSP)

## Docs

<!-- Marcá lo que aplique. Cambios user-facing del lenguaje exigen
actualizar al menos la guía. -->

- [ ] Actualicé `docs/guide.md` y el ejemplo en `examples/guide/`
- [ ] Actualicé `docs/syntax-spec.md` (si introduje sintaxis nueva)
- [ ] Actualicé `docs/architecture.md` (si moví piezas entre módulos)
- [ ] Sumé entrada al `CHANGELOG.md` con versión + descripción
- [ ] No aplica (cambio interno sin impacto en docs)

## Notas para el reviewer

<!-- Decisiones de diseño no triviales, trade-offs evaluados,
alternativas descartadas. Cualquier cosa que ayude a contextualizar
el diff. -->

---

<!-- Al abrir este PR aceptás que tu contribución se distribuya bajo
la licencia MIT del proyecto (ver LICENSE). -->
