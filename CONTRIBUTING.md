# Cómo colaborar con Fitz

Gracias por mirar este archivo — significa que pensás aportar algo al
lenguaje. Esta guía cubre el flujo que usamos hoy: qué hace falta para
arrancar, cómo está armado el compilador, dónde están los buenos
primeros tickets, y qué esperar al abrir un PR.

> Fitz es un proyecto de un solo autor con asistencia de IA (ver
> [README — Cómo se construye](README.md#cómo-se-construye)). El
> proceso colaborativo externo está empezando — agradezco paciencia
> con la fricción que pueda aparecer en este arranque.

---

## Antes de tocar código

### 1. Familiarizate con el contexto

| Documento | Para qué sirve |
|-----------|----------------|
| [README.md](README.md) | Pitch, tabla comparativa, cómo arranca |
| [docs/vision.md](docs/vision.md) | Por qué existe Fitz, para quién |
| [docs/guide.md](docs/guide.md) | Guía del lenguaje (33 caps con ejemplos) |
| [docs/architecture.md](docs/architecture.md) | Pipeline + qué hace cada módulo |
| [docs/roadmap.md](docs/roadmap.md) | Fases cerradas + próximo norte + deudas |
| [docs/syntax-spec.md](docs/syntax-spec.md) | Sintaxis completa (incluye features futuras) |
| [docs/deudas-post-5b.md](docs/deudas-post-5b.md) | Inventario vivo de deuda residual |
| [docs/design-decisions.md](docs/design-decisions.md) | Por qué `Result` y no excepciones, por qué transpile-a-Rust, etc. |

### 2. Hablalo antes (si es algo grande)

- **Bug fix chico o doc fix** → mandá PR directo.
- **Feature nueva, refactor, o algo > ~50 LoC** → abrí un issue primero
  describiendo qué querés hacer y por qué. Alineamos scope antes de
  que invertís tiempo.
- **Cambio de sintaxis o semántica del lenguaje** → siempre issue
  primero. Estas decisiones son del autor y suelen tener contexto que
  no está documentado.

---

## Arquitectura en una imagen

El pipeline de Fitz es clásico de compilador, con bifurcaciones según
subcomando:

```
.fitz → lexer.rs → parser.rs → types.rs (checker) → ┬─ evaluator.rs  (fitz run/test/dev/repl)
                                                    ├─ codegen.rs    (fitz build → Cargo project → binario nativo)
                                                    ├─ openapi.rs    (fitz openapi)
                                                    ├─ fmt.rs        (fitz fmt)
                                                    ├─ lint.rs       (fitz lint)
                                                    └─ migrations.rs + db.rs  (fitz db ...)
```

Detalle por módulo y por qué cada uno está donde está en
[docs/architecture.md](docs/architecture.md).

**Regla de oro del proyecto**: paridad bit-a-bit `fitz run` ↔ `fitz
build`. Toda feature user-facing del lenguaje tiene que producir el
mismo output en ambos caminos. Si tu cambio toca `evaluator.rs`, casi
seguro toca también `codegen.rs` — y al revés.

---

## Setup local

### 1. Clonar y compilar

```bash
git clone https://github.com/Thegreekman76/fitz.git
cd fitz
cargo build --release
```

`rust-toolchain.toml` pinea la versión exacta de Rust — `rustup` la
baja sola. No importa qué Rust tengas instalado globalmente.

### 2. Features opcionales

| Feature | Para qué | Cómo |
|---------|----------|------|
| `python` | Interop con CPython (Fase 8) | `cargo build --release --features python` — requiere Python 3.10+ en el PATH. Detalle en CLAUDE.md → "Setup dev con feature `python`". |
| `lsp` | Compila el bin `fitz-lsp` para la extensión VSCode | `cargo build --release --features lsp` |

### 3. Tests

```bash
cargo test                       # ~2562 unit + 314 compile_e2e + 81 cli_e2e + 3 openapi
cargo test --features python     # añade los tests de interop (~30 más)
cargo test --features lsp        # añade los tests del LSP (~36 más)
```

Para los tests del driver Postgres (`db_real_postgres`) hace falta un
Postgres real corriendo. CI usa el service container `postgres:16` del
workflow. Localmente: cualquier Postgres 15+ accesible apunta el test
suite con la URL en el env var documentada en `tests/db_real_postgres.rs`.

### 4. Checks que CI corre (y bloquean PR)

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Si alguno falla local, falla en CI. Corrélos antes de pushear.

---

## Buenos primeros tickets

Hoy no hay issues abiertos con label `good first issue`, pero la
[deuda residual está inventariada](docs/deudas-post-5b.md) con
evidencia de código línea por línea. Algunos puntos de entrada
chicos:

| Item | Scope | Dónde toca | Doc |
|------|-------|------------|-----|
| Tier C.1 — `ts_rank` full-text ranking en `.order_by` | ~2h | `evaluator.rs` + `codegen.rs` | deudas-post-5b.md §Tier C |
| Tier A.2 — `ALTER COLUMN TYPE` con `USING` automático | ~3h | `migrations.rs:1529` | deudas-post-5b.md §Tier A |
| Tier A.7 — drift check para `@check_constraint` | ~2h | `migrations.rs` introspect | deudas-post-5b.md §Tier A |
| Tier A.8 — introspect cross-schema FK | ~2h | `migrations.rs:65` | deudas-post-5b.md §Tier A |
| Tier D.2 — LSP hover sobre `@table` muestra `CREATE TABLE` | ~2h | `lsp.rs` | deudas-post-5b.md §Tier D |

Mirá [docs/deudas-post-5b.md](docs/deudas-post-5b.md) para la lista
completa por tier (A/C/D/E).

Tareas no-código que también ayudan:

- **Reportar bugs** con un programa Fitz mínimo que reproduzca el
  problema (template de issue lo guía).
- **Mejorar docs** — capítulos de la guía, ejemplos, troubleshooting
  de los boilerplates.
- **Probar boilerplates** en tu plataforma y reportar fricciones.

---

## Workflow esperado

### 1. Branch desde `main`

```bash
git checkout -b feat/<descripcion-corta>
# o fix/, docs/, refactor/, test/, chore/
```

### 2. Trabajá en un commit por unidad lógica

- Mensaje en imperativo, en español (consistente con el repo).
- Prefijo `feat:`/`fix:`/`docs:`/`refactor:`/`test:`/`chore:`/`style:`
  + scope opcional. Mirá `git log --oneline -30` para el patrón.
- Cuerpo opcional explicando el **por qué**, no el qué (el diff
  cuenta el qué).

### 3. Tests primero (o en paralelo)

- **Feature del lenguaje user-facing** → al menos un test E2E en
  `tests/compile_e2e.rs` que valide paridad `fitz run` ↔ `fitz build`.
- **Cambio del checker / evaluator / codegen** → unit tests en el
  módulo (`#[cfg(test)]`).
- **Cambio CLI** → integration test en `tests/cli_e2e.rs`.

### 4. Si toca features del lenguaje, actualizá docs

- **`docs/guide.md`** + ejemplo en `examples/guide/<NN-tema>.fitz`.
- **`docs/syntax-spec.md`** si introducís sintaxis nueva.
- **`docs/architecture.md`** si movés piezas entre módulos.
- **`CHANGELOG.md`** con entrada nueva versionada.

Validá que el smoke `GUIDE_EXAMPLES_COMPILE` (en `tests/compile_e2e.rs`)
siga verde.

### 5. Si toca el lenguaje y afecta la UX del editor

- Verificá grammar TextMate en
  [`editors/vscode/syntaxes/`](editors/vscode/syntaxes/).
- Verificá completions / hover en `src/lsp.rs`.
- Re-build el `.vsix` si es necesario (`cd editors/vscode && npm run
  build:vsix`).

### 6. Antes de abrir el PR

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
git status   # asegurate que no haya .exe/.pdb sueltos en examples/
```

### 7. PR contra `main`

- El template de PR te guía con un checklist.
- Mencioná el issue que cierra (si aplica) con `Fixes #N`.
- Describí el trade-off si tomaste decisiones de diseño no triviales.
- Reviews pueden tardar un poco — el proyecto tiene un solo
  mantenedor.

---

## Convenciones de código

- **Rust idiomático**: si `cargo clippy --all-targets -- -D warnings`
  pasa, vas bien.
- **Nombres en inglés en el código Rust**, comentarios en español OK.
- **Errores con `FitzError`** (módulo `error.rs`) con `line` y `column`
  cuando aplique.
- **Tests unitarios en cada módulo** (`#[cfg(test)] mod tests {}`).
- **Mensajes de error claros para el usuario final del lenguaje** —
  el usuario es alguien que escribe `.fitz`, no alguien que lee el
  source de Fitz. Lenguaje natural antes que jerga.
- **Sin abreviaturas crípticas** en identificadores (`type_env`, no
  `te`; `register_route`, no `reg_rt`).

---

## Estilo de commits

```
feat: agregar `.in_tz(iana)` a DateTime
fix: TLS verify-ca rechazaba certs intermedios válidos
docs: cap 31 — agregar sección sobre composite PKs
refactor: extraer `infer_param_type_from_call_sites` de gen_top_fn
test: cubrir el caso edge de `.preload` con relation typo
chore: bump axum a 0.8.4
```

Cuerpo (opcional, una línea en blanco después del título):

```
feat: agregar `.in_tz(iana)` a DateTime

Cierra Tier B.7 del inventario de deudas-post-5b. Usa chrono-tz
para no agregar deps user-facing. Decisión: `to_local()` queda
con formato fijo ISO 8601 + offset; formato custom es deuda menor.
```

**NO** mencionar a Claude, Anthropic, ni IA en el mensaje del commit
(es preferencia del autor — el SHA es del autor, no del proceso).

---

## Code of Conduct

Este proyecto adhiere al [Código de Conducta](CODE_OF_CONDUCT.md)
basado en el Contributor Covenant 2.1. Al participar (issues, PRs,
discussions) te pedimos respetarlo.

Reportes confidenciales: **palopoli.martin@gmail.com**.

---

## Licencia

Al abrir un PR aceptás que tu contribución se distribuya bajo la
[licencia MIT](LICENSE) del proyecto.

---

## Preguntas frecuentes

**¿Puedo usar IA para escribir el código?**
Sí. El proyecto mismo se construye así (ver
[README — Cómo se construye](README.md#cómo-se-construye)). Lo que
importa es que entendás el código que enviás, que pase los checks
de CI, y que tomes la responsabilidad técnica del PR. Si no entendés
una parte de tu propio cambio, abrí un issue de "estoy pensando hacer
X" antes y conversamos.

**¿Cuánto tarda un PR review?**
Depende del tamaño y del momento. PRs chicos y bien testeados se
revisan en horas o días. Cambios grandes o de diseño pueden tardar
semanas — usá un issue previo para alinear scope.

**¿Cómo hablo con el autor antes de un PR?**
Abrí un issue. Discussions todavía no está habilitado.

**¿El repo acepta breaking changes?**
Hoy sí, Fitz está pre-1.0 y el lenguaje cambia. Cuando un breaking
es necesario lo documentamos en `CHANGELOG.md` con el rationale.
Después de 1.0 va a ser distinto.
