# Roadmap — Fitz

---

## Codegen — B17 (`for x in <List<T>>` con `.await` rompe Send) ✅ CERRADO v0.18.1 (2026-06-20)

**Hito**: cosecha post-fitzwatch sesión 2. Cierra el último bug del codegen que bloqueaba `fitz build` de fitzwatch. **Sin sintaxis nueva** — fix interno transparente para programas existentes.

**El bug**: `for x in <List<T>>` con `.await` adentro del body en handler `async` rompía Send. El codegen emitía `for x in (xs).lock().unwrap().clone().into_iter() { ... }`, donde el `MutexGuard` temporal vivía hasta el final del statement del `for`. Cualquier `.await` adentro del body lo cruzaba, y `axum::Handler` exige `Future + Send` → fallaba.

**El fix**: emitir un bloque acotado con `let __for_snap` previo:

```rust
{
    let __for_snap = (xs).lock().unwrap().clone();
    for mut x in __for_snap.into_iter() {
        // body — el guard YA NO sobrevive cross-await
    }
}
```

El `let __for_snap = ...;` libera el `MutexGuard` al `;`, dejando solo el `Vec<T>` owned. Aplicable a List<T>, List<Tuple> destructuring, Map<K,V> destructuring y Map con wildcard `_`. Cambios en `src/codegen.rs::gen_for_loop` (los 4 sitios del lock chain).

**Verificación pre-bump v0.18.1**:
- `cargo test --lib` → **3171/3171** ✓
- `cargo clippy --lib --tests --bins -- -D warnings` ✓
- `cargo fmt --all --check` ✓
- `cargo test --test compile_e2e for_over_list_with_await_in_body_does_not_break_send_b17` → **1/1** ✓ (test nuevo del bug específico)
- `cargo test --release --test compile_e2e` → **365/368** ✓ (3 fallas pre-existentes documentadas: `hidden_decorator` file lock Windows + `http_coverage` routing 404 + `orm_w17_eager_loaded` codegen drift #7; cero regresiones por el fix B17)
- `smoke_ejemplos_guia_compilables_compilan` → **1/1, ~290 ejemplos guide+curso+TaskHub verdes** ✓ (525s)

**Próximo norte**: retomar **fitzwatch deploy** — TODOS los bugs B1-B17 cerrados.

Detalle release-style en [`CHANGELOG.md`](../CHANGELOG.md) → v0.18.1.

---

## Cosecha codegen post-fitzwatch ✅ CERRADA ENTERA (2026-06-19, [Sin publicar])

**Hito**: 14 de los 15 bugs del codegen descubiertos durante el desarrollo de **fitzwatch** (status page open-source en Fitz puro, pausado el 2026-06-18 esperando estos fixes) cerrados en 6 sub-pasos a lo largo de 2 días. **`fitz check` y `fitz run` nunca estuvieron afectados** — todos los bugs eran específicos del path codegen → cargo build. B13 no reprodujo en v0.17.0 (W18 lo había cerrado); B16 abierto como deuda separada porque el fix vive en el checker (no es mecánico).

| Sub-paso | Bugs | Trigger | Fix |
|---|---|---|---|
| **1** | B3 + B11 | Cross-module `ws_broadcast` + Response con `List<Nominal>` | Detectores unificados (~150 LoC) |
| **2** | B4 + B5 + B6 + meta **B14** | `match` con `return`/`break`/`continue` widening spuriously a `Nullable(T)` | Flow-sensitive refinement filtra arms divergentes del LUB (~80 LoC + 4 unit) |
| **3** | B7 | LUB del `match` queda `Nullable(T)` pero arms no se envuelven en `Some()`/`None` | Post-procesamiento en `gen_match` (~60 LoC + 3 unit) |
| **4** | B8 | `Option<T>: __IntoPgValue` no implementado | Blanket impl en preludio DB (~12 LoC + 3 unit) |
| **5** | B1 + B2 + B9 + B10 + B12 | 5 fixes mecánicos asimétricos | ~375 LoC + 16 tests |
| **6** | **B15** 🔴 | **`.preload()` con FK Nullable** (`@belongs_to ... X_id: Int? = null`) | FK-nullable-aware emission en BelongsToCompanion + HasMany sibling (~115 LoC + 3 unit + 1 E2E) |

**Sub-paso 6 (B15)** — el grande. Dos cambios coordinados en `src/codegen.rs`:

1. `emit_belongs_to_companion_preload_arm` recibe `parent_fields` y, cuando el FK es `Type::Nullable(_)`, emite `filter_map(|p| p.<fk>.map(__FitzPgValue::Int))` para el `IN (...)` + `match __fk { None => None, Some(v) => find(v) }` para el lookup.
2. `emit_preload_dispatch` (path HasMany sibling) detecta FK Nullable del child y emite `__cg2.<fk> == Some(__pid)` en lugar de bare `__pid`.

Hallazgo al diagnosticar: el doc original de B15 decía "nullables en el parent type" como trigger; el trigger REAL es FK Nullable. Los models de fitzwatch declaran todos los FK como `Int = 0` sentinel, así que probablemente fitzwatch estaba pausado por W18 (cerrado en v0.17.0) y no por B15 estricto. Pero B15 sigue siendo crítico: cualquier usuario con FK opcional lo dispara sin workaround user-side.

**Verificación pre-bump (cosecha entera)**:
- `cargo test --release --lib` → **3141/3141** ✓
- `cargo test --release --test compile_e2e cross_module_orm_preload_nullable_fk_b15` → **1/1** ✓
- `cargo test --release --test compile_e2e smoke_ejemplos_guia_compilables_compilan` → **366/366** ✓ (~5 min)
- `cargo fmt --all --check` ✓
- `cargo clippy --release --lib --tests --bins -- -D warnings` ✓

**Próximo norte**: retomar **fitzwatch** — el blocker está cerrado. Si dispara errores nuevos (improbable), se documentan como bugs numerados nuevos. **B16** abierto como deuda separada (match arms `i64` vs `()` E0308 cuando uno es call que retorna `Null`; fix sugerido en el checker).

Plan completo + tabla de estado con todos los bugs en [`docs/deudas-post-5b.md`](deudas-post-5b.md) → "🟢 Cosecha codegen post-fitzwatch CERRADA ENTERA 2026-06-19". Detalle release-style en [`CHANGELOG.md`](../CHANGELOG.md) → "Sin publicar".

---

## Mini-tanda — SMTP builtin ✅ CERRADA v0.18.0 (2026-06-19)

**Hito**: el módulo `smtp` builtin (cliente SMTP outbound) entra al lenguaje como ciudadano de primera clase, paralelo bit-a-bit al HTTP client de v0.17.0. **Cierra la otra brecha chica del stack web outbound** — HTTP client + SMTP cubren los dos casos canónicos: cliente para APIs externas + cliente para email. **Detectada como deuda explícita** el 2026-06-18 durante el desarrollo de fitzwatch (que necesita despachar emails de incidents y workaroundeaba via webhook + n8n/zapier).

**API**: una sola función — `smtp.send(opts: Map) -> Future<Result<SmtpResult>>`.
- Required keys: `to`/`subject` (Str).
- At least one of: `body`/`body_text`/`body_html` (Str). `body` solo es alias de `body_text` para el caso 90%.
- Optional: `from` (Str — opcional si `SMTP_FROM` env var está seteada).
- Texto plano + HTML juntos → multipart/alternative automático.
- Tipo built-in nuevo `SmtpResult { delivered: Bool, message_id: Str, duration_ms: Int }` pre-registrado en `TypeEnv`.

**Configuración via env vars** (al boot):
- `SMTP_HOST` (required), `SMTP_PORT` (default según TLS: 587/465/25).
- `SMTP_USER` + `SMTP_PASSWORD` (juntos o ninguno, error claro si desbalanceado).
- `SMTP_FROM` (default `From`).
- `SMTP_TLS` (`starttls`/`implicit`/`none`, aliases `tls`/`ssl`/`plain`).

**Modelo de errores**: `Result::Err(Str)` con prefijo `"smtp: "` para fallos de transporte (DNS, conexión, auth, TLS handshake, server reject, address parse, missing config). Status 5xx del SMTP server cuenta como Err — lettre solo acepta el relay con response 250.

**Backend**: `lettre = "0.11"` con features `tokio1-rustls-tls` + `smtp-transport` + `pool` + `builder` + `hostname`. Linkeado estático sin openssl. Connection pool reusa TCP/TLS entre sends.

**5 diferenciales** (ver CHANGELOG v0.18.0 + cap 17 de la guía sub-sección "SMTP outbound"):
1. Built-in del lenguaje — no `pip install yagmail` / `npm install nodemailer` / `cargo add lettre` / Maven `JavaMail`. Solo Go tiene `net/smtp` stdlib pero es low-level.
2. Paridad bit-a-bit `fitz run` ↔ `fitz build` — `lettre` linkeado estático con `tokio1-rustls-tls`, sin openssl en el host.
3. Async ciudadano de primera (`@cron` digests/reportes, `@background`+`spawn` welcome emails, handlers HTTP magic-link auth).
4. `Result<T>` automático — `?` propaga, checker exige manejo (regla 5.3.3).
5. Sin deps externas en el host — el binario standalone NO requiere SMTP libs del SO.

**8 sub-bloques cerrados**: B1 evaluator (`src/smtp.rs` nuevo ~520 LoC + 10 unit tests) + B2 checker (pre-registro + 6 unit tests) + B3 codegen (`SMTP_PRELUDE` ~280 LoC + dispatch `gen_smtp_call` + 11 unit tests + 24 call sites de `cargo_toml_for` actualizados) + B4 LSP (scope_level + after_dot completions + 3 unit tests) + B5 guía+ejemplos (sub-sección nueva en cap 17 + 3 ejemplos runnable `17i`/`17j`/`17k` contra MailHog) + B6 cross-docs + B7 boilerplate + B8 cierre formal v0.18.0.

**Verificación pre-bump**: 3171 unit lib (+30 vs cosecha: 10 smtp + 6 checker + 11 codegen + 3 LSP) + smoke `GUIDE_EXAMPLES_COMPILE` verde 369 ejemplos en 282.28s (+3 SMTP nuevos) + build manual de `17i`/`17j`/`17k` verde a binario nativo.

Plan completo + decisiones técnicas en [`docs/deudas-post-5b.md`](deudas-post-5b.md) → "🟢 SMTP builtin — CERRADA v0.18.0". Detalle release-style en [`CHANGELOG.md`](../CHANGELOG.md) → v0.18.0.

---

## Mini-tanda — HTTP client builtin ✅ CERRADA v0.17.0 (2026-06-18)

**Hito**: el módulo `http` builtin (cliente HTTP outbound) entra al lenguaje como ciudadano de primera clase, paralelo al HTTP server-side cerrado en Fase 4. 9 bloques cerrados en una sesión + fix coordinado W18 para destrabar el codegen cross-module observability descubierto al validar B8 sobre `boilerplates/api-orm-full` multi-archivo.

**API completa día 1**:
- `http.get/head/post/put/delete/request` — los 6 builtins async devuelven `Future<Result<HttpClientResponse>>`.
- Body shapes: `Str` (as-is) / `Map<Str, Any>` (auto-JSON + `Content-Type: application/json`) / `Bytes` (as-is octetos).
- Tipo built-in nuevo `HttpClientResponse { status, body, headers, duration_ms }` paralelo a `Request`/`Response` del HTTP server-side.
- Errores: `Result::Err(Str)` con mensajes claros; status 4xx/5xx NO son Err (mirar `r.status`).
- Backend: `reqwest = "0.12"` features `["json", "rustls-tls"]` linkeado estático.

**5 diferenciales** (ver CHANGELOG v0.17.0 + cap 17 de la guía):
1. Built-in del lenguaje — no `pip install requests` / `npm install axios` / `cargo add reqwest`.
2. Paridad bit-a-bit `fitz run` ↔ `fitz build`.
3. Async ciudadano de primera (`@cron`/`@background`/handlers HTTP/`spawn(...)`).
4. `Result<T>` automático — `?` propaga, checker exige manejo.
5. Sin deps externas en el host — `rustls-tls` no exige openssl.

**Bloques cerrados** (un commit por bloque): B1 evaluator (`3cefd2e`) + B2 checker (`d1aaf70`) + B3 codegen (`9e214e3`) + B4 LSP (`29bc041`) + B5 guía+ejemplos (`03cd71e`) + B6 cross-docs (`931fd18`) + B7 curso M5.C5 (`b499c36`) + B8 boilerplate api-orm-full webhook (`1468e0d`) + W18 codegen cross-module observability fix (`63b3d3f`) + B9 cierre formal v0.17.0.

**W18 — Fix codegen cross-module observability**: cierra la deuda más visible del codegen heredada de Fase 12.3.b. Sin W18, programas multi-archivo donde un módulo importado declaraba `@get`/`@post`/etc y/o llamaba `log.X(...)` rompían `fitz build` con 12-134+ errores rustc. 3 sub-cambios coordinados en `src/codegen.rs`: imports observability del wrapper HTTP en módulos + imports de log helpers paralelo a uses_logging + propagación de `@server(observability=false)` main → módulos via nuevo helper `extract_main_observability_enabled` + threading via `ModuleLoader.main_observability_enabled`. 9 unit tests nuevos. Detalle completo en `docs/deudas-post-5b.md` → "🟢 W18".

**Verificación pre-bump completa**: 3116 unit lib + smoke 363 ejemplos verde + fmt + clippy (default + lsp) limpios + `boilerplates/api-orm-full` con webhook outbound compila a binario nativo end-to-end (134 errores rustc pre-W18 → 0 post-W18) + arranca + `/healthz` y `/readyz` responden 200 + validación bit-a-bit `fitz run` ↔ binario sobre `17e` contra `httpbin.org`. Extensión VSCode bumpeada a 0.17.0 con `.vsix` regenerado.

**Próximo norte**: **retomar fitzwatch** (status page open-source en Fitz puro, pausado el 2026-06-18 por falta de `http.head` builtin). El blocker está cerrado.

Plan completo + tabla de estado con SHAs por bloque en [`docs/http-client-roadmap.md`](http-client-roadmap.md). Detalle release-style en [`CHANGELOG.md`](../CHANGELOG.md) → v0.17.0.

---

## Mini-tanda — Traducción ES→EN del código del compilador ✅ CERRADA v0.16.0 (2026-06-15)

**Hito de internacionalización**: tras 47 commits coordinados en 7 sub-fases (F1-F6 + F5.d), el surface user-facing del compilador (CLI, LSP, errors), los test assertions internas y los comentarios del grammar TextMate quedaron 100% en inglés. `grep "esperaba|esperaban" src/` → 0 ocurrencias post-cierre.

**NO cambia el comportamiento del lenguaje** — AST, checker, runtime, codegen, ORM, package manager y LSP funcionan bit-a-bit idénticos. Release cosmética + i18n, pero cross-cutting (47 commits) y visible al usuario en mensajes de error CLI, hints LSP, descripciones de completion.

**Sub-fases ejecutadas**:
- F1: Comments en `src/**/*.rs` (17 batches)
- F2: Test function names (~2647 renames en 44 archivos)
- F3: Error messages + runtime emit strings (10 batches)
- F4: Output user-facing — CLI, LSP, examples, docs internacionales (syntax-spec, architecture)
- F5.a/b/c: Test assertions internas + barrida cross-archivo + F4 leftover + driver Postgres TLS
- F5.d: Residual "esperaba"/"esperaban" — 253 strings en `mod tests` de 11 archivos + 1 user-facing en `src/db.rs:535` que F5.c.2 había perdido
- F6: Comentarios del grammar TextMate (`editors/vscode/syntaxes/fitz.tmLanguage.json`)

**Verificación pre-bump completa**: 3052 unit (sin feature) / 3170 (lsp) / 3143 (python) + 352 compile_e2e + 1 smoke gigante (290 ejemplos guía/curso/TaskHub) + 98 cli_e2e + 3 openapi + 3 builds release (default + lsp + python) + clippy strict en 3 modos + fmt → todos verdes. 8 failures de compile_e2e son pre-existentes documentados, cero regresiones de F1-F6+F5.d.

**NO cubierto** (deuda explícita): `docs/guide.md`, `docs/curso/`, `docs/taskhub/`, `README.md`, `docs/index.md` — material pedagógico se mantiene en castellano por decisión de proyecto. Traducción a inglés queda como sub-paso futuro si el material gana tracción internacional.

Detalle por sub-paso en [`CHANGELOG.md`](../CHANGELOG.md) → v0.16.0.

---

## Estado actual del proyecto (v0.9.57 — 2026-05-24)

**Hito**: tras 15 releases consecutivos cerrando deudas (v0.9.43 → v0.9.57), el inventario activo queda **vacío**. El proyecto está en estado **production-ready** para todos los patrones canónicos del lenguaje:

- **Fases 1-9 entera CERRADAS**: lexer + parser + AST + checker estático + evaluador async + HTTP nativo + middleware + Result/match + módulos + interop Python con bundling distroless + LSP MVP completo + package manager + DX (fmt/test/dev/repl/lint) + stack web first-class (auth/WS/jobs).
- **Cierre Bundle B/I (Python interop codegen)**: 3 deudas residuales cerradas (8.7-ok-propagation, 8.7-await-binding-split, dict→Map<K,V> primitivo). Codegen Python production-ready para el caso 90%+.
- **8-pyi-stubs CERRADO entero** (v0.9.57): auto-pickup loader detecta `.pyi` adyacente al `.fitz` raíz, registra nominales en TypeEnv (pase 1) + crea nominal sintético por módulo con field tipado por cada fn/var (pase 2). Field access `api.fetch_user(42)` ahora tipa estáticamente como `Result<User>` con arity + type check de args. Cubre el `Type::PyModule` que faltaba sin tocar la signature de `check_program` (vía `TypeEnv.pyi_modules`).
- **6 boilerplates Dockerizados validados end-to-end**, los 2 con Postgres+Python en variante distroless real (imagen ~136 MB).
- **CI strict reactivado**: `cargo fmt --check` + `cargo clippy --all-targets -D warnings` en los 3 modos (default, `python`, `lsp`).
- **R.bug-pyo3-abi3-portable-link Linux/macOS RECLASIFICADO** (v0.9.56) como constraint arquitectural permanente.
- **Race condition Windows en compile_e2e fixeado** (v0.9.57): helpers usan stem único per test → cero choque de file handles entre runs secuenciales.
- **2304 unit (default) / 2395 lsp** + 90+ E2E + 79+ compile_e2e.

**Deudas activas cerrables**: **ninguna**. El inventario está vacío después de v0.9.57.

**Constraints arquitecturales documentados** (no cerrables):

- R.bug-pyo3-abi3-portable-link Linux/macOS — bundling Python requiere builder con versión específica de Python (PyO3 + Linux/glibc constraint).
- Métodos custom dentro de `class` del stub (`def method(self, ...)`) — el parser MVP los ignora; refinable si entra demanda real.
- Lookup `.pyi` solo adyacente — decisión consciente (NO PYTHONPATH/site-packages) por reproducibilidad. Opt-in futuro si entra demanda.

**Fase 10 — Stack DB nativo + ORM declarativo: CERRADA + Tier S + cierre masivo v0.10.29**. Driver Postgres puro Fitz (sin libpq) + ORM declarativo sobre `type` + paridad bit-a-bit `fitz run` ↔ `fitz build` + 10 sub-comandos `fitz db ...` + Tier S de observabilidad + cierre masivo v0.10.29 con 12 features residuales cerradas. Detalle por release en `CHANGELOG.md`.

**Estado al 2026-05-31 (post-v0.10.29)**:

- **Fase 10 entera (10.1 → 10.10)** + **Fase 10.b** (paridad codegen ORM) + **Tier S** (v0.10.27 bulk_insert + composite PK + @index, v0.10.28 fitz db inspect + @index using + FITZ_DB_LOG/HTTP_LOG) cerrados.
- **v0.10.29 — Cierre masivo del ORM**: 12 features en bloque (JSON path operators + @@ text search + @unique composite + @check_constraint + cross-schema FK + diff completo de indexes + fitz db inspect --all-schemas + redaction secrets en FITZ_DB_LOG + DB errors con SQLSTATE+SQL+params + FITZ_DB_MAX_CONNS + skip deliberado JSON || merge + docs masivos).
- **Tests al cierre**: 2739 unit + 292 smoke + 3 openapi + 81 cli_e2e + 52 db_real_postgres. fmt + clippy --all-targets + clippy --features lsp limpios.

**Próximo norte (post-v0.10.29)**: tres opciones priorizadas por scope. Detalle en [`docs/deudas-post-5b.md`](deudas-post-5b.md) sección **"Deuda residual del ORM/DB post-v0.10.29"** (29 ítems agrupados en Tier A-E):

1. **Tier A + B** (~50h): cierre del MVP fuerte del ORM (pre-flight destructive, ALTER USING auto, SAVEPOINT/nested tx, ADD CONSTRAINT CHECKs, drift checks @check + cross-schema FK, isolation level, FITZ_DB_* mid-run reload) + completion API Date/DateTime/Uuid (add_days/diff/comparison operators/Uuid.v7/timezone). Recomendado si el norte es "ORM completo sin fricciones".
2. **Tier C + D** (~17h): operadores SQL faltantes (ts_rank, expression indexes, JSON || merge) + DX/LSP (completion ORM methods en .where, hover @table → CREATE TABLE).
3. **Tier E** (días-semanas, expansión del lenguaje): Decimal/Numeric, async streaming cursor-based, COPY FROM/TO, LISTEN/NOTIFY tipado, window functions, CTE/WITH, UNION/INTERSECT/EXCEPT.

Alternativas no-ORM: nuevos boilerplates Dockerizados, contenido educativo (curso `Fitz de 0 a experto`), **Fase 11 (frontend en `.fitz`)** — única fase grande pendiente del roadmap, V6 (DAP — debugging interactivo en VSCode), deudas residuales menores del LSP/checker. Fases 12 (deployment) y 13 (CLI builder) ya están cerradas (v0.12.5+v0.13.0 y v0.11.0 respectivamente).

Detalle exhaustivo de cada cierre en [`CHANGELOG.md`](../CHANGELOG.md) y deudas residuales en [`docs/deudas-post-5b.md`](deudas-post-5b.md).

---

## Fase 1 — Aprender Rust 🦀
**Estado: COMPLETADA**

Antes de escribir el compilador, dominar las herramientas.

### Objetivos
- [x] The Book capítulos 1-10 (rustlang-es.org)
- [x] Rustlings — ejercicios básicos completos
- [x] Entender ownership, borrowing y lifetimes
- [x] Entender enums y pattern matching
- [x] Primer proyecto Rust propio (pequeño)

### Recursos
- https://book.rustlang-es.org
- https://rustlings.cool
- https://doc.rust-lang.org/rust-by-example

### Criterio de completitud
Poder escribir un lexer básico en Rust sin consultar el libro en cada línea.

---

## Fase 2 — Intérprete base 🔬
**Estado: COMPLETADA**

El corazón del lenguaje. Al final de esta fase, Fitz puede ejecutar
programas básicos.

El criterio de éxito se cumple: `cargo run -- run examples/phase2.fitz`
ejecuta el programa de referencia end-to-end (270 tests pasando, incluida
la deuda accionable de 2.3/2.4 cerrada).

Tras cerrar la fase se publicó **docs/guide.md v0.1**: guía pedagógica
en español, 13 capítulos, 11 ejemplos ejecutables en `examples/guide/`.
La guía solo documenta lo que el intérprete ejecuta hoy; crece con
cada feature que se cierre. **Regla operativa**: cualquier cambio al
proyecto exige verificar la guía y sus ejemplos antes de declarar el
trabajo cerrado.

### Módulos a implementar

#### 2.1 Lexer ✓
**Completado** — `src/lexer.rs` con 16 tests pasando.
Convierte texto fuente en tokens.

```
"let x = 42 + 1"
→ [Let, Ident("x"), Eq, Int(42), Plus, Int(1)]
```

Tokens necesarios:
- Literales: Int, Float, Str, Bool, Null
- Operadores: +, -, *, /, ==, !=, <, >, <=, >=, =>, ?
- Delimitadores: (, ), {, }, [, ], ,, :, .
- Keywords: fn, async, return, if, else, for, while, match, let, type, import, from, true, false, null
- Decoradores: @get, @post, @put, @delete, @server
- Identificadores y comentarios

#### 2.2 AST (Abstract Syntax Tree) ✓
**Completado** — `src/ast.rs` con 3 tests pasando. Soporta el programa del criterio de éxito de Fase 2 (incluye `StrInterp` para interpolación).
Define las estructuras de datos que representan el programa.

```rust
enum Expr {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
    Ident(String),
    BinOp { op: Op, left: Box<Expr>, right: Box<Expr> },
    Call { name: String, args: Vec<Expr> },
    // ...
}

enum Stmt {
    Let { name: String, value: Expr },
    Return(Expr),
    If { cond: Expr, then: Block, else_: Option<Block> },
    // ...
}
```

#### 2.3 Parser ✓
**Completado** — `src/parser.rs` con 111 tests pasando. Convierte tokens
en AST mediante recursive descent. El criterio de éxito de Fase 2 parsea
end-to-end (lexer → parser → AST).

```
[Let, Ident("x"), Eq, Int(42), Plus, Int(1)]
→ Let { name: "x", value: BinOp { op: Add, left: Int(42), right: Int(1) } }
```

**Alcance de 2.3 (lo que SÍ se implementa):**
- Expresiones: literales, identificadores, paréntesis, operadores aritméticos
  (`+`, `-`, `*`, `/`), comparación (`<`, `>`, `<=`, `>=`), igualdad
  (`==`, `!=`), unario `-`.
- Postfix: field access (`user.name`), llamadas a función (`f(args)`) con
  nombre simple.
- `StrInterp`: parsing del contenido de `Token::Str` para detectar `{...}`.
- Sentencias: `let`/asignación, `return`, expr-statement, `fn` (forma de
  bloque y de flecha), `type`, `if`/`else` (como sentencia o expresión),
  `match`, `break`, `continue`.
- Decoradores HTTP: `@get`/`@post`/`@put`/`@delete` envolviendo una `FnDef`.

**Fuera de alcance — deuda explícita, retomar después:**
- ~~**Operadores lógicos `and` / `or`**~~ ✓ cerrado tras Fase 2 — tokens
  emitidos por el lexer, precedencia en parser (`or` < `and` < `==`),
  short-circuit ya estaba implementado en el evaluador.
- ~~**`while` / `loop`**~~ ✓ cerrado tras Fase 2 — AST tiene las variantes,
  parser y evaluador funcionan, `break`/`continue` se capturan dentro del
  loop.
- **`for`** — queda como deuda. Necesita rangos (`0..10`) o listas como
  fuente de iteración; espera Fase 3. El parser emite error explícito.
- **Method calls (`expr.method(args)`)** — `Expr::Call` solo admite
  `name: String`. Cuando mute a `callee: Box<Expr>` se desbloquea. Por
  ahora, el parser tira error explícito.
- **Asignación a campos (`user.name = ...`)** — `Stmt::Assign` solo admite
  identificador como destino. Retomar cuando definamos mutabilidad.
- ~~**Posición de errores en subexpresiones de interpolación**~~ ✓ mejorado
  tras Fase 2 — ahora apunta al `{` específico y traslada errores del
  sub-parser. Limitación residual: un char menos de precisión por cada
  escape (`\n`, `\t`) anterior al error, porque no tenemos el source
  original.
- ~~**`return` sin expresión**~~ ✓ cerrado tras Fase 2 — `return` solo
  equivale a `return null` implícito.
- **Patrones de match** — soporta `Int`, `Float`, `Str`, `Bool`, `Null`
  (con negativos), `Ident`, `_`, y `Ok(x)`/`Err(e)` (estos últimos parsean
  pero el evaluador emite error hasta Fase 3). Faltan rangos (`0..12`),
  tuples y listas.
- **Struct literals (`User { id: 1, name: "x" }`)** — el AST no los
  modela. Por ahora la instanciación tiene que hacerse vía función
  constructora.
- **Listas y mapas literales (`[]`, `[1,2]`, `{"k": v}`)** — ni el lexer
  los reconoce especialmente ni el AST tiene `Expr::List`/`Expr::Map`.
- **Tipos compuestos en anotaciones (`List<T>`, `Map<K,V>`, `Str?`)** —
  `Stmt::Assign.type_` y `Param.type_` son `Option<String>`, solo
  nombres simples. (El `?` post-tipo SÍ se modela en campos de `type`
  vía `Field.nullable`, pero no en anotaciones de variables/parámetros.)
- **Error recovery** — el primer error mata el parseo. Sin paniqueo y
  resincronización todavía.

#### 2.4 Evaluador ✓
**Completado** — `src/evaluator.rs`, `src/value.rs`, `src/env.rs` con 113
tests pasando (92 evaluador + 12 value + 9 env). Recorre el AST y ejecuta
el programa.

**Alcance de 2.4 (lo que SÍ se implementa):**
- Valores en runtime: `Int`, `Float`, `Str`, `Bool`, `Null`, `Function`
  (con closures), `Builtin`, `Type` (inerte hasta Fase 3).
- Operaciones: aritmética con promoción Int↔Float, comparación numérica
  y de strings, igualdad con coerción, `and`/`or` con short-circuit,
  unario `-`. División por cero → error explícito.
- Strings: concatenación con `+`, interpolación de expresiones con `{...}`.
- Control de flujo: `if`/`else`/`else if` como expresión y como sentencia.
- Funciones: `fn`/`=>`, closures con captura léxica, recursión, validación
  de aridad, `return` propagado vía `EvalSignal::Return`.
- `match` con patrones `Ident` (bind) y `Wildcard`.
- `type` registrado en el env como marcador inerte.
- Builtins: `print` (sigue la semántica de Python — args separados por
  espacio, newline final).
- Manejo unificado de errores y control de flujo vía `EvalSignal`
  (`Error` / `Return` / `Break` / `Continue`). Signals "huérfanos" (return
  fuera de función, break/continue fuera de loop) se reportan al usuario.

**Fuera de alcance — deuda explícita, retomar después:**
- ~~**Operadores `and`/`or`**~~ ✓ cerrado tras Fase 2 — lexer emite tokens,
  parser inserta en la cadena de precedencia, evaluador con short-circuit.
- ~~**`break` / `continue`** sin loops~~ ✓ cerrado tras Fase 2 —
  `while`/`loop` los capturan correctamente vía `run_loop_body`.
- **Patrones `Ok(x)` / `Err(e)`** — el evaluador emite error explícito
  citando "requiere el tipo Result (Fase 3)". Bloqueado hasta tener
  `Value::Result` o similar.
- **Field access (`obj.campo`)** — error explícito "requiere tipos custom
  instanciados (Fase 3)" porque sin struct literals no hay struct values
  en runtime.
- **Instanciación de tipos (`User { id: 1, name: "x" }`)** — el AST no
  los modela. `Value::Type` está listo para recibir instancias cuando se
  agregue.
- **HTTP endpoints (`@get`, etc.)** — Fase 4. El evaluador devuelve error
  explícito si se evalúan.
- **Async** — `is_async` en `FnDef` se ignora silenciosamente. Fase 4.
- **Anotaciones de tipo** — `let x: Int = ...` parsea, pero el evaluador
  ignora la anotación. El tipado gradual sin checks runtime es la
  intención; el type checker estático llega en Fase 5.
- **Scope de bloques (if/match/función)** — los bloques de `if` no crean
  scope nuevo (estilo Python). Variables definidas adentro persisten
  afuera. Si esto trae sorpresas, revisamos.
- **Overflow numérico** — `Int + Int` puede overflowear (paniquea en
  debug, wrappea en release). Sin `checked_*` por ahora.

### Criterio de completitud
Este programa funciona:
```fitz
name = "Fitz"
x = 10 + 5
print("Hola {name}, x es {x}")

fn double(n) => n * 2
print(double(x))
```

---

## Fase 3 — El lenguaje crece 🌱
**Estado: COMPLETADA**

Agregar las features que hacen a Fitz expresivo. La fase está dividida
en cinco pasos; cada uno cierra una pieza independiente y suma su
capítulo a la guía.

### Pasos

#### 3.1 Listas, mapas, rangos ✓
**Completado** — el lenguaje ya tiene colecciones básicas y `for`.

- AST nuevo: `Expr::List`, `Expr::Map`, `Expr::Range`, `Expr::Index`,
  `Stmt::For`, `Pattern::Range`.
- Parser: literales `[...]` y `{...}`, rangos `start..end` con
  precedencia entre comparación y suma, indexing postfix `xs[i]`,
  `for var in iter { ... }`, patrón de rango `0..10` en match.
- Evaluator: `Value::List`, `Value::Map`, `Value::Range`; iteración
  para `for` sobre listas y rangos; matching de rango contra Int;
  errores explícitos para índices fuera de rango, claves no
  encontradas, tipos no indexables.
- Builtin: `len` para List/Map/Str/Range.

Tests del proyecto al cerrar 3.1: 366 (270 al cerrar Fase 2 + 96
nuevos repartidos entre ast, parser, value y evaluator).

Guía: capítulo 9 "Listas, mapas y rangos" sumado, capítulo de Match
extendido con patrones de rango, capítulo de Loops limpiado de la
deuda de `for`. Ejemplo nuevo: `examples/guide/09-listas-mapas.fitz`.

**Deuda explícita — retomar después:**
- **Mutación de listas** (`push`/`pop`/asignación a `xs[i]`) — espera
  3.4 (method calls). Por ahora las listas son inmutables desde el
  código fuente.
- **`for` sobre mapas** — necesita el tipo `Pair`/`entry`. El
  evaluador emite error explícito por ahora.
- **Índices negativos** (`xs[-1]` estilo Python) — sin soporte; el
  evaluador corta con "índice negativo".
- **`Range` indexable** (`(0..10)[3]`) — no soportado, sin uso claro
  hasta no tener method calls.
- **`Str` indexable** — pendiente decisión sobre la unidad (char vs
  byte vs grafema).
- **Rango inclusivo `..=`** — sin soporte, se suma si aparece la
  necesidad.

#### 3.2 Tipos custom instanciables ✓
**Completado** — los tipos declarados con `type` ahora se pueden
instanciar y consultar.

- AST nuevo: `Expr::StructLit { type_name, fields }`.
- Parser: `Nombre { campo: expr, ... }` como expresión, en cualquier
  posición salvo las condiciones directas de `if`/`while`/`for`/`match`
  (donde el `{` arranca un bloque). En esas posiciones, el flag
  `no_struct_literal` corta con un error explícito sugiriendo
  paréntesis. Adentro de `(...)`, `[...]`, args de llamada e
  indexing, los struct literals están permitidos sin envolver.
- Evaluator: `Value::Instance { type_name, fields }`. Al instanciar
  se valida campo extra → error, falta de campo sin default ni
  nullable → error; se aplican defaults (evaluados en el env de
  instanciación) y nullables (`Null` por omisión); los campos
  quedan ordenados según la declaración del `type`. Field access
  (`obj.campo`) implementado sobre `Value::Instance`. Igualdad
  estructural (mismo tipo, mismos campos en orden, coerción
  Int↔Float adentro).
- Cierra deuda de 2.3 (struct literals no parseaban) y de 2.4 (field
  access e instanciación en runtime).
- Capítulo 12 de la guía sale del estado "preview" y pasa a documentar
  el feature real; se sumó al ejemplo
  `examples/guide/12-type.fitz` la parte de instanciación y acceso a
  campos.

Tests del proyecto al cerrar 3.2: 405 (366 al cerrar 3.1 + 39 nuevos
repartidos entre ast, parser, value y evaluator).

**Deuda explícita — retomar después:**
- **Mutación de campos** (`user.name = "x"`) — espera 3.4
  (asignación a destinos no-identificador). Hoy `Stmt::Assign.name`
  es `String`; cuando mute a un destino más rico se desbloquea.
- **Métodos sobre instancias** (`user.greet()`) — espera 3.4
  (mutación de `Expr::Call` a `callee: Box<Expr>`).
- **Chequeo de tipos en runtime** — descartado por diseño (tipado
  gradual). Las anotaciones se guardan pero no se validan en
  runtime; el chequeo estático llega con el compilador en Fase 5.
- **Tipos compuestos en anotaciones de campo** (`emails: List<Str>`)
  — sigue siendo deuda de 2.3 (`Field.type_` es `String` simple).

#### 3.3 Result + Ok/Err + `?` ✓
**Completado** — el lenguaje maneja errores estilo Rust, sin
excepciones.

- AST nuevo: `Expr::Ok(Box<Expr>)`, `Expr::Err(Box<Expr>)`,
  `Expr::Try(Box<Expr>)` (operador `?` postfix).
- Parser: `Ok` y `Err` se detectan como keywords contextuales
  cuando aparecen como receptor de llamada (aridad 1 obligatoria);
  `?` se parsea en la cadena de postfix junto a `.`, `(...)`,
  `[...]`, encadenable con field access (`get(id)?.name`).
- Value: variante propia `Value::Result(ResultVariant)`, con
  `ResultVariant::Ok(Box<Value>)` y `ResultVariant::Err(Box<Value>)`.
  Display: `Ok(v)` / `Err(e)`, strings con comillas adentro (mismo
  criterio que List/Map/Instance). Igualdad estructural con la
  coerción Int↔Float recursiva.
- Evaluator: `Ok`/`Err` envuelven el inner evaluado. `?` desempaqueta
  cuando es `Ok`, y emite `EvalSignal::Return(Value::Result(Err))`
  cuando es `Err`, reusando la maquinaria existente de `return`.
  Sobre un valor que no es `Result`, `?` corta con error de tipo.
- Patrones `Ok(x)`/`Err(e)` en `match` ahora matchean contra
  `Value::Result` y bindean el inner — cierra deuda explícita de
  2.4.

Tests del proyecto al cerrar 3.3: 441 (405 al cerrar 3.2 + 36
nuevos repartidos entre ast, parser, value y evaluator —
incluyendo dos tests end-to-end con `find_user` y `divide` desde
fuente).

Guía: capítulo 13 nuevo "Result y manejo de errores" entre el cap
de Tipos y el de Errores del intérprete. Renumeración: cap 13
(Errores y mensajes) → 14; cap 14 (Qué sigue) → 15. Ejemplo nuevo
`examples/guide/12-result.fitz`; el antiguo `12-errores.fitz`
renombró a `13-errores.fitz`.

**Deuda explícita — retomar después:**
- **Mensaje específico para `?` fuera de función** — hoy reutiliza
  el signal de `return`, así que el usuario ve `` `return` solo
  puede usarse adentro de una función ``. Querido: signal propio
  (`EvalSignal::TryOutsideFunction`) con texto dedicado.
- **`Ok(_)` / `Err(_)` con wildcard real** — hoy `_` adentro del
  patrón funciona como nombre, no como wildcard, así que ensucia
  el scope con una variable llamada `_`. Solución: variantes
  `Pattern::OkWildcard` / `Pattern::ErrWildcard`.
- **Anotación `Result<T>` en parámetros y retorno** — sigue siendo
  deuda de 2.3 (`Param.type_` y `FnDef.return_type` son `String`
  simple, no admiten genéricos).
- **Chequeo de tipo del retorno cuando se usa `?`** — descartado por
  diseño hasta el type checker estático (Fase 5). Tipado gradual.

#### 3.4 Funciones anónimas + higher-order + method calls ✓
**Completado** — el paso más grande de Fase 3. Mutación del AST para
método calls, fn anónimas como expresión, dispatch por tipo del
receptor, primera tanda de built-ins, y representación compartida
(`Rc<RefCell<>>`) para listas, mapas y campos de instancia.

- AST mutado:
  - `Expr::Call` → `{ callee: Box<Expr>, args }`. Cierra deuda de 2.3.
  - Nueva `Expr::FnExpr { params, body }` — `fn(x) => x*2` y
    `fn(x) { return x*2 }` como expresión (sin nombre).
  - `Stmt::Assign` → `{ target: AssignTarget, type_, value }` con
    `AssignTarget::Ident(String)` y
    `AssignTarget::Field { object, field }`. Cierra deuda de 2.3 y 3.2.
- Parser:
  - `Token::Fn` seguido de `(` en posición de expresión → `FnExpr`.
    `fn name(...)` sigue siendo `Stmt::FnDef`.
  - `postfix` sin restricción sobre el callee: `xs.map(f)`,
    `(fn(x)=>x+1)(2)`, `find(id)?.name`, todo encadenable.
  - `parse_expr_or_assign_stmt`: parsea el LHS como expresión y
    decide después si era `Ident = ...`, `Ident : Tipo = ...`,
    `expr.campo = ...` o expr-stmt.
- Value:
  - `Value::List(Shared<Vec<Value>>)`,
    `Value::Map(Shared<Vec<(Value, Value)>>)`,
    `Value::Instance { fields: Shared<Vec<(String, Value)>>, ... }`,
    donde `Shared<T> = Rc<RefCell<T>>`. Constructores
    `Value::new_list`, `new_map`, `new_instance`.
  - Alias por referencia (estilo Python/JS): pasar una lista a una
    función o guardarla en un campo no clona. `.push(...)` y
    `user.name = "x"` se ven a través de todos los aliases.
  - Display, igualdad estructural y coerción `Int↔Float` mantienen
    su comportamiento observable.
- Evaluator:
  - `Expr::Call`: si el callee es `Expr::Field`, hace **method
    dispatch** por `(tipo del receptor, nombre del método)`. Si no,
    evalúa el callee como cualquier expresión e invoca el `Value`
    resultante (`Function` o `Builtin`).
  - `Expr::FnExpr`: crea `Value::Function` con closure sobre el env
    actual. Sin nombre y sin binding en el env — pura expresión.
  - `Stmt::Assign` con `AssignTarget::Field`: evalúa el objeto,
    valida `Value::Instance`, muta el campo. Errores explícitos si
    no es instancia o el campo no existe.
- Built-ins (primera tanda):
  - `List`: `push(v)` muta, `pop()` muta, `map(fn)`, `filter(fn)`,
    `find(fn) -> Result`, `len()`.
  - `Map`: `get(k) -> Result`, `has(k) -> Bool`, `keys() -> List`,
    `values() -> List`, `len()`.
  - `Str`: `len() -> Int`, `upper() -> Str`, `lower() -> Str`.
- Tests del proyecto al cerrar 3.4: 472 (441 al cerrar 3.3 + 31
  nuevos: 3 en ast, 1 ajuste en parser, y 27 en evaluator entre
  fn anónimas, mutación de campos, método dispatch, built-ins y
  el E2E del criterio de éxito de Fase 3).

Guía: capítulo 13 nuevo "Métodos y mutación" entre Tipos y Result.
Renumeración 13→14, 14→15, 15→16. Cap 9 (Listas/mapas) y cap 12
(Tipos) limpiados de deuda. Cap 11 (Funciones) suma sección
"Funciones anónimas inline". Ejemplo nuevo
`examples/guide/13-metodos.fitz`; `12-result.fitz` renombró a
`14-result.fitz` y `13-errores.fitz` a `15-errores.fitz`.

**Deuda que se cierra:**
- 2.3 "method calls (`expr.method()`)" — el parser ya construye
  `Expr::Call { callee: Expr::Field {...}, args }`.
- 2.3 "asignación a campos (`user.name = ...`)" — `Stmt::Assign.target`
  admite `AssignTarget::Field`.
- 3.1 "mutación de listas (`push`/`pop`)" — métodos vivos, con alias
  compartido. `xs[i] = v` queda como deuda explícita (abajo).
- 3.2 "mutación de campos" — vivo, visible vía alias.
- 3.2 "métodos sobre instancias" (la infraestructura): el dispatch
  está; métodos custom declarados por el usuario sobre `type`
  siguen siendo deuda (queda como Fase 5+).

**Deuda explícita — retomar después:**
- **Asignación a índice (`xs[0] = v`)** — `AssignTarget` admite
  `Ident` y `Field` pero no `Index`. Habilitarlo es agregar la
  variante al AST, una rama en el parser y otra en el evaluator
  (con el mismo patrón de `Field`). Fácil; se suma cuando importe.
- **Métodos custom sobre `type`** — `type User { ... fn greet() => ... }`
  no se parsea. El dispatch del evaluador ya prevé sumar otra
  fuente de lookup sin retoques.
- **`return` adentro de un brazo de `match` como expresión** — hoy
  el cuerpo de cada brazo es expresión, no statement, así que
  `Ok(u) => return Ok(u)` rompe en el parser. Salvable con
  `match`-statement (variante con bloque por brazo) o con
  expression-with-return. Decisión pendiente.
- **Encadenamiento multi-línea** — `xs.map(...)\n.filter(...)` corta
  en el newline porque el parser termina la sentencia. Lo arreglamos
  cuando moleste (newline-soft-after-postfix-token).
- **`Pattern::OkWildcard` / `ErrWildcard`** — sigue deuda de 3.3.
- **Mensaje propio para `?` huérfano** — sigue deuda de 3.3.
- **Anotaciones compuestas (`List<T>`, `Result<T>`, `Map<K,V>`)** —
  sigue deuda de 2.3.

#### 3.5 Módulos / `import` ✓
**Completado** — el último paso de Fase 3. Carga de archivos, dos
formas de importar, namespaces aislados, cache por path canonicalizado
y detección de ciclos.

- AST nuevo: `Stmt::Import { path: Vec<String> }` y
  `Stmt::FromImport { path: Vec<String>, names: Vec<String> }`.
- Parser:
  - `import foo` / `import sub.foo.bar` — paths puntudos
    acumulando segmentos. El parser garantiza al menos un
    segmento.
  - `from foo import a, b, c` — lista de nombres (al menos
    uno; acepta trailing comma). Path puede ser punteado
    (`from sub.foo import bar`).
- Value: `Value::Module { name, env: EnvRef }`. Display
  `<module name>`. Igualdad por identidad del `Rc<RefCell>>`
  del env (dos imports del mismo archivo dan módulos iguales
  porque el cache devuelve el mismo Rc).
- Evaluator:
  - Loader como thread_local (`Option<Loader>`). Estado:
    `base_dir` (rotativo al cargar módulos anidados, vuelve
    al salir), `loading: Vec<PathBuf>` (stack para ciclos),
    `cache: HashMap<PathBuf, Value>` (por path canonicalizado).
  - Resolución relativa al `base_dir` actual: `["sub","foo"]`
    → `<base>/sub/foo.fitz`. `canonicalize` valida existencia
    y normaliza para cache + cycle.
  - Eager: al ver `Stmt::Import`/`FromImport`, carga, lexea,
    parsea y evalúa el archivo entero en un env aislado antes
    de seguir. El env del módulo registra builtins propios.
  - `import` bindea bajo el último segmento del path
    (`sub.foo` → `foo`). `from import` bindea cada nombre
    directo y NO expone el módulo.
  - Field access sobre `Value::Module` resuelve en el env
    del módulo (`utils.foo` → `utils.env.get("foo")`).
  - Method dispatch sobre `Value::Module` busca el método en
    el env del módulo y lo invoca con `invoke_value`
    (reuso del path de llamada normal).
- `eval_with_base(program, base_dir)` como entrada explícita;
  `eval(program)` queda como wrapper que usa el cwd. `main.rs`
  pasa el directorio del archivo `.fitz` que se está ejecutando.
- Cierra deuda original de 2.1: los tokens `Import` y `From`
  del lexer dejan de ser huérfanos.
- Tests del proyecto al cerrar 3.5: 503 (472 al cerrar 3.4 +
  31 nuevos: 3 en ast, 10 en parser, 3 en value, 15 en
  evaluator — incluyendo E2E con fixtures en tempdir).

Guía: capítulo 16 nuevo "Módulos" entre Errores (15) y "Qué
sigue" (que pasa a 17). Ejemplo nuevo
`examples/guide/16-modulos.fitz` + auxiliar
`examples/guide/guide_utils.fitz` (sin numeración para que el
binding generado por `import` sea un identificador válido).

**Deuda explícita — retomar después:**
- **Qualified struct literals** (`foo.User { ... }`) — el parser
  de struct literal espera `Ident { ... }`. Para usar el literal,
  hoy hay que `from foo import User`. Se puede extender el parser
  para aceptar paths como type name si la asimetría molesta.
- **`as` / aliasing** (`import foo as f`, `from foo import bar as b`)
  — sin soporte. Cuando importe se suma; es palabra contextual,
  no necesita token nuevo.
- **`pub` / privacidad** — hoy todo top-level del módulo es
  público. Si más adelante queremos marcar internos, se suma
  `pub` o convención `_underscore` validada.
- **stdlib** (`from fitz import http`) — sin soporte. El prefijo
  `fitz/` no es especial todavía; se reserva para Fase 4+.
- **Multi-línea en `from ... import (...)` con paréntesis** — sin
  soporte. La lista de nombres tiene que ir en una línea.
- **Imports anidados en bloques/funciones** — hoy nada lo
  prohíbe sintácticamente, pero el caso no está pensado: el
  binding queda en el env donde se ejecuta el import. Si
  conviene restringir a top-level, se suma flag en el parser.

### Criterio de completitud
```fitz
type User {
    id: Int
    name: Str
}

fn find_user(users: List<User>, id: Int) -> Result<User> {
    let user = users.find(fn(u) => u.id == id)
    match user {
        Ok(u)  => return Ok(u)
        Err(_) => return Err("no encontrado")
    }
}
```

---

## Fase 4 — HTTP nativo 🌐
**Estado: COMPLETADA**

La feature que diferencia a Fitz. HTTP como ciudadano de primera clase.
Cinco pasos cerrables; cada uno suma su tanda de tests y, al final, su
capítulo a la guía.

### Decisiones de diseño (tomadas antes de arrancar)

- **Runtime HTTP**: Axum + tokio (multi-thread). Bypasseamos los
  extractors tipados (nuestros handlers son `Value::Function`, no `fn`
  de Rust) — `Router::route` + closure que recibe `Request<Body>` y
  devuelve `Response`.
- **Decoradores en AST**: `decorators: Vec<Decorator>` adentro de
  `FnDef`. Genérico, no atado a HTTP. El evaluator despacha por nombre
  (`@get`/`@post`/`@put`/`@delete` → registran ruta; `@server` →
  configura; cualquier otro → error explícito). Por ahora args son
  solo positionals (named args queda como deuda — destrabaría
  `@server(port: 8080)`).
- **Arranque del servidor**: automático al final de eval si hay rutas
  registradas. `@server(...)` configura host/port, no dispara nada.
  Coincide con el syntax-spec.
- **Bridge sync/async**: Fitz sigue siendo intérprete síncrono;
  `is_async` se sigue ignorando. Cada request lee el body async
  (axum/tokio) y corre el handler Fitz vía `spawn_blocking` para no
  bloquear el reactor. Para evitar `Rc`-cross-thread, vamos por un
  thread dedicado al intérprete + canal de tareas. Async real adentro
  del lenguaje es deuda explícita (Fase 4.x o 5).
- **Serialización JSON**: automática desde `Value`. Primitivos
  obvios; `List` → array; `Map` → object (claves Str obligatorias);
  `Instance` → object con campos en orden; `Result::Ok(v)` → 200
  con `v`; `Result::Err(e)` → 500 con `{"error": e}`; tipos no
  serializables → error explícito.
- **Deserialización del body**: el handler declara `body: TipoCustom`;
  el runtime valida el JSON contra el `type` (mismas reglas que
  `StructLit`) y construye `Value::Instance`. Errores → 400.
- **Path params**: convertidos según el tipo del parámetro
  (`id: Int` → parsear como int; fallo → 400). Sin anotación
  default a `Str`. Query params quedan como deuda.

### Pasos

#### 4.1 — Decoradores genéricos sobre FnDef ✓
**Completado** — refactor preparatorio. El AST y el parser hablan
decoradores apilables; el cableado real con el runtime HTTP llega
en 4.2.

- AST: nueva `Decorator { name: String, args: Vec<Expr> }`;
  `Stmt::FnDef` gana `decorators: Vec<Decorator>`. Eliminados
  `Stmt::HttpEndpoint` y `HttpMethod` (cumplido el TODO de Fase 4
  que tenía el AST desde Fase 2).
- Parser: `parse_decorated_fndef` apila uno o más
  `@nombre(args...)` antes de `[async] fn ...`. Args usan
  `parse_call_args` — son expresiones cualquiera. Cada decorator
  exige paréntesis (incluso vacíos: `@server()`), mantiene la
  sintaxis predecible.
- Evaluator: `Stmt::FnDef` con `decorators` no vacío corta con
  error explícito mencionando los nombres concretos
  (`@get`/`@server`/etc.) y "requieren Fase 4.2 — runtime HTTP".
  Es un puente intencional: AST y parser ya soportan la sintaxis,
  la semántica espera el runtime.

Tests al cerrar 4.1: 511 (503 al cerrar 3.5 + 8 nuevos: 3 en ast,
5 netos en parser, 2 en evaluator).

**Observación de diseño cerrada**: el path en `@get("/users/{id}")`
llega al evaluator como `Expr::StrInterp` (porque `{id}` es sintaxis
de interpolación de Fitz). En 4.2 los `StrPart::Expr(Ident(...))`
del path se reconocerán como path params sin necesidad de un mini
parser dedicado dentro del decorator. Es una buena noticia, no un
bug — reusa la maquinaria existente.

#### 4.2 — Runtime HTTP mínimo (GET + Result handling) ✓
**Completado** — server axum + tokio + bridge sync/async funcionando.
GET con path params tipados, serialización JSON automática, Result
auto-handling. `cargo run -- run server.fitz` levanta el server.

- Nuevo módulo `src/http.rs` con:
  - `HttpRegistry` + `RouteSpec` + `RouteMeta` + `HttpMethod`.
  - `with_active_registry(...)` para que el evaluator vea un registry
    durante eval vía thread_local.
  - `parse_path_template`: traduce `Expr::Str` o `Expr::StrInterp` del
    decorator a path axum (`/users/{id}`) + lista de param names.
  - `value_to_json` + `value_to_outcome`: serialización total con
    Result auto-handling (`Ok(v)`→200, `Err(e)`→500 con `{"error":e}`),
    tipos opacos (Function/Module/Type/Range) → 500 explícito.
  - `coerce_path_param`: convierte string crudo a Int/Float/Str/Bool
    según el tipo declarado del parámetro del handler. Falla → 400.
  - `build_router` + `serve(registry, addr)`: arranca axum en un
    std::thread spawneado; el thread main entra al
    `run_interpreter_loop`. Bridge vía
    `mpsc::UnboundedSender<InterpTask>` + `oneshot::Sender<HandlerOutcome>`
    por request. Graceful shutdown con Ctrl-C.
- Evaluator: cuando ve `Stmt::FnDef` con decorator `@get`/`@post`/
  `@put`/`@delete` y hay registry activo, valida (1 arg path, path
  starts with `/`, cada `{x}` tiene su parámetro en el handler) y
  registra una `RouteSpec`. Sin registry → error explícito con
  sugerencia "ejecutá con `fitz run`". Decoradores no implementados
  (`@server`, `@patch`, etc.) → error con el nombre.
- `main.rs`: envuelve `eval_with_base` en `with_active_registry`. Si
  después de eval el registry tiene rutas, llama a `http::serve` en
  127.0.0.1:3000 (`@server(...)` configurable llega en 4.4).
- Axum 0.8 (no 0.7): la sintaxis `{id}` del syntax-spec mapea directa
  al matcher de axum. Bumpeo deliberado.

Tests al cerrar 4.2: 558 (511 al cerrar 4.1 + 47 nuevos repartidos
en `src/http.rs` — 28 unit tests del módulo + 7 E2E con `Router::oneshot`
sobre `LocalSet` — y 8 nuevos en `evaluator::tests` para el flujo de
registro). Validado manualmente: server real con `curl` responde 200,
400, 500 según corresponde.

**Decisión de threading documentada en código**: el intérprete vive
en el thread main (donde corrieron los `Rc<RefCell<>>` del eval), no
en un thread spawneado — `Value` no es `Send`. Tokio corre en un
std::thread propio. Lo que cruza el canal son strings y números.

**Limitaciones de 4.2 (deuda explícita, no nueva):**
- Solo GET/POST/PUT/DELETE sin body — el body llega en 4.3.
- Sin query params, sin headers, sin status codes custom.
- `@server(...)` parsea pero no hace nada (4.4).
- `async fn` se acepta sintácticamente pero no aporta nada en
  runtime (deuda vieja: async real adentro del lenguaje).

#### 4.3 — Body + deserialización JSON ✓
**Completado** — handlers POST/PUT/DELETE (y cualquier método)
pueden declarar un body. El runtime parsea el JSON, valida contra
el `type` declarado y construye una `Value::Instance` antes de
invocar al handler. Body sin anotación de tipo llega como
`Value` libre (Map/List/primitivos).

- AST/Parser: sin cambios (decorators ya genéricos desde 4.1, body
  es simplemente un parámetro del handler).
- `http::RouteSpec.body_param: Option<BodyParam>` y
  `RouteMeta.expects_body: bool`. `BodyParam` lleva nombre, el
  `Value::Type` declarado (clonado del env durante registro) y el
  nombre del tipo para mensajes.
- `http::json_to_value`: deserialización total sin schema. Números
  enteros → Int, con parte fraccional → Float, objects → Map con
  claves Str, arrays → List.
- `http::json_to_instance`: con schema — valida contra los campos
  del `type` (faltantes con default OK, faltantes nullables → Null,
  extras → 400 mencionando el nombre, body no objeto → 400). Los
  defaults soportan literales constantes; defaults complejos
  (expresiones que usan otros bindings) son deuda explícita.
- `http::InterpTask` gana `body: Vec<u8>`. `handle_task` parsea el
  body antes de armar args; body roto o inválido → 400 con mensaje;
  body vacío con handler que lo espera → 400 ("body requerido").
- `build_method_router` ahora tiene 4 ramas (path × body) porque
  los extractors de axum aparecen como args del handler. El helper
  `wrap(method, h)` evita repetir el match por verbo en cada rama.
- Convención de registro: cada parámetro del handler es path param
  (su nombre está en `path_params`) o body. Máximo un body por
  handler — más de uno → error explícito al registrar.
- `serde_json` con feature `preserve_order` para que el JSON de
  respuesta respete el orden declarado del `type`.

Tests al cerrar 4.3: 581 (558 al cerrar 4.2 + 23 nuevos: 15 en el
módulo http — 3 de `json_to_value`, 6 de `json_to_instance`,
6 de `handle_task` con body — 4 E2E nuevos sobre `Router::oneshot`,
y 4 nuevos del evaluator validando registro de body).

Validado manualmente con `curl`:
  - POST `/users` body válido → 200 con orden de campos preservado.
  - Campo nullable faltante → `email: null`.
  - Campo extra → 400 con nombre.
  - Body JSON roto → 400 con error de parseo.
  - PUT con path param + body mezclando ambos.
  - POST con body sin anotación → echo como Value libre.

**Limitaciones de 4.3 (deuda explícita):**
- Defaults complejos (no literales) en campos del body — fallan
  silencioso al validar; mensaje sugerente "pasalo explícito".
- Sin validación de Content-Type: cualquier body se intenta como
  JSON. Multipart/form-data, urlencoded → cuando hagan falta.
- Sin validación de tipos compuestos en campos del body (`emails: List<Str>`)
  — sigue siendo deuda vieja del type system (Fase 5).
- Query params siguen sin soporte.

#### 4.4 — `@server(...)` configuración ✓
**Completado** — el programa puede declarar puerto y host del
server con `@server(port, host)` sobre cualquier `fn` (típicamente
`fn main()` como placeholder; la fn queda definida en el env pero
no se ejecuta automáticamente).

- `http::ServerConfig { host: String, port: u16 }` con
  `default_addr()` (127.0.0.1:3000) y `to_socket_addr()` (parsea
  el host como IP literal, sin DNS).
- `HttpRegistry.server_config: Option<ServerConfig>` y
  `resolved_config()` que devuelve el explícito o el default.
- `http::set_server_config` impone unicidad: dos `@server` →
  `Err` con el config previo.
- Evaluator `register_server_config`:
  - 0/1/2 args positionals; >2 → error.
  - Port: Int en `[1, 65535]`, otro tipo/literal → error.
  - Host: Str literal que parsea como IP (IPv4/IPv6), otro → error.
  - Sin registry activo → error explícito.
- `main.rs` usa `registry.resolved_config().to_socket_addr()` antes
  de llamar a `http::serve`.

Razón de diseño documentada en código: `@server` se aplica como
decorator sobre una fn (la fn queda definida pero no se ejecuta).
Mantiene uniformidad con `@get/@post/etc` y evita un caso especial
en el parser. La forma con named args (`@server(port: 8080)`) sigue
siendo deuda — espera a que el lenguaje tenga named args en `Call`.

Tests al cerrar 4.4: 595 (581 al cerrar 4.3 + 14 nuevos: 5 en
`http::tests` para `ServerConfig`/`set_server_config`/
`resolved_config`, 9 en `evaluator::tests` cubriendo registro
válido, defaults parciales, errores de tipo, rango, IP inválida,
doble decorator).

Validado a mano: `@server(8181, "127.0.0.1")` levanta en 8181 y
3000 no responde.

#### 4.5 — Guía + ejemplos + cierre de fase ✓
**Completado** — cierre formal de Fase 4 con documentación viva.

- Capítulo 17 nuevo "HTTP nativo" entre Módulos (16) y Qué sigue
  (renumerado a 18). Cubre: primer endpoint, los cuatro verbos,
  path params tipados, body con `type` o libre, serialización JSON
  automática, `@server(port, host)`, integración con `Result + ?`,
  modelo de threading (intérprete sync + tokio en thread aparte),
  qué todavía no anda (`async` real, status codes custom, query
  params, headers, middleware, named args).
- `examples/guide/17-http.fitz` ejecutable: mini API con `/`,
  `/users`, `/users/{id}`, `POST /users`. Memoria del server =
  env del programa.
- `examples/server.fitz` reescrito como criterio de éxito de Fase 4:
  CRUD completo (GET/POST/PUT/DELETE) con `Result + ?`. Validado
  a mano contra `curl` end-to-end.
- Guía pre-cap-17: actualizado preámbulo (fecha, número de tests,
  HTTP movido de "no funciona aún" a "feature core"), índice
  reorganizado en 8 partes, "Qué cubre" y "Qué no anda"
  reactualizados, cierres de cap obsoletos limpiados, deuda del
  cap 5 (métodos sobre strings) corregida (los básicos sí existen
  desde 3.4).
- Cierre cap 18 "Qué sigue" reescrito a post-Fase-4: lo aprendido
  incluye HTTP, el "más adelante" apunta a Fase 5 (compilador) y
  Fase 9 (ecosistema).

**Decisión de implementación documentada en la guía**: `return`
adentro de un brazo de match no parsea (deuda viva del 3.4) — el
ejemplo del cap 17 usa `return match { ... }` con el valor directo
en cada brazo. Sigue siendo deuda explícita; cuando se cierre,
ambas formas van a funcionar.

Tests al cerrar 4.5: 595 (sin cambios respecto de 4.4 — el cierre
de fase es documentación + ejemplos, sin código nuevo).

### Deuda explícita ya identificada para Fase 4

- **Named args en decorators** (`@server(port: 8080, host: "0.0.0.0")`)
  — hoy sólo positionals (`@server(8080, "0.0.0.0")`).
- **Async real en el lenguaje** — `await`, futures, async fn que de
  verdad sea async. Comprometido como Fase 6.
- **Response builder rico** — `return 401 { ... }` del syntax-spec,
  status code custom, headers, content-type. Sigue siendo deuda.
- **Query params** — `?page=1&size=10`. Sin soporte en 4.x.
- **Middleware** — auth, logging, CORS via decoradores apilables.
- **Hot reload** del server al cambiar el `.fitz`.

### Criterio de completitud
```fitz
type User {
    id: Int
    name: Str
}

@get("/users/{id}")
async fn get_user(id: Int) -> User {
    return User { id: id, name: "Test" }
}
```
```bash
fitz run api.fitz
# GET http://localhost:3000/users/1
# → {"id": 1, "name": "Test"}
```

---

## Fase 5 — Compilador ⚡
**Estado: 5a COMPLETADA / 5b EN CURSO (5b.2 cerrado)**

Plan aprobado: dos mitades cerrables.
- **5a — Type checker estático** (sobre el intérprete actual) ✓
  Cerrado al cerrar el paso 5.4: el checker recorre lexer → parser →
  resolución de anotaciones → expresiones (synthesis, llamadas,
  return, Result/`?`/match exhaustivo, métodos built-in,
  FnExpr.ret inferido, Index). `fitz run` corre el checker por
  default y aborta si hay errores; `--no-typecheck` lo salta.
- **5b — Codegen a binario nativo** — backend elegido:
  **transpile-a-Rust** sobre Cranelift/LLVM. Razones decisivas:
  reuso de toda la infra del compilador, async real cuando llegue
  (sin escribir runtime propio), cross-compile a todos los targets
  de rustc, y la posibilidad de mapear handlers `@get`/`@post` a
  `async fn` axum sin trabajo extra cuando llegue 5b.6. Trade-off:
  compile times = rustc. Se divide en siete sub-pasos cerrables
  (5b.1 → 5b.7) con criterio de "hello world compilado" hoy y
  CRUD HTTP compilado al cerrar 5b.7.

### Pasos

#### 5.1 — TypeExpr en AST y parser ✓
**Completado** — refactor preparatorio del checker. El AST y el
parser ahora modelan tipos compuestos; el evaluator y el runtime
HTTP los consumen sin cambiar comportamiento observable.

- AST nuevo: `TypeExpr` con tres variantes:
  - `Named(String)` — `Int`, `Str`, `User`.
  - `Generic { name, args }` — `List<Int>`, `Map<Str, User>`,
    `Result<List<User>>`, anidable.
  - `Nullable(Box<TypeExpr>)` — sufijo `?` (`User?`, `List<Int>?`).
  - Helpers: `display_name()` (reproduce la forma del fuente),
    `head_name()` (cabeza ignorando genéricos y nullables, para que
    el runtime HTTP resuelva tipos custom), `is_nullable()`.
- AST refactor: `Param.type_`, `Stmt::Assign.type_` y
  `Stmt::FnDef.return_type` pasan de `Option<String>` a
  `Option<TypeExpr>`. `Field.type_` pasa de `String` a `TypeExpr`,
  y el flag `Field.nullable: bool` se elimina — la nullabilidad
  vive adentro como `TypeExpr::Nullable(...)`.
- Parser: nueva regla `parse_type_expr` (gramática
  `atom '?'?`, `atom = Ident generic_args?`). Reemplaza los tres
  call sites (`parse_optional_type_annotation`,
  `parse_optional_return_type`, field type adentro de
  `parse_typedef`). El lexer ya emitía `>` como `Token::Gt` único
  (no hay `>>` como un solo token), así que `Result<List<Int>>`
  se cierra consumiendo dos `Gt` separados sin trabajo extra.
- Evaluator: migrado para usar `head_name()` al resolver el
  `Value::Type` declarado del body param de un handler HTTP, y al
  empaquetar tipos de path params (que siguen siendo primitivos
  para `coerce_path_param`). Sin cambios de semántica en runtime
  — las anotaciones se siguen ignorando, exactamente como antes.
- http.rs: misma migración + `field.nullable` reemplazado por
  `field.type_.is_nullable()`.

Cierra deuda 2.3 "tipos compuestos en anotaciones
(`List<T>`, `Map<K,V>`, `Str?`)" a nivel sintáctico.

Tests al cerrar 5.1: 614 (595 al cerrar 4.5 + 19 nuevos: 6 en
ast.rs cubriendo `display_name`/`head_name`/`is_nullable` y
formas anidadas; 13 en parser.rs cubriendo `let x: T = ...`,
params/return, fields, nullable de generic vs adentro del
generic, errores `List<>` / generic sin cerrar / `:` sin tipo,
y round-trip de display sobre `Map<Str, Result<List<User>?>>`).

**Deuda explícita — retomar después:**
- **Función como tipo** (`fn(Int) -> Int` como `TypeExpr`) — no
  se modela todavía. Se suma cuando el checker (5.3) lo necesite
  para callbacks tipados.
- **Validación semántica** (nombre no resoluble, aridad incorrecta
  del genérico, coerción Int↔Float consistente) — es 5.2.
- **`T??` repetido** — el parser solo consume un `?`. Un segundo
  `?` queda sin consumir y rompe en la siguiente etapa con un
  mensaje desafortunado. Definir si lo permitimos (semánticamente
  `Nullable(Nullable(T)) == Nullable(T)`) o si erroramos explícito.
- **Guía**: sin capítulo nuevo. Tipos compuestos en anotaciones
  se aceptan en sintaxis pero el evaluator los sigue ignorando
  igual que antes; el capítulo entra cuando 5.2 los chequea.

#### 5.2 — Resolución de tipos y type checker base ✓
**Completado** — primer chequeo estático real. Nuevo módulo
`src/types.rs` con representación interna `Type`, tabla `TypeEnv`,
resolución de `TypeExpr → Type` con validación de aridad y
existencia, y pasada de chequeo sobre las anotaciones del programa.

- `Type` con primitivos como singletons (`Int`, `Float`, `Str`,
  `Bool`, `Null`, `Range`), genéricos built-in con aridad fija
  (`List<T>`, `Map<K, V>`, `Result<T>`), `Nominal(TypeId)` para
  tipos declarados, `Nullable(Box<Type>)` para `T?`. Identidad
  nominal por `TypeId` — dos `type User` en módulos distintos
  serían tipos distintos.
- `TypeEnv` con declare_nominal/set_fields/lookup, soporta forward
  refs cross-tipo (`type A { b: B }; type B { a: A }`).
- `resolve_type_expr`: traduce `TypeExpr` a `Type` validando
  primitivo + 0 args, genéricos con aridad exacta, nominal sin
  args. Errores claros: "tipo desconocido `Foo`", "el tipo `List`
  espera 1 argumento(s) de tipo, recibió 2".
- `resolve_program` en tres vueltas: nombres → fields → resto de
  anotaciones (params, return type, lets — incluso adentro de
  bodies de funciones). Acumula todos los errores en lugar de
  cortar al primero.
- `check_field_default` valida defaults literales contra el tipo
  declarado del campo (`Int = "x"` → error; `Float = 1` → OK por
  coerción; `Str? = null` → OK). Defaults no-literales se
  postergan a 5.3.
- CLI: `fitz check <file>` corre lexer + parser + resolución,
  reporta errores con contexto, exit code 1 si hay alguno.
  `fitz run` lo corre **en modo warning**: imprime los errores
  pero no aborta — los programas existentes siguen ejecutándose
  igual durante 5.x. El default flipea a "strict aborta" al
  cerrar 5a (después de 5.4).
- `FitzError` Display ahora omite el prefijo `en línea 0:0` cuando
  no hay posición. Beneficia al checker (sin posiciones todavía)
  y a varios errores del evaluator que ya estaban así.

Cierra parcialmente deuda 2.4 "anotaciones de tipo se ignoran"
— las anotaciones ahora se resuelven y validan; el chequeo de
valores contra los tipos resueltos entra en 5.3.

Tests al cerrar 5.2: 651 (614 al cerrar 5.1 + 37 nuevos en
`types.rs`: resolución por primitivo, genérico con aridad
correcta e incorrecta, nullable de primitivo y de generic,
nominal declarado y desconocido, generic con arg inválido que
propaga, programa vacío, type con primitivos, type con
generic+nullable, type que referencia otro type, forward refs
mutuas, type con field de tipo inexistente, type redeclarado,
defaults literales compatibles/incompatibles, default null sobre
nullable/no-nullable, default Int sobre Float, default
no-literal aceptado, fndef con anotaciones válidas, fndef con
param/return/generic inválido, assign con tipo inválido, lets
adentro de body validados, múltiples errores acumulados, AST
construido a mano).

Validado a mano: los 17 ejemplos de la guía y `examples/server.fitz`
pasan `fitz check` sin errores. Un archivo de prueba con 4 errores
distintos (tipo desconocido en campo, default incompatible, otro
tipo desconocido, aridad incorrecta de generic) se reporta
completo y con contexto en `fitz check` (exit 1) y como warnings
en `fitz run` (exit 0, programa ejecuta).

**Deuda explícita — retomar después:**
- **Posiciones de error** — `TypeExpr` no carga línea/columna, así
  que los errores del checker salen sin posición. Mismo issue que
  varios `FitzError` del evaluator. Pelarlo es un refactor amplio
  del parser; cuando se cierre, el Display vuelve a mostrar
  línea/columna en todos los casos.
- **Chequeo de expresiones contra el tipo declarado** — `let x:
  Int = "hola"` hoy no falla (el valor es un literal Str, el tipo
  declarado es Int, pero el checker no compara). Es el corazón
  de 5.3.
- **Sugerencias "¿quisiste decir...?"** — sin similaridad de
  nombres todavía. Nice-to-have post-5.4.
- **Imports cross-módulo en el checker** — el evaluator carga
  módulos lazy/eager, pero `resolve_program` chequea cada archivo
  por separado. Cuando 5.3 valide expresiones, los tipos
  importados van a tener que aparecer en el `TypeEnv` del archivo
  que los usa. Por ahora, los handlers que usan tipos importados
  (caso típico: `examples/server.fitz` sin imports) siguen
  funcionando porque cada archivo se resuelve solo y sus `type`s
  locales están en su env.

#### 5.3 — Type checker de expresiones y funciones ✓
**Completado** — los cinco sub-pasos cerrados. Cubre la
sintaxis completa del lenguaje observable hoy. El cierre formal
con la lista de pendientes naturales (que no bloquean 5.4) está
al final de 5.3.5.

##### 5.3.1 — Synthesis básico ✓
**Completado** — primera pasada del checker que mira EXPRESIONES.
Sintetiza tipos para literales, idents, BinOp aritmético/comparación/
lógico, UnaryOp Neg, StrInterp, `if`, list/map literales, struct
lit, field access sobre Nominal, Range. Asignaciones con anotación
validan compatibilidad. Scopes locales para FnDef/FnExpr/while/for/
loop. Match bindea las variables de los patrones (Ident, OkBinding,
ErrBinding). Imports registran nombres como vars (`import foo`,
`from foo import X`) y los nombres de `FromImport` se registran
también como nominales en el TypeEnv para que `User { ... }` no
falle.

- Nuevas variantes de `Type`: `Function { params, ret }` (modelado
  pero ret todavía es Any en 5.3.1) y `Any` (escape gradual para
  expresiones que el checker aún no modela y para anotaciones que
  faltan).
- `CheckCtx`: stack de scopes para variables, errores acumulados,
  builtins (`print`, `len`) pre-registrados como Any.
- `infer_expr`: synth bottom-up. Cubre los Expr listados arriba;
  Call/FnExpr/Index/Match/Ok/Err/Try devuelven Any o info parcial
  hasta que las sub-fases siguientes los refinen.
- `check_stmt` + `check_block`: walker de Stmt con scopes. Stmt::
  Assign con anotación compara RHS contra el tipo declarado;
  para FnDef abre scope y bindea params con tipo declarado o Any.
- `is_compatible`: Any compatible con todo, Null compatible con
  `T?`, `T` compatible con `T?`, Int compatible con Float
  (coerción), resto = igualdad estructural.
- Pre-registro de firmas: las `FnDef` top-level se registran como
  `Type::Function` en el scope global antes de walkear los bodies,
  habilitando referencias hacia adelante y recursión mutua.
- Entry point público nuevo: `check_program` (corre
  `resolve_program` + pasada de expresiones). `resolve_program`
  queda como API privada del módulo para tests granulares.
  `fitz check` y `fitz run` ahora llaman a `check_program`.

Tests al cerrar 5.3.1: 700 (651 al cerrar 5.2 + 49 nuevos
cubriendo ident desconocido/conocido/nominal-como-value,
builtins, BinOps aritmético/comparación/lógico con tipos OK y
errores, UnaryOp Neg, Range, lists vacías/homogéneas/anotadas
mal, maps vacíos, StructLit con tipo conocido/desconocido/campo
mal tipado/campo extra, field access OK e incompatible, Assigns
con varias formas de compatibilidad, if/while con cond mala,
for sobre Range/List/no-iterable, FnDef y FnExpr bindeando
params, match con Ident/OkBinding/ErrBinding bindings, imports
y from-imports, acumulación de múltiples errores).

Validado a mano: los 17 ejemplos de la guía y
`examples/server.fitz` pasan `fitz check` limpios; `fitz run`
produce el mismo output que antes; un archivo de prueba con 7
errores variados (assign con tipo mal, BinOp con Str/Int, var
desconocida, if con cond Int, for sobre Int, StructLit con
campo mal tipado, StructLit con tipo desconocido) se reportan
con mensajes específicos y contexto.

**Limitaciones conocidas de 5.3.1 (todas pendientes en sub-pasos siguientes):**
- Llamadas no validan aridad ni tipos de args — Call devuelve
  el ret del Function si lo conoce, Any si no. Es 5.3.2.
- `Stmt::Return` no se compara contra return_type — es 5.3.2.
- `?` sobre no-Result no falla, y no exige que la fn contenedora
  devuelva Result. Es 5.3.3.
- Match sobre Result no exige ambas ramas (exhaustividad). Es 5.3.3.
- Métodos built-in (`xs.map`, `m.get`, etc.) no se chequean: el
  Field dentro de un Call devuelve Any. Es 5.3.4.
- `FnExpr.ret` queda en Any. 5.3.5 sintetizará a partir del body.

##### 5.3.2 — Llamadas, return contra return_type ✓
**Completado** — el checker valida llamadas (aridad + tipos de
args) y `Stmt::Return` contra el return type declarado. Reusa el
pre-registro de firmas top-level de 5.3.1.

- `is_compatible` ahora recursa adentro de generics built-in:
  `List<a>↔List<b>`, `Map<ka,va>↔Map<kb,vb>`, `Result<a>↔Result<b>`,
  `Nullable<a>↔Nullable<b>`, y `Function` (estructural: misma
  aridad + cada param compatible + ret compatible). Caso clave
  que destraba: `Err("...")` sintetiza `Result<Any>` y ahora pasa
  contra una declaración `-> Result<User>` sin escape adicional.
- `CheckCtx.return_stack: Vec<Type>` — stack para soportar
  funciones anidadas. `Stmt::FnDef` pushea el return type
  resuelto (o `Any` si la anotación faltó / no resolvió);
  `Expr::FnExpr` pushea `Any` porque el AST no carga return type
  declarado para FnExpr (la inferencia desde el body llega en
  5.3.5).
- `Stmt::Return` infiere el tipo de la expresión y, si hay algo
  en `return_stack`, compara con `is_compatible`. Mensaje:
  ``` `return` devuelve `X` pero la función declara `Y` ```.
  Return huérfano (fuera de función) no chequea — el evaluator
  lo emite en runtime, sin solapamiento.
- `Expr::Call` valida aridad y compatibilidad de cada arg contra
  `Function.params`, y devuelve `*ret` como tipo sintetizado.
  Reglas:
  - Callee `Any` → no chequea (escape gradual). Esto cubre
    variables traídas por `from import` cuyo tipo real
    desconocemos hasta que carguemos módulos cross-archivo.
  - Callee `Function { params, ret }` → aridad estricta + tipos
    estrictos; en errores incluye índice 1-based del argumento.
  - Callee de tipo concreto distinto (`Int`, `Str`, etc.) →
    error explícito "`X` no es una función".
- Helper `describe_callee` produce etiquetas amigables para los
  mensajes de error: `Expr::Ident("foo")` → "la función `foo`",
  `Expr::Field { field: "map", .. }` → "el método `map`", resto
  → "esta llamada".
- Builtins: `len` deja de ser `Any` y pasa a
  `Function { params: [Any], ret: Int }`. Captura `len(1, 2)` /
  `len()` como errores de aridad y permite asignar el resultado a
  un `Int` sin warning. `print` queda como `Any` (variádico, sin
  representación dedicada todavía). **Convención que se
  establece**: builtins de aridad fija reciben firma real;
  variádicos siguen siendo `Any` hasta tener `Type::Variadic` o
  un mecanismo equivalente.
- E2E: `examples/server.fitz` (CRUD con `-> Result<User>` y
  `return Err("...")` / `return Ok(...)`) pasa `fitz check`
  limpio gracias a la recursividad de `is_compatible`. Los 17
  ejemplos de la guía pasan limpios excepto `15-errores.fitz`,
  que es intencional — el ejemplo demuestra un error de aridad
  (`fn add(a, b) => a + b; print(add(5))`) y ahora el checker lo
  capta estáticamente antes del error de runtime. Cap 15 de la
  guía actualizado con una nota corta.

Tests al cerrar 5.3.2: 727 (700 al cerrar 5.3.1 + 27 nuevos en
`types.rs` — calls con aridad correcta/menos/más/tipos, coerción
Int→Float, Null→nullable, recursión + forward ref, callee no-fn,
FnExpr inline; builtins `len` y `print`; Stmt::Return con tipo
compatible/incompatible, sin anotación, arrow implícito,
`Ok`/`Err` contra `Result<User>`, return huérfano; recursividad
de `is_compatible` en List/Map/Result/Function).

**Deuda explícita — retomar después:**
- **Métodos built-in sin chequear** — el callee `Expr::Field`
  evalúa a `Any` (hasta 5.3.4), por lo que `xs.map(1, 2, 3)` o
  `s.upper(extra)` pasan sin warning. Es 5.3.4.
- **`Expr::Try` (`?`) sin chequeo** — sigue devolviendo `Any` y
  no exige que el operando sea `Result` ni que la fn contenedora
  retorne `Result`. Es 5.3.3.
- **Match sobre `Result` no exige exhaustividad** — un brazo
  solo `Ok(x)` sin `Err(_)` (o viceversa) pasa. Es 5.3.3.
- **`FnExpr.ret` queda en `Any`** — el cuerpo no informa el ret
  sintetizado. Es 5.3.5.
- **Llamadas a vars `Any` no chequean aridad** — cuando un
  nombre importado (`from foo import bar`) viene como `Any`, las
  llamadas a `bar(...)` no validan nada. Es consistente con el
  modelo gradual; cuando 5.3.x cargue módulos cross-archivo, las
  firmas reales destrabarían el chequeo.

##### 5.3.3 — Result, `?`, match exhaustivo ✓
**Completado** — el checker valida el operador `?` y exige
exhaustividad de `match` cuando el scrutinee tipa como `Result<T>`.

- `Expr::Try` (`?`) valida el operando:
  - `Type::Any` → devuelve `Any` sin chequear (gradual escape;
    cubre el caso típico de método built-in cuyo callee `Field`
    todavía devuelve `Any` hasta 5.3.4).
  - `Type::Result(inner)` → desempaca a `*inner`. Si el operando
    es Result concreto y estamos adentro de una función con
    `return_type` concreto, exige que ese return type también
    sea `Result<...>` (o `Any`) — el `?` propaga el `Err(_)` vía
    `return`, así que la fn contenedora tiene que poder
    recibirlo. Reusa `return_stack`. Mensaje: "el operador `?`
    solo puede usarse adentro de una función que retorne
    `Result<...>`; esta retorna `X`". Top-level y `Expr::FnExpr`
    no disparan la regla (return_stack vacío o `Any`).
  - Otro tipo concreto → error: "el operador `?` requiere un
    `Result`, recibió `X`".
- `Expr::Match` exige exhaustividad **solo cuando el scrutinee
  tipa como `Result<T>` puro** (no nullable). Decisión de
  diseño: `Result<T>?` (un valor que puede ser ok/err/null) es
  semánticamente raro; lo dejamos sin exigir hasta que aparezca
  como necesidad real. Match sobre Int/Str/Bool/Any/etc. tampoco
  exige exhaustividad — no tenemos semántica de variantes para
  ellos todavía.
- Helper nuevo `check_result_match_exhaustiveness`: recorre los
  arms y setea `has_ok`, `has_err`, `has_catchall`. Catch-all =
  `Pattern::Wildcard` o `Pattern::Ident(_)`. Si hay catch-all o
  ambos `Ok` y `Err` → exhaustivo. Si no, error mencionando qué
  variante falta (`Ok`, `Err`, o ambas). `Ok(_)` / `Err(_)`
  cuentan como `Ok` / `Err` (la deuda de `Pattern::OkWildcard` /
  `ErrWildcard` de 3.3 queda fuera de scope; el `_` adentro se
  comporta hoy como un nombre que ensucia el scope pero a nivel
  exhaustividad pesa lo mismo). Patrones literales/de rango
  sobre Result son técnicamente "imposibles"; no los rechazamos
  acá (sería un check separado).
- E2E: los 17 ejemplos + `examples/server.fitz` pasan
  `fitz check` sin warnings nuevos. Razón: las fns que usan `?`
  no declaran `-> Result<X>` (return_stack queda `Any`, la regla
  no dispara) y los `?` operan típicamente sobre métodos
  built-in que devuelven `Any` hasta 5.3.4. Los matches sobre
  Result existentes (`14-result.fitz`, `server.fitz`) son todos
  `Ok + Err`, exhaustivos.

Tests al cerrar 5.3.3: 742 (727 al cerrar 5.3.2 + 15 nuevos en
`types.rs`: `?` sobre Result + fn Result OK, sobre Any (no
chequea), sobre no-Result (error), adentro de fn no-Result
(error), adentro de fn sin return_type (no chequea), top-level
(no chequea regla de fn), encadenado con field access
(`r?.id`); match sobre Result con Ok+Err exhaustivo, solo Ok
(error falta Err), solo Err (error falta Ok), wildcard solo,
Ok+wildcard, ident catch-all, match sobre Int / Any
(no exige exhaustividad)).

**Deuda explícita — retomar después:**
- **`Pattern::OkWildcard` / `ErrWildcard`** — sigue deuda vieja
  de 3.3. Hoy `Ok(_)` parsea como `OkBinding("_")` y ensucia el
  scope con una variable llamada `_`. El checker lo trata como
  `has_ok = true` sin diferenciar.
- **Patrones literales sobre Result** son "imposibles" pero no
  se emiten warnings. Es un check separado (dead-code de match)
  que podría llegar como nice-to-have.
- **Métodos built-in que deberían retornar `Result<T>`** —
  `xs.find(...)`, `m.get(...)`. Hoy son `Any` (el chequeo de `?`
  los deja pasar gradual). Cuando 5.3.4 les dé firma real, el
  encadenado `xs.find(...)?` va a chequear con precisión real.
- **`?` adentro de `Result<X>?`** — no exigido, decisión de
  diseño. Revisitable si aparece el caso.

##### 5.3.4 — Métodos built-in con templates paramétricos ✓
**Completado** — `Expr::Call` con `callee: Expr::Field` ahora
despacha por `(tipo del receptor, nombre del método)` a una tabla
built-in en lugar de caer en el camino general (que no podía
modelar signatures paramétricas).

- Nuevo helper `infer_method_call(ctx, receiver_ty, method,
  args_ty) -> Option<Type>` con sub-dispatchers
  `infer_list_method`, `infer_map_method`, `infer_str_method`.
  Cada uno hace match sobre el nombre del método, valida aridad
  + tipos vs la signature concreta del receptor, y devuelve el
  ret instanciado.
- Tabla de signatures cubierta (14 métodos):
  - `List<T>`: `push(T) -> Null`, `pop() -> T`, `len() -> Int`,
    `map(fn(T) -> U) -> List<U>`,
    `filter(fn(T) -> Bool) -> List<T>`,
    `find(fn(T) -> Bool) -> Result<T>`.
  - `Map<K, V>`: `get(K) -> Result<V>`, `has(K) -> Bool`,
    `keys() -> List<K>`, `values() -> List<V>`, `len() -> Int`.
  - `Str`: `len() -> Int`, `upper() -> Str`, `lower() -> Str`.
- Helpers compartidos:
  - `check_method_arity(name, args_ty, expected) -> bool` —
    aridad fija, devuelve `false` cuando no coincide (caller
    puede saltarse validaciones extra).
  - `check_unary_callback(cb, elem_ty, method, expected_ret)
    -> Type` — exige `Function` con aridad 1, valida que el
    param sea compatible con T, y opcionalmente que el ret sea
    compatible con un tipo esperado (caso `filter`/`find`
    exigen `Bool`). Callback `Any` pasa sin chequear (gradual).
- Política sobre método desconocido:
  - Receptor built-in concreto (`List`/`Map`/`Str`) → error con
    el nombre del receptor y del método. Captura typos
    (`xs.lenght()`).
  - Receptor `Nominal(id)` → `None`; la llamada cae a gradual
    (Any) sin chequear. Los métodos custom sobre `type` siguen
    siendo deuda de 3.2; no rompemos código que los use.
  - Receptor `Any` → `None`, gradual.
  - Otro tipo concreto (Int, Bool, Range, Result, etc.) →
    error "no tiene el método X". El evaluator también lo
    rechazaba en runtime; ahora se atrapa estáticamente.
- Impacto sobre 5.3.3: `xs.find(...)` y `m.get(k)` antes eran
  `Any` y ahora son `Result<T>` / `Result<V>` concretos. Eso
  hace que:
  - `users.find(...)?` opere sobre `Result<User>` concreto en
    vez de gradual. La regla "fn contenedora retorna Result"
    sigue chequeando — si la fn no declara return_type
    (`return_stack.last() == Any`), no dispara. Los ejemplos
    existentes (server.fitz `update_user`) no declaran return
    type así que siguen pasando.
  - `match users.find(...) { Ok(u) ... Err(_) ... }` ahora pasa
    por el chequeo de exhaustividad de 5.3.3 (antes el
    scrutinee Any no exigía exhaustividad). Los matches
    existentes son todos `Ok + Err`, completos.
- E2E: los 17 ejemplos + `examples/server.fitz` pasan
  `fitz check` sin regresiones — la mayor parte del código
  built-in en los ejemplos era invisible al checker hasta acá,
  y ahora se valida sin warnings nuevos.

Tests al cerrar 5.3.4: 767 (742 al cerrar 5.3.3 + 25 nuevos en
`types.rs`: List push/pop/len/map/filter/find con tipos
compatibles, incompatibles, aridad incorrecta, callback sin
anotaciones (gradual), callback param incompatible; Map
get/has/keys/values/len con tipos compatibles e incompatibles;
Str upper/lower/len; método desconocido sobre cada built-in
(typos), método sobre Int (error), método sobre Nominal sin
chequeo (gradual), encadenado `xs.map(...).filter(...)` en una
sola línea).

**Deuda explícita — retomar después:**
- **FnExpr.ret inferido del body** — los callbacks inline
  (`fn(x) => x * 2`) hoy tienen `ret = Any`. Eso significa que
  `.filter(fn(x: Int) -> Int { ... })` con ret no-Bool no se
  detecta si el callback es FnExpr inline (sí se detecta si
  viene como Function declarada con ret concreto). Lo cubre
  5.3.5.
- **`Expr::Index`** (`xs[i]`, `m[k]`) sigue devolviendo `Any`.
  Es un paso análogo a métodos built-in pero independiente;
  candidato a 5.3.5 o sub-paso separado.
- **Encadenamiento multi-línea** (`xs.map(...)\n.filter(...)`)
  sigue siendo deuda explícita del parser (3.4). El test usa
  forma de una sola línea.
- **Métodos custom sobre `type`** — sigue deuda de 3.2.

##### 5.3.5 — FnExpr.ret inferido + Expr::Index + cierre de 5.3 ✓
**Completado** — último paso del checker de expresiones. Cierra
la sub-fase 5.3 entera.

- `CheckCtx` gana `inferred_returns: Vec<Vec<Type>>` paralelo a
  `return_stack`. Cada frame recolecta los tipos sintetizados
  de los `Stmt::Return` del body de su función. `Expr::FnExpr`
  lo consume al salir para sintetizar `ret` vía `unify_returns`
  + `lub`. `Stmt::FnDef` también pushea un frame por
  consistencia pero descarta el contenido (ya tiene
  `return_type` declarado; la unificación queda disponible para
  un eventual check futuro "declarado vs inferido"). `Stmt::Return`
  pushea su tipo al frame de la fn contenedora.
- `lub(a, b)`: "least upper bound" pragmático para unificar
  tipos de ramas distintas de un `return`. Reglas:
  - `a == b` → `a`.
  - Cualquiera Any → el otro (Any cede al concreto).
  - Int + Float → Float (coerción).
  - Null + T → `T?` (caso típico de "una rama devuelve null").
  - T + T? → `T?`.
  - Generics built-in (`List`/`Map`/`Result`/`Nullable`)
    recursivos.
  - Mix arbitrario → Any.
  No es un lattice formal — prioriza preservar información
  útil (`lub(Result<User>, Result<Any>) = Result<User>`).
- `unify_returns(types)`: fold con `lub`. Lista vacía → `Null`
  (matchea la semántica del evaluator: una fn que termina sin
  `return` explícito devuelve `Value::Null`).
- Caso clave destrabado: `xs.filter(fn(x: Int) => x * 2)`
  ahora detecta que el ret inferido del callback (`Int`) no es
  el `Bool` que filter exige. El test correspondiente que
  abandonamos en 5.3.4 volvió.
- **`Expr::Index`** (`xs[i]`, `m[k]`): deja de devolver `Any`
  silenciosamente.
  - `List<T>[Int]` → `T`. Índice no-Int → error.
  - `Map<K, V>[K]` → `V`. Índice incompatible con K → error.
  - `Str[?]` → error "no soporta indexing todavía" (deuda 3.1
    sobre unidad char/byte/grafema).
  - `Any` o `Nominal` → `Any` (gradual; los indexers custom
    sobre `type` no existen todavía).
  - Otro tipo concreto → error "no soporta indexing".

Tests al cerrar 5.3.5: 784 (767 al cerrar 5.3.4 + 17 nuevos en
`types.rs`: FnExpr ret inferido para arrow/block/sin-return,
lub con Int+Float, Null+T, Result+Result; Index sobre
List/Map/Str/Int/Any con tipos compatibles e incompatibles;
helpers `lub` y `unify_returns` directos).

Verificación end-to-end: los 17 ejemplos + `examples/server.fitz`
pasan `fitz check` limpios. El cambio más sensible (FnExpr.ret
real) no rompe nada porque todos los callbacks de filter/find
en los ejemplos retornan `Bool` (`u.id == id`, `n == 2 or n == 4`,
etc.); los de map retornan tipos arbitrarios sin chequeo de ret
forzado.

**Cierre formal de 5.3 — Type checker de expresiones y
funciones:**

El checker estático cubre hoy el lenguaje observable:
literales, ident, operadores aritméticos/lógicos/comparación
con coerción Int↔Float, StrInterp, control de flujo
(if/while/for/loop), list/map/struct literals, field access,
match (con exhaustividad sobre Result), Range, Ok/Err, `?`,
struct lit, llamadas a fn/builtin con aridad y tipos,
`Stmt::Return` contra `return_type`, métodos built-in
paramétricos para List/Map/Str (14 métodos), FnExpr con `ret`
inferido del body, y `Expr::Index` sobre receptores conocidos.
La regla gradual (`Any` cede a cualquier tipo) preserva el
modelo del lenguaje sin obligar a anotar.

**Deuda explícita — pendientes naturales que NO bloquean 5.4:**
- **Métodos custom sobre `type`** — deuda vieja de 3.2. El
  dispatch del checker está preparado para sumar otra fuente
  de lookup sin retoques.
- **`Pattern::OkWildcard` / `ErrWildcard`** — deuda de 3.3.
  Hoy `Ok(_)` parsea como `OkBinding("_")` y ensucia el scope.
- **`Result<X>?` como scrutinee de match** — no exigido,
  decisión de diseño. Revisitable si aparece como caso real.
- **Patrones imposibles sobre Result** (literal `Int` en match
  sobre `Result<T>`) — son dead code; el chequeo es independiente.
- **Encadenamiento multi-línea en method chains** (deuda 3.4
  del parser) — `xs.map(...)\n.filter(...)` corta en el
  newline; el checker funciona pero la sintaxis no llega.
- **Posiciones de error** — `TypeExpr` y muchos `FitzError` del
  checker todavía salen sin línea/columna. Refactor amplio,
  pendiente.
- **FnExpr con tipo de retorno declarado en sintaxis**
  (`fn(x: Int) -> Int { ... }` como expresión) — el AST y el
  parser no lo modelan. Hoy el ret se infiere siempre. Cuando
  aparezca la sintaxis, el checker compara declarado vs
  inferido sin trabajo extra.

#### 5.4 — Modo strict en `fitz run` + cierre de 5a ✓
**Completado** — flip del default de `fitz run`: ahora aborta
cuando el checker estático encuentra errores, en lugar de
emitirlos como warnings y seguir. Cierra formalmente la
sub-fase 5a (Type checker estático sobre el intérprete).

- `src/main.rs`:
  - Nueva flag `--no-typecheck` en la variante `Run` del enum
    `Commands` (`#[arg(long)]`). Sin la flag, modo strict; con
    la flag, los errores se reportan como warnings y el
    programa se ejecuta igual.
  - Strict (default): mensaje `✗ <archivo> — N error(es) de
    tipo:` + lista de errores + sugerencia `Usá \`fitz check\`
    para revisar, o \`fitz run --no-typecheck <archivo>\` para
    correr igual.` Exit code 1. El evaluator nunca arranca.
  - Gradual (`--no-typecheck`): mensaje `⚠ N warning(s) del
    checker de tipos (modo \`--no-typecheck\`):` + lista. Sigue
    al evaluator.
- `examples/guide/15-errores.fitz` reescrito: cambió de
  aridad-incorrecta (que el checker ahora detectaba desde
  5.3.2) a división por cero (error de runtime puro que el
  sistema de tipos no analiza por diseño). Sigue cumpliendo el
  rol pedagógico del capítulo — mostrar cómo se ve un error de
  runtime — sin entrar en conflicto con el modo strict del
  checker.
- `docs/guide.md` cap 15 reorganizado: documenta cuatro etapas
  (lexer → parser → **checker** → evaluador) en lugar de tres;
  suma sección "Modo strict y `--no-typecheck`"; tabla nueva
  de errores típicos del checker; ejemplo final de runtime
  puro. Cap 3 actualizado (anotaciones de tipo se chequean en
  compile time). Cap 18 actualizado: Fase 5a cerrada, 5b
  (codegen) como próximo bloque. Header bumpeado.

Sin tests automatizados nuevos en 5.4 — el feature es CLI y la
infraestructura para tests CLI no existe todavía. Verificado a
mano:
- Programa con error de tipo → `fitz run` aborta con exit 1.
- Mismo programa con `--no-typecheck` → ejecuta con warning.
- `examples/guide/15-errores.fitz` pasa `fitz check` (división
  por cero no es tipo) y `fitz run` emite `Error — división
  por cero` desde el evaluator.
- Los 14 ejemplos no-HTTP de la guía corren limpios con
  `fitz run`.

Total al cerrar 5.4: 784 tests (sin cambios respecto de 5.3.5).

**Cierre formal de Fase 5a — Type checker estático:**

5a queda completada. El checker cubre la sintaxis completa
observable del lenguaje hoy: anotaciones en variables,
parámetros, return types y campos de `type`; expresiones de
todos los nodos del AST; llamadas con aridad y tipos contra la
signature declarada; `Stmt::Return` contra return_type
declarado; operador `?` con la regla de fn contenedora;
exhaustividad de `match` sobre `Result<T>`; métodos built-in
paramétricos sobre `List`/`Map`/`Str` (14 métodos); `Expr::Index`
sobre `List`/`Map`; inferencia básica (synthesis + `lub` para
FnExpr.ret).

Lo que queda abierto para futuras fases (no bloquea 5b):
- Métodos custom sobre `type` (deuda 3.2; dispatch del checker
  preparado).
- ~~`Pattern::OkWildcard` / `ErrWildcard` (deuda 3.3).~~ ✓
  Cerrada en el paso de deuda residual post-5a: el parser
  reconoce `Ok(_)` y `Err(_)` como wildcards dedicados, el
  evaluator matchea sin bindear y el checker los cuenta para
  exhaustividad.
- Patrones imposibles sobre Result (dead-code check separado).
- Encadenamiento multi-línea en method chains (deuda 3.4 del
  parser).
- ~~Reasignación sin anotación contra tipo previo (`m: Int = 1;
  m = "x"` no chequea — el binding se relaja al tipo nuevo).~~
  ✓ Cerrada en el paso de deuda residual post-5a: `VarBinding`
  ahora guarda un flag `annotated`; reasignaciones sin
  anotación contra una var anotada se chequean contra el tipo
  declarado.
- Posiciones de error precisas en TypeExpr y errores del
  checker. **Pospuesta para post-5b**: el refactor amplio del
  AST (posiciones en `Expr`/`Stmt`/`TypeExpr` con propagación
  desde el parser) cubre los errores de anotación pero no los
  de expresiones; mejor combinarlo con la infra del IR tipado
  que va a sumar 5b. Hoy los errores del checker reportan
  posición `0:0` con mensajes descriptivos como puente.
- FnExpr con return type declarado en sintaxis (AST/parser).
- Sugerencias "did you mean..." en typos.

5b arranca con un IR tipado encima de lo que produce este
checker.

#### 5b.1 — Codegen a binario nativo (subset primitivo) ✓
**Completado** — primer paso de Fase 5b. Transpile AST de Fitz →
código Rust → binario via `rustc`. Cubre programas CLI con
primitivos, sin tipos compuestos, sin HTTP.

**Backend elegido — transpile-a-Rust** sobre Cranelift/LLVM:
- Reusamos el compilador de Rust completo. Optimizaciones (LTO,
  inlining) gratis.
- Cross-compile a todos los targets de rustc sin trabajo extra.
- `async fn` Fitz puede mapear a `async fn` Rust cuando llegue
  5b.6 (HTTP / async real).
- Type-safety: si el codegen tiene un bug, rustc lo va a cazar.
- Trade-off explícito: compile times = los de rustc (~2s para
  programas pequeños). Para servicios web —el público objetivo
  de Fitz— se hace una vez por deploy, no es interactive.

**Nuevo módulo `src/codegen.rs`**:
- `pub fn generate_rust(program: &Program, env: &TypeEnv) ->
  Result<String, FitzError>` — entry point.
- `CodegenCtx` con stack de scopes (`HashMap<String, Type>`) y
  tabla `fn_sigs` de firmas pre-registradas. Las firmas top-level
  se computan **antes** de generar cuerpos, así las llamadas
  resuelven el return type sin importar el orden de las fns.
- Visitor sobre AST tipado. No introducimos IR intermedio en
  5b.1: para un subset chico, un visitor a un buffer `String`
  alcanza. Cuando 5b.2+ traiga tipos compuestos posiblemente
  sumemos uno.
- Helpers: `coerce(code, from, to)` para Int→Float, `numeric_coerce`
  para BinOp con tipos mixtos, `rust_type_for(t)` para mapear
  primitivos, `rust_str_literal(s)` para literales escapados,
  `type_name(t)` para mensajes.

**Mapping AST de Fitz → Rust**:

| Fitz | Rust |
|------|------|
| `Int` | `i64` |
| `Float` | `f64` |
| `Str` | `String` |
| `Bool` | `bool` |
| `Null` | `()` |
| `let x: Int = 42` | `let mut x: i64 = 42i64;` |
| `let x = 1` (inferido) | `let mut x: i64 = 1i64;` (usa tipo del checker) |
| `"hola {x}"` | `format!("hola {}", x)` |
| `s1 + s2` (Str) | `format!("{}{}", s1, s2)` |
| `1 + 2.0` | `((1i64 as f64) + 2f64)` |
| `print(a, b)` | `println!("{} {}", a, b)` |
| `for i in 0..3` | `for mut i in (0i64 as i64)..(3i64 as i64)` |
| `fn f(n: Int) -> Int { ... }` | `fn f(mut n: i64) -> i64 { ... }` |

Convenciones:
- Variables siempre `let mut` para simplificar reasignación.
  Reasignación detectada mirando si la var ya existe en algún
  scope visible (no solo el top); si sí, emite `x = ...` en
  vez de `let mut x = ...`.
- Strings se concatenan siempre con `format!` para evitar los
  juegos de ownership de `String + &str`. Ineficiente pero
  correcto.
- Strings pasados como args usan `.clone()` (ineficiente pero
  evita refactor de ownership).
- Coerción `Int → Float` se inserta como `(x as f64)` en cada
  punto donde se necesita (BinOp mixto, asignación a Float
  anotado, paso de Int a param Float).
- `print()` sin args → `println!()` (newline).
- `print(a, b, c)` → format string con `{}` separados por
  espacio, replicando la semántica del intérprete.

**Subset soportado en 5b.1**:
- Literales Int / Float / Str / Bool / Null.
- BinOp (aritméticos con coerción, comparación numérica y de
  Str via `.as_str()`, lógicos `and`/`or`).
- UnaryOp `Neg`.
- StrInterp.
- Asignación con o sin anotación, reasignación.
- `if`/`else` como sentencia.
- `while`, `loop`, `break`, `continue`.
- `for var in start..end` (rangos exclusivos).
- Funciones top-level con params/return tipados.
- `print()` builtin.
- `return` explícito.

**Fuera de scope (refinamos en pasos siguientes con errores
explícitos)**:
- Tipos custom, struct lit, field access → 5b.2.
- Listas, mapas, indexing → 5b.3.
- `Result`/`?`/`match` → 5b.4.
- Módulos → 5b.5.
- HTTP / `@server` / handlers → 5b.6.
- Funciones anónimas (FnExpr) → 5b.2 o 5b.3.
- Decoradores → error "5b.6".

**Subcomando `fitz build`** (antes era stub que imprimía "🚧"):
- Flow: lex → parse → checker en **modo strict siempre** (no hay
  `--no-typecheck` en build; build exige programa correcto) →
  `codegen::generate_rust` → escribe `target/fitz-build/<nombre>/main.rs`
  (visible para debug) → invoca `rustc --edition 2021 -O <main.rs>
  -o <bin>` → copia el binario adyacente al .fitz fuente.
- Naming: `hello.fitz` → `hello.exe` (Windows) o `hello`
  (Linux/macOS).
- Sin Cargo todavía — 5b.1 no tiene dependencias externas. Cuando
  llegue 5b.4+ con serde (para Result) o 5b.6 con axum/tokio,
  pasamos a generar `Cargo.toml + src/main.rs` y llamar
  `cargo build --release`.

**Tests**:
- **28 tests unitarios** en `codegen.rs`: cubren cada feature
  del subset (literales, BinOp con coerciones, StrInterp, print,
  fns top-level con block y arrow, llamadas, if/while/for/loop,
  reasignación, UnaryOp, lógicos, comparación Str con
  `.as_str()`) + cada feature fuera de scope con error
  específico mencionando el sub-paso futuro.
- **8 tests E2E** en `tests/compile_e2e.rs` (integration tests):
  invocan `fitz build` sobre programas reales, ejecutan el
  binario, comparan stdout y exit code. Cubren: criterio de éxito
  hello-world, `if`/`else`, `while`+reasignación, `for`-range,
  coerción Int→Float, recursión, build aborta con error de tipo
  strict, build aborta sobre feature no soportada (lista). Usan
  un `Mutex` global para serializar las invocaciones de rustc
  (múltiples rustc paralelos sobre el mismo target dir producían
  cross-talk de outputs en Windows).

**Criterio de éxito**:
```fitz
let name = "Fitz"
let x = 10 + 5
print("Hola, {name}, x es {x}")

fn double(n: Int) -> Int => n * 2
print(double(x))
```
```bash
fitz build hello.fitz
./hello
# Hola, Fitz, x es 15
# 30
```
Validado a mano y vía test E2E.

Tests al cerrar 5b.1: **833** (797 al cerrar deuda residual + 28
unit codegen + 8 E2E = 833). Los 17 ejemplos + `server.fitz`
pasan `fitz check` limpios sin cambios.

**Deuda explícita — retomar en pasos siguientes**:
- **`Type::Any` en variables sin anotación inferible**: el
  codegen exige conocer el tipo. Si el checker no pudo
  sintetizar (caso raro en 5b.1), error. En 5b.2+ podemos
  refinar a `Box<dyn Any>` o pedir anotación al usuario.
- **Vars declaradas adentro de bloques quedan confinadas en
  Rust**: en Fitz `while { x = 5 }` deja `x` definida afuera;
  en el binario generado, no. Discrepancia conocida; cierra
  con pre-declaración de vars en el outer scope si se pide.
- **Compile time del binario**: ~2s para programas chicos por
  `rustc` cold. Aceptable para 5b.1; si molesta en 5b.6+ con
  axum, pasamos a `cargo build` que cachea.
- **Strings pasados con `.clone()` siempre**: ineficiente.
  Optimización post-5b cuando estabilicemos el modelo de
  ownership en codegen.

##### 5b.2 — Tipos custom + field access + struct literal ✓
**Completado** — segundo paso de Fase 5b. `type Foo { ... }` se
transpila a `struct FooData { ... }` con type alias
`type Foo = Rc<RefCell<FooData>>;` para preservar la semántica
de referencia compartida del intérprete. Trade-off conocido:
field access caro (`u.borrow().name.clone()`), optimizable
post-5b.

Cubierto:
- Struct literal con defaults inline-eados y wrapping
  `Some(...)`/`None` automático para campos nullables.
- Field access (`u.name`) con `.clone()` selectivo según
  `needs_clone` (Str/Nominal/Nullable clonan, primitivos Copy no).
- Field assignment (`u.name = "x"`).
- Igualdad estructural entre instancias (`==`/`!=`) via
  `#[derive(PartialEq)]` — `Rc<RefCell<T>>` compara por
  contenido, recursando en campos nominales anidados igual que
  el intérprete.
- Tipos custom como campo de otro tipo custom
  (`type Order { user: User? }`).
- Pasaje a/desde funciones: el `Rc` se clona en cada uso de
  `Ident` Nominal — el clone es del puntero refcontado, así que
  preserva aliasing.
- `Display for FooData` reproduce el formato del intérprete
  (strings con comillas, Float con `.0`, Nullable como `null`,
  nominales recursivos).

Bonus que entraron en el mismo bloque (cierran deuda chica de
5b.1):
- **`if` como expresión con valor**: cuando ambas ramas terminan
  en `Stmt::Expr` no-`print`, `gen_if_expr` emite el `if` como
  expresión Rust con tail sin `;`. LUB simple: `Int+Float→Float`,
  `T+Null→T?`. Statement-mode preservado para `if cond { print(x) }`.
  Cierra deuda de 5b.1 sobre `let x = if cond { a } else { b }`.
- **Métodos built-in sobre Str**: `s.len()`→`chars().count()`,
  `s.upper()`→`to_uppercase()`, `s.lower()`→`to_lowercase()`.
  Despacho por callee `Expr::Field { object, field }`.
- **StrInterp con Null/Float/Nominal/Nullable**: alineado al
  formato del intérprete. `"x es {null}"` ahora produce `"x es
  null"` (antes rustc no compilaba por `()` sin Display).

Deuda explícita marcada con error de codegen claro:
- Métodos custom sobre `type`: depende de cerrar la deuda de 3.2
  en parser/AST (hoy `Stmt::TypeDef` solo guarda `fields`, sin
  bloque de métodos).
- Tipos importados: hasta 5b.5.
- Listas/mapas (literales, indexing, métodos): hasta 5b.3.
- `Result`/`?`/`match`: hasta 5b.4.

Tests: 21 unit nuevos en `src/codegen.rs` + 14 E2E nuevos en
`tests/compile_e2e.rs`. Total acumulado: 868 (846 unit + 22 E2E).
Validado a mano contra `fitz run`: `examples/types.fitz`,
`examples/guide/05-strings.fitz`, `examples/guide/07-if.fitz`
y `examples/guide/12-type.fitz` producen output idéntico.

##### 5b.3 — Listas, mapas, indexing, method calls ✓
**Completado** — tercer paso de Fase 5b. `List<T>` →
`Rc<RefCell<Vec<T>>>` y `Map<K, V>` → `Rc<RefCell<Vec<(K, V)>>>`:
mismo modelo de referencia compartida que 5b.2 (Nominal). Orden de
inserción preservado por Vec. Aliasing por referencia: `xs.push(x)`,
`xs[i].name = "x"` vía cualquier alias se ve en la colección original.
Trade-off conocido (paralelo a 5b.2): `xs[i]` → `xs.borrow()[i as
usize].clone()`. Optimizable post-5b.

Cubierto:
- **Literales**: `[e1, e2, ...]` con coerción de cada item al tipo
  común (LUB pragmático: Int↔Float→Float, T↔Null→T?, T?↔T→T?,
  recursivo en generics built-in). `{k: v, ...}` análogo a `Vec<(K, V)>`.
  Vacíos sintetizan `List<Any>`/`Map<Any, Any>` y el contexto
  (anotación destino) los resuelve.
- **Heterogéneos irrecuperables** (`[1, "dos"]`, `{"a": 1, "b": "x"}`)
  → error de codegen con mensaje claro. El subset compilado no
  soporta tagged unions runtime.
- **Indexing** `xs[i]` (List) y `m[k]` (Map). List: borrow + clone
  del item (clone del Rc para Nominal/List/Map → preserva
  aliasing). Map: búsqueda lineal con `panic!` si la clave falta,
  mensaje idéntico al del intérprete. Para evitar E0716 con `Rc`
  temporales, el bloque liga primero el Rc a una var local antes
  del `.borrow()`.
- **`for v in xs`** sobre `List<T>`: snapshot via `borrow().clone()
  .into_iter()` para evitar re-entrancia si el body muta la lista.
  Map como iterable directo NO se soporta (alineado con el
  intérprete).
- **Métodos**: `push`, `pop`, `len`, `map`, `filter` sobre List;
  `has`, `keys`, `values`, `len` sobre Map. `pop` paniquea sobre
  lista vacía con el mensaje del intérprete. `keys`/`values`
  devuelven `List<K>`/`List<V>` envuelto en Rc, permitiendo
  method chaining.
- **`find` (List) y `get` (Map)** devuelven `Result<T>` y se
  difirieron a 5b.4 con error de codegen específico mencionando
  "5b.4". El cap 13 entero (que usa find + match) queda bloqueado
  hasta 5b.4; la versión reducida sin find/match compila bit-a-bit.
- **Builtin global `len(x)`**: despacha por tipo del argumento
  (Str → `chars().count()`, List/Map → `borrow().len()`). Una fn
  `len` definida por el usuario gana — `fn_sigs` se chequea antes
  del builtin.
- **FnExpr inline como callback** de `.map(...)` y `.filter(...)`:
  emite Rust closure tipado `|p: T| -> U { body }`. Inferencia
  mini-LUB del ret type sobre el primer `Stmt::Return` del body
  (o último `Stmt::Expr` no-print). **Higher-order completo**
  (FnExpr como var, param o retorno) → error explícito con
  referencia a "sub-paso posterior". El cap 11 (closures,
  `make_adder`, `apply`) queda como deuda visible.
- **`print`/interpolación de List/Map**: formato bit-a-bit
  idéntico al intérprete (`[1, 2, 3]`, `{"a": 1, "b": 2}`,
  strings entre comillas adentro vía `show_expr_inline`). El
  bloque inline liga el Rc a `let __list`/`let __map` antes del
  `.borrow()` (vida del temporal); itera con `.iter().cloned()`
  para que `__it` venga por valor.
- **`lub_for_if` renombrado a `lub`** y extendido recursivamente
  para generics built-in (List/Map/Result/Nullable). Reusado
  desde if-as-expression (5b.2) y desde unificación de items de
  literales (5b.3).

Tests: 28 unit nuevos en `src/codegen.rs` (literales, indexing,
métodos, FnExpr inline, builtin global, print bit-a-bit, errores
explícitos) + 7 E2E nuevos en `tests/compile_e2e.rs` (push+len+
for, indexing+pop, mapa has/keys/values/len, lista de instancias +
alias del cap 13, chain `.filter().map()`, promoción Int→Float,
lista heterogénea aborta). Total acumulado: **902 tests** (873
unit + 29 E2E). El test viejo `listas_no_soportadas` se reemplazó
por `listas_heterogeneas_son_error`; el E2E
`build_aborta_si_codegen_no_soporta_feature` ahora apunta a
`Ok(...)` (Result, 5b.4).

Validado a mano contra `fitz run`: `examples/guide/09-listas-mapas.fitz`
reducido al subset compilable (sin Range como valor, sin mezclas
heterogéneas, con anotaciones) y `examples/guide/13-metodos.fitz`
reducido (sin find/match/get) producen output idéntico bit-a-bit.

**Deuda explícita — retomar en pasos siguientes**:
- **Heterogéneos**: introducir un `FitzValue` runtime tagged si
  el caso aparece como bloqueante en la práctica. Por ahora se
  resuelve con `fitz run`.
- **find/get → Result**: se desbloquean en 5b.4.
- **Higher-order completo**: closures que escapan, FnExpr como
  var/param/retorno. Probable sub-paso 5b.4.5 o post-5b.6 con
  `Box<dyn Fn(...)>` + captura por clone explícita.
- **Range como valor / `print(range)`**: deuda residual de 5b.1
  (sigue siendo error de codegen). Cierra cuando lo necesite un
  ejemplo concreto.

##### 5b.4 — Result, `?`, match ✓
**Cerrado.** Cuarto paso de Fase 5b. Desbloquea el cap 13 entero
(find + match + get) y la mecánica completa de `Result` en el
compilador.

**Decisión clave**: `Result<T>` Fitz → `Result<T, String>` Rust
nativo — el Err side está **pinned a `String`**. Trade-off
aceptado: `Err(42)` o cualquier inner no-Str se coerce con
`format!("{}", x)` a String. Justificación:
- Todos los ejemplos de la guía y `examples/server.fitz`
  construyen `Err(...)` con strings literales.
- El intérprete mismo emite `Value::Str` desde `find`/`get`/
  divisiones por cero.
- Encaje natural con el `?` Rust (E = String en ambos lados):
  propagación sin glue.
- Encaje natural con HTTP 5b.6: el handler serializa Err como
  `{"error": <inner>}`, y un String se mapea directo.

**Alternativa rechazada — tagged `FitzValue` runtime**: añade un
módulo de tipos boxed + Display + Eq custom, fuera del scope de
5b. Reabrible post-5b si aparece presión real.

**Implementación**:
- `rust_type_for(Result<T>)` → `Result<T, String>`. T = Any
  (Err suelto sin contexto) → `Result<_, String>`, rustc infiere
  desde la anotación destino.
- `gen_expr` para `Ok(e)`: emite `Ok(<coerced e>)`. Tipo
  sintetizado `Result<T>` donde T es el tipo del inner.
- `gen_expr` para `Err(e)`: emite `Err(e.to_string())` si el
  inner es Str, o `Err(format!("{}", e))` si no. Tipo Fitz
  sintetizado: `Result<Any>` — el contexto destino refina.
- `gen_expr` para `Try(e)` (operador `?`): emite `(<expr>)?`
  Rust nativo. Nuevo `CodegenCtx.ret_stack: Vec<Type>` con
  push/pop en `gen_top_fn` y `gen_callback_inline` para
  validar que el contenedor retorna `Result<...>` (o `Any`,
  para el escape gradual). Top-level `?` y `?` en fn con ret
  concreto distinto de Result → error de codegen explícito.
- `gen_match`: emite el match siempre como **expresión Rust**
  (`(match s { ... })`); en stmt position, el `;` de `Stmt::Expr`
  lo cierra. Patrones soportados:
  - Literales Int/Float/Bool/Null directos como pattern Rust.
  - Str via guard `ref __s if __s.as_str() == "..."` (Rust no
    acepta `"x"` contra `String` directo).
  - Ident (binding), Wildcard.
  - Ok(x)/Err(e)/Ok(_)/Err(_) — Rust nativos.
  - Range `a..b` via guard `__n if (a..b).contains(&__n)`.

  **Exhaustividad**: si los arms no cubren todo (sin Ident/
  Wildcard ni cobertura completa Ok+Err sobre scrutinee Result),
  agregamos arm `_ => panic!("el `match` no matcheó ningún
  brazo")` — mismo mensaje del intérprete.

  **Bodies con `print(...)`**: como `print` no es expresión en
  Fitz, los emitimos como bloque stmt-wrapped `{ println!(...); }`.
  Detalle pequeño, importante para que arms de match con `print`
  compilen.
- **find/get**: los errores "5b.4" desaparecen. `.find(callback)`
  sobre `List<T>` emite loop que devuelve `Ok(item)` al primer
  match, `Err("no encontrado".to_string())` si nada matchea
  (mensaje idéntico al intérprete). `.get(k)` sobre `Map<K,V>`
  devuelve `Ok(v)` o `Err(format!("clave no encontrada: {}", k))`.

  Detalle clave detectado en validación bit-a-bit del cap 13:
  la clave en el mensaje se formatea con `show_expr` (modo
  Display de Value, sin comillas para Str), no con
  `show_expr_inline` (que sí mete comillas — solo aplica
  adentro de listas/mapas).
- **`print` de Result**: `show_expr` agrega caso `Type::Result(_)`
  con sub-match inline que emite `Ok(<inline T>)` / `Err("<msg>")`
  con comillas dobles alrededor del mensaje. Bit-a-bit como el
  intérprete (que usa `write_inline_value` con `Value::Str` →
  comillas).
- **`needs_clone(Result<_>)`** → `true` (Result no es Copy).
- **`lub`**: agregamos caso `Result(a) ↔ Result(b)` recursivo y
  caso `Any ↔ T → T` (Err sin contexto unifica con `Ok(<T>)`).

**Tests**: 15 unit nuevos en `src/codegen.rs` (15 nuevos − 3
viejos reemplazados que esperaban error "5b.4"; los reapunté a
testear el nuevo comportamiento) + 6 E2E en `tests/compile_e2e.rs`
(cap 14 con anotaciones, `?` propagation, find+match, get+match
con clave faltante, print Result, match-range). Reapunté el E2E
"feature no soportada en codegen" desde `Ok(...)` a `import`
(5b.5).

**Validación bit-a-bit**:
- `examples/guide/13-metodos.fitz` entero (find + match + get)
  compila contra `fitz run` — salida idéntica.
- `examples/guide/14-result.fitz` se actualizó con anotaciones
  de tipo en las fns (`divide(a: Int, b: Int) -> Result<Int>`,
  etc.) para que también compile end-to-end. Las anotaciones
  son didácticas — refuerzan el contrato `Result<T>` que las
  fns ya respetaban implícitamente. Salida bit-a-bit idéntica
  al run anterior.

**Deuda residual que sigue post-5b.4**:
- **Higher-order completo**: closures que escapan, FnExpr como
  var/param/retorno → post-5b.6 con `Box<dyn Fn(...)>` + clone
  explícito en captura.
- **`?` adentro de FnExpr inline**: el codegen no maneja el caso
  (el callback hoy no tiene un return type "Result" propio).
  Ningún ejemplo lo usa; queda como deuda visible.
- **Inferencia de tipos de params de fns sin anotar**: deuda
  vieja de 5b.1. Bloquea compilación de fns como
  `fn divide(a, b) { ... }`. Workaround: anotar.
- **Posiciones de error precisas en codegen**: deuda
  pospuesta de 5a.

##### 5b.5 — Módulos / `import` ✓
**Cerrado.** Quinto paso de Fase 5b. Cierra la brecha más visible
entre `fitz run` y `fitz build`: los programas con `import foo` /
`from foo import X` ya compilan a binario nativo. Habilita el cap
16 de la guía end-to-end con `fitz build`.

**Decisión clave de pipeline**: pasamos de `rustc` directo a
**siempre generar un Cargo project**. Trade-off aceptado: la
primera compilación cuesta ~1-2s más. Justificación:
- Los imports cross-archivo necesitan múltiples `.rs` con `mod`,
  que es la abstracción nativa de Cargo.
- Cuando llegue 5b.6 con axum/tokio, las deps se suman al
  `Cargo.toml` generado sin reescribir pipeline.
- Cargo cachea incremental — segunda compilación rápida.

**Estructura del project generado**:
```
target/fitz-build/<stem>/
├── Cargo.toml         # [package] / [bin] / sin deps por ahora
└── src/
    ├── main.rs        # mod foo; use foo::{...}; + fn main()
    ├── foo.rs         # pub fn / pub struct / pub type / pub const
    └── ...
```
El binario final se copia adyacente al `.fitz` original. Sanitización
del nombre del crate: `02-hola.fitz` → crate `fitz_02-hola`, binario
adyacente `02-hola.exe` (el stem original se preserva).

**Implementación del codegen**:
- Nuevo `ModuleLoader` con cache por path canonicalizado y
  stack de loading para detección de ciclos. Para cada import,
  lee + lexea + parsea + chequea el módulo y lo genera como
  Rust en modo `Module` (todo `pub`, sin `fn main()`).
- Nuevo `GenMode { Main, Module }` en el `CodegenCtx`: `Module`
  marca todas las defs top-level como `pub` y soporta
  `let X = <literal>` → `pub const`/`pub static`.
- **Bindings cross-module**:
  - `import foo` → `mod foo;` + binding namespace. `foo.greet(x)`
    se traduce a `foo::greet(x)` Rust.
  - `from foo import User` → `mod foo;` + `use foo::{User,
    UserData};`. Permite usar `User { ... }` en el importer
    porque el codegen importa el data struct también.
  - `from foo import greet` (fn) → `use foo::greet;` con la firma
    del módulo para resolver la llamada.
  - `from foo import PREFIX` (const Str) → `use foo::PREFIX;`,
    consumido como `String::from(PREFIX)`.
- **Enriquecimiento de TypeEnv del importer**: el checker
  registra los tipos importados sin fields (no carga el módulo).
  El codegen copia los fields del módulo cargado al
  `fields_by_id` del importer, manteniendo el `TypeId` del
  importer — así `User { id: 1 }` y `u.id` resuelven correcto.
- **Top-level del módulo**:
  - `type X { ... }` → `pub struct XData` + `pub type X = ...`
    + `impl Display`.
  - `fn f(p: T) -> U` → `pub fn` (anotaciones requeridas —
    deuda 5b.1).
  - `let X = <literal>` → `pub const X: T` (primitivos) o
    `pub static X: &str` (Str). El módulo pre-registra las
    consts antes de emitir bodies, así una fn del módulo puede
    referenciar la const en su cuerpo.
  - RHS no literal o stmts no `type`/`fn`/`let` → error de
    codegen citando "5b.5" como deuda.

**Limitaciones aceptadas en 5b.5**:
- **Imports transitivos no soportados**: un módulo cargado por
  el main no puede tener su propio `import`. Loader aborta con
  mensaje claro citando 5b.5 como deuda residual. Workaround:
  aplanar imports al main.
- **`fitz build` con cap 14 / cap 16**: ambos requieren
  anotaciones de tipo en fns. El cap 16 (`guide_utils.fitz`) se
  actualizó: `fn greet(name: Str) -> Str => ...`. El intérprete
  sigue infiriendo.

**Tests**: 6 unit nuevos en `src/codegen.rs` (modo Module emite
`pub` en struct/alias/fn, `let` top-level → static/const, RHS no
literal aborta, fn body referencia const local) + 5 E2E nuevos
en `tests/compile_e2e.rs` (`from import` type+fn, `from import`
const Str, `import` namespace con fn, módulo inexistente aborta,
módulo con `import` propio aborta por transitividad). Reapunté
el E2E "feature no soportada en codegen" desde `import` a `@get`
(5b.6).

**Validación bit-a-bit**: `examples/guide/16-modulos.fitz` +
`guide_utils.fitz` (con `greet` anotado) compilan con `fitz
build` y producen output idéntico a `fitz run`.

**Deuda explícita que sigue post-5b.5**:
- **Imports transitivos**: un módulo importado puede tener
  imports propios. Quitar la restricción requiere recursar el
  loader sin perder el binding cross-archivo. Sub-paso futuro
  si aparece presión.
- **`import foo as f`** (aliases) y **`from foo import X as Y`**:
  no soportado.
- **`foo.User { ... }`** (struct literal con path): el parser
  no acepta `Path { ... }`. Workaround: `from foo import User`.
- **`let X = <expr>`** no literal a nivel mod: deuda 5b.5.
- **Inferencia de tipos de params** en fns sin anotar: deuda
  vieja de 5b.1, sigue.

##### 5b.6 — HTTP / `@server` / handlers ✓
**Cerrado.** Sexto paso de Fase 5b. Los binarios producidos por
`fitz build` pueden ser servidores HTTP nativos: `@get`/`@post`/
`@put`/`@delete` registran rutas en un `axum::Router`,
`@server(port, host)` configura la addr, y el `fn main()`
generado es `#[tokio::main] async fn main()`.

**Cargo.toml condicional**: el codegen escanea decoradores HTTP
en el AST. Si hay alguno, agrega `axum = "0.8"`, `tokio` (macros
+ rt-multi-thread), `serde` (derive), `serde_json`
(preserve_order). Programas sin HTTP siguen siendo builds
livianos sin estas deps.

**Implementación**:
- `has_http_routes(program)`: scan que mira si alguna `FnDef`
  tiene `decorators` no-vacíos. Decide modo HTTP vs CLI.
- `generate_main_rs` particiona stmts en categorías nuevas:
  `http_fns` (FnDef con `@get/...`), `top_fns` (FnDef sin decos),
  `type_defs`, `main_stmts`. Si hay HTTP, llama a
  `gen_http_main(...)` en lugar de `gen_main(...)`.
- **Handler wrapper** (`gen_http_handler_wrapper`): por cada
  `@<method>("/path") fn name(params) -> Ret`, emite un
  `async fn __handler_<name>(extractors) -> axum::response::Response`.
  Extractors:
  - Path params: `axum::extract::Path<i64>` para single, `Path<(T1,
    T2)>` para múltiples. Los nombres del template (`{id}`) se
    extraen con `parse_http_path` (soporta tanto `Expr::Str` como
    `Expr::StrInterp` con idents).
  - Body: `axum::Json<serde_json::Value>` → `<T as
    __FromFitzJson>::__from_fitz_json(...)`. Si falla → 400.
  - Llamada a la fn Fitz original con los args.
  - Si retorna `Result<T>`: match Ok/Err → 200/500. Si no:
    siempre 200.
- **Main HTTP** (`gen_http_main`): emite `#[tokio::main]\nasync fn
  main() { let __app = Router::new() .route(...) ...; let __addr =
  "host:port".parse(); axum::serve(...) }`.
- **`@server(port, host)`** (`parse_server_decorator`): valida
  args (port Int en [1,65535], host Str), defaults
  `(3000, "127.0.0.1")` si falta.
- **Serialización** (preludio HTTP):
  - Traits `__ToFitzJson` y `__FromFitzJson` con impls genéricos
    para primitivos, `Option<T>`, `Rc<RefCell<T>>`, `Vec<T>`,
    `Vec<(K, V)>`, `Result<T, String>`.
  - Trait `__MapKey` para que `Vec<(K, V)>` produzca objetos
    JSON con claves String (impls para String, i64, f64, bool).
  - Por cada `type Foo`: `impl __ToFitzJson for FooData` (objeto
    con field por field) + `impl __FromFitzJson for FooData`
    (valida extras, aplica defaults, chequea required).
  - Replica `value_to_json` / `json_to_instance` del intérprete.

**Limitación aceptada — state compartido entre handlers**: el
intérprete permite `let users = [...]` top-level usado por
handlers (env del módulo capturado). En Rust, fns top-level no
acceden al scope de `main`. Modelarlo bien requiere
`Arc<Mutex<...>>` + `axum::extract::State` y un refactor profundo
de la representación de tipos. **Decisión**: error de codegen
explícito si hay `Stmt::Assign` top-level + decoradores HTTP,
citando 5b.6 como deuda. Workaround: pasar state como arg, o
`fitz run`.

Esto significa que **`examples/server.fitz` y
`examples/guide/17-http.fitz` NO compilan con `fitz build`** —
usan `users` como state compartido. Siguen funcionando con
`fitz run`. La guía documenta la diferencia.

**`fn main()` del programa**: cuando tiene decoradores
(`@server(...) fn main() => 0`), su decorator se procesa pero
la fn NO se emite como item Rust (colisión con el `fn main` del
crate, que ahora es generado por el codegen). El cuerpo `=> 0`
es un placeholder — no se ejecuta.

**Tests**: 11 unit nuevos en `src/codegen.rs` (HTTP main async,
Router emit, path params Int/Str, handler Result match,
body deserializa, @server custom, default 127.0.0.1:3000,
state compartido aborta, Cargo.toml condicional, impl
ToFitzJson por type) + 7 E2E nuevos en
`tests/compile_e2e.rs` con un helper `build_spawn_request` que
buildea, spawnea el binario, abre un TCP socket directo a la
addr, envía request HTTP cruda, parsea status + body, y mata
el server. Cubre: GET simple, GET con path Int, Result Ok→200/
Err→500, POST body, POST defaults, POST extra→400, state
compartido aborta. El E2E "feature no soportada" reapuntado a
state compartido HTTP.

**Validación bit-a-bit**: server simple sin state (`fn
double(n: Int) => n * 2`) produce `42` para `/double/21` tanto
con `fitz run` como con `fitz build && ./bin`.

**Deuda explícita que sigue post-5b.6**:
- **State compartido entre handlers**: lo más visible. Bloquea
  los ejemplos canónicos del cap 17. Sub-paso futuro (`5b.6.1`?)
  con `Arc<Mutex<...>>` + `State` extractor + refactor de
  List/Map representación.
- **Status codes específicos por Err kind**: hoy todo Err → 500.
  Idea futura: `Err(e: NotFound)` → 404, etc.
- **Middleware, CORS, logging, TLS, streaming**: fuera de scope.
- **Body sin tipo declarado**: el codegen acepta solo body con
  tipo. Para body como `Map` libre necesitamos un FromFitzJson
  para `Rc<RefCell<Vec<(String, T)>>>` con keys auto-converted.

##### 5b.7 — Guía + ejemplos + cierre formal de Fase 5b ✓
**Cerrado.** Séptimo y último paso de Fase 5b. **Cierra la fase
entera.** El compilador está completo para todos los aspectos
centrales del lenguaje; los pendientes restantes son sub-pasos
opcionales que se abrirán post-fase si aparece presión.

**Cambios**:
- **Cap 18 nuevo en `docs/guide.md`** — "`fitz build` — compilar
  a binario nativo". Cubre:
  - Pipeline lex → parse → check → codegen → Cargo project →
    `cargo build` → binario adyacente al `.fitz`.
  - Estructura del project generado en `target/fitz-build/<stem>/`.
  - Mapping de tipos Fitz → Rust en tabla compacta (Int → i64,
    `List<T>` → `Rc<RefCell<Vec<T>>>`, `type Foo` → `struct
    FooData` + alias, etc.).
  - Tabla "Qué se soporta" — primitivos, control flow, tipos
    custom, listas/mapas homogéneos, Result, módulos, HTTP.
  - Sección "Qué todavía no anda con `fitz build`" — fns sin
    anotar, lista/mapa heterogéneos, higher-order completo,
    state HTTP compartido, `let X = <expr>` no literal en
    módulo, imports transitivos, división por cero literal,
    comparar tipos distintos.
  - Ejemplos CLI primitivo y HTTP simple (referenciando el
    nuevo `examples/guide/18-build.fitz`).
  - Criterio "cuándo usar `fitz run` vs `fitz build`".
  - Nota sobre cross-compilation gratis via rustc targets.
- **Re-numeración** del viejo cap 18 ("Qué sigue") a **cap 19**.
  Contenido actualizado: quita referencias a "5b.1/5b.2
  cerrados, 5b.3-5b.6 pendientes" (Fase 5 ahora cerrada
  entera). Apunta a Fase 6 (Async nativo), Fase 7 (DX HTTP),
  Fase 8 (Interop Python) y Fase 9 (Ecosistema) como próximo
  norte. Lista la deuda residual como sub-pasos futuros
  opcionales.
- **`examples/guide/18-build.fitz` nuevo** — server HTTP **sin
  state compartido** que compila end-to-end. Cubre GET
  estático, GET con path param + Result Ok/Err, POST con body
  deserializado a `type` custom con defaults + nullables.
  Sirve como referencia del cap 18 y como smoke E2E.
- **Fixes mínimos a ejemplos** para que entren a la lista
  compilable sin tocar el codegen:
  - `04-operadores.fitz`: agrega `let x = 10` (antes asignaba
    sin declarar), comenta `1 == "1"` (el intérprete devuelve
    `false`, el compilador rechaza Int vs Str).
  - `06-logica.fitz`: agrega `let age = 20`, anota
    `fn ruido() -> Bool`.
- **`examples/guide/{09,11,15,17}-*.fitz` mantenidos
  intérprete-only**, cada uno con razón explícita documentada
  en el cap 18 (lista heterogénea, higher-order, error
  intencional, state HTTP).
- **Smoke test E2E** (`smoke_ejemplos_guia_compilables_compilan`)
  en `tests/compile_e2e.rs`: itera la constante
  `GUIDE_EXAMPLES_COMPILE` con 13 entradas y verifica que cada
  una compile con `fitz build`. Limpia `.exe`/`.pdb` adyacente
  después de cada build. ~11s costo total, vale para prevenir
  regresiones futuras del codegen sobre la guía.
- **Validación bit-a-bit manual**: 12 ejemplos CLI compilables
  verificados con md5 del stdout `fitz run` vs `fitz build &&
  ./bin`. Todos coinciden bit-a-bit. El 13 (18-build) es HTTP
  y se valida con curl + status codes.

**Tests sumados**: 1 E2E nuevo (smoke). Sin unit nuevos —
5b.7 es docs + ejemplos + smoke, no toca el codegen.

**Total al cierre de Fase 5b**: 949 tests (901 unit + 48 E2E).

### Features de la fase entera
- [x] TypeExpr en AST y parser (5.1)
- [x] Resolución de tipos y checker base (5.2)
- [x] Checker de expresiones — synthesis básico (5.3.1)
- [x] Llamadas y return contra return_type (5.3.2)
- [x] Result, `?`, match exhaustivo (5.3.3)
- [x] Métodos built-in con templates paramétricos (5.3.4)
- [x] FnExpr.ret inferido + Expr::Index + cierre formal de 5.3 (5.3.5)
- [x] Modo strict y cierre de 5a (5.4)
- [x] Inferencia de tipos básica (synthesis de expresiones,
  unión de returns en FnExpr — la inferencia bidireccional más
  rica queda como deuda)
- [x] Backend de codegen decidido — transpile-a-Rust (5b)
- [x] Codegen subset primitivo + `fitz build` (5b.1)
- [x] Tipos custom + field access (5b.2)
- [x] Listas, mapas, indexing, métodos built-in (5b.3)
- [x] Result, `?`, match (5b.4)
- [x] Módulos / `import` (5b.5)
- [x] HTTP / `@server` / handlers (5b.6)
- [x] Guía + ejemplos + cierre formal de Fase 5b (5b.7)
- [ ] Optimizaciones básicas (post-5b — strings sin `.clone()`,
  pre-declaración de vars que cruzan bloques)
- [x] Binario nativo standalone (5b.1 — subset primitivo; tipos
  custom 5b.2; listas/mapas 5b.3; Result 5b.4; módulos 5b.5;
  HTTP 5b.6)
- [x] Cross-compilation (gratis via rustc targets)

### Criterio de completitud de Fase 5b — Codegen ✓

**Cerrada al cierre de 5b.7.** Resumen de lo que está disponible
hoy con `fitz build`:

- **Pipeline**: lex → parse → check (strict) → codegen → Cargo
  project (`target/fitz-build/<stem>/`) → `cargo build --release`
  → binario adyacente al `.fitz` fuente.
- **Subset compilable**:
  - Primitivos (Int/Float/Str/Bool/Null) + operadores + interp.
  - Control flow: `if`/`else`/`while`/`loop`/`for`/`match` con
    literales/ranges/Ok-Err/wildcard.
  - Tipos custom (`type Foo`): instanciación, fields, defaults,
    nullables, igualdad estructural, mutación con aliasing.
  - Listas/mapas **homogéneos**, indexing, métodos built-in
    (push/pop/map/filter/find/len, get/has/keys/values).
  - `Result<T>` con `?` y match exhaustivo. `Err` pinned a
    `String` en el código generado.
  - Módulos: `import foo`, `from foo import X` (tipos, fns,
    consts).
  - HTTP nativo: `@get`/`@post`/`@put`/`@delete` + path params
    + body JSON contra `type` custom + Result → 200/500
    + `@server(port, host)`.
  - Serialización JSON automática para tipos custom (objeto con
    field por field), listas, mapas, Result.

**Deuda residual visible** que queda como sub-paso futuro
opcional:
- **State compartido HTTP** — la limitación más visible. Bloquea
  `examples/server.fitz` y `examples/guide/17-http.fitz`. Requiere
  `Arc<Mutex<...>>` + `axum::extract::State` y refactor profundo
  de la representación de `List`/`Map`.
- **Inferencia de tipos de params** en fns sin anotar (deuda
  5b.1).
- **Listas/mapas heterogéneos** — el intérprete los maneja con
  `Value` tagged; el compilador exige homogéneo.
- **Higher-order completo** — closures escapadas, fns como
  param/retorno (más allá del callback inline).
- **`?` adentro de FnExpr inline**.
- **`let X = <expr>` no literal a nivel mod** (deuda 5b.5).
- **Imports transitivos** (deuda 5b.5).
- **HTTP avanzado**: status codes custom, middleware, query
  params, headers, TLS, streaming.

Estos NO son bloqueantes para declarar Fase 5b cerrada: el
80% del lenguaje compila end-to-end (validado con 13 ejemplos
guía compilados bit-a-bit contra el intérprete + 48 E2E del
codegen). La deuda restante se abre como sub-paso cuando aparezca
presión real.

---

## Post-Fase 5b — Cierre de deudas residuales

Tras cerrar Fase 5b, una auditoría del compilador
(`docs/deudas-post-5b.md`) generó ~45 hallazgos. La mayoría son
incrementales; algunos son sub-pasos formales que se abren como
mini-fases dedicadas. Los cerrados hasta hoy:

- **Ruta A — Quick wins (clippy + helpers + validaciones) ✓**
- **B.1 — Span en Stmt ✓** — los errores stmt-level del checker
  citan línea/columna reales en lugar de `0:0`.
- **C-F2 — Field assignment chequeo ✓** — el checker valida
  tipos en `obj.field = value`.
- **F12 — Higher-order completo ✓** — closures escapadas, fn
  nombrada como valor, FnExpr asignado a var, fn como param y
  como tipo de retorno.
  - Nueva variante `TypeExpr::Function { params, ret }` en el AST.
  - Parser: keyword contextual `Fn(T1, T2) -> U`. `Fn` seguido de
    `(` se reconoce como tipo función; en cualquier otro contexto
    sigue siendo nombre normal.
  - Checker: `resolve_type_expr` mapea a `Type::Function` (que
    ya existía). La validación de aridad/tipos de llamada
    funciona sin cambios.
  - Codegen: `Type::Function { params, ret }` → `Rc<dyn Fn(P1,
    ...) -> R>` Rust. Decisión: **Rc, no Box** — permite clone
    barato y matchea el patrón uniforme de List/Map/Nominal
    (referencia compartida). Trade-off: indirección por puntero
    por llamada, pero uniforme para vars/params/returns.
  - `Expr::FnExpr` deja de ser error de codegen; emite
    `Rc::new(move |p1: T1, ...| -> R { body }) as Rc<dyn Fn(...)
    -> R>`. Detección de capturas: walker recursivo identifica
    idents libres en el body (no params, no locals, sí en outer
    scope). Para capturas no-Copy se clonan **afuera** del closure
    (`let x = x.clone();`) para preservar aliasing sin consumir
    la var del caller con `move`.
  - `Expr::Ident` con nombre de fn top-level emite `(Rc::new(name)
    as Rc<dyn Fn(...) -> R>)` cuando se usa como valor (no como
    callee de Call directo). Las fn items de Rust implementan
    `Fn(...)` así que el cast compila sin glue.
  - `gen_call` detecta callee `Ident` que es var local de tipo
    `Type::Function` y la invoca via la firma. Rust auto-derefs
    el `Rc<dyn Fn>` a callable.
  - `examples/guide/11-funciones.fitz` anotado con tipos
    (criterio igual que caps 14/16/18 en 5b.7), agregado al
    smoke `GUIDE_EXAMPLES_COMPILE`. Compila bit-a-bit con
    `fitz run`.
  - Param de FnExpr sin anotar → error de codegen claro
    (deuda 5b.1).
  - **Tests sumados (+24)**: 5 parser (`type_expr_funcion_*`),
    6 checker (`type_expr_function_*` + `anotacion_function_*`),
    8 codegen unit (`fnexpr_suelta_emite_rc_dyn_fn`,
    `fn_nombrada_como_valor_*`, `fn_param_de_tipo_funcion_*`,
    `fn_como_return_type_*`, `closure_que_captura_var_no_copy_*`,
    `var_de_tipo_funcion_se_llama_*`,
    `fn_anonima_inline_como_arg_*`, `fnexpr_sin_anotacion_*`),
    6 E2E (`fn_anonima_asignada_a_var_*`,
    `fn_nombrada_como_valor_*`, `apply_con_fn_y_fnexpr_inline`,
    `closure_con_captura_int_*`, `closure_que_captura_str_*`,
    `fnexpr_sin_anotacion_*`), -1 viejo reemplazado
    (`fnexpr_suelta_da_error_claro` → `fnexpr_suelta_emite_rc_dyn_fn`).
  - **Total al cierre de F12**: 986 tests (932 unit + 54 E2E).
  - **Deuda residual que sigue**: `FnMut`/`FnOnce` (mutación de
    capturas) — hoy solo `Fn` inmutable; `?` adentro de FnExpr
    inline; inferencia de tipos de params en fns sin anotar
    (deuda 5b.1); F11 (state HTTP compartido).

---

## Fase 6 — Async nativo ⚡
**Estado: CERRADA (2026-05-13)** — `async fn`, `.await`,
`Future<T>`, builtin `sleep`, evaluator async, handlers HTTP
async, codegen async todo end-to-end. **Excepción**: el sub-paso
6.4 original (eliminar bridge HTTP `mpsc/oneshot`) quedó
**POSPUESTO hasta F17** (Send completo) por bloqueo de
`axum::Handler: Send`. Compromiso documentado en
`docs/deudas-post-5b.md`.

1085+ tests pasando al cierre. Validación bit-a-bit `fitz run`
vs `fitz build` para programa CLI con `async fn` + `sleep` y
para handler HTTP `async fn`.

Hoy `async fn` se parsea pero el runtime es sincrónico. Los
handlers HTTP corren en un thread del intérprete con bridge a
tokio vía `mpsc + oneshot` (en `fitz run`), o como fns sync
wrapeadas por axum (en `fitz build`). La promesa "HTTP nativo"
es cierta a nivel ergonómico pero tiene asterisco a nivel de
ejecución: no hay `await`, no hay concurrencia real adentro de
un handler, y un futuro driver de DB no podría aprovechar
tokio sin un bridge feo.

Esta fase **cumple la promesa**: async universal en el lenguaje,
con `await` postfix y `Future<T>` como tipo genérico de primera
clase. El evaluator pasa de sync a async; el runtime HTTP migra a
handlers async reales; el codegen emite `async fn` Rust directo.

### Decisión de alcance — Async universal

Se descartó explícitamente la alternativa "async sólo en
contexto HTTP". El compromiso es que `async fn` y `await` sean
válidos en cualquier parte del lenguaje, no sólo adentro de
handlers. Trade-off aceptado: refactor profundo del evaluator
(de `fn eval_expr(...) -> Result<Value>` a async con boxed
futures), pero la promesa queda limpia: "Fitz tiene async
nativo punto".

### Pasos

> **Re-numeración (sesión post-5b/T1)**: el plan original tenía 6.1-6.6.
> Al ejecutar la fase se partió en 7 sub-pasos más chicos. Numeración
> nueva: 6.1 AST/parser, 6.2 checker, 6.3 builtin `sleep`, 6.4 evaluator
> async, 6.5 bridge HTTP eliminado, 6.6 codegen, 6.7 guía. Los párrafos
> siguientes mantienen los IDs originales del documento; los estados
> reales por sub-paso están en los mensajes de commit.

#### 6.1 — Sintaxis: `await` postfix + `Future<T>` en AST/parser
**CERRADO** — base sintáctica.

- Nueva variante `Expr::Await { inner: Box<Expr>, span: Span }`.
- Parser: `expr.await` como sufijo postfix, con la misma
  prioridad que method calls y field access. Encaja en el
  parser existente de chains (`expr.field`, `expr.method()`).
- `TypeExpr::Generic` ya cubre `Future<T>` sin cambios — es un
  generic más con aridad fija 1.
- Tests: parser smokes para `x.await`, `f(x).await`, `xs.map(...)
  .await`, errores claros si falta `expr` antes de `.await`.

**Criterio de éxito**: `cargo run -- check archivo.fitz` parsea
`async fn f() -> Int { return x.await }` sin error sintáctico
(el checker todavía lo rechazaría hasta 6.2).

#### 6.2 — Type checker para async/await
**CERRADO** — semántica de tipos.

- `Type::Future(Box<Type>)` como generic built-in nuevo (aridad
  fija 1, registrado en `TypeEnv` junto con `List`/`Map`/`Result`
  /`Nullable`).
- `Stmt::FnDef { is_async: true }` → la firma externa de la fn
  envuelve el return type declarado en `Future<T>`. Adentro del
  body, `return_stack.last()` sigue siendo `T` (sin envolver) —
  el `async` es transparente desde adentro.
- `Expr::Await`: legal sólo si el `CheckCtx` está adentro de un
  `async fn`. Nuevo `CheckCtx.await_stack: Vec<bool>` paralelo a
  `return_stack` (cada `FnDef`/`FnExpr` pushea su flag).
  - Operando debe ser `Future<T>` (o `Any` para escape gradual).
  - Resultado: `T`.
  - Operando concreto distinto de `Future`/`Any` → error
    explícito: "`.await` solo aplica a `Future<T>`".
  - Fuera de `async fn` → error: "`.await` solo es válido
    adentro de `async fn`".
- Llamada a `async fn`: `is_compatible` reconoce que llamar
  `f(x)` donde `f: Function { ret: Future<T> }` produce
  `Future<T>`. El usuario tiene que await-earlo o pasarlo como
  valor.
- Tests: await fuera de async fn → error; await sobre Int →
  error; await sobre `Future<Int>` → `Int`; async fn que
  retorna `T` externamente tipa `Future<T>`.

#### 6.3 — Evaluator async (el costo grueso)
**CERRADO (re-numerado como 6.4 en commits)** — refactor profundo del
evaluator. La sesión metió un sub-paso 6.3 intermedio dedicado al
builtin `sleep` antes del refactor amplio del evaluator.

- `eval_expr` y `eval_stmt` pasan de `fn(...) -> Result<Value>` a
  `async fn(...) -> Result<Value>` con futures boxeados (Rust no
  permite `async fn` recursivo directo; se resuelve con
  `async-recursion` crate o `Pin<Box<dyn Future<...>>>`
  manual — decidir al implementar, ambos funcionan).
- `main.rs::run_file` corre el future raíz con `tokio::runtime::
  Runtime::new()?.block_on(eval_program(...))`. Único runtime
  compartido entre evaluator y server HTTP — no más
  `std::thread` para tokio.
- I/O builtins existentes (`print`, `len`, métodos de
  List/Map/Str) siguen sync. El evaluator es async, sus llamadas
  internas se await-ean trivialmente. La firma sync de las
  builtins no cambia.
- `Stmt::FnDef { is_async }` se convierte en `Value::Function {
  is_async }`. Cuando el evaluator llama una `async fn` Fitz
  desde un contexto sync (top-level del archivo, por ejemplo,
  o llamada desde una sync fn) sin `.await`, devuelve un
  `Value::Future` que envuelve el future. La política exacta
  se afina en este paso.
- `Expr::Await` evalúa el operando y await-ea el future.
- Tests: programa CLI con `async fn` + `await` corre con
  `fitz run`; runtime se inicializa una sola vez; await sobre
  no-future → panic claro (defensivo; el checker ya cierra el
  caso).

**Riesgo principal**: el refactor toca muchos lugares del
evaluator. Probablemente requiera un PR grande en vez de
incrementos chicos. Recomendación: rama dedicada
`feature/async-evaluator`, tests E2E del intérprete corriendo
verde antes de mergear.

#### 6.4 — Runtime HTTP: handlers async reales
**CERRADO en F17.5 (2026-05-14)** — el bridge HTTP `mpsc/oneshot`
desapareció.

- Plan original: hoy `src/http.rs` corre tokio en un `std::thread` y
  bridgea via `mpsc::UnboundedSender<InterpTask> + oneshot::Sender<...>`.
  Con evaluator async (cerrado en 6.4), el handler axum **debería**
  poder llamar `call_handler(handler_fn, args).await` directo.
- **Realidad descubierta al intentar (sesión 2026-05-13)**:
  `axum::handler::Handler` requiere `Send + 'static`. Los closures
  axum capturan `Value::Function` cuya `closure: EnvRef = Rc<RefCell<
  Environment>>` **no es Send**. La única salida real era migrar
  `Value`/`EnvRef`/módulos a `Arc<Mutex>` — eso es exactamente la
  deuda **F17** en `docs/deudas-post-5b.md`.
- **Cierre real (2026-05-14)**: F17.2 (Arc/Mutex), F17.3 (Send
  completo) y F17.4a (multi-thread) destrabaron la eliminación.
  F17.5 reescribió `serve()`, `build_router` y `build_method_router`
  para invocar `handle_task(&registry, ...).await` directo sobre un
  `Arc<HttpRegistry>` compartido. `InterpTask`, `TaskTx`,
  `run_interpreter_loop` y `dispatch_request` (versión vieja con
  canal) borrados — ~269 LoC netas menos en `http.rs`.

**Criterio de éxito cumplido**: `examples/guide/19b-paralelismo.fitz`
demuestra paralelismo HTTP real — 5 requests concurrentes a un
handler `sleep(1000).await` responden en ~1.2s; las mismas en serie
toman ~5.3s. Pre-F17 ambos casos eran ~5s.

#### 6.5 — Codegen: `async fn` Fitz → `async fn` Rust
**Pendiente (re-numerado como 6.6 en commits)** — emisión directa,
paso fácil después del refactor.

- `gen_top_fn` con `is_async: true` emite `pub async fn` en vez
  de `pub fn`. Return type sigue siendo `T` (Rust ya envuelve
  en `Future<Output = T>` automáticamente).
- `Expr::Await` Fitz → `<expr>.await` Rust. Mapping 1:1.
- Handlers HTTP en `fitz build` dejan de wrapearse en el código
  generado: el handler async es directamente lo que axum
  espera. El wrapper `__handler_<name>` actual sigue existiendo
  pero su cuerpo se simplifica (no hay más `from_fitz_json` +
  `.await` falso wrappeado).
- Tests: codegen unit para `async fn` + `.await`; E2E que
  compila un binario con `async fn` y verifica que corre.

#### 6.6 — Guía + ejemplo + cierre formal de Fase 6
**Pendiente (re-numerado como 6.7 en commits)** — documentación viva.

- Cap nuevo en `docs/guide.md` titulado "Async y concurrencia",
  probablemente entre el cap 17 (HTTP) y el 18 (`fitz build`).
- Tema central: cuándo usar `async fn` y cuándo sync;
  `Future<T>` como tipo; ergonomía de `.await` postfix;
  ejemplo concreto con HTTP concurrente.
- Ejemplo ejecutable `examples/guide/NN-async.fitz` (numeración
  tentativa, se decide al cerrar). Mínimo: `sleep_ms(n).await`
  builtin demostrando concurrencia. Idealmente: HTTP client
  builtin chico (`fetch(url).await`) si entra en scope.
- Actualizar cap 17 con la versión async de los handlers (y
  mantener nota explicando que sync sigue siendo válido).
- Cap 19 ("Qué sigue") actualizado: quitar la mención de async
  como deuda; sumar referencia a Fase 7 (DX HTTP) como
  próximo norte.
- README + roadmap.

### Decisiones cross-cutting

1. **Ergonomía de `.await` postfix vs prefix**: Fitz adopta la
   variante postfix (`expr.await`) en línea con Rust y con
   method chains. Razón: `db.find(id).await?` lee
   naturalmente de izquierda a derecha; `await db.find(id)?`
   obliga a leer al medio. La decisión queda formalizada acá
   y se documenta en syntax-spec.

2. **Entry point con async**: `fn main()` puede ser sync o
   async. Si es async, el runtime tokio lo await-ea al arrancar.
   Si hay rutas HTTP, el server arranca después de `main()`
   (mismo patrón actual). `@server(...)` sigue declarativo;
   NO se introduce un builtin `serve().await` (alternativa
   considerada y rechazada para preservar ergonomía actual).

3. **Async como gradual escape**: `async fn` sin anotaciones
   sigue tipando como `Function { ret: Future<Any> }`. El
   checker no exige que toda fn sea async ni que el código
   sync sea convertido — sync y async conviven libremente.

4. **No async/cancellation primitives todavía**: `tokio::select!`,
   `tokio::join!`, cancelación cooperativa, timeouts — todo eso
   queda como deuda visible para Fase 6.x o post-Fase 6.
   Esta fase entrega el modelo base; lo demás se construye encima.

### Riesgos

- **Refactor del evaluator es el mayor cambio interno desde el
  parser inicial**. Boxed futures, lifetimes async, tracing del
  flow — todo se complica. Mitigación: rama dedicada, tests
  intensivos antes de mergear, considerar partir 6.3 en
  sub-pasos chicos (6.3.1 = firma async + futures triviales,
  6.3.2 = chains complejos, etc.) si aparece presión.
- **Performance del evaluator async**: cada `eval_expr` ahora
  asigna un future. Para programas CLI cortos no importa; para
  hot loops podría doler. Mitigación: medir antes/después con
  los E2E del intérprete; si duele >2x, considerar técnicas
  estilo `tokio::task::yield_now` o async-await selectivo.
- **Compatibilidad con código existente**: cero programas Fitz
  hoy usan `await`. El parser lo aceptaba pero ninguna fn lo
  llamaba (era deuda silenciosa). Riesgo bajo.

### Features de la fase entera
- [ ] `Expr::Await` + `Future<T>` en AST/parser (6.1)
- [ ] Type checker valida `await`/`async fn` y `Future<T>` (6.2)
- [ ] Evaluator async con boxed futures + tokio runtime único (6.3)
- [ ] Runtime HTTP migra a handlers async reales (6.4)
- [ ] Codegen emite `async fn` Rust + `.await` (6.5)
- [ ] Guía + cap nuevo + ejemplo async + cierre formal (6.6)

---

## Mini-tanda PreF8 — Cleanup antes de Interop Python
**Estado: CERRADA (2026-05-14, 1172 unit + 79 E2E + 3 openapi)**

Sale del cierre de F17. Survey honesto de la matriz de deudas
identificó 4 items que F8 iba a estresar fuerte; se cerraron en
4 sub-pasos antes del salto a Fase 8 para no entremezclar deuda
existente con la parte real de Python interop. Cinco commits
(uno por sub-paso + cierre formal).

Resumen de sub-pasos cerrados:

- **PreF8.1** — refactor M1+M2 del codegen (`generate_main_rs` 232 LoC
  → 18 LoC + 3 helpers; `gen_http_handler_wrapper` 532 LoC → 9 LoC
  + 6 métodos). Cero cambio funcional, AST bit-a-bit idéntico.
- **PreF8.2** — method chain multi-línea en parser. `postfix()` loop
  tolera Newline antes de `.`. Cap 13 documenta como forma idiomática.
- **PreF8.3** — auditoría F4 de field defaults. 5/6 casos andaban;
  el bug era defaults de tipos importados que referencian símbolos
  del módulo de origen. Fix con eager-at-import en evaluator +
  `__default_<T>_<F>()` helpers en codegen.
- **PreF8.4** — import aliasing con `as`. Lexer `Token::As`, AST
  cambia `Stmt::Import.alias` y `Stmt::FromImport.names`. Evaluator
  + codegen + checker actualizados. Display canónico para paridad
  bit-a-bit.

### PreF8.1 — Refactor M1 + M2 codegen
**CERRADO** — extraer sub-fns de las dos funciones más grandes de
`codegen.rs`.

- **M1 — `generate_main_rs`** (~140 LoC, líneas 779-920 aprox).
  Mezcla particionado de stmts (HTTP fns vs CLI fns vs main_stmts),
  validaciones (cf. R1 cerrado: `fn main` solo con `@server`), y
  emisión (preamble + types + state HTTP + fns + main). Extraer a
  helpers: `partition_program(program) -> (http_fns, main_stmts)`,
  `validate_program_structure(program) -> Result<()>`,
  `emit_modules_recursive(...)`.
- **M2 — `gen_http_handler_wrapper`** (~160 LoC, líneas 3529-3688
  aprox). Resuelve params (path/query/body/headers/middlewares),
  los categoriza, emite el wrapper async con los extractors axum y
  el dispatch al handler Fitz. Extraer: `resolve_handler_params`,
  `categorize_handler_params`, `emit_axum_extractors`,
  `emit_dispatch_call`.

**Criterio de éxito**: AST del Rust generado bit-a-bit idéntico
pre/post refactor sobre los ejemplos compilables (smoke
`GUIDE_EXAMPLES_COMPILE` + suite T1). Cero cambio funcional.

**Decisión técnica a presentar al arrancar**: granularidad de la
extracción. Opciones: (a) muchas fns chicas con nombres
descriptivos (más jumps, más navegable); (b) pocas fns medianas
agrupando pasos relacionados (menos navegable, menos cognitive
overhead); (c) helper struct con métodos si hay estado compartido
no-trivial entre los pasos. Recomendado (b) con extracción
selectiva de sub-helpers donde la lógica se repita.

### PreF8.2 — F10 method chain multi-línea en parser
**CERRADO** — el parser hoy corta el statement al ver newline
después de un `Ident`/llamada cuando el siguiente token es `.` en
la línea siguiente. Eso rompe el patrón idiomático de chains
largas que Python/JS/Rust permiten:

```fitz
let activos = users
    .filter(fn(u) => u.active)
    .map(fn(u) => u.name)
    .find(fn(n) => n != "")
```

Hoy esto sería 3 statements rotos. F8 (interop Python con
SQLAlchemy `session.query(M).filter(...).order_by(...).first()`)
lo va a usar muchísimo; necesario antes del salto.

**Implementación tentativa**: lookahead en el parser después de
un newline cuando el último token significativo fue una expresión
"chainable" (ident, llamada, field access, indexing) — si el
siguiente token no-whitespace es `.`, suprimir el statement
terminator implícito y continuar la expresión.

**Decisión técnica**: ¿el parser solo se vuelve tolerante a
newlines en method chains, o también en operadores binarios
(`a +\n  b`)? Recomendado: solo method chains por ahora; binops
multi-línea es deuda separada con menos pago.

**Criterio de éxito**: ejemplo nuevo en cap 13 de la guía con
chain de 3+ líneas que parsea y ejecuta igual que la versión
de una línea. Tests parser dedicados.

### PreF8.3 — F4 field default audit
**CERRADO** — auditar que `Field.default` se popule en todos
los contextos donde un `type` aparece, y que el evaluator +
codegen los apliquen consistente.

**Casos a verificar**:
- Root del archivo: `type User { id: Int = 0 }` — sabemos que
  funciona (cap 12 de la guía lo cubre).
- Importado: `from foo import User` donde el `User` definido en
  `foo.fitz` tiene defaults — ¿se preservan?
- Field nullable + default: `type C { x: Int? = null }` — ¿el
  default `null` es válido?
- Struct lit anidado: `Outer { inner: Inner { ... } }` donde
  `Inner` tiene defaults — ¿se aplican correctamente?
- Reasignación: `u.x = ...` después de struct lit con defaults
  — ¿el field tiene el tipo correcto?
- Default que es expresión, no literal — ¿el parser lo acepta?
  ¿El evaluator lo evalúa en el contexto correcto?

**Implementación**: tests focales por cada combinación, fix de lo
que esté roto. Probablemente refactor menor del parser y/o
struct_lit en el evaluator.

**Criterio de éxito**: tests cubriendo todas las combinaciones
verdes + ejemplo en cap 12 que demuestre default importado.

### PreF8.4 — Import aliasing
**CERRADO** — sintaxis ya en `syntax-spec.md`, falta
implementación. Sub-paso adelantado de F8.1 (que lo promete
adentro). Adelantarlo deja F8.1 con solo Python interop puro y
cierra una deuda independiente.

**Sintaxis**:
```fitz
import foo as f                  // namespace alias
from foo import bar as b         // single binding alias
from foo import bar as b, baz as z  // múltiples aliases
```

**Capas a tocar**:
- **Parser**: lookahead `as` después de `import name` o de cada
  binding en `from foo import ...`. AST: `Stmt::Import { path,
  alias: Option<String> }`, `Stmt::FromImport { path, names:
  Vec<(String, Option<String>)> }` (name, optional alias).
- **Evaluator**: bindea el alias en lugar del nombre original.
  Sin alias, sigue funcionando idéntico (alias = None).
- **Codegen**: emite `use foo::bar as b;` cuando hay alias;
  módulos via `mod foo;` + `use foo as f;` para namespace.
- **Tests**: parser dedicados + ejemplo en cap 16 de la guía
  con alias real (caso típico: alias para librería con nombre
  largo).

**Criterio de éxito**: `import foo as f` y `from foo import bar
as b` funcionan en `fitz run` y `fitz build` bit-a-bit.

### PreF8.5 — Cierre formal
**CERRADO** — housekeeping al final de la mini-tanda.

- M1, M2, F4, F10, "import aliasing" marcados como CERRADOS en
  `docs/deudas-post-5b.md` (M1/M2 + F4 + F10 + nuevo F18 para
  aliasing).
- Entrada `v0.8.1 — Mini-tanda PreF8` sumada a `CHANGELOG.md`.
- Smoke completo verde: 1172 unit + 79 compile_e2e + 3 openapi_e2e.
  Clippy `-D warnings` limpio. Working tree limpio post-commits.
- Bit-a-bit `fitz run` ↔ `fitz build` validado sobre los ejemplos
  modificados (cap 13 `13-metodos.fitz` con chain multi-línea,
  cap 16 `16-modulos.fitz` con defaults importados + aliases).

**Total de la mini-tanda**: 5 commits (1 por sub-paso + 1 de
cierre formal). Después de cerrar, arranca **Fase 8.1**.

---

## Fase 8 — Interop Python 🐍
**Estado: PROPUESTA — no comprometida**

Una vez cerrada la Fase 5b, Fitz va a tener HTTP nativo, type
checker estricto y binarios standalone — pero no va a poder
hablarle a una base de datos, ni a NumPy/pandas, ni a librerías
de criptografía o de scraping. Construir desde cero todo el
ecosistema de un lenguaje de producción tomaría años, y
mientras tanto Fitz quedaría como lenguaje de juguete para
APIs in-memory.

La Fase 8 abre una puerta lateral: importar librerías de Python
desde código Fitz para heredar el ecosistema, mientras Fitz
construye su propio stack a su ritmo.

El caso de uso motivador es concreto: un proyecto Fitz con
handlers `@get`/`@post` que adentro usan SQLAlchemy contra
Postgres, mapeando los modelos del ORM a `type` de Fitz para
que el checker siga validando los handlers end-to-end.

### Posicionamiento estratégico

Esta es una decisión más política que técnica, y la propuesta
la pone arriba de todo a propósito: **Fitz NO se vuelve "Python
con sintaxis distinta"**.

- El código Fitz sigue compilando a binario nativo via 5b.
- El checker sigue mandando sobre el código Fitz.
- HTTP sigue siendo decoradores del lenguaje.
- Result + match + `?` siguen siendo el modelo de errores.

Python entra como **backend de librerías**, no como identidad
del lenguaje. La regla operativa: si los usuarios terminan
escribiendo handlers en `def` con sintaxis Python adentro de
archivos `.fitz`, perdimos. Si escriben `fn` Fitz que llaman a
`session.query(...)` y arman la respuesta con `Result<User>`,
ganamos.

Esta regla se traduce en concretos para la propuesta:
- La guía nueva (8.8) ilustra el patrón "Python para librerías
  pesadas, Fitz para todo lo demás" explícitamente.
- Los ejemplos canónicos mantienen handlers `fn` Fitz puros que
  consumen Python solo donde no hay alternativa nativa.
- La documentación enumera qué partes del stack van a migrar a
  Fitz nativo en fases futuras (DB driver, ORM) — Python es el
  puente, no el destino.

### Caso de uso canónico

Lo concreto que esta fase tiene que habilitar:

```fitz
from python import sqlalchemy as sa
from python.sqlalchemy.orm import Session

type User { id: Int, email: Str, name: Str }

let engine = sa.create_engine("postgresql://localhost/app")

@get("/users/{id}")
fn get_user(id: Int) -> Result<User> {
    let session = Session(engine)
    let row = session.query(UserModel).filter_by(id: id).first()?
    return Ok(User { id: row.id, email: row.email, name: row.name })
}

@post("/users")
fn create_user(body: UserInput) -> Result<User> {
    let session = Session(engine)
    let model = UserModel(email: body.email, name: body.name)
    session.add(model)
    session.commit()?
    return Ok(User { id: model.id, email: model.email, name: model.name })
}
```

La sintaxis exacta es propuesta — cada sub-paso puede afinarla.

### Pasos

#### 8.1 — Embedding básico de CPython
**Estado: CERRADA (2026-05-15)** — 1213 unit + 80 compile_e2e
+ 3 openapi_e2e con feature `python`; 1175 + 80 + 3 sin feature.

Embebe CPython en el runtime de Fitz via PyO3. Punto de partida
de toda la fase. Cubre lo esencial: importar un módulo Python y
llamar funciones top-level con argumentos y returns primitivos
(Int, Float, Str, Bool, Null). Marshaling compuesto (List/Map/
Instance) llega en 8.2; wrap automático a `Result<T>` en 8.3.

**Decisiones técnicas tomadas al arrancar el sub-paso**
(2026-05-14):

- **Sintaxis** (cross-cutting #1 del roadmap): reuse total de
  `Stmt::Import` y `Stmt::FromImport` con discriminante en
  runtime `path[0] == "python"`. Sin cambios al lexer, parser, ni
  AST — la mecánica de aliasing de PreF8.4 sirve tal cual. "python"
  queda reservado como prefijo de path top-level; un usuario que
  tenga un `python.fitz` local lo vería inaccesible (deuda menor,
  documentable).
- **ABI** (cross-cutting #6): PyO3 0.28 con feature `abi3-py310`.
  Un solo binario `fitz` corre contra cualquier CPython 3.10+
  instalado en la máquina (3.10/3.11/3.12/3.13/3.14).
- **Feature gate** (cross-cutting #3): opt-in con
  `cargo build --features python` (o `cargo install --features
  python`). Sin la feature, el binario `fitz` default sigue
  siendo standalone sin link a libpython — preserva la promesa
  "binario nativo standalone" de Fase 5b. Con la feature, los
  imports Python disparan error claro si el path no es soportado;
  sin la feature, error claro citando el flag de build.
- **Alcance** (decisión nueva al arrancar): 8.1 cubre solo
  `fitz run`. `fitz build` con imports Python aborta con mensaje
  claro citando "deuda comprometida para sub-paso futuro"
  (F19 / probable sub-paso de 8.7). Razón: el codegen agrega
  complejidad real (Cargo.toml con pyo3 condicional, traducción
  de getattr/call a `Python::attach` Rust, marshaling primitivo
  build-time), y validar el shape end-to-end en el evaluator
  primero permite iterar el shape del marshaling sin comprometer
  output Rust prematuramente.
- **Política de GIL**: `Python::attach` por cada operación
  pública del módulo `py_interop`. Para 8.1 (CLI sin async/HTTP
  concurrente) es suficiente y simple; revisitable en 8.6 cuando
  entre el bridge tokio↔asyncio.
- **Política de venvs**: estándar Python sin magia. El usuario
  activa su venv antes de `fitz run` y CPython embebido lee
  `VIRTUAL_ENV` al boot. Cero código nuevo en Fitz. Auto-detect
  de `./venv/` queda como deuda menor.
- **Inicialización**: lazy. PyO3 con `auto-initialize` bootea
  CPython solo en el primer `Python::attach`. Programas sin
  imports Python no pagan el costo del boot.

**Sub-pasos**:

- **8.1.1** — Dep PyO3 opcional + variante `Value::PyObject` ✓ —
  `Cargo.toml` suma `pyo3 = "0.28"` como dep opcional bajo la
  feature `python`. Features de PyO3: `abi3-py310` (un binario
  corre 3.10+) y `auto-initialize` (boot lazy en el primer
  `Python::attach`). `Value::PyObject(PyObjectHandle)` feature-
  gated; handle envuelve `Arc<Py<PyAny>>` para que `Value::clone()`
  sea O(1) sin tomar el GIL. Implementaciones: PartialEq por
  identidad via `Py::as_ptr()` (matchea la semántica de Module/
  Function que también comparan por identidad), Display
  `<python object>`, `type_name()` `"PyObject"`. `src/http.rs`
  suma arm feature-gated en `value_to_json` que rechaza
  serializar PyObject a JSON con mensaje claro — el handler debe
  coercionar a Fitz primitivo antes de devolver. Cero impacto en
  el binario sin la feature.
- **8.1.2** — `from python import X` + loader CPython ✓ —
  módulo nuevo `src/py_interop.rs` (feature-gated) con
  `import_module(dotted: &str) -> FitzResult<Value>` envuelto en
  `Python::attach` + `py.import(dotted)`. Helper privado
  `py_err_to_fitz(py, err) -> FitzError` traduce excepciones
  Python a `FitzError` con formato `"<ClassName>: <message>"` —
  formato compatible con el wrap automático a `Result<T>` que
  llega en 8.3 (solo cambia el envoltorio, no el string).
  Evaluator: `Stmt::FromImport` con `path[0] == "python"` rutea
  a `eval_python_from_import(path, names, env)`. Con feature:
  importa cada `name` como módulo top-level Python via
  `py_interop::import_module(name)`, bindea al scope respetando
  `as` alias. Sin feature: error claro citando
  `cargo build --features python`. `Stmt::Import` con prefijo
  `python` se rechaza con sugerencia de usar
  `from python import X` (forma canónica en 8.1). Alcance:
  `path == ["python"]` exacto. `from python.X.Y import Z` queda
  como deuda menor (workaround actual: `from python import X` +
  field access cuando 8.1.3 cierre).
- **8.1.3** — `Expr::Field` + auto-coerción primitiva ✓ —
  `py_interop::get_attr(handle, name)` toma GIL, hace
  `bound.getattr(name)`, y aplica `py_to_value` para coercionar
  el resultado. Política de coerción: `None` → `Value::Null`,
  `bool` → `Value::Bool` (chequea **antes** que `int` porque en
  Python `bool ⊂ int`), `int` → `Value::Int` si cabe en `i64`
  (overflow → error explícito; bignum support queda como deuda
  menor), `float` → `Value::Float`, `str` → `Value::Str`, resto
  (función, clase, instancia, submódulo, etc.) → `Value::PyObject`
  opaco. Helper `py_to_value` queda reusable para `call` (8.1.4)
  que procesa el return value igual. Evaluator: `Expr::Field`
  despacha sobre `Value::PyObject` con feature on; el error de
  getattr se enriquece con el span del field access (un
  `math.no_existe` apunta al `.no_existe`, no a línea 0:0).
  Desbloquea `math.pi` (Float), `math.__name__` (Str),
  `os.path` (submódulo opaco listo para field access anidado).
  `math.sqrt` queda como PyObject opaco listo para call (8.1.4).
- **8.1.4** — `Expr::Call` con args primitivos (criterio cerrado) ✓ —
  `py_interop::call(handle, &args)` toma GIL una vez, marshalla
  args via `value_to_py`, invoca `bound.call1(tuple)` (positional
  only — kwargs queda deuda menor), baja el return via
  `py_to_value`. Helper nuevo `value_to_py(py, &Value)` con
  política simétrica: primitivos Fitz → primitivos Python,
  `Value::PyObject(h)` → passthrough con `clone_ref` (preserva
  identidad), tipos compuestos (List/Map/Instance/Range/
  Function/...) → error explícito citando 8.2. Evaluator: nueva
  rama `Value::PyObject(handle)` en `invoke_value` (cubre
  `let f = math.sqrt; f(25.0)` — callee opaco después de field
  access) y en `dispatch_method` (cubre `math.sqrt(16.0)` directo
  parseado como `Expr::Call { callee: Expr::Field {...} }`, y
  chained calls como `json.dumps("hola")`). Excepciones Python
  emiten `FitzError` con span del call enriquecido. Cumple el
  criterio del roadmap end-to-end: `math.sqrt(16.0)` → `4.0`,
  `math.pi` → `3.141592653589793`.
- **8.1.5** — Guard de codegen + error path completo ✓ —
  `fitz build` con `from python import` aborta con mensaje claro
  sugiriendo `fitz run` (binario con `--features python`). Nueva
  fn libre `check_no_python_imports(program: &Program)` escanea
  top-level del AST buscando `Stmt::Import` o `Stmt::FromImport`
  con `path[0] == "python"`; devuelve `FitzError` con sugerencia
  específica para cada caso. Llamada en dos puntos:
  `generate_project` (path real, ANTES de `loader.collect_imports`
  para fallar rápido sin tocar disk y evitar el mensaje confuso
  "no se encontró `python.fitz`") y `generate_main_rs` (path de
  tests unit que usan `generate_rust` directo, sin loader).
  Deuda residual queda como **F19** en `deudas-post-5b.md`:
  soporte real en `fitz build` (emitir Rust con `pyo3` linkeado +
  Cargo.toml condicional + traducción de `getattr`/`call` a Rust)
  queda como probable sub-paso de 8.7 cuando cierre distribución
  con CPython bundled.

**Criterio de éxito** (del roadmap, cerrado bit-a-bit):
```fitz
from python import math
print(math.sqrt(16.0))  // 4
print(math.pi)          // 3.141592653589793
```

Validado end-to-end en `examples/python-interop-8.1.fitz`
(`cargo run --features python -- run examples/python-interop-8.1.fitz`).
NO entra al smoke `GUIDE_EXAMPLES_COMPILE` porque 8.1 es
`fitz run` only.

**Cierre formal de Fase 8.1**: 1213 unit + 80 compile_e2e +
3 openapi_e2e con feature `python`; 1175 + 80 + 3 sin feature.
Clippy `-D warnings` limpio en ambos modos. **Próximo norte**:
Fase 8.2 (marshaling de tipos compuestos).

**Deuda residual visible que se queda como sub-paso futuro**:
- F19 — codegen interop Python en `fitz build` (probable sub-
  paso de 8.7 con CPython bundled).
- `from python.X.Y import Z` con submódulos profundos
  (workaround: `from python import X` + field access anidado).
- `import python.X` sintaxis (deuda menor; la forma canónica es
  `from python import X`).
- Kwargs en llamadas Python (`obj.method(key=value)`) — `call1`
  hoy solo positional. Para SQLAlchemy `filter_by(id=5)` necesita
  Map → dict (8.2) + extender a `call`.
- Bignum: `int` Python > `i64` → error. Cuando entre demanda,
  agregar variante `Value::BigInt` o fallback a `Value::PyObject`.
- Stubs `.pyi` para que el checker tipe llamadas Python concretas
  (pospuesto explícitamente al menos hasta 8.4).

#### 8.2 — Marshaling de tipos compuestos
**Estado: CERRADA (2026-05-15)** — 1245 unit + 80 compile_e2e
+ 3 openapi_e2e con feature `python`; 1175 + 80 + 3 sin feature.

Extiende las conversiones bidireccionales a `List`, `Map` e
`Instance` (`Null` ya estaba en 8.1.4). Cumple el criterio
canónico del roadmap end-to-end: una función Python que recibe
`List<User>` y devuelve un mapping `email → cantidad`
(`collections.Counter` del módulo estándar) funciona sin perder
data — pipeline completo `users.map(fn(u) => u.email)` → `Counter`
→ `Map<Str, Int>` Fitz indexable nativamente.

**Decisiones técnicas tomadas al arrancar el sub-paso** (todas
alineadas con el roadmap original):

- **Copia eager bidireccional** (cross-cutting #4): los dos GCs
  no comparten estado. Una `List<T>` Fitz que va a Python se
  convierte en una `list` Python independiente; mutaciones del
  lado Python no se propagan a la List original Fitz, y
  viceversa. Trade-off conocido (caro para listas grandes),
  evita pesadillas de lifetime entre `Arc<Mutex>` Rust y el
  GC de CPython, y race conditions GIL/tokio. Optimizaciones
  zero-copy con buffer protocol quedan para Fase 9+.
- **Map keys cuando va a Python**: solo primitivos hashables
  (Int/Float/Str/Bool/Null). Tipos compuestos (List/Map/Instance)
  no son hashables en Python; los validamos antes de tocar
  `dict.__setitem__` para dar mensaje específico citando la
  restricción.
- **Heterogéneos OK**: Python `list`/`dict` admite mezcla de
  tipos naturalmente; no imponemos T concreto desde el lado Fitz.
- **`dict` Python → `Map` Fitz, NO `Instance`**: la coerción a
  `Instance` necesita anotación destino del lado Fitz y se cubre
  en 8.4 con la regla `let user: User = py_call(...)?`. Sin
  anotación, `dict` queda como `Map<Any, Any>` semánticamente.
- **Orden preservado**: CPython 3.7+ garantiza orden de inserción
  para `dict`; aprovecharlo da paridad bit-a-bit con
  `serde_json::preserve_order` que ya usa el resto del proyecto
  y con `Vec<(Value, Value)>` que es la representación interna
  de `Value::Map`.
- **Breadcrumb de errores con `path: &str`** propagado
  recursivamente: un `Range` adentro de `List<Map<Str, List<Range>>>`
  reporta `arg0[2]["k"][3]` o similar. Crítico para debugging de
  estructuras compuestas grandes.
- **Tuple/set/bytes** quedan como `Value::PyObject` opaco (no
  marshalleados a List/Map). Si entra demanda real, se pueden
  agregar en una fase futura sin romper la API.

**Sub-pasos**:

- **8.2.1** — Fitz → Python (`value_to_py`) ✓ — refactor con
  parámetro `path: &str` para breadcrumb. Nuevas ramas:
  `Value::List` → `PyList`, `Value::Map` → `PyDict` (con helper
  `marshal_map_key` validando keys hashables), `Value::Instance`
  → `PyDict` por field name. Recursión sobre elementos via la
  misma `value_to_py`. Errores con path informativo
  (`arg0.User.range`). Helper privado `fmt_map_key` para
  segmentar keys en el path (`"a"` con comillas para Str, literal
  para Int/Float/Bool/Null — cosmético). Tipos no marshalleables
  (Range, Function, Type, Module, HttpResponse, CorsConfig,
  Future, Result) → error con path. Test viejo de 8.1.4 que
  asumía "List como arg → error citando 8.2" se reapunta a
  Range (sigue sin ser marshalleable); test del evaluator
  análogo se reapunta a "Python rechaza con TypeError" porque
  la List ahora SÍ pasa y `math.sqrt([1,2,3])` lanza TypeError
  de Python.
- **8.2.2** — Python → Fitz (`py_to_value`) ✓ — nuevas ramas
  antes del fallback opaco: `PyList` → `Value::List` (vía
  `obj.cast::<PyList>()` — PyO3 0.28 deprecó `downcast` en favor
  de `cast`); `PyDict` → `Value::Map` con orden de inserción
  preservado. Recursión simétrica sobre elementos. Resultado
  semánticamente `List<Any>`/`Map<Any, Any>` desde Fitz (Python
  no nos da tipo estático); refinar a tipos concretos requiere
  anotación destino (deuda 8.4). Decisión explícita: `dict` NO
  se auto-coerce a `Instance`. El fallback `Value::PyObject`
  ahora solo aplica a tuple/set/bytes/función/clase/instancia/
  submódulo.
- **8.2.3** — Criterio de éxito end-to-end + ejemplo runnable ✓ —
  pipeline canónico `List<User>` Fitz → `users.map(fn(u) =>
  u.email)` → `Map<Str, Int>` Python via `collections.Counter`
  → `Map<Str, Int>` Fitz indexable. `Counter` es subclass de
  `dict`; `is_instance_of::<PyDict>()` matchea subclases, así
  que el round-trip funciona sin glue adicional (sin `dict()`
  envoltorio). Test adicional valida la política "copia eager":
  la `List<User>` Fitz original sigue accesible como Fitz nativa
  después del round-trip, con `Value::Instance` adentro (no
  contaminada por PyObjects). `examples/python-interop-8.2.fitz`
  con 5 secciones (Fitz → Python, Python → Fitz, round-trip que
  preserva estructura, criterio canónico, copia eager) validado
  a mano bit-a-bit. NO entra al smoke `GUIDE_EXAMPLES_COMPILE`
  porque interop Python es `fitz run` only (deuda F19).

**Criterio de éxito** (del roadmap, cumplido):
```fitz
type User { id: Int, email: Str }
from python import collections

let users = [
    User { id: 1, email: "alice@x.com" },
    User { id: 2, email: "bob@x.com" },
    User { id: 3, email: "alice@x.com" },
]
let emails = users.map(fn(u) => u.email)
let counts = collections.Counter(emails)
// counts: Map<Str, Int> indexable como cualquier Map Fitz
// counts["alice@x.com"] == 2
```

Validado bit-a-bit en `criterio_8_2_count_by_email_con_counter`
(test del evaluator) y en `examples/python-interop-8.2.fitz`.

**Cierre formal de Fase 8.2**: 1245 unit + 80 compile_e2e +
3 openapi_e2e con feature `python`; 1175 + 80 + 3 sin feature.
Clippy `-D warnings` limpio en ambos modos. **Próximo norte**:
Fase 8.3 (excepciones Python → `Result<T>`).

**Deuda residual visible que se queda como sub-paso futuro**:
- **Tuple/set/bytes** ↔ `List`/`Map`/`Value::Str(bytes hex)`
  (deuda menor — el fallback `PyObject` los preserva opacos).
- **Cycles** en `List`/`Map` Fitz (técnicamente posible con
  `Arc<Mutex>`) no se manejan explícitamente: caso edge raro en
  interop práctica.
- **Bignum**: int Python > `i64` sigue dando error (deuda
  arrastrada de 8.1.3). Cuando entre demanda, agregar variante
  `Value::BigInt` o fallback a `PyObject`.
- **Marshalling perf**: una llamada Python con N args primitivos
  + ret compuesto cuesta O(tamaño del ret) por la copia. Las
  queries SQLAlchemy típicas devuelven decenas de filas con
  pocos campos, entra cómodo en el presupuesto de latencia de
  un endpoint HTTP. Optimizar con zero-copy queda para Fase 9+
  si entra presión real.

#### 8.3 — Excepciones Python → Result<T>
**Estado: CERRADA (2026-05-15)** — 1252 unit + 80 compile_e2e
+ 3 openapi_e2e con feature `python`; 1175 + 80 + 3 sin feature.

Convención automática: **toda llamada a una función Python desde
Fitz se envuelve en `Result<T>`**. Si Python lanza `ValueError("x")`,
Fitz lo recibe como `Err("ValueError: x")`. Si el marshaling de args
falla (Range/Function/Type/etc. no marshalleable), también va como
`Err` con breadcrumb del path. Preserva la decisión de diseño "sin
excepciones" intacta y evita que excepciones Python escapen al
runtime de Fitz como panics opacos.

**Decisiones técnicas tomadas al arrancar el sub-paso** (alineadas
con el roadmap original):

- **`call` envuelve siempre; `get_attr` NO**. Solo llamadas pasan
  por `Result`; field access (`math.pi`, `obj.attr`) sigue
  devolviendo el valor coercionado directo. Matchea la letra del
  roadmap ("toda **llamada** a una función Python") y preserva la
  ergonomía de leer constantes y submódulos sin `match` por cada
  acceso. AttributeError fallido sigue siendo `FitzError` que
  aborta — es típicamente error de programación, no de runtime.
- **Marshaling de args también va en `Err`** (uniformidad): el
  usuario ve UN solo punto de error en el path call,
  independiente de qué falló — excepción Python o tipo Fitz no
  marshalleable. Sin esto, habría dos caminos de error (panic vs
  Result) y el código del usuario quedaría inconsistente.
- **`Err` lleva `Value::Str` con el mensaje plano**, formato
  `"<ClassName>: <message>"` ya estable desde 8.1.2 (compatibilidad
  bit-a-bit). `Value::Instance(PyException)` con inspección
  estructurada (type, traceback) queda como deuda menor si entra
  demanda real.
- **KeyboardInterrupt/SystemExit también van como `Err`** según el
  roadmap. No hay forma de matar el runtime Fitz desde una
  excepción Python.
- **El checker NO cambia en 8.3**. Sigue tipando call Python como
  `Any`. El refinamiento a `Result<Any>` llega en 8.4 cuando el
  checker se actualice. Esto preserva el modelo gradual: hoy
  `match`/`?` sobre Any pasan sin chequeo estático; mañana en 8.4
  el checker forzará el manejo explícito.
- **Cambio de comportamiento**: técnicamente rompe los ejemplos
  viejos de 8.1/8.2 que asumían call sin wrap. Se reescribieron en
  8.3.2.

**Sub-pasos**:

- **8.3.1** — `call` envuelve return en `Result` + tests viejos
  actualizados ✓ — `py_interop::call(handle, args)` ahora SIEMPRE
  devuelve `Ok(Value::Result(...))`. Éxito produce
  `Value::Result(Ok(v))` con el valor coercionado adentro; cualquier
  falla del path Python (excepción Python, marshaling de args
  imposible, marshaling del return) produce
  `Value::Result(Err(Str("<ClassName>: <message>")))`. Helper
  privado `err_value_from_message(msg)` construye el wrap. Los ~16
  tests viejos del call path (8.1.4 + 8.2.1 + 8.2.2 + 8.2.3)
  actualizados con helpers `ok_inner(v)` (extrae el Ok) y
  `err_message(v)` (extrae el string del Err). 4 tests `py_interop`
  nuevos sobre el shape: `call_exitoso_devuelve_value_result_ok`
  (invariante), `call_jsonloads_malformado_es_err_con_jsondecodeerror`
  (criterio textual del roadmap), `call_typeerror_python_se_envuelve_no_aborta`,
  `call_formato_err_es_classname_dos_puntos_message` (estabilidad).
  En el evaluator: 3 tests nuevos del criterio 8.3
  (`criterio_8_3_json_loads_malformado_con_match`,
  `criterio_8_3_propagacion_con_try_operator`,
  `criterio_8_3_field_access_no_se_envuelve`) — el último valida
  explícitamente la decisión de NO envolver field access. Tests
  viejos de 8.1/8.2 reescritos con `ok_inner` en asserts o con
  `match { Ok(v) => v, Err(_) => ... }` adentro del código Fitz
  cuando necesitan operar sobre el valor (ej. indexing
  `counts["a"]`).
- **8.3.2** — Ejemplos 8.1 y 8.2 actualizados al modelo `Result` ✓ —
  `examples/python-interop-8.1.fitz` reescrito: cada call Python se
  desempaqueta con `match { Ok(v) => v, Err(_) => fallback }` o se
  propaga con `?` adentro de fns dedicadas (`fn floor_x(x: Float)
  -> Result<Int> { return Ok(math.floor(x)?) }`). Field access
  (`math.pi`, `math.e`, `os.path.__name__`) sigue sin wrap.
  Sección nueva "Errores Python como Result::Err" muestra el caso
  `math.sqrt(-1.0) → err: ValueError: ...`.
  `examples/python-interop-8.2.fitz` reescrito análogo: helper
  `fn unwrap_str(r: Result<Str>) -> Str` para los `json.dumps` que
  ahora devuelven `Result<Str>`; caso nuevo
  `loads(malformado) → JSONDecodeError: ...`. **Caveat documentado**:
  literales compuestos (Map literal `{"a": 1}`) extraídos a
  variables antes del `print` porque el parser de interpolación no
  acepta `{...}` adentro de strings interpolados (toma `{` como
  inicio de sub-interpolación). Ambos ejemplos validados bit-a-bit.
- **8.3.3** — Ejemplo dedicado + cierre formal ✓ — nuevo
  `examples/python-interop-8.3.fitz` con 6 secciones del modelo de
  errores: (1) criterio textual del roadmap `parse(malformado)`
  con `match`, (2) distintas excepciones Python como Err
  (`ValueError` desde sqrt/log/int), (3) propagación con `?`
  adentro de `fn root_safe(x) -> Result<Float>`, (4) marshaling
  fallido como Err con breadcrumb (`math.sqrt(0..10) → err:
  ...Range...arg0`), (5) field access sin wrap (decisión interna),
  (6) chaining con desempaquetado intermedio
  (`fn round_trip(xs) -> Result<List<Int>> { dumps?; loads? }`).
  Validado bit-a-bit. CHANGELOG v0.8.4, roadmap marca 8.3 a
  CERRADA, deudas-post-5b nota de cierre paralela a las de
  8.1/8.2/F17, README refresh.

**Criterio de éxito** (del roadmap, cumplido bit-a-bit):
```fitz
from python import json

fn parse(input: Str) -> Result<Map<Str, Any>> {
    return json.loads(input)
}

match parse("{ malformado") {
    Ok(m)  => print("ok: {m}"),
    Err(e) => print("error: {e}")
    // → "error: JSONDecodeError: Expecting property name enclosed in double quotes: ..."
}
```

Validado en `criterio_8_3_json_loads_malformado_con_match` (test
del evaluator) y bit-a-bit en `examples/python-interop-8.3.fitz`.

**Cierre formal de Fase 8.3**: 1252 unit + 80 compile_e2e +
3 openapi_e2e con feature; 1175 + 80 + 3 sin feature. Clippy
`-D warnings` limpio en ambos modos. **Próximo norte**: Fase 8.4
(refinar el tipo de call Python a `Result<Any>` en el checker +
anotaciones explícitas del lado Fitz).

**Deuda residual visible que se queda como sub-paso futuro**:
- **`Value::Instance(PyException)`** con type/traceback/cause
  estructurados. Para 8.3 el string `"<ClassName>: <message>"`
  alcanza; estructurado entra si aparece presión real.
- **`get_attr` envuelto opcionalmente** (`obj.attr?` style):
  ergonomía vs ortogonalidad. Por ahora la decisión es asimétrica
  (solo call envuelve), revisitable.
- **`async fn` Python (corutinas) → `Future<Result<T>>`**: queda
  para 8.6 cuando entre el bridge tokio ↔ asyncio.
- **Errores con structured payload en Err** (tipo `Map` con
  fields class/message/details): el `Err` con `Str` alcanza para
  display pero no para parsing programático. Si aparece demanda,
  promover a `Value::Instance(PyError)` con campos accesibles.

#### 8.4 — Tipos del lado del checker (anotaciones y opacidad)
**Estado: CERRADA (2026-05-15)** — 1271 unit + 80 compile_e2e
+ 3 openapi_e2e con feature `python`; 1193 + 80 + 3 sin feature.

Cierra el ciclo "call Python → tipo Fitz concreto" con tres
cambios coordinados: el checker estático distingue valores Python
de Any genérico (`Type::PyAny`), refina los calls a `Result<Any>`
forzando manejo de errores estático, y el runtime coerciona
`Value::Map` → `Value::Instance` cuando hay anotación nominal en
el binding. Una sola anotación basta para salir del "limbo Python"
a tipos Fitz concretos.

**Decisiones técnicas tomadas al arrancar el sub-paso** (alineadas
con el roadmap):

- **`Type::PyAny` dedicado** (no `Type::Any` genérico ni
  `Type::PyObject<"...">` fantasma). Empezar simple — distingue
  "esto viene de Python" de "esto es Any general", suficiente
  para refinar los calls. PyObject con string fantasma queda como
  upgrade futuro si entra fricción real.
- **Coerción Map → Instance vive en el evaluator**, no en el
  checker. El checker ya acepta el cast (gradual Any → T) y no
  duplicaría la lógica. El runtime hace la coerción real con
  validación de fields, defaults, nullables.
- **Campos extras del dict se ignoran silenciosamente**. Python
  suele devolver más data de la necesaria (queries SQLAlchemy
  con todos los joins, responses HTTP con campos auxiliares);
  ser permisivos evita fricción.
- **Field requerido faltante → `FitzError` que aborta**, no
  `Result::Err`. Diseño: este caso indica datos malformados a
  nivel de fuente (DB schema desalineado, API contract roto),
  no un error de runtime esperable como una excepción Python.
  El programador debe validar el dict antes o declarar el campo
  nullable/con default si la fuente lo omite legítimamente.
- **Métodos sobre objetos Python (`engine.connect()`)**: el
  call sobre `PyAny` refina a `Result<Any>` automáticamente.
  La cadena ya funciona sin nuevo dispatch — el checker
  preserva el comportamiento gradual existente para argumentos
  y receptores opacos.
- **El operador `?` solo se chequea adentro de fn con return
  `Result<...>`** (regla heredada de 5.3.3). `?` a top-level se
  reporta en runtime, no en el checker — comportamiento
  consistente con calls nativas Fitz.
- **Stubs `.pyi` quedan pospuestos a Fase 9+** (equivalente a
  `@types/...` de TypeScript). 8.4 cubre el caso canónico
  (anotaciones explícitas + coerción runtime) sin necesidad de
  stubs — el checker pasa por gradual y el runtime valida.

**Sub-pasos**:

- **8.4.1 + 8.4.2** — `Type::PyAny` + bindings Python + call
  refina a `Result<Any>` ✓ (combinados en un commit; el
  refinamiento del call eran ~5 LoC adicionales al cambio del
  Type, los tests del checker validan el pipeline en bloque):
  nueva variante `Type::PyAny` en `types.rs` con identidad propia
  y bidireccionalmente compatible con cualquier tipo (gradual
  escape, igual que Any). `Stmt::Import` y `Stmt::FromImport` con
  `path[0] == "python"` tipan los bindings como `PyAny`; imports
  normales siguen como `Any` (`import utils` sigue gradual).
  `Expr::Field` sobre `Type::PyAny` devuelve `PyAny` (permite
  chaining como `os.path` → submódulo opaco). `Expr::Call` con
  receptor PyAny (callee o `Field.object`) refina el ret type a
  `Type::Result(Box::new(Type::Any))` — activa estáticamente la
  regla de exhaustividad sobre Result (5.3.3) y la regla del
  operador `?` (5.3.3). `is_compatible` suma rama temprana para
  PyAny espejo de Any. `display_type` y `type_name` en codegen.rs
  suman el caso por exhaustividad del match (PyAny no aparece
  nunca en codegen porque el guard `check_no_python_imports`
  aborta `fitz build` antes). 9 tests nuevos del checker
  (bindings PyAny, call → Result<Any>, match exhaustivo OK vs
  no exhaustivo error, `?` adentro de fn Result OK vs fn Int
  error, field access encadenado, anotación concreta pasa por
  gradual, import normal NO es PyAny).
- **8.4.3** — Coerción runtime `Value::Map` → `Value::Instance`
  con anotación nominal ✓: `Stmt::Assign` con `target: Ident` y
  anotación dispara `coerce_to_annotation(annot, value, env)`
  antes de bindear. La fn nueva resuelve `Named(T)` o
  `Nullable(Named(T))` a `(type_name, allows_null)`; si la
  anotación no encaja, passthrough. Si el value no es `Map`
  (Instance ya, primitivo, etc.), passthrough. Si es Map,
  resuelve el `Value::Type` en el env (built-ins como `Int`
  passthrough — los primitivos no se construyen desde dicts) e
  itera los fields declarados en orden: `provided` en el Map
  (key `Str` con nombre del field) → ese; resuelto en
  `resolved_defaults` (PreF8.3, tipos importados con default
  eager-evaluado) → ese; `default` Expr declarado en el field →
  evalúa con env actual; field nullable → `Null`; else →
  `FitzError` claro citando type + field. Campos extras del Map
  se ignoran silenciosamente. Result: `Value::Instance` con
  `type_name` canónico (mismo criterio de PreF8.4: nombre del
  `Value::Type`, no la anotación sintáctica). `AssignTarget::Field
  { ... }` (`obj.field = ...`) NO dispara la coerción — la
  anotación del field vive en el `type` declarado, no en el
  sitio del assign. 9 tests nuevos en evaluator: 8 sin feature
  (coerce básica, field faltante error, default aplicado,
  nullable faltante Null, extras ignorados, nullable con Null
  passthrough, nullable con Map sí coerce, value que no es Map
  passthrough, anotación a tipo no-nominal passthrough); 1 con
  feature validando el criterio canónico end-to-end
  (`fn parse_user(s) -> Result<User> { let row: User =
  json.loads(s)?; return Ok(row) }`).
- **8.4.4** — Ejemplo runnable + cierre formal ✓: nuevo
  `examples/python-interop-8.4.fitz` con 5 secciones del patrón
  canónico (happy path, nullable faltante → Null, extras
  ignorados, JSON malformado propagado por `?`, default aplicado)
  + comentario explícito sobre el caso "field requerido
  faltante" que aborta por diseño. Validado bit-a-bit. CHANGELOG
  v0.8.5, roadmap.md actualiza Fase 8.4 a CERRADA con sub-pasos
  detallados, deudas-post-5b.md nota de cierre paralela, README
  refresh.

**Criterio de éxito** (del roadmap, cumplido bit-a-bit):

```fitz
type User { id: Int, email: Str, name: Str, age: Int? }
from python import json

fn fetch_user(s: Str) -> Result<User> {
    let row: User = json.loads(s)?
    return Ok(row)
}

match fetch_user("{\"id\": 1, ...}") {
    Ok(u)  => print("usuario: {u}"),
    Err(e) => print("error: {e}"),
}
```

Cumple los tres pilares del roadmap: (a) el checker tipa
`fetch_user(...)` como `Result<User>` (refinado por PyAny en el
call interno → `?` → anotación `User`); (b) el `?` desempaca el
`Result<Any>` Python; (c) el runtime coerciona el dict resultante
a `Instance` validando los campos del `User`. Sin stubs `.pyi`,
con UN solo punto de anotación.

**Cierre formal de Fase 8.4**: 1271 unit + 80 compile_e2e +
3 openapi_e2e con feature; 1193 + 80 + 3 sin feature. Clippy
`-D warnings` limpio en ambos modos. **Próximo norte**: Fase 8.5
(`fitz py-types` auto-mapeo SQLAlchemy → `type` Fitz).

**Deuda residual visible que se queda como sub-paso futuro**:
- **Stubs `.pyi` parseados** (pospuesto explícitamente a Fase
  9+ — equivalente a `@types/...` de TypeScript). 8.4 cubre el
  caso canónico sin stubs.
- **`PyObject<"...">` fantasma** (distinguir engines de sessions
  sin saber su estructura). Promover si la fricción aparece en
  proyectos reales.
- **Validación de tipo de cada field del dict** (hoy passthrough
  estructural — un Map con `id: "no es int"` cruza sin chequeo y
  el uso posterior falla por type mismatch). Posible refinement:
  coerciones primitivas Str → Int cuando el field declara Int,
  o errores de coerción tempranos.
- **Coerción anidada de fields complejos** (List<User> adentro
  de un Map → List<Instance>). Hoy passthrough — el inner Map se
  coerce solo si hay anotación explícita en el binding del lado.
  Recursión podría agregarse con flag `--strict-coerce`.

#### 8.5 — Auto-mapeo de modelos SQLAlchemy a `type` de Fitz
**Estado: CERRADA (2026-05-15)** — 1281 unit + 80 compile_e2e
+ 3 openapi_e2e con feature `python`; 1193 + 80 + 3 sin feature.

Sub-comando nuevo `fitz py-types <archivo.py> [--out <archivo.fitz>]`
que introspecciona modelos SQLAlchemy (o mocks equivalentes) en
un archivo Python y emite los `type` Fitz correspondientes,
listo para commitear. Reduce el doble-tipado (Python + Fitz) en
el caso canónico — los modelos se definen UNA vez en Python y
Fitz los importa con sus tipos resueltos vía `from models import
User, Order`.

**Decisiones técnicas tomadas al arrancar el sub-paso**:

- **In-process via PyO3** (no subprocess): reusa el GIL + dep
  PyO3 ya disponible con `--features python`. Más simple que armar
  subprocess management + parseo de output. Sin la feature, el
  sub-comando aborta con error claro citando el flag de build.
- **Duck typing sobre `__table__.columns`** (no `isinstance(cls,
  DeclarativeBase)`). Permite tests con classes Python mock sin
  requerir `pip install sqlalchemy`. Funciona igual con SQLAlchemy
  real porque la introspección depende solo del shape (`name`,
  `type`, `nullable`, `default`), no de la herencia.
- **Solo SQLAlchemy en 8.5**. Django, Tortoise, peewee y
  dataclasses quedan como sub-comandos futuros si entra demanda
  real (`fitz py-types-django`, etc.). La arquitectura es
  reusable — el dispatch va por shape del object, no por ORM
  específico.
- **Tipos desconocidos → `Any` con comentario** `// ? tipo
  SQLAlchemy <Name> mapeado a Any`. Permite al usuario detectar
  y refinar a mano (ej. `JSON` → `Map<Str, Any>` con un editor).
- **Defaults callable ignorados** silenciosamente. SQLAlchemy
  usa mucho `default=datetime.utcnow` y similares — emitir
  `= func()` no es evaluable estáticamente desde Fitz y confunde
  más que aporta. Solo literales Int/Float/Str/Bool/None se
  emiten inline.
- **Output a stdout por default**; `--out <archivo>` opcional.
  El header del archivo generado dice `// Generado por
  fitz py-types — no editar a mano` + cita la fuente. Facilita
  el flujo "commiteás el `.fitz`, regenerás cuando cambia el
  schema".
- **Sin verificación de drift** entre `.py` y `.fitz` generado.
  Regeneración manual; linter de drift queda para Fase 9+ si
  entra demanda.

**Sub-pasos**:

- **8.5.1** — Sub-comando + introspección + mapping ✓:
  `Commands::PyTypes { source, out }` en el CLI con flag
  opcional `--out`. Nuevo módulo `src/py_types.rs` feature-gated.
  `generate_from_file(source) -> Result<String, String>`
  canonicaliza el path, importa el archivo Python via
  `importlib.util.spec_from_file_location` +
  `importlib.util.module_from_spec` + `loader.exec_module`,
  itera el `__dict__` filtrando: clases (`isinstance type`),
  definidas EN ese módulo (filtra re-exports SQLA), con
  `__table__.columns`. Por cada modelo recolecta los fields
  iterando `columns` (cada item con `name`, `type`, `nullable`,
  `default`). El mapping va por **nombre canónico de la clase
  Python** de `Column.type` (sin requerir `isinstance` contra
  jerarquía SQLA): `Integer/BigInteger/SmallInteger/INTEGER/...`
  → `Int`; `Float/Numeric/Double/REAL/FLOAT/NUMERIC` → `Float`;
  `String/Text/Unicode/VARCHAR/TEXT/CHAR/CLOB` → `Str`;
  `Boolean/BOOLEAN` → `Bool`; `DateTime/Date/Time/TIMESTAMP/
  DATE/TIME` → `Str` (ISO 8601 placeholder — `DateTime` nativo
  Fitz es Fase 9+); resto → `Any` con `// ? tipo SQLAlchemy
  <Name> mapeado a Any` como pista para el usuario.
  `nullable=True` agrega sufijo `?`. `default=<literal>` (Int/
  Float/Str/Bool/None) se emite inline; callable se ignora con
  `builtins.callable(value)`. Helpers privados: `import_file_as_module`,
  `collect_models`, `collect_fields`, `python_type_to_fitz_type`,
  `extract_default`, `emit_type`, `emit_literal`, `pyerr_to_string`.
  10 tests unit (modelo simple, mapping completo, nullable,
  default literal, default callable ignorado, tipo desconocido
  con comentario, múltiples modelos, archivo sin modelos error
  claro, clases sin `__table__` filtradas, header cita fuente).
  Todos con classes Python mock sin requerir SQLAlchemy real
  instalado.
- **8.5.2** — Ejemplo runnable + cierre formal ✓:
  `examples/py-types/models.py` autosuficiente con ~25 LoC de
  mock SQLAlchemy + dos modelos (`User` con 6 fields incluyendo
  `age: Int?` y `is_admin: Bool = false`; `Order` con 5 fields
  incluyendo `currency: Str = "USD"` y `notes: Str?`).
  `examples/py-types/models.fitz` (generado y commiteado): output
  literal de `fitz py-types models.py --out models.fitz` — sirve
  como referencia del output esperado y como módulo importable.
  `examples/py-types/usage.fitz`: `from models import User, Order`
  + fns `parse_user`/`parse_order` con coerción 8.4.3
  (`let row: User = json.loads(s)?`). Cubre happy path completo,
  default de `currency` aplicado, nullable `notes` como Null,
  JSON malformado propagado como `Result::Err`. Validado bit-a-bit
  con `cargo run --features python -- run examples/py-types/usage.fitz`.
  Cierre formal (CHANGELOG v0.8.6, roadmap a CERRADA, deudas
  nota de cierre, README refresh).

**Criterio de éxito** (del roadmap, cumplido bit-a-bit):

```bash
$ fitz py-types examples/py-types/models.py --out models.fitz
✓ types Fitz emitidos a models.fitz

$ cat models.fitz
// Generado por `fitz py-types` — no editar a mano.
// Fuente: examples/py-types/models.py

type User {
    id: Int,
    email: Str,
    name: Str,
    age: Int?,
    is_admin: Bool = false,
    created_at: Str
}

type Order {
    id: Int,
    user_id: Int,
    total: Float,
    currency: Str = "USD",
    notes: Str?
}
```

Después, `from models import User, Order` adentro de un programa
Fitz funciona como cualquier otro import + las anotaciones de 8.4
coercionan dicts Python a Instance.

**Cierre formal de Fase 8.5**: 1281 unit + 80 compile_e2e +
3 openapi_e2e con feature; 1193 + 80 + 3 sin feature. Clippy
`-D warnings` limpio en ambos modos. **Próximo norte**: Fase 8.6
(bridge tokio ↔ asyncio para `py_coro().await`).

**Deuda residual visible que se queda como sub-paso futuro**:

- **Otros ORMs** (Django, Tortoise, peewee, dataclasses) →
  sub-comandos futuros si entra demanda real.
- **Verificación de drift** entre `.py` y `.fitz` generado.
  Hoy regeneración manual; podría haber una regla del linter
  Fase 9+ que detecte schema mismatch y avise.
- **DateTime nativo Fitz** (vs string ISO 8601 hoy). Deuda más
  general — Fase 9+ con tipos nativos de fecha/hora/duration.
- **Defaults callable parseados** (`datetime.utcnow` →
  `current_timestamp()` built-in Fitz). Hoy se ignoran; podría
  agregarse mapping de callables conocidos.
- **Foreign keys + relationships**. SQLAlchemy expone
  `relationship(...)` que crea atributos lazy (no `Column`).
  Hoy se ignoran porque no aparecen en `__table__.columns`.
  Para emitir `user: User?` en `Order` haría falta dispatch
  específico para relationships.
- **Generación directa al `lib.fitz` del proyecto** (en vez de
  archivo separado). Workaround actual: el usuario decide
  dónde commitear el output.

#### 8.6 — Async + GIL — bridge tokio ↔ asyncio
**Estado: CERRADA (2026-05-15)** — 1284 unit + 80 compile_e2e
+ 3 openapi_e2e con feature `python`; 1193 + 80 + 3 sin feature.

Permite `py_async_fn().await` desde cualquier `async fn` Fitz.
Cuando un call a una función Python devuelve una corutina (caso
típico de `async def`), Fitz la detecta vía `inspect.isawaitable`
y la envuelve automáticamente en `Value::Future` adentro del
`Result::Ok`. El `.await` postfix existente (Fase 6) desempaca el
Future, ejecuta la corutina, y devuelve el valor coercionado a
`Value`. Excepciones asyncio bajan como `Result::Err` con el
formato canónico ya estable desde 8.1.2. Bridge invisible al
usuario — escribe `py_async_fn().await` igual que con futures
Fitz nativos.

**Decisiones técnicas tomadas al arrancar el sub-paso**:

- **Detección automática de awaitable en `call`** (no `.await`
  manual sobre PyObject opaco): el usuario escribe
  `py_async_fn().await` natural. La detección usa
  `inspect.isawaitable` (canónica en Python stdlib).
- **Approach "baseline blocking"** en vez de
  `pyo3-async-runtimes::tokio::into_future`. La crate
  `pyo3-async-runtimes` 0.28 (matchea pyo3 0.28) requiere
  control del runtime tokio (vía `init_with_runtime` o la
  macro `#[tokio::main]` de pyo3-async-runtimes), lo cual
  choca con el tokio que Fitz ya tiene corriendo
  (current_thread CLI / rt-multi-thread HTTP). Intentar
  inicializarla con un `Handle::try_current` no calienta el
  event loop asyncio que `into_future` necesita ("no running
  event loop"). Para 8.6.1, optamos por `tokio::task::spawn_blocking`
  + `asyncio.new_event_loop().run_until_complete(coro)`:
  Send-safe (Py<PyAny> viaja al worker), no deadlockea, simple.
  La versión future-based real (event loop asyncio persistente
  compartido entre awaits) queda como **deuda menor** — el shape
  `Value::Future` ya es estable, sólo cambia la implementación
  interna.
- **El GIL serializa Python** (riesgo central del roadmap,
  esperado): N awaits concurrentes a corutinas distintas se
  serializan en el GIL del lado Python. Para APIs DB-bound
  (caso típico SQLAlchemy/asyncpg con queries cortas), la DB
  es el cuello de botella, no el GIL — funcional. Para APIs
  CPU-intensivas con NumPy o long-running asyncio.gather,
  subóptimo (deuda menor con bridge persistente).
- **Política de GIL por default**: el GIL se mantiene durante
  todo el `run_until_complete` en el worker thread. PyO3 no lo
  suelta automáticamente entre pasos de asyncio porque toda la
  coordinación es de un solo thread. Cuando entre la versión
  future-based, el GIL se podría soltar entre awaits.
- **Sin marshaling Future Fitz → corutina Python**: pasar un
  `Value::Future` Fitz como arg a una función Python no se
  soporta (Future no es marshalleable, igual que Range/Function).
  Caso afectado: `asyncio.gather(fut1, fut2)` desde Fitz no
  funciona si los futs vienen de calls Python anteriores
  (el primer `.await` los desempaca a Values, no quedan como
  corutinas para gather). Workaround: definir un módulo Python
  helper con un `async def` que haga el gather internamente.
- **Caso runnable de excepción asyncio** se documenta en el
  ejemplo pero no se demuestra: requiere definir una `async def`
  Python custom, lo cual exige un archivo helper aparte (el
  `from python import` carga módulos top-level, no archivos
  del usuario). El patrón de manejo de errores es idéntico al
  de calls sync (8.3).
- **Compatibilidad con Fase 6 cumplida**: el modelo `async fn`
  + `Future<T>` + `.await` postfix se reusa tal cual. Cero
  cambios al checker ni al parser; solo `py_interop::call` se
  extiende para envolver en Future en lugar de PyObject opaco.

**Sub-pasos**:

- **8.6.1** — Bridge baseline + tests ✓: en `py_interop.rs`,
  `call` detecta cuando el return Python es awaitable (helper
  `is_coroutine(py, obj)` invoca `inspect.isawaitable` con
  fallback defensivo a `false`) y lo envuelve en
  `Value::Future` adentro del `Result::Ok`. El
  `py_coro_to_fitz_future(coro)` construye el `FitzFuture`
  capturando el `Py<PyAny>` Send y delegando a
  `tokio::task::spawn_blocking` que adentro del worker hace
  `Python::attach` + `asyncio.new_event_loop()` +
  `run_until_complete(coro)` + `close()`. El JoinHandle del
  blocking task se `.await`a; si el thread paniquea, devolvemos
  `FitzError` claro. 3 tests nuevos en evaluator bajo
  `#[cfg(feature = "python")]`.
- **8.6.2** — Ejemplo runnable + cierre formal ✓:
  `examples/python-interop-8.6.fitz` con 3 secciones (patrón
  canónico `doble_eventual` con `sleep+return Ok(x*2)`, awaits
  encadenados `pipeline` con 3 sleeps + cálculo, lazy `Result<
  Future>` sin `.await`). Notas extensas sobre modelo de errores
  asyncio (heredado de 8.3), trade-off baseline blocking y por
  qué no hay caso runnable de excepción asyncio (requiere
  helper Python externo). Validado bit-a-bit. Cierre formal
  (CHANGELOG v0.8.7, roadmap, deudas, README).

**Criterio de éxito** (del roadmap, cumplido bit-a-bit con
adaptación pragmática — el caso textual del roadmap usa
SQLAlchemy real que requiere setup adicional; lo simulamos con
`asyncio.sleep + cálculo`):

```fitz
from python import asyncio

async fn doble_eventual(x: Int) -> Result<Int> {
    let _ = asyncio.sleep(0)?.await
    return Ok(x * 2)
}

let r = match doble_eventual(21).await {
    Ok(v)  => v,
    Err(_) => -1,
}
// r → 42
```

El test `fase_8_6_async_fn_fitz_que_await_python_devuelve_valor_calculado`
valida exactamente este shape. Para el caso SQLAlchemy real,
reemplazás `asyncio.sleep` con `session.execute(...).await` y
el patrón es idéntico.

**Cierre formal de Fase 8.6**: 1284 unit + 80 compile_e2e +
3 openapi_e2e con feature; 1193 + 80 + 3 sin feature. Clippy
`-D warnings` limpio en ambos modos. **Próximo norte**: Fase 8.7
(codegen interop Python en `fitz build` — cierra deuda F19).

**Deuda residual visible que se queda como sub-paso futuro**:

- **Event loop asyncio persistente** (vs `new_event_loop` por
  call): la implementación baseline crea un loop por await,
  ~ms de overhead cada uno + sin paralelismo entre awaits.
  Versión future-based real compartiría un loop y soltaría el
  GIL entre awaits — habilitaría paralelismo I/O real
  (decenas/cientos de queries concurrentes). Migración
  contenida porque el shape `Value::Future` ya es estable.
- **Future Fitz → corutina Python** (marshaling inverso). Sin
  esto, `asyncio.gather(fut1, fut2)` desde Fitz no funciona
  con futs de calls Python previos. Workaround: helper Python
  externo que hace el gather. Solución completa requiere wrap
  bidireccional Future↔Coroutine — complejidad considerable.
- **Política de GIL configurable** (`@release_gil`, opt-in). El
  roadmap propone anotaciones por handler para soltar el GIL
  automáticamente en calls async, mantener en sync. Hoy no se
  implementa; con el approach baseline, soltarlo no aplica
  (todo el `run_until_complete` ocupa el GIL). Llegará con el
  bridge real.
- **Cancelación de Futures Python**. Un `.await` Fitz que se
  cancela (ej. timeout del request HTTP) no cancela la
  corutina Python — el worker thread sigue ejecutando.
  Solución requiere wiring del `asyncio.CancelledError`.
- **Tests con paralelismo real**: el roadmap pide 50 requests
  concurrentes en `flavor = "multi_thread"`. Hoy los tests
  son `current_thread` por compatibilidad con el resto de
  Fitz. Bench dedicado queda como deuda.

#### 8.7 — Codegen interop Python en `fitz build` (cierra F19) — CERRADA (2026-05-15)

**Decisión de alcance al arrancar 8.7** — la versión original del
roadmap mezclaba dos pistas ortogonales bajo el nombre "8.7":
**(a)** codegen interop Python en `fitz build` (deuda F19) y
**(b)** distribución con CPython embebido (`--bundle-python`).
Al revisar la deuda F19 y el setup necesario, decidimos
**partir el sub-paso en dos**: 8.7 cierra solo (a), F19 queda
resuelta, y (b) — bundling de CPython — queda como sub-paso
**futuro post-8** con dos opciones reales evaluadas: PyOxidizer
(mantenimiento muy lento en 2024-2025) y python-build-standalone
(la opción que usa `uv` de Astral, mantenida activamente). La
promesa "binario nativo standalone" se preserva: programas
**sin** interop Python siguen produciendo binarios libres
exactamente como Fase 5b. Programas **con** interop asumen
Python instalado en el destino — modo (1) del roadmap original,
el caso "pasa solo si tenés Python".

**Sub-pasos cerrados** (1284 unit + 80 E2E + 3 openapi sin
feature; **1295 unit + 88 E2E + 3 openapi con `--features python`**;
clippy `-D warnings` limpio en ambos modos; paridad bit-a-bit
`fitz run` ↔ `fitz build` validada con `examples/python-interop-8.7.fitz`):

- **8.7.1 — Preludio Python + import + getattr + Cargo.toml**:
  - `collect_python_imports(program) → Vec<PythonImport>` separa
    los imports Python del top-level del AST. El `ModuleLoader`
    de Fitz los skipea (no hay archivo `.fitz` que cargar).
  - Cargo.toml condicional: si hay imports Python, suma
    `pyo3 = { version = "0.28", features = ["abi3-py310",
    "auto-initialize"] }`. Programas sin interop no pagan el
    costo de bajar/linkear pyo3.
  - Preludio Python emitido en `emit_python_prelude` (solo
    cuando `uses_python = true`):
    - `struct __FitzPyObject(Arc<Py<PyAny>>)` con Clone, Debug,
      PartialEq (por puntero), Display que delega a `__str__`
      Python (paridad bit-a-bit con `print(math.pi)` del
      intérprete).
    - Helpers `__fitz_py_import(dotted) → __FitzPyObject`
      (panic on fail al boot), `__fitz_py_get_attr_obj`,
      `__fitz_py_extract_{i64,f64,string,bool}`,
      `__fitz_py_err_to_string` (formato canónico
      `<Class>: <msg>` paralelo a 8.1.2).
  - **Bindings globales**: cada `from python import X` se
    emite como `static __FITZ_PY_BIND_X: OnceLock<__FitzPyObject>` +
    getter `__fitz_py_bind_x()` al top-level del crate. Lazy
    init en el primer `Python::attach`; cualquier fn
    (main, handlers HTTP, helpers) los puede referenciar.
  - `Type::PyAny` → `__FitzPyObject` en `rust_type_for`.
    `gen_field_access` despacha sobre receptor PyAny.
    `coerce(PyAny → T)` para primitivos emite extracción
    directa (`let pi: Float = math.pi` →
    `__fitz_py_extract_f64(...)`).

- **8.7.2 — Call + marshaling Fitz → Python + Result wrap**:
  - `gen_call` y `gen_method_call` aceptan receptor PyAny y
    emiten `__fitz_py_invoke(&<callable>, |py| Ok(vec![<args
    marshalled>]))` con resultado `Result<__FitzPyObject,
    String>`. Excepciones Python aparecen como
    `Err(Str("<Class>: <msg>"))` con el formato 8.1.2/8.3.
  - Trait `__FitzToPy` con impls genéricos para primitivos
    (i64/f64/bool/()/String), `__FitzPyObject` (passthrough
    con clone_ref), `Option<T>`, `Arc<Mutex<Vec<T>>>` (List
    → list Python con breadcrumb `arg0[i]`), y
    `Arc<Mutex<Vec<(K,V)>>>` (Map → dict con `__fitz_py_marshal_map_key`
    para validar primitivos hashables).
  - **Marshaling Instance Fitz → Python dict**: `gen_type_def`
    emite `impl __FitzToPy for FooData` (iterando los fields y
    construyendo un PyDict) + `impl __FitzToPy for Arc<Mutex<FooData>>`
    (wrapper que delega vía lock) cuando `uses_python = true`.
    Destraba el caso canónico 8.5: pasar `User { id: 1,
    name: "Ada" }` a `json.dumps(user)`.
  - `gen_python_call_args(args)` emite cada arg como
    `<code>.__fitz_to_py(py, "arg<i>")?` con breadcrumb
    numerado paralelo a `value_to_py(path: &str)` del
    intérprete 8.2.

- **8.7.3 — Bridge async tokio ↔ asyncio**:
  - Helper `async fn __fitz_py_invoke_await<F>(callable, args_fn)
    → Result<__FitzPyObject, String>` emitido en el preludio
    (solo cuando `uses_async = true`). Combina call sync +
    detección `inspect.isawaitable` + ejecución vía
    `tokio::task::spawn_blocking` + `asyncio.new_event_loop().
    run_until_complete()`. Si no es awaitable, devuelve el
    value directo — `.await` ergonómico aún sobre fns Python
    sync. Paralelo a `py_coro_to_fitz_future` 8.6.1 (mismo
    baseline blocking, mismo trade-off).
  - **Patrón canónico Fitz**: `<py_call>?.await`. El AST es
    `Await(Try(Call PyAny))`. El codegen detecta el patrón
    (`try_gen_python_await` + `try_gen_python_call_await`) y
    emite `__fitz_py_invoke_await(&callable, |py| Ok(vec![<args>])).
    await?` — el `?` Rust al final propaga excepciones asyncio
    del await mismo. Tipo Fitz resultante: `PyAny` (gradual).
  - Checker (`Type::PyAny.await → Any`) acepta el patrón
    canónico estáticamente; rechaza `<call>.await` directo
    sin `?` (paridad con evaluator del intérprete, que rechaza
    en runtime con "se esperaba Future").

- **8.7.4 — Cierre formal**:
  - `examples/python-interop-8.7.fitz` con 3 secciones (constantes
    + coerción primitiva, calls + Result + marshaling List/
    Instance, bridge async con patrón canónico). Validado
    bit-a-bit `fitz run` ↔ `fitz build` + binario standalone.
  - CHANGELOG.md v0.8.8, roadmap actualizado, deudas-post-5b
    marca **F19 como CERRADO** con nota detallada, README refresh.

**Decisiones técnicas tomadas al arrancar**:
- **Alcance acotado (codegen sí, bundling no)**: F19 era la
  deuda comprometida; bundling era el "siguiente paso" del
  roadmap original. Separados, F19 es chico y medible
  (~3 sub-pasos), bundling es proyecto separado con decisión
  de herramienta pendiente. Carta blanca del autor para tomar
  esta decisión al ver que mezclar las pistas inflaba el
  alcance.
- **Bindings globales con OnceLock + getter** (vs `let X =
  __fitz_py_import(...)` en main body): destraba uso adentro
  de handlers HTTP y user-fns separadas. Cero refactor para
  los próximos sub-pasos (interop Python en handlers, CRUD
  contra SQLAlchemy/asyncpg).
- **Trait `__FitzToPy` con impls condicionales por nominal**
  (vs un mini-Value runtime): se mantiene en el modelo
  estático del codegen. El impl genérico para `Arc<Mutex<Vec<T>>>`
  (List) no conflictúa con el wrapper de nominales sobre
  `Arc<Mutex<FooData>>` porque `Vec<T>` ≠ `FooData`.
- **Patrón canónico `?.await` único** (en lugar de soportar
  también `<call>.await` directo): paridad bit-a-bit con
  intérprete + un solo camino que mantener. La rama "checker
  permite Result<PyAny>.await" se evaluó y descartó porque
  el evaluator del intérprete no la acepta — sería divergencia
  semántica entre los dos paths.
- **Auto-coerción primitiva via `coerce(PyAny → T)`** (vs un
  trait `__FitzFromPy` simétrico): aprovecha la infraestructura
  existente del codegen. La extracción dispara solo cuando
  hay anotación destino concreta (`let pi: Float = math.pi`);
  sin anotación, el binding queda opaco con Display delegado
  a `__str__` Python.

**Deuda residual visible (sub-paso futuro)**:

- **Coerción Python list/dict → Fitz `List<T>` / `Map<K,V>` /
  `Instance`**: los helpers `__fitz_py_to_list_{i64,f64,string}`
  ya están emitidos en el preludio, pero falta el wiring en
  `coerce(PyAny → List<T>)` y equivalentes. Sub-paso futuro
  habilita el patrón `let users: List<User> = py_call(...)?`
  que es el caso canónico 8.5 para recibir filas de SQLAlchemy.
  Paralelo a `coerce_to_annotation` 8.4.3 del intérprete.
- **`.await` con binding intermedio** (`let fut = py_call()?;
  fut.await` con split del call y el await): hoy solo el patrón
  `<py_call>?.await` inmediato. Deuda menor — requiere helper
  separado `__fitz_py_await_value(&pyobj)` que opere sobre
  PyAny aislado.
- **Bundling CPython embebido** (`fitz build --bundle-python`):
  proyecto separado. Decisión de herramienta entre
  python-build-standalone (recomendado al cierre) y PyOxidizer
  (verificar estado al arrancar). Tres opciones de distribución
  del roadmap original siguen vigentes.
- **Trait `__FitzFromPy` simétrico**: el actual `coerce(PyAny → T)`
  solo cubre primitivos. Un trait dual permitiría conversiones
  estructurales completas. Sin presión real hoy.

#### 8.8 — Guía + ejemplo CRUD + cierre formal de Fase 8 — CERRADA (2026-05-15)

**Decisiones de scope al arrancar 8.8** (confirmadas con el autor):
- **Posición del capítulo**: cap 21 nuevo "Interop Python" entre
  cap 20 (`fitz build`) y el viejo cap 21 (Qué sigue). Una sola
  renumeración (21 → 22). Lectura lineal — el cap 20 menciona
  limitaciones que cierra interop, así que conviene leerlos en
  ese orden.
- **Backend de DB del ejemplo CRUD**: SQLite + SQLAlchemy
  in-process (sobre las tres opciones consideradas: SQLite,
  Postgres+docker-compose+async, sin DB). Setup mínimo
  (`pip install sqlalchemy`), sin Docker ni Postgres remoto.
  Cubre el mismo patrón conceptual que Postgres (sesiones,
  models, queries) — el código Fitz es idéntico salvo la URL
  de conexión.
- **Modo de ejecución del ejemplo**: solo `fitz run`
  (intérprete) con nota explícita sobre 8.7. El intérprete ya
  tiene la coerción `Map → Instance` de 8.4.3 que el ejemplo
  necesita; `fitz build` cubre el codegen interop pero la
  coerción de compuestos sigue siendo deuda residual. Honesto
  sobre lo que está y lo que falta.

**Sub-pasos cerrados**:

- **8.8.1 — Capítulo 21 "Interop Python" en `docs/guide.md`**:
  - Capítulo nuevo con 12 sub-secciones cubriendo todo lo de
    8.1-8.7: setup, sintaxis (`from python import` + alias +
    path punteado), constantes/atributos, calls con Result
    wrap automático, propagación con `?`, marshaling Fitz →
    Python (List/Map/Instance), recuperación con anotaciones,
    `fitz py-types` SQLAlchemy, bridge async (`<py_call>?.await`),
    `fitz build` con interop (qué anda y qué es deuda residual),
    ejemplo CRUD ejecutable referenciado, y limitaciones
    honestas (GIL, numpy C extensions, herencia, gather con
    futures Fitz).
  - Renumeración: cap 21 viejo "Qué sigue" → cap 22; índice
    actualizado con la parte 10 nueva ("Cerrando").
  - Cap 22 ("Qué sigue") refrescado: la sección "Lo que ya
    sabés" suma el bullet de interop Python; la sección "Lo
    que viene" pasa de "más allá de Fase 7" a "más allá de
    Fase 8" + próximo norte Fase 9 + sub-paso futuro separado
    de bundling + stack DB nativo (Fase 10+).

- **8.8.2 — Ejemplo CRUD ejecutable**:
  - `examples/guide/21-python-crud/` con:
    - `models.py` — modelo SQLAlchemy `User` sobre SQLite.
    - `db.py` — helpers DB (`init_db`, `add_user`, `list_users`,
      `get_user`, `reset`) que devuelven dicts/lists nativos
      Python para marshaling directo a Fitz.
    - `models.fitz` — output de `fitz py-types models.py`
      (versionado para que el ejemplo funcione sin requerir
      `sqlalchemy` solo para regenerar).
    - `app.fitz` — programa Fitz principal con 3 handlers HTTP
      (`POST /users`, `GET /users`, `GET /users/{id}`).
  - Helper `user_from_py(raw)` — round-trip por JSON
    (`json.dumps` + `json.loads`) para disparar la coerción
    `Map → Instance` de 8.4.3 sobre dicts Python opacos.
  - Setup: `pip install sqlalchemy` + `PYTHONPATH=examples/guide/21-python-crud`
    antes del comando. El cap 21 explica por qué (preferimos
    respetar el estándar Python sobre magia de Fitz para
    sys.path; además `sys.path.insert` no funciona desde Fitz
    porque `sys.path` se coerce a `List` Fitz nativa via
    `py_to_value` y no tiene `.insert`).
  - `.gitignore` suma reglas para `__pycache__/`, `*.pyc`, y
    `examples/guide/21-python-crud/crud.db` (la DB SQLite que
    el ejemplo crea al boot).
  - Validación end-to-end con curl: POST inserta filas con id
    auto-asignado, GET lista todas como JSON, GET por id devuelve
    `User` Fitz tipado.

- **8.8.3 — Cierre formal**: CHANGELOG v0.8.9, este roadmap
  marca Fase 8.8 CERRADA + sección de cierre de Fase 8 entera,
  deudas-post-5b refresh con nota final, README refresh para
  reflejar Fase 8 completa.

**Limitaciones documentadas** (honesta) en el ejemplo + cap:
- Iterar `List<Any>` opaca de Python con coerción por item a
  `List<User>` requiere el wiring de `coerce(PyAny → List<T>)`
  recursivo — deuda residual de 8.7. El ejemplo `list_users()`
  devuelve JSON crudo en lugar de `List<User>` Fitz, y la guía
  documenta el workaround "iterar con `user_from_py` adentro
  de `.map(...)`" como ejercicio.

### Cierre formal de Fase 8 entera (Interop Python)

Roadmap original cumplido al 100%:

- ✅ **8.1** — Embedding básico de CPython (`from python import X`
  desde `fitz run`)
- ✅ **8.2** — Marshaling List/Map/Instance bidireccional
- ✅ **8.3** — Excepciones Python → `Result<T>` automático
- ✅ **8.4** — Tipos del checker (`Type::PyAny`) + coerción
  runtime `Map → Instance` con anotaciones
- ✅ **8.5** — `fitz py-types` auto-mapeo SQLAlchemy → `type`
  Fitz
- ✅ **8.6** — Bridge tokio ↔ asyncio (`<py_call>?.await`
  baseline blocking)
- ✅ **8.7** — Codegen interop Python en `fitz build`
  (cierra deuda F19)
- ✅ **8.8** — Guía + ejemplo CRUD + cierre formal

**Tests al cierre**: 1204 unit + 79 E2E + 3 openapi sin feature;
1295 unit + 88 E2E + 3 openapi con `--features python`. Clippy
`-D warnings` limpio en ambos modos.

**Sub-paso separado CERRADO 2026-05-23**: bundling CPython
embebido (`fitz build --bundle-python`) — ver
**Fase 8.b** abajo. Decisión python-build-standalone (Astral,
mantenido activamente) vs PyOxidizer (ralentizado 2024-2025):
**PBS ganó sin discusión** después de investigación. Modelo
launcher pattern (Datasette Desktop) elegido tras descartar
extract-on-first-run naive, linking estático con PBS full
(PyOxidizer-style), y delay-load/dlopen manual.

**Próximo norte**: **Fase 9 — Ecosistema** (package manager,
LSP con autocomplete + hover + go-to-def, formatter, linter).
Pre-reqs habilitantes ya identificados en deudas-post-5b: F15
(parser error recovery) + F16 (IR tipado persistido por nodo).

**Deuda residual derivada de Fase 8** (NO bloquea Fase 9):
- Coerción Python `list`/`dict` → Fitz `List<T>`/`Map<K,V>`/
  `Instance` en `fitz build` (helpers `__fitz_py_to_list_*` ya
  emitidos en preludio 8.7.2, falta wiring en `coerce(PyAny → ...)`)
- `.await` con binding intermedio split (hoy solo el patrón
  `<py_call>?.await` inmediato)
- Trait `__FitzFromPy` simétrico al `__FitzToPy` actual
- Stubs `.pyi` parseados (pospuesto a Fase 9+)
- KeyboardInterrupt/SystemExit estructurados como tipos
  específicos (hoy bajan como Err genérico)

### Fase 8.b — `fitz build --bundle-python` (CPython embebido) — CERRADA (2026-05-23)

Sub-paso separado de Fase 8 entera. Habilita producción de
binarios standalone con CPython 3.14.5 embebido vía
[python-build-standalone (PBS)](https://github.com/astral-sh/python-build-standalone)
de Astral. El binario resultante **NO requiere Python instalado
en el destino** — corre en cualquier máquina del triple soportado,
en frío. Único lenguaje moderno activamente mantenido con esta
capacidad (PyOxidizer hizo algo parecido pero está ralentizado
desde 2023).

**Sub-pasos cerrados** (8 commits en una sesión):

- ✅ **8.b.1** — Cache + descarga PBS (`src/pbs.rs`). Release
  pinned `20260510` con CPython `3.14.5`,
  `install_only_stripped`. Cache en `~/.fitz/cache/pbs/` paralelo
  a `git_dep`. Subprocess `curl`. 10 unit tests.
- ✅ **8.b.2** — Real binary codegen validado sin cambios
  funcionales. El output de `fitz build --features python` es
  exactamente el "real binary" que el launcher embebe. Smoke
  Windows: 180 KB binario, depende dinámicamente de `python3.dll`
  stable ABI shim.
- ✅ **8.b.3** — Launcher binary template (`src/launcher_template.rs`).
  Datasette Desktop-style: extrae tarball + real binary a
  `$TMPDIR/fitz-py-<hash>/`, setea PYTHONHOME + ENV de lib según
  OS, exec/spawn del real binary. Subprocess `tar -xzf` (bsdtar
  Win11/macOS, GNU tar Linux). Cero deps Rust en el launcher.
  Hash FNV-1a 16-char. 11 unit + 2 E2E.
- ✅ **8.b.4 + 8.b.5** (combinados) — Flag CLI `--bundle-python`
  + función nueva `main::build_file_with_bundle()` que orquesta
  el pipeline. Validaciones tempranas (host triple, programa usa
  `from python import`). Validado end-to-end manual: 21.8 MB
  binario Windows que corre sin Python en PATH.
- ✅ **8.b.6** — Tests E2E del error path (`tests/bundle_python_e2e.rs`).
  2 tests verdes sin red ni Python. El happy path no se testea
  en CI por costo (~10s download tarball + Python 3.14 requerido
  en builder).
- ✅ **8.b.7** — Cap 21.11 nuevo "`fitz build --bundle-python` —
  binario standalone" en `docs/guide.md` (renumeración
  21.11→21.12, 21.12→21.13). Ejemplo `examples/python-interop-8.b.fitz`
  runnable. README footnote § actualizado con emphasis del
  diferencial.
- ✅ **8.b.8** — Cierre formal: CHANGELOG v0.9.40, esta sección,
  deudas, smoke GUIDE_EXAMPLES_COMPILE verde (sin regresión del
  codegen normal).

**Decisiones técnicas tomadas al arrancar**:

- **Versión Python embebida**: CPython 3.14.5 (último stable en
  el rango `abi3-py310` que PyO3 soporta). Decisión confirmada
  con el autor al ver que la máquina del autor tiene 3.14.2
  instalado (no 3.13) — switch de 3.13.x → 3.14.5 alinea con
  el host actual sin pedir instalar versión extra.
- **Sabor del tarball**: `install_only_stripped` (~70% más chico
  que `install_only` por eliminación de debug symbols).
- **Modelo de bundling**: launcher pattern (Datasette-style).
  Descartamos: extract-on-first-run naive (no funciona — el OS
  resuelve libpython ANTES de `main()` en las 3 plataformas);
  linking estático con PBS full (PyOxidizer-style,
  "multi-month rabbit hole"); delay-load/dlopen manual (sin
  soporte en PyO3, brittle entre versiones).
- **Cache TMP extract**: `$TMPDIR/fitz-py-<hash>/` con sentinel
  `.extracted`. Rename atómico para evitar races concurrentes.
  Hash determinístico (FNV-1a) → cada versión bundleada tiene
  su propio dir; cambio del bundle = re-extract automático.
- **Tar subprocess** (no `flate2 + tar` crates Rust): bsdtar
  está nativo en Windows 11 (`C:\WINDOWS\system32\tar.exe`),
  macOS, y todo Linux moderno. Cero deps en el launcher.
- **Curl subprocess** para descarga (no `ureq`/`reqwest`):
  garantizado en Win11/macOS/Linux moderno. Cero deps Rust
  nuevas.

**Cierre parcial de `R.bug-pyo3-abi3-portable-link`** + reclasificación final v0.9.56:

El launcher pattern bypasea el bug en Windows completamente: el
real binary linkea contra `python3.dll` (stable ABI shim REAL) y
cualquier libpython 3.10+ del bundle satisface la dependencia.

En Linux/macOS, la verificación empírica del 2026-05-24 confirmó
que **no es un bug cerrable** — es un constraint arquitectural de
PyO3 + Linux/glibc. El archivo `libpython3.so` que traen las
imágenes `python:3.X-slim` es un dummy de 13 KB sin símbolos del
API Python (verificado con `nm -D`). En Linux NO hay equivalente
al stable ABI shim de Windows; los símbolos abi3 viven solo en
la libpython versioned (`libpython3.X.so.1.0`).

Consecuencia permanente: el bundle PBS 3.14.5 exige builder con
Python 3.14.x en Linux/macOS — no es temporal. Ver
`docs/deudas_lenguaje.md` para detalle empírico y razones.

**Tests al cierre**:

- **25 tests nuevos** específicos de Fase 8.b: 10 unit (pbs) +
  11 unit + 2 E2E (launcher_template) + 2 E2E (bundle_python).
- **Smoke `GUIDE_EXAMPLES_COMPILE`** sigue verde (sin regresión
  del codegen normal).
- **Smoke manual end-to-end Windows**: validated
  bit-a-bit con programa interop simple (math.pi + math.sqrt)
  ejecutado SIN Python en PATH.

**Deuda residual de Fase 8.b** (NO bloquea uso real, abre items
para iteración 2 si aparece presión):

- ~~**Bundling de pip packages**~~ ✓ **CERRADO 2026-05-23 (Fase 8.c
  v0.9.41)**. `--bundle-pip <pkg>` repetible empaqueta paquetes
  pip junto al CPython base. Ver "Fase 8.c" abajo.
- **Boilerplates 5/6 simplificación**: con `--bundle-pip` los
  Dockerfiles podrían `FROM scratch` o `FROM distroless` en
  lugar de `FROM python:3.X-slim`. Ahorro imagen ~150 MB → ~40
  MB.
- **Linux/macOS smoke end-to-end**: solo validado en Windows
  hoy. Los primeros usuarios en Linux/macOS confirmarán el
  flow ahí.
- **Bundle más chico vía stdlib stripping**: ~30% reducción
  posible eliminando módulos no usados.
- **Hash SHA256** en lugar de FNV-1a (mayor defensa contra
  cambios silenciosos del PBS upstream).

### Fase 8.c — `fitz build --bundle-pip` (paquetes pip embebidos) — CERRADA (2026-05-23)

Continuación natural de Fase 8.b en la misma sesión. Habilita
`--bundle-pip <PACKAGE>` repetible para empaquetar paquetes pip
junto al CPython base. Implica `--bundle-python` automáticamente.
El binario resultante embebe CPython 3.14.5 + paquetes pip + el
real binary, todo en un solo archivo standalone.

**Decisiones técnicas tomadas al arrancar** (confirmadas con el
autor):

- **Modelo de embedding**: tarball secundario embebido (sobre
  pip install en primer run / tarball PBS custom per-user). El
  launcher embebe DOS tarballs (PBS base compartido + pip per-
  proyecto), separación de concerns limpia. Reusa ~95% de la
  infra 8.b.
- **Smoke**: requests (~500 KB) para tests rápidos + sqlalchemy
  como caso real de boilerplates 5/6 (con #[ignore] por costo
  CI). Validado smoke manual con requests; sqlalchemy queda
  como deuda derivada.
- **Boilerplates 5/6 simplificación**: entra al mismo release
  v0.9.41 vía actualización de READMEs con plan concreto.
  Dockerfiles funcionando se mantienen sin cambios; smoke
  Docker real es deuda derivada nueva.

**Sub-pasos cerrados** (8 sub-pasos):

- ✅ **8.c.1** — Flag CLI `--bundle-pip <PACKAGE>` repetible en
  `Commands::Build`. Acepta version pin nativo de pip
  (`"sqlalchemy==2.0.0"`). Implica `--bundle-python` (ambos
  rutean a `build_file_with_bundle`).
- ✅ **8.c.3** — Launcher template extendido con 2 placeholders
  nuevos (`PIP_DECL_BLOCK` + `PIP_EXTRACT_BLOCK`).
  `gen_launcher_main_rs(...)` suma param
  `pip_packages_path: Option<&str>`. None = backward compat con
  8.b (template bit-a-bit idéntico). Bloque de extracción
  cubre Windows (`python/Lib/site-packages`) y Unix
  (`python/lib/python3.X/site-packages` buscado dinámico).
- ✅ **8.c.2 + 8.c.4** — Pipeline pip install integrado en
  `build_file_with_bundle`. Si `bundle_pip` no vacío: extrae
  PBS a cache local del proyecto, ejecuta `pip install --target`
  con `--quiet` + `--no-warn-script-location`, empaca el
  resultado en `<bin>_pip_packages.tar.gz`. Hash combinado
  (PBS bytes + pip bytes) para que dos proyectos con distintos
  pkgs tengan TMP dirs distintos.
- ✅ **8.c.5** — Tests E2E nuevos
  (`bundle_pip_implica_bundle_python_y_aborta_sin_from_python_import`,
  `bundle_pip_repetible_acepta_varios_paquetes`). Validación
  temprana sin red ni Python. Smoke manual real con requests
  validado: 22.9 MB binario Windows + cold ~5s + warm ~50ms +
  `requests.__version__ = "2.34.2"` sin Python en PATH.
- ✅ **8.c.6** — Cap 21.12 nuevo "`fitz build --bundle-pip` —
  empaquetar paquetes pip" en `docs/guide.md` + ejemplo
  runnable `examples/python-interop-8.c.fitz`. Renumeración:
  21.12 (CRUD)→21.13, 21.12 (Limitaciones)→21.14 (fix de bug
  de doble 21.12 que quedó de la renumeración previa en 8.b.7).
- ✅ **8.c.7** — READMEs boilerplates 5/6 actualizados con plan
  concreto de simplificación: builder Python 3.14 +
  `--bundle-pip sqlalchemy psycopg2-binary` + runtime
  `gcr.io/distroless/cc-debian12`. Imagen ~150 MB → ~80-100 MB.
  Dockerfiles actuales mantenidos sin cambios (smoke real
  Docker como deuda residual nueva — el primer user que lo
  pruebe confirma el flow Linux completo).
- ✅ **8.c.8** — Cierre formal: CHANGELOG v0.9.41, esta
  sección, deudas actualizadas (Fase 8.b suma marca de cierre
  del bullet "Bundling pip packages"), CLAUDE.md actualizado,
  README footnote § actualizado con énfasis fuerte del flag
  como diferencial único.

**Tests al cierre**:

- **4 tests nuevos** específicos de Fase 8.c: 2 unit (launcher
  template pip placeholders + Windows path escape) + 2 E2E
  (flag CLI validation + repetible). Acumulado con 8.b: **29
  tests** del bundling entero.
- **2263 unit tests totales** sin feature (smoke regresión
  verde). Smoke `GUIDE_EXAMPLES_COMPILE` sigue verde.

**Smoke manual end-to-end validado** (Windows):

```
$ fitz build --bundle-pip requests examples/python-interop-8.c.fitz
→ compilando real binary…
→ asegurando PBS tarball (cpython 3.14.5 / x86_64-pc-windows-msvc)…
→ extrayendo PBS al cache local para correr pip (1 paquete(s))…
→ pip install --target (1 paquete(s))…
→ empacando pip_packages.tar.gz…
→ compilando launcher…
✓ binario standalone (CPython 3.14.5 + 1 pip pkg(s) embebidos):
  python-interop-8.c.exe (22.9 MB)

# Sin Python en PATH:
$ ./python-interop-8.c.exe
Módulo requests cargado desde el bundle pip:
requests
2.34.2
```

**Deuda residual de Fase 8.c** (NO bloquea uso real):

- ~~**Smoke real Docker de boilerplates 5/6 con --bundle-pip**~~
  ✓ **CERRADO 2026-05-23 (v0.9.42)** — smoke alternativo en
  workspace temp con programa flat (`from python import` solo
  en main, sin módulos transitivos) corrió end-to-end VERDE:
  binario 37.4 MB con CPython 3.14.5 + `requests` embebido,
  ejecutado adentro de container `debian:bookworm-slim`, GET
  `/version` devuelve `"2.34.2"` (versión de `requests`) sin
  Python en el runtime. **3 blockers descubiertos** en el path
  de los boilerplates originales (5/6) que bloquean su
  simplificación directa: (a) ~~**deuda del codegen Fase 8.7.1**
  rechaza `from python import` en módulos transitivos~~ ✓
  **CERRADO 2026-05-23 (v0.9.43)** + ~~**sub-deuda 1.5/1.6**
  coerción y impls HTTP para tipos importados~~ ✓ **CERRADO
  2026-05-24 (v0.9.44)** — el codegen reusa helpers del preludio
  Python del crate root via `use crate::__fitz_py_*`, emite
  statics + getters locales por módulo, Y emite los helpers
  `__fitz_py_to_instance_<T>` + impls `__ToFitzJson`/
  `__FromFitzJson` también para tipos custom definidos en módulos
  transitivos (los módulos los referencian con `crate::__fitz_py_*`
  vía post-procesamiento del output). Smoke validado: `fitz
  build` del boilerplate 5 compila limpio end-to-end. (b) **GLIBC
  mismatch** entre `python:3.14-slim` (trixie 2.39) y
  `debian:bookworm-slim` (2.36) → fix pinear builder a
  `python:3.14-slim-bookworm`; (c) **beneficio real ~10-20 MB
  de imagen** (no 50-70 MB del plan original) — binario embedded
  con CPython pesa ~37 MB que compensa el ahorro de no tener
  Python en runtime. Dockerfiles de boilerplates 5/6 quedan con
  `python:3.12-slim` + `fitz run` hasta que se aplique el ajuste
  GLIBC del builder. READMEs actualizados.
- **Constraint Linux/macOS heredado**: builder requiere Python
  3.14.x (R.bug-pyo3-abi3-portable-link componente Linux/macOS
  reclasificado v0.9.56 como constraint arquitectural permanente
  tras verificación empírica). El requerimiento del builder es
  estructural, no cerrable.
- **C extensions cross-platform**: `pip install` al build time
  baja wheels específicos del triple del builder. Buildear
  Linux desde Windows requiere `cross` o Docker (igual que
  todo cross-compile Rust).
- ~~**`--bundle-pip-requirements <file>`**~~ ✓ **CERRADO
  2026-05-23 (cosecha 8.c v0.9.42)** — flag repetible que lee
  paquetes desde un `requirements.txt` estándar. Implica
  `--bundle-python` igual que `--bundle-pip` y es combinable
  con éste (pip acumula). Sin parsing del lado de Fitz: el
  archivo se pasa directo a `pip install -r <file>`, toda la
  sintaxis nativa funciona (comments, includes, version pins,
  `--hash`, etc.). Validación temprana fail-fast antes de tocar
  PBS/pip. 3 E2E tests nuevos (total 7/7 del bundling). Cap
  21.12 actualizado con sub-bloque dedicado.
- ~~**Cache key por lista de paquetes**~~ ✓ **CERRADO
  2026-05-23 (v0.9.42, deuda D)** — helper
  `pip_inputs_hash(bundle_pip, requirements_contents) ->
  String` con FNV-1a 64-bit sobre positionals ordenados +
  bytes de los requirements files (separador `\n---\n` entre
  las dos secciones). Sidecar `<bin>_pip_packages.inputs_hash`
  adyacente al tarball. Cache hit reusa el tarball sin re-
  correr pip install + tar (builds subsiguientes sin cambios
  pasan a ~instantáneo). 8 unit tests verdes del helper:
  determinismo, insensibilidad al orden de positionals,
  sensibilidad al cambio de contenido del requirements, no
  colisión entre positionals y requirements vía separador,
  shape de 16 chars hex. Sensibilidad al **orden** de
  requirements files preservada (pip los procesa en orden con
  potenciales conflicts/overrides — conservadora).
- **Distroless runtime requiere `tar` embebido en Rust** —
  nueva deuda derivada del smoke real Docker. El launcher de
  `--bundle-python` invoca `Command::new("tar")` subprocess
  para extraer el PBS embedded al primer run (decisión de
  diseño consciente: "cero deps Rust en el launcher").
  `gcr.io/distroless/cc-debian12` NO trae tar → bloquea esta
  opción de runtime. `debian:bookworm-slim` (~85 MB base con
  tar) es el runtime mínimo viable hoy. Mover a distroless
  requiere un crate de tar inline (sub-paso futuro de la deuda
  menor del launcher).
- **Drift extensión VSCode (audit 2026-05-23, v0.9.42)** ✓
  CERRADO — grammar TextMate faltaba 15 builtins (`spawn` +
  5 Bits-extras + 9 Math), LSP scope_level_completions
  faltaba 5 Bits-extras. Fixeado en ambos. Extensión bumpeada
  a 0.9.3, `.vsix` re-construido. Próximo workflow_release del
  CI publicará binarios alineados.

### Decisiones cross-cutting

Estas decisiones no caen en un sub-paso específico; son
consistencias que la fase entera tiene que mantener.

1. **Sintaxis de import desde Python**. Opciones consideradas:
   - **(A)** `from python import sqlalchemy` (namespace virtual
     `python` reservado).
   - **(B)** `import py:sqlalchemy` (prefijo de scheme estilo
     URI).
   - **(C)** `@python use sqlalchemy` (decorator a nivel
     módulo).
   - Recomendación: **(A)**. Reusa la sintaxis `from X import Y`
     que el usuario ya conoce, no introduce caracteres
     especiales nuevos, el AST puede reusar `Stmt::FromImport`
     con un discriminante. Es la forma más cercana a
     "se siente igual que un import normal pero el target es
     Python".

2. **Aliasing en imports (`as`)**. Fitz hoy no lo soporta para
   imports normales; agregarlo es necesario para Python (los
   módulos `sqlalchemy.orm.declarative_base`, etc., son
   imposibles de usar sin alias). Decisión: cerrar la sub-deuda
   en 8.1 — agregar `as` para imports normales también, no
   solo Python. Beneficio bonus para el sistema de módulos
   Fitz existente.

3. **Dependencia Python como condicional**. Un proyecto Fitz
   que no usa `from python import` no debería pagar nada
   (tamaño del binario, tiempo de compilación, dependencia
   runtime). Implementación: el codegen detecta presencia de
   imports Python en el AST; sólo entonces emite el código de
   embedding y la dependencia a PyO3 en `Cargo.toml`. Build
   sin interop = binario igual al de la Fase 5b. **Esta
   decisión preserva la promesa "binario nativo standalone"
   como modo por default.**

4. **Identidad en marshaling**: cubierto en 8.2. Default:
   copia eager bidireccional, sin aliasing entre los dos
   runtimes. Optimizaciones (zero-copy) son trabajo de Fase
   7+.

5. **Herencia desde clases Python**: explícitamente **NO
   soportada** en Fase 8. Un `type` de Fitz no puede heredar
   de una clase Python. Composición sí — un `type` puede
   tener un campo `engine: Any` que envuelve un objeto Python.
   La razón: herencia cruza modelos de objetos (Python tiene
   MRO, Fitz no tiene clases) y abre una caja de pandora que
   no aporta al caso de uso central.

6. **Versionado de Python soportado**: ABI3 para amortizar
   versiones. Mínimo Python 3.10 (versión más vieja con
   soporte upstream a fecha de cierre de la fase; revisar al
   arrancar 8.1).

7. **Métodos sobre objetos Python (`obj.method()`)**: el
   dispatch sobre Any/PyObject cae al modelo gradual del
   checker existente (5.3.4). Sin trabajo nuevo. Anotaciones
   explícitas siguen disponibles para casos donde se quiere
   precisión.

### Trade-offs reconocidos

La fase tiene costos reales que vale la pena enumerar honestamente:

- **Tensión con "binario nativo standalone"** (cap "Lo que
  Fitz NO es" de vision.md). Resolución: dos modos de build
  — puro nativo (sin interop, igual que Fase 5b) y nativo +
  CPython embebido (con `--bundle-python`). El usuario elige
  al deployar. La promesa original se preserva como modo
  default; la interop es opt-in al nivel del proyecto.
- **El GIL limita la concurrencia** justo donde Fitz quiere
  brillar (handlers HTTP concurrentes). Mitigado en 8.6 con
  soltado oportunístico del GIL en llamadas async, pero es
  un techo real para APIs CPU-intensivas en Python.
- **Costo de marshaling** Rust↔Python en cada cruce. Un
  handler que hace 5 queries SQLAlchemy genera 10+ cruces.
  Cuantificable; los benchmarks deberían formar parte del
  criterio de cierre de 8.6.
- **Riesgo de identidad del lenguaje**: si los usuarios usan
  Python para TODO (HTTP, queries, JSON, lógica de negocio),
  Fitz queda como cáscara sintáctica. Mitigación: la guía y
  los ejemplos muestran el patrón explícitamente —
  "Python para librerías pesadas, Fitz para lo demás" — y
  los handlers de la guía siguen siendo `fn` Fitz puros con
  Result, no envoltorios delgados.
- **Tooling complejo**: la fase introduce dos artefactos
  nuevos (CPython embebido, herramienta `fitz py-types`),
  dos dependencias externas pesadas (PyO3,
  posiblemente PyOxidizer), y una superficie nueva de bugs
  que cruzan dos runtimes (refcount Rc + GC Python).
  Estimar tiempo de implementación realista: probablemente
  meses, no semanas.

### Precedentes consultados

Lenguajes y proyectos de los que esta fase aprende:

- **PyO3** — la biblioteca base. Madura, mantenida, con tres
  patrones de uso (extension modules, embedding, shared
  layout). Fitz usa embedding como base, eventualmente puede
  sumar extension modules para "Fitz como librería de
  Python" en una fase futura.
- **pyo3-asyncio** — bridge tokio ↔ asyncio. Crítico para
  8.6.
- **PyOxidizer** — para 8.7, empaquetado de CPython en
  binarios standalone. Revisar estado del proyecto.
- **Mojo** (Modular) — superset sintáctico de Python con
  interop directa. Lección: ellos eligieron compatibilidad
  sintáctica total. Fitz NO tiene esa restricción (y la
  rechaza explícitamente — ver "Posicionamiento estratégico").
- **Nim + nimpy** — precedente más directo. Lenguaje
  compilado a nativo que importa Python. Resolvió bien la
  sintaxis (`pyImport`) y el marshaling automático. Vale la
  pena leer el código de nimpy antes de 8.1.
- **Julia + PyCall** — lenguaje JIT con interop Python.
  Resolvió el GIL con threading separado. Aplicable
  parcialmente (Fitz no es JIT, pero el modelo de GIL
  traduce).
- **Crystal + Python bindings** — lenguaje compilado con
  macros para envolver código externo. Menos relevante para
  el caso DB, más para extender el lenguaje con C.

### Riesgos

- **Magnitud de la fase**: probablemente la más larga del
  proyecto hasta acá. Estimación gruesa: 3-6 meses de
  trabajo enfocado, muy por encima de cualquier fase de la
  5b. Antes de arrancar 8.1, decidir explícitamente si la
  fase entra completa o se parte en (8 — embedding y
  marshaling, suficiente para casos simples) + (un sub-paso
  posterior — SQLAlchemy/async/bundling, llevándolo al caso
  de uso canónico). Decisión a tomar al cerrar Fase 7.
- **Dependencia externa pesada**: la fase atan Fitz a PyO3,
  al equipo PyO3, y por extensión al ABI de CPython. Si
  CPython cambia su API de embedding (cosas como GIL-free
  Python en 3.13+), Fitz se ve afectado. Mitigación: ABI3
  amortiza versiones, pero no protege contra cambios
  estructurales.
- **Fragmentación de proyectos**: parte de los proyectos
  Fitz usan interop, parte no. Convivencia, distribución,
  CI, todo se complica. La decisión cross-cutting #3
  (dependencia condicional) mitiga pero no elimina.
- **Riesgo de canibalización**: si interop Python es
  demasiado bueno, ¿por qué alguien escribiría una librería
  en Fitz puro? Mitigación: la performance del código Fitz
  nativo tiene que seguir siendo claramente mejor que llamar
  a Python (binario sin GIL, sin marshaling), y la
  ergonomía de Fitz nativo (HTTP, tipos, Result) tiene que
  estar varios pasos arriba de lo equivalente en Python.

### Alternativa explícita: ORM y stack DB nativos (Fase 10+)

A futuro, Fitz debería tener su propio stack de DB nativo:
- Driver Postgres en Fitz puro (bindings directos a `libpq`
  o port de `tokio-postgres` al codegen de Fitz).
- ORM nativo declarativo sobre `type` (estilo Diesel o sqlx).
- Migraciones, pool de conexiones, async nativo end-to-end.

Eso es un proyecto en sí mismo, probablemente Fase 10+. La
interop Python de la Fase 8 es **el puente** hasta llegar
ahí, no el destino final. Vale la pena decirlo explícitamente
en la documentación que la fase produce: "interop existe
para que Fitz sea usable hoy; el stack nativo llega cuando
lleguemos".

### Features de la fase entera
- [ ] Embedding básico de CPython + sintaxis `from python
  import` (8.1)
- [ ] Aliasing en imports (`as`) — sub-deuda de 8.1 que
  beneficia a imports normales también
- [ ] Marshaling List/Map/Instance/Null ↔ list/dict/None
  (8.2)
- [ ] Excepciones Python → Result<T> automático (8.3)
- [ ] Anotaciones explícitas del lado Fitz + opacidad PyObject
  (8.4)
- [ ] Stubs `.pyi` — pospuesto a Fase 9+
- [ ] Auto-mapeo SQLAlchemy → `type` via `fitz py-types` (8.5)
- [ ] Bridge tokio ↔ asyncio + política de GIL (8.6)
- [ ] Distribución con `--bundle-python` (8.7)
- [ ] Guía + ejemplo CRUD + cierre formal (8.8)

---

## Fase 7 — DX HTTP ✓
**Estado: COMPLETADA (2026-05-13)** — 1150 tests pasando al cerrar.

Con async nativo cerrado (Fase 6), Fitz cumple la promesa de
HTTP de primera clase a nivel de ejecución. Falta la otra mitad
de la paridad con FastAPI: **documentación de la API
autogenerada**. Toda la información necesaria ya vive en el
`HttpRegistry` + `TypeEnv` (verbo, path, tipos de path params,
**query params** ya cerrados en post-5b, schema del body,
return type, defaults, nullables, **status codes custom** ya
cerrados en post-5b). Esta fase camina ese metadato a un
OpenAPI 3.1 + UI embebida.

### Decisiones de diseño tomadas

1. **Built-in en el runtime HTTP, no librería externa**. Coherente
   con "HTTP es parte del lenguaje" (decisión de diseño 1 del
   proyecto). Sin imports, sin configuración — viene gratis.
2. **Scalar > Redoc > Swagger UI**. Scalar es la opción más
   moderna (mantenida activamente, mejor look default, más
   liviana). Redoc es alternativa razonable; Swagger UI queda
   descartado por peso y look anticuado.
3. **Scalar vía CDN** (confirmado en sesión post-6). El HTML
   embebido es ~10 líneas que cargan el bundle de Scalar desde
   unpkg/jsdelivr. Cero peso extra al binario; trade-off
   aceptado: la primera carga necesita red (después el
   navegador cachea). Mismo patrón que FastAPI con Swagger.
   Bundle local queda como deuda post-7 si aparece presión.
4. **Default `enable_docs: true`, opt-out con
   `@server(docs=false)`**. Que el camino feliz "fitz build &&
   ./bin" entregue `/docs` sin tocar nada. Quien no quiera la
   superficie extra lo apaga explícito.
5. **Mismo schema en `fitz run` y `fitz build`**. El generador
   vive en un módulo nuevo `src/openapi.rs` reusado por ambos
   pipelines. Bit-a-bit idéntico es el contrato.
6. **Kwargs en decoradores como sub-paso dedicado** (confirmado
   en sesión post-6). En lugar de hack-ear `@server(docs=...)`
   como caso especial, sumamos kwargs generales al parser/AST
   de decorators: `@deco(pos1, pos2, key=value, ...)`. Sirve
   para 7.4 (`@server(docs=false)`) y desbloquea futuros
   decorators con configuración. Sub-paso 7.0.
7. **Doc-strings sobre handlers (descripciones OpenAPI):
   pospuestos** (confirmado en sesión post-6). El parser hoy
   descarta comentarios; retenerlos como doc-strings es
   refactor mediano (lexer + parser + AST + tests). F7 sale
   con `summary`/`description` ausentes en el schema. Refresh
   menor cuando la deuda se cierre.

### Pasos

#### 7.0 — Kwargs en decoradores ✓
**Cerrado** — pre-requisito para `@server(docs=false)` y para
cualquier decorator futuro con configuración no-posicional.

- Parser: `@deco(pos1, pos2, key1=value1, key2=value2)` — los
  kwargs van al final, después de los positionals. Sintaxis
  alineada con Python/Rust (que ya conocen la mayoría de los
  usuarios objetivo de Fitz).
- AST: `Decorator { name: String, args: Vec<Expr>, kwargs:
  Vec<(String, Expr)> }`. Mantenemos los positionals como
  `Vec<Expr>` para compat; los kwargs van separados.
- Checker: validación de tipos de cada kwarg igual que un arg
  posicional (cada decorator define qué kwargs acepta).
- Evaluator: `process_decorator` consume kwargs sin tocar el
  flow general; el handler de cada decorator (`@server`,
  `@get`, etc.) decide cuáles acepta.
- Tests: parser smokes para `@get("/x", a=1)`, validar mezcla
  positional + kwarg, rechazar `@get(a=1, "/x")` (positional
  después de kwarg), rechazar kwargs duplicados.

**Criterio de éxito**: `@server(3000, host="0.0.0.0",
docs=false)` parsea, chequea, y evalúa correctamente — con
`docs=false` aún no haciendo nada (lo cierra 7.4 cuando el
campo se exponga en `ServerConfig`).

#### 7.1 — Generador de schema OpenAPI + subcomando `fitz openapi` ✓
**Cerrado** — base de la fase.

- Nuevo módulo `src/openapi.rs` con
  `generate_openapi(registry: &HttpRegistry, type_env: &TypeEnv)
  -> serde_json::Value`. Schema OpenAPI 3.1.
- Walker que recorre el registry: por cada ruta, emite
  `paths.<path>.<method>` con:
  - `parameters` para path params (`in: path`, tipo).
  - `requestBody` para POST/PUT con body tipado, con
    `$ref: "#/components/schemas/<TypeName>"`.
  - `responses` con `200` (return type serializado) y `500`
    (sólo si el return es `Result<T>`).
- `components.schemas` con un `JSON Schema` por cada `type`
  Fitz declarado, incluyendo defaults, nullables, required.
- Subcomando nuevo `fitz openapi archivo.fitz` que escupe el
  JSON a stdout. Útil para CI, para generar SDKs con
  openapi-generator, para snapshot testing.
- Tests: schema generado para un `@get` simple, schema con
  body tipado, schema con `Result<T>`, schema con nullables y
  defaults.

#### 7.2 — Endpoint `/openapi.json` autoregistrado en `fitz run` ✓
**Cerrado** — runtime HTTP sirve el schema.

- El runtime axum suma la ruta `/openapi.json` automáticamente
  cuando hay decorators HTTP en el programa.
- El handler de `/openapi.json` invoca `generate_openapi` sobre
  el `HttpRegistry` actual y devuelve el JSON.
- Tests: `curl localhost:3000/openapi.json` devuelve un schema
  válido para `examples/server.fitz`.

#### 7.3 — UI embebida `/docs` con Scalar ✓
**Cerrado** — la pieza visible.

- HTML estático embebido con `include_str!("templates/scalar.html")`
  apuntando al CDN de Scalar (o al bundle si se decide
  empaquetar). El HTML carga `/openapi.json` con fetch.
- Ruta `/docs` autoregistrada igual que `/openapi.json`.
- Tests: `curl localhost:3000/docs` devuelve HTML válido;
  smoke E2E que abre el server, hace GET a `/docs`, y verifica
  que el HTML referencia `/openapi.json`.

#### 7.4 — Flag `@server(docs=false)` para opt-out ✓
**Cerrado** — control de superficie.

- `ServerConfig` suma campo `enable_docs: bool` (default `true`).
- Aprovecha los kwargs de 7.0: `@server(...)` acepta el kwarg
  `docs: Bool`. El handler de `@server` en el evaluator lee
  `kwargs` y popula `enable_docs` si está; si no, default
  `true`.
- Cuando `enable_docs: false`, las rutas `/openapi.json` y
  `/docs` NO se registran (ni en `fitz run` ni en el binario
  de `fitz build`).
- Tests: `@server(3000)` levanta con docs, `@server(3000,
  docs=false)` levanta sin docs (404 en `/docs`).

#### 7.5 — Paridad en `fitz build` ✓
**Cerrado** — el binario nativo también sirve docs.

- Codegen detecta presencia de handlers HTTP y emite las rutas
  `/openapi.json` y `/docs` adentro del `Router` generado.
- El schema OpenAPI se calcula en build time (no en runtime —
  el `HttpRegistry` no existe en el binario generado) y se
  embebe como `&'static str` con `include_str!` o constante
  generada.
- HTML de Scalar embebido como string constante en el código
  Rust generado.
- Tests: programa HTTP con `fitz build` produce binario que
  sirve `/docs` y `/openapi.json` con el mismo schema que
  `fitz run`.

#### 7.6 — Headers como params del handler ✓
**Cerrado** — sintaxis confirmada: **Opción A — decorator
dedicado** `@header(name="HTTP-Name")` apilado antes del decorator
de ruta. Aprovecha kwargs de 7.0; sin cambios al parser.

Convención de mapping: lowercase + `-` → `_` deriva el nombre del
param Fitz (`Authorization` → `authorization`, `X-Auth-Token` →
`x_auth_token`). Solo `Str` o `Str?`. Lookup case-insensitive en
HTTP.

Runtime: `HeaderSpec` en `RouteSpec`; `HeaderMap` extractor en
cada case de `build_method_router`; `handle_task` bindea con 400
si obligatorio falta. Codegen del wrapper paralelo. OpenAPI emite
parameters con `in:"header"`.

15 tests dedicados (eval 6, runtime E2E 4, openapi 2, codegen 3).
Smoke manual: paridad bit-a-bit fitz run ↔ fitz build.

#### 7.7 — Guía + ejemplo + cierre formal de Fase 7 ✓
**Cerrado** — documentación viva al día.

- Cap 18 nuevo "Docs automáticas" en `docs/guide.md` (entre cap
  17 HTTP y cap 19 Async). Renumeración: 18-async → 19, 19-build
  → 20, 20-qué-sigue → 21.
- Ejemplo ejecutable `examples/guide/18-docs.fitz` (CRUD chico
  sin state compartido — compila con `fitz build`). Renombrados
  `18-async.fitz` → `19-async.fitz` y `19-build.fitz` →
  `20-build.fitz`. Smoke `GUIDE_EXAMPLES_COMPILE` actualizado.
- Cap 17 (HTTP) menciona el cap 18 al cierre. Cap 21 (Qué sigue)
  marca Fase 7 cerrada y apunta a Fase 8 como próximo norte.
- README y `docs/roadmap.md` actualizados.
- `docs/deudas-post-5b.md` suma párrafo de cierre F7 con deuda
  residual (middleware/CORS comprometido, doc-strings, status
  codes custom en schema, aliases @header, bundle Scalar offline).

### Decisiones cross-cutting

1. **Schema OpenAPI vs JSON Schema interno**: usamos OpenAPI
   3.1 que incluye JSON Schema completo (resolvió la
   incompatibilidad histórica con 3.0). Compatible con
   herramientas tipo openapi-generator, Scalar, Postman, Insomnia.
2. **Tags y descripciones desde comentarios**: pospuesto. Hoy
   el parser no retiene comentarios; agregar eso es deuda
   nueva. Sub-paso candidato post-7.7 si aparece presión.
3. **Versionado de la API**: hoy el schema declara
   `info.version` fijo en `"0.1.0"`. Habilitar override via
   `@server(api_version="...")` es deuda chica, pospuesta.

### Features de la fase entera
- [x] Kwargs en decoradores (7.0) — pre-req
- [x] Generador de schema OpenAPI + `fitz openapi` (7.1)
- [x] `/openapi.json` autoregistrado en `fitz run` (7.2)
- [x] `/docs` con Scalar via CDN (7.3)
- [x] `@server(docs=false)` opt-out (7.4)
- [x] Paridad en `fitz build` (7.5)
- [x] Headers como params del handler con `@header(name="X")` (7.6)
- [x] Guía + ejemplo + cierre formal (7.7)

### Cierre formal de Fase 7

Criterio de éxito original: `examples/server.fitz` corriendo con
`fitz run` expone `/openapi.json` con schema válido y `/docs`
con UI Scalar interactiva. El mismo programa compilado con `fitz
build` sirve ambas rutas con el mismo schema bit-a-bit.
`@server(3000, docs=false)` apaga ambas.

Cumplido al 100%:

- ✓ `/openapi.json` y `/docs` autoregistrados en `fitz run` y
  en el binario nativo de `fitz build`.
- ✓ Schema bit-a-bit idéntico entre los 3 caminos (`fitz run`,
  `fitz openapi`, `fitz build`).
- ✓ `@server(docs=false)` apaga ambas rutas (404).
- ✓ `@header(name="X")` para headers como params del handler.
- ✓ Subcomando `fitz openapi archivo.fitz` (CI / generación de
  SDKs / snapshot testing).
- ✓ Cap 18 nuevo en la guía + ejemplo `18-docs.fitz` compilable
  end-to-end.

Tests al cerrar: **1150 totales** (1080 unit + 67 codegen E2E +
3 openapi E2E). Distribución de tests nuevos en Fase 7: 7.0 (+8),
7.1 (+23), 7.2 (+4), 7.3 (+3), 7.4 (+4 netos), 7.5 (+7), 7.6
(+15), 7.7 (smoke +1 implícito en `GUIDE_EXAMPLES_COMPILE`).

### Deuda residual de Fase 7 (post-7.7)

Visible y comprometida — algunas con prioridad alta porque la
promesa "HTTP nativo" todavía cojea sin ellas:

- **Middleware y CORS** — **CERRADO en mini-fase MW** (ver
  sección abajo). Decorator `@middleware(fn)` apilable +
  built-in `cors(...)`. Sub-pasos MW.1 (intérprete) → MW.4
  (guía + cierre). Total 1189 tests al cerrar.
- **Doc-strings sobre handlers** (descripciones OpenAPI): el
  parser hoy descarta comentarios. Retenerlos como doc-strings
  es refactor mediano (lexer + parser + AST + tests). F7 sale
  con `summary`/`description` ausentes en el schema.
- **Status codes custom en el schema**: handlers que hacen
  `return 404 { ... }` (cap 17) no aparecen en `responses` del
  schema — solo `200` y `500`. Cazarlos requiere análisis del
  body del handler.
- **Aliases en `@header`**: hoy el nombre del param Fitz se
  deriva por convención (lowercase + `-` → `_`). Permitir
  `@header(name="X-Auth", into="token")` con alias explícito
  sería útil para nombres no convencionales.
- **Bundle Scalar embebido offline**: hoy la UI carga desde CDN
  jsdelivr. Embebir el bundle en `SCALAR_HTML` haría que la UI
  funcione sin red (deuda menor, depende de presión real).
- **`info.version` override**: hoy fijo en `"0.1.0"`. Habilitar
  via `@server(api_version="...")` o similar.

### Pre-reqs / contexto al arrancar Fase 7

- **Fase 6 cerrada** ✓. El runtime HTTP corre sobre tokio
  current_thread con el bridge mpsc/oneshot que sobrevivió a
  6.5 (postergado a F17). Las rutas nuevas `/openapi.json` y
  `/docs` se suman al `Router` que arma `serve()` igual que
  cualquier ruta del usuario.
- **Query params ya cerrados** en una mini-fase post-5b. El
  schema OpenAPI (7.1) los expone en `parameters` con
  `in: query` reusando la metadata existente de `RouteSpec`.
- **Status codes custom ya cerrados** en una mini-fase post-5b.
  El schema OpenAPI debe reflejar los posibles statuses por
  handler — pero como hoy el AST guarda solo `Stmt::ReturnStatus
  { status, body, span }` sin contar el conjunto completo a
  nivel del handler, el primer pass emite solo `200` (caso
  feliz) y `500` (Result Err). Códigos custom específicos
  quedan como deuda menor de F7.
- **F17 sigue pendiente** y no bloquea F7. La fase trabaja
  sobre la superficie del lenguaje (schema + UI), no sobre la
  mecánica interna del runtime.

---

## Mini-fase MW — Middleware + CORS 🛡️
**Estado: CERRADA (1189 tests)**

Sub-paso comprometido post-Fase 7. Server web real necesita
interceptar requests para logging, auth, CORS, rate limiting,
etc. Sintaxis final: decorator `@middleware(fn)` apilable sobre
handlers HTTP + built-in `cors(...)` configurable como un
middleware especial.

### Decisiones cross-cutting

1. **Modelo gate-only para middleware genérico**: el middleware
   retorna `null`/sin valor → la chain continúa; retorna un
   response (`return <status> { ... }`) → short-circuit. Sin
   modelo "wrap" con `next` callable. Pros: simple, sin overhead
   de wrap cuando no se necesita, encaja con `Stmt::ReturnStatus`
   existente. Contras: no permite post-process (medir tiempo
   real, agregar headers custom desde un middleware). CORS lo
   evita con tratamiento especial (slot dedicado + preflight).
   Si aparece presión real por timing/tracing, mini-fase
   dedicada post-F8.
2. **CORS como slot dedicado, no parte de la chain**: aplicar
   `@middleware(cors(...))` carga `RouteSpec.cors` (vs
   `RouteSpec.middlewares` que vive aparte). Razón: CORS
   necesita preflight OPTIONS automático e inyección de
   headers en response real — ambos no expresables con
   gate-only. Trade-off: un caso especial documentado a cambio
   de una API user-facing uniforme (`@middleware(cors(...))`).
3. **`Request` y `Response` como nominales built-in**: pre-
   registrados en `TypeEnv` desde la fase MW. El usuario los
   puede anotar sin `import`. Si declara `type Request {...}`
   propio, gana el error de redeclaración existente. Costo
   semántico: dos nominales fijos en cualquier programa, aún
   sin HTTP. Aceptable.
4. **CORS config en build-time**: `cors({...})` parsea su
   Map literal en codegen (no eval runtime para el bin). El
   resultado se emite como `static __FITZ_CORS_<NAME>` —
   cero allocs por request. El intérprete sí lo evalúa runtime
   (built-in registrado).

### Features de la fase entera

- [x] Decorator `@middleware(fn)` apilable + chain gate-only (MW.1)
- [x] `Request` built-in (method/path/headers) + `Response`
      opaco (MW.1)
- [x] `Stmt::ReturnStatus` adentro de fns referenciadas como
      middleware (MW.1 checker relax)
- [x] Built-in `cors(...)` configurable + slot `RouteSpec.cors` (MW.2)
- [x] Preflight OPTIONS automático + inyección de headers en
      response real (MW.2)
- [x] Codegen completo: chain en wrapper async + cors build-time
      + preflight (MW.3)
- [x] Sub-sección "Middleware y CORS" en cap 17 de la guía +
      `examples/guide/17b-middleware.fitz` (MW.4)

### Cierre formal mini-fase MW

Criterio de éxito: un programa con `@middleware(auth)` y
`@middleware(cors())` aplicados a un handler HTTP responde
identico bit-a-bit entre `fitz run` y `fitz build && ./bin`.
Validado por E2E que arma un programa, lo compila, lo spawnea,
y manda requests crudos por TCP (passthrough 200, short-circuit
401, preflight OPTIONS 204, response real con headers).

Tests al cerrar: **1189 totales** (1118 unit + 71 E2E).
Reparto del aporte de la fase: MW.1 +18 unit + 1 E2E neto; MW.2
+20 unit + 4 E2E router; MW.3 0 unit + 3 E2E netos (un E2E viejo
reemplazado); MW.4 sin tests nuevos (smoke +1 implícito en
`GUIDE_EXAMPLES_COMPILE`).

### Deuda residual de la mini-fase MW

- **Modelo wrap (post-process)** — middleware genérico hoy es
  gate-only. Para timing/tracing/headers post-call hace falta
  un middleware con `next` callable. CORS lo evita con
  tratamiento especial. Mini-fase dedicada post-F8 si aparece
  presión real.
- **CORS request-aware** — `allow_origin` hoy es valor fijo.
  Para echo del Origin recibido (cuando se admite un set
  acotado de orígenes), hace falta inspección de la request
  en build-time del response. Deuda menor.
- **OpenAPI schema con middleware/CORS** — el schema no
  refleja los middlewares aplicados. Para SDKs generados está
  bien (server-side concern), pero docs UI podrían mencionar
  las reglas CORS en `info.description` o un campo custom.
- **Body en `Request`** — hoy el Request expone method/path/
  headers. Body queda en el handler (parseado contra el `type`
  declarado, post-middleware). Para autenticación basada en
  body (HMAC, signing) habría que exponerlo al middleware,
  con el costo de parsear antes del short-circuit. Deuda menor.

---

## Fase 9 — Ecosistema 🌍
**Estado: LSP MVP CERRADO entero (9.x.1 → 9.x.5)** — Fase 9.0
(pre-reqs F15 + F16) cerrada el 2026-05-15; las cinco sub-fases
visibles del LSP cerradas el 2026-05-15/16. Próximo: package
manager + DX + stack web first-class (plan detallado abajo).

- [x] **F15 — error recovery del parser** (Fase 9.0, 2026-05-15) — ver 9.0 abajo
- [x] **F16 — IR tipado persistido por nodo** (Fase 9.0, 2026-05-15) — ver 9.0 abajo
- [x] **LSP + extensión VSCode** (Fase 9.x.1 → 9.x.5, CERRADO 2026-05-15/16) — diagnostics, hover, go-to-def, autocomplete, distribución multi-platform. Ver 9.x abajo.
- [ ] **Package manager + registry** (Fase 9.y) — `fitz.toml`, `fitz new`, `fitz add`, `fitz publish`, registry escrito en Fitz. Ver sección detallada abajo.
- [ ] **DX completo: formatter, test, dev, repl, linter** (Fase 9.z) — `fitz fmt` (cero config), `fitz test`, `fitz dev` (hot reload), `fitz repl`, `fitz lint`. Ver sección detallada abajo.
- [ ] **Stack web first-class: auth, websockets, jobs** (Fase 9.w) — `@authenticated`, `@ws`, `@cron`, `@background` como decoradores nativos. Ver sección detallada abajo.
- [ ] Stubs `.pyi` para interop Python (pospuesto desde Fase 8)
- [ ] Driver Postgres nativo (paso previo al ORM Fitz, ver Fase 10+ en "Visión post-Fase 9")
- [ ] Compilación a WebAssembly (relacionado con Fase 11 frontend, ver "Visión post-Fase 9")
- [ ] Documentación oficial en español e inglés
- [ ] Website del lenguaje

### Fase 9.0 — F15: error recovery del parser ✓ (CERRADA, 2026-05-15)

Primer sub-paso de Fase 9. Cierre formal de la deuda F15
identificada post-5b. Habilita que herramientas externas (LSP,
formatter, futuros analizadores) reciban un AST parcial sobre
buffers en construcción.

**Sin cambio user-facing**: `fitz run`/`build`/`check` siguen
usando `parse()` strict y abortando al primer error de parser. La
API recovering es para tooling externo.

#### 9.0.1 — Nodos Error en AST + parse_with_recovery + tests del parser

- **AST**: nuevas variantes `Expr::Error(Span)` y `Stmt::Error(Span)`.
  Solo las construye `parse_with_recovery`. Mantienen la forma
  estructural del árbol cuando hay errores recuperados.
  Marcadas con `#[allow(dead_code)]` puntual sobre `Expr::Error`
  hasta que aterricen sub-stmt recovery (parser produce
  `Stmt::Error` en 9.0; nunca `Expr::Error` suelto).
- **Parser**: flag interno `recovery_mode` + cota dura
  `MAX_RECOVERED_ERRORS = 100` + helper `synchronize()`.
- **Sync points**: `Newline` (consumido), `RBrace` / `EOF`
  (preservados), keywords que típicamente arrancan stmt:
  `Let`, `Fn`, `Async`, `Type`, `Return`, `Break`, `Continue`,
  `While`, `Loop`, `For`, `If`, `Import`, `From`, `At`
  (preservadas). **La regla de keywords fue necesaria** porque
  `primary()` consume el token actual antes de validar: un
  `Newline` inesperado se consume y el cursor termina parado en
  el `Let` del próximo stmt; sin la parada en keywords, sync se
  comía stmts enteros.
- **API pública nueva**:
  `pub fn parse_with_recovery(tokens) -> (Program, Vec<FitzError>)`.
  Nunca retorna `Err`: los errores se acumulan en la lista
  paralela. `#[allow(dead_code)]` justificado hasta que aterricen
  los consumidores (LSP/formatter).
- **Defensas en evaluator/codegen**: si llegan a ver Error nodes
  (no deberían — strict no los produce), emiten `FitzError` claro
  con span, no panic.
- **Tests del parser** (10 unit nuevos en
  `parser::tests::recovery_*`): programa válido sin errores,
  stmt roto top-level, dos errores consecutivos, recovery dentro
  de `if`/`fn` body, span del Error node apunta al inicio del
  stmt, posición del error apunta al token problemático, EOF
  inesperado se acumula, cota de 100 errores se respeta, fn con
  body roto preserva estructura, parse strict sigue abortando al
  primer error.

#### 9.0.2 — Tolerancia del checker a Error nodes

- **Checker silencioso**: `Expr::Error` sintetiza `Type::Any`,
  `Stmt::Error` no-op. Sin emisión de errores derivados — los
  errores reales viven en la lista del parser.
- **Tests del checker** (5 unit nuevos en `types::tests::checker_*`):
  helper local `check_recovering(src)` que corre el pipeline
  LSP-style (`parse_with_recovery` → `check_program`). Tests:
  Stmt::Error no agrega errores derivados; el silencio sobre
  Error nodes no afecta detección de errores genuinos en stmts
  vecinos válidos; Error nodes en fn body no abortan el check
  del resto del programa; smoke con 3 stmts rotos no panic;
  Expr::Error directo en AST tipa como Type::Any.

#### 9.0.3 — Validación end-to-end + cierre formal

- **Smoke a mano**: `fitz check` sobre un buffer con 3 stmts
  rotos intercalados → exit 1 con un error reportado del primer
  stmt roto (comportamiento strict idéntico a antes).
- **Smoke `GUIDE_EXAMPLES_COMPILE`**: 13 ejemplos compilables de
  la guía siguen verdes.
- **Docs**: CHANGELOG v0.9.0, este roadmap, `deudas-post-5b.md`
  con F15 marcado CERRADO, README refresh.

#### Decisiones técnicas tomadas

- **Representación de errores**: nodos `Expr::Error(Span)` /
  `Stmt::Error(Span)` in-band + `Vec<FitzError>` paralelo. El
  árbol mantiene su forma estructural (mejor para LSP/formatter
  que recorren sin chequear cada nodo); la lista paralela lleva
  los mensajes ricos.
- **Sync points stmt-level + keywords**: el comentario en
  `synchronize()` documenta el porqué. La iteración con solo
  `Newline`/`RBrace`/`EOF` se quedó corta — los tests
  inmediatamente detectaron que `primary()` consume tokens al
  fallar.
- **API strict intacta**: `parse()` mantiene firma y
  comportamiento. La CLI sigue priorizando fail-fast.
- **Cota 100 errores**: cubre el caso 90% del LSP con margen sin
  permitir cascadas runaway.
- **Recovery solo stmt-level**: errores DENTRO de un stmt
  descartan el stmt entero. Recovery sub-stmt (preservar bindings
  parciales, args parciales) queda como sub-paso futuro
  post-LSP MVP.
- **Cascadas "variable no definida" del checker**: aceptables
  como trade-off. Cuando un Stmt::Error reemplaza un `let x =
  ...` roto, referencias posteriores a `x` pueden generar "no
  definida"; el error real del parser apunta al lugar del
  problema. IDEs muestran ambos diagnostics. Refinar requiere
  preservar bindings parciales.

#### Total al cierre

**1219 unit + 79 E2E + 3 openapi sin feature** (1310 + 88 + 3 con
`--features python`). Clippy `-D warnings` limpio en ambos modos.

#### Deuda residual derivada de F15

- **Recovery sub-stmt** — errores adentro de un stmt (paréntesis
  sin cerrar, expresión incompleta) descartan el stmt entero.
  Para LSP/completion fino (mostrar fields de `User` tras `user.`
  cuando el cursor está parado ahí), eventualmente conviene
  recovery sub-expression. Sub-paso futuro si aparece presión.
- **Bindings parciales** — `let x = <roto>` no preserva el
  binding `x`. Eso genera "x no definido" en referencias
  posteriores. Refinable con un nodo "stmt incompleto" que sí
  parsea el nombre y la anotación. Post-LSP MVP.
- **`Expr::Error` con metadata** — el nodo es opaco hoy. Si
  hubiera más info (qué tipo de token rompió, qué se esperaba),
  el LSP podría sugerir fixes ("¿quisiste decir...?"). Deuda
  baja prioridad.

#### Próximo norte

**Fase 9.0 — F16 (IR tipado persistido por nodo)** — segundo
pre-req habilitante del LSP. **CERRADO 2026-05-15** — ver sección
inmediatamente abajo.

---

### Fase 9.0 — F16: IR tipado persistido por nodo ✓ (CERRADA, 2026-05-15)

Segundo y último sub-paso de Fase 9.0. Cierre formal de la deuda
F16 identificada post-5b. Habilita que el LSP (sub-fases 9.x.2
hover y 9.x.4 completion) responda "¿qué tipo tiene esta
expresión?" sin re-correr el checker por cada request.

**Sin cambio user-facing**: el side-table se devuelve junto al
`TypeEnv` y los errores, pero `fitz run` / `fitz build` /
`fitz check` lo descartan con `_types`. Consumidores del LSP en
9.x lo usan directamente.

#### 9.0.4 — Side-table TypeInfo + populación + tests

- **AST**: sin cambios. Los spans ya estaban en `Expr` (S1.2).
- **`SpanKey(usize, usize)`** como clave hashable. `Span`
  propio no sirve porque su `PartialEq` devuelve `true` siempre
  (intencional para que los tests de AST comparen nodos
  estructuralmente sin re-derivar posiciones del parser; ver
  comentario en `src/ast.rs`).
- **`TypeInfo`** con `record(span, ty)` (omite `Span::ZERO` —
  nodos sintéticos y de tests colisionarían bajo la misma clave),
  `type_at(span) -> Option<&Type>`, `len()`, `is_empty()`.
- **`infer_expr` pasa a ser wrapper sobre `synthesize_expr`**: el
  match con la lógica de síntesis queda intacto en
  `synthesize_expr`; el wrapper hace el `record` al salir.
  Cobertura amplia desde un solo punto — recursión incluida (el
  wrapper se invoca por cada subnodo).
- **`check_program` cambia firma** de `(TypeEnv, Vec<FitzError>)`
  a `(TypeEnv, TypeInfo, Vec<FitzError>)`. Los 13 call sites
  migrados:
  - `main.rs` (4 sitios): `let (env, _types, errors) = ...`.
  - `codegen.rs` (6 sitios): mismo patrón en `generate_project`
    (módulo) y en tests.
  - `types.rs` (3 sitios internos en tests).
- **`Expr::Error` (F15)** se persiste como `Type::Any` uniforme
  con la semántica del checker. Decisión: el LSP decide qué
  mostrar (probable "expresión inválida"); no omitirlos del
  side-table mantiene cobertura visible.
- **Tests del side-table** (8 unit nuevos en
  `types::tests::types_info_*`):
  - `types_info_persiste_tipos_de_literales` — Int/Float/Str/Bool/Null.
  - `types_info_persiste_ident_y_binop` — `let y = x + 5` debe
    persistir el ident `x`, el literal `5` y el BinOp todos
    como Int.
  - `types_info_persiste_call_y_field` — call de fn nominal
    tipa como su return, struct lit tipa como Nominal.
  - `types_info_persiste_match_arms` — match sobre Result<Int>
    persiste Int en las ramas.
  - `types_info_omite_span_zero` — programa construido a mano
    con Span::ZERO en todos los nodos no debe agregar nada al
    side-table.
  - `types_info_expr_error_se_persiste_como_any` — Error node
    con span known tipa como Any en el side-table.
  - `types_info_type_at_devuelve_none_para_span_desconocido` —
    lookup por span ausente / Span::ZERO devuelve None.
  - `types_info_smoke_programa_real` — programa con type
    custom + fn + struct lit + call + print persiste ≥10
    entries.

#### 9.0.5 — Cierre formal

- **Docs**: CHANGELOG v0.9.1, este roadmap (Fase 9.0 — F16
  documentada paso a paso, Fase 9.0 marcada CERRADA entera),
  `docs/deudas-post-5b.md` con F16 marcado CERRADO + nota
  paralela a F15, README refresh.

#### Decisiones técnicas tomadas

- **`HashMap<SpanKey, Type>` (D1)**: simple, reusa los spans del
  AST post-S1.2, sin refactor invasivo. Alternativas
  consideradas y descartadas: NodeId asignado al nodo (refactor
  de cientos de sitios), `*const Expr` (lifetime gymnastics),
  side-vector indexado paralelo al AST (no ergonómico para
  lookup por posición).
- **Cobertura amplia (D2)**: todo `Expr` que pasa por
  `infer_expr` queda registrado. Alternativa "solo nodos clave
  (Ident/Field/Call/StructLit)" descartada — el costo extra es
  trivial (un insert por nodo) y centralizar en el wrapper
  evita "olvidé tal caso" futuro.
- **API: una sola firma de `check_program` (D3)**: los 13 call
  sites se actualizan con `_types` trivialmente. Alternativa
  "agregar `check_program_with_types` separada" descartada —
  dos APIs en paralelo es churn sin valor.
- **`Expr::Error` como `Type::Any` en el side-table (D4)**:
  uniforme con la semántica del checker; el LSP decide qué
  mostrar.
- **Solo `Expr` (no `Stmt`, `TypeExpr`, `Pattern`) (D6)**: el
  LSP obtiene info de variables y fns por scope lookup;
  persistir `Stmt` es ortogonal. Spans en `TypeExpr` y
  `Pattern` siguen como deuda menor S1 — refinable post-LSP
  MVP si aparece presión.

#### Total al cierre

**1227 unit + 79 E2E + 3 openapi sin feature** (1318 + 88 + 3
con `--features python`). Clippy `-D warnings` limpio.

#### Deuda residual derivada de F16

- **Sin index espacial (rango inicio-fin)** — el side-table
  guarda solo el span del primer token de cada `Expr`. Para
  hover, el LSP elige el nodo cuyo span está más cerca del
  cursor; un refinamiento con rangos completos requiere
  `end_span` en `Expr` y queda como sub-paso post-LSP MVP.
- **Spans en `TypeExpr` y `Pattern`** — heredado de S1.
  Necesario si el LSP quiere hover sobre anotaciones
  (`let x: User = ...` → hover sobre `User`) o sobre patrones
  (`Ok(x)` → hover sobre `x`). Refinable cuando aterrice el
  primer caso de uso real del LSP.
- **Cobertura de `Stmt`** — el side-table cubre `Expr`. Si el
  LSP quiere hover sobre el nombre de una variable en su sitio
  de declaración (`let X = ...`), necesita o caminar al RHS o
  registrar el Stmt. Caso 9.x.3 (go-to-definition) lo resuelve
  con tabla de resolución de scopes — vía paralela al
  side-table, no competidora.

#### Próximo norte

**Sub-fases visibles del LSP** (9.x.1 → 9.x.5). Ver sección
"9.x — LSP + extensión VSCode" más abajo en este roadmap.

---

### 9.x — LSP + extensión VSCode (candidata)

**Objetivo**: que escribir Fitz en VSCode (y Neovim, Helix, Zed,
etc., gratis por LSP) se sienta tan vivo como TypeScript: errores
subrayados al tipear, hover con tipos, autocomplete contextual,
go-to-definition.

**Dos piezas separadas**:

1. **`fitz-lsp`** (Rust, en este repo) — nuevo bin que reusa
   `lexer`/`parser`/`types` y habla LSP por stdio.
   Crate sugerido: [`tower-lsp`](https://github.com/ebkalderon/tower-lsp).
2. **Extensión VSCode** (TypeScript, carpeta `editors/vscode/`) —
   `.vsix` con TextMate grammar para syntax highlighting +
   cliente LSP que spawnea `fitz-lsp` como proceso hijo.
   La extensión es delgada: toda la inteligencia vive en
   `fitz-lsp`.

**Prerrequisitos habilitantes** (deuda post-5b, ver
`deudas-post-5b.md`):

- ✅ **F15** — error recovery del parser. **CERRADO 2026-05-15**
  en Fase 9.0 (ver arriba). API pública `parse_with_recovery`
  + nodos `Expr::Error`/`Stmt::Error` + checker silencioso.
- ✅ **F16** — IR tipado persistido por nodo. **CERRADO 2026-05-15**
  en Fase 9.0 (ver arriba). `pub struct TypeInfo` con
  `HashMap<SpanKey, Type>` retornado por `check_program`; el
  LSP lo consume para hover (9.x.2) y completion contextual
  (9.x.4).
- **S1.Pattern/TypeExpr** — completar spans en los nodos que
  todavía no los tienen (deuda residual de S1.2). Refinable
  cuando aterrice el primer caso de uso real del LSP.

**Sub-pasos sugeridos** (granito incremental, cada uno con
valor entregable):

- **9.x.1 — Diagnostics MVP** ✅ **CERRADA 2026-05-15**: server con
  `did_open`/`did_change` que corre `check_program` y publica
  `Diagnostic`s. Extensión VSCode con grammar TextMate básica +
  cliente LSP apuntando al binario en `target/release/fitz-lsp`.
  Resultado: highlighting + errores en vivo. Detalle paso-a-paso
  en sección "Fase 9.x.1" abajo.
- **9.x.2 — Hover** ✅ **CERRADA 2026-05-16**: `textDocument/hover`
  devuelve el tipo del nodo bajo el cursor (consume el `TypeInfo`
  de F16). Detalle paso-a-paso en sección "Fase 9.x.2" abajo.
- **9.x.3 — Go-to-definition** ✅ **CERRADA 2026-05-16**:
  `textDocument/definition` resuelve `Ident` → span de declaración
  (let, fn, type, param, for, match binding). Detalle paso-a-paso
  en sección "Fase 9.x.3" abajo.
- **9.x.4 — Autocomplete** ✅ **CERRADA 2026-05-16**:
  `textDocument/completion` con dos contextos del MVP — scope-
  level (top-level + builtins + tipos + keywords) y after-dot
  (fields de Nominal, métodos de List/Map/Str). Imports
  (`from mod import `) queda como deuda visible — requiere
  cargar el módulo remoto. Detalle paso-a-paso en sección "Fase
  9.x.4" abajo.
- **9.x.5 — Distribución** ✅ **CERRADA 2026-05-16**:
  extensión VSCode multi-platform aware con binario `fitz-lsp`
  bundleado en el `.vsix` per-plataforma (6 targets: win32-x64/
  arm64, linux-x64/arm64, darwin-x64/arm64) + logo oficial
  (engranaje Rust + Fitz Roy) + script reproducible de build
  local. Publicación real al Marketplace queda como acción del
  autor (cuenta de publisher + repo público). Detalle paso-a-paso
  en sección "Fase 9.x.5" abajo.

**Trade-offs y decisiones pendientes**:

- ¿Distribuir binarios bundleados o descargarlos al instalar?
  (Tamaño del `.vsix` vs complejidad del activador.)
- ¿Grammar TextMate o tree-sitter? Tree-sitter es más preciso
  e incremental pero requiere otra dependencia y otra spec.
- ¿Reusar `fitz check` directo o forkear el pipeline? Reusar
  es lo natural; forkear sería solo si la performance del
  check sobre buffers grandes duele.

**Por qué encaja en Fase 9**: es tooling de ecosistema, no del
lenguaje core. Una vez que el lenguaje está estable (Fase 5
cerrada) y el ecosistema empieza a expandirse, el LSP es lo
que hace que escribir Fitz pase de "compilar y revisar" a
"sentir el lenguaje mientras lo escribís".

---

## Fase 9.x.1 — LSP MVP: diagnostics + extensión VSCode (cerrada, 2026-05-15)

Primera sub-fase visible del LSP. Habilita la experiencia "escribir
Fitz en VSCode con errores subrayados al tipear" — equivalente al
nivel de servicio que ofrece TypeScript en sus primeros segundos.
Tres sub-pasos coordinados (un commit por sub-paso):

- **9.x.1.a — Server skeleton**:
  - Dep `tower-lsp = "0.20"` opcional; feature `lsp = ["dep:tower-lsp"]`
    paralela a `python = ["dep:pyo3"]`. Bin `[[bin]] name = "fitz-lsp"`
    con `required-features = ["lsp"]` — el `cargo build` default sigue
    standalone.
  - `src/bin/fitz-lsp.rs`: skeleton con `Backend` impl `LanguageServer`.
    `initialize` → response con `serverInfo` + `textDocumentSync: FULL`.
    `initialized` (log via `client.log_message`). `shutdown`.
    `#[tokio::main(flavor = "current_thread")]` (LSP es I/O-bound).
  - 1 test E2E `tests/lsp_e2e.rs` que spawnea el bin y valida el
    handshake. Frames JSON-RPC construidos a mano via Content-Length.

- **9.x.1.b — Lib refactor + helper diagnostics + lifecycle hooks**:
  - **Lib refactor**: `src/lib.rs` nuevo expone los módulos como
    `pub mod`. `src/main.rs` migra de `mod X;` a `use fitz::{...};`.
    Habilita que `fitz-lsp` reuse `lexer`/`parser`/`types` sin
    compilación duplicada.
  - **`src/lsp.rs` (nuevo, lib, feature-gated)**: dos APIs públicas
    pure-function:
    - `check_source(&str) -> Vec<FitzError>` — pipeline LSP-style:
      tokenize → `parse_with_recovery` → `check_program` (descarta
      el `TypeInfo` que llega 9.x.2 hover).
    - `fitz_errors_to_diagnostics(&[FitzError]) -> Vec<Diagnostic>` —
      mapea 1-based Fitz → 0-based LSP. Range 1-char (refinable post-S1).
      `hint` concatenado al `message`. Severity ERROR, source "fitz".
      Sentinel `(0, 0)` → range degenerado.
  - **Backend con DocumentStore**: `documents: Arc<parking_lot::Mutex
    <HashMap<Url, String>>>`. `did_open`/`did_change`/`did_close`
    disparan el pipeline fuera del lock.
  - 9 unit tests + 1 E2E nuevo (`did_open` con buffer roto valida
    `textDocument/publishDiagnostics`).
  - **Deuda nueva**: `#[allow(clippy::result_unit_err)]` puntual sobre
    `Environment::assign`. Lint apareció en clippy 1.95 + expuesto
    por el refactor lib. El `Result<(), ()>` es sentinel intencional.

- **9.x.1.c — Extensión VSCode** (`editors/vscode/`, paquete TypeScript):
  - `package.json` con language `fitz`, extension `.fitz`, activation
    `onLanguage:fitz`, settings `fitz.lspPath` + `fitz.trace.server`.
  - Grammar TextMate: comments, strings con interpolación recursiva,
    números, decoradores, keywords, tipos built-in + nominales,
    constantes, built-ins, operadores, defs/calls fns.
  - `language-configuration.json`: comments, brackets, autoClose,
    indent rules.
  - `src/extension.ts`: capa fina sobre `vscode-languageclient/node`.
    `resolveServerPath` distingue absoluto / relativo-a-workspace /
    PATH.
  - Validaciones: JSON OK, `npm install` (12 packages, 0 vulns),
    `tsc strict`, `vsce package` produce `.vsix` 294 KB.

**Decisiones técnicas tomadas al arrancar**:

- bin `fitz-lsp` separado del CLI principal (vs subcomando) — convención
  ecosistema rust-analyzer/gopls/tsserver.
- `tower-lsp` sobre `lsp-server` crudo — async-first, framing JSON-RPC
  automático.
- Grammar TextMate sobre tree-sitter — ~120 LoC JSON, suficiente para
  MVP.
- Setting `fitz.lspPath` para descubrimiento del binario (vs bundling)
  — alfa simple, bundling rust-analyzer-style llega en 9.x.5.
- `textDocumentSync: FULL` (vs `INCREMENTAL`) — default razonable.
- `tokio current_thread` para LSP — I/O-bound, sin work-stealing.

**Total al cierre**: 1227 unit + 79 E2E + 3 openapi sin cambios.
9 unit + 2 E2E nuevos con `--features lsp`. Clippy `-D warnings` limpio.

**Próximo norte**: **9.x.2 (hover)** — consume `TypeInfo` (F16).

**Deuda residual derivada (NO bloquea 9.x.2)**:

- Range de 1 carácter en Diagnostics — refinable cuando S1.Pattern/
  TypeExpr sume `end_span` a los nodos del AST.
- Solo `INCREMENTAL` ausente — sync FULL por ahora.
- Smoke visual del `.vsix` instalado en VSCode real es manual del
  autor (sin test automatizado).

---

## Fase 9.x.2 — LSP hover: tipo del nodo bajo el cursor (cerrada, 2026-05-16)

Segunda sub-fase visible del LSP. Habilita la experiencia "pasá el
mouse y ve qué tipo tiene esta expresión" — desbloquea el patrón
de exploración interactiva del código que TypeScript ofrece desde
hace años. Dos sub-pasos coordinados (un commit por sub-paso):

- **9.x.2.a — Persistencia de TypeInfo por documento**:
  - Nueva API `fitz::lsp::check_source_with_types(src) -> (TypeEnv,
    TypeInfo, Vec<FitzError>)` que retiene el side-table poblado
    por F16. La fn vieja `check_source` se mantiene como wrapper
    para consumidores que solo necesitan diagnostics.
  - `DocumentState { text, type_env, type_info }` reemplaza el
    `String` plano en `documents`. `did_open`/`did_change` persisten
    los tres; `did_close` limpia.
  - 4 unit tests nuevos.

- **9.x.2.b — Hover handler + lookup heurístico + capability**:
  - `hover_for_position(&TypeInfo, line, character) -> Option<&Type>`
    en `fitz::lsp` (pure function). Heurística "max col <= cursor
    en la misma línea". Convierte 0-based LSP a 1-based Fitz.
  - `make_hover(&Type, &TypeEnv) -> Hover` arma respuesta con
    `MarkupContent::Markdown` y bloque ```fitz<tipo>```. `range:
    None` (sin end_span).
  - Capability `hover_provider: Some(Simple(true))`.
  - `Backend::hover` lee state bajo lock, delega al helper.
  - `pub fn iter()` sobre `TypeInfo` (mínimo y backward-compatible).
  - 8 unit tests + 1 E2E nuevo (hover sobre `42` devuelve `Int`,
    posición sin spans devuelve `null`).

**Decisiones técnicas tomadas al arrancar**:

- Persistencia de TypeInfo por URI (vs re-correr pipeline en cada
  hover) — el TypeInfo pesa nada.
- Heurística "max col <= cursor en la misma línea" (vs lookup
  exacto) — sin `end_span` en los nodos (S1.deuda), el 90% del
  caso funciona.
- Colisiones en TypeInfo aceptadas como están (heredado de F16) —
  cuando un `BinOp` comparte span con su primer operando, gana el
  último escrito; en práctica el tipo "grande" es lo que el
  usuario quiere ver.
- Persistir TypeEnv junto con TypeInfo — `Type::display` lo
  necesita para resolver nominales.
- `MarkupContent::Markdown` con bloque ```fitz``` (vs PlainText) —
  syntax highlighting nativo en VSCode, sin costo extra.
- `range: None` en Hover — sin `end_span` no podemos devolver el
  rango; el tooltip funciona igual.

**Total al cierre**: 1227 unit + 79 E2E + 3 openapi sin cambios.
12 unit + 1 E2E nuevos con `--features lsp`. Clippy `-D warnings`
limpio.

**Próximo norte**: **9.x.3 (go-to-definition)** —
`textDocument/definition` resuelve `Ident` → span de declaración.
Requiere mantener tabla de resolución de scopes.

**Deuda residual derivada (NO bloquea 9.x.3)**:

- Range exacto en la respuesta Hover — depende de `end_span` en
  los nodos del AST (deuda S1.Pattern/TypeExpr).
- Lookup heurístico imperfecto en tokens largos — cuando el cursor
  está muy al final de un identificador de >10 chars, puede
  agarrar el siguiente Expr en la línea. Refinable con `end_span`.
- Cursor multi-línea (string interpolation, expression sobre
  varias líneas) — la heurística "misma línea" no cruza. Para los
  casos comunes de Fitz hoy alcanza; refinable si pinta corto.

---

## Fase 9.x.3 — LSP go-to-definition (cerrada, 2026-05-16)

Tercera sub-fase visible del LSP. Habilita "F12 sobre un nombre te
lleva a su definición" — core del workflow de exploración. Dos
sub-pasos coordinados:

- **9.x.3.a — Side-table `DefinitionInfo` + populación en el checker**:
  - `VarBinding` suma `def_span: Span`. Builtins usan `Span::ZERO`
    (filtrados al responder).
  - `declare_var`/`declare_var_annotated` reciben `def_span`; 12
    call sites actualizados con el span apropiado. Aproximaciones
    documentadas donde el AST no tiene span propio del binding
    (deuda S1).
  - `pub struct DefinitionInfo` paralelo a `TypeInfo` (F16).
    Política: omite `Span::ZERO` en use y def.
  - Wrapper `infer_expr` para `Expr::Ident` registra `(use_span,
    def_span)` cuando `lookup_binding` encuentra binding con span
    conocido.
  - `check_program` retorna 4-tupla; 18 call sites internos
    actualizados.
  - 6 unit tests nuevos en `types::tests::def_info_*`.
  - Limpieza colateral: `lookup_var` eliminado (duplicaba
    `lookup_binding`).

- **9.x.3.b — Handler `definition` + helpers + capability**:
  - `definition_for_position(&DefinitionInfo, line, character) ->
    Option<Span>` en `fitz::lsp`. Misma heurística que hover.
  - `make_definition_location(Url, Span) -> Location` convierte
    1-based Fitz a 0-based LSP; range 1-char.
  - Capability `definition_provider: Some(OneOf::Left(true))`.
  - `Backend::goto_definition` devuelve
    `GotoDefinitionResponse::Scalar(loc)` (un solo Location).
  - 5 unit tests + 1 E2E nuevo (`definition` sobre uso de var local
    devuelve Location con line:0; sobre builtin devuelve `null`).

**Decisiones técnicas tomadas al arrancar**:

- Side-table dedicado `DefinitionInfo` (vs reuso de TypeInfo) —
  mismo patrón que F16.
- `VarBinding.def_span` con `Span::ZERO` para builtins (filtrados).
- Aproximaciones de granularidad: AST sin span propio en `Param`/
  `For.var`/`AssignTarget::Ident`/`MatchArm.pattern` (deuda S1)
  — usamos el span del stmt contenedor.
- Reasignaciones sobrescriben `def_span` con el último let stmt
  (semántica simplificada del MVP; TypeScript apunta a la primera
  declaración).
- Lookup heurístico = mismo que hover (max col <= cursor en la
  misma línea).
- `range` 1-char (sin end_span); `uri` = documento abierto (sin
  cross-module).
- `OneOf::Left(true)` para la capability — Fitz no tiene
  overloading, un solo Location alcanza.

**Total al cierre**: 1233 unit + 79 E2E + 3 openapi sin cambios.
+6 unit en `types::tests` + 5 unit + 1 E2E nuevos con `--features lsp`
(acumulado 26 unit + 4 E2E en LSP). Clippy `-D warnings` limpio.

**Próximo norte**: **9.x.4 (autocomplete contextual)** — cuatro
contextos (símbolos en scope, fields tras `obj.`, métodos built-in
tras `xs.`/`m.`/`s.`, símbolos importados).

**Deuda residual derivada (NO bloquea 9.x.4)**:

- Cross-module go-to-def: `from foo import X` apunta al Stmt::Import
  local, no al módulo remoto. Requiere mapear paths del loader a
  URIs.
- `def_span` granular por nombre — depende de `Span` propio en
  nodos `Param`/`AssignTarget::Ident`/`For.var`/`MatchArm.pattern`
  (deuda S1).
- Reasignaciones apuntan al último let stmt (vs primera declaración
  como TypeScript). Refinable con tracking adicional.

---

## Fase 9.x.4 — LSP autocomplete contextual (cerrada, 2026-05-16)

Cuarta sub-fase visible del LSP. **Completa el MVP del language
server** — diagnostics + hover + go-to-def + autocomplete cubren la
experiencia core de editing. Dos sub-pasos coordinados:

- **9.x.4.a — Persistir Program + helper `completion_at_position`**:
  - `check_source_with_types` retorna 5-tupla incluyendo `Program`:
    `(Program, TypeEnv, TypeInfo, DefinitionInfo, Vec<FitzError>)`.
    Call sites del LSP actualizados.
  - `fitz::lsp::completion_at_position(text, program, type_info,
    type_env, line, character) -> Vec<CompletionItem>` (pure
    function). Despacha por contexto:
    - **Scope-level**: top-level (let/fn/type/import) + builtins +
      tipos built-in + keywords. NO scope-aware (deuda MVP).
    - **After-dot**: identifica el receiver, resuelve el tipo con
      dos fallbacks (TypeInfo heurístico + walk del Program por
      nombre). Tipos cubiertos: Nominal (fields), List (6 métodos),
      Map (5 métodos), Str (3 métodos).
  - Helpers internos: `CompletionContext` enum,
    `detect_completion_context`, `position_to_offset`/
    `offset_to_position` (UTF-8 char-based), `is_ident_continue`,
    `method_items`, `after_dot_completions`, `scope_level_completions`.
  - `DocumentState` suma `program: Program` con `#[allow(dead_code)]`
    puntual hasta 9.x.4.b.
  - 10 unit tests nuevos.

- **9.x.4.b — Handler `completion` + capability + E2E**:
  - Capability `completion_provider: Some(CompletionOptions {
    trigger_characters: Some(vec![".".into()]), resolve_provider:
    Some(false), ... })`.
  - `Backend::completion` lee state bajo lock, delega al helper,
    devuelve `CompletionResponse::Array(items)`.
  - `#[allow(dead_code)]` removido de `text` y `program`.
  - 1 E2E nuevo (`completion_after_dot_sobre_str_lista_metodos_built_in`):
    valida capability con triggerCharacters, after-dot sobre `s.`
    con `s: Str` lista `upper`/`lower` sin `push`, scope-level
    lista `s` + `print` + `Int` + `let`.

**Decisiones técnicas tomadas al arrancar**:

- Alcance MVP: scope-level + after-dot. Imports (`from mod import `)
  queda deuda visible — requiere loader cross-module.
- Scope-level no scope-aware: top-level + builtins + tipos +
  keywords. Vars locales/params requieren refactor del checker.
- After-dot solo `<ident>.`: chain `a.b.c.` deuda — requiere parser
  parcial.
- After-dot con dos fallbacks: TypeInfo heurístico + walk del
  Program por nombre. El walk cubre el caso `obj.<cursor>` al final
  del buffer (deuda F15 recovery sub-stmt).
- `Program` persistido en `DocumentState`: el AST es necesario en
  cada completion request. Re-walkar es barato vs re-parsear.
- `CompletionItem` con kind (Variable/Function/Field/Method/Keyword/
  Class/Module) + detail opcional con firma.
- UTF-8 char-based para position↔offset (LSP default UTF-16 queda
  como refinamiento si aparece presión con código no-ASCII).

**Total al cierre**: 1233 unit + 79 E2E + 3 openapi sin cambios.
10 unit + 1 E2E nuevos con `--features lsp` (acumulado 36 unit + 5
E2E en LSP). Clippy `-D warnings` limpio.

**Cierre del LSP MVP**: con 9.x.4 cerrada, las cuatro features
core del language server están vivas. Lo que falta (9.x.5) es
distribución.

**Próximo norte**: **9.x.5 (distribución VSCode Marketplace)** —
binarios pre-compilados por plataforma (Windows x64, macOS x64+ARM,
Linux x64+ARM) bundleados en el `.vsix`, al estilo rust-analyzer.

**Deuda residual derivada (NO bloquea 9.x.5)**:

- Completion para imports (`from mod import `): requiere loader
  cross-module.
- Scope-aware en scope-level: vars locales + params según cursor.
- Chain `a.b.c.` en after-dot: parser parcial.
- Position UTF-16 strict (LSP default).
- Completion context-sensitive del parser: tras `@` sugerir
  decoradores, tras `import ` sugerir paths.

---

## Fase 9.x.5 — LSP distribución multi-platform + logo (cerrada, 2026-05-16)

Quinta y última sub-fase visible del LSP. **Completa el plan LSP
entero (9.x.1 → 9.x.5)**. Deja la extensión lista para publicar
al Marketplace; la publicación real queda como acción del autor.

- **9.x.5.0 — Logo de Fitz**:
  - Engranaje Rust naranja `#CE412B` (12 dientes) + silueta Fitz
    Roy adentro (3 picos, central más alto, vía `clipPath`).
  - `assets/logo.svg` source + `assets/logo.png` (256×256) +
    `assets/logo-social.svg/.png` (1280×640) + `editors/vscode/
    icon.png` (copia para .vsix).
  - `scripts/build-icon.mjs` regenera los PNGs vía `@resvg/resvg-js`.
  - README raíz con hero image al inicio.

- **9.x.5.a — Extensión multi-platform aware + script build-vsix**:
  - `resolveServerPath` con prioridad: (a) override del user, (b)
    bundled en `server/`, (c) fallback PATH. Backward-compatible.
  - `scripts/build-vsix.mjs` orquesta cargo build → copia binario
    → tsc → vsce package con sufijo `-<platform>-<arch>`. 6
    plataformas soportadas.
  - Estructura `editors/vscode/server/` con `.gitignore` que
    excluye binarios (se regeneran cada build).
  - `activationEvents` removido (auto-derived por VSCode ≥1.74).

**Decisiones técnicas tomadas al arrancar**:

- Logo: engranaje Rust + Fitz Roy (inspiración del nombre del
  lenguaje + lenguaje de implementación).
- SVG single source en `assets/` (raíz, no enterrado en la
  extensión).
- `@resvg/resvg-js` para SVG→PNG (puro JS bindings, sin
  compilación nativa pesada).
- Per-plataforma `.vsix` (estándar rust-analyzer/Marketplace) vs
  mega-.vsix con todos los binarios (rechazado por tamaño).
- Resolución del binario override > bundled > PATH (backward-
  compatible con 9.x.1.c).
- CI multi-platform y publicación al Marketplace fuera de scope —
  acciones del autor.

**Cierre del plan LSP (9.x.1 → 9.x.5)** — MVP completo:
diagnostics + hover + go-to-def + autocomplete + distribución.

**Acciones manuales pendientes del autor** (no commit técnico):

1. GitHub Social Preview (Settings → upload `assets/logo-social.png`).
2. Hacer el repo público (decisión del autor).
3. Crear publisher en Marketplace + PAT.
4. `vsce publish --packagePath` por cada plataforma.
5. CI multi-platform (GitHub Actions, opcional).

**Total al cierre**: 1233 unit + 79 E2E + 3 openapi sin cambios.
36 unit + 5 E2E LSP sin cambios. Validación local Windows:
✅ `.vsix` 1.49 MB con `server/fitz-lsp.exe` bundleado.

**Próximo norte (técnico)**: resto de Fase 9 — **package manager
+ registry**, **formatter**, **linter**. Plan a definir.

**Deuda residual derivada (NO bloquea próximas fases)**:

- CI multi-platform.
- Publicación automática al Marketplace.
- Cross-compile local (hoy cada plataforma genera su propio .vsix).
- Logo: variantes adicionales (favicon, app icon, monochrome).

---

## Fase 9.y — Package manager + registry 📦

**Estado: PENDIENTE (siguiente bloque post-LSP)** — primer trabajo
del resto de Fase 9.

**Objetivo**: convertir Fitz de "lenguaje con CLI que ejecuta
archivos sueltos" en "ecosistema con proyectos, dependencias, y
compartición". Cubre el ciclo completo: scaffolding inicial → uso
de paquetes ajenos → publicar el propio. Es la pieza más visible
para el usuario nuevo: la primera impresión de "¿qué tan serio es
Fitz?" pasa por `fitz new mi-app`.

**Por qué encaja en Fase 9**: el LSP fue la primera capa de tooling
alrededor del lenguaje core; el package manager es la segunda.
Ambos asumen que el lenguaje en sí ya está estable (Fase 5
cerrada). Ningún cambio breaking del lenguaje se espera para
soportar esto — todo es tooling encima.

**Inspiración**:

- **Cargo (Rust)** — convención de proyecto, manifest TOML,
  lockfile, workspaces.
- **npm (Node)** — modelo de registry centralizado + scoped
  packages.
- **uv (Python por Astral)** — velocidad de resolución, lockfile
  multi-platform.
- **deno** — registry federado por URLs (alternativa interesante
  a rechazar o adoptar como complemento al centralizado).

### Decisiones pendientes globales del bloque

Antes de arrancar 9.y.1, hay que cerrar estas decisiones — todas
tienen impacto en el formato de archivo y el modelo mental del
usuario:

1. **¿TOML, YAML, o JSON para el manifest?** Lean: **TOML**
   (convención Rust, legible, comentarios, encaja con la
   implementación en Rust del propio Fitz). YAML descartado por
   indentation-sensitive. JSON descartado por falta de comentarios.
2. **¿Estructura del project: `src/` o raíz?** Lean: **`src/`**
   (convención Rust/Java/TS, separa source de manifest/docs).
3. **¿Versionado: semver estricto o pre-releases permitidos?**
   Lean: **estricto + canal `prerelease` opt-in** (estilo Rust).
4. **¿Registry centralizado (npm/crates.io) o federado (deno.land)?**
   Lean: **centralizado**, con soporte para git deps + path deps
   como en Cargo. Centralizado da mejor DX (`fitz add foo` corto);
   federado escala sin servidor central pero `import "https://..."`
   es feo.
5. **¿Resolución: SAT solver (PubGrub) o greedy (MVS estilo Go)?**
   Decisión abierta. **PubGrub** (uv, dart pub) es correcto y da
   mensajes de error excelentes pero es complejo. **MVS** ("usá
   la versión mínima compatible") es simple. Lean: empezar greedy,
   migrar a PubGrub si aparece dolor.
6. **¿Scope: `paquete` flat o `@usuario/paquete` namespaceado?**
   Lean: **namespaceado siempre**. Evita squatting + ambiente más
   sano.
7. **¿Múltiples versiones del mismo paquete coexistiendo (Cargo) o
   single-version (npm flat / pip)?** Lean: **Cargo-style**, más
   espacio pero menos conflictos.
8. **¿Cache local: `~/.fitz/cache/` global o `target/` por
   proyecto?** Lean: **global con dedupe** (Rust cargo + uv).
9. **¿Workspaces (multi-paquete bajo manifest raíz)?** Sí, pero
   **no en el MVP** — sub-paso 9.y.8+ futuro.

### Sub-pasos

#### 9.y.1 — Manifest + scaffolding ✓ (CERRADA, 2026-05-16)

Primer sub-paso del bloque. Sin red, sin deps remotas, sin nada
que se rompa.

- **Nuevo formato `fitz.toml`** en la raíz de cada proyecto.
  Mínimo viable:
  ```toml
  [package]
  name = "mi-app"
  version = "0.1.0"
  edition = "2026"

  [bin]
  main = "src/main.fitz"
  ```
- **Sub-comando `fitz new <nombre> [--http] [--no-git]`**: crea
  carpeta con `fitz.toml` + `src/main.fitz` + `.gitignore` + (si
  no `--no-git`) `git init`. Templates: default CLI (top-level
  `print(...)` estilo `02-hola.fitz`) y `--http` (patrón canónico
  `@get("/")` + `@server(3000) fn main() => 0`). El `--lib` queda
  como deuda hasta que library publishing sea real (9.y.5+).
- **Sub-comando `fitz init [--name X] [--http] [--no-git]`**:
  inicializa proyecto en el directorio actual sin crear carpeta
  nueva. Nombre derivado del cwd (o del flag `--name`). Falla si
  ya existe `fitz.toml` o si el nombre derivado no matchea la
  regex.

#### Decisiones técnicas tomadas

- **Formato**: TOML (`fitz.toml`). Lean del bloque cerrada.
- **Estructura**: `src/main.fitz`. Lean del bloque cerrada.
- **Field versionado**: `edition = "2026"` (Cargo-style year).
- **Bin único** (`[bin]`, no `[[bin]]` array). Multi-bin queda
  como sub-paso 9.y.8+ post-MVP.
- **Validación de nombre**: `^[a-z][a-z0-9_-]{0,63}$` (política
  crates.io: lowercase + alfanumérico + `-`/`_`, máx 64). Regex
  implementada a mano para no agregar dep `regex` por algo tan
  chico.
- **git init**: corre por default con `--quiet`. Flag `--no-git`
  para opt-out (CI, monorepos, autores que prefieren manual).
  Si `git init` falla (git no instalado), emite warning pero NO
  aborta — el proyecto sigue siendo válido sin git.
- **`.gitignore`**: excluye `target/` + `*.exe`/`*.pdb` (binarios
  generados por `fitz build`). **NO** excluye `fitz.lock` — el
  lockfile se commitea (Cargo-style, sub-paso 9.y.3).
- **Manifest serialization**: serde con `skip_serializing_if`
  para que el TOML default sea **mínimo** (sin campos vacíos
  `authors = []`, `description = ""`, etc.).
- **`find_manifest`**: walk-up del cwd buscando `fitz.toml`
  (Cargo-style). Ya implementado y testeado, sin consumidor
  todavía — `#[allow(dead_code)]` puntual hasta 9.y.2.

#### Total al cierre

**1246 unit** (+13 de `manifest::tests`) **+ 11 cli_e2e** (nuevos)
**+ 79 compile_e2e + 3 openapi** sin features. Clippy
`-D warnings` limpio en lib + bins + tests. **Sin breaking**: el
modo single-file (`fitz run archivo.fitz`) sigue funcionando
idéntico — 9.y.1 solo agrega comandos nuevos, no cambia
comportamiento existente.

Archivos:

- `Cargo.toml` — dep nueva `toml = "0.8"` (no opcional).
- `src/lib.rs` — `pub mod manifest`.
- `src/manifest.rs` (nuevo, +279 LoC) — `Manifest`/`Package`/`Bin`
  structs serde-able, `parse`/`to_toml_string`/`new_default`,
  `is_valid_package_name`, `find_manifest`, 13 unit tests.
- `src/main.rs` — `Commands::New`/`Init` en clap + `new_project`/
  `init_project`/`scaffold_project` + templates inline CLI/HTTP.
- `tests/cli_e2e.rs` (nuevo, +173 LoC) — 11 E2E tests:
  estructura completa default, template `--http`, git init,
  `--no-git`, error si carpeta ya existe, error nombre inválido,
  init usa nombre del directorio, init con `--name` override,
  init falla si manifest ya existe, init falla con nombre
  inválido sin override, programa generado corre con `fitz run`.

#### Decisiones residuales (NO bloquean 9.y.2)

- **Multi-bin (`[[bin]]`)**: sin `[[bin]]` array todavía. Sub-paso
  9.y.8+ post-MVP.
- **`--lib` template**: sin template `--lib` todavía. Aterriza
  cuando library publishing sea real (9.y.5+).
- **Repo público pre-existente con `git init`**: `git init` sobre
  un dir que YA es git repo no rompe (git lo detecta y no toca
  nada). Sin caso especial necesario.
- **Nombres reservados** (ej. `fitz`, `main`, `test`): sin lista
  de reservados todavía. Aterriza con el registry (9.y.5).
- **Manifest con `[lib]` o `[[bin]]`** en `parse`: el parser los
  rechaza silenciosamente (campos desconocidos para serde por
  default). Si el usuario edita a mano y agrega `[lib]`, el
  parse falla. Refinable con `#[serde(deny_unknown_fields)]` o
  agregando explícitamente las variantes futuras.

#### Próximo norte

**9.y.2 — `fitz run`/`build`/`check` integrados con manifest** —
detectar `fitz.toml` en cwd o ancestros (Cargo-style) y usar
`[bin].main` como entry point. `find_manifest` ya está listo
desde 9.y.1.

#### 9.y.2 — `fitz run`/`build`/`check` integrados con manifest ✓ (CERRADA, 2026-05-16)

Cambio de comportamiento del CLI: detectar `fitz.toml` en el
directorio actual o en padres (Cargo-style).

- **`fitz run`** sin argumentos: lee `fitz.toml`, ejecuta
  `[bin].main`. Con argumento `fitz run archivo.fitz` sigue
  funcionando como hoy (modo "single file").
- **`fitz build`** sin argumentos: lee el manifest y compila
  `[bin].main`, emitiendo el binario a
  `<manifest_dir>/target/release/<pkg-name>(.exe)` con el nombre
  del paquete (NO el stem del fuente). Con argumento explícito
  sigue copiando adyacente al `.fitz` (comportamiento pre-9.y.2).
- **`fitz check`** sin argumentos: chequea el `[bin].main`. El
  loader walks los `import`s transitivamente, así que la cobertura
  del checker llega a todo el proyecto vía el grafo de módulos.
  Auto-discovery de archivos sueltos (`*.fitz` no importados) es
  deuda menor.

#### Decisiones técnicas tomadas

- **`target/release/<pkg-name>(.exe)`** adyacente al manifest,
  hardcodeado en el dispatch. Configurable via `[build] target =
  "..."` post-MVP si aparece presión.
- **`fitz check` solo el entry**: el loader walks imports, así que
  todos los módulos referenciados se chequean transitivamente. Un
  modo que recorra TODOS los `.fitz` del directorio (incluyendo
  archivos sueltos no importados) es deuda menor — refinable si
  aparece presión.
- **Compat single-file silenciosa**: sin warning. Los ejemplos de
  la guía siguen corriendo idénticos con `fitz run examples/
  guide/02-hola.fitz`. La promesa "sin breaking" del bloque 9.y
  se cumple bit-a-bit.
- **Manifest sin `[bin]`**: error claro con la sección sugerida
  inline. Multi-bin (`[[bin]]` array) sigue siendo deuda 9.y.8+.
- **TOML corrupto**: error explícito via `ManifestError::Parse`
  (delegado al mensaje de `toml::de::Error`).
- **`find_manifest` ya estaba en 9.y.1**: ahora tiene consumidor,
  saco el `#[allow(dead_code)]` puntual.
- **Estructura del dispatch**: `ResolvedEntry { entry,
  manifest_ctx: Option<ManifestCtx> }` devuelto por
  `resolve_entry()`. Single-file mode: ctx = None. Manifest mode:
  ctx = Some con la `Manifest` parseada + `manifest_dir`. El
  `build_file` toma `override_dest: Option<&Path>` y decide el
  destino del copy del binario.

#### Total al cierre

**1246 unit + 20 cli_e2e** (+9 nuevos de 9.y.2 sobre los 11 de
9.y.1) **+ 79 compile_e2e + 3 openapi** sin features. Clippy
`-D warnings` limpio. **Sin breaking**: los 79 tests de
`compile_e2e` (single-file mode) siguen verdes idénticos.

Archivos:

- `src/main.rs` — `Commands::Run/Build/Check.file` pasa a
  `Option<PathBuf>`. Nuevo `ResolvedEntry` + `ManifestCtx` +
  `resolve_entry()`. `build_file` toma `override_dest:
  Option<&Path>` con `create_dir_all` del parent destino para que
  `target/release/` se cree on-demand al primer build.
- `tests/cli_e2e.rs` — 9 E2E nuevos: run/check sin args, walk-up
  desde subdir, single-file mode compat, errores (sin manifest +
  sin archivo, sin `[bin]`, TOML corrupto), build sin args produce
  binario con pkg-name en `target/release/`.

#### Decisiones residuales (NO bloquean 9.y.3)

- **`fitz check` recorriendo TODOS los `.fitz` del proyecto** (no
  solo el entry + imports): refinable post-MVP. El loader ya
  cubre transitivamente cualquier módulo importado; archivos
  sueltos no importados quedan sin chequear hasta que aparezca el
  caso de uso real.
- **Configuración del output dir**: `[build] target = "..."`
  hardcodeable post-MVP. Hoy `target/release/` adyacente al
  manifest, Cargo-style.
- **Multi-bin (`[[bin]]` array)**: sigue siendo deuda 9.y.8+.
- **`fitz build` con `--release`/`--debug`**: hoy siempre release
  (idem comportamiento pre-9.y.2). Modo debug llega cuando aparezca
  presión.
- **Discovery de `tests/`**: auto-discovery de `tests/*.fitz`
  llega con 9.z.2 (`fitz test`), no es scope de 9.y.

#### Próximo norte

**9.y.3 — Resolución de deps + lockfile** — agregar
`[dependencies]` con tipos `path = "..."` y `git = "..."`,
algoritmo de resolución, lockfile `fitz.lock` commiteable, cache
local en `~/.fitz/cache/`. Sin registry todavía (9.y.5).

#### 9.y.3 — Resolución de deps + lockfile ✓ (CERRADO ENTERO, 2026-05-16)

Antes del registry, primero la mecánica de resolución sobre deps
**locales y git** (sin servidor). Sub-paso grande — partido en
tres sub-commits para que cada deliverable cierre algo testeable
sin meternos en commits gigantes:

- **9.y.3.a — Path deps + sección `[lib]` + lockfile**
  ✓ CERRADO (2026-05-16). Detalle abajo.
- **9.y.3.b — Loader integration** (deps usables desde código):
  ✓ CERRADO (2026-05-16).
- **9.y.3.c — Git deps + cache local**:
  ✓ CERRADO (2026-05-16).

**Próximo norte tras 9.y.3 entera**: 9.y.4 — `fitz add` /
`fitz remove` / `fitz update` (sub-comandos de manipulación del
manifest + lockfile).

##### 9.y.3.a — Path deps + sección `[lib]` + lockfile ✓ (CERRADA, 2026-05-16)

Primer slice del tercer sub-paso. Habilita declarar
`[dependencies]` con `foo = { path = "../foo" }` en el manifest;
el `fitz.lock` se emite/sincroniza automáticamente en cada
`fitz run`/`build`/`check` (manifest mode). **NO toca el loader
del lenguaje** — las deps quedan declaradas y bloqueadas en el
lockfile pero `from foo import X` todavía no las resuelve. Esa
promesa es 9.y.3.b.

- **Sintaxis manifest**:
  ```toml
  [dependencies]
  utils-lib = { path = "../utils-lib" }
  # Versión suelta (futuro 9.y.5 = registry):
  # algo = "1.0.0"
  # Git deps (futuro 9.y.3.c):
  # helpers = { git = "https://github.com/foo/bar", tag = "v1.0.0" }
  ```
- **Sección `[lib]`** nueva, paralela a `[bin]`. Marca al
  proyecto como librería importable:
  ```toml
  [lib]
  entry = "src/lib.fitz"
  ```
- **`fitz.lock`** TOML, Cargo-style:
  ```toml
  version = 1

  [[package]]
  name = "utils-lib"
  version = "0.2.0"
  ```
  Sin campo `source` para path deps (convención Cargo: las path
  deps son implícitas; `source = "git+..."` y `source = "registry+..."`
  vienen en 9.y.3.c y 9.y.5 respectivamente).

###### Decisiones técnicas tomadas

- **Lockfile en TOML** (uniforme con `fitz.toml`). v1 = path deps
  solamente; bumps incrementales si rompemos compat.
- **`Dependency` enum con `serde(untagged)`**: acepta
  `foo = "1.2.3"` (`Version(String)`) y
  `foo = { path = "..." }` (`Detailed(...)`). Los campos
  `git`/`tag`/`rev` se aceptan al parse pero el resolver los
  rechaza con error claro citando `9.y.3.c`. La versión suelta
  se rechaza citando `9.y.5` (registry). Política: parser
  permisivo, resolver estricto → mensajes accionables.
- **`Lib.entry` obligatorio** (sin default mágico). Quien quiere
  exponer su proyecto como library lo declara explícito.
- **Path deps son librerías por definición**: si una dep tiene
  solo `[bin]` (sin `[lib]`), el resolver aborta con la sección
  `[lib]` sugerida inline en el mensaje.
- **Resolución determinística**: trivial para path deps (un dep,
  un path). Para git/registry vendrá un algoritmo real cuando
  aparezca (lean global 5: empezar greedy, migrar a PubGrub si
  duele).
- **Lockfile siempre re-generado** en `fitz run`/`build`/`check`
  (idempotente para path deps). `write_lockfile_if_changed` hace
  short-circuit byte-a-byte cuando el contenido coincide — no
  spam de mtime, diff vacío.
- **Lockfile NO se emite si no hay deps**: proyecto vacío de
  `[dependencies]` no genera `fitz.lock` (sin valor, ruido).
- **Notificación discreta**: `✓ actualizado fitz.lock` solo
  cuando re-escribe; caso 90% silencioso.

###### Total al cierre

**1270 unit** (+24: 10 manifest + 14 lockfile) **+ 28 cli_e2e**
(+8 de 9.y.3.a sobre los 20 previos) **+ 79 compile_e2e + 3
openapi**. Clippy `-D warnings` limpio. **Sin breaking**: las
79 corridas de `compile_e2e` (single-file mode) verdes idéntico,
todos los E2E previos de 9.y.1/9.y.2 verdes.

Archivos:

- `src/manifest.rs` (+~320 LoC) — `Dependency` enum,
  `DetailedDependency`, `Lib`, `Manifest.lib`,
  `Manifest.dependencies: BTreeMap<String, Dependency>`,
  `ResolvedDep`, `ResolvedDepSource`, `resolve_dependencies`,
  5 variants nuevas en `ManifestError` con mensajes accionables.
- `src/lockfile.rs` (nuevo, +~250 LoC) — `Lockfile`,
  `LockedPackage`, `LockfileError`, `parse`/`to_toml_string`,
  `from_resolved` (ordena alfabéticamente), `lockfile_matches`,
  `write_lockfile_if_changed`.
- `src/lib.rs` — `pub mod lockfile`.
- `src/main.rs` — `sync_lockfile_if_needed(&ResolvedEntry)`
  llamado desde Run/Build/Check arms.
- `tests/cli_e2e.rs` (+~130 LoC) — 8 E2E nuevos: path dep
  emitido, idempotencia, regen en cambio de versión, sin deps
  no emite, errores (`version`/`git`/`path inexistente`/`sin [lib]`).

###### Decisiones residuales (NO bloquean 9.y.3.b)

- **Loader integration**: las deps están en el lockfile pero
  `from utils-lib import helper` desde el código del importer
  todavía NO resuelve. Es exactamente el scope de 9.y.3.b.
- **Cache local** en `~/.fitz/cache/`: no aplica para path deps
  (los paths son referencias directas al filesystem). Necesario
  para git/registry — llega con 9.y.3.c.
- **`source` field en lockfile**: el campo está reservado en el
  schema (`LockedPackage.source: Option<String>`) pero solo se
  emite en `None` para path deps en 9.y.3.a. 9.y.3.c lo poblará
  con `"git+url#sha"`.
- **Aliasing de deps** (`foo = { path = "../bar", package = "bar" }`):
  diferido. Hoy el nombre en `[dependencies]` debe matchear el
  `package.name` del manifest de la dep.
- **Validación cruzada** (no permitir `path` + `git` juntos):
  diferida. El resolver hoy chequea `path` primero, después
  `git`/`tag`/`rev` por separado; combinaciones inválidas dan
  errores secuenciales en vez de combinados.

###### Próximo norte tras 9.y.3.a

**9.y.3.b — Loader integration**: cambiar el module loader
(usado por `evaluator::load_module` en `fitz run` y por
`codegen` en `fitz build`) para que consulte el dep registry
resuelto antes de fallback a paths relativos. `from utils-lib
import helper` resuelve contra `<utils-lib>/src/lib.fitz`
(el `lib_entry` de `ResolvedDep`). Toca lo más invasivo de
9.y.3 — refactor del loader compartido.

##### 9.y.3.b — Loader integration ✓ (CERRADA, 2026-05-16)

Segundo slice. Hace que las deps declaradas y bloqueadas en
9.y.3.a sean **realmente usables** desde código Fitz. El loader
del evaluator (`fitz run`) y el del codegen (`fitz build`)
consultan ahora el `dep_registry` resuelto del manifest ANTES de
fallback a paths relativos del importer. `from <dep-name> import
X` resuelve al `lib_entry` absoluto de la dep.

###### Decisiones técnicas tomadas

- **`dep_registry` como `HashMap<String, PathBuf>`** (alias
  `manifest::DepRegistry`) — map liviano `dep-name → lib_entry-
  absoluto`. Helper `build_dep_registry(&[ResolvedDep])`
  centralizado en `manifest.rs`.
- **Resolución orden 1 — dep registry shortcut**: en
  `resolve_module_path` (evaluator) y `resolve_path` (codegen),
  si `segments.len() == 1` y matchea key del registry, devolver
  `lib_entry` directo. Solo single-segment porque los nombres de
  dep son siempre identificadores planos.
- **Resolución orden 2 — path relativo (fallback)**: si no hay
  match en el registry, comportamiento idéntico a pre-9.y.3.b.
  Single-file mode (sin manifest) pasa registry vacío → siempre
  fallback.
- **Dep shadowea archivo local con mismo nombre**: si tenés
  `[dependencies] foo = { ... }` Y un `src/foo.fitz` local, gana
  la dep. Decisión explícita: la declaración en `[dependencies]`
  es intención primaria del usuario. Test cubre.
- **API del evaluator extendida con backward compat**:
  `eval_with_base(_sync)` quedan como wrappers de los nuevos
  `eval_with_base_and_deps(_sync)` con registry vacío. Tests y
  callers externos (openapi) no necesitan migrar si no quieren
  consumir deps.
- **`fitz check` NO consume dep_registry**: el checker no
  recursea en módulos importados (los nombres se tipan como
  Any/nominal placeholder). La validación real ocurre en run/
  build. Wiring queda como futuro si el checker mejora cross-
  module.
- **Transitive deps NO soportadas en 9.y.3.b**: el dep_registry
  es flat (solo deps del importer raíz). El check de 5b.5 del
  codegen ("imports transitivos no soportados") sigue activo.
  Una dep no puede a su vez tener `[dependencies]` propias
  usables. Para soportarlo hay que resolver recursivamente las
  deps de las deps + extender el registry — refactor mayor, deuda
  futura.
- **`ManifestCtx.resolved_deps` poblado en `resolve_entry`**:
  single source of truth. `sync_lockfile_if_needed` ya no
  re-resuelve. `dep_registry_from(&ResolvedEntry)` helper arma
  el registry desde ahí.

###### Total al cierre

**1270 unit + 33 cli_e2e** (+5 de 9.y.3.b sobre los 28 previos)
**+ 79 compile_e2e + 3 openapi**. Clippy `-D warnings` limpio.
**Sin breaking**: los 79 compile_e2e (single-file mode) verdes
idénticos; los 28 cli_e2e previos también.

Archivos:

- `src/manifest.rs` — `DepRegistry` type alias +
  `build_dep_registry()` helper.
- `src/evaluator.rs` — `Loader.dep_registry` campo nuevo;
  `install_loader(base, deps)` signature ampliada;
  `resolve_module_path` con shortcut + fallback; pub APIs nuevas
  `eval_with_base_and_deps(_sync)`; los viejos
  `eval_with_base(_sync)` quedan como wrappers con empty deps.
- `src/codegen.rs` — `ModuleLoader.dep_registry` campo nuevo;
  `ModuleLoader::new(base, deps)` signature ampliada;
  `resolve_path` con shortcut + fallback; `generate_project`
  signature ampliada; 2 test call sites actualizados.
- `src/main.rs` — `ManifestCtx.resolved_deps` campo nuevo
  populated en `resolve_entry`; `sync_lockfile_if_needed` ya no
  re-resuelve (consume directo); `dep_registry_from()` helper;
  dispatch de Run/Build pasa el registry a `run_file`/`build_file`;
  `run_file` invoca `eval_with_base_and_deps_sync`; `build_file`
  toma `dep_registry` y la pasa a `generate_project`.
- `tests/cli_e2e.rs` — 5 E2E nuevos:
  `run_resuelve_from_dep_import_via_dep_registry`,
  `run_dep_no_referenciada_no_falla_si_no_se_importa`,
  `run_local_fitz_no_es_shadoweado_por_archivo_inexistente`
  (fallback path-relativo en proyecto sin deps en manifest),
  `run_dep_shadowea_archivo_local_con_mismo_nombre`,
  `build_resuelve_from_dep_import_via_dep_registry`.

###### Decisiones residuales (NO bloquean 9.y.3.c)

- **Hyphens en dep names**: aceptados en el manifest (la regex
  crates.io-style los permite), pero NO importables porque el
  parser de Fitz no acepta `-` en identifiers (`from utils-lib
  import X` no parsea: `-` separa expresiones). Auto-translation
  `-` ↔ `_` queda como deuda 9.y.4 (igual que Rust auto-traduce
  `my-crate` ↔ `my_crate` en `use`).
- **Transitive deps** (deps de deps): no soportadas. La dep
  registry es flat. Requiere refactor del manifest loading para
  resolver recursivamente. Deuda futura sin sub-paso comprometido.
- **`fitz check` cross-module**: no chequea contra el código real
  de las deps. Los nombres importados se tipan como Any/nominal
  placeholder; la validación real es en run/build. Si el LSP
  o `fitz check` mejoran para cross-module, hay que extender el
  pipeline del checker.
- **Smoke E2E con HTTP + deps**: no testeado en 9.y.3.b (los
  E2E nuevos son CLI). El path debería funcionar igual (el
  codegen no diferencia tipo de programa al cargar deps), pero
  sin E2E que lo valide. Deuda menor.

###### Próximo norte tras 9.y.3.b

**9.y.3.c — Git deps + cache local**: tercer slice del bloque.
Habilita `[dependencies] helpers = { git = "...", tag = "..." }`
clonando a `~/.fitz/cache/git/<hash>/`, lockfile registra commit
hash exacto. Cierra 9.y.3 entera.

##### 9.y.3.c — Git deps + cache local ✓ (CERRADA, 2026-05-16)

Tercer y último slice. Habilita deps de repos remotos. **Cierra
9.y.3 entera** (a + b + c).

- **Sintaxis** en `fitz.toml`:
  ```toml
  [dependencies]
  helpers = { git = "https://github.com/foo/bar", tag = "v1.0.0" }
  # `rev` también soportado:
  # helpers = { git = "https://github.com/foo/bar", rev = "abc123..." }
  ```
- **Cache local** en `<cache>/git/<url-sanitized>@<ref>/`. Default
  `~/.fitz/cache/`, override con env var `FITZ_CACHE_DIR` (esencial
  para tests aislados + power users).
- **Lockfile** registra el commit hash exacto Cargo-style:
  ```toml
  [[package]]
  name = "helpers"
  version = "1.0.0"
  source = "git+https://github.com/foo/bar#abc123def4567890..."
  ```

###### Decisiones técnicas tomadas

- **Subprocess `git`** sobre crate (`git2`/`gix`): zero deps
  adicionales, asume `git` en el PATH (caso 99% para dev de
  Fitz). Trade-off: si git no está instalado, error claro
  pidiendo instalar.
- **`tag` XOR `rev`**, NUNCA juntos. `branch` NO soportado
  intencionalmente — los branches mutan upstream y rompen
  reproducibilidad. Validado al resolver con mensaje accionable.
- **Cache directory naming**: URL sanitizada + `@` + ref, sin
  hashing. Determinístico y human-readable
  (`github.com_foo_bar@v1.0.0/`). Trunca a 200 chars para
  filesystem safety en Windows. Sin sha2 dep — sanitización
  textual alcanza para el caso 99%.
- **Cache reuse**: si el dir existe, asume el clone previo es
  válido y solo lee el commit hash. Sin re-clone automático.
  Invalidación manual (borrar el dir, o `fitz cache clean` que
  llega post-MVP).
- **Estrategia de clone**:
  - Tag → `git clone --depth 1 --branch <tag>` (eficiente,
    shallow).
  - Rev → `git clone` full + `git checkout <sha>` (git no acepta
    SHAs en `--branch`). Wasteful pero correcto; optimización
    con `--filter=blob:none` es deuda.
- **Source format del lockfile**: `git+<url>#<commit-hash>`
  (Cargo-style, 40-char SHA completo). Path deps siguen sin
  emitir `source` (convención Cargo: implícitas).
- **Validaciones cruzadas con mensajes accionables**: `path` +
  `git` (combinación inválida), `tag` + `rev` (mutuamente
  exclusivos), `tag`/`rev` sin `git` (falta url), `git` sin
  `tag`/`rev` (cita reproducibilidad + por qué no `branch`),
  `tag` o `rev` vacíos.
- **`FITZ_CACHE_DIR` env var** override del default
  `~/.fitz/cache/`. Esencial para tests E2E (cada test crea un
  tempdir y lo apunta como cache para no contaminar el home
  real).

###### Total al cierre

**1283 unit + 37 cli_e2e** (+8 unit en git_dep + 5 nuevos en
manifest − 1 viejo eliminado; +4 E2E nuevos sobre los 33
previos) **+ 79 compile_e2e + 3 openapi**. Clippy `-D warnings`
limpio. **Sin breaking**: los 33 cli_e2e + 79 compile_e2e
previos verdes idénticos.

Archivos:

- `src/git_dep.rs` (nuevo, +~320 LoC) — `cache_root` con
  `FITZ_CACHE_DIR` override, `GitRef` enum, `sanitize_url`,
  `cache_path_for`, `clone_or_use_cache`, `git_rev_parse_head`,
  `lockfile_source_string`, `GitDepError`. 8 unit tests.
- `src/manifest.rs` (+~180 LoC) — `ResolvedDepSource::Git { url,
  requested, commit_hash }` variant nuevo, `parse_git_ref`
  (valida tag XOR rev), `resolve_git_dep` (clone + read manifest
  dep + valida `[lib]`), 2 variants nuevas en `ManifestError`
  (`DepInvalidGitShape`, `DepGitError`), routing en
  `resolve_single_dep`. 6 unit tests nuevos.
- `src/lockfile.rs` — `from_resolved` emite `source =
  "git+<url>#<sha>"` para git deps via
  `git_dep::lockfile_source_string`.
- `src/lib.rs` — `pub mod git_dep`.
- `tests/cli_e2e.rs` (+~200 LoC) — 4 E2E nuevos:
  `git_dep_clona_al_cache_y_emite_lockfile_con_commit`,
  `git_dep_reusa_cache_sin_re_clonar` (verifica con marker file
  que el dir no se sobrescribe),
  `git_dep_lockfile_idempotente_si_commit_no_cambia`,
  `git_dep_tag_inexistente_aborta_con_mensaje_de_git`. Helpers
  `init_git_repo_with_tag`, `setup_git_dep_project`,
  `run_fitz_with_cache` (env-aware).

###### Decisiones residuales (NO bloquean 9.y.4)

- **Drift entre lockfile commit y cache borrado**: si el cache
  desaparece y al re-clone el upstream movió el tag, el nuevo
  commit difiere del lockfile registrado. Hoy NO se detecta —
  prevalence baja (tags inmutables son convención), pero deuda
  explícita para 9.y.4 (lockfile-driven re-clone con verificación).
- **`fitz cache clean`**: borrar el cache es manual hoy
  (`rm -rf ~/.fitz/cache/git/...`). Sub-comando explícito llega
  post-MVP.
- **Auth para repos privados**: delegado al `git` del sistema
  (SSH/HTTPS credentials del usuario). Sin tokens explícitos en
  el manifest todavía — diseño futuro cuando aparezca demanda.
- **Shallow clone con `--filter`**: tags usan `--depth 1` ya;
  revs (SHAs) usan full clone porque `--branch` no acepta SHAs.
  Optimización con `--filter=blob:none` para revs queda como
  deuda menor de performance.
- **Verificación de integridad** (commit signature, GPG): no
  en MVP. Si aparece demanda real, integrar via `git verify-commit`.
- **Concurrencia de clone**: dos `fitz run` simultáneos sobre el
  mismo proyecto podrían intentar clonar la misma dep en paralelo
  (race condition al crear el dir). Hoy no lo manejamos — caso
  raro en práctica. Lockfile-style locking sobre el cache root
  llega si aparece.

###### Próximo norte tras 9.y.3 entera

**9.y.4 — `fitz add` / `fitz remove` / `fitz update`** —
sub-comandos de manipulación del manifest + lockfile. `fitz add
foo@1.0.0` agrega entry a `[dependencies]`, resuelve, actualiza
`fitz.lock`. Hoy el usuario edita el manifest a mano; 9.y.4 lo
automatiza con UX `cargo add`-style.

#### 9.y.4 — `fitz add` / `fitz remove` / `fitz update` ✓ (CERRADA, 2026-05-16)

Cuarto sub-paso del package manager. Automatiza la edición del
manifest + lockfile que hasta 9.y.3 era manual. Tres subcomandos
nuevos con UX cargo-style.

- **`fitz add <name> --path <p>`** — agrega path dep.
- **`fitz add <name> --git <url> --tag <t>`** (o `--rev <r>`) —
  agrega git dep. clap valida conflicts entre `path`/`git` y
  entre `tag`/`rev`, y exige `git` si se pasa `tag`/`rev`.
- **`fitz add <name>`** sin flags — error claro citando 9.y.5
  (registry futuro).
- **`fitz remove <name>`** — quita entry del manifest y
  sincroniza `fitz.lock`. Si la dep era la única, borra el
  lockfile entero (deps vacías).
- **`fitz update [name]`** — invalida el cache de git deps
  (force re-clone con commit fresh del tag/rev). Para path
  deps es no-op (siempre fresh). Sin `name` actualiza todas;
  con `name` solo esa (error claro si no existe).

###### Decisiones técnicas tomadas

- **`toml_edit = "0.22"`** dep nueva para preservar comentarios
  y formatting al modificar `fitz.toml`. La crate `toml` que ya
  teníamos serializa de cero — pierde comentarios y orden del
  usuario. `toml_edit` mantiene la representación lo más fiel
  posible. Test cubre: manifest con comments sobrevive
  add+remove.
- **Persist eager incluso si la resolución falla** (cargo-style):
  `fitz add` escribe primero, después resuelve. Si la resolución
  falla, el manifest ya está modificado — el usuario puede
  `fitz remove <name>` para revertir. Trade-off documentado.
- **Validación cruzada delegada a clap** (`conflicts_with` /
  `requires`): mensajes de error limpios sin código custom para
  `--path` + `--git`, `--tag` + `--rev`, `--tag`/`--rev` sin
  `--git`.
- **`fitz add` sobre dep existente**: sobreescribe sin warning
  (cargo-style). Test cubre.
- **`fitz remove` cuando la dep era la única**: borra el
  `fitz.lock` entero (sync_lockfile_if_needed sería no-op porque
  `resolved_deps` queda vacío, dejaría stale state).
- **`fitz update` flow**: borra cache dirs de git deps + llama
  `resolve_entry(None)` que re-resuelve via `find_manifest`
  (re-clone automático porque el cache no existe). Las path deps
  no tienen cache, son no-op explícitas.
- **`fitz update no-existe`**: error claro (no silent no-op) —
  UX: si el user typea mal el nombre, queremos feedback.
- **Dev deps `[dev-dependencies]`**: diferidas a 9.z.2
  (`fitz test`) — no scope de 9.y.4.
- **`AddDepSpec` enum** (`Path` | `Git`) en `manifest.rs` —
  aislado del shape parsed (`DetailedDependency`) para que el
  helper de edición sea explícito sobre qué shape produce.

###### Total al cierre

**1294 unit + 48 cli_e2e** (+11 nuevos sobre los 37 previos) **+
79 compile_e2e + 3 openapi**. Clippy `-D warnings` limpio. Sin
breaking en los E2E previos de 9.y.1/.2/.3.

Archivos:

- `Cargo.toml` — dep nueva `toml_edit = "0.22"` (preserva
  comments + formatting en edits de `fitz.toml`).
- `src/manifest.rs` (+~180 LoC) — `AddDepSpec` enum,
  `add_dep_to_manifest`, `remove_dep_from_manifest`,
  `build_inline_dep_table`, `ManifestError::EditParse` variant.
  11 unit tests nuevos (add path/git tag/rev, sobreescribe, sin
  `[dependencies]`, preserva comentarios; remove existente /
  inexistente / borra sección si queda vacía; add+remove inversa).
- `src/main.rs` (+~200 LoC) — `Commands::Add`/`Remove`/`Update`
  en clap con flags + validation via `conflicts_with`/`requires`;
  `add_dep_cmd` / `remove_dep_cmd` / `update_deps_cmd`;
  `find_local_manifest_or_exit` helper compartido.
- `tests/cli_e2e.rs` (+~250 LoC) — 11 E2E nuevos cubriendo
  todos los caminos del CLI + errores: add path/git/sin flags/
  sin tag-rev/conflicts/fuera de proyecto/sobreescribe; remove
  existente/inexistente; update sin git deps/invalida cache con
  marker file/dep inexistente.

###### Decisiones residuales (NO bloquean 9.y.5)

- **`fitz add foo@1.2.3` (npm-style sugar)**: no soportado.
  Solo flags (`--version`, `--tag`, `--rev`). Si aparece demanda,
  trivial agregar como parser layer encima de clap.
- **Default version range** (`^1.2.3` vs `=1.2.3`): irrelevante
  hasta 9.y.5 (registry). Hoy las versiones sueltas
  (`foo = "1.0.0"`) abortan al resolver.
- **`fitz upgrade <dep>`** (semver bump dentro del rango):
  diferido a 9.y.5 (necesita registry).
- **Dev deps `[dev-dependencies]`**: diferidas a 9.z.2
  (`fitz test` discovery).
- **Drift detection en `fitz update`** (avisar si el commit nuevo
  difiere del lockfile previo): no implementado. El re-clone
  silenciosamente sobreescribe el lockfile entry. Refinable
  si aparece presión.

###### Próximo norte tras 9.y.4

**9.y.5 — Registry: servicio + protocolo** — el paso más grande
del bloque 9.y. Diseño y deployment del servicio que aloja los
paquetes (`crates.io`-style centralizado, escrito en Fitz mismo).
Habilita `fitz add foo@1.0.0` con resolución contra el registry.

#### 9.y.5 — Registry: servicio + protocolo

**El paso más grande del bloque.** Diseño y deployment del
servicio que aloja los paquetes.

- **Diseño del servicio**: API HTTP que expone:
  - `GET /api/v1/packages/<name>` — metadata + lista de versiones
  - `GET /api/v1/packages/<name>/<version>` — metadata de versión
  - `GET /api/v1/packages/<name>/<version>/download` — tarball
  - `PUT /api/v1/packages/<name>/<version>` — publish (auth
    required)
- **Implementación en Fitz** — autoesfecto: el registry escrito
  en Fitz mismo. Validación viviente de que el lenguaje sirve
  para construir servicios reales. **Esto es lo que hace único a
  Fitz vs npm/crates.io**: el registry corre en su propio
  lenguaje.
- **Storage del tarball**: filesystem local para empezar,
  S3-compatible para producción. Decisión: ¿S3, Cloudflare R2, o
  algo más barato como Backblaze B2?
- **Auth**: token bearer en header. Los tokens se generan via
  `fitz login` con username/password contra el registry. JWT
  firmado.
- **Index de paquetes**: ¿base de datos relacional o git-backed
  (Cargo crates.io fue git-backed hasta 2023)? Lean: **SQLite/
  Postgres directo, sin git index** (más simple, evita el
  problema de escala que crates.io enfrentó).
- **Decisiones críticas**:
  - **¿Quién hospeda?** Decisión grande. Opciones: Railway
    (fácil, caro a escala), VPS propio (más control, más
    sysadmin), self-hostable (federación). Lean: **VPS propio o
    Railway para arrancar**; documentar self-hosting como
    tier-1.
  - **¿Versiones inmutables?** Una vez publicada `1.2.3`, no se
    puede sobreescribir (npm tiene esto). Sí, regla estricta.
  - **¿Yanking?** Permite marcar versión como "no usar" sin
    removerla. Sí, estándar.
  - **¿Hard delete?** Solo por admin del registry, casos
    extremos (paquete con secret leak). Documentado.
  - **¿Search en el registry?** Por ahora `GET /api/v1/search?q=foo`.
    UI web llega post-MVP del registry.
- **Trade-offs**: implementar el registry en Fitz toma 2-3x más
  tiempo que escribirlo en algo conocido, pero da una **historia
  poderosa** para la web del lenguaje. "El registry de Fitz está
  escrito en Fitz" es un argumento de marketing real.

#### 9.y.6 — `fitz publish` + auth

Cierra el ciclo: ahora el usuario puede compartir.

- **`fitz login`** — pide token al registry, lo guarda en
  `~/.fitz/credentials.toml`.
- **`fitz logout`** — borra el credential.
- **`fitz publish`** — empaqueta el proyecto en tarball (`*.fitz`
  + `fitz.toml` + `README.md` + `LICENSE`), valida que el
  manifest tenga campos obligatorios, sube al registry.
- **`fitz yank <pkg>@<ver>`** — marca versión como obsoleta sin
  borrar.
- **Verificaciones pre-publish**:
  - Versión nueva (no sobreescribe existente).
  - Nombre disponible o usuario es owner.
  - Manifest tiene `description`, `license`, `authors` (campos
    obligatorios para publicar — opcionales para uso local).
- **Decisiones**:
  - ¿`.fitzignore` (estilo `.gitignore`) o lista explícita en
    manifest (`include`/`exclude` de Cargo)?
  - ¿Squat protection en nombres? (npm tiene reservados,
    crates.io permitió squatting durante años).

#### 9.y.7 — Guía + ejemplo end-to-end + cierre formal

- Capítulo nuevo en `docs/guide.md`: "Tu primer paquete Fitz" —
  `fitz new`, agregar dep, publicar.
- Ejemplo en el repo: paquete chico publicable (ej. `fitz-uuid`
  con un generador UUID v4 puro).
- CHANGELOG bumps, roadmap marca todo como CERRADO.
- Documentar el setup del registry (auto-hosting + servicio
  oficial cuando exista).

### Deuda anticipada (NO bloquea cierre del bloque)

- **Workspaces** (multi-paquete bajo manifest raíz) — sub-paso
  futuro 9.y.8+.
- **Build scripts** (ejecutar tooling externo desde manifest) —
  futuro.
- **Vendoring** (`fitz vendor` para offline builds) — futuro.
- **Feature flags** (Cargo features) — futuro, cuando aparezca
  el primer uso real.
- **Web UI del registry** (npm-style browse + docs) — post-MVP
  del servicio.
- **2FA + OAuth** (login con GitHub) — post-MVP.

### Próximo norte tras 9.y

**Fase 9.z (DX completo)** — formatter + test + dev + repl +
lint. Naturalmente engrana con el package manager: `fitz test`
lee el manifest, `fitz dev` se beneficia del project layout,
`fitz fmt` se aplica a todo el proyecto, no a archivos sueltos.

---

## Fase 9.z — DX completo: formatter, test, dev, repl, lint ✨

**Estado: PENDIENTE (siguiente bloque post-package manager)** —
segundo trabajo del resto de Fase 9.

**Objetivo**: cerrar la experiencia del developer al nivel de los
lenguajes modernos. Cubre las 5 herramientas que un dev espera al
sentarse a escribir código serio: formatter (sin debates), tests
(sin libs externas), hot reload (sin nodemon), REPL (para
experimentar), linter (para patrones más allá de tipos). **Apuesta
de diferenciación**: la combinación viene en el lenguaje, no como
un ecosistema disperso de packages a elegir.

**Por qué encaja en Fase 9**: tooling secundario. Cada uno reusa
la infraestructura del compilador (parser, checker, AST, TypeInfo
de F16). Ninguno requiere cambios del lenguaje core.

**Inspiración**:

- **gofmt** — cero config, opinionado, indiscutible.
- **cargo test + rust-analyzer test runner** — discovery por
  convención + decorators.
- **vite / uvicorn --reload** — file watcher + restart con
  feedback inmediato.
- **deno repl + ipython** — REPL persistente y multi-line.
- **clippy + biome** — lint estructural (no solo estilo).

### Decisiones pendientes globales del bloque

1. **¿`fitz fmt` cero config o config opt-in?** Lean: **cero
   config** (gofmt-style) — evita bikeshedding eterno.
2. **¿Tests: decorator `@test` o convención por nombre
   (`fn test_*`)?** Lean: **decorator** — consistente con
   `@get`/`@background`.
3. **¿Tests inline en el mismo archivo o separados (`tests/`)?**
   **Ambos**. Tests inline al final del archivo (Rust-style) +
   carpeta `tests/` para integration tests (Cargo-style).
4. **¿`fitz dev` reinicia el server al cambiar archivo, o solo
   recompila para CLI?** **Ambos**. Si hay `@get`/etc, restart
   server. Si no, just recompile + run.
5. **¿REPL: persistente del scope o cada comando independiente?**
   Lean: **persistente** — Python/Deno style.

### Sub-pasos

#### 9.z.1 — `fitz fmt` (formatter sin config) ✓ (CERRADO ENTERO, 2026-05-16)

Sub-paso grande — partido en dos sub-commits porque el descubrimiento
al hacer el smoke fue que **el lexer strippea comentarios antes de
llegar al AST**, así que sin trabajo adicional el formatter era
destructivo (borraba comments + blank lines). Comment preservation
es table-stakes para un formatter de producción (gofmt, prettier,
black todos preservan).

- **9.z.1.a — Pretty-printer + CLI con warning loud**
  ✓ CERRADO (2026-05-16). Cubre los nodos comunes del AST.
- **9.z.1.b — Comment + blank line preservation**
  ✓ CERRADO (2026-05-16). Lexer emite comments + blank lines como
  side-stream `Trivia`; el formatter threadea todo según posición
  original. El warning loud del modo write fue removido —
  `fitz fmt` es production-ready. Detalle abajo.

**Próximo norte tras 9.z.1**: **9.z.2** (`fitz test` con `@test`
builtin).

##### 9.z.1.a — Pretty-printer + CLI con warning loud ✓ (CERRADA, 2026-05-16)

- **Sub-comando** `fitz fmt [files...]` formatea in-place. Sin
  argumentos formatea todo el proyecto (vía manifest: walk
  recursivo de `src/`, excluye `target/` y dirs ocultos).
- **Modo check** `fitz fmt --check`: read-only, exit 1 si hay
  diffs (para CI). Safe — no rompe nada aunque la deuda 9.z.1.b
  no haya cerrado todavía.
- **Modo write**: emite warning ⚠ loud antes de cualquier
  modificación, citando 9.z.1.b como remediación.
- **Convenciones (cero config)**: 4 espacios indent, comillas
  dobles, blank line obligatoria solo entre fn/type top-level
  consecutivos, paréntesis en condición de `if`/`while`.
- **Implementación**: pretty-printer escrito a mano sobre el AST
  (no usa `parse_with_recovery` todavía — strict parse). Cubre
  >20 nodos: literales, let, fn (con/sin async, con/sin
  decorators), if/while/for/loop, match, struct lit, list/map,
  BinOp/UnaryOp, Call/Field/Index, Range, Ok/Err/Try/Await,
  FnExpr (preserva flecha si body es Return único), TypeDef
  con defaults, Decorator, Import/FromImport.

###### Decisiones técnicas tomadas

- **Indent: 4 espacios** (Python heritage). Comillas dobles.
- **Línea máxima**: NO se enforcea en MVP. Auto-wrap requiere
  análisis de break-points sensato — deuda futura.
- **Trailing comma**: solo en multi-línea (match arms,
  type fields).
- **`is_let` recuperado del source via `Span`**: el parser
  produce el mismo `Stmt::Assign` para `let x = 1` y `x = 1`.
  El formatter inspecciona `source[span.line]` para detectar la
  keyword. Hack contenido en `stmt_has_let_keyword`. Refactor
  del AST (agregar `is_let: bool`) es deuda menor.
- **`fn f() => expr` se normaliza a bloque**: el parser ya
  convierte `=>` a `[Return(expr)]`; sin info en AST para
  distinguir. Trade-off documentado.
- **`if` con paréntesis obligatorios** en la condición:
  consistente con el parser actual.
- **FnExpr inline preserva la flecha** (`fn(x) => expr`) cuando
  el body es un único `Return` — sintaxis expr-context.
- **Project discovery**: walk recursivo de `src/`, excluye
  `target/` y dirs ocultos (`.git/`, etc.), dedup por canonical
  path.
- **Warning loud en modo write**: una línea ⚠ al inicio de cada
  invocación de fmt (write mode), citando 9.z.1.b. `--check`
  silencioso.

###### Total al cierre

**1315 unit + 55 cli_e2e** (+21 unit en `fmt::tests` y +7 cli_e2e
fmt sobre los previos) **+ 79 compile_e2e + 3 openapi**. Clippy
`-D warnings` limpio.

Archivos:

- `src/fmt.rs` (nuevo, +~700 LoC) — `FmtCtx`, `format_source`,
  formatters por categoría de nodo, 21 unit tests inline (incl.
  idempotencia sobre programas complejos).
- `src/lib.rs` — `pub mod fmt`.
- `src/main.rs` (+~120 LoC) — `Commands::Fmt { files, check }` +
  `fmt_cmd` + `fmt_one_file` + `discover_project_fitz_files` +
  `collect_fitz_recursive` + warning loud en write mode.
- `tests/cli_e2e.rs` (+~120 LoC) — 7 E2E nuevos: archivo
  explícito canonicaliza, `--check` idempotente devuelve 0,
  `--check` no canónico devuelve 1 sin modificar, warning loud
  en write mode, `--check` no emite warning, error de sintaxis
  aborta sin escribir, sin args descubre archivos de `src/`.

###### Decisiones residuales (NO bloquean 9.z.1.b)

- **Comment + blank line preservation**: scope de 9.z.1.b.
- **`is_let` en AST**: deuda menor, refactor en cualquier
  momento; mientras tanto el hack via Span funciona.
- **Forma flecha de fn def**: AST no preserva. Refactor menor.
- **Auto-wrap líneas largas**: deuda futura post-9.z.1.b.
- **`parse_with_recovery` integration**: el formatter strict
  hoy aborta si el source tiene errores de sintaxis. Cuando
  9.z.1.b cierre podemos integrar recovery para formatear
  hasta donde el parser entiende.
- **Format on save desde el LSP** vía `textDocument/formatting`:
  llega gratis cuando 9.z.1.b cierre — `fmt::format_source` ya
  es una API libray-able.
- **`docs/fmt-style.md`** (referencia formal de convenciones):
  diferido a 9.z.1.b cuando el formatter sea production-ready.

##### 9.z.1.b — Comment + blank line preservation ✓ (CERRADA, 2026-05-16)

Cierra la deuda crítica de 9.z.1.a: el formatter ahora preserva
comentarios y blank lines del usuario al reescribir archivos.
`fitz fmt` es production-ready — el warning loud del modo write
fue removido.

###### Decisiones técnicas tomadas

- **Lexer Trivia side-stream**: nueva fn `tokenize_with_trivia(src)
  -> (Vec<TokenWithPos>, Trivia)` paralela a `tokenize`. La vieja
  `tokenize` sigue zero-overhead — parser/LSP/etc. no se ven
  afectados. AST sin cambios.
- **`Trivia` struct**: `Vec<Comment>` (con `kind: Line | Block`,
  `text`, `line`, `column`) + `Vec<usize>` con números de línea
  blank. Ambos en orden de aparición.
- **`Lexer.collect_trivia` flag** + `line_had_code` /
  `line_had_comment` para distinguir líneas blank (sin nada) de
  líneas comment-only (no son blanks).
- **Comments style normalization**: `//foo` → `// foo` (espacio
  post-`//`). Block comments `/* */` se preservan raw.
- **Trailing comments**: emitidos con **2 espacios** de separación
  (`stmt  // comment`).
- **Blank lines colapsadas**: máximo **1 consecutiva** (3 blanks
  del user → 1 en output).
- **Smart blank suppressed** cuando un leading comment se acaba
  de emitir: el comment "se ata" al stmt siguiente y no queremos
  blank entre ellos.
- **`fmt_stmt_list` con `in_block`**: blocks NO emiten trailing
  footer comments (los deja para el outer scope). Top-level sí.
- **`end_line_of_stmt`/`end_line_of_expr` recursivos**: necesario
  para detectar trailing comments en stmts multi-línea.
- **`expr_to_inline_string` usa `Trivia::default()`**: comments
  adentro de expresiones inline son deuda futura — no MVP.

###### Total al cierre

**1333 unit + 55 cli_e2e + 79 compile_e2e + 3 openapi** (+18 unit
sobre 9.z.1.a: 8 lexer trivia + 10 fmt threading + 0 cli_e2e neto
porque reemplazo 1 viejo). Clippy `-D warnings` limpio.

Archivos:

- `src/lexer.rs` (+~180 LoC) — `Trivia` + `Comment` + `CommentKind`
  + `Lexer.collect_trivia/trivia/line_had_*` + `tokenize_with_trivia`.
  8 unit tests nuevos.
- `src/fmt.rs` (+~280 LoC) — `FmtCtx.trivia` + `fmt_stmt_list`
  threading + `emit_leading_comments`/`emit_trailing_comments`/
  `emit_single_comment`/`peek_comment_at_line` + `end_line_of_stmt`/
  `end_line_of_expr` + `has_blank_between` + smart blank logic.
  10 unit tests nuevos (preserva comment líneas, multiples,
  blanks, trailing, normaliza `//foo`→`// foo`, mix, smoke
  02-hola, colapsa blanks, idempotencia con comments).
- `src/main.rs` — warning loud removido + docstring de `Commands::Fmt`
  reescrita reflejando production-ready.
- `tests/cli_e2e.rs` — `fmt_emite_warning_loud` reemplazado por
  `fmt_no_emite_warning_post_9z1b` (regresión guard) + nuevo
  `fmt_preserva_comments_y_blank_lines` (E2E del round-trip).

###### Smoke validado a mano

- `examples/guide/02-hola.fitz` round-trip exacto bit-a-bit:
  2 comments + 2 blank lines preservados.
- `examples/guide/04-operadores.fitz`: solo normaliza espacios
  pre-trailing (de 4-5 espacios alineados a 2 espacios canónicos).
- `examples/guide/13-metodos.fitz`: colapsa listas y method chains
  multi-línea a single-line (limitación documentada abajo).

###### Decisiones residuales (NO bloquean 9.z.2)

- **Multi-líneas de listas/maps/method chains se colapsan a
  single-line**: el formatter no preserva multi-líneas que el
  usuario haya formateado a mano para legibilidad. Si una lista
  era multi-línea en el source y entra (o no) en una sola línea,
  se inlinea. Auto-wrap line-length-aware es deuda futura — el
  problema general de pretty-printing con line breaks sensatos
  es no trivial.
- **`type` defs siempre multi-línea**: regardless de si el user
  los tenía inline.
- **Comments entre último stmt de un bloque y el `}`**: terminan
  saliendo del bloque al re-formatear (caso raro en práctica).
  Documentado.
- **Comments adentro de expresiones** (`f(x, // foo\n y)`): no
  soportados. Fitz no genera mucho de este patrón, deuda futura.
- **`parse_with_recovery` integration**: el formatter strict hoy
  aborta si hay errores de sintaxis. Cuando aparezca presión,
  integrar recovery para formatear hasta donde el parser entiende.
- **Format on save desde el LSP** vía `textDocument/formatting`:
  llega gratis ahora que `fmt::format_source` es library-able y
  production-ready. Sub-paso opcional si aparece demanda.

###### Próximo norte tras 9.z.1 entera

**9.z.2 — `fitz test`** — decorator `@test` sobre fn sin args +
builtins de aserción (`assert`, `assert_eq`, `assert_ne`,
`assert_throws`) + sub-comando `fitz test` con discovery por
proyecto (inline + carpeta `tests/`). **CERRADO entero el
2026-05-17** (a + b + c).

#### 9.z.2 — `fitz test` (testing built-in) — CERRADO 2026-05-17

Test runner integrado al lenguaje. Tres sub-pasos cerrados:

##### 9.z.2.a — `@test` decorator + assertion builtins + TestRegistry ✓

- `src/testing.rs` nuevo: `TestRegistry`, `TestSpec`,
  thread-local + `with_active_test_registry` (sync/async).
  Mirror chico de `http::HTTP_REGISTRY` con la asimetría clave:
  sin registry activo, `@test` es **no-op silencioso** (paralelo
  a `#[cfg(test)]` Rust).
- `evaluator.rs::process_decorator` branch `@test` con
  `register_test`: valida args/kwargs/params vacíos, empuja
  `TestSpec` al registry activo.
- 4 assertion builtins en `register_builtins`: `assert(cond: Bool,
  msg: Str?)`, `assert_eq(a, b)`, `assert_ne(a, b)`,
  `assert_throws(fn)`. Estilo cargo (`left`/`right` en
  `assert_eq`). Igualdad estructural recursiva con coerción
  Int↔Float (reusa `PartialEq` de Value).
- `assert_throws` **caso especial** en `invoke_value`: los
  builtins son sync pero invocar el callback Fitz requiere
  async-recurse. Solo SYNC callbacks en MVP — async cb
  rechazado en runtime (deuda visible).
- Pre-registro en checker (`types.rs::register_builtins`) +
  completion en LSP (`lsp.rs`).
- **Cambio retro-compatible al parser**: paréntesis opcionales en
  decorators (`@test fn ...`). Los demás siguen funcionando con
  o sin paréntesis.
- Tests: +6 testing.rs (registry), +7 evaluator (@test decorator),
  +18 evaluator (asserts), +2 parser (regression). Total +33 unit.

##### 9.z.2.b — `Commands::Test` + discovery + runner cargo-style ✓

- `Commands::Test { filter, file }` en CLI.
- **Single-file mode** (`fitz test --file archivo.fitz`): carga
  el archivo, descubre `@test`, los corre.
- **Manifest mode** (`fitz test`): discovery automático con
  estrategia de dedup:
  - Si hay `tests/*.fitz` top-level: solo carga esos. El `[lib]`
    se carga vía import (auto-self-registrado en `dep_registry`
    bajo `package.name` — paralelo a `use my_crate::*` Rust).
  - Si NO hay tests integration: carga el `[lib].entry` direct
    para soportar tests inline solo-lib.
- **Filtrado**: substring case-sensitive del nombre del test
  (cargo default). Tests excluidos cuentan como "filtered out"
  en el output.
- **Output estilo cargo**: `running N tests` + por test
  `test <file>::<name> ... ok/FAILED` + sección `failures:` con
  detalle + summary `test result: ... passed; ... failed;
  finished in Ts`. ANSI colors auto cuando stdout es TTY
  (`std::io::IsTerminal`, **cero deps nuevas**).
- **Async tests**: `evaluator::run_test_handler` encapsula
  invoke + await del `Future` resultante.
- Exit code 1 si ≥1 falla, 0 si todos pasan.
- El loader sobrescribe `CURRENT_TEST_SOURCE` al cargar módulos
  importados: los `@test` quedan etiquetados con su archivo
  declarante real (no con el del importer).
- Tests: +11 cli_e2e nuevos.

##### 9.z.2.c — Guía + ejemplo + cierre formal ✓

- Cap 24 nuevo **"`fitz test` — testing built-in"** en
  `docs/guide.md`: features, CLI (single-file / manifest),
  filtrado, output cargo-style, async tests, estructura típica
  de proyecto, limitaciones. Renumeración cap 24→25 ("Qué sigue").
- Ejemplo runnable `examples/guide/24-tests.fitz` con `factorial`
  + 3 tests OK + 1 FAILED intencional. Sumado al smoke
  `GUIDE_EXAMPLES_COMPILE` (compila con `fitz build` porque
  codegen ignora `@test`).
- Codegen: `@test fn` se **ignora silenciosamente** en
  `fitz build` (paralelo a `#[cfg(test)]`). Bug fix colateral
  en `has_http_routes`: contar `@test` como HTTP disparaba
  servidor en CLI puros — refinado a solo `get`/`post`/`put`/
  `delete`/`server`.
- CHANGELOG v0.9.16, este archivo (este bloque), `docs/deudas-
  post-5b.md` (bloque "Fase 9.z.2 entera CERRADA"), README,
  `docs/syntax-spec.md` (sección "Testing" pasa de "futuro" a
  "implementado").

**Decisiones tomadas durante 9.z.2**:
- `panic(msg)` (mencionado en el ejemplo del syntax-spec) NO entra
  al MVP. Los 4 oficiales bastan.
- `assert_throws` solo SYNC callbacks. Async cb es sub-paso
  futuro si aparece presión.
- Discovery dedup: lib direct solo si NO hay tests integration.
- Auto-self-import bajo `package.name`: requiere nombre usable
  como ident Fitz (sin hyphens). Deuda visible.

**Tests al cierre**:
- 1366 unit / 66 cli_e2e / 79 compile_e2e / 3 openapi.
- Clippy `-D warnings` limpio.

**Deudas residuales (NO bloquean 9.z.3)**:
- `assert_throws` con callback async.
- Span del fallo en builtins (el `FitzError` lleva `line: 0,
  column: 0` — los builtins son sync y no reciben el span del
  call site). Refinamiento útil; necesita propagar el span al
  builtin via wrapper en `invoke_value`.
- Nombres de paquete con hyphens (`my-pkg`) no son importables
  desde Fitz (`from my-pkg import X` no parsea). Workaround:
  usar underscores. Documentado en cap 24.
- En modo "tests integration", si el `[lib]` tiene `@test` inline
  pero NINGÚN test integration importa la lib, esos tests no se
  descubren. Edge case raro.

#### 9.z.3 — `fitz dev` (hot reload) — CERRADO 2026-05-17

Sub-comando `fitz dev` que watchea el proyecto y kill+respawnea el
child al detectar cambio en `.fitz` o `fitz.toml`. Modo desarrollo
con feedback inmediato sin re-tipear `fitz run` en cada save.

**Implementación**:

- Dep nueva `notify = "6"` (file watcher cross-platform). Bridge
  sync→async via `std::thread::spawn` + `tokio::sync::mpsc` para
  consumir desde un runtime tokio current_thread.
- `Commands::Dev { file }`: sin args → manifest mode (busca
  `fitz.toml`, watch su dir, corre `fitz run`); con `--file` →
  single-file mode (watch parent del archivo).
- `dev_cmd` → `run_dev_loop`: outer loop spawnea child
  (`tokio::process::Command`) + select! sobre 3 eventos:
  - **Cambio del watcher**: debounce 100ms + kill child + respawn.
  - **Child terminó solo** (error de tipo, programa CLI corto):
    no salimos — esperamos próximo cambio para reiniciar.
  - **Ctrl+C** (`tokio::signal::ctrl_c()`): kill child +
    return Ok. Evita procesos zombie.
- Path filtering (`path_is_relevant`): sólo `*.fitz` y `fitz.toml`.
  Excluye en cualquier nivel: `target/`, `.git/`, `node_modules/`,
  `.fitz/`, `dist/`, `build/`, componentes ocultos.
- Banner UX (`clear_screen_and_banner`): ANSI `\x1b[2J\x1b[H` si
  stdout es TTY (`std::io::IsTerminal`), sino separa con líneas.
  Run number incremental + display del target.

**Decisiones tomadas**:

- `[dev]` section en `fitz.toml` para configurar watcher: **NO
  en MVP**, solo defaults.
- Browser auto-refresh para HTTP (inyectar WebSocket): **NO en
  MVP**. Live Server externo es workaround.
- Print errors del checker live sin restart: **NO** — el child
  imprime errors en arranque. El LSP (cap 22) ya da
  diagnostics in-editor para feedback continuo.
- Debounce 100ms manual con `tokio::time::timeout` (sin layer
  separado de debouncer).
- Smoke E2E automatizado: **NO** — el dev_cmd es interactivo y
  los file watchers son flaky en tests. Smoke manual valida.

**Cap 25 nuevo "`fitz dev` — hot reload"** en `docs/guide.md`:
features, CLI single-file/manifest, qué dispara restart, output
típico, limitaciones, integración con `fitz test`. Renumeración
cap 25→26 ("Qué sigue").

**Tests al cierre 9.z.3**:
- 1366 unit / 66 cli_e2e / 79 compile_e2e / 3 openapi.
- Smoke manual: arrancar `fitz dev --file`, modificar archivo,
  observar run #2 con código nuevo + banner ANSI.
- Clippy `-D warnings` limpio.

**Deudas residuales (NO bloquean 9.z.4)**:

- **Incremental rebuild**: kill+respawn full es el approach del
  MVP. Modelo de módulos pre-compilados queda como sub-paso
  futuro si los tiempos de re-eval duelen.
- **Filter "modify sin cambio real"**: timestamps tocados sin
  cambio de contenido disparan restart. Comparar hashes si
  aparece presión.
- **`fitz dev --test`**: workaround documentado con dos
  terminales (`fitz dev` + `while true; do fitz test; sleep 2;
  done`). Sub-paso si aparece presión.
- **Smoke E2E automatizado**: pendiente. Las pruebas de file
  watchers requieren orquestación específica.

#### 9.z.4 — `fitz repl` (REPL interactivo) — CERRADO 2026-05-17

Sub-comando `fitz repl` con prompt `fitz> `. Cada línea se evalúa
contra un env compartido, con multi-line continuation, comandos
especiales `:nombre`, history persistente y async transparente.

**Implementación**:

- Dep nueva `rustyline = "14"` (terminal handling: arrow keys,
  Ctrl+R history search, line editing, Ctrl+C/D diferenciados,
  file history).
- `Commands::Repl` sin args en CLI. Manifest mode/single-file no
  aplica al REPL (siempre single-session).
- `repl_cmd` adentro de `evaluator::build_runtime()` para que
  `sleep(100).await` y similares funcionen desde el prompt.
- `read_complete_input` lee líneas hasta balanced brackets
  (`input_is_complete`: cuenta `{`/`(`/`[` skipeando strings
  literales y comments). Heurística, no parser real — el parser
  puede aún emitir errores sintácticos distintos que se muestran
  y vuelven al prompt.
- 6 comandos especiales en `handle_special_command`:
  - `:help` / `:h` — lista comandos.
  - `:quit` / `:q` / `:exit` (o Ctrl+D) — sale.
  - `:env` — lista bindings del scope raíz, filtrando builtins
    (`evaluator::builtin_names()`).
  - `:reset` — re-crea el env (perdés todo).
  - `:type <expr>` — arma programa sintético `let __repl_type =
    <expr>` y lee el tipo del span del value desde `TypeInfo`.
    Limitación: no scope-aware (no ve vars previas del REPL).
  - `:load <archivo>` — eval del archivo contra el env actual,
    los bindings quedan disponibles.
- **History persistente**: `~/.fitz/history` (Linux/macOS) o
  `%USERPROFILE%\.fitz\history` (Windows). Carga al inicio,
  save al salir. rustyline maneja todo.
- **Pretty-print Python-style**: si el último stmt es
  `Stmt::Expr` y devuelve `Value != Null`, se imprime con
  `= <value>`. Los `let`/`fn` son silenciosos. `print(...)`
  imprime su cosa y devuelve Null (no doble línea).
- **Filtro de warning spurio**: el checker arma scope desde cero
  por invocación; sin filtro emitía "variable desconocida `x`"
  cada vez que el user usaba `x` después de `let x = 5`. Filtramos
  por substring del mensaje (no kind: todos los errors del checker
  llevan `ErrorKind::TypeError`).

**APIs nuevas (pub) en evaluator/env**:
- `evaluator::eval_program_with_env(program, base_dir, env,
  dep_registry) -> FitzResult<Value>`: evalúa contra un env
  externo que persiste entre invocaciones. Devuelve el `Value`
  del último stmt.
- `evaluator::new_repl_env() -> EnvRef`: crea env + registra
  builtins.
- `evaluator::builtin_names() -> &'static [&'static str]`: lista
  de builtins para filtrar `:env`.
- `Environment::local_names() -> Vec<String>`: nombres del scope
  actual sin recursar.

**Decisiones tomadas**:

- `:type` scope-aware: NO en MVP. Aceptado como deuda visible —
  refinable feedeando env al checker.
- Filtro warning checker por **substring** del mensaje (no por
  kind — `UndefinedVariable` es kind del evaluator, no del
  checker).
- Smoke E2E automatizado: NO — REPL es interactivo, smoke manual
  con stdin scripted valida. Tests automáticos serían flaky con
  rustyline + terminal handling.
- Manifest mode en REPL: NO — siempre single-session. `:load
  src/lib.fitz` es el workaround.
- Auto-completion de paths/nombres: NO en MVP.

**Cap 26 nuevo "`fitz repl` — REPL interactivo"** en
`docs/guide.md`: features (env compartido, multi-line, async,
pretty-print), comandos especiales con tabla, history persistente
con shortcuts (↑/↓/Ctrl+R/Ctrl+A/E), limitaciones, comparación
"cuándo usar repl vs run vs dev vs test". Renumeración cap 26→27
("Qué sigue").

**Tests al cierre 9.z.4**:
- 1366 unit / 66 cli_e2e / 79 compile_e2e / 3 openapi (sin
  cambios; repl_cmd interactivo no agrega tests automáticos).
- Smoke manual validó: literales, let/fn definitions, async
  await, comandos especiales, multi-line, history, `:load` con
  path absoluto, typo real produce error claro.
- Clippy `-D warnings` limpio.

**Deudas residuales (NO bloquean 9.z.5)**:

- **`:type` scope-aware**: refactor del checker para aceptar
  pre-declared scope. Sub-paso si aparece presión.
- **Smoke E2E automatizado**: rustyline + readline + raw mode
  son difíciles de testear sin TTY. Workaround: stdin scripted
  + matching de stdout. Pendiente si los smokes manuales se
  vuelven tediosos.
- **Indentación automática en multi-line continuation**: hoy el
  prompt `... ` no ajusta indent. El user lo tipea.
- **Comandos extras** (`:save`/`:undo`/`:debug`/auto-completion
  de paths/nombres): post-MVP si aparece demanda.
- **Manifest mode en `fitz repl`**: hoy single-session. Si el
  user quiere su proyecto cargado, usa `:load src/lib.fitz`.

#### 9.z.5 — `fitz lint` (más allá de tipos) — CERRADO 2026-05-17

Linter de patrones más allá de tipos. Complementa a `fitz check`
(tipos = errores bloqueantes) con sugerencias de estilo/patrón
(warnings, exit 0 por default). **Cierra Fase 9.z entera**.

**Implementación**:

- Módulo nuevo `src/lint.rs` (~700 LoC incluyendo 15 unit tests):
  framework `LintFinding` + walkers recursivos `collect_uses_in_*`
  y `walk_exprs_in_stmt` + supresión via inspección del source
  raw.
- `Commands::Lint { files, deny }` en CLI: sin args lintea todo
  el proyecto (manifest mode, reusa `discover_project_fitz_files`
  heredado de `fitz fmt`); con archivos lintea solo esos. `--deny
  <name>` repetible.
- Output cargo-clippy style: `warning:` amarillo / `error:` rojo
  con `--deny`, `--> file:line:col`, hint con `= nota:`, summary
  final. ANSI colors auto via `IsTerminal`.

**4 lints implementados** (de los 6 sugeridos en el roadmap
original):

- **`unused_variable`**: `let x = ...` (target Ident) cuyo nombre
  no aparece en `Expr::Ident` del programa. Skipea prefijo `_`.
  Walkea fns, while, loop, for. Params NO se flaguean en MVP.
- **`unused_import`**: `import X` y `from X import Y` cuyo
  binding no se usa. Maneja alias.
- **`useless_match`**: `match expr { _ => body }` con UN solo
  arm catch-all = equivalente a `let`.
- **`string_concat`**: `BinOp { op: Add, left: Str, right: Str }`
  con AMBOS literales. Sugiere interpolación. Concat con var
  queda OK.

**Lints skipeados del roadmap**:

- **`panic_in_test_only`**: NO aplica — Fitz no tiene `panic!`
  builtin distinguido (asserts son builtins normales).
- **`redundant_clone`**: requiere análisis de movimientos que
  el compilador no hace.

**Supresión**: `// @allow(<lint>)` en la línea inmediatamente
anterior al stmt. Lookup directo sobre el source raw (no trivia
del lexer). Solo línea anterior, no multi-línea ni inline.

**Decisiones tomadas**:

- 4 lints en MVP (no 6 del roadmap).
- Auto-fix (`--fix`) DIFERIDO: todos los lints emiten sugerencias
  textuales pero no modifican código. `string_concat` es el
  candidato natural a auto-fix.
- Análisis de uses globales (no scope-aware estricto): shadowing
  no se detecta.
- Catálogo cerrado (sin plugins).
- Default warnings, `--deny <name>` promueve a error.

**Tests al cierre 9.z.5**:
- 15 unit + 7 cli_e2e nuevos.
- 1381 unit / 73 cli_e2e / 79 compile_e2e / 3 openapi.
- Clippy `-D warnings` limpio.

**Cap 27 nuevo "`fitz lint` — linter de patrones"** en
`docs/guide.md`: los 4 lints con tabla, CLI, supresión, output
cargo-clippy, integración con CI, limitaciones. Renumeración
cap 27→28 ("Qué sigue").

**Deudas residuales de 9.z.5 (NO bloquean Fase 9.w)**:
- Auto-fix `--fix`.
- Lints adicionales (`shadowing`, `useless_clone` cuando el
  compilador haga análisis de movimientos).
- `unused_variable` scope-aware estricto (shadowing detection).
- Suppression cross-line (`// @allow(name) { ... }` bloque).
- Plugins externos.

#### Cierre formal de Fase 9.z entera

Los 5 sub-pasos de DX (formatter + test + dev + repl + lint)
cerrados en 2 días consecutivos (2026-05-16/17). Total
acumulado: ~3500 LoC nuevas (sin contar tests), 5 capítulos
nuevos en `docs/guide.md` (caps 23-27), renumeración cap "Qué
sigue" del 22 original al 28 actual, dep tree expandido con
`rustyline` + `notify`.

**Próximo norte**: Fase 9.w (stack web first-class:
`@authenticated`, `@ws`, `@cron`, `@background`) — extiende
"HTTP nativo" al resto del stack web. O sub-paso dedicado de
refresh masivo de docs acumulado en `docs/deudas-post-5b.md`
(cap "Package manager" + `architecture.md` + walk completo de
la guía).

### Próximo norte tras 9.z

**Fase 9.w (stack web first-class)** — sumar `@authenticated`,
`@ws`, `@cron`, `@background` como decoradores nativos del
lenguaje. Cierra la apuesta de "HTTP nativo" extendiéndola a todo
el stack web.

---

## Fase 9.w — Stack web first-class: auth, websockets, jobs, ORM 🌐

**Estado: MVP CERRADO (2026-05-21)** — 9.w.1 (Auth nativa) +
9.w.2 (WebSockets tipados) + 9.w.3 (Jobs sin Celery) cerradas
entre 2026-05-20 y 2026-05-21. 9.w.4 (ORM nativo + migraciones)
diferida a **Fase 10** por scope (driver Postgres puro en Fitz
es comparable en tamaño a todo Fase 5-9 combinado). El gap de
DB nativa queda cubierto por interop Python con SQLAlchemy (cap
21 de la guía).

**Objetivo**: extender la filosofía "HTTP es parte del lenguaje"
al resto del stack web típico. Auth, WebSockets, cron jobs,
background tasks y ORM dejan de ser elecciones de biblioteca
(cada framework Python/TS tiene 3-4 opciones por categoría) y
pasan a ser decoradores del lenguaje. **Apuesta de
diferenciación**: lo que en FastAPI son `fastapi-users` +
`python-jose` + `celery` + `apscheduler` + `sqlalchemy` (5 deps +
integración) en Fitz son 4 decorators ya cargados.

**Por qué encaja en Fase 9**: extiende patrones ya establecidos
(`@get`/`@post`/`@middleware`/`@server`). La mecánica de
decoradores (Fase 4.1) lo soporta sin cambios. El runtime axum +
tokio ya cubre lo subyacente para WS y background tasks.

**Cuidado del scope**: 9.w.4 (ORM nativo + migraciones)
probablemente arranca **Fase 10**, no Fase 9. Está acá porque
conceptualmente cierra la apuesta del bloque; se documenta como
link a Fase 10 (ver Visión post-Fase 9 abajo).

### Decisiones pendientes globales del bloque

1. **¿Auth: JWT, sessions cookie-based, o ambos?** Lean: **JWT
   primero** (stateless, simple). Sessions post-MVP.
2. **¿Mensajes WebSocket: solo JSON o también binary?** Lean:
   **JSON en MVP**, binary post-MVP.
3. **¿Cron timezone configurable o solo UTC?** Lean: **default
   UTC + override** via kwarg.
4. **¿Background jobs persistentes (retry tras crash)?** Lean:
   **NO en MVP** — in-memory queue. Persistencia post-MVP
   (requiere storage backend).
5. **¿ORM-first o query-builder-first?** Decisión grande de Fase
   10. Lean: **ambos**, query builder como capa baja, ORM como
   capa alta opcional.

### Sub-pasos

#### 9.w.1 — `@authenticated` / `@admin` — auth nativo (CERRADA 2026-05-20)

**Estado: CERRADA**. El MVP entero de auth nativa está implementado
en intérprete y codegen, con paridad bit-a-bit, OpenAPI auto-
documentado, ejemplo runnable end-to-end y cap dedicado en la guía.
Roadmap original 100% cubierto; deuda residual derivada documentada
abajo (no bloquea uso real).

**Decisiones tomadas al arrancar** (lean original confirmado):

- JWT en el core via built-ins `jwt.encode`/`jwt.decode` (HS256
  default, HS384/HS512 opcional). Asimétricos (RS256/ES256) post-MVP.
- Argon2id para password hashing (recomendación OWASP).
- `@auth_provider` único global; múltiples scoped post-MVP.
- `@admin` shorthand de `@authenticated` + check `user.role ==
  "admin"`; el checker exige `role: Str` en el `User` retornado.

**Sub-pasos cerrados (6 commits, 2026-05-20)**:

- **9.w.1.a** — Checker valida los 3 decorators estáticamente.
  Pre-scan `collect_auth_provider` (signature `fn(Map<Str,Str>)
  -> Result<T-nominal>`, singleton); `check_auth_decorators` por
  `Stmt::FnDef` (provider registrado + handler HTTP +
  `User` param compatible; `@admin` exige `role: Str` no
  nullable). 16 unit tests en `types::tests::auth_*`/`admin_*`.
- **9.w.1.b** — Built-ins `jwt`/`hash` como `Value::Module`
  pre-registrados en `register_builtins`. `jwt.encode(payload:
  Map<Str,Str>, secret: Str, alg: Str?) -> Str`,
  `jwt.decode(token, secret, alg?) -> Result<Map<Str,Str>>`,
  `hash.password(plain: Str) -> Str` (Argon2id PHC string),
  `hash.verify(plain: Str, hashed: Str) -> Bool`. Deps
  `jsonwebtoken = "9"` + `argon2 = "0.5"` (feature `std`) +
  `rand_core = "0.6"` (feature `getrandom` para `OsRng`).
  Checker tipa `jwt`/`hash` como `Any` (deuda de `Type::Function`
  sin opcionales). LSP `scope_level_completions` lista como
  `MODULE` + `after_dot_completions` shortcut por nombre. 16
  unit tests.
- **9.w.1.c** — Runtime auth en `fitz run`. `AuthSpec`
  enum + `AuthProviderHandle` + `RouteSpec.auth` +
  `HttpRegistry.auth_provider` en `src/http.rs`. Wrapper en
  `handle_task` después de middlewares y antes de body parsing:
  construye `Map<Str,Str>` de headers, invoca al provider (con
  `.await` si async), match `Result<User>` → 401/200 o 403
  (admin). `register_auth_provider` + `collect_route_auth` +
  branch `@auth_provider` en `process_decorator`. Orden
  requerido: provider antes que handlers que lo usan (runtime
  error con sugerencia). 9 unit E2E vía `Router::oneshot`.
- **9.w.1.d** — Codegen `fitz build`. `program_uses_auth`
  detector + `cargo_toml_for` suma deps cuando aplica.
  `emit_auth_prelude` con helpers `__fitz_jwt_encode/decode`/
  `__fitz_hash_password/verify`. Dispatch en `gen_call` para
  `jwt.encode/decode/hash.password/verify` (validación de aridad
  y tipos; payload restringido a `Map<Str, Str>` strict en MVP
  — heterogéneos requieren `__FitzValue` integration post-MVP).
  `HandlerSig` suma `auth + auth_user_param_name`;
  `emit_auth_check` entre middleware chain y param coercions
  (espejo del intérprete); `emit_axum_extractors` agarra
  `HeaderMap` cuando hay auth; param coercions skipea el user
  (ya bindeado por emit_auth_check). 2 tests compile_e2e (CLI
  puro con jwt+hash + flujo HTTP end-to-end con login/me/admin).
- **9.w.1.e** — OpenAPI security scheme.
  `OpenApiRouteInfo.auth: AuthSpec`; propagado en
  `route_info_from_spec` (runtime) y `pseudo_routes_from_ast`
  (codegen — body_param excluye el user param).
  `components.securitySchemes.bearerAuth` (type=http,
  scheme=bearer, bearerFormat=JWT, description) sólo cuando hay
  routes con auth. `security: [{bearerAuth: []}]` por handler
  protegido (handlers públicos sin `security`).
  `build_responses_with_auth` suma 401 (auth) y 403 (admin) con
  shape `{"error": Str}`. 5 unit tests del schema.
- **9.w.1.f** — Cap 28 nuevo "Auth nativa" en `docs/guide.md`
  (renumeración 28→29 "Qué sigue") + ejemplo runnable
  `examples/guide/28-auth.fitz` con login/me/admin completos en
  ~100 líneas + suma a `GUIDE_EXAMPLES_COMPILE` smoke + README
  emphasis del diferencial (tabla feature + footnote ♦ + bullet
  en "Estado del proyecto" + bullet en "Qué funciona hoy") +
  cierre formal (CHANGELOG v0.9.21, esta entrada, deudas).

**Decisiones técnicas del MVP** (más allá del lean original):

- `Map<Str, Str>` strict para el payload de `jwt.encode` y
  return de `jwt.decode`. Heterogéneos (numbers/bools/nested)
  requieren `__FitzValue` en codegen — deuda post-MVP.
- `hash.verify` devuelve `Bool` (no `Result<Bool>`); hash
  malformado → `false` por seguridad.
- `jwt.decode` siempre devuelve `Result<Map>` — token
  malformado/signature inválida/expirado son runtime events
  esperables.
- Provider order required: `@auth_provider` antes que cualquier
  handler `@authenticated`/`@admin` (limitación del pass-único
  del codegen registrando types/fns top-down).
- Handler protegido en MVP NO admite body separado del user —
  el param "leftover" debe ser exactamente uno. Workaround
  documentado (headers o split handlers).
- Provider validador del user param via regla "leftover" (el
  param que no es path/query/header): NO requiere magic name
  como `user`, el handler nombra como quiera.

**Deuda residual derivada de 9.w.1** (no bloquea uso real; abre
items para 9.w iteración 2 post-Fase 10):

- **Sessions cookie-based** como alternativa a JWT (requiere
  session store server-side, DB nativa).
- **RBAC con múltiples roles personalizados** (más allá de
  `@authenticated`/`@admin`; modelo de permisos pluggable).
- **Token refresh / revocación** server-side (blacklist con DB
  nativa).
- **Asimétricos JWT** (RS256/ES256 con par de llaves PEM; rotación).
- **Provider request-aware** más allá de los headers (body,
  método HTTP).
- **Heterogéneos en `jwt.encode/decode`** (claims con tipos
  mixtos; requiere `__FitzValue` integration en codegen).
- **OpenAPI 401/403 ya emitidos** pero descripciones genéricas;
  refinable con doc-strings sobre el provider/handlers.

**Sintaxis OpenAPI cumplida**: `securitySchemes.bearerAuth` +
`security` por handler + 401/403 en responses — todo
auto-generado, sin escribir specs OpenAPI a mano.

#### 9.w.2 — `@ws("/chat")` — WebSockets tipados (CERRADA 2026-05-21)

**Segundo sub-paso de Fase 9.w (stack web first-class).**
`@ws("/path")` sobre `async fn` + `WsConn<T>` con métodos
`recv`/`send`/`broadcast`/`close` montan un servidor de
WebSockets tipado end-to-end. **Cinco diferenciales** que
vuelven a Fitz único en este espacio: marshaling JSON
automático, AsyncAPI 3.0 auto-generado, heartbeat built-in,
auth integrada en el handshake, codegen con paridad bit-a-bit.

**Sub-pasos (6 commits)**:

- **9.w.2.a** — Checker estático. `Type::WsConn(Box<Type>)`
  variant, `resolve_type_expr` para `WsConn<T>` aridad 1,
  `infer_wsconn_method` con signatures paramétricas:
  `recv() -> Result<T>` (T del receptor),
  `send(T) -> Result<Null>`,
  `broadcast(T) -> Result<Null>`,
  `close() -> Result<Null>`.
  `check_ws_handler` valida shape del handler en compile-time:
  async fn, primer param `WsConn<T>` resoluble, return `Null`,
  compatibilidad con `@authenticated`/`@admin` (segundo param
  `User`). 14 unit tests.
- **9.w.2.b** — Value runtime + evaluator. `WsConnHandle`,
  `WsBroadcasterTrait`, `WsReadStreamTrait`, `WsOutMessage`
  (Text/Close), `Value::WsConn(Arc<WsConnHandle>)` con
  manual Debug impl. `register_ws_route` en evaluator paralelo
  a `register_http_route`; `process_decorator` branch para
  `@ws`; `dispatch_method` arms para `(Value::WsConn, _)` con
  `ws_conn_recv/send/broadcast/close`. `ws_conn_recv` usa
  `coerce_to_annotation` (heredado de 8.4.3) para Map →
  Instance cuando T es nominal — el frame JSON deserializa al
  `type` declarado.
- **9.w.2.c** — Runtime HTTP. `WsBroadcaster` con
  `parking_lot::Mutex<HashMap<endpoint, Vec<(conn_id,
  outbox_tx)>>>` + `AtomicU64` next_id (pool de conexiones por
  endpoint). `WsReadStreamImpl` envuelve `SplitStream<WebSocket>`
  filtrando ping/pong/binary (deuda: binary). `RouteSpec` suma
  `is_ws/ws_conn_param_name/ws_msg_type`.
  `HttpRegistry.ws_broadcaster: Arc<WsBroadcaster>`.
  `build_ws_method_router` emite axum GET handler con
  `WebSocketUpgrade` extractor + **auth pre-upgrade** (devuelve
  401/403 vía HTTP Response ANTES de `ws.on_upgrade` — menos
  attack surface). `build_ws_conn` signature
  `(socket, endpoint, broadcaster, msg_type, env,
  heartbeat_secs) -> (Value, u64, JoinHandle)`: spawnea writer
  task (mpsc::UnboundedReceiver → sink, separa contención de
  send/broadcast operations) + opcional heartbeat task. axum
  0.8 con feature `ws` + `futures-util` (default-features =
  false, features ["std"]) + dev-dep `tokio-tungstenite = "0.29"`
  para E2E. **Decisión: outbox por conn unbounded** (deuda:
  backpressure).
- **9.w.2.d** — AsyncAPI 3.0 schema (`src/asyncapi.rs` nuevo,
  ~350 LoC). Spec hermana de OpenAPI 3.1 para event-driven APIs.
  `AsyncApiChannelInfo` struct + `channels_from_registry`
  (runtime) + `pseudo_channels_from_ast` (codegen).
  `generate_asyncapi_with_version` emite: channels (uno por
  endpoint `@ws`), operations receive/send por channel
  (`receive_<endpoint>` + `send_<endpoint>`),
  `components.securitySchemes.bearerAuth` cuando hay auth.
  `BTreeMap` para orden determinístico (paralelo a
  `serde_json::preserve_order` de OpenAPI). `build_router_with_asyncapi`
  registra `/asyncapi.json`. En codegen, `auto_asyncapi` gate
  emite `__FITZ_ASYNCAPI_SCHEMA` const + handler
  `__serve_asyncapi_json` + route. 8 unit tests. Consumible por
  AsyncAPI Studio (asyncapi.com/studio) + generadores de
  clientes JS/TS/Python/Java estándar.
- **9.w.2.e** — Heartbeat ping/pong automático.
  `WsOutMessage::Ping` agregado al enum (paralelo a Text/Close)
  + `ServerConfig.ws_heartbeat_secs: u64` (default 30s, el más
  bajo de los proxies comunes: Nginx 60s, Cloudflare ~100s, AWS
  ALB 60s). Parsing de `@server(ws_heartbeat_secs=N)` con
  validación (`Int` literal, no negativo; 0 desactiva
  heartbeat). Si N > 0, `build_ws_conn` spawnea
  `tokio::time::interval(N segundos)` que envía Ping frames
  por el outbox; si el cliente no responde Pong, el sink falla
  en el próximo write y el writer task termina limpio (no
  requiere tracking explícito de Pong — tokio-tungstenite del
  cliente auto-responde, si está muerto el write falla).
  `CodegenCtx.ws_heartbeat_secs` capturado ANTES de emitir WS
  wrappers (timing crítico: gen_ws_handler_wrapper corre antes
  de gen_http_main). 6 unit tests.
- **9.w.2.f** — Cap 29 nuevo "WebSockets tipados" en
  `docs/guide.md` (renumeración 29→30 "Qué sigue") + ejemplo
  runnable `examples/guide/29-ws.fitz` (servidor de chat
  completo con login HTTP + JWT + `@authenticated @ws("/chat")`
  + broadcast multi-client + `@server(43929,
  ws_heartbeat_secs=30)`, <100 líneas) + README emphasis con
  los 5 diferenciales en la tabla feature comparison + footnote
  dedicado + bullets en "Estado del proyecto" y "Qué funciona
  hoy". Suma a `GUIDE_EXAMPLES_COMPILE` (smoke compile_e2e).
  Validado bit-a-bit `fitz run` ↔ `fitz build` con curl +
  wscat (2 clientes concurrentes haciendo broadcast del JSON
  tipado `{"user":"Ada","text":"hola"}` + heartbeat
  funcionando).

**Decisiones técnicas tomadas durante 9.w.2**:

- **`Arc<HttpRegistry>` compartido** vs registry global por hilo:
  `Arc` simple (mismo modelo que F17 cerrada). El broadcaster
  vive adentro del registry; las conns lo clonan vía `Arc::clone`.
- **`tokio::sync::Mutex` en `WsConnHandle.rx`** (no
  `parking_lot::Mutex`): solo `tokio::sync::MutexGuard` es Send
  across `.await`. Recv() necesita esto porque bloquea hasta el
  próximo frame.
- **`parking_lot::Mutex` en `WsBroadcaster.conns`**: no
  cruza await points, performance > tokio::sync (no parking, no
  poisoning).
- **Manual Clone impl para `__FitzWsConn<T>`** en codegen: sin
  `T: Clone` bound porque los fields del struct son Arcs/
  atomics/strings/PhantomData; el derive default falla.
- **Broadcast incluye al sender** (convención
  Socket.IO/Phoenix): broadcast() es a TODOS los conns vivos
  del endpoint, incluido el que envió. Filtrar al sender es
  trivial del lado del cliente con un message_id; del lado del
  server consume estado por conn.
- **Auth pre-upgrade vs post-upgrade**: PRE. El handshake
  HTTP/1.1 Upgrade pasa por el auth wrapper igual que un
  `@get`; si devuelve Err, respondemos 401/403 sin llamar
  `ws.on_upgrade`. Menos attack surface (no se abre el socket
  para tokens inválidos), menos recursos consumidos.
- **`@server(ws_heartbeat_secs=0)`** desactiva heartbeat sin
  romper la decoración (vs error). Útil para deploys sin
  proxies idle-killers.

**Por qué importa**:

- **Marshaling JSON automático**: declarás `WsConn<ChatMsg>` y
  cada frame text se serializa/deserializa al `type` sin
  `json.loads` + Pydantic / `JSON.parse` + Zod manual. El mismo
  trait (`__ToFitzJson`/`__FromFitzJson`) que sirve HTTP cubre
  WS — coherencia interna del stack.
- **AsyncAPI auto-generado**: el schema sale del código fuente
  (vs Socket.IO/Phoenix/SignalR/FastAPI WebSocket donde vive
  manual en un README). Tooling estándar consume directo.
- **Heartbeat built-in**: 1 kwarg y listo. No hay que escribir
  `setInterval` + `ws.ping()` manual.
- **Auth integrada**: `@authenticated`/`@admin` apilados sobre
  `@ws` validan bearer ANTES del HTTP upgrade — 401/403 sin
  abrir el socket.
- **Codegen con paridad**: el flow WS funciona idéntico en
  intérprete y binario nativo. Cero `cargo add
  tokio-tungstenite` o `pip install websockets`.
- **Ningún otro lenguaje hoy combina** WS tipados con AsyncAPI
  auto-generado del código fuente, heartbeat built-in y auth
  integrada en el handshake.

**Deuda residual derivada de 9.w.2** (no bloquea uso real):

- ~~**Binary frames** (`Vec<u8>` como payload).~~ ✓ CERRADO 2026-05-23
  (v0.9.34) — `WsConn<Bytes>` con paridad bit-a-bit `fitz run` ↔
  `fitz build`. Un endpoint es text-only XOR binary-only según el
  T declarado; mismatch entre frame y T → `Err` con mensaje claro.
  AsyncAPI 3.0 emite `contentType: application/octet-stream` +
  `payload: { type: "string", format: "binary" }`. Detalle en
  `CHANGELOG.md` v0.9.34. Endpoints mixtos (texto + binary en el
  mismo socket) quedan como sub-paso futuro si aparece presión.
- ~~**AsyncAPI UI** equivalente al `/docs` de OpenAPI (Scalar).~~ ✓ CERRADO 2026-05-23
  (v0.9.35) — `GET /asyncapi` sirve HTML embebido con
  `@asyncapi/react-component` (CDN). Paridad runtime ↔ codegen,
  opt-out con `@server(docs=false)`, override por handler user
  paralelo a `/docs`. Detalle en `CHANGELOG.md` v0.9.35.
- ~~**Tipado bidireccional separado** (`WsConn<In, Out>`).~~ ✓ CERRADO 2026-05-23
  (v0.9.38) — `Type::WsConn { recv, send }` con dos type params.
  `WsConn<T>` (aridad 1) sigue funcionando como antes (compat).
  Checker valida tipos por dirección; AsyncAPI emite messages
  separados (msg_in/msg_out) cuando son asimétricos. Detalle en
  `CHANGELOG.md` v0.9.38.
- **Reconnect con state replay** del lado del server. Hoy si el
  cliente se reconecta, no replicamos los frames perdidos.
  Requiere persistencia (Fase 10).
- **Rooms / channels dentro de un endpoint**. Hoy `broadcast()`
  manda a TODOS los clientes del endpoint. Sub-canales (rooms
  Socket.IO, topics Phoenix) son deuda visible.
- **Backpressure explícito**. Hoy el outbox por conn es
  unbounded. Bounded channels + drop policy es deuda residual.

**Próximo norte**: resto de Fase 9.w — 9.w.3 (`@cron` +
`@background` — jobs sin Celery) y 9.w.4 (ORM nativo +
migraciones, escala a Fase 10).

#### 9.w.3 — `@cron` + `@background` + `spawn` — jobs sin Celery (CERRADA 2026-05-21)

**Tercer sub-paso de Fase 9.w (stack web first-class).** Tres
piezas nativas del lenguaje montan jobs sin broker externo:
**`@cron("expr")`** para tareas periódicas, **`@background`** como
marcador opt-in para autorizar el callsite, y **`spawn(fn_call)`**
fire-and-forget que devuelve `Future<T>` tipado. Sin Celery, sin
Redis, sin systemd timers — todo en el mismo binario con paridad
bit-a-bit `fitz run` ↔ `fitz build`.

**Sub-pasos (4 commits)**:

- **9.w.3.a** — Checker estático. `CheckCtx.background_fns:
  HashSet<String>` poblado por `collect_background_fns` antes del
  walk (paralelo a `collect_auth_provider`). `check_cron_decorator`
  valida shape (arg Str, sin params, return Null/Result/Future,
  conflictos). `check_background_decorator` valida sin args/
  kwargs + conflictos. `synthesize_expr` para `Expr::Call`
  intercepta cuando callee es Ident "spawn" y binding no fue
  shadowed; refina ret type a `Future<T>` con T del target. LSP
  completion list `spawn` con detail `fn(fn_call) -> Future<T>
  // requiere @background`. 17 unit tests.

- **9.w.3.b** — Runtime intérprete. Nuevo módulo
  `src/cron_jobs.rs` con `CronJob` (handler + Schedule parseado)
  + `CronRegistry` (paralelo a `HttpRegistry`; vive adentro como
  `cron_registry: Arc<CronRegistry>` para reusar lifecycle entre
  HTTP server y cron-only) + `spawn_cron_scheduler` (un
  `tokio::spawn` por job con loop `sleep_until → invoke`)
  + `run_scheduler_only` (cron-only mode con multi_thread
  runtime + ctrl_c). `process_decorator` branches para `@cron`
  (parsea expression via crate `cron`, registra job) y
  `@background` (no-op runtime — solo marcador del checker).
  `eval_call` intercepta `spawn(fn_call)` ANTES de evaluar args
  para capturar el AST del inner call. `eval_spawn_call`
  resuelve el handler en el env, evalúa args, hace
  `tokio::spawn(invoke)` con await del Future si target es
  async, envuelve el JoinHandle en `Value::Future`. Cron-only
  mode en `main.rs`: cuando NO hay rutas HTTP pero SÍ jobs
  `@cron`, llama `cron_jobs::run_scheduler_only` que bloquea
  hasta Ctrl+C. **Fix bug preexistente**: handlers `async fn`
  HTTP en intérprete retornaban "Future pendiente no es
  serializable" porque `handle_task` nunca awaiteaba el Future
  — solo afectaba `fitz run` (codegen lo hacía bien). Helper
  `await_if_future` en `http.rs` para extraer el Value final.
  Normalización 5→6 fields del cron expression: si el usuario
  provee Unix clásico (5 fields), el runtime prependa `"0 "`
  (segundo 0). Deps nuevas: `cron = "0.12"` y `chrono = "0.4"`
  no-opcionales. 8 unit tests.

- **9.w.3.c** — Codegen `fitz build`. Cargo.toml condicional
  suma `cron`/`chrono` cuando `uses_jobs = true`. Tokio con
  feature `signal` adicional en cron-only mode (para
  `signal::ctrl_c()` que mantiene el proceso vivo). Multi_thread
  flavor por default cuando hay jobs. Preludio
  `__fitz_run_cron_job(name, schedule, handler)` análogo al
  intérprete + helper `__fitz_normalize_cron`.
  `PartitionedProgram.cron_fns` paralelo a `http_fns`/`ws_fns`
  — una fn con `@cron` aparece en BOTH `cron_fns` (para emitir
  el spawn) y `top_fns` (para emitir la fn invocable). `gen_main`
  (CLI) y `gen_http_main` ambos invocan `emit_cron_job_spawns()`
  que itera `ctx.cron_jobs_info` y emite por job:
  ```
  tokio::spawn(__fitz_run_cron_job(
      "name".to_string(),
      cron::Schedule::from_str(&__fitz_normalize_cron("expr"))?,
      || async { name().await; },
  ));
  ```
  CLI cron-only mode añade `signal::ctrl_c().await` al final
  del main. HTTP + cron arranca el scheduler ANTES de
  `axum::serve`. `spawn(fn_call)` dispatch en `gen_call` solo
  dispara cuando `spawn` no fue shadowed; emite
  `tokio::spawn(async move { target(args...).await })` con
  `.await` solo si target es async; envuelve el JoinHandle en
  `Box::pin(async move { jh.await.unwrap() })` para case con
  `Pin<Box<dyn Future>>` del codegen. 7 unit tests.

- **9.w.3.d** — Cap 30 nuevo "Jobs sin Celery" en
  `docs/guide.md` (renumeración 30→31 "Qué sigue") + ejemplo
  runnable `examples/guide/30-cron-background.fitz` (URL
  shortener con `type Link`, HTTP + cron stats cada 5 seg +
  `spawn(track_click)` de tracking async sin bloquear la
  response, <100 LoC) + README emphasis con los 5 diferenciales
  en tabla feature comparison + footnote dedicado ♠ + bullets en
  "Estado del proyecto" y "Qué funciona hoy". Suma a
  `GUIDE_EXAMPLES_COMPILE` (smoke compile_e2e).

**Decisiones técnicas del MVP** (no en roadmap original):

- **Cron-only mode vivo bloqueante** (vs run-once o flag opt-in):
  modo systemd-friendly drop-in. Confirmado con el autor.
- **`@cron` acepta sync y async** (vs solo async): ergonomía
  consistente con el resto del lenguaje. Confirmado con el autor.
- **`@background` como marcador opt-in** (vs cualquier fn
  spawneable): evita usos accidentales sobre fns regulares cuyo
  retorno el caller espera consumir.
- **`spawn(...)` exige call literal a fn `@background`** (vs var
  o expression compuesta): permite refinamiento estático del ret
  type y validación clara en compile-time.
- **Crate `cron = "0.12"`** (vs parser propio o `tokio-cron-
  scheduler`): liviano, audit history limpio, soporta 5/6/7
  fields. `tokio-cron-scheduler` arrastraba más deps + concept
  de "job ID" que no necesitamos en MVP.
- **Normalización 5→6 fields automática**: el crate `cron` exige
  6+ fields; nosotros prependeamos `"0 "` para preservar la UX
  familiar del Unix cron sin reescribir la sintaxis.
- **JoinHandle envuelto en `Value::Future`/`Pin<Box<dyn Future>>`**:
  unifica la API con `Future<T>` Fitz existente — descartar el
  Future deja la task detached (fire-and-forget natural).

**Por qué importa** (resumen del cap 30 de la guía):

- **Sin broker externo**: para 90% de servicios reales (tareas
  de mantenimiento, scripts periódicos, fire-and-forget de
  notificaciones), los jobs en memoria del proceso son
  suficientes. Persistencia entre restarts llega con Fase 10 +
  DB nativa, sin cambiar la sintaxis.
- **Checker estático**: validación en compile-time del callsite
  `spawn(...)` (target con `@background` Y refinamiento del ret
  type) vs `tokio::spawn` sin marcador, `asyncio.create_task`
  sin tipos, Celery con string-based task names.
- **Paridad bit-a-bit**: el flow corre idéntico en intérprete
  (rapid dev) y binario nativo (deploy a prod).
- **Cero deps externas**: `cron` + `chrono` van en el binario
  `fitz`. No hay `pip install celery`, `npm install bull`,
  `cargo add tokio-cron-scheduler`.
- **Ningún otro lenguaje** combina cron + background workers +
  spawn tipado en el core sin broker externo y con paridad
  intérprete↔binario.

**Deuda residual derivada de 9.w.3** (NO bloquea uso real; abre
items para iteración 2 post-Fase 10):

- Persistencia de jobs entre restarts (requiere DB nativa).
- Visibility de jobs (panel admin con runs, stats, retries).
- Retry con backoff exponencial cuando un job falla.
- Coordinación entre múltiples instancias (locks distribuidos
  para que un cron solo corra en un nodo).
- `spawn` con coordinación múltiple (Promise.all style requiere
  agregación manual con vectores de futures).
- Cron timezone configurable (hoy `chrono::Utc::now()`).

**Próximo norte**: cierre formal de Fase 9.w MVP entera (ver
sección de abajo).

### Cierre formal de Fase 9.w MVP entera (2026-05-21)

**Las 3 sub-fases del MVP de Stack web first-class están
cerradas** (9.w.1 Auth + 9.w.2 WebSockets + 9.w.3 Jobs).
9.w.4 (ORM nativo + migraciones) queda diferida a **Fase 10**
con justificación técnica concreta.

**Total acumulado al cierre de 9.w MVP**:
- **2156 unit tests** sin feature (+33 de 9.w.1 + 14 de 9.w.2.a
  + N de 9.w.2.b-e + 17 de 9.w.3.a + 8 de 9.w.3.b + 7 de 9.w.3.c
  = ~80 unit tests nuevos en el bloque).
- **90 LSP unit tests** con `--features lsp` (incluye completion
  de `jwt`/`hash`/`WsConn`/`spawn`).
- **76 cli_e2e + 3 openapi**.
- **255 compile_e2e** con smoke ejemplos guía (incluye
  `28-auth.fitz`, `29-ws.fitz`, `30-cron-background.fitz`).
- Clippy `-D warnings` limpio.
- 3 caps nuevos en `docs/guide.md` (28 Auth + 29 WS + 30 Jobs)
  + 3 ejemplos runnable end-to-end.

**Diferenciales del bloque** (validados con caps y ejemplos
runnable):
1. **Auth como decoradores del lenguaje** (`@auth_provider` +
   `@authenticated` + `@admin`) con `jwt`/`hash` built-ins —
   vs FastAPI (5 deps + reflection), Spring `@PreAuthorize`
   (reflection runtime), ASP.NET `[Authorize]` (framework +
   reflection).
2. **WebSockets tipados** (`@ws("/path")` + `WsConn<T>`) con
   marshaling JSON automático + AsyncAPI 3.0 auto-generado +
   heartbeat built-in + auth integrada en el handshake — vs
   Socket.IO (sin schema), Phoenix Channels (solo Elixir),
   SignalR (solo C#), FastAPI WebSocket (Pydantic + schema
   manual).
3. **Jobs sin Celery** (`@cron` + `@background` + `spawn`)
   sin broker externo (jobs en memoria del proceso), checker
   estático del callsite `spawn(...)`, cron-only mode
   systemd-friendly — vs Celery+Redis (Python con broker
   externo), Bull/BullMQ (Node con Redis), Spring
   `@Scheduled` (reflection).

**Ningún otro lenguaje hoy junta** auth + JWT/Argon2 +
WebSockets tipados + AsyncAPI auto + cron + spawn tipado en
el core del compilador, sin broker externo, con paridad
bit-a-bit intérprete↔binario nativo, cero deps externas para
features intrínsecas.

**Por qué 9.w.4 (ORM nativo) escala a Fase 10**:

El driver Postgres puro en Fitz es un proyecto del tamaño de
todo Fase 5-9 combinado. Implementar el protocolo binario de
Postgres desde cero (handshake + autenticación SCRAM-SHA-256
+ prepared statements + tipos OID ~40 + cursors + transacciones
+ COPY + LISTEN/NOTIFY + connection pooling + retry/reconnect)
sin via libpq es comparable a `tokio-postgres` o `sqlx` que
llevaron años de desarrollo. Encima va combinado con ORM
declarativo (`@table`/`@primary`/`@unique`/relaciones lazy vs
eager + query builder tipado) + migraciones autogeneradas
(`fitz db diff`/`migrate`) + decisiones de diseño abiertas
(Postgres-first vs multi-DB, async-first vs sync-first, pool
config, migraciones reversibles).

**Gap cubierto por interop Python**: cap 21 de la guía
documenta el uso de SQLAlchemy desde Fitz (`fitz py-types`
auto-mapea modelos SQLA a `type` Fitz, ejemplo CRUD
runnable). Hoy podés correr un proyecto real con DB en Fitz
sin esperar a Fase 10.

**Decisión**: Fase 10 arranca cuando aparezca un proyecto real
en Fitz que choque con las limitaciones de interop Python con
SQLAlchemy (típicamente performance, missing features de
queries complejas, o ergonomía del round-trip Map/Instance).
Entonces tenemos caso de uso concreto que guía las decisiones
de diseño.

**Sub-paso transversal pendiente post-9.w**: cierre formal de
Fase 9 entera (cuando Fase 9.w.4 también cierre — diferido
junto con Fase 10) — esto se hará al cerrar Fase 10 con un
commit de "Fase 9 entera CERRADA" que cubra el balance final
de Fase 9 (LSP + PM + DX + Stack web).

#### 9.w.4 — ORM nativo + migraciones autogeneradas (LINK A FASE 10)

**Probablemente arranca Fase 10**, citado acá porque cierra la
apuesta del bloque conceptualmente.

- **`type` Fitz con metadata DB**:
  ```fitz
  @table("users")
  type User {
      @primary id: Int
      @unique email: Str
      name: Str
      created_at: DateTime = now()
  }
  ```
- **API tipada**: `User.find_by(id=1)`,
  `User.where(name="Ada").all()`, `User.create(email=...)`,
  `user.save()`.
- **Migraciones autogeneradas**: `fitz db diff` compara `type`
  actual vs schema en DB, emite SQL de migración.
  `fitz db migrate` aplica.
- **Driver Postgres en Fitz puro** (no via libpq) — Fase 10.
  Hasta entonces, interop Python con SQLAlchemy cubre el gap
  (cap 21 de la guía).
- **Decisiones a tomar al arrancar Fase 10**:
  - ¿Postgres-first o multi-DB? Lean: **Postgres-first**,
    MySQL/SQLite vía abstracción opt-in.
  - ¿Async-first o sync-first? Lean: **async-first** (consistente
    con Fase 6).
  - ¿Connection pooling built-in?
  - ¿Migraciones reversibles?
- Ver "Visión post-Fase 9" abajo para Fase 10 completa.

#### 9.w iteración 2 (post-Fase 10) — endurecimiento para servicios críticos

**Estado: PENDIENTE.** Diseño detallado se hace al arrancar, no
ahora — diseñarlo hoy sería doble especulación (Fase 10 también
es especulativa y su storage backend afecta directo cómo se hace
persistencia).

**Pre-requisito**: storage backend nativo de Fase 10 cerrado
(driver Postgres + pool de conexiones + ORM básico). Sin DB
nativa, sessions persistentes y jobs persistentes seguirían
dependiendo de SQLAlchemy/Redis externos — contradice la
filosofía "stack en el lenguaje" de 9.w.

**Items comprometidos como post-MVP en 9.w** (ya marcados en sus
sub-pasos correspondientes — esta sección es índice findable, no
re-diseño):

- **Sessions cookie-based** como alternativa a JWT — ver 9.w.1.
  Requiere DB nativa para session store server-side.
- **RBAC con roles custom** más allá de `@authenticated`/`@admin`
  — ver 9.w.1. Modelo de permisos pluggable.
- **Persistencia de jobs** sobre DB nativa (retry tras crash,
  visibility de jobs) — ver 9.w.3. Reemplaza el queue in-memory
  del MVP.
- **WebSocket binary messages** (no solo JSON) — ver decisiones
  globales del bloque.

**Cuándo arrancar**: cuando aparezca proyecto real en Fitz que
choque con las limitaciones del MVP (sesión que expire mal, job
que se pierda en deploy, etc.). Sin presión real, no hay rush —
9.w MVP cubre el caso típico de "API CRUD con login + tareas de
mantenimiento" que es el 80% del uso esperado.

**Disparador concreto: avance del curso al M5 (acordado
2026-06-01)**. M5 del curso "Fitz de 0 a experto" (caps C23 async
+ C24 auth + C25 ws + C26 jobs) necesita que ciertas deudas de
9.w cierren antes de ser escrito, para no enseñar funcionalidad
que tiene gaps obvios. Tiers definidos en `docs/curso-plan.md` →
"Tiers pre-M5":

- **T1 — bloquean M5** (cerrar antes de arrancar a escribir
  caps):
  - **9.w.3.iter2 — Persistencia + retry + timezone + catch_up
    de jobs** ✅ **CERRADA 2026-06-02 (v0.11.2)**. 5 sub-pasos
    coordinados (a checker, b runtime tipos, c scheduler + E2E
    Postgres real, d codegen `fitz build` paridad, e cap 30 +
    ejemplo runnable + LSP refresh) + cierre formal f. ~3000
    LoC netas entre `src/types.rs` / `src/cron_jobs.rs` /
    `src/evaluator.rs` / `src/codegen.rs` / `src/lsp.rs` +
    `tests/cron_jobs_real_postgres.rs` + `docs/guide.md` cap 30
    + `examples/guide/30b-cron-persistente.fitz`. Sub-pasos:
    - 9.w.3.iter2.a: checker estático con helpers libres
      `check_job_kwargs` + `check_retry_map` parametrizados por
      allowed-list; valida shape sintáctico (`tz` Str literal,
      `retry` Map literal con keys `max/backoff/initial_secs/
      max_secs`, `catch_up` Bool literal, `store` Ident no-null),
      rechaza desconocidos y duplicados con la lista de
      aceptados. `extract_int_literal` reconoce `Int(N)` y
      `UnaryOp { Neg, Int(N) }`. `@background` acepta `tz`/
      `retry` (no `store`/`catch_up`). +20 unit tests checker.
    - 9.w.3.iter2.b: tipos runtime en `cron_jobs.rs` — `enum
      BackoffKind` (default `Exponential`) con
      `derive(Default)`, `struct RetryConfig` con `Default`
      (max=0 = sin retry) y `delay_for_attempt(attempt)` que
      calcula el backoff (`exponential` shift saturado a 63
      para no overflowear) capeado por `max_secs`. `struct
      CronJobOptions { tz, retry, catch_up, store }` con
      `Default` (UTC/None/false/None). `CronJob` gana los 4
      campos; `CronRegistry::register` toma `CronJobOptions`
      como parámetro final. `evaluator::register_cron_job`
      parsea kwargs del `Decorator` vía
      `parse_cron_job_options` + sub-helpers
      (`parse_retry_kwarg`, `resolve_store_kwarg` que valida
      contra `Value::DbConn` con error claro si tipo no
      compatible). +11 unit cron_jobs + +7 unit evaluator.
    - 9.w.3.iter2.c: scheduler intérprete adaptado. 7 helpers
      SQL `init_storage` (CREATE TABLE IF NOT EXISTS de dos
      tablas + un índice idempotentes) / `upsert_job_row` (ON
      CONFLICT DO UPDATE) / `record_run_start` (RETURNING id,
      acepta i64 o Text del BIGSERIAL) / `record_run_finish`
      (UPDATE finished_at=now()) / `update_job_last_run` /
      `read_last_run_at` / `parse_pg_timestamptz` (normaliza
      offset Postgres sin minutos `+00`/`-03`/`+05` → `±DD:00`
      para que `chrono::DateTime::parse_from_rfc3339` matchee).
      `run_cron_job` boot con init + upsert + catch_up
      (`Schedule::after(last)` en `job.tz`); loop tz-aware con
      `Schedule::upcoming(job.tz)` convertido a UTC para el
      sleep; `invoke_with_retry` que persiste cada attempt con
      status `running`→`ok`|`retrying`|`failed`. Schema:
      `fitz_cron_jobs(name PK, schedule, tz, last_run_at,
      last_status, last_error, next_run_at)` + `fitz_cron_runs
      (id BIGSERIAL, job_name, started_at, finished_at, status,
      attempt, error)` + índice `(job_name, started_at DESC)`.
      Helpers SQL marcados `#[doc(hidden)] pub` para uso de
      tests E2E. +6 tests E2E reales `#[ignore]` en
      `tests/cron_jobs_real_postgres.rs` contra Postgres 15
      local del autor (memoria `reference_postgres_local`).
    - 9.w.3.iter2.d: codegen `fitz build` paridad bit-a-bit.
      `CronJobInfo` extendido con `tz_name/retry/catch_up/
      store_var` parseados build-time vía
      `parse_cron_kwargs_into_info` + `parse_cron_retry_map`.
      `program_has_persistent_cron` walka AST buscando `@cron
      (..., store=<Ident>)`; cuando true, fuerza `uses_db=true`
      (suma `chrono-tz` al Cargo.toml generado si
      `uses_date_or_uuid=false` para evitar dupe). Preludio
      dividido en 4 constantes condicionales (`JOBS_COMMON_
      PRELUDE` con trait `__FitzCronReturn` que mapea `()` o
      `Result<(), String>` a uniform `Result<(), String>`,
      siempre emitido; `JOBS_RUN_PRELUDE_SIMPLE` sin field
      `store` + sin SQL helpers cuando no hay persistencia para
      evitar referenciar `__FitzDbConn`; `SQL_HELPERS_PRELUDE`
      con 7 helpers `__fitz_cron_*` paralelos al intérprete +
      `JOBS_RUN_PRELUDE_PERSISTENT` con field `store:
      Option<__FitzDbConn>` + trait polimórfico
      `__FitzCronStoreFrom` que acepta `__FitzDbConn` directo
      Y `Result<__FitzDbConn, String>` con panic claro si era
      `Err` — destraba el patrón canónico `let db = db.connect
      (...).await` top-level sin `?`). `gen_main` reordena:
      stmts del usuario van ANTES de `emit_cron_job_spawns`
      para que bindings top-level estén en scope al
      referenciarlos como `store=db`. `gate de kwargs` del
      partition extendido para aceptar kwargs en `@cron`/
      `@background`. `emit_cron_job_spawns` construye
      `__FitzCronOptions { tz: __tz, retry: ..., catch_up:
      ..., (store: (&db).into_store())? }` con parseo IANA al
      boot vía `.parse::<chrono_tz::Tz>().unwrap_or_else(panic)`.
      `evaluator::resolve_store_kwarg` extendido en paralelo
      para aceptar `Value::Result(Ok(DbConn))` y `Err(msg)`
      con error claro (paridad runtime exacta). Validado
      contra Postgres 15 local: binario nativo con `@cron("*/2
      * * * * *", store=db)` crea las dos tablas, persiste 3
      runs `status='ok' attempt=1` en 6s.
    - 9.w.3.iter2.e: cap 30 de `docs/guide.md` "Jobs sin
      Celery" suma sub-sección **"Persistencia, retry y
      timezone (iter2)"** antes de "Cron-only mode" con shape
      de cada kwarg + defaults + backoffs aceptados + schema
      DDL + queries de visibility manual con `psql` + notas
      sobre el binding `Result<DbConn>` top-level. Limitación
      conocida documentada: `fitz run` cron-only con `store=db`
      tiene bug del runtime tokio del intérprete (workarounds:
      `fitz build` o sumar handler HTTP trivial). Sub-sección
      "Qué no está en el MVP" reescrita: salen los 3 items
      cerrados; entra `@background` con persistencia + retry
      (diferido a iter3). `examples/guide/30b-cron-persistente.
      fitz` nuevo (~50 LoC HTTP+cron con los 4 kwargs
      combinados) sumado al smoke `GUIDE_EXAMPLES_COMPILE`
      (~290 ejemplos verde en ~7 min). `src/lsp.rs`
      descripciones de `@cron`/`@background` actualizadas con
      los kwargs nuevos; grammar TextMate sin cambios. 112
      tests LSP verdes.
    - 9.w.3.iter2.f: cierre formal (CHANGELOG v0.11.2 +
      roadmap actualizado + curso-plan.md marca T1 CERRADO
      + memoria `project_curso_pre_m5_tiers` actualizada +
      `docs/deudas-post-5b.md` nota de cierre + README +
      CLAUDE.md refresh + prompt M5 con cap C26 reescrito
      al iter2 entregado para próxima sesión).

    **Total al cierre v0.11.2**: 2792 unit + 6 E2E real
    Postgres + 1 compile_e2e smoke + 112 LSP. `cargo fmt --all`
    + `cargo clippy --all-targets -- -D warnings` limpios.
    mkdocs build sin warnings nuevos.

    **Decisiones técnicas confirmadas con el autor al arrancar
    (2026-06-01)**: D1 dos tablas (jobs + runs) vs una sola;
    D2 CREATE TABLE IF NOT EXISTS auto al boot vs `fitz db
    migrate` manual; D3 `store=<binding>` kwarg explícito vs
    convention global singleton; D4 Map literal para `retry={...}`
    vs kwargs sueltos; D5 3 backoff kinds desde el día 1
    (exponential/linear/constant) vs solo exponential; D6
    `@background` solo `tz`/`retry` en memoria (persistencia
    diferida a iter3 — los args de `spawn` requieren JSON estable
    + tabla `fitz_bg_jobs` separada); D7 `catch_up=false` default
    (skip); D8 `chrono-tz = "0.10"` no-opcional (ya estaba
    transitive desde Fase 10 — sin nueva dep).

- **T2 — evaluar antes de M5** (cierran caso de uso del cap si
  entran):
  - **9.w.1.iter2 — RBAC custom + token refresh**:
    - **9.w.1.iter2.a CERRADA (2026-06-03, v0.12.4)** — RBAC con
      roles custom más allá de `@authenticated`/`@admin`:
      decorator nuevo `@requires("role")` apilable. Validación
      estática (shape + provider + `role: Str` en User type).
      Runtime intérprete + paridad bit-a-bit codegen. Multi-role
      via apilamiento (OR). Mensaje de 403 cita role actual +
      requeridos. 14 unit tests (9 checker + 5 runtime). Detalle
      técnico completo en `docs/deudas-post-5b.md` →
      "Fase 9.w.1.iter2.a — `@requires` (RBAC custom)".
    - **9.w.1.iter2.b CERRADA (2026-06-03, v0.12.6)** — Token blacklist.
      Módulo built-in `auth` con 3 builtins async sobre Postgres:
      `auth.blacklist(db, jti, expires_at) -> Future<Result<Null>>`,
      `auth.is_blacklisted(db, jti) -> Future<Result<Bool>>`,
      `auth.cleanup_expired(db) -> Future<Result<Int>>`. Tabla
      `fitz_token_blacklist(jti TEXT PRIMARY KEY, expires_at BIGINT
      NOT NULL)` auto-creada con CREATE TABLE IF NOT EXISTS al primer
      call (paralelo a Fase 9.w.3.iter2 cron persistente). Decisiones:
      `expires_at` Unix epoch (BIGINT) para matchear JWT `exp` claim
      sin conversiones; auto-filtro `expires_at > now()` en
      is_blacklisted (tokens vencidos no necesitan seguir bloqueando,
      jwt.decode los rechaza por expirado primero); ON CONFLICT DO
      UPDATE en blacklist (re-blacklisteo del mismo jti actualiza sin
      fallar); server-clock manda (now() en SQL, no en Rust); tabla
      auto-creada idempotente (Postgres serializa con LOCK interno);
      paridad bit-a-bit `fitz run` ↔ `fitz build`. Sub-pasos:
      **b.1** intérprete — 4 helpers `pub` en `src/evaluator.rs`
      (constantes SQL + `ensure_token_blacklist_table`), 3 builtins
      con validación de args, registro paralelo a jwt/hash/log,
      checker con `auth` en scope base, **6 unit + 6 E2E real Postgres**;
      **b.2** codegen — `expr_uses_auth` detecta `auth.X`,
      `emit_auth_prelude` cuando `uses_auth && uses_db` emite 4
      constantes SQL + ensure_table + 3 helpers `__fitz_auth_*` async
      retornando `Result<T, String>`, `gen_call` despacha `auth.X`
      paralelo a `gen_auth_jwt_*`, import cross-module, **1 E2E
      compile test**; **b.3** docs/LSP/cierre — cap 28 sub-sec `auth`
      con API + decisiones + patrón canónico completo (`/auth/logout`
      + `/auth/refresh` + provider con check + `@cron` cleanup) + lo
      que NO está en el MVP (auto-mount, in-memory, refresh tokens
      dedicados), LSP `auth` module + scope_level_completion +
      after-dot con signatures, CHANGELOG/roadmap/deudas/CLAUDE.
      Total al cierre 9.w.1.iter2.b: 2957 unit + 93 cli_e2e + 3
      openapi + 358 compile_e2e + 6 E2E real Postgres `#[ignore]`
      (opt-in con `FITZ_TEST_PG_URL`). Cierra **Fase 9.w.1.iter2
      entera** (.a RBAC custom + .b token blacklist).
  - **9.w.2.iter2 — Rooms + reconnect state replay** (solo si el
    ejemplo del C25 del curso lo exige): sub-canales dentro de un
    endpoint WS, reconnect con state replay (acopla con
    persistencia de T1).

- **T3 — post-M5** (mencionar como deuda visible en los caps, no
  bloquean): coordinación multi-instancia con locks
  distribuidos, `spawn` con coordinación múltiple (Promise.all
  style), sessions cookie-based, JWT asimétricos (RS256/ES256),
  backpressure explícito en WS, heterogéneos en
  `jwt.encode/decode` (Map<Str, Any>).

**Orden de ejecución acordado**:

1. ✅ **CERRADO 2026-06-02 (v0.11.2)** — 9.w.3.iter2 (T1).
2. 9.w.1.iter2 (T2 — RBAC + refresh) — release dedicado tras T1.
3. 9.w.2.iter2 (T2 — rooms + replay) — solo si el ejemplo del
   C25 del curso lo exige; si no, baja a T3.
4. Arrancar M5 del curso.

C23 (Async) no tiene deudas relevantes — sale directo cuando
llegue su turno.

### Próximo norte tras 9.w

**Fase 10** arranca con el stack DB nativo (driver Postgres puro
Fitz + ORM declarativo + migraciones). Cierra la promesa "todo el
stack web en el lenguaje". Ver sección **Fase 10** dedicada abajo
con el diseño completo y los 10 sub-pasos.

---

## Fase 10 — Stack DB nativo + ORM declarativo 🗄️

**Estado: planificada, diseño cerrado el 2026-05-25.** Las 12
decisiones técnicas fundacionales están resueltas (ver sub-sección
"Decisiones de diseño" abajo). Próximo paso: arrancar 10.1.

**Promesa**: cerrar el "stack web completo en el lenguaje" —
backend + DB nativo, sin necesidad de interop con Python/Ruby/Node
para acceder a una base de datos relacional. Después de Fase 10,
los boilerplates 5/6 (que hoy usan SQLAlchemy via interop Python)
pasan a usar la DB nativa, y la interop queda como **opcional**
para el ecosistema Python (numpy, pandas, scikit) en vez de
**obligatoria** para DB.

**Tamaño estimado**: ~6300 LoC nuevas + 1 cap nuevo en la guía
+ 1 ejemplo runnable + refresh de 2 boilerplates. Comparable a
todo Fase 9.w combinada (Auth + WS + Cron = ~3500 LoC) **multiplicado
por dos**. 12-15 sesiones para cerrar la fase entera. Es la fase
más grande comprometida hasta hoy.

### Decisiones de diseño (cerradas 2026-05-25)

Sesión de diseño previa a cualquier código. Las 12 decisiones
fundacionales:

| # | Decisión | Razón |
|---|---|---|
| 1 | **Driver Postgres puro en Fitz** (~3-4k LoC, protocolo wire implementado adentro del binario) | Standalone binary preservado (cero deps de sistema como libpq), paridad bit-a-bit `fitz run` ↔ `fitz build` trivial, alineado con la filosofía HTTP nativo / Auth nativa / Jobs sin Celery. **Rechazado**: wrap libpq via FFI (rompe standalone, complica Docker), wrap crate `tokio-postgres` (Fitz como wrapper en vez de implementación nativa, choca con espíritu de 9.w) |
| 2 | **API toda async** (`db.query(...).await?`) | Postgres es I/O-bound; bloquear thread esperando I/O mata throughput HTTP bajo carga (lección F17). Encaja natural con Fase 6 (async nativo) y 9.w.3 (`spawn`). **Rechazado**: sync con bridge (anti-pattern, F17 cerró exactamente esa deuda), dual API (doble superficie sin ganancia real) |
| 3 | **Pool built-in en el driver** (`connect(url, max_conns=10)`) | First-class, cero ceremonia para 90% del caso real (server HTTP cualquiera lo necesita). ~300 LoC extra. **Rechazado**: sin pool (el user lo re-implementa mal), pool como módulo aparte (doble API, conveniencia perdida) |
| 4 | **ORM con decoradores sobre `type`** (`@table` / `@primary` / `@column`) | Reusa el sistema de tipos del lenguaje, encaja con la filosofía establecida de decoradores (@get/@cron/@authenticated/@auth_provider/@middleware/@ws/@background). El checker valida queries against fields en compile-time. Tipo Fitz canonical, ORM lo extiende con metadata. **Rechazado**: builder/macro (modelo opaco, pierde el sistema de tipos), schema-first (doble fuente de verdad, ceremonia extra) |
| 5 | **Migraciones schema-diff autogenerado** (`fitz db diff`) | Promesa Prisma/Drizzle: cambiá el `type`, te doy la migration SQL lista para revisar y commitear. ~800 LoC extra (introspección via `information_schema` + diff engine). **Rechazado**: Alembic-style (más ceremonia, refactors caros), SQL plano sin tooling (anti-pattern para feature first-class) |
| 6 | **Query builder tipado + raw SQL escape hatch** | API principal builder type-safe (`User.where(fn (u) => u.age > 18).all().await?`), escape hatch para queries complejas (`db.query("SELECT ... ", args)`). ~600 LoC del builder + integration con checker. Best of both — type-safe default + power user escape. **Rechazado**: solo raw SQL (checker no valida queries), solo builder (mata Postgres avanzado como CTEs/window/FTS) |
| 7 | **Transactions closure-scope** (`db.transaction(fn (tx) => ...).await?`) | Closure retorna `Ok` → commit, `Err` → rollback, panic → rollback. Imposible olvidarse de cerrar. Encaja natural con `Result` + `?`. **Rechazado**: begin/commit/rollback explícito (Fitz no tiene try/catch, fácil olvidar rollback en error path), dual API (doble superficie sin ganancia) |
| 8 | **Core Postgres extendido**: JSONB + LISTEN/NOTIFY + arrays + FTS | MVP completo, no diferimos features que Postgres brilla. JSONB para `data: Json` con `->` y `->>` (150 LoC), LISTEN/NOTIFY integrado con `@background` reemplaza Redis pub/sub (200 LoC), arrays `tags: List<Str>` → `text[]` (100 LoC), FTS con `@tsvector` + `.search()` (300 LoC) |
| 9 | **Relaciones: decorators dedicados sobre fields** (`@belongs_to` / `@has_many` / `@has_one` / `@many_to_many`) | Explicit, checker valida en compile-time que la tabla referenciada existe y los tipos cuadran. SQL emitido con FKs automáticos. Estilo Rails ActiveRecord / Django ORM. **Rechazado**: inferencia mágica desde tipo del field (pierde control sobre nombre de FK, cascades, lazy/eager), API explícita sin sintaxis (pierde validación compile-time + ergonomía) |
| 10 | **Cascade vía kwarg del decorator** (`@belongs_to("User", on_delete="cascade", on_update="restrict")`) | Default `"restrict"` seguro (Postgres default). Valores: `"cascade" / "set_null" / "restrict" / "no_action"`. Validado por checker en compile-time. Descubrible via autocomplete LSP. **Rechazado**: decorator dedicado apilable (más verbose, ganancia marginal), solo restrict + edit migration manual (intent desincronizado del modelo) |
| 11 | **Lazy por default + `.include(...)` para eager opt-in** | `post.author().await?` siempre dispara query (lazy, explicit). Para evitar N+1: `Post.where(...).include("author", "tags", "comments.author").all().await?` carga todo con LEFT JOINs en una sola query. Estilo Prisma `include` / Drizzle `with`. **Rechazado**: eager por default (queries enormes, anti-pattern para tablas grandes), sin abstracción (joins manuales en cada query) |
| 12 | **Core relacional**: self-referential + M2M con pivot explícita + soft deletes | Self-ref via `@has_many("Comment", via="parent_id")` para threading (50 LoC validación). M2M con `@many_to_many("Tag", via="PostTag")` donde `PostTag` es un `type` declarado con 2 FKs (200 LoC). Soft deletes via `@soft_delete` agrega `deleted_at: Timestamp?` + filtro automático en queries (150 LoC). **Diferido a Fase 10.x post-MVP**: polymorphic associations (~400 LoC, complica el checker con unión de tipos, pocos casos reales) |

### Shape del código Fitz (target)

Diseño concreto de cómo se ve un programa Fitz usando el stack DB
nativo. Combina relaciones, cascades, soft deletes, JSONB, eager
loading + las features ya existentes (HTTP + Auth + decoradores):

```fitz
from db import connect, transaction

@table("users")
type User {
  @primary id: Int = 0
  name: Str
  email: Str
  data: Json?

  @has_many("Post", via="author_id", on_delete="cascade")
  posts: List<Post> = []

  @has_one("Profile", on_delete="cascade")
  profile: Profile? = null
}

@table("posts")
@soft_delete
type Post {
  @primary id: Int = 0
  title: Str
  body: Str
  tags_raw: List<Str> = []

  @belongs_to("User", on_delete="cascade")
  author_id: Int

  @many_to_many("Tag", via="PostTag")
  tags: List<Tag> = []
}

@table("post_tags")
type PostTag {
  @belongs_to("Post", on_delete="cascade")
  post_id: Int

  @belongs_to("Tag", on_delete="cascade")
  tag_id: Int
}

@table("comments")
type Comment {
  @primary id: Int = 0
  body: Str

  @belongs_to("User", on_delete="cascade")
  author_id: Int

  @belongs_to("Comment", on_delete="cascade")
  parent_id: Int?

  @has_many("Comment", via="parent_id")
  replies: List<Comment> = []
}

@server(3000)
async fn main() => 0

@get("/posts/{id}")
async fn get_post(id: Int) -> Result<Post> {
  let post = Post
    .where(fn (p) => p.id == id)
    .include("author", "tags", "comments.author")
    .first()
    .await?
  return Ok(post)
}

@post("/posts")
async fn create_post(post: Post) -> Result<Post> {
  return db.transaction(fn (tx) => {
    let inserted = tx.insert(post).await?
    db.notify("post_created", inserted.id).await?
    Ok(inserted)
  }).await?
}

@background
async fn index_post(post_id: Int) -> Null {
  let post = Post.where(fn (p) => p.id == post_id).first().await?
  // ... lógica de indexing FTS
}
```

### Sub-pasos (10 sub-fases)

División incremental, cada sub-paso cierra una pieza testeable
end-to-end. La línea de cierre se mantiene en cada sub-paso: tests
unit + E2E + smoke `GUIDE_EXAMPLES_COMPILE` + clippy limpio +
paridad bit-a-bit `fitz run` ↔ `fitz build`.

#### 10.1 — Driver wire protocol mínimo (~1500 LoC)

Primer sub-paso: solo el driver, sin ORM todavía. Habilita
`db.query(sql, args).await?` desde Fitz, devolviendo `List<Row>`
donde `Row` es `Map<Str, Any>` (sin tipado fuerte hasta 10.3).

- **Módulo built-in `db`** registrado en `register_builtins` (sin
  import). `db.connect(url) -> Result<Db>` y `Db.query(sql, args)`
  / `Db.exec(sql, args)`. Parseo de connection string URI estándar
  (`postgres://user:pass@host:port/dbname?sslmode=...`).
- **TCP + framing**: socket tokio + parseo de mensajes Postgres
  (PostgreSQL wire protocol v3.0). Header (1 byte tag + 4 bytes
  length) + body variable. Funciones `read_message` / `write_message`.
- **Startup + auth**: StartupMessage → AuthenticationXxx →
  PasswordMessage. Soporta cleartext (legacy), MD5 (legacy), y
  **SCRAM-SHA-256** (Postgres 14+ default). SASL handshake con
  iteraciones HMAC + signing. ~500 LoC solo de SCRAM.
- **Simple Query Protocol**: Query → CommandComplete / RowDescription
  + DataRow* / ErrorResponse. Soporte para múltiples statements en
  un solo Query (uso interno para `BEGIN`/`COMMIT`).
- **Extended Query Protocol**: Parse → Bind → Execute → Sync.
  Necesario para parametrized queries (`$1`/`$2` placeholders) y
  evita SQL injection. Todas las queries del user pasan por aquí.
- **Tipos OID core en MVP**: Int4 (23), Int8 (20), Float4 (700),
  Float8 (701), Text (25), Varchar (1043), Bool (16), Timestamp
  (1114), Timestamptz (1184), UUID (2950), Bytea (17). Conversión
  bidireccional Postgres binary format ↔ Fitz `Value`. Date/Time/
  JSON/arrays/etc. quedan para 10.5.
- **SSL/TLS**: `sslmode=disable` (default para dev) y `sslmode=require`
  (TLS handshake antes del StartupMessage). Sin cert verification
  en MVP (`verify-ca` / `verify-full` post-MVP).
- **Errors**: `ErrorResponse` Postgres → `Err(Str)` Fitz con
  formato `"<severity>: <message>"`. Códigos SQLSTATE preservados
  en el mensaje cuando sean útiles.
- **Tests**: unit sobre framing + parsing de mensajes + SCRAM
  handshake (vectors RFC 7677). E2E **opcionales bajo feature
  `postgres-tests`** que arranca un Postgres 14+ en docker para
  smoke real (no obligatorio en CI de cada commit, sí en CI de
  release tags).

**Criterio de éxito**: `let db = connect("postgres://...").await?;
let rows = db.query("SELECT 1 AS n", []).await?; print(rows[0]["n"])`
imprime `1`. Connection con SCRAM, query parametrizada, rows
deserializados a `Map<Str, Any>`, paridad `fitz run` ↔ `fitz build`.

#### 10.2 — Pool de conexiones + reconnect + health check (~400 LoC)

Refina 10.1 con pool. API pública sin cambios visibles para el
user — `connect` ahora acepta `max_conns` opcional.

- **`connect(url, max_conns=10)`** devuelve un `Db` que internamente
  tiene un pool de N conexiones. Default `max_conns=10`.
- **Semaphore dispatch**: cada `db.query(...)` espera un permit del
  semaphore, agarra una conn libre, ejecuta, libera. Sin
  contention en queries paralelas hasta `max_conns`.
- **Health check periódico**: thread/task background que cada 30s
  hace `SELECT 1` sobre conns idle, descarta las que fallan.
  Reconnect lazy en el próximo `query`.
- **Reconnect automático**: si una conn del pool muere mid-query
  (red caída), el driver reintenta una vez con otra conn del pool.
  Si la query había hecho `INSERT` y no hay retry seguro (no
  idempotente), error explícito.
- **Tests**: unit sobre el pool (acquire/release, max_conns limit,
  reconnect path), E2E (concurrent queries no se bloquean entre sí).

**Criterio de éxito**: 10 handlers HTTP concurrentes haciendo
queries no se serializan; cada uno agarra su conn del pool.

#### 10.3 — `@table` + `@primary` + `@column` + CRUD básico (~800 LoC)

Primer sub-paso del ORM declarativo. Sin relaciones todavía, solo
CRUD sobre tablas individuales.

- **Decoradores sobre `type`**: el lenguaje extiende decoradores
  para aceptar `@table("posts")` / `@primary` / `@column(name="...")` /
  `@unique` / `@index` sobre el `type` y sobre fields. Hoy
  decoradores solo van sobre `fn`. Requiere extensión del parser
  (decorators antes de `type` y antes de campos) + checker
  (registra metadata en el `TypeId`).
- **Builder type-safe**: `User.where(fn (u) => u.age > 18)
  .order_by(fn (u) => u.name).limit(10).all().await?`. Métodos:
  `.where(fn)` / `.order_by(fn)` / `.limit(n)` / `.offset(n)` /
  `.all()` / `.first()` / `.count()`. El builder NO ejecuta hasta
  `.all()/.first()/.count()` — composable.
- **CRUD operations**: `User.create(data).await?` / `User.where(...)
  .update(data).await?` / `User.where(...).delete().await?` /
  `db.insert(user).await?` (sobre instancia).
- **Translación**: el builder se traduce a SQL parametrizado en
  build time (`fitz build`) o run time (`fitz run`). Cada `fn (u)
  => u.age > 18` es una closure tipada → AST traversal → SQL `WHERE
  age > $1`. Soporta operadores `== / != / > / >= / < / <= / && / ||
  / !`, function calls `like(s, pattern)` / `in_(x, list)` /
  `is_null(x)` / `contains(s, sub)`.
- **Tipos de columna**: mapping `type` Fitz → SQL Postgres. `Int →
  bigint`, `Float → double precision`, `Str → text`, `Bool →
  boolean`, `Null en field nullable → NULL`, `default literal →
  DEFAULT`. Tipos avanzados (JSONB/arrays/dates) llegan en 10.5.
- **Tests**: unit sobre traducción closure → SQL (cubre operadores,
  comparaciones, defaults, NULL), E2E con Postgres real (insert +
  query + update + delete sobre `type User`).

**Criterio de éxito**: el ejemplo del shape arriba sin relaciones
(solo `User` y queries básicas) corre end-to-end contra Postgres
real, paridad `fitz run` ↔ `fitz build`.

**Cierre formal 10.3 (sesiones del 2026-05-25 al 2026-05-26)**:
ejecutada en 4 sub-pasos:
- **10.3.a — Parser + checker para decorators ORM**: AST extendido
  con `decorators` en `Stmt::TypeDef` y `Field`. Parser acepta
  `@x(args)` antes de `type` y antes de cada field. Checker
  procesa `@table`/`@primary`/`@column(name, sql_type)`/`@unique`/
  `@index` y persiste `TableMetadata` por `TypeId` en `TypeEnv`.
  +18 tests (parser + checker).
- **10.3.b1 — Builder `User.all(db)`**: `Value::Type` cachea
  `Option<Box<TableMetadata>>`. Dispatch ORM sobre `Value::Type` con
  metadata. `SELECT col1, col2, ... FROM "table"` con columnas
  explícitas (no `SELECT *`), `@column(name=...)` respetado en SQL
  emitido. Helper `pg_row_to_instance` deserializa rows a
  `Value::Instance`. +5 unit + 1 E2E real (`orm_user_all_db_devuelve_instancias_reales`).
- **10.3.b2 — `.where(closure)` con translator Expr→SQL**:
  `Value::QueryBuilder(Arc<dyn Any>)` opaco para chain. Translator
  cubre BinOp (==/!=/</<=/>/>= +/-/*//and/or), UnaryOp (not/-),
  Field access (con override `@column`), literales primitivos
  (Int/Float/Str/Bool/Null) extraídos como `$N`. Chain
  `.where(f).where(g)` combina con AND. +9 unit + 2 E2E real
  (filtra por age, chain con AND).
- **10.3.b3 — order_by/limit/offset/first/count**: state extendido
  con `order_by_clauses` (DESC via UnaryOp Neg `-u.field`),
  `limit/offset: Option<i64>`. `first(db)` ejecuta con LIMIT 1
  override + devuelve `Result<Instance>` (`Err("no rows")` si vacío).
  `count(db)` emite `SELECT COUNT(*) AS "__count" FROM ... WHERE ...`
  ignorando order/limit/offset (count es agregado). +9 unit + 1
  E2E real combinando todo el subset.
- **10.3.c — insert / update / delete**: `User.insert(db, record)`
  emite `INSERT ... RETURNING *` y devuelve la Instance completa
  (con auto-IDs generados por Postgres). `qb.update(db, changes_map)`
  emite `UPDATE table SET ... WHERE ...`. `qb.delete(db)` emite
  `DELETE FROM table WHERE ...`. Guard de seguridad: update/delete
  REQUIEREN `.where(...)` previo (rechazo explícito para evitar
  ops masivas accidentales). Helper `renumber_placeholders` para
  combinar args del SET con args del WHERE. +4 unit + 1 E2E real
  (cycle CRUD completo).

**Estado al cierre 10.3**: la API SELECT + CRUD del MVP del ORM
declarativo sobre `type` está completa contra Postgres real.
Validado con 13 E2E end-to-end (driver + ORM combinados) +
~40 unit del builder + translator. Paridad `fitz run` ↔ `fitz
build` aún PENDIENTE (codegen ORM llega cuando aparezca demanda
real — por ahora el codegen guard sigue rechazando `fitz build`
de programas con `db.connect()`, lo que cubre transitively cualquier
uso del ORM). Deuda residual derivada listada en 10.x abajo.

**Deuda residual de 10.3** (NO bloquea 10.4):
- Auto-skip del primary key con default `Int = 0` en INSERT (hoy
  el user debe declarar el id explícito; refinable con detección
  "field con default = 0 + tipo Int + @primary → skip en INSERT").
- `.update(db, instance)` con instancia (en lugar de Map) — más
  ergonómico, traduce instance → Map automático.
- `.where(_ => true)` sigue siendo el escape para update/delete
  sin filtro; el guard chequea `where_sql.is_none()` directo, no
  detecta WHERE trivialmente-true. Refinable.
- `function calls` adentro del closure de where (`u.name.like("%a%")`,
  `u.tags.contains("rust")`) — llegan en 10.5 cuando tipos compuestos.
- Codegen del ORM para `fitz build` (codegen Python lo cerró en
  8.7; este es paralelo). Cubre todos los programs del 10.x.

#### 10.4 — Relaciones + cascades + lazy loading (~600 LoC)

Agrega navegación entre tablas. Cierra el shape "ORM declarativo"
del MVP.

- **Decorators de relación sobre fields**: `@belongs_to("User",
  on_delete="cascade")` / `@has_many("Post", via="author_id")` /
  `@has_one("Profile")` / `@many_to_many(...)` (M2M en 10.6).
  Validación del checker: tabla referenciada existe, FK column
  existe en la tabla origen.
- **Lazy navigation methods**: por cada decorator de relación, el
  ORM emite un método sobre el `type`. Ejemplo: `post.author().
  await? -> Result<User>` ejecuta `SELECT * FROM users WHERE id =
  $author_id LIMIT 1`. `user.posts().await? -> Result<List<Post>>`.
  Cada llamada dispara una query (lazy explicit).
- **Cascades en migrations**: el decorator emite `ON DELETE
  CASCADE` / `ON UPDATE RESTRICT` etc. en el SQL del FK constraint
  cuando se genera la migration (10.7). Valores: `"cascade" /
  "set_null" / "restrict" (default) / "no_action"`.
- **FK column inference**: `@belongs_to("User")` sobre `type Post`
  busca un field `author_id: Int` o `user_id: Int` (convención).
  Si quiere otro nombre, override con `@belongs_to("User",
  fk="created_by")`.
- **Loading guard**: si el user hace `post.author_loaded()` sin
  haber hecho `.include("author")` antes, error claro en runtime
  ("relación 'author' no fue eager-loaded; usá `.include(\"author\")`
  o `post.author().await?`").
- **Tests**: unit (decorators parsean, checker valida tablas
  referenciadas, métodos lazy se emiten), E2E (FK constraints
  emitidos en SQL, cascade real con DELETE que dispara).

**Criterio de éxito**: dado `post: Post`, `post.author().await?`
devuelve `Result<User>` con UN query SELECT. DELETE de un User
cascadea a sus Posts via FK constraint.

**Cierre formal 10.4 (sesiones del 2026-05-26)**: ejecutada en
2 sub-pasos:
- **10.4.a — checker + persistencia + skip virtual fields**:
  `RelationKind` (BelongsTo/HasOne/HasMany) + `CascadeAction`
  (Cascade/SetNull/Restrict default/NoAction con `.as_sql()`) +
  `RelationMetadata { kind, target_type, fk_field, on_delete,
  on_update }`. `TableMetadata.relations: HashMap<String,
  RelationMetadata>` + helper `is_virtual_field(name)`.
  `process_table_decorators` extendido con
  `parse_relation_decorator`. Defaults convencionales
  (`<lowercase(type)>_id` para `via`/`fk`). SQL builder
  (`build_select_sql`, `orm_type_insert`, `pg_row_to_instance`)
  respeta fields virtuales: skip en SELECT cols + INSERT cols +
  RETURNING; deserialización inicializa con default sentinel
  (`Value::Null` para HasOne, `Value::new_list(vec![])` para
  HasMany). +11 unit (9 checker + 2 SQL builder).
- **10.4.b — navigation methods runtime**: `dispatch_method`
  sobre `Value::Instance` ahora chequea `meta.relations` ANTES
  de los métodos custom. Helper `orm_instance_navigate`:
  resuelve THIS/TARGET TableMetadata desde el env, construye un
  `QueryBuilderState` con el WHERE apropiado según kind, delega a
  `orm_qb_first` (BelongsTo/HasOne) o `orm_qb_all` (HasMany).
  Para BelongsTo: target.primary = this.fk_field. Para HasOne/
  HasMany: target.fk_field = this.primary. El user invoca como
  `post.author_id(db).await?` (BelongsTo) o `user.posts(db).await?`
  (HasMany). +1 E2E real (tablas relacionadas con FK constraint).

**Estado al cierre 10.4**: navegación entre tablas funciona
end-to-end contra Postgres real con 1 query por método. Total
acumulado de E2E reales: **13 tests** (7 del driver core +
6 del ORM SELECT/CRUD/navigation). Validación cross-type del
target (el target_type DEBE estar en el env con `@table`) sucede
en runtime al invocar la navegación — error claro si falta.

**Deuda residual de 10.4** (NO bloquea 10.5):
- Eager loading con `.include("author")`: diferido a 10.6 según
  roadmap original. Por ahora cada call dispara 1 query (lazy
  pure) — el user es responsable de evitar N+1.
- Cascade en migrations: el `CascadeAction.as_sql()` está listo
  para emisión cuando llegue `fitz db diff` en 10.7. El runtime
  NO genera las constraints automáticamente; el user las
  declara en el `CREATE TABLE` manual hoy.
- Validación cross-type en check time: hoy solo en runtime. Si
  `@belongs_to("Nonexistent")` apunta a un type que no existe,
  el checker NO falla; falla en runtime con mensaje claro al
  invocar. Refinable post-MVP.
- Method name collisions: si un type tiene un field con relación
  + un método custom con el mismo nombre, la relación gana
  (chequea ANTES). Documentado en código; refinable con
  detección explícita si entra demanda.

#### 10.5 — Tipos avanzados (~600 LoC)

JSONB, arrays Postgres, Date/Time/Timestamp, UUID. Cierra el
mapping completo entre `type` Fitz y columnas Postgres reales.

- **JSONB**: nuevo tipo `Json` en el lenguaje (alias para `Map<Str,
  Any>` por debajo, marshaling JSON automático). Field `data:
  Json?` → columna `jsonb`. Builder methods con `->` (`u.data ->
  "address"`) y `->>` (text extraction) traducen a JSONB operators
  Postgres. Indexes GIN auto cuando `@index` sobre Json field.
- **Arrays**: `tags: List<Str>` → `text[]`. `categories: List<Int>`
  → `bigint[]`. Builder methods `.contains(x)` → `ANY($1)`,
  `.overlaps(xs)` → `&&`. Decoración `@array(dim=2)` para arrays
  multi-dimensionales (raro pero útil).
- **Date / Time / Timestamp**: tipos nuevos `Date` / `Time` /
  `Timestamp` (alias estructurales por debajo). Coerción ISO 8601
  bidireccional con Postgres. Built-ins `now()` / `today()` /
  `parse_date(s)` / `parse_timestamp(s)`. Field con default `=
  now()` emite `DEFAULT NOW()` en SQL.
- **UUID**: tipo `Uuid` (representado como Str adentro pero validado
  como UUID en marshaling). Field `@primary @uuid id: Uuid =
  uuid_v4()` emite `DEFAULT gen_random_uuid()`.
- **Tests**: unit sobre cada tipo (marshaling Postgres binary ↔
  Fitz Value), E2E (insert + query con cada tipo).

**Criterio de éxito**: un `type Post` con `data: Json?`, `tags:
List<Str>`, `created_at: Timestamp = now()`, `uuid: Uuid =
uuid_v4()` se inserta y se consulta con paridad bit-a-bit.

#### 10.6 — Eager loading + M2M + self-referential + soft deletes (~700 LoC)

Cierra las features relacionales del MVP.

- **`.include(...)` eager loading**: `Post.where(...).include("author",
  "tags", "comments.author").all().await?`. El builder traduce a
  LEFT JOIN(s) en una sola query, agrupa resultados por PK del row
  principal, hidrata las relaciones embebidas en cada instancia. El
  user accede via `post.author_loaded() -> User` (sin `.await?`,
  ya está en memoria) o sigue usando `post.author().await?` (lazy,
  refetch). Métodos `_loaded()` emitidos por el ORM por cada
  relación.
- **Nested include**: `.include("comments.author.profile")` carga
  comments → author → profile en cascada. Genera múltiples LEFT
  JOINs o subqueries optimizadas según depth.
- **Many-to-many con pivot explícita**: `@many_to_many("Tag",
  via="PostTag")` exige que `PostTag` sea un `type` declarado con
  `@table("post_tags")` y dos `@belongs_to` (a Post y Tag). El
  checker valida que existe y tiene el shape correcto. Navigation
  `post.tags().await?` resuelve via 2 FKs.
- **Self-referential**: `@has_many("Comment", via="parent_id")`
  sobre `type Comment { parent_id: Int? }`. El checker permite el
  self-reference si el FK column es nullable. Útil para threading
  (replies), categorías anidadas, árboles de organización.
- **Soft deletes**: `@soft_delete` sobre el `type` agrega un field
  implícito `deleted_at: Timestamp?` (NULL = activo). Queries
  default filtran rows con `deleted_at IS NULL`. Opt-out con
  `.with_deleted()` builder method. `Post.delete()` setea
  `deleted_at = NOW()` en vez de hacer DELETE real. `.hard_delete()`
  fuerza DELETE real.
- **Tests**: unit (translación de include → LEFT JOIN, M2M
  validation, self-ref validation, soft delete filter), E2E (un
  query con includes carga todo en una sola pasada, M2M real con
  pivot table, threading de comments funciona, soft delete oculta
  rows).

**Criterio de éxito**: el ejemplo entero del shape arriba
(`get_post` con includes anidados + comments self-referential +
tags via M2M + soft delete sobre Post) corre bit-a-bit con UN
solo query SQL.

#### 10.7 — Transactions + raw SQL + `fitz db diff` + `fitz db migrate` (~1000 LoC)

Migraciones autogeneradas + escape hatch + transactions. Sub-paso
más grande del MVP (cierra el ciclo "dev workflow"). 

- **Transactions closure-scope**: `db.transaction(fn (tx) => {
  tx.insert(...).await?; tx.update(...).await?; Ok(value) }).await?`.
  El closure recibe un `Tx` (subset de `Db` con queries) en vez
  del pool. `Ok(...)` → COMMIT, `Err(...)` → ROLLBACK, panic → 
  ROLLBACK. El `Tx` agarra UNA conn del pool durante la duración
  del closure. Isolation level configurable: `db.transaction_with(
  level="serializable", fn (tx) => ...)`. Niveles: `"read_uncommitted" /
  "read_committed" (default) / "repeatable_read" / "serializable"`.
- **Raw SQL escape hatch**: `db.query("SELECT ... WHERE x = $1",
  [arg]).await?` con placeholders `$1`/`$2`. Devuelve `List<Row>`
  donde `Row = Map<Str, Any>`. Variante tipada `db.query_as<User>("...",
  args).await?` mapea cada row a `User` validando shape (paralelo
  a `__FromFitzJson` de HTTP).
- **`fitz db diff`** subcomando nuevo:
  - Conecta a Postgres con la URL del manifest (sección
    `[database]` en `fitz.toml`).
  - Introspecciona el schema vivo via `information_schema`
    (tables, columns, FKs, indexes).
  - Compara con los `type` con `@table` del proyecto.
  - Calcula el diff: `CREATE TABLE` para tablas nuevas, `ALTER
    TABLE ADD COLUMN` para fields nuevos, `ALTER TABLE DROP
    COLUMN` con confirmación (destructivo), `ALTER TABLE ALTER
    COLUMN TYPE` cuando el tipo cambió, `CREATE INDEX` /
    `DROP INDEX`, FK constraints con `ADD CONSTRAINT` /
    `DROP CONSTRAINT`.
  - Emite `migrations/0042_<auto_label>.sql` con timestamp + label
    derivada del diff (`0042_add_email_to_user.sql`).
  - El user revisa, edita si hace falta, y commitea.
- **`fitz db migrate`** aplica las migrations pendientes:
  - Tabla `_fitz_migrations` en Postgres con `(filename, applied_at)`.
  - Detecta archivos nuevos en `migrations/` (orden alfabético) que
    no están en `_fitz_migrations`.
  - Aplica cada uno adentro de una transaction. Falla → ROLLBACK +
    error claro (qué archivo, qué error, qué línea SQL).
  - Idempotente: re-correr sin migrations nuevas es no-op.
- **`fitz db rollback`** revierte la última migration aplicada
  (requiere que el archivo tenga sección `-- DOWN` con SQL inverso).
  Sin sección `-- DOWN`, error claro pidiéndolo.
- **Tests**: unit sobre diff engine (cubre cada tipo de cambio:
  add/drop column, change type, add/drop FK, add/drop index), E2E
  con Postgres real (full workflow: edit `type`, `fitz db diff`,
  inspect SQL, `fitz db migrate`, verify schema en Postgres).

**Criterio de éxito**: agregar `email: Str` a `type User`, correr
`fitz db diff`, ver `0042_add_email_to_user.sql` con `ALTER TABLE
users ADD COLUMN email text NOT NULL`, correr `fitz db migrate`,
ver la columna en Postgres real.

#### 10.8 — LISTEN/NOTIFY pub/sub (~250 LoC)

Pub/sub nativo de Postgres integrado con `@background` de 9.w.3.
Reemplaza Redis pub/sub para el caso típico.

- **`db.listen(channel, fn (payload) => ...).await?`** registra un
  handler para mensajes en ese channel. Internamente: una conn
  dedicada del pool queda en estado `LISTEN <channel>`, el driver
  parsea `NotificationResponse` y dispara el callback via
  `tokio::spawn` (paralelo a `@background`).
- **`db.notify(channel, payload)`** emite. Payload es `Str` (Postgres
  limit ~8000 bytes). Para payloads grandes, convención: emitir un
  ID y dejar que el listener fetchee.
- **Reconnect handling**: si la conn dedicada al LISTEN muere, el
  driver re-conecta + re-emite `LISTEN <channel>`. Pero notificaciones
  emitidas durante el downtime se pierden (limitación de Postgres,
  documentada).
- **Integración con `@background`**: el callback de `listen` puede
  ser una fn `@background`, encaja con el modelo de 9.w.3 sin glue
  extra.
- **Tests**: unit (parsing de NotificationResponse, dispatch del
  handler), E2E (Postgres real, un proceso emite, otro escucha,
  payload llega).

**Criterio de éxito**: dos procesos Fitz conectados al mismo
Postgres, uno hace `db.notify("user_created", "42")`, el otro
escucha con `db.listen("user_created", fn (payload) => ...)` y
ejecuta el callback.

#### 10.9 — Full-text search (~400 LoC)

FTS nativo de Postgres con builder method. Reemplaza Elasticsearch
para muchos casos.

- **`@tsvector("title, body")`** decorator sobre `type` agrega
  field implícito `search_tsv: Tsvector` + trigger Postgres auto
  que actualiza el tsvector cuando title/body cambian.
- **`.search(query)`** builder method: `Post.where(...).search("rust
  postgres").rank().limit(10).all()`. Traduce a `WHERE search_tsv
  @@ websearch_to_tsquery($1) ORDER BY ts_rank(search_tsv, query)
  DESC`.
- **Highlighting opcional**: `.search(query).highlight("title",
  "body")` agrega columnas calculadas con `ts_headline()` para
  resaltar matches con `<mark>...</mark>`.
- **Language config**: `@tsvector("title, body", lang="spanish")`
  para stemming en idioma específico. Default `english`.
- **Tests**: unit (traducción `.search` → SQL), E2E (Postgres
  real, FTS query con ranking devuelve rows ordenados por relevancia).

**Criterio de éxito**: `Post.search("postgres tutorial").rank()
.limit(5).all().await?` devuelve los 5 posts más relevantes
ordenados por ts_rank.

#### 10.10 — Guía + ejemplo runnable + refresh boilerplates + cierre formal (~docs)

Último sub-paso. Documentación + cierre.

- **Cap nuevo en `docs/guide.md`** "Stack DB nativo" entre cap 30
  ("Jobs sin Celery") y cap 31 ("Qué sigue", renumeración 31→32).
  Sub-secciones:
  - "Cómo se ve" (panorama vs Prisma / Drizzle / Diesel / SQLAlchemy)
  - Setup (`connect(...)`, connection string, pool)
  - `@table` + `@primary` + `@column`
  - CRUD básico (where, order_by, limit, all, first)
  - Relaciones (belongs_to, has_one, has_many, M2M con pivot)
  - Cascades
  - Lazy vs eager con `.include(...)`
  - Self-referential
  - Soft deletes
  - JSONB / arrays / dates / UUID
  - Transactions
  - Raw SQL escape hatch
  - Migraciones (`fitz db diff` / `migrate` / `rollback`)
  - LISTEN/NOTIFY + integración con `@background`
  - Full-text search
  - Por qué Fitz hace esto distinto (5 diferenciales con panorama
    vecino vs ORMs existentes — política `feedback_guide_emphasize_uniqueness`)
- **Ejemplo runnable** `examples/guide/31-db.fitz`: app completa
  con usuarios + posts + comments + tags. POST/GET endpoints +
  transactions + LISTEN/NOTIFY + FTS. <150 LoC. Se suma al smoke
  `GUIDE_EXAMPLES_COMPILE`.
- **Refresh boilerplates 5/6**: reemplaza la layer SQLAlchemy via
  interop Python por la DB nativa. Mejora visible: simplifica
  Dockerfile (~150 MB → ~80-100 MB sin necesidad de runtime
  Python), elimina `models.py` + `db.py` Python, todo en `.fitz`.
- **README emphasis**: bullets en feature table del README + nota
  en "Estado del proyecto" + footnote dedicada con los 5
  diferenciales contra Prisma/Drizzle/Diesel/SQLAlchemy.
- **CHANGELOG v0.10.0** entrada gigante.
- **Roadmap update**: marca Fase 10 entera como CERRADA, suma
  deuda residual derivada.

**Criterio de éxito**: el cap del guide compila el ejemplo
runnable con `fitz build`, paridad bit-a-bit con `fitz run`,
boilerplates 5/6 simplificados y validados end-to-end.

### Decisiones tácticas chicas (resolver al arrancar 10.1)

- **Postgres mínimo: 14+**. Cubre ~98% del market, SCRAM-SHA-256
  es default desde 13, JSONB maduro. Postgres 16 tiene goodies
  (logical replication, MERGE) pero no son MVP.
- **Nombre del módulo built-in: `db`**. Paralelo a `jwt` / `hash`
  / `json` de 9.w.1 y 8.x. Genérico, abre puerta a otros drivers
  post-MVP (mysql/sqlite) sin breaking change.
- **Connection string format**: URI estándar
  `postgres://user:pass@host:port/dbname?sslmode=...`. Mismo que
  libpq, psycopg2, pgx, sqlx. Familiar para todos.
- **SSL/TLS modes**: `disable` (default dev) + `require` en MVP.
  `verify-ca` / `verify-full` post-MVP (cert pinning).
- **Encoding**: UTF-8 only. Postgres default desde 9.0.
- **Migrations format**: SQL plano emitido por `fitz db diff`.
  Sin DSL Fitz (sin mid-level IR). Máxima transparencia, el user
  lee y entiende. DSL Fitz queda como deuda futura si aparece
  pedido de portar a otro motor.
- **DB URL en manifest**: nueva sección `[database]` en `fitz.toml`:
  ```toml
  [database]
  url = "postgres://localhost:5432/myapp"
  # o env-driven:
  url = "${env:DATABASE_URL}"
  ```
  Resolución de `${env:...}` reusa la mini-fase env builtin
  (`env(key)` / `env_or(key, default)`) ya cerrada el 2026-05-22.

### Deuda residual diferida a Fase 10.x post-MVP

Lo que NO entra al MVP pero está documentado para sub-fases futuras:

- **Polymorphic associations** (~400 LoC, complica el checker con
  unión de tipos, pocos casos reales). Estilo Rails `commentable_id` +
  `commentable_type`.
- **Multi-DB** (MySQL, SQLite). El abstract `db` module está
  diseñado para soportarlo, pero los drivers concretos son fases
  separadas. SQLite es razonable de meter por su simplicidad
  (cero auth, archivo local) si aparece presión para dev workflow
  sin Postgres.
- **TLS verify-ca / verify-full**. Production security hardening.
- **Read replicas**. `connect(primary, replicas=[...])` con routing
  automático (writes → primary, reads → replica round-robin).
- **Prepared statements cache**. Hoy cada query parametriza con
  Extended Query desde cero. Cache de prepared statements ahorra
  ~1 round-trip por query repetida.
- **Query plan inspection** (`EXPLAIN`). Útil para debugging pero
  no MVP.
- **Copy protocol** (bulk insert). `db.copy_from(table, rows)`
  para bulk loading. ~10x más rápido que `INSERT` masivo. Útil
  para data import.
- **Savepoints anidados explícitos**. Hoy las transactions son
  flat (un solo nivel). Anidamiento real (`SAVEPOINT` / `RELEASE`
  / `ROLLBACK TO`) post-MVP.
- **Soft delete con cascade soft** (cuando un parent se soft-deletea,
  los children también se marcan). Hoy cascade es solo en hard
  delete.
- **Validation hooks**: `@before_create fn validate_user(u: User) ->
  Result<Null>`. Estilo ActiveRecord callbacks.
- **DSL Fitz para migrations** (vs SQL plano). Sub-paso futuro si
  aparece pedido de multi-DB portability.

### Por qué Fitz hace esto distinto (diferenciales vs ORMs existentes)

Política `feedback_guide_emphasize_uniqueness` aplicada al diseño:

1. **Driver puro Fitz, cero deps de sistema**. Prisma necesita
   Node.js + Rust engine binario. SQLAlchemy necesita Python +
   psycopg2 + libpq. Diesel necesita libpq instalado. Fitz no
   necesita nada — el binario produced por `fitz build` habla
   Postgres wire protocol directo.
2. **Paridad bit-a-bit `fitz run` ↔ `fitz build`**. El ORM funciona
   IDÉNTICO en interpretado y compilado. Mismo SQL emitido, mismas
   queries, mismo resultset. No hay "modo dev" vs "modo prod" que
   diverja.
3. **Validación estática end-to-end**. El checker valida `user.email`
   contra el field declarado en el `type User`, `User.where(fn (u)
   => u.invalid_field == 1)` falla en compile time. Prisma genera
   un client TypeScript que valida en el TypeScript checker (capa
   adicional). SQLAlchemy valida en runtime. Fitz valida directo
   en el checker del lenguaje.
4. **Migrations del diff sin DSL intermedio**. Prisma tiene
   `schema.prisma` (DSL custom). Alembic Python tiene
   `op.add_column()` (Python). Fitz emite SQL plano directo, el
   user lo lee y entiende.
5. **Integrado con el resto del lenguaje**. JSONB acoplado a `type
   Foo` Fitz. Relaciones con type-safety. LISTEN/NOTIFY conectado
   con `@background` de Jobs (9.w.3). Auth (9.w.1) + DB (Fase 10)
   + WS (9.w.2) + Cron (9.w.3) son piezas que se ensamblan natural,
   no librerías separadas que el user tiene que pegar.

---

## Fase 10.b — Cierre del codegen ORM (paridad `fitz run` ↔ `fitz build`) ✅ CERRADA 2026-05-26

**Estado**: **CERRADA 2026-05-26 (v0.10.1)**. Arrancada 2026-05-25
post-release v0.10.0, cerrada en 2 días con 17 sub-pasos (10.b.0 →
10.b.17) y 23 commits totales.

### Estadísticas finales (v0.10.1)

- **~9580 LoC netas** (19222 insertions / 9642 deletions) sobre 10
  archivos del codegen, evaluator, types, http, db, tests y docs.
- **2552 unit + 81 cli_e2e + 291 compile_e2e + 3 openapi** sin
  feature (smoke `GUIDE_EXAMPLES_COMPILE` incluye `32-orm.fitz` —
  291 ejemplos guía).
- **44 db_real_postgres** (`#[ignore]` opt-in, 16 son paridad
  codegen E2E nuevos vs evaluator) corriendo en cada push a `main`
  via job nuevo `db-postgres` en `.github/workflows/ci.yml` con
  service container `postgres:16`.
- Clippy `--all-targets -D warnings` limpio, fmt `--all --check`
  limpio.
- **Paridad bit-a-bit cumplida**: todo lo que `fitz run` soporta
  del ORM ahora compila a binario nativo con `fitz build`.

### Diferencial logrado (post-10.b)

**Único lenguaje moderno** con driver Postgres puro + ORM declarativo
+ paridad bit-a-bit `fitz run` ↔ `fitz build` + LSP completo
**sin macros derive ni introspection runtime**. La paridad codegen
cierra la última brecha que separaba el intérprete del binario.

**Decisiones técnicas que se mantuvieron**: estrategia híbrida (SQL
constante en codegen-time + rows tipadas concretas + `__FitzValue`
sólo para JSONB libre y agregados con groupby). Cero overhead
runtime para construir SQL — comparable a Diesel/sqlx, mejor que
SQLAlchemy/ActiveRecord. Eager loading con dispatch estático: el
relation name como Str literal en compile-time produce un match
exhaustivo, typos detectados en compile-time.

**Próximo norte tras v0.10.1**: cap nuevo "Postgres + ORM nativo"
en `docs/guide.md` (cap 31), ejemplos del ORM en los boilerplates
Dockerizados (5 y 6 SQLAlchemy → Fitz ORM o boilerplate nuevo
dedicado), benchmarks Fitz ORM vs SQLAlchemy. Ver
`docs/deudas-post-5b.md` para el detalle de deuda residual de
Fase 10/10.b (cero residual del codegen ORM; refinements
opcionales como migraciones automáticas, transactions, TLS
strict, JSON operators del lado SQL).

---

### Detalle de sub-pasos (histórico)

### Motivación

Al cerrar Fase 10 entera (v0.10.0) el ORM nativo está funcional en
`fitz run` pero **el codegen para `fitz build` está sólo parcialmente
implementado**:

- ✅ Driver Postgres en codegen (10.1.c): `db.connect`/`query`/`exec`/
  `close`/`is_closed` emiten Rust válido que compila y corre.
- ❌ **ORM declarativo: CERO codegen.** `User.all(db)` /
  `User.where(closure).first(db)` / `User.insert(db, row)` /
  `.update`/`.delete`/`.order_by`/`.limit`/`.offset`/`.group_by` /
  agregados (`.sum`/`.avg`/`.min`/`.max`) / navigation methods
  (`post.author_id(db)`/`user.posts(db)`) NO emiten código.
- ❌ **Traductor closure → SQL: CERO codegen.** El evaluator tiene
  `translate_expr_to_sql` (~390 LoC) que recorre AST de closures
  `fn(u) => u.age > 18` y emite SQL parametrizado. El codegen NO
  tiene equivalente.
- ❌ **JSONB / arrays marshalling**: parcialmente en preludio
  driver, pero no integrado con el ORM (porque no hay ORM en
  codegen).

Bug síntoma reportado al arrancar 10.b: `let db = db.connect(...)`
en `fitz build` emite Rust con `let db: Pin<Box<dyn Future...>> =
__fitz_db_connect(...)` sin envolver en `Box::pin(...)` (E0308),
más warnings de imports unused.

### Decisiones de diseño (tomadas 2026-05-25)

**Estrategia híbrida** sobre las 3 alternativas evaluadas:

1. **SQL constante en codegen-time**: cada `.where(fn(u) => u.age >
   18)` se walka del AST del closure DURANTE EL CODEGEN, emitiendo
   un fragmento SQL constante (`"\"age\" > $1"`) más un Vec<PgValue>
   construido en runtime con los args. Zero overhead runtime para
   construir el SQL (comparable a Diesel/sqlx, mejor que SQLAlchemy/
   ActiveRecord que construyen SQL via objetos en runtime).
2. **Instances tipadas**: `User.where(...).first(db).await?`
   retorna `User` (alias `Arc<Mutex<UserData>>` ya existente del
   codegen 5b.2), NO `__FitzValue` dinámico. Cero overhead de enum
   dispatch en el hot path de los rows.
3. **`__FitzValue` solo donde de verdad hace falta**: JSONB
   (`Map<Str, Any>` ↔ jsonb) y agregados con `group_by` cuyo
   retorno es `List<Map<Str, Any>>` con shape heterogéneo. El enum
   `__FitzValue` ya existe en el preludio (F13 SPIKE para literales
   heterogéneos), se reusa.
4. **Arrays tipados**: `List<Int>` ↔ `Vec<i64>`, `List<Str>` ↔
   `Vec<String>`, etc. Marshalling directo sin pasar por
   `__FitzValue`.
5. **Deserializador per-type emitido**: por cada `type Foo` con
   `@table`, codegen emite `impl __FromFitzDbRow for FooData` con
   conversión field-por-field desde `__FitzPgValue` (paralelo a
   `__FromFitzJson` para JSON HTTP).
6. **`QueryBuilderState` Rust runtime**: struct `__FitzQueryBuilder
   <Row>` con state mínimo (Vec<(String, Vec<PgValue>)> de WHERE
   fragments + Vec<(String, bool)> de ORDER BY + Option<i64> de
   limit/offset + Vec<String> de GROUP BY). Métodos `.where_sql_
   fragment(...)` agregan al state, terminales `.all/.first/.count`
   componen el SQL final con `INNER JOIN`s (relations futuras) +
   ejecutan via `__fitz_db_runtime`.

### Por qué híbrida (y no las otras 2)

- **Opción 1 (`__FitzValue` puro paralelo al evaluator)**: scope
  conocido, pero hereda el overhead de enum dispatch en cada field
  access, contradice "binario nativo performante". Rechazada.
- **Opción 2 (Rust idiomático cero-overhead con todo concreto)**:
  máximo perf pero scope ~14 días y mucha superficie nueva donde
  romper paridad bit-a-bit. Rechazada.
- **Opción 3 (HÍBRIDA, elegida)**: SQL constante en codegen-time +
  rows tipadas + `__FitzValue` sólo para JSONB/agregados con
  groupby. Mejor balance scope/perf/paridad.

### Sub-pasos (11 sub-fases)

División incremental, cada sub-paso cierra una pieza testeable
end-to-end con `fitz build` produciendo binario que corre contra
Postgres real con paridad bit-a-bit al `fitz run` equivalente.

#### 10.b.1 — Fixes preludio runtime + smoke `fitz build` con `db.connect` solo (~200 LoC)

Cierra los 3 bugs detectados al arrancar 10.b. Smoke: `let db =
db.connect("postgres://...").await?` compila a Rust válido sin
ORM methods.

- **`Box::pin(...)` wrap del Future de `__fitz_db_connect`**: en
  `gen_db_module_call`, el Type retornado es `Type::Future(...)`,
  el emit del callsite debe envolver en `Box::pin(...)`. Paralelo
  a cómo se emiten otros futures `.await`-eables (Fase 6).
- **Imports condicionales `Arc, Mutex`**: el preludio HTTP emite
  `use std::sync::{Arc, Mutex};` siempre. Con `db.connect` sin HTTP
  ni state compartido, las imports quedan unused. Fix: emit
  condicional según los flags `has_http`/`uses_db`/`uses_python`.
- **Feature `time` de tokio**: el Cargo.toml emitido necesita
  `tokio = { features = ["..., time"] }` cuando hay `sleep(...)` o
  cuando `uses_db = true` (el pool del driver usa intervals
  internos).
- **Tests**: 3 unit + 1 E2E (binario que conecta a Postgres,
  ejecuta `SELECT 1` via `db.query`, imprime el resultado).

**Criterio de éxito**: `fitz build` sobre `let db =
db.connect(url).await?; let rows = db.query("SELECT 1 AS n",
[]).await?; print(rows[0]["n"])` produce binario que imprime `1`
contra Postgres local.

#### 10.b.2 — Closure → SQL translator en codegen (~400 LoC)

Port de `translate_expr_to_sql` del evaluator al codegen. Esta es
la pieza más crítica porque habilita todo el `.where(...)`.

- **Helper `gen_closure_to_sql(closure: &Expr, table_meta:
  &TableMetadata) -> (String, Vec<RustExpr>)`**: el primer return
  es el SQL fragment con placeholders `$N`, el segundo es el
  Vec<Rust expression code> que se evalúa en runtime para los
  bindings. Recorre AST: BinOp (Eq/NotEq/Lt/Gt/Lte/Gte/And/Or),
  UnaryOp (Not), Field access sobre el param (`u.field` → nombre
  SQL de la columna respetando `@column(name="...")`), literales
  (Int/Float/Str/Bool/Null), variables del scope outer (capturadas
  como bindings runtime).
- **Validación estática**: si el closure referencia un field que
  no existe en el `type`, error de codegen claro con span.
  Replica la regla del checker.
- **Method calls dentro del closure**: `.is_in([...])` /
  `.starts_with(s)` / `.contains(s)` / `.is_null()` se traducen a
  IN / LIKE / IS NULL (Fase 10.5.g). Implementación paralela al
  `translate_method_call_to_sql` del evaluator (~150 LoC).
- **Tests**: replicar los 17 unit tests `translate_*` del evaluator
  apuntados al helper de codegen.

**Criterio de éxito**: golden tests que validan SQL emitido
bit-a-bit idéntico entre evaluator y codegen sobre 17 patterns
(BinOps, And/Or chains, is_null, is_in, starts_with, etc.).

#### 10.b.3 — ORM read methods en `gen_call`: `.all`/`.first`/`.count` (~600 LoC)

Detección de método ORM sobre `Type::Nominal(id)` con
`@table` + emisión del SELECT correspondiente.

- **Dispatch en `gen_call`**: cuando `callee` es `Type.method(...)`
  donde Type tiene metadata `@table`, ruta a `gen_orm_terminal_*`.
- **`User.all(db)`** → emite `__fitz_db_runtime::query(&db,
  "SELECT col1, col2, ... FROM \"users\"", &[])` + deserialización
  de cada row a `UserData`. Retorna `Result<Vec<User>, String>`.
- **`User.first(db)`** → SELECT con `LIMIT 1`, fallback a
  `Err("no rows".to_string())` si vacío.
- **`User.count(db)`** → `SELECT COUNT(*) FROM ...`, parse del
  Int del row.
- **Deserializador per-type**: `impl __FromFitzDbRow for UserData`
  emitido en `gen_type_def` cuando el type tiene `@table`. Lee
  cada field del row + coerce desde `__FitzPgValue` al tipo Fitz
  concreto del field.
- **Tests**: 5 unit + 3 E2E (User.all sin filter, User.first hit/
  miss, User.count, con/sin LIMIT).

**Criterio de éxito**: `fitz build` sobre `let users = User.all(db)
.await?; print(users.len())` produce binario que imprime el count
real desde la DB, idéntico bit-a-bit al `fitz run` equivalente.

#### 10.b.4 — ORM write methods: `.insert`/`.update`/`.delete` (~500 LoC)

- **`User.insert(db, User { ... })`** → INSERT con RETURNING * del
  PK y campos generados. Skip de fields virtuales (relations).
  Skip del PK si tiene default Int = 0 (convención para auto-id).
  Retorna `Result<User, String>` con la instance hidratada
  (incluyendo el id generado).
- **`User.where(...).update(db, changes_map)`** → UPDATE con SET
  fields del map. Guard: requiere `.where(...)` previo, error si
  no. Retorna `Result<i64, String>` con rows affected.
- **`User.where(...).delete(db)`** → DELETE WHERE. Mismo guard.
  Retorna `Result<i64, String>`.
- **Marshalling de field values**: bool/Int/Float/Str/Bytes
  directos via `__IntoPgValue`. List<Int>/Str/Float/Bool → arrays
  PG (preparación para 10.b.8). Map<Str, Any> → JSONB
  (preparación). `Value::Null` para nullable.
- **Tests**: 4 unit + 3 E2E (insert + read-back, update con count,
  delete con count, guard que aborta sin .where).

**Criterio de éxito**: cycle CRUD completo sobre binario nativo,
paridad con el cycle del intérprete.

#### 10.b.5 — QueryBuilder chain: `.where`/`.order_by`/`.limit`/`.offset`/`.group_by` (~400 LoC)

- **Struct `__FitzQueryBuilder<T>` en preludio**: state con
  `where_fragments: Vec<(String, Vec<__FitzPgValue>)>`, `order_by:
  Vec<(String, bool)>` (col + descending), `limit: Option<i64>`,
  `offset: Option<i64>`, `group_by: Vec<String>`.
- **Chain methods**: cada `.where(closure)` agrega un fragment con
  el SQL ya traducido en codegen-time + los runtime bindings.
  `.order_by(fn(u) => u.field)` y `.order_by(fn(u) => -u.field)`
  parsean el `-` como descending. `.limit(n)/.offset(n)` setean
  campos. `.group_by(fn(u) => u.field)` agrega a la lista.
- **Compose final**: el método terminal (`.all/.first/.count/etc.`)
  combina state + table + cols en el SELECT final.
- **Múltiples `.where()` se combinan con AND**: matchea evaluator.
- **Tests**: 6 unit + 4 E2E (chain de 3 métodos, múltiples wheres,
  order_by + limit, group_by + count).

**Criterio de éxito**: `User.where(fn(u) => u.age > 18)
.order_by(fn(u) => -u.age).limit(10).all(db).await?` produce el
mismo resultset en `fitz run` y `fitz build`.

#### 10.b.6 — Agregados: `.sum`/`.avg`/`.min`/`.max` (~250 LoC)

- **Sintaxis**: `.sum(fn(u) => u.field, db)` — closure que
  identifica el field + db como segundo arg.
- **Sin group_by**: retorna scalar `Result<Int|Float|Null, String>`
  (Null cuando 0 rows). Emite `SELECT SUM(col) FROM ... WHERE ...`.
- **Con group_by**: retorna `Result<Vec<HashMap<String,
  __FitzValue>>, String>` con una entry por grupo. Emite
  `SELECT group_col1, ..., AGG(col) AS sum FROM ... GROUP BY
  group_col1, ...`.
- **Coerción ret type**: `sum`/`min`/`max` retornan el tipo del
  field (Int o Float). `avg` siempre Float (paralelo a Postgres).
- **Tests**: 4 unit + 3 E2E (sum scalar, avg con groupby, min/max
  combinados, set vacío → Null).

**Criterio de éxito**: agregados sobre binario nativo coinciden
bit-a-bit con evaluator, incluyendo el caso `Null` sobre set
vacío.

#### 10.b.7 — Relations + navigation methods (`@belongs_to` / `@has_many` / `@has_one`) (~500 LoC)

- **Metadata persistida**: `TableMetadata.relations` ya está en el
  checker (Fase 10.4.a). El codegen consume la metadata y emite,
  por cada relation, un método sobre el struct generado del type
  origin.
- **`@belongs_to("User")` sobre `Post.author_id: Int`**: emite
  `impl PostData { pub async fn author_id(&self, db: &__FitzDbConn)
  -> Result<User, String> { /* SELECT * FROM users WHERE id =
  $self.author_id LIMIT 1 */ } }`. El nombre del método es el FK
  field name (`author_id`).
- **`@has_many("Post", via="author_id")` sobre `User.posts:
  List<Post>`**: emite `impl UserData { pub async fn posts(&self,
  db: &__FitzDbConn) -> Result<Vec<Post>, String> { /* SELECT *
  FROM posts WHERE author_id = $self.id */ } }`.
- **`@has_one`**: igual que `has_many` pero `.first()` interno.
- **Validación cross-type en codegen**: target_type debe existir
  con `@table` en el env. Error de codegen claro si falta.
- **Virtual fields**: el field `posts: List<Post>` con `@has_many`
  NO va al `UserData` struct (es virtual). Skip en gen_type_def
  + `__FromFitzDbRow`.
- **Tests**: 3 unit + 2 E2E (belongs_to navega correcto, has_many
  retorna list, has_one retorna Result).

**Criterio de éxito**: `post.author_id(db).await?` y
`user.posts(db).await?` funcionan idéntico en `fitz run` y `fitz
build`.

#### 10.b.8 — JSONB + arrays marshallers en preludio (~300 LoC)

- **JSONB**: helpers `__fitz_value_to_jsonb_pg(&__FitzValue) ->
  __FitzPgValue` y `__fitz_jsonb_pg_to_value(&__FitzPgValue) ->
  __FitzValue`. Para field `data: Map<Str, Any>` del type ORM, el
  marshalling pasa por aquí.
- **Arrays**: `__FitzPgValue::ArrayInt(Vec<i64>)`/`ArrayFloat`/
  `ArrayStr`/`ArrayBool`. Conversiones bidireccionales con
  `List<T>` Fitz. Heredan el formato `{e1,e2,e3}::int8[]` que el
  driver ya soporta.
- **Tests**: 5 unit + 3 E2E (JSONB round-trip simple, JSONB
  anidado, JSONB nullable Null, array<text>, array<bigint>, array
  vacío).

**Criterio de éxito**: `INSERT` con field jsonb anidado + array, y
`SELECT` posterior, devuelve la misma estructura bit-a-bit en
`fitz run` y `fitz build`.

#### 10.b.9 — Operadores extendidos en `.where`: `is_in`/`starts_with`/`contains`/`is_null` (~200 LoC)

Cubierto en 10.b.2 (el translator hace dispatch sobre method calls
del closure), pero los marshallers de args + tests E2E quedan
separados acá.

- **`.is_in([...])`**: cuando el arg es lista literal, expande a
  `col IN ($1, $2, ...)`. Cuando el arg es variable (lista
  runtime), construye los `$N` dinámicos.
- **`.starts_with(s)` / `.contains(s)`**: traducen a `col LIKE
  's%'` / `col LIKE '%s%'`. Escape automático de `%`/`_` en `s`
  (igual que evaluator).
- **`.is_null()`** sobre nullable field: traduce a `col IS NULL`.
- **Tests**: 4 unit + 2 E2E (is_in con literal, starts_with con
  escape, contains, is_null nullable).

**Criterio de éxito**: filtros complejos `.where(fn(u) =>
u.country.is_in(["AR", "UK"]) and u.deleted_at.is_null())` en
binario nativo idéntico al intérprete.

#### 10.b.10 — Tests E2E paridad bit-a-bit (~ports de db_real_postgres.rs)

Replicar los ~20 tests E2E de `tests/db_real_postgres.rs` que hoy
solo corren contra el evaluator, agregando una variante por test
que use `fitz build` + binario + ejecuta el mismo programa con la
misma DB, comparando outputs.

- **Infrastructure compartida**: env var `FITZ_TEST_PG_URL` ya
  existe. Sumar feature flag `postgres-build-tests` (paralelo al
  `postgres-tests` actual) para que el CI no las corra siempre.
- **Helper test**: dado un programa Fitz src, lo compila con
  `fitz build`, lo ejecuta, captura stdout, lo compara contra el
  stdout de `fitz run` del mismo src. Test pasa si match.
- **Cobertura objetivo**: 100% de los E2E del evaluator
  reportados en paridad bit-a-bit.
- **Tests**: ~15-20 E2E nuevos en `tests/db_build_paridad.rs`.

**Criterio de éxito**: todos los E2E del evaluator (CRUD, relations,
JSONB, arrays, agregados, group_by) pasan también en `fitz build`
con outputs idénticos.

#### 10.b.11 — `.update` con List literal + Map literal

Branches nuevos en `gen_qb_update_set_args` para que
`.update(db, {"tags": ["a", "b"]})` y `.update(db, {"data": {"k":
1}})` emitan los casts apropiados (`::text[]`/`::jsonb`) y
serialicen los valores literales. Workaround Windows UAC: stem del
helper renombrado a `orm_upd_list_map_codegen` (ERROR_ELEVATION_
REQUIRED 740 con "update" en el nombre).

#### 10.b.12 — Tipos compuestos: NULL en arrays + Map<Str, T> concretos

- **12.a**: `List<Int?>` ↔ `int8[] NULL`. `__FitzPgValue::Array
  { elem_oid, values }` codifica `NULL` sin quotes en el text format
  `{a,NULL,c}`. Parser/encoder simétricos. Branches específicos en
  `orm_field_coerce_block` y `orm_marshal_field_to_pg` para arrays
  nullable inner.
- **12.b**: `Map<Str, T>` con T concreto (Int/Float/Str/Bool) ↔
  `jsonb` con shape homogéneo `HashMap<String, T>` Rust. Marshaling
  directo sin pasar por `__FitzValue`. K se restringe a Str (Postgres
  jsonb keys son strings); `Map<Int, Int>` rechazado con error claro.

#### 10.b.13 — Navigation chain + JSONB shape (by design)

Decisión: las navigations siempre devuelven `QueryBuilder<Target>`
cuando `args.is_empty()`, permitiendo `user.posts().order_by(...).
all(db)`. Terminales obligatorios para ejecutar. JSONB conserva el
shape libre del Map<Str, Any> — no se valida shape (by design: el
user opera el dict de retorno con `.get(...)?`).

#### 10.b.14 — GROUP BY + aggregate (Type::Aggregated)

Nueva variante `Type::Aggregated(Box<Type>)` separada de
`Type::QueryBuilder(Box<Type>)` para el path GROUP BY. El checker
refina `.group_by(closure)` a `Aggregated<Row>` y los métodos
agregados (`.count(db)` / `.sum(closure, db)` / etc.) sobre
`Aggregated` devuelven `Result<List<Map<Str, Any>>>` (vs scalar
sobre `QueryBuilder`). Helper `aggregate_groups` paralelo al scalar.
Por qué separado: el path scalar devuelve `Float` con cast `::float8`;
el path grouped devuelve rows heterogéneos con shape `{group_field:
value, count: N, sum_x: N, ...}`.

#### 10.b.15 — Eager loading (.preload sobre HasMany)

`User.preload("posts").all(db)` resuelve N+1 con 1 query batch
(`SELECT * FROM posts WHERE user_id IN (1, 2, 3)`) + dispatch
estático del relation name en compile-time vía `match`. Helper
`emit_preload_dispatch` por type con `@has_many`. El relation name
como Str literal queda hard-coded en el binario — typos detectados
en compile-time, no runtime. Cierra la deuda "Eager loading
(`.include(...)`)" que arrastraba desde Fase 10.5.

#### 10.b.16 — Postgres en CI default (paridad real corre en cada push)

Job nuevo `db-postgres` en `.github/workflows/ci.yml` que levanta
`postgres:16` como service container, exporta
`FITZ_TEST_PG_URL=postgres://postgres:postgres@localhost:5432/
fitz_test`, y corre `cargo test --test db_real_postgres -- --ignored
--test-threads=1`. Solo Linux (Docker service containers más
estables en GHA Linux runners; los tests no dependen de plataforma
— el binario standalone es x86_64-linux). Los 16 paridad codegen
E2E + los 27 evaluator E2E ahora corren en cada push a `main`.
`#[ignore]` se mantiene para que `cargo test` default sin env var
siga rápido en local del autor.

#### 10.b.17 — Ejemplo guía `32-orm.fitz` pedagógico + smoke GUIDE

Nuevo `examples/guide/32-orm.fitz` (~100 LoC) que muestra el shape
canónico del ORM end-to-end: `@table` con `@primary` + `@column` +
`@belongs_to` + `@has_many`, insert, where + first, chain
order_by/limit/offset, operadores starts_with/is_in/between,
aggregates scalares count/avg, GROUP BY con `Aggregated<Row>`,
navigation belongs_to/has_many, eager loading con preload, y
update/delete con guard `.where(...)` obligatorio. Sumado al smoke
`GUIDE_EXAMPLES_COMPILE` (291 ejemplos compilan en cada push).
`fitz build` produce binario aunque no haya Postgres real — el
`connect` runtime falla con `Err` clara si la URL es inválida, así
el ejemplo es ejecutable como guía sin Postgres local.

Cierre formal de Fase 10.b acompañando el sub-paso: CHANGELOG
v0.10.1, esta sección con status CERRADA + stats finales,
deudas-post-5b.md con la deuda smoke GUIDE marcada ✅, CLAUDE.md
actualizado.

### v0.10.6 — Bloque W1-W7: workarounds residuales del ORM cerrados (2026-05-27)

Tras cerrar las 4 deudas grandes del ORM en v0.10.4/v0.10.5,
durante la actualización de ejemplos y boilerplates encontramos
7 workarounds menores que el user tropezaría al escribir código
real. Los 7 cerrados en bloque en v0.10.6, uno por commit:

- **W4** — `id: 0` con `@primary Int` skipea el field del INSERT
  para que Postgres asigne via `bigserial`/`IDENTITY DEFAULT`.
  Branch runtime `if __g.<pk> == 0` con dos SQLs alternativos
  (con/sin PK) en `gen_orm_type_insert`. Paralelo bit-a-bit al
  evaluator.
- **W5** — `db.close()` devuelve `Future<Result<Null>>` (antes
  `Future<Null>`). Helper preludio `__fitz_db_close` retorna
  `Result<(), String>`. Los docs ya prometían esta semántica
  desde v0.10.5 — ahora el código se alineó.
- **W7** — `.update(db, Map var)` además del literal. Nuevo
  `UpdateSetEmission { Static, Dynamic }`. Dynamic emite un
  closure IIFE con match runtime sobre `key.as_str()` ramificado
  por field del type, soporta primitivos + Map<...> (jsonb) +
  List<scalar> (arrays).
- **W3** — `.starts_with`/`.ends_with`/`.contains` aceptan var Str.
  Str literal mantiene escape Rust-side de `%`/`_`; var/expr
  envuelve SQL-side con `||` Postgres. Sin escape runtime para
  vars (igual que `.like(var)`).
- **W6** — `body.field` en closures de `.where`. Translator
  (evaluator + codegen) detecta field access sobre vars externas
  al `param_name` del closure y bindea como `$N`. Soporta chains
  arbitrarios (`req.inner.email`).
- **W1** — Map literal homogéneo context-aware. Nuevo wrapper
  `gen_map_lit_with_hint(pairs, span, hint)`. Si hint es
  `Map<_, Any>` (peleamos `Nullable` outer), force shape
  heterogéneo. Aplica en struct literals y `let` con anotación.
- **W2** — Nullable refinement en match arms. Dos correcciones:
  `Pattern::Null` sobre Nullable emite `None` (antes `_` —
  bug silencioso que matcheaba TODO); `Pattern::Ident` sobre
  Nullable emite `Some(name)` con `name: T` refinado. Checker
  estático también gana refinement flow-sensitive.

**Tests al cierre del bloque**: 2562 unit + 295 compile_e2e + 81
cli_e2e + 3 openapi + 46+ db_real_postgres. Smoke
`GUIDE_EXAMPLES_COMPILE` (292 ejemplos) verde. Clippy
`--all-targets -D warnings` limpio.

**Barrida de documentación**: workarounds removidos del prose de
`docs/db-orm.md` (sec 28 ahora marca los 7 como CERRADOS),
ejemplos pedagógicos (`examples/guide/31-orm.fitz` +
`31b-orm-crud-http.fitz`) actualizados con la sintaxis canónica.

**Próximo norte tras v0.10.6**: boilerplates ORM Dockerizados —
convertir `api-postgres-python` (SQLAlchemy) a Fitz ORM nativo
side-by-side, crear boilerplate nuevo dedicado al ORM full
(relations + JSONB + arrays + auth + ws + cron), benchmarks
Fitz ORM vs SQLAlchemy.

---

### Deuda residual derivada de 10.b (NO bloquea v0.10.1)

Estos items son refinements post-10.b. No bloquean la promesa de
paridad bit-a-bit cumplida en v0.10.1.

- **Migraciones automáticas (`fitz db diff`/`migrate`)**: roadmap
  original de Fase 10.6+. NO entra a 10.b. Hoy el user crea las
  tablas con `db.exec("CREATE TABLE ...")` al boot.
- ✅ **Eager loading (`.preload(...)`)** — CERRADO 2026-05-26
  (10.b.15). HasMany cubierto con dispatch estático match en
  compile-time. BelongsTo eager queda como refinamiento futuro
  (caso menos común).
- **Transactions** (`BEGIN`/`COMMIT`/`ROLLBACK`): cada query corre
  en auto-commit. Bloques transaccionales llegan en 10.7 separada.
- **Composite primary keys**: hoy solo un `@primary` por type.
- **Date/Time/UUID nativos como tipos del lenguaje**: hoy se
  modelan como `Str` ISO 8601. Tipos dedicados son mini-fase
  aparte.
- **TLS strict (`sslmode=require`)**: driver MVP solo soporta
  `disable`. TLS llega en sub-paso 10.1.b separado.
- **JSON operators (`->`, `->>`, `@>`)**: el JSONB se trae completo
  como Map<Str, Any> y se opera del lado Fitz, o se baja a SQL
  crudo via `db.query(...)`.
- **Async wave 1 chain methods**: refinable post-10.b si entra
  presión real.

---

## Fase 10.6 — Migraciones automáticas ORM ✅ CERRADA v0.10.16 (2026-05-29)

`fitz db diff/migrate/status/new` con introspección + diff
determinístico + tracking idempotente en `_fitz_migrations`.
Cierre completo del MVP + `@db_default("expr")` para defaults SQL
auto-emitidos. Cero deps externas (ni Alembic ni Flyway ni
Liquibase ni TypeORM CLI). Detalle técnico completo en
`CHANGELOG.md` v0.10.16.

---

## Fase 10.6.b → 10.6.e — Migraciones: completar el paquete estilo Alembic 📊

Plan de cierre del feature set de migraciones contra el baseline
Alembic (el más maduro del espacio). Agrupado por **tier de uso
real**, no por orden alfabético — Tier 1 es lo que más extraña la
gente en producción seria, Tier 3 son casos raros.

### Tier 1 — Gap visible que duele (≈ 1 release c/u)

**Fase 10.6.b — Forward/back + renames seguros** (en curso, próxima
release v0.10.17):

- **10.6.b.1 — `-- UP` / `-- DOWN` + `fitz db rollback [N]`**:
  sintaxis backward-compatible en `.sql` (archivos sin marcador
  → todo `-- UP` implícito, sin DOWN). `MigrationFile` suma
  `down_sql: Option<String>`. Subcomando nuevo `fitz db rollback
  [N]` (default `N=1`) lee last N migrations aplicadas (DESC por
  `applied_at`), ejecuta `-- DOWN` adentro de tx, borra registro
  de `_fitz_migrations`. Sin `-- DOWN` → aborta con mensaje claro
  citando filename. Stub de `fitz db new` actualizado para
  incluir ambas secciones por convención. Opcional: flag
  `--with-down` en `fitz db diff` para inferir DOWN del current
  → target.
- **10.6.b.2 — Renames vía `@renamed_from("old_name")`**:
  decorator transient sobre field o `@table`. El diff lo lee y
  emite `ALTER TABLE ... RENAME COLUMN "old" TO "new"` (o RENAME
  TABLE) en vez de `DROP + ADD`. Tras aplicar la migration el
  user borra el decorator (single-use). Por qué decorator vs
  subcomando: el subcomando divorcia rename del cambio en el
  code (fácil de olvidar uno); el decorator es declarativo y
  atómico.

**Fase 10.6.c — Drift check + stamping** ✅ CERRADA v0.10.18 (2026-05-29):

- ✅ **`fitz db check`**: corre el diff, exit 0 si sincronizado,
  exit 1 con SQL pendiente al stderr si hay drift. Hook clave
  para CI bloqueante. Smoke real Postgres validado end-to-end.
- ✅ **`fitz db stamp <version> [--all]`**: marca
  `_fitz_migrations` sin ejecutar el SQL. Adopción inicial en DB
  legacy. Idempotente (ya-applied → no-op). Warning sobre
  versions que no existen en el dir.
- ✅ **Driver fix OID 19 (`name` type de `pg_catalog`)**: tratar
  como Text. Destrabador crítico — sin esto, TODO `fitz db ...`
  que introspecciona fallaba con "tipo Postgres OID 19 no
  soportado" porque `information_schema.columns.column_name` es
  `sql_identifier` que es alias de `name`.

**Fase 10.6.d — Data migrations en `.fitz`** ✅ CERRADA v0.10.19 (2026-05-30):

- ✅ Discovery: `read_migrations_dir` acepta `.sql` y `.fitz`;
  intercala por orden alfabético del prefix timestamp.
- ✅ Modelo: `.fitz` declara `async fn migrate(db: DbConn) ->
  Result<Null>` (obligatoria) + `async fn rollback(db: DbConn)
  -> Result<Null>` (opcional). El runner valida pre-flight.
- ✅ Runner: parsea + verifica fn declarada + crea env con
  builtins + bindea `db` como `Value::DbConn` + appendea stmt
  sintético `let __mig_result = migrate(db).await` + eval con
  `evaluator::eval_program_with_env` + inspecciona Ok/Err en el
  env.
- ✅ Tracking: `track_fitz_migration_applied` + `untrack_fitz_migration`
  helpers en migrations.rs para INSERT/DELETE en `_fitz_migrations`.
- ✅ Dispatch CLI: `db_migrate_cmd` y `rollback_n_dispatch` en
  main.rs iteran por kind y delegan a `apply_migration` (Sql)
  o `apply_fitz_migration_async` / `revert_fitz_migration_async`
  (Fitz).
- ✅ Pre-flight rollback: chequea que cada target `.fitz` tenga
  `async fn rollback` declarada ANTES de tocar la DB (vía
  helper `fitz_migration_has_rollback` que parsea source-only).
- ✅ Atomicidad: `.fitz` NO se envuelve auto en tx (a diferencia
  de `.sql`); el user decide granularidad típicamente con
  `return db.transaction(fn(tx) -> Result<Null> { ... }).await`.
- ✅ Smoke E2E real Postgres local validado bit-a-bit: migrate
  `.sql` + `.fitz` mixtos → ambos aplican en orden → status OK
  → rollback `.fitz` → re-migrate → status OK. Plus error path:
  `.fitz` sin `rollback` fn aborta pre-flight.

### Tier 2 — Útil para teams grandes (≈ 1-2 releases c/u)

**Fase 10.6.e — History, offline SQL, squashing (parcial CERRADA v0.10.20 2026-05-30)**:

- ✅ **`fitz db history`**: log con `version` + `applied_at` + filename.
  Orden `applied_at DESC`. Si version applied sin file en dir,
  marca `(file removido)`. CERRADA v0.10.20.
- ✅ **`fitz db migrate --sql`**: offline mode. Emite SQL pendiente
  al stdout. Sigue conectando para leer tracking + skipear
  applied. Rechaza `.fitz` (no se materializan offline). CERRADA
  v0.10.20.
- ✅ **`fitz db squash <from> <to>`**: combina migrations del rango
  [from, to] en `<from>_squashed.sql`. Concatena UP + DOWN
  inverso. Mueve files originales a `migrations/squashed/`.
  Tracking inteligente: si alguna del range applied, borra todas
  + stampea `from`. Flag `--no-tracking` para CI-only. CERRADA
  v0.10.20.
- ✅ **Schemas custom (10.6.e.3) CERRADA v0.10.21 (2026-05-30)**:
  - Sintaxis `@table("schema.name")` (split por `.` en el
    parser del decorator). Validación: ambos segmentos no
    vacíos, sin whitespace, máximo 1 `.`.
  - `TableMetadata.schema: Option<String>` poblado por el
    checker; `None` = `public` (compat con código pre-v0.10.21).
  - `TableRef { schema, name }` nuevo en migrations: identidad
    cross-schema por `(schema, name)`. Diff por qualified_id.
  - Introspect schemas-aware: `list_user_tables_qualified`
    iterar TODAS las user schemas (excluye `pg_catalog`,
    `information_schema`, `pg_toast*`, `pg_temp_*`, `_fitz_migrations`).
    `introspect_columns`/`indexes`/`foreign_keys` parametrizados
    por schema.
  - Nuevo `Change::CreateSchema { name }` emitido PRIMERO en el
    diff (antes de CREATE TABLE en ese schema). Idempotente vía
    `IF NOT EXISTS`.
  - SQL emit qualified everywhere via helper `quote_qualified` +
    `TableMetadata::qualified_sql_name()` (`"schema"."name"`
    o `"name"`).
  - `__FitzQueryBuilder.table` (codegen preludio) ahora
    almacena la forma ya-quoteada (`"users"` o `"public"."x"`);
    los `format!` SQL del preludio cambian de `\"{}\"` a `{}`.
    ~7 sitios en preludio + ~6 en codegen + ~5 en evaluator.
  - Smoke E2E real Postgres local validado bit-a-bit:
    - `db check` con `analytics.events` + `users` (mixed):
      emite `CREATE SCHEMA IF NOT EXISTS "analytics";` +
      `CREATE TABLE "analytics"."events"` + `CREATE TABLE "users"`.
    - `db migrate` aplica todo correctamente.
    - `db check` post-migrate → `✓ schema sincronizado`.
    - ORM nativo `User.insert(conn, ...)` (public) Y
      `Event.insert(conn, ...)` (analytics) ambos retornan id=N.
    - `Event.all(conn)` SELECT contra `"analytics"."events"`
      devuelve rows correctamente.
- **Composite primary keys / CHECK constraints**: deuda del ORM
  desde v0.10.0 que migrations las usaría. Out of scope estricto
  de migrations pero entran acá si aterrizan en el ORM.

### Tier 3 — Casos raros (deuda explícita, sin plan inmediato)

- **Branches + merge migrations** (Alembic multi-heads): teams
  grandes que mergean ramas con migrations divergentes. Rails y
  Django no lo tienen y nadie llora.
- **Pre/post hooks**: callbacks custom antes/después de cada
  migration. Útil en monolitos enterprise.
- **Multi-database**: read replicas, sharding, DBs separadas.
- **Migration testing helpers** (`fitz db reset` + factories para
  fixtures).

### Out of scope estricto (features del ORM, no del migrator)

- Triggers / functions / stored procedures
- Materialized views
- Partitioning
- Full-text search index management custom

### Diferenciales de Fitz que Alembic NO tiene (refuerzan el pitch)

- **Cero deps externas**: `pip install alembic + sqlalchemy +
  psycopg2` vs binario `fitz` solo.
- **Schema desde código tipado del propio lenguaje**: Alembic
  genera desde SQLAlchemy models (otro layer). Fitz lo genera
  desde el lenguaje mismo, sin DSL paralelo.
- **Paridad bit-a-bit con el resto del stack**: el wire protocol
  no cambia entre `fitz run`, `fitz build`, y `fitz db ...`.

### Orden comprometido

1. ✅ **10.6.b** (rollback + renames) — v0.10.17 CERRADA.
2. ✅ **10.6.c** (drift check + stamp) — v0.10.18 CERRADA.
3. ✅ **10.6.d** (data migrations en `.fitz`) — v0.10.19 CERRADA.
4. ✅ **10.6.e.1+.2** (history + offline SQL + squash) — v0.10.20
   CERRADA.
5. ✅ **10.6.e.3** (schemas custom) — v0.10.21 CERRADA. **Cierre
   formal de Fase 10.6 entera**: el paquete migrations completo
   vs Alembic.
5. Tier 3 y out-of-scope quedan en deuda explícita; NO bloquean
   declarar "paquete completo de migraciones".

---

## Visión post-Fase 10 — Fase 11+ 🔮

**Estado al cierre v0.15.0 (2026-06-05)**: de las 3 fases
originalmente especulativas de esta sección, **2 ya cerraron**:

- ✅ **Fase 12 (Deployment ciudadano)** — CERRADA en v0.12.5 + v0.13.0.
- ✅ **Fase 13 (CLI builder)** — CERRADA en v0.11.0 + v0.11.1.
- 🔮 **Fase 11 (Frontend en `.fitz`)** — sigue como visión a futuro,
  no arrancada.

Las secciones de Fase 12 y 13 abajo se mantienen como **referencia
histórica** del diseño + cierre. La parte realmente especulativa del
roadmap se reduce hoy a:

| Item futuro | Estimación |
|---|---|
| Fase 11 (Frontend `.fitz` SFC + SSR) | meses, requiere ronda de diseño |
| V6 (DAP — debugging interactivo VSCode) | ~2 semanas, anotada en backlog |
| Fase 12.6+ targets extra (`fitz deploy fly/railway/k8s`) | 1-2 semanas por target |
| Deuda residual técnica menor | ver `docs/deudas-post-5b.md` |

> **Fase 10 (Stack DB nativo + ORM declarativo)** salió de esta
> sección al cerrar — ver la sección dedicada **Fase 10 — Stack
> DB nativo + ORM declarativo** arriba.

### Fase 11 — Frontend en `.fitz` (SFC + SSR)

**Promesa**: el mismo lenguaje en backend y frontend. Resuelve el
problema del **doble tipado** que el autor sufre todos los días
con Vue+FastAPI: definís `type User` en el backend, lo re-definís
en TypeScript, los tipos divergen, bugs en producción.

- **Single-file components**: `.fitz` con secciones
  `<template>`/`<script>`/`<style>` estilo Vue SFC.
- **Compilación a WASM o JS** — TBD según target. WASM para apps
  grandes, JS para apps chicas + SEO.
- **SSR built-in** — el mismo handler `@get("/users")` puede
  devolver JSON (API) o HTML renderizado server-side (página)
  según headers.
- **Sharing de `type`** entre backend y frontend — el
  `type User` se define una vez.
- **Reactividad**: signals (estilo Solid.js) o ref/reactive
  (estilo Vue 3). Decisión grande.

**Por qué importa**: **la apuesta más ambiciosa de Fitz**. Si
funciona, posiciona a Fitz como "el lenguaje que resuelve el
split frontend/backend" — un nicho que ningún lenguaje moderno
ataca de frente (Elixir + LiveView se acerca, Phoenix se acerca,
pero ambos requieren mucha JS para apps ricas).

**Por qué tarde**: es básicamente construir Fitz otra vez en el
navegador. Implementación grande. Antes hay que tener: package
manager (Fase 9.y), DX maduro (Fase 9.z), y al menos un stack DB
nativo (Fase 10) para validar que el lenguaje sirve para apps
completas.

**Inspiración**: Vue SFC + Svelte + Solid + Phoenix LiveView +
HTMX. Mezcla de las mejores ideas.

### Fase 12 — Deployment ciudadano primera clase ✅ CERRADA (v0.12.5 + v0.13.0)

> **Estado al cierre (2026-06-04)**: Fase 12 **completa** vive en
> producción desde v0.13.0. **Tier 1** cerrado en v0.12.5 con cap
> 35 entero + curso M7 + cierre formal. **Tier 2** cerrado en
> v0.13.0 (`fitz deploy` + `@trace`/`@metric` + `@flag`/`flag()`).
> Detalle por sub-fase abajo. El texto histórico de esta sección
> describe el diseño y el plan tal como se ejecutó.

**Promesa cumplida**: del repo a producción en 1 comando. El binario de
`fitz build` no solo corre — es **deployable end-to-end** sin
pegar mil archivos YAML / scripts shell / Dockerfile manuales.
Health checks built-in, graceful shutdown automático, secrets
manageables con tipos opacos, observability auto-instrumentada
con OpenTelemetry estándar, Dockerfile generado del shape del
programa. Cero `helm install` para infra básica.

**Estado histórico al planificar**: planificada con MVP comprometido.
Desbloquea el **cap M7 entero del curso** que estaba en espera.
Pre-reqs: ninguno bloqueante (SIGTERM via `tokio::signal::ctrl_c` ya
disponible; runtime multi-thread post-F17 cubre concurrencia).

**Deuda residual derivada de Fase 12** (NO bloquea uso real):

- **`fitz deploy` targets extra**: hoy solo `docker` y `compose`.
  Targets `fly`/`railway`/`k8s` quedan como deuda visible documentada
  — para esos, correr los CLIs directo. Estimación por target:
  1-2 semanas.

### Scope MVP (Tier 1) — sub-fases 12.1-12.5

**12.1 — Health checks + graceful shutdown** (3 sub-pasos)

Decoradores nuevos del lenguaje:

- `@healthz fn liveness() -> Bool` — liveness probe (K8s decide
  si reiniciar).
- `@readyz fn readiness() -> Bool` — readiness probe (K8s decide
  si rutear tráfico).
- Auto-mount `GET /healthz` + `GET /readyz` cuando se declaran
  (paralelo al `/openapi.json` autoregistrado).
- Defaults:
  - Sin `@readyz`: 200 si `db.is_closed() == false`.
  - Sin `@healthz`: 200 siempre.
- SIGTERM handler en `serve()`: al recibir SIGTERM,
  - `/readyz` empieza a retornar 503 (K8s deja de rutear).
  - Server stop accepting new conns; in-flight terminan.
  - WS conns reciben close frame `1001 Going Away`.
  - Cron scheduler espera jobs en curso a terminar.
  - Timeout configurable con `@server(shutdown_timeout_secs=N)`,
    default 30s.

Sub-pasos:
- **12.1.a** — Checker: parse `@healthz`/`@readyz`, validar fn
  shape, registrar en `HttpRegistry`.
- **12.1.b** — Runtime intérprete: auto-mount handlers,
  integrar SIGTERM handler, drain logic.
- **12.1.c** — Codegen `fitz build`: paridad bit-a-bit, emitir
  SIGTERM trap en el main generado, drain de cron/WS.

**12.2 — Secrets + config management** (2 sub-pasos)

API:

- `secret(key: Str) -> Result<Secret<Str>>` builtin. Multi-source
  con precedencia:
  1. Env var (`$DB_PASS`).
  2. Mounted file (`/run/secrets/DB_PASS`) — convención K8s/
     Docker secrets.
  3. `.env` file en cwd.
  4. Si nada: `Err("secret 'DB_PASS' no encontrado")`.
- `Secret<T>` **tipo opaco built-in**:
  - `Display`/`print()` emite `<redacted Secret<Str>>`. Cero way
    de leak accidental.
  - `Debug` también redacta.
  - `Secret<T>` no se serializa a JSON (`__ToFitzJson` retorna
    error explícito en codegen).
  - `.expose() -> T` desempaca explícito — para pasar al
    `jwt.encode`/`hash.password`/`db.connect`.
  - Comparación con `==` está OK (uso típico: validar
    passwords). Constant-time.
- `config(key: Str, default: T) -> T` builtin para valores
  no-sensitive:
  - Auto-coerciona según tipo del default: `config("PORT", 8080)`
    → Int; `config("DEBUG", false)` → Bool.
  - Multi-source: env var → `fitz.toml [config]` section →
    default.
- `env_or` existing sigue funcionando (deprecation suave —
  el cap del curso recomienda `config(...)` como reemplazo).

Sub-pasos:
- **12.2.a** — Tipo `Secret<T>` en checker + runtime + display
  redactado + builtins `secret`/`config`.
- **12.2.b** — Codegen paridad: emit `__FitzSecret<T>` con
  `Display` redactado + `expose()` + integración con
  `__ToFitzJson` (error claro citando `.expose()`).

**12.3 — Observability minimal con OpenTelemetry** — **CERRADA**
(2026-06-03)

Decisión confirmada: OTel estándar (OTLP) con sintaxis kwargs
`name: value`. Opt-out con `@server(observability=false)` (12.3.b.5).

Cierre formal con 11 commits a lo largo de 3 bloques. Total al
cierre: 2894 unit + 81 cli_e2e + 3 openapi_e2e + 4 compile_e2e
del logging. Clippy `--all-targets -- -D warnings` limpio,
`cargo fmt --all --check` limpio.

#### 12.3.a — Setup base + structured logging (CERRADA)

Builtins `log.info/warn/error/debug` con kwargs heterogéneos
(Int/Float/Str/Bool/Null/Secret/List/Map), `Secret<T>`
redactado automático, paridad bit-a-bit intérprete↔binario.

- **12.3.a.1**: módulo `log` como `Value::Module` + 4 builtins
  stub con `eprintln!` + dispatch de kwargs en
  `dispatch_builtin_kwargs` con kwargs reservados `level`/`msg`/
  `timestamp` rechazados. Stubs para validar el wiring; el
  output JSON real llega en 12.3.a.2. 14 unit tests.
- **12.3.a.2**: JSON estructurado real con `tracing` +
  `tracing-subscriber` (`EnvFilter::from_default_env()` con
  `RUST_LOG`, default `info`); TTY detection con override
  `FITZ_LOG_FORMAT=json|pretty`; pretty mode con ANSI bold
  colors por level (DEBUG=magenta/INFO=green/WARN=yellow/
  ERROR=red); JSON shape flat con `timestamp`+`level`+`msg`+
  kwargs al mismo nivel; redacción recursiva de `Value::Secret`
  en kwargs directos y dentro de `List`/`Map`; ChronoUtc RFC
  3339 millis. Nuevo módulo `src/logging.rs` (~470 LoC) +
  init_logging() en main.rs. 14 unit tests del módulo.
- **12.3.a.3**: codegen paridad bit-a-bit en `fitz build`.
  `LOGGING_PRELUDE` constante (~150 LoC del Rust emitido) con
  enum tagged `__FitzLogValue` (Null/Bool/Int/Float/Str/Secret
  marker/List/Map) + `__fitz_log_init` + 4 fns
  `__fitz_log_<level>` + format JSON/pretty. Deps emitidas:
  `tracing` + `tracing-subscriber` + `chrono` + `serde_json`
  (con dedup según otros flags). `gen_log_call` traduce
  `log.<level>(msg, k: v)` al helper preludio con conversión
  recursiva del tipo al enum. 16 unit tests + 4 E2E del binario
  nativo.

#### 12.3.b — Spans HTTP + métricas + correlación trace_id (CERRADA)

Auto-instrumentation HTTP completa sin opt-in del user. Cada
request abre un SpanContext root con IDs OTel-compatibles
(trace_id 32 hex / span_id 16 hex) y los logs heredan
automático. Al final del request, access log + Counter +
Histogram.

- **12.3.b.1**: infraestructura del `SpanContext` (struct +
  `tokio::task_local!` storage + `with_span_context()` +
  `current_span_context()`). `format_json`/`format_pretty`
  inyectan automático `trace_id`/`span_id` cuando hay span
  activo. Kwargs reservados extendidos a `level`/`msg`/
  `timestamp`/`trace_id`/`span_id`. IDs generados con
  `uuid::Uuid::new_v4()`. 12 unit tests nuevos.
- **12.3.b.2**: wrapper HTTP automático en `dispatch_request`.
  Envuelve `handle_task` con `with_span_context(ctx, ...)` para
  que logs del handler hereden trace_id. Al final del request,
  emite `log.info("http.access", ...)` con `http.method`/
  `http.target`/`http.status_code`/`duration_ms` (OTel naming).
  Access log adentro del scope para correlación con logs del
  handler. `http.target` con TEMPLATE del route (no path
  resuelto) — convención OTel.
- **12.3.b.3**: métricas built-in con `metrics = "0.24"` crate.
  Counter `http_requests_total{method, path, status}` +
  Histogram `http_request_duration_seconds{method, path,
  status}` registrados en `dispatch_request` adentro del scope
  del span. Sin recorder global instalado, macros son no-op
  silenciosas (zero overhead). 4 unit tests con
  `DebuggingRecorder`.
- **12.3.b.4**: codegen paridad bit-a-bit en `fitz build`.
  `LOGGING_PRELUDE` extendido con SpanContext + helpers; split
  preludio STUB/TOKIO según `has_tokio_runtime`. `uses_logging`
  implícito con HTTP. Deps `uuid` + `metrics` condicionales en
  el Cargo.toml emitido. Wrapper HTTP del binario nativo
  paralelo bit-a-bit al intérprete.
- **12.3.b.5**: opt-out `@server(observability=false)` parseado
  en evaluator + codegen. `dispatch_request` bypasea TODO el
  wrapper de instrumentación cuando flag está apagado —
  handlers bare-metal, cero overhead. Bug fix incluido: fix
  cargo fmt CI del 12.3.b.4 (`let has_tokio_runtime` condensado
  en una línea). Bug de orden corregido: `ctx.observability_enabled`
  capturado ANTES del loop `gen_http_handler_wrapper`. Smoke E2E
  con 4 escenarios verde: intérprete default/off + binario
  default/off bit-a-bit paralelos.

#### 12.3.c — OTLP exporter + sampling (CERRADA)

Conexión a backend OTel real (Jaeger/Tempo/Honeycomb/Datadog)
cuando `OTEL_EXPORTER_OTLP_ENDPOINT` está seteado. Sin la env
var, no-op silencioso — zero overhead, zero conexiones de red.

- **12.3.c.1**: setup base en intérprete. Crates
  `opentelemetry = "0.32"` + `opentelemetry_sdk = "0.32"` +
  `opentelemetry-otlp = "0.32"` con features `http-proto` +
  `reqwest-blocking-client` + `trace` (default-features off).
  Nuevo módulo `src/observability.rs` (~140 LoC) con
  `init_otel()` + `is_otel_enabled()` (OnceLock) + `tracer()`.
  Env vars OTel-standard: `OTEL_EXPORTER_OTLP_ENDPOINT` (sin
  default), `OTEL_SERVICE_NAME` (default `"fitz-app"`),
  `OTEL_TRACES_SAMPLER_ARG` (clamp `[0.0, 1.0]`, default 1.0).
  `dispatch_request` abre OTel span paralelo al SpanContext
  propio cuando provider instalado; cierra con `http.status_code`
  + `Status::Ok`/`Status::error()`. HTTP/proto transport sobre
  gRPC por simplicidad + compat proxy + recomendación
  Datadog/Honeycomb. 2 unit tests smoke.
- **12.3.c.2**: codegen paridad bit-a-bit en `fitz build`.
  `OTEL_PRELUDE` constante paralela a `crate::observability`.
  `emit_otel_prelude` method + llamada en `emit_main_rs_body`.
  `__fitz_otel_init` en el main generado después de
  `__fitz_log_init`. Wrapper HTTP del binario nativo emite el
  OTel span open/close con los mismos atributos del intérprete.
  Smoke E2E del binario nativo verde con endpoint dummy (puerto
  vacío): batch exporter silencia errores de red en background
  sin bloquear el handler.
- **12.3.c.3**: cierre formal de Fase 12.3 entera. Roadmap
  + deudas + README + CLAUDE refresh. Smoke
  `GUIDE_EXAMPLES_COMPILE` verde.

#### Deudas residuales derivadas de 12.3 (NO bloquean 12.4)

1. **Bridge métricas OTel** **— INTENTADO en Fase 12.3.iter2.Tier2
   (2026-06-03), BLOQUEADO por version conflict**. El crate
   `metrics-exporter-opentelemetry = "0.2.1"` (último release en
   crates.io) pinea `opentelemetry_sdk = "0.31"` mientras nosotros
   estamos en 0.32 para traces+logs. Tipos `MetricExporter`/
   `Resource`/`SdkMeterProvider` no unifican entre las dos
   versiones del SDK. Master del crate ya está en 0.32, esperando
   release oficial para reintentar. Workaround: Tier3 (Prometheus
   scrape) cubre 90% — OTel collector hace pull del `/metrics`.
2. ~~**Bridge logs OTel**~~ **CERRADO en Fase 12.3.iter2.b
   (2026-06-03)**: cuando `is_otel_enabled()` es `true` y el
   `LogExporter` se instaló, `emit_log_record` emite el LogRecord
   en paralelo al backend OTel via OTLP HTTP/proto (`/v1/logs`).
   Stderr logs intactos (emit ADITIVO). Trace context derivado
   del `SpanContext` → correlación logs↔spans automática en el
   backend. Secret values redactados. Paridad bit-a-bit codegen.
   Decisión arquitectónica: SDK directa, no
   `opentelemetry-appender-tracing` (no se justifica refactor
   del formatter custom JSON/pretty de 12.3.a).
3. ~~**Correlación trace_id Fitz↔OTel**~~ **CERRADO en Fase
   12.3.iter2.a (2026-06-03)**: `dispatch_request` abre el span
   OTel ANTES del `SpanContext` propio y deriva el `trace_id`/
   `span_id` desde el span OTel via
   `SpanContext::with_ids(...)`. El `trace_id` en logs stderr
   matchea el del backend (Jaeger/Tempo/Datadog) → queries
   cross-pipeline habilitadas. Paridad bit-a-bit codegen.
4. ~~**Endpoint `/metrics` Prometheus opcional**~~ **CERRADO en
   Fase 12.3.iter2.Tier3 (2026-06-03)**: `metrics-exporter-
   prometheus = "0.18"` con `default-features = false`. Dual
   gate: `@server(prometheus=true)` + env var
   `FITZ_PROMETHEUS=1` (env var override útil en producción).
   `serve()` instala el recorder + `build_router` auto-mounta
   `GET /metrics` con exposition format. Mismo puerto que la
   app. Paridad bit-a-bit codegen via `PROMETHEUS_PRELUDE`.

#### 12.3.iter2 — Cierre de deudas residuales de 12.3 (en curso)

Mini-tanda dedicada a cerrar las deudas más visibles de 12.3
antes de saltar a 12.4. Cada sub-paso es un commit con
validación completa.

- **12.3.iter2.a — Correlación `trace_id` Fitz↔OTel (CERRADA
  2026-06-03)**. `dispatch_request` (intérprete) y el wrapper
  HTTP del codegen abren el span OTel ANTES del `SpanContext`
  propio. Cuando hay OTel activo, el SpanContext se construye
  via nuevo constructor `SpanContext::with_ids(trace_id,
  span_id)` derivando los IDs del span OTel
  (`sctx.trace_id().to_string()` + `sctx.span_id().to_string()`).
  El `trace_id` en logs stderr/JSON es EL MISMO que el del
  backend OTel (Jaeger/Tempo/Honeycomb/Datadog) — habilita
  cross-pipeline queries. Sin OTel activo, `new_root()` sigue
  generando uuids frescos. Paridad bit-a-bit `fitz run` ↔ `fitz
  build`. **2 unit tests nuevos**: `logging::iter2a_span_context_
  with_ids_preserva_ids_pasados_y_parent_es_none` (constructor
  API) + `codegen::iter2a_codegen_http_emite_with_ids_branch_
  derivada_de_otel_span` (paridad codegen — verifica que el
  wrapper emite el branch `with_ids` derivada del span OTel +
  fallback `new_root` para `!is_otel_enabled`). Doc comments
  de `src/observability.rs` actualizados marcando la deuda como
  cerrada. Total al cierre: **2896 unit** (+2 vs 2894 del cierre
  Fase 12.3). Clippy `--lib --tests --bins -- -D warnings`
  limpio, fmt clean. Smoke `GUIDE_EXAMPLES_COMPILE` verde.

- **12.3.iter2.Tier2 — Bridge métricas OTel (INTENTADO 2026-06-03,
  BLOQUEADO por version conflict, ABIERTO)**. El crate
  `metrics-exporter-opentelemetry = "0.2.1"` (último release en
  crates.io, 2025-11-15) pinea `opentelemetry_sdk = "0.31"`,
  pero nosotros usamos `0.32` desde 12.3.c. El árbol de deps
  Cargo no unifica las dos versiones del SDK (`MetricExporter`,
  `Resource`, `SdkMeterProvider` son tipos DISTINTOS bajo el
  mismo nombre — errores `E0277` + `E0308` al intentar
  conectar el `MetricExporter` de `opentelemetry-otlp 0.32` al
  `MeterProvider` de `metrics-exporter-opentelemetry 0.2.1`).

  Master del crate ya está en 0.32 (verificado en
  https://github.com/Noelware/metrics-exporter-opentelemetry/blob/master/Cargo.toml).
  Esperando release oficial — probable v0.3.x.

  **Diseño listo** (no toca codegen): `serve()` llama
  `init_otel_metrics()` DESPUÉS de `init_prometheus()` para que
  Prometheus tenga precedencia cuando ambos activos (solo UN
  recorder global de `metrics`); instala `SdkMeterProvider`
  con `MetricExporter` OTLP sobre `/v1/metrics` + reader
  periódico; instala `metrics_exporter_opentelemetry::Recorder`
  como global. Cuando el crate ship una versión 0.32-compatible,
  el cierre debería ser ~50 LoC.

  **Workaround**: Tier3 (Prometheus) cubre el caso 90%. OTel
  collector hace scrape del `/metrics` endpoint del binario.
  Pierde el beneficio "single sink OTLP push" pero funciona
  end-to-end (collector consolida traces + logs OTLP + metrics
  scrape Prometheus).

- **12.3.iter2.Tier3 — Endpoint `/metrics` Prometheus (CERRADA
  2026-06-03)**. Cierra la deuda residual #4 de Fase 12.3.
  `metrics-exporter-prometheus = "0.18"` con `default-features = false`
  (skipea `http-listener` que armaría su propio HTTP server, y
  `push-gateway` que no es el caso 90%). Solo usamos
  `PrometheusBuilder::new().build_recorder() + handle.render()`.

  Dual gate de activación: (a) `@server(prometheus=true)` como
  flag compile-time, parseado por evaluator + codegen, populando
  `ServerConfig.prometheus_enabled: bool` (default `false`);
  (b) env var `FITZ_PROMETHEUS=1`/`true`/`yes` como runtime
  override — el flag y la env var se combinan con OR, así que
  activar cualquiera de los dos enciende el endpoint. El env var
  es útil en producción para activar/desactivar sin recompilar.

  Cuando activo, `serve()` llama a `observability::init_prometheus`
  que instala `PrometheusBuilder` como recorder global del crate
  `metrics`. Los `metrics::counter!("http_requests_total", ...)`
  + `metrics::histogram!("http_request_duration_seconds", ...)`
  que YA emite `dispatch_request` (desde 12.3.b.3) empiezan a
  popular el recorder Prometheus automático. `build_router`
  auto-mounta `GET /metrics` con response que llama
  `handle.render()` para producir el exposition format
  Prometheus (text/plain con content-type
  `text/plain; version=0.0.4; charset=utf-8`). El endpoint vive
  en el MISMO puerto + transporte que el resto de la app (NO un
  puerto separado) — menos surface área, menos config (port
  forwarding/firewall rules). Si el user declaró su propio
  `@get("/metrics")`, gana — mismo patrón que `/openapi.json`/
  `/healthz`.

  Paridad bit-a-bit `fitz run` ↔ `fitz build`: codegen emite un
  `PROMETHEUS_PRELUDE` nuevo con `__FITZ_PROMETHEUS_HANDLE`
  static + `__fitz_init_prometheus(compile_time_enabled: bool)`
  (dual gate paralelo) + `__fitz_prometheus_route() ->
  axum::Router` (retorna sub-Router con `/metrics` cuando handle
  instalado, vacío cuando no). `gen_http_main` invoca
  `__fitz_init_prometheus(<cfg.prometheus_enabled>)` después de
  `__fitz_log_init` y `__fitz_otel_init`, y emite `.merge(
  __fitz_prometheus_route())` al construir el Router (Router
  vacío se merge sin efecto, equivalente a no agregar nada).
  Cargo.toml emitido suma `metrics-exporter-prometheus = "0.18"`
  con `default-features = false` cuando `has_http`.

  Decisión deliberada: el dep va en Cargo.toml para CUALQUIER
  programa HTTP, no solo cuando `@server(prometheus=true)`. Eso
  habilita el path del env var sin necesidad de recompilar
  (caso típico de producción: deployar con `@server` sin la
  flag, después activar `FITZ_PROMETHEUS=1` en el container
  config). Costo: dep pequeña (~few hundred KB), compile-time
  impact moderado (smoke ~290 ejemplos tarda ~5-6 min con
  cache caliente). Trade-off aceptado.

  5 unit tests nuevos: 3 del evaluator (parsing del kwarg
  `prometheus`) + 2 del codegen (emisión del PROMETHEUS_PRELUDE
  + init call con flag false/true).

  Tests E2E con scrape real omitidos — requeriría arrancar el
  server real y hacer GET /metrics. La validación se hizo a
  mano fuera de los tests automatizados.

- **12.3.iter2.b — Bridge logs OTel (CERRADA 2026-06-03)**.
  Cuando `is_otel_enabled()` está activo Y el `LogExporter` se
  instaló correctamente, `emit_log_record` (intérprete) y
  `__fitz_log_emit` (codegen) emiten el LogRecord en paralelo
  al backend OTel via OTLP HTTP/proto al endpoint `/v1/logs`.
  Logs en stderr siguen intactos (emit ADITIVO, no reemplazo).
  El LogRecord exportado lleva: severity (text + numérico),
  body (msg), observed_timestamp (`SystemTime::now()`),
  attributes (kwargs Fitz → AnyValue OTel preservando shape
  de List/Map), y trace context derivado del `SpanContext`
  activo. Como el `SpanContext` ya está sincronizado con el
  span OTel adentro de un request HTTP (cierre iter2.a), esto
  habilita **correlación automática logs↔spans** en el backend
  (Jaeger/Tempo/Datadog muestran los logs del request al
  drill-down del span). Valores `Secret` se redactan a `"***"`
  consistente con stderr.

  Decisión arquitectónica: SDK `opentelemetry::logs` directa
  (LoggerProvider en static `OnceLock`). El path canónico
  recomendado era `opentelemetry-appender-tracing` (tracing
  layer que captura `tracing::event!()`), pero requiere
  refactorizar el formatter custom JSON/pretty de 12.3.a a
  una impl de `FormatEvent` — costo no justificado. Nuestra
  emisión a stderr usa write directo, no eventos tracing;
  paralelamente emitimos al SDK OTel con la misma información.

  Paridad bit-a-bit `fitz run` ↔ `fitz build`: en codegen,
  `OTEL_PRELUDE` se extendió con `__FITZ_OTEL_LOGGER_PROVIDER`
  static + impl real de `__fitz_emit_log_to_otel` +
  `__fitz_logvalue_to_any_value`. Cuando OTEL_PRELUDE no se
  emite (CLI puro con log.*), `LOGGING_OTEL_NOOP_STUB` incluye
  el stub no-op para que `__fitz_log_emit` siempre tenga el
  helper disponible. El SpanContext struct + helpers se
  extrajeron a una constante separada `SPAN_CONTEXT_STRUCT`
  para emisión limpia. Cargo.toml emitido suma feature `logs`
  a los 3 crates OTel.

  4 unit tests nuevos: `logging::iter2b_value_to_any_value_
  primitivos_mapean_directo` + `_secret_se_redacta` +
  `_list_y_map_son_recursivos` (helper de conversión Value Fitz
  → AnyValue OTel) + `codegen::iter2b_codegen_cli_log_emite_
  stub_no_op_de_emit_to_otel` + `_http_log_emite_logger_
  provider_real_y_log_exporter` (paridad codegen).

  Sin tests E2E con backend OTel real (requiere collector
  vivo + InMemoryExporter complejo de instalar). Validación
  smoke `GUIDE_EXAMPLES_COMPILE` verde + manual con curl
  contra el binario emitido para verificar que no hay
  regresiones de comportamiento.

**12.4 — Dockerfile autogenerado + `fitz docker`** (2 sub-pasos)

Sub-comando nuevo `fitz docker init` produce 3 archivos:

- **`Dockerfile`** multi-stage: builder rust:1.95 → runtime
  `gcr.io/distroless/cc-debian12`.
- **`.dockerignore`** con `target/`, `.git/`, `*.fitz` (excluye
  source code del binario final).
- **`docker-compose.yml`** smart por defecto.

Smart Dockerfile detection del shape del programa:

- Si declara `@server(port)` → `EXPOSE <port>`.
- Si usa `secret(...)` → comentario sobre `/run/secrets/` mount.
- Si tiene Python bundleado (Fase 8.b) → fallback a
  `debian:bookworm-slim` automático.

Smart compose.yml:

- Si usa `db.connect(env_or("DATABASE_URL", ...))` → suma
  service `postgres:16-alpine` con healthcheck.
- Si tiene `@cron` → `restart: unless-stopped`.
- Si tiene `@healthz/@readyz` → `healthcheck:` agregado en
  compose.

Sub-comando `fitz docker build [--tag X]` — wrapper que invoca
`docker build` con tags del `package.name`.

Sub-pasos:
- **12.4.a (CERRADA 2026-06-03, v0.12.2)** — `fitz docker init
  [--force]` con templates fijos + detección AST-only de
  `@server(port)` (Int literal) y `db.X(...)` (heurística generosa
  paralela a `program_uses_db` del codegen). Nuevo módulo
  `src/docker.rs` (~520 LoC con tests) expone `DockerShape`,
  `detect_shape`, `render_dockerfile`/`render_dockerignore`/
  `render_compose`, e `init(target_dir, shape, force) -> InitResult`
  con política skip-por-default + sugerencia de `--force`. CLI suma
  `Commands::Docker(DockerCmd::Init { force })` con sub-enum dedicado
  (deja la puerta abierta a `fitz docker build` de 12.4.b sin breaking
  change); handler `docker_init_cmd` (~95 LoC) reusa
  `resolve_entry(None)` para walkear hasta `fitz.toml`. 18 unit tests
  + 6 E2E (`cli_e2e`). Smoke real validado contra
  `boilerplates/api-simple` (HTTP, no DB) y `boilerplates/api-postgres-fitz`
  (HTTP + DB con compose smart). Tests al cierre: 2924 unit (+18) +
  87 cli_e2e (+6) + 3 openapi_e2e. Decisiones técnicas: (a) AST-only
  del entry point (fast ~50ms vs ~2s del eval, no cross-module);
  (b) runtime distroless siempre en 12.4.a (Python interop fallback
  diferido a 12.4.b); (c) `ports:` en compose siempre que haya
  `@server`; (d) compose con db sin `restart:` policies en 12.4.a
  (diferido a 12.4.b según `@cron`). Deudas residuales derivadas
  documentadas: cross-module detection, falso positivo de `db` local,
  detección Python interop, healthchecks `@healthz/@readyz` y
  restart-policies `@cron`, `fitz docker build` wrapper.
- **12.4.b (CERRADA 2026-06-03, v0.12.3)** — Smart detection rica +
  `fitz docker build` wrapper. Cierra **Fase 12.4 entera**. Sub-pasos:
  - **12.4.b.1** — `DockerShape` gana 2 campos (`uses_python`,
    `uses_cron`) + `Default` derive. Helpers nuevos `stmt_uses_python`
    (mira `Stmt::Import`/`FromImport` con `path[0] == "python"`) y
    `stmt_uses_cron` (mira `Stmt::FnDef.decorators` con `name ==
    "cron"`). `render_dockerfile` consulta `runtime_image(shape)` que
    devuelve `"python:3.12-slim-bookworm"` cuando `uses_python` (el
    binario emitido por `fitz build` con interop necesita
    `libpython3.12.so` que distroless no incluye; slim-bookworm trae
    libpython + wget) o `"gcr.io/distroless/cc-debian12"` (default).
    `render_compose` suma `restart: unless-stopped` cuando `uses_cron`
    y healthcheck HTTP contra `/healthz` cuando `server_port = Some` Y
    `uses_python` (wget disponible); con distroless emite comentario
    explicativo con la receta para agregarlo a mano. Handler
    `docker_init_cmd` reporta los nuevos detectados. 13 unit tests
    nuevos + 4 E2E nuevos.
  - **12.4.b.2** — Sub-enum nuevo `DockerCmd::Build { tag: Option<String> }`.
    Handler `docker_build_cmd(tag)` reusa `resolve_entry(None)` +
    valida `Dockerfile` existe + invoca `std::process::Command::new(
    "docker") build -t <tag> .` en `manifest_dir` con propagación del
    exit code. Default `--tag` = `<package.name>:latest`. Aborta con
    mensaje claro cuando falta Dockerfile (sugiere `fitz docker init`)
    o `fitz.toml`. 2 E2E nuevos.
  - Tests al cierre: 2937 unit (+13) + 93 cli_e2e (+6) + 3 openapi_e2e.
    Clippy `--lib --tests --bins -- -D warnings` limpio, fmt clean.
  - Decisiones técnicas: (a) runtime swap atómico al detectar interop
    Python — alternativa rechazada de "siempre distroless + bundle
    libpython" requeriría empaquetar libpython.so a mano, deuda mayor;
    (b) healthcheck con `wget --spider` solo en slim-bookworm —
    distroless sin shell no permite `CMD-SHELL`, opciones rechazadas
    incluían bundlear mini-probe (deuda separada) y healthcheck TCP
    (no valida endpoint exacto, sí estado del socket); (c) thin wrapper
    `fitz docker build` sin `--push`/`--platform`/`--no-cache` — el
    user que necesita flags advanced corre `docker build` directo.
  - Smoke real validado contra `boilerplates/api-postgres-python`
    (interop SQLAlchemy): runtime `python:3.12-slim-bookworm`
    automático + healthcheck HTTP en compose.
  - Deudas residuales derivadas: detección DB indirecta vía interop
    Python (no dispara `uses_db`), healthcheck HTTP sin distroless,
    `fitz docker build` thin (sin flags avanzados), cross-module
    detection heredada de 12.4.a.

**12.5 — Guía + curso M7 + cierre formal** (3 sub-pasos) **— CERRADA
2026-06-03 (v0.12.5). Cierra Fase 12 entera.**

- **12.5.a (CERRADA, v0.12.5)** — Cap 35 nuevo "Deployment ciudadano
  primera clase" en `docs/guide.md` con 8 sub-secciones (panorama,
  healthz/readyz, Secret + config, observability, Docker, ejemplo
  runnable, 12-factor, deudas). Vista integradora cross-link al cap 33
  para detalle de OTel. Ejemplo runnable `examples/guide/35-deploy.fitz`
  (<100 LoC con @server + @auth_provider + @admin + @requires +
  @healthz + secret() + config() + log.info estructurado). Sumado al
  smoke `GUIDE_EXAMPLES_COMPILE` (357 ejemplos, verde). Renumeración
  caps 36 (Plantillas) + 37 (Qué sigue).
- **12.5.b (CERRADA, v0.12.5)** — Caps del curso M7.C1-C4 completos en
  `docs/curso/m7-produccion-deploy/`:
  - **M7.C1 — `c1-distribucion-binarios.md`** (Distribución avanzada
    — cross-compile gratis vía rustc targets, `--bundle-python`/
    `--bundle-pip`, optimización con strip/LTO/UPX, inspección con
    ldd/dumpbin).
  - **M7.C2 — `c2-observability-otel.md`** (logs estructurados,
    spans HTTP auto con trace_id propagado, métricas Prometheus,
    bridge OTLP con Jaeger local, patterns de production).
  - **M7.C3 — `c3-secrets-config.md`** (distinción config()/secret(),
    `Secret<T>` opaco con `.expose()` explícito, redacción recursiva,
    `load_env(.env)` para dev, K8s secrets + fly/Railway/Heroku
    patterns).
  - **M7.C4 — `c4-deploy-docker-k8s.md`** (`fitz docker init`/`build`,
    healthz/readyz auto-mount + custom overrides, SIGTERM drain con
    30s grace, rolling deploy K8s, tabla 12-factor compliance, cierre
    del curso entero con resumen de los 10 diferenciales).
  - `docs/curso/index.md` marca M7 ✅ cerrado; `mkdocs.yml` suma
    la nav del módulo.
- **12.5.c (CERRADA, v0.12.5)** — Cierre formal: CHANGELOG v0.12.5
  detallado, roadmap actualizado (esta entrada), deudas-post-5b.md
  con nota de cierre, CLAUDE.md, README.md/index.md, smoke
  `GUIDE_EXAMPLES_COMPILE` cubre el ejemplo nuevo. Sin cambios de
  código — release 100% docs.

**Cierre formal de Fase 12 entera**: deployment ciudadano primera
clase está completo. Los 5 sub-pasos cumplen el plan original al
100%. Total de Fase 12 al cierre: ~2000 LoC docs nuevas (caps de
guía + M7 entero) + 1 ejemplo runnable + 2 sub-comandos CLI nuevos
(`fitz docker init` + `fitz docker build` de Fase 12.4) + tabla
auth refinada con `@requires` (Fase 9.w.1.iter2.a paralela).

### Tier 2 — CERRADO en bloque coordinado v0.13.0 (2026-06-04)

Los tres sub-pasos del Tier 2 cerraron juntos en un release
coordinado tras detectar suficiente demanda interna para
justificar el bloque entero. Plan original cumplido al 100%.

- **12.6 — `fitz deploy` orchestrator** ✅ CERRADO. Sub-comando
  nuevo `fitz deploy <target>` en CLI. Targets MVP: `docker`
  (build + push opt-out con `--no-push`) y `compose` (up local;
  `--no-detach`/`--no-build` opt-outs). Thin wrappers sobre
  `docker build` + `docker compose up`. Aborta con sugerencia
  clara si falta `Dockerfile`/`docker-compose.yml` (recomienda
  `fitz docker init`). Propaga exit codes para CI. Targets
  extendibles (`fly`, `railway`, `k8s`) quedan diferidos a
  Fase 13+ por demanda real. Módulo nuevo `src/deploy.rs`
  (~430 LoC + 7 unit + 5 cli_e2e tests).

- **12.7 — `@trace`/`@metric` decoradores explícitos** ✅
  CERRADO. Decorators apilables sobre fns user (rechazados
  sobre HTTP/WS — auto-instrumentation Fase 12.3 cubre esos
  casos con span + access log + métricas automáticos).
  `@trace(name="X")` abre `tracing::info_span!("X")`;
  `@metric(name="X")` registra `<name>_duration_seconds`
  (histogram) + `<name>_calls_total` (counter) al Drop del
  scope vía `__FitzMetricGuard` RAII (funciona con `return X`
  explícito sin código muerto). Kwarg `name=` opcional sobre
  cada uno (fallback al nombre de la fn). Paridad bit-a-bit
  `fitz run` (no-op honesto en evaluator) ↔ `fitz build`
  (instrumentación real con `tracing`+`metrics` crates
  linkeados). Cap 33.5 nuevo en guía + ejemplo runnable
  `examples/guide/34-trace-metric.fitz`.

- **12.8 — Feature flags built-in** ✅ CERRADO. Tres piezas:
  (a) decorator `@flag("name")` sobre HTTP/WS handlers que
  retorna 404 si la flag está off (gate hot path antes de
  middlewares/auth); (b) builtin global `flag(name) -> Bool`
  para branches dentro del código; (c) módulo `flags` con
  `is_enabled(name)` (alias) y `list()` (enumera flags
  conocidos en orden BTreeSet). Dos fuentes: sección
  `[flags]` en `fitz.toml` (defaults compile-time, baked-in
  al binario via `__fitz_flag_init(...)` al boot) + env vars
  `FITZ_FLAG_<UPPERCASE>` (override runtime sin recompilar,
  acepta `1`/`0`/`true`/`false`/`yes`/`no`/`on`/`off`).
  Default `false` (fail-safe — features opt-in). Paridad
  bit-a-bit `fitz run` ↔ `fitz build` con registry estático
  `OnceLock` + cache lookup. Cap 33.11 nuevo en guía +
  ejemplo runnable `examples/guide/34b-feature-flags.fitz`.

**Tests al cierre v0.13.0**: 3001 unit (+44 nuevos) + 112 LSP
+ 360 compile_e2e (+2 ejemplos guía nuevos + smoke verde) + 3
openapi. `cargo fmt --all --check` + `cargo clippy --lib
--tests --bins -- -D warnings` limpios.

**Extensión VSCode bumpeada a 0.13.0**: LSP completions
sumadas para `@trace`/`@metric`/`@flag` decorators, `flag()`
global builtin, `flags.X` after-dot. Grammar TextMate sin
cambios (decorators caen bajo `@<ident>` genérico).

**Próximo norte post-Tier2**: Fase 13+ según demanda real
(orquestación distribuida, multi-tenant, plugin architecture
para deploy targets `fly`/`railway`/`k8s`). Sin presión
inmediata.

### v0.13.2 (2026-06-04) — Bugfix LSP: positionEncoding UTF-16 (extensión 0.13.1 inservible)

Bug crítico descubierto durante el curso M1.C1 reportado por un
usuario haciendo la instalación: la extensión VSCode 0.13.1 no
podía conectarse al language server en VSCode fresh con error
`Unsupported position encoding (utf-8)` y la frase "Fitz Language
Server crashed 5 times in 3 minutes".

**Causa**: v0.9.51 declaró `position_encoding: utf-8` en
`capabilities` del initialize response. El cliente
`vscode-languageclient@9.0.1` hard-codea
`generalCapabilities.positionEncodings = ['utf-16']`
(`client.js:1370`) y rechaza cualquier encoding distinto de
`utf-16`/`undefined` (`client.js:835`). Handshake fallaba antes
de poder hablar JSON-RPC. El binario `fitz.exe` no estaba
afectado — `fitz run`/`build`/`check` funcionaban normal; solo
rompía la extensión.

**Fix completo** (Opción B — sin deuda residual):

- Server omite `position_encoding` (default LSP = UTF-16).
- `position_to_offset` / `offset_to_position` (`src/lsp.rs`)
  migrados de contar chars Unicode a UTF-16 code units vía
  `ch.len_utf16()`. Tolerancia defensiva con `>=` para
  mid-surrogate de cliente mal comportado (VSCode no genera
  ese caso).
- Helper nuevo `utf16_to_unicode_char(text, line, char_utf16) -> u32`
  (pub) traduce el `character` del cliente a chars Unicode
  1-based del lexer. Necesario porque `TypeInfo` /
  `DefinitionInfo` siguen indexados por chars Unicode (heredado
  de F16 + `lexer.rs::advance`).
- Backend handlers (`fitz-lsp.rs`): `hover` y `goto_definition`
  traducen `pos.character` con el helper antes de llamar a
  `hover_for_position` / `definition_for_position` /
  `ident_under_cursor` / `make_hover_with_range`.
- `detect_completion_context` traduce `recv_col` interno post-
  `offset_to_position` antes de armar
  `CompletionContext::AfterDot`, así el lookup en TypeInfo
  funciona aunque haya SMP en la línea antes del receiver.

**Tests nuevos** (`src/lsp.rs::tests`): 7 tests nuevos (el viejo
`position_to_offset_cuenta_chars_unicode_no_utf16_code_units`
invertido a `_cuenta_utf16_code_units_no_chars_unicode`, +
`_tolera_mid_surrogate`, `offset_to_position_emoji_retorna_utf16_units`,
`utf16_to_unicode_char_identidad_para_ascii`,
`utf16_to_unicode_char_colapsa_smp`,
`utf16_to_unicode_char_multilinea`,
`detect_context_after_dot_traduce_recv_col_con_smp_antes`).

Soporta chars del Supplementary Multilingual Plane (emoji,
símbolos matemáticos avanzados) sin off-by-one en hover/
definition/completion. **Deuda "UTF-16 position strict" CERRADA
por completo** — no se re-abre.

**Deuda residual cosmética** (NO afecta navegación funcional):
`make_definition_location` y `ident_range_from_def` retornan
`Range` LSP con char en chars Unicode (no UTF-16). En práctica
char_unicode == char_utf16 en líneas de def porque keywords +
identifiers son ASCII por reglas del lexer (deuda menor
documentada en `docs/deudas-post-5b.md`).

**Docs**: cap C1 del curso M1 suma dos entradas de
troubleshooting: `vcruntime140.dll no se encuentra` (VC++ Redist
falta en Windows fresh, afecta ambos binarios) y
`Unsupported position encoding (utf-8)` (bug específico 0.13.1,
fix en 0.13.2).

**Tests al cierre v0.13.2**: 3121 lib (+8 vs v0.13.1) + 112 LSP +
360 compile_e2e + 3 openapi. fmt + clippy `--lib --tests --bins
--features lsp -- -D warnings` limpios.

### v0.13.1 (2026-06-04) — Smoke gating de deps emitidas (deuda boring cerrada)

Refinamiento del Cargo.toml emitido por `cargo_toml_for` para que
las deps pesadas se gateen por el flag específico que las
activa, NO por `has_http` genérico. Cierre de la deuda
"Smoke compile_e2e — gating de deps emitidas" abierta en
v0.12.1 cuando Tier3 sumó `metrics-exporter-prometheus = "0.18"`
(commit `ci: bumpear timeout 15→25 min`, 2026-06-03).

**Concretamente**:

- `metrics-exporter-prometheus` solo se emite cuando hay
  `@server(prometheus=true)` literal en código. Detector nuevo
  `program_uses_prometheus_export(program)` paralelo a
  `program_uses_trace_metric`. Propagado a `CodegenCtx` +
  `cargo_toml_for` (param nuevo `uses_prometheus_export`,
  último positional). 20 call sites de tests actualizados.
- `emit_prometheus_prelude` + `__fitz_init_prometheus(...)` call
  + `.merge(__fitz_prometheus_route())` gateados por el mismo
  flag (paralelo bit-a-bit).
- **Breaking behavior**: el path env var `FITZ_PROMETHEUS=1` ya
  no funciona como override de runtime — exige
  `@server(prometheus=true)` literal. Documentado en
  `docs/guide.md` cap 33.4. Trade-off aceptado: el opt-in
  compile-time cubre el 95% del caso real.

**Decisión de scope confirmada**: las deps OTel siguen
emitidas con `has_http` (sin cambio) — el wrapper HTTP del
codegen emite `__fitz_with_span_context(...)` +
`__fitz_log_info("http.access", ...)` + branches sobre
`__fitz_otel_is_enabled()` sin opt-in del user (la línea
`uses_logging = has_http || ...` fuerza el preludio entero
cuando hay HTTP). Removerlas requiere también gatear el access
log auto del wrapper — queda como deuda residual separada
abierta en `docs/deudas-post-5b.md` ("Smoke compile_e2e —
gating de OTel deps + access log auto").

**Tests al cierre v0.13.1**: **3003 unit (+2 nuevos vs v0.13.0)** + 112
LSP + 360 compile_e2e + 3 openapi. `cargo fmt --all --check` +
`cargo clippy --lib --tests --bins -- -D warnings` limpios.

**Timing**: baseline pre-fix local Windows fresh
~522s (~8.7 min) sobre 360 ejemplos. CI Linux fresh runner
proporciona mayor mejora absoluta — el cold-compile de
`metrics-exporter-prometheus` + transitivos cae completo en
~95% de los ejemplos.

### Resumen de archivos del lenguaje a tocar

| Componente | Cambio | Sub-fase |
|---|---|---|
| `src/types.rs` | Tipo `Secret<T>` + checker para `@healthz`/`@readyz` | 12.1, 12.2 |
| `src/evaluator.rs` | Builtins `secret`/`config`/`log.*` + SIGTERM | 12.1, 12.2, 12.3 |
| `src/http.rs` | Auto-mount healthz/readyz + drain logic + OTel instrumentation | 12.1, 12.3 |
| `src/codegen.rs` | Paridad bit-a-bit de todo lo anterior | 12.1, 12.2, 12.3 |
| `src/cron_jobs.rs` | Drain logic al SIGTERM | 12.1 |
| `src/main.rs` | Sub-comando `fitz docker init`/`build` | 12.4 |
| `src/docker.rs` (nuevo) | Templates + smart detection | 12.4 |
| `Cargo.toml` | Deps: `opentelemetry-otlp`, `tracing-opentelemetry`, `dotenvy` | 12.2, 12.3 |

### Riesgos identificados y mitigaciones

1. **OTel SDK pesa ~10MB**: opt-out con
   `@server(observability=false)` para programas que no lo
   necesitan. Binario sin OTel queda como antes (~30MB).
2. **distroless no tiene shell**: para debug, el cap del curso
   documenta `docker run -it --entrypoint=/busybox/sh` con
   sidecar busybox temporal. Alternativa: flag `--base=alpine`.
3. **Auto-trace puede ser ruidoso**: sampling head-based
   default 100% en dev, 10% en prod via env var. Fácil de
   tunear.
4. **`Secret<T>` rompe paridad con tipos JSON**: si un handler
   retorna `Secret<Str>`, el codegen falla con error claro
   citando "Secret no es serializable; usar `.expose()` solo
   donde realmente lo necesitás".

**Inspiración**: fly.io DX, Vercel deployment, Datadog
instrumentation as code.

### Fase 13 — CLI builder nativo ✅ CERRADA (v0.11.0 + v0.11.1)

> **Estado al cierre (2026-06-01)**: Fase 13 completa vive en
> producción desde v0.11.0. Polish menor (short flags + Bool=true
> negation + List<Str> variadic) cerrado en v0.11.1. Detalle
> exhaustivo en el CHANGELOG.

**Promesa cumplida**: Fitz no es solo para servicios web — también para
scripts CLI con la misma ergonomía.

**Sintaxis canónica IMPLEMENTADA** (sin `@arg`/`@flag` separados —
**convención del default** lo cubre):

```fitz
@command("greet", desc="Greet a person")
fn greet(name: Str, loud: Bool = false, count: Int = 1) -> Int {
    let n = count
    while n > 0 {
        if loud { print("HELLO, {name}!") } else { print("hello, {name}") }
        n = n - 1
    }
    return 0
}
```

Convención:

- Param **sin default** → positional arg requerido (`mybin <name>`).
- Param **con default** → flag opcional (`--loud`, `--count 3`).
- `Bool = false` → flag bool. `Int/Float/Str = X` → flag con valor.

Help autogenerado (`mybin --help`, `mybin <cmd> --help`), exit codes
POSIX, multi-command dispatch, paridad bit-a-bit `fitz run` ↔ `fitz build`.
Sin imports — `clap`/`typer`/`click` no son necesarios.

**Por qué importa**: amplía el público de Fitz a "devs que
escriben scripts en Python/Bash" — un mercado más amplio que
solo web.

**Decisión de diseño**: la sintaxis ORIGINAL del roadmap mencionaba
`@arg("name", help="...")` y `@flag("loud", short="l")` como
decorators separados. Al implementar, decidimos que la **convención
del default cubre el 90% del caso** sin verbosidad extra (vs Click
que exige `@click.argument`/`@click.option` por cada param). Short
flags se auto-derivan (`--loud` → `-l`) con detección de conflictos.
`@arg`/`@flag` separados quedan como deuda menor si aparece presión.

**Inspiración**: typer (Python), clap (Rust), cobra (Go).

### Por qué este orden — retrospectiva al cierre v0.15.0

1. ✅ **Package manager primero** (Fase 9.y, cerrada) — pre-requisito
   de todo lo demás. Sin manifest no hay tests con discovery, sin
   deps no hay ecosystem.
2. ✅ **DX completo** (Fase 9.z, cerrada) — segunda capa de tooling.
   Necesita el manifest del package manager pero no más.
3. ✅ **Stack web first-class** (Fase 9.w, MVP cerrado) — primera
   extensión al lenguaje core post-tooling. Aprovecha el momentum
   del LSP + package manager + DX para meter features grandes.
4. ✅ **Fase 10 (DB nativo + ORM)** — cerrada, vive en producción.
   La pieza más grande del proyecto. Destrabó el "stack completo"
   para validar Fitz como lenguaje de apps full.
5. 🔮 **Fase 11 (Frontend en `.fitz`)** — la apuesta más grande.
   Sigue sin arrancar. Requiere ronda de diseño dedicada — pre-reqs
   ya están todos cerrados.
6. ✅ **Fase 12 (Deployment ciudadano primera clase)** — cerrada en
   v0.12.5 (Tier 1: healthz + Secret + observability + Docker) +
   v0.13.0 (Tier 2: `fitz deploy` + `@trace`/`@metric` + `@flag`).
7. ✅ **Fase 13 (CLI builder)** — cerrada en v0.11.0 (adelantada
   respecto al plan original — entró antes que Fase 12 por
   coincidencia de bandwidth).

**Hito real al cierre v0.15.0**: el lenguaje tiene **stack web
first-class** (HTTP + WS + auth + middleware + OpenAPI + jobs +
ORM nativo) + **stack CLI first-class** (`@command`) + **producción
ciudadana primera** (healthz/readyz + Secret + observability OTel +
`fitz docker init/build` + `fitz deploy`) + **interop Python** +
**LSP MVP fuerte**. La única pieza grande que falta es **Fase 11
(Frontend)**.

---

## Iniciativas paralelas — Curso `Fitz de 0 a experto` 📚

**Estado**: planificada, sin arrancar. Plan detallado en
[`docs/curso-plan.md`](curso-plan.md).

**Resumen**: 7 módulos / 36 capítulos, español, progresivo desde
instalación + `fitz new` + "hola mundo" en VSCode hasta capstone
con SQLAlchemy + Postgres + Docker + CI. Cada capítulo con código
commiteable bajo `examples/curso/cXX-*/` que entra al smoke
`GUIDE_EXAMPLES_COMPILE`.

**Diferencial vs `guide.md`**: la guía es referencia
feature-por-feature; el curso es narrativo con un proyecto que
crece. Se complementan, no se solapan. Cross-link explícito al
inicio de ambos.

**Requisito explícito**: VSCode + extensión Fitz instalada (el
curso muestra el LSP en funcionamiento desde C3 — hover,
autocomplete, go-to-def, diagnostics live).

**Hilo conductor pedagógico**: organización de proyectos con
buenas prácticas de carpetas (`src/models/` + `src/services/` +
`src/handlers/` + `src/db/` + `tests/`) y "namespaces" via
módulos Fitz (`from src.models.user import User`).

**Decisiones tomadas (2026-05-23)**:
1. Idioma español.
2. Ubicación `docs/curso/`.
3. Screenshots solo para 3 hitos visuales (M1 install, M4
   Scalar UI, M6 hot reload); resto descripciones ASCII.
4. M7 (producción + CI) incluido como parte mandatoria (no
   opcional).
5. Smoke `GUIDE_EXAMPLES_COMPILE` cubre los ejemplos del curso.

**Pre-reqs**: ninguno técnico. Puede arrancar en paralelo con
cualquier fase del lenguaje cuando el autor decida.

**Orden propuesto al arrancar**: M1 entero → release público →
ver tracción → iterar M2-M7. Cada módulo es unidad releasable
independiente.

**Por qué importa**: hoy la guía es la única puerta de entrada
y asume cierta intención técnica. El curso ocupa el espacio de
"alguien cae al sitio, lee la lista de features, y necesita
verlo construirse paso a paso desde cero". También es marketing
implícito de la extensión VSCode.

---

## Hitos clave

| Hito | Descripción |
|------|-------------|
| v0.1 | `print("hola")` funciona |
| v0.2 | Variables, funciones, control de flujo |
| v0.3 | Tipos custom, match, manejo de errores |
| v0.4 | HTTP nativo funcional |
| v0.5 | Primera API real escrita en Fitz |
| v1.0 | Compilador, binario nativo, package manager |

---

## Fase F17 — Send completo + paralelismo HTTP real + bridge eliminado (cerrada, 2026-05-14)

Mini-fase post-tanda Q. Cierra la deuda más grande arrastrada desde
Fase 4: el bridge HTTP `mpsc/oneshot` que serializaba todas las
requests a través del thread main del intérprete. Eliminación
condicionada a migrar `Value` y `EnvRef` a contenedores Send. Seis
sub-pasos:

- **F17.1** — Agregar dep `parking_lot` al `Cargo.toml` del
  intérprete. Commit verde, sin cambios funcionales.
- **F17.2** — Migrar `Shared<T>` y `EnvRef` de `Rc<RefCell<T>>` a
  `Arc<parking_lot::Mutex<T>>`. Refactor atómico (Value y EnvRef
  entrelazados), ~284 sitios `.borrow()/.borrow_mut()` → `.lock()`,
  `Rc::ptr_eq` → `Arc::ptr_eq`. LOADER del evaluator y
  HTTP_REGISTRY siguen como `RefCell` adentro de `thread_local!`
  (single-thread por definición). `#[allow(clippy::
  arc_with_non_send_sync)]` puntual en `Value::new_future` y
  `Environment::new/new_child` mientras los futures siguen `!Send`
  hasta F17.3. Doc-comments stale ajustados a la nueva realidad.
- **F17.3** — Quitar `(?Send)` del macro `#[async_recursion]` en
  los 13 sitios del evaluator. `FitzFuture` pasa de
  `Pin<Box<dyn Future<...>>>` a `Pin<Box<dyn Future<...> + Send>>`.
  Único fix funcional: el `for` sobre List/Range emitía
  `Box<dyn Iterator>` (!Send); cambiado a `Vec<Value>` materializado
  (el caso List ya era snapshot, solo Range necesitó `.collect()`).
  Los `#[allow(arc_with_non_send_sync)]` de F17.2 quedan
  redundantes y se eliminan.
- **F17.4a** — Switch `serve()` tokio runtime de `new_current_thread()`
  a `new_multi_thread()` (auto-detección de cores). Cambio chico
  (~17 LoC). Habilita los siguientes pasos sin cambiar
  funcionalidad observable todavía (el bridge sigue serializando
  hasta F17.5).
- **F17.5** — Eliminar el bridge HTTP `mpsc/oneshot`. Borra
  `InterpTask`, `TaskTx`, `run_interpreter_loop` y `dispatch_request`
  (versión vieja basada en canal). `serve()` ahora corre tokio
  directo en el thread main (sin spawn) sobre un `Arc<HttpRegistry>`
  compartido; cada handler axum invoca
  `handle_task(&registry, ...).await` directo. `build_router` y
  `build_method_router` cambian firma de `TaskTx` a
  `Arc<HttpRegistry>`. Test helpers `run_oneshot_*` simplificados
  (sin `LocalSet`, sin `select!`, sin canal — solo
  `router.oneshot(req).await`). ~269 LoC netas menos en `http.rs`.
  Smoke real: `fitz run examples/server.fitz` + curl secuencial
  con state compartido entre requests responde bit-a-bit como
  pre-F17.5.
- **F17.4b** — Migración paralela del **codegen output** (lo que
  `fitz build` emite). `type Foo = Rc<RefCell<FooData>>` →
  `type Foo = Arc<Mutex<FooData>>` (std::sync — sin dep extra en
  el binario generado). `.borrow()/.borrow_mut()` →
  `.lock().unwrap()`. F12 closures pasan de `Rc<dyn Fn(...) -> R>`
  a `Arc<dyn Fn(...) -> R + Send + Sync>`. State HTTP de F11
  pasa de `thread_local! { static __FITZ_STATE_X: T = init; }` a
  `static __FITZ_STATE_X: LazyLock<Arc<Mutex<T>>> = LazyLock::new(|| ...);`,
  con materialización `(*X).clone()` en cada handler. Runtime
  emitido pasa de `#[tokio::main(flavor = "current_thread")]` a
  `#[tokio::main]` default (multi-thread). Bug residual descubierto
  y arreglado: el field access se emitía como
  `(u.clone()).lock().unwrap().<f>`, dos accesos al mismo Mutex en
  el mismo `format!(...)` deadlock-eaban (`std::sync::Mutex` no es
  reentrante); cambio a bloque acotado
  `{ let __obj = ...; let __g = __obj.lock().unwrap(); __g.<f> }`
  para liberar el guard inmediato. `#[derive(PartialEq)]` falla
  para `FooData` con campos nominales (porque `Mutex<T>` no impl
  `PartialEq`); reemplazado por impl manual con helper recursivo
  `field_eq_expr` que sigue el patrón del intérprete
  (`Arc::ptr_eq` shortcut + lock+deref).
- **F17.6** — Guía cap 19 sub-sección "Paralelismo HTTP real"
  (reescribe la vieja "Limitación actual: server single-threaded").
  Ejemplo nuevo `examples/guide/19b-paralelismo.fitz` con un
  handler `sleep(1000).await`; validado a mano con curl + xargs:
  5 requests concurrentes en **1.2s** vs 5 en serie en **5.3s**
  (pre-F17 ambos eran ~5s). Smoke `GUIDE_EXAMPLES_COMPILE`
  incluye el ejemplo. Roadmap + deudas-post-5b actualizados con el
  cierre. README.md ajustado para reflejar
  paralelismo HTTP real.

**Total al cierre**: 1153 unit + 74 E2E verdes, clippy
`-D warnings` limpio. **Próximo norte**: Fase 8 (Interop Python).

Decisiones técnicas relevantes:

- **`parking_lot::Mutex` para el intérprete, `std::sync::Mutex`
  para el codegen output**. El intérprete acepta una dep nueva a
  cambio de `.lock()` sin `.unwrap()` y mejor performance; el
  codegen prioriza un `Cargo.toml` generado mínimo (sin deps
  extras especialmente para binarios CLI sin HTTP) — el
  `.lock().unwrap()` queda en código generado, no visible al
  usuario.
- **`LazyLock` sobre `OnceLock`** para el state HTTP del codegen.
  std-puro desde Rust 1.80; el `Cargo.toml` generado no agrega
  `once_cell`. Sintaxis cleaner.
- **Política de re-entrancia**: lock scope mínimo + clone-out.
  Auditoría manual sobre `eval_call` y `EnvRef::get` en F17.2.
  Convención en codegen output: bloque acotado por field access
  para que el guard del Mutex se libere al fin del bloque
  inmediato (no al fin del statement contenedor).

Deudas residuales que NO bloquean Fase 8:

- Performance de `MutexGuard` en hot paths del intérprete vs
  el `Ref<T>` de RefCell — esperable que sea similar o mejor
  con parking_lot, pero sin benchmarks. Re-evaluar si aparece
  presión.
- Re-entrancia detectable en compile-time o tests: la auditoría
  fue manual. Una mejora a futuro sería un lint/test que detecte
  patrones de re-lock potencial.
- El intérprete `LOADER` sigue como `thread_local! { RefCell<...> }`.
  Con multi-thread hoy cada worker tendría su propia copia del
  cache de módulos — re-cargando los archivos. Wasteful pero
  correcto. Si aparece presión, migrar a `LazyLock<Mutex<...>>`
  igual que el state HTTP del codegen.

---

## Mini-tandas post-Fase 8 — polish del lenguaje base (2026-05-17 → 2026-05-20)

Serie de bundles de polish cerrados consecutivamente, llevando al
lenguaje + LSP + HTTP a un estado pulido antes de Fase 9.w (Stack
web first-class). Detalle completo de cada mini-tanda con design
decisions, implementación cross-cutting, tests y deudas residuales
vive en [`docs/deudas_lenguaje.md`](deudas_lenguaje.md) — esta
sección es índice resumen + cronología.

**Cronología:**

| Fecha | Mini-tanda | Sumario |
|-------|------------|---------|
| 2026-05-17 | R.1 (R.1.1 → R.1.5) | Sintaxis polish: `not`, `%`, `xs[i] = v`, `0..=10`, `"""..."""` |
| 2026-05-17 | R.2 (R.2.1 → R.2.4) | Match expressivo: or-patterns, guards, `+=`/`-=`, return/break/continue checker |
| 2026-05-17 | R.3 | Métodos custom sobre `type` (opción A: fields como locales) |
| 2026-05-17 | V | VSCode catch-up: grammar TextMate + LSP autocomplete |
| 2026-05-17 | S | Métodos de Str + List (sort/reverse/contains) |
| 2026-05-17 | I, T, L | Index/slicing, tuples + Pattern::Tuple, loops con labels |
| 2026-05-18 | Md, It, Ex, Up | For sobre Map destructuring, iteradores, extras de API |
| 2026-05-18 | Mb2 → Mb4 | Métodos chicos + Range.step_by + comprehensions extendidas |
| 2026-05-18 | C, Fm, Err+, Re+ | Comprehensions, format specs, Result avanzado |
| 2026-05-18 | Bits, Cmp, Xor | Operadores `& | ^ << >> ~`, ops compuestos, `xor` lógico |
| 2026-05-18 | Núm, Lit, F8, F9 | Separadores `1_000`, hex/bin/oct, identifiers Unicode, escapes `\u{}`/`\x` |
| 2026-05-18 | Mln, F14, F15, F16 | Import multi-línea, `let X = expr` mod top, imports transitivos, IR tipado |
| 2026-05-18 | Rt, Lt | Tuple patterns ricos en match, `let` destructuring rico |
| 2026-05-18/19 | Mb5, Mb6, Mb7, Mb8 | Bundles analíticos + async closures + HTTP + bits-extras + g/G |
| 2026-05-19 | Math + Mb9 + Int/Float | Builtins numéricos + dispatch sobre primitivos |
| 2026-05-19 | Fp.1, Fp.2, Fp.3, Sp.2 | Default params, varargs, named args, `return` en match arm |
| 2026-05-19 | Vp, Vm, St, CM, Cd | Visibility, static methods, métodos cross-module, codegen polish |
| 2026-05-20 | HC.1, HC.2 | HTTP polish: status fuera de rango + custom en schema OpenAPI |
| 2026-05-20 | LSPx, LSPy | LSP cross-module go-to-def, Range exacto, scope-aware autocomplete |
| 2026-05-20 | Hpx.1, Hpx.2 | Content-Type 415 validation, return type inference en handlers `fitz build` |
| 2026-05-20 | Mw.next, 5b.1, P2 | Middleware post-process (run), param type inference, chained fix |
| 2026-05-20 | P1, RP, MP | Mw.next codegen, Result+post mws codegen, urlencoded bodies |

**Estado al cierre de la serie (2026-05-20):**

- **1979 unit tests** sin feature, **2069 con `--features lsp`**.
- **233+ compile_e2e tests** validan bit-a-bit `fitz run` ↔ `fitz build`.
- Clippy `-D warnings` limpio en lib + bin `fitz-lsp`.
- **Paridad run↔build**: completa modulo decisiones grandes (traits,
  herencia, operator overloading, F13 heterogéneos) y deudas
  residuales documentadas en `deudas_lenguaje.md`.
- **LSP MVP**: completo (diagnostics + hover + go-to-def +
  autocomplete + cross-module + Range exacto + scope-aware).
- **HTTP stack**: completo para uso real (handlers + middlewares
  Pre/Post + CORS + headers + OpenAPI + status codes custom +
  Content-Type validation + body JSON/urlencoded).
- **Interop Python**: completo end-to-end (Fase 8.1 → 8.8).

**Deudas residuales (NO bloquean Fase 9.w):**

- Wrap-style `next` callable middleware (~6-8h dedicado).
- Multipart con files (multipart/form-data) en HTTP (~4-6h).
- Heterogéneos `[1, "dos", true]` en `fitz build` (F13, ~12-15h —
  requiere `FitzValue` tagged runtime).
- Traits/herencia/operator overloading (decisiones grandes, cada
  uno ~10-15h cuando aparezca caso de uso real).

**Próximo norte: Fase 9.w — Stack web first-class** (WebSockets
`@ws`, streaming HTTP/SSE, `@authenticated`/`@admin` auth nativa
JWT, `@cron`, `@background` jobs sin Celery, etc.). ORM nativo +
migraciones autogeneradas escala a Fase 10.

---

## Mini-fase D — Distribución / instaladores multi-platform 📦

Estado: **pendiente** (planificada, sin sub-pasos cerrados).

Cierra el último gap entre "Fitz tiene binarios en cada release"
y "un usuario nuevo escribe un solo comando y queda con `fitz`
en el PATH". Hoy el flujo asume que el usuario sabe bajar el
binario correcto de GitHub Releases, extraerlo y agregarlo al
PATH manualmente — fricción innecesaria para el caso 90% (alguien
que ya escribe código y quiere probar el lenguaje sin compilar
desde fuente ni levantar Docker).

**Pre-requisitos ya cumplidos**:
- Binarios multi-platform en cada release (`release.yml` con
  matriz Linux x64/ARM + Windows x64 + macOS ARM).
- Imagen Docker `ghcr.io/<owner>/fitz:vX.Y.Z` publicada en cada
  tag.
- Sitio MkDocs en `thegreekman76.github.io/fitz/` (host natural
  para los install scripts).
- Repo público (cumplido antes del 2026-05-22).

**Sub-pasos**:

### D.1 — Install script (curl|sh + PowerShell)

`install.sh` (Unix) e `install.ps1` (Windows) hospedados en el
sitio MkDocs / GitHub Pages. El usuario corre:

```bash
# Linux / macOS
curl -sSf https://thegreekman76.github.io/fitz/install.sh | sh

# Windows (PowerShell)
irm https://thegreekman76.github.io/fitz/install.ps1 | iex
```

Lo que hace el script:

1. Detecta OS + arch via `uname -sm` (Unix) o `$env:PROCESSOR_ARCHITECTURE` (Windows).
2. Consulta el release más reciente con `gh api repos/Thegreekman76/fitz/releases/latest`
   (o `curl` directo a la API REST de GitHub si `gh` no está
   instalado).
3. Baja el asset correcto del release (Linux x64 / Linux ARM /
   Windows x64 / macOS ARM).
4. Extrae el archivo a `~/.fitz/bin/fitz` (Unix) o
   `%USERPROFILE%\.fitz\bin\fitz.exe` (Windows).
5. Sugiere agregar `~/.fitz/bin` al PATH (no modifica `.bashrc`/
   `.zshrc`/`PATH` automáticamente — el usuario decide).
6. Smoke `fitz --version` al final.

Flags opcionales: `--version <vX.Y.Z>` (instala una versión
específica en vez de la última), `--prefix <path>` (override de
`~/.fitz`).

**Hosting**: el `install.sh` vive como archivo estático adentro
de `docs/` y se sirve via MkDocs. Workflow `docs.yml` lo deploya
junto al sitio. URL canónica: `https://thegreekman76.github.io/fitz/install.sh`.

**Tests**: smoke manual en cada plataforma (no automatizado en
CI — los scripts tocan filesystem real, agregan complejidad sin
mucho payoff).

### D.2 — Homebrew tap (`fitz-lang/homebrew-tap`)

Repo separado `Thegreekman76/homebrew-tap` con formula que apunta
al release de GitHub. Usuarios:

```bash
brew tap Thegreekman76/tap
brew install fitz
```

La formula referencia los assets del release de GitHub (no
construye desde fuente — usa los binarios pre-compilados):

```ruby
class Fitz < Formula
  desc "Lenguaje compilado con HTTP/async/interop Python first-class"
  homepage "https://thegreekman76.github.io/fitz/"
  version "X.Y.Z"

  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/Thegreekman76/fitz/releases/download/vX.Y.Z/fitz-darwin-arm64.tar.gz"
    sha256 "..."
  elsif OS.linux? && Hardware::CPU.intel?
    url ".../fitz-linux-x86_64.tar.gz"
    sha256 "..."
  elsif OS.linux? && Hardware::CPU.arm?
    url ".../fitz-linux-aarch64.tar.gz"
    sha256 "..."
  end

  def install
    bin.install "fitz"
    bin.install "fitz-lsp" if File.exist?("fitz-lsp")
  end

  test do
    system "#{bin}/fitz", "--version"
  end
end
```

**Auto-actualización**: workflow GitHub Actions en el repo del
tap (o adentro del `release.yml` principal) que en cada tag `v*`
abre PR al tap actualizando `version` + `url` + `sha256` de cada
plataforma.

**Alcance**: tap propio sin ir a `homebrew-core` (que exige
proyecto estable + uso comprobado + maintainer activo).

### D.3 — Scoop bucket (`fitz-lang/scoop-bucket`)

Repo separado `Thegreekman76/scoop-bucket` con manifest del bin
de Windows. Usuarios:

```powershell
scoop bucket add fitz https://github.com/Thegreekman76/scoop-bucket
scoop install fitz
```

Manifest `bucket/fitz.json` mínimo:

```json
{
  "version": "X.Y.Z",
  "description": "Lenguaje compilado con HTTP/async/interop Python first-class",
  "homepage": "https://thegreekman76.github.io/fitz/",
  "license": "MIT",
  "url": "https://github.com/Thegreekman76/fitz/releases/download/vX.Y.Z/fitz-windows-x86_64.zip",
  "hash": "...",
  "bin": ["fitz.exe", "fitz-lsp.exe"],
  "checkver": "github",
  "autoupdate": {
    "url": "https://github.com/Thegreekman76/fitz/releases/download/v$version/fitz-windows-x86_64.zip"
  }
}
```

`checkver: "github"` + `autoupdate` hacen que `scoop` detecte
nuevas versiones sin update manual del manifest.

### Cierre formal

Al cerrar D.3, las tres modalidades de instalación quedan vivas
y el README del repo principal suma sección "Instalación" con:

- Una línea para Unix (`curl | sh`).
- Una línea para Windows (`irm | iex`).
- Una línea para Homebrew.
- Una línea para Scoop.
- Link al release de GitHub para descarga manual.
- Link a la imagen Docker para deploys.

Cap "Cómo empezar" de la guía (cap inicial) refresca con los
nuevos comandos.

### Deuda anexa (NO bloquea la mini-fase)

- **Code signing macOS** (Apple Developer ID, ~$99/año) — sin
  esto, primer launch en macOS muestra warning "developer
  cannot be verified". Requiere notarization de Apple
  (workflow `codesign + notarytool` adentro del `release.yml`).
- **Code signing Windows** (cert code-signing, ~$200-400/año
  según CA) — sin esto, SmartScreen muestra warning "publisher
  unknown" al primer ejecutar. Decidible cuando el proyecto
  tenga uso real.
- **WinGet manifest** (`winget install fitz-lang.fitz`) —
  equivalente Microsoft Store al Scoop. Más alcance que Scoop
  (viene pre-instalado en Windows 11) pero exige proceso de
  approval en el repo `microsoft/winget-pkgs`. Diferible hasta
  que Scoop quede chico.
- **Publicar a crates.io** (`cargo install fitz`) — gratis,
  exige decidir nombre + ownership en el registry. Es la ruta
  más directa para usuarios Rust pero NO sirve para el caso 90%
  (compila desde fuente, ~3-5 min en una máquina lenta).
- **Bundling CPython embebido** (`fitz build --bundle-python`)
  — tracked separadamente como sub-paso futuro post-8.7
  (decisión python-build-standalone vs PyOxidizer pendiente).

**Estimación**: D.1 ~1-2 días (script + smoke en cada
plataforma), D.2 ~medio día (formula + workflow de
auto-update), D.3 ~medio día (manifest + smoke). Total
mini-fase: ~3-4 días de trabajo dedicado.

**Próximo norte tras la mini-fase D**: cierre del inventario
del plan post-boilerplates (paso 4 del plan) o salto directo a
Fase 10 (Stack DB nativo) según prioridad del autor al
arrancar.
