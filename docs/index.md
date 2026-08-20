---
hide:
  - navigation
  - toc
---

<div class="fitz-hero" markdown="0">
  <img src="assets/logo.png" alt="Fitz logo — engranaje de Rust con la silueta del Fitz Roy adentro" />
  <h1>Fitz</h1>
  <p class="fitz-tagline">
    Un lenguaje de programación compilado con HTTP, async, auth, WebSockets,
    jobs e interop Python como ciudadanos de primera clase del core del lenguaje.
  </p>
</div>

Sintaxis inspirada en Python/TypeScript, compilado a binario nativo
standalone (sin runtime en el destino), tipado gradual con checker
estático en compile-time.

```fitz
type User { id: Int, email: Str, name: Str, role: Str }

@auth_provider
fn check_token(headers: Map<Str, Str>) -> Result<User> {
    let auth = headers.get("authorization")?
    let claims = jwt.decode(auth, "secret")?
    return Ok(User { id: 1, email: claims["email"], name: "Ada", role: "admin" })
}

@authenticated
@get("/me")
fn me(user: User) -> User => user

@server(3000)
fn main() => 0
```

```bash
$ fitz build server.fitz
$ ./server
🏔️  Fitz HTTP escuchando en http://127.0.0.1:3000
   GET /me  🔒 (bearerAuth)
   GET /openapi.json  (schema autogenerado)
   GET /docs          (UI Scalar)
```

---

## ¿Por qué Fitz?

| | Python | TypeScript | Go | **Fitz** |
|---|---|---|---|---|
| Sintaxis limpia | ✅ | ⚠️ | ❌ | ✅ |
| Tipado gradual | ❌ | ✅ | ❌ | ✅ |
| Compilado nativo | ❌ | ❌ | ✅ | ✅ |
| **Multiplataforma** | ⚠️ | ⚠️ | ✅ | ✅ |
| HTTP en el core | ❌ | ❌ | ❌ | ✅ |
| Async nativo | ⚠️ | ✅ | ✅ | ✅ |
| Docs HTTP auto | ⚠️ | ❌ | ❌ | ✅ |
| **Auth nativa** | ❌ | ❌ | ❌ | ✅ |
| **WS tipados + AsyncAPI auto** | ⚠️ | ⚠️ | ⚠️ | ✅ |
| **Jobs sin Celery** | ⚠️ | ⚠️ | ⚠️ | ✅ |
| **Postgres + ORM nativo** | ⚠️ | ⚠️ | ⚠️ | ✅ |
| **Observability OTel** | ⚠️ | ⚠️ | ⚠️ | ✅ |
| **HTTP client built-in** | ⚠️ | ⚠️ | ⚠️ | ✅ |
| **SMTP built-in** | ⚠️ | ⚠️ | ⚠️ | ✅ |
| **Response built-in** | ⚠️ | ⚠️ | ⚠️ | ✅ |
| **Cross-module middleware** | ✅ | ⚠️ | ⚠️ | ✅ |
| **Cookies nativas (`@cookie` + `Response.cookies`)** | ⚠️ | ⚠️ | ⚠️ | ✅ |
| **Aleatoriedad sembrada reproducible** | ⚠️ | ⚠️ | ✅ | ✅ |
| Interop Python | ✅ | ❌ | ❌ | ✅ |

**Multiplataforma**: cada [release](https://github.com/Thegreekman76/fitz/releases/latest)
publica binarios + extensión VSCode + imagen Docker
(`ghcr.io/thegreekman76/fitz:latest`) para **4 plataformas**:
Windows x64, Linux x64, Linux ARM64 y macOS Apple Silicon. El mismo
programa Fitz corre en cualquiera; cross-compile gratis vía rustc
targets.

### Benchmark Fitz ORM vs SQLAlchemy

Para validar la promesa "binario nativo sin overhead" del ORM,
mantenemos un **bench reproducible cabeza-a-cabeza** entre los dos
boilerplates equivalentes
([`api-postgres-fitz`](https://github.com/Thegreekman76/fitz/tree/main/boilerplates/api-postgres-fitz)
vs [`api-postgres-python`](https://github.com/Thegreekman76/fitz/tree/main/boilerplates/api-postgres-python))
— mismo Postgres, mismos endpoints, misma firma. **Headline numbers
en v0.37.12** (Intel Core Ultra 7 155H, Docker 29.2.1, sustained
30s c=10, mediana de 3 corridas):

| Métrica | Fitz ORM | Python+SQLAlchemy | Speedup |
|---|---:|---:|---:|
| Memory peak | **9.2 MB** | 52.4 MB | **5.7x más eficiente** |
| GET /users p50 | **3.57 ms** | 31.24 ms | **8.75x** |
| GET /users RPS | **2618** | 297 | **8.81x** |
| GET /users/{id} p50 | **2.74 ms** | 21.52 ms | **7.85x** |
| GET /users/{id} RPS | **3377** | 411 | **8.22x** |
| Cold start | 0.34 s | **0.31 s** | 0.91x (~empate) |
| Image size | 134 MB | 272 MB | 2x más liviano |

Detalle, metodología y "cómo reproducir" en
[Benchmarks](benchmarks.md).

### Bench complementario — Mixed workload (3 stacks)

Cuando aparece carga peak con writes concurrentes intercalados
con reads (el patrón real de un servicio web), el bench
[`mixed-workload`](https://github.com/Thegreekman76/fitz/tree/main/benchmarks/mixed-workload)
compara Fitz vs Python+SQLAlchemy vs **Node+Prisma** sobre
`users + posts` con FK, 100 VUs ramping via [`k6`](https://k6.io/):

| Métrica (peak, mixed) | Fitz | Python+SQLAlchemy | Node+Prisma |
|---|---:|---:|---:|
| Memory peak | **14.6 MB** | 60.6 MB | 165 MB |
| p50 latency | **5.61 ms** | 201 ms | 23.6 ms |
| p95 latency | **16.36 ms** | 629 ms | 108 ms |
| p99.9 latency | **87.41 ms** | 1011 ms | 299 ms |
| Throughput (RPS) | **454.3** | 142.8 | 351.5 |

Fitz mantiene **<90 ms hasta el p99.9** mientras Python+SQLAlchemy
satura a 629 ms p95 y Node carga **11.3x más memoria**. Detalle
y reproducible en [Benchmarks](benchmarks.md#mixed-workload-fitz-vs-pythonsqlalchemy-vs-nodeprisma).

---

## Por dónde arrancar

[Curso de 0 a experto →](curso/index.md){ .md-button .md-button--primary }
[Construyendo TaskHub →](taskhub/index.md){ .md-button }
[Guía completa →](guide.md){ .md-button }
[DB y ORM →](db-orm.md){ .md-button }
[Benchmarks →](benchmarks.md){ .md-button }
[Boilerplates →](https://github.com/Thegreekman76/fitz/tree/main/boilerplates){ .md-button }
[Ver el roadmap →](roadmap.md){ .md-button }
[GitHub →](https://github.com/Thegreekman76/fitz){ .md-button }

El **[curso `Fitz de 0 a experto`](curso/index.md)** es la entrada
recomendada si arrancás de cero (8 módulos / 42 caps, M1-M8 cerrados).
Te lleva paso a paso desde la instalación hasta una app real con
HTTP + auth + Postgres + Docker + observability, con un proyecto
que crece capítulo a capítulo.

**[Construyendo TaskHub](taskhub/index.md)** es el **proyecto
integrador post-curso**: un Trello colaborativo en vivo Dockerizado
desde el día 1 con los 5 services del compose (app + Postgres +
Prometheus + Jaeger + nginx), RBAC custom de 3 roles, WebSocket por
board, cron jobs persistentes, interop Python con LLM para
priorización IA, frontend vanilla JS y observability completa. **Es
la app más ambiciosa del material pedagógico de Fitz** — pensada
para quien terminó el curso y quiere ver el stack entero integrado,
o para quien ya conoce Fitz y necesita ver un proyecto real
end-to-end.

La **guía** cubre 34 capítulos con ejemplos runnable en
[`examples/guide/`](https://github.com/Thegreekman76/fitz/tree/main/examples/guide):
desde `print("hola")` hasta servidores HTTP con auth + WebSockets +
cron jobs + Postgres + ORM nativo en menos de 100 líneas. Para el
stack DB completo (driver puro + ORM declarativo + relations +
JSONB/arrays + GROUP BY + eager loading + recetas), ver la
[guía exhaustiva DB y ORM](db-orm.md) (~2500 LoC dedicados al
diferencial con SQLAlchemy/Prisma/Diesel).

Para ver el stack completo en acción, los **6 boilerplates
Dockerizados** del repo cubren CLI puro, REST API, auth + frontend,
WebSockets con chat, CRUD multi-archivo con SQLAlchemy + Postgres,
y un showcase fullstack con frontend rico + Postgres en 3
containers. Cada uno con README exhaustivo. Ver el
[cap 35 de la guía](guide.md#37-plantillas-y-boilerplates).

### Extensión VSCode

La extensión con LSP (highlighting + diagnostics + hover +
go-to-def + autocomplete) viene en `.vsix` per-plataforma como
asset de cada [release](https://github.com/Thegreekman76/fitz/releases/latest).
Bajá el de tu OS/arquitectura (`fitz-lang-win32-x64.vsix`,
`fitz-lang-linux-x64.vsix`, `fitz-lang-linux-arm64.vsix` o
`fitz-lang-darwin-arm64.vsix`) e instalá desde VSCode:
`Ctrl+Shift+P` → "Extensions: Install from VSIX...". El binario
`fitz-lsp` viene bundleado adentro — no necesitás compilar nada
local.

Cuando la cuenta de publisher en el VSCode Marketplace esté lista,
la extensión va a estar instalable en un clic desde la UI de
Extensions. Detalle en
[cap 22 de la guía](guide.md#22-soporte-para-editores).

---

## Estado del proyecto

**Fases cerradas** (cierre formal de cada bloque en
[`CHANGELOG.md`](https://github.com/Thegreekman76/fitz/blob/main/CHANGELOG.md)):

- **Fase 2-3** — Lexer + parser + AST + intérprete + tipos
  custom + `Result` + módulos.
- **Fase 4** — HTTP nativo (`@get`/`@post`/`@put`/`@delete` +
  `@server`).
- **Fase 5a** — Type checker estático.
- **Fase 5b** — Codegen `fitz build` a binario nativo standalone.
- **Fase 6** — Async nativo (`async fn` + `.await` + `sleep` +
  `Future<T>`).
- **Fase 7** — OpenAPI 3.1 auto-generado + UI Scalar.
- **Fase 8** — Interop Python end-to-end (PyO3 + marshaling
  bidireccional + bridge async + `fitz py-types`).
- **Fase 9.x** — LSP MVP (diagnostics + hover + go-to-def +
  autocomplete) + distribución multi-platform.
- **Fase 9.y** — Package manager (`fitz new`/`add`/`remove` +
  `fitz.toml` + `fitz.lock` + path/git deps).
- **Fase 9.z** — DX completo (`fitz fmt`/`test`/`dev`/`repl`/
  `lint`).
- **Fase 9.w MVP** — Stack web first-class (auth + JWT/Argon2 +
  WebSockets tipados + AsyncAPI auto + cron + spawn).
- **Fase 10** — Stack DB nativo (driver Postgres puro Fitz + ORM
  declarativo + migraciones automáticas + transactions + composite
  PK + indexes + tipos avanzados Date/DateTime/Uuid).
- **Fase 13** (v0.11.0) — CLI builder nativo (`@command` con
  `--help` autogenerado, sin `clap`/`argparse`/`click`). *Nota
  histórica: originalmente numerada como "Fase 11" antes del
  reordenamiento del roadmap que promovió el frontend nativo
  a Fase 11.*
- **Fase 12 ENTERA** — **Deployment ciudadano primera clase**.
  Healthz/readyz auto-mount + SIGTERM drain (12.1), `Secret<T>`
  opaco con redacción automática + builtins
  `secret()`/`config()`/`load_env()` (12.2), Observability OTel
  built-in con logs estructurados + spans HTTP + métricas
  Counter/Histogram + bridge OTLP + endpoint `/metrics` Prometheus
  (12.3), Dockerfile + compose autogenerados con
  `fitz docker init`/`build` y detección AST del shape (12.4),
  cap 35 integrador en la guía + curso M7 completo
  (12.5).
- **9.w.1.iter2.a** (v0.12.4) — RBAC custom con `@requires("role")`
  apilable. Mensaje 403 enriquecido con role actual + requeridos.
- **9.w.1.iter2.b** (v0.12.6) — Token blacklist + refresh con
  módulo built-in `auth` (`auth.blacklist`/`auth.is_blacklisted`/
  `auth.cleanup_expired`). Tabla auto-creada idempotente.
- **Fase 12 Tier 2** (v0.13.0) — **CERRADO en bloque coordinado**:
  `fitz deploy <docker|compose>` (12.6 — sub-comando con thin
  wrappers sobre `docker build`/`compose up`); decoradores
  explícitos `@trace(name="X")` y `@metric(name="X")` sobre fns
  user con paridad bit-a-bit `fitz run` ↔ `fitz build` (12.7);
  feature flags built-in `@flag("name")` + `flag(name) -> Bool`
  + módulo `flags` con manifest `[flags]` y override por env
  var `FITZ_FLAG_<UPPERCASE>` (12.8).
- **Hito MatHelp — Hito 1 + Hito 2** (v0.49.0, 2026-08-20) —
  arranque + confianza + deployment/i18n del backlog de la
  primera app real de terceros (`docs/norte-mathelp.md`):
  módulos `rand` (CSPRNG + PRNG sembrado determinístico con
  secuencia idéntica `run`↔`build`), `fs` (filesystem en
  runtime) y `num` (formateo locale-aware `es-AR`/`en-US`);
  API de cookies (leer `@cookie(name="X")` + escribir
  `Response { cookies: [Cookie {...}] }`); fixes de paridad
  `-> T?` con `return` (destraba el binario nativo de
  fitz-liveviews) y `Str + Any`; **differ de paridad**
  `fitz run` ↔ `fitz build` sobre un corpus CLI-puro; `git`
  en la imagen Docker oficial.
- **Hito MatHelp — Hito 3/4 (cierre entero)** (v0.50.0 +
  v0.51.0, 2026-08-20) — v0.50.0: `Map.remove(key) -> Bool`
  (muta in place, desbloquea la eviction de fitz-liveviews);
  `is_in(<var>)` en el ORM con una variable `List<T>` →
  `= ANY($N)` (paridad bit-a-bit validada contra Postgres
  real); paridad `form-urlencoded` → `type` del handler en
  `fitz run` (login zero-JS idéntico run↔build); `.preload()`
  en el intérprete da error dedicado; limpieza de codegen.
  **v0.51.0 (FITZ-02)**: **servido de archivos estáticos** con
  `@server(static_dir="./public", static_prefix="/static")` —
  Content-Type por extensión, `ETag` basado en contenido +
  `If-None-Match` → 304, `Cache-Control`, `Last-Modified`,
  path-traversal bloqueado; favicon + `manifest.webmanifest`
  → **PWA instalable** sin nginx. `fitz build --embed-static`
  hornea los assets en el binario con `include_bytes!` → sirve
  su propio frontend sin el dir en disco (**distroless**).
  Paridad bit-a-bit `fitz run` ↔ `fitz build`.
- **Fase 11.7 R3.5 + Frente 2** (v0.22.0, 2026-07-19) — **el
  kanban como WASM SPA + composición `<Child />` completa**.
  Cierra Phase 11.7 entera para el target client-WASM: el Board
  del kanban colaborativo compila a una SPA WebAssembly
  standalone desde UN `.fitzv` (~57 KB raw / ~21.5 KB gzipped,
  `examples/view/kanban/`). R3.5 trae el lowerer de listas
  (closures + `.map`/`.filter`/`.len` + `{#for}` sobre un call),
  fns clásicas importadas transpiladas al crate WASM, y payload
  de click + form (`data-flv-*`). Frente 2 trae event bubbling
  child→parent (`<Child @event="h" />`) y `<slot>fallback</slot>`
  con contenido rellenado por el parent. 7 ejemplos runnable en
  `examples/view/` (counter → kanban), cada uno a WASM real. Los
  mismos convenios `data-flv-*` sirven a SSR y WASM. Cero cambios
  a classic Fitz (aditivo al emitter `.fitzv` → WASM).
- **Fase 11 hasta 11.6.e §9.bb** (v0.21.0, 2026-07-16) — **Native
  frontend `.fitzv` compilado a WASM + SSR emitter for
  `fitz-liveviews`**. Nueva extensión `.fitzv` (single-file
  components à la Vue/Svelte) con parser + expand + checker + dos
  backends: **WASM** para interactividad client-side
  (`fitz build --bin <web> --target wasm-client` bajo feature
  opt-in `client-wasm`, 11.4 KB gzipped sobre 40 KB gate en el
  counter demo) y **SSR** targeting `fitz-liveviews`
  (`@live_component` + `@render_for` + `@on`). Module loader
  routes `.fitzv` transparente — sibling `.fitz` gana cuando
  both existen, migration opt-in y additiva. Cross-module
  `@live_component` auto-inject (§9.bb) paralelo bit-a-bit a
  W12+B10 elimina el manual `flv_register(...)` boilerplate
  para components declarados en imported `.fitzv`/`.fitz`
  sibling modules. Cierra 11.1 (POC parser), 11.2.a/b/c (bridge
  classic AST + checker + directives `{#if}`/`{#for}`/`{#else}`/
  `<slot>`), 11.3.a/b/c (scoped styles + `apply_scope`),
  11.4.a/b/c/d (WASM emitter approach A2 hand-rolled
  `wasm-bindgen`; bundle-size gate cerrado; browser smoke
  manual validado), 11.5.a/b/c/d/e (CLI wiring + manifest
  `[[bin]]` array — cierra debt 9.y.8+ multi-bin — + wasm-client
  emit + multi-component composition `<Child prop="v" />` +
  cierre formal), 11.6.a/b/c/d (SSR emitter + full expression
  grammar + module loader integration + same-file `<Child />`
  composición), 11.6.e PARTIAL (§9.z payload scope +
  `fitz_liveviews` missing-dep hint; §9.aa event-body widening;
  §9.bb cross-module auto-inject). Detalle exhaustivo en
  [`docs/fase-11-plan.md`](fase-11-plan.md) §9.a–§9.bb.
- **Phase 11 Session C — Pedagogic docs** (v0.21.4,
  2026-07-18) — **Phase 11.9 CERRADO ENTERO**. La última
  sub-fase visible de Fase 11 cierra con **cero código
  Rust nuevo** — todo el diff es contenido pedagógico
  cross-doc: guía + curso + architecture. **Cap 36 nuevo en
  `docs/guide.md`** (~1050 LoC) dedicado a `.fitzv` (SFC)
  con panorama vecino (Vue/Svelte/React/Elm/HTMX/Phoenix
  LiveView) + Las piezas + interpolación con la regla de
  scoping de 4 niveles + cross-file types + composición de
  components + los dos backends (SSR + WASM 11.4 KB gzipped
  sobre 40 KB gate) + LSP support (cross-link cap 22) +
  ejemplo runnable Counter completo + compatibilidad con
  classic Fitz + qué no está en el MVP. **Cap 22 refresh**
  con nueva sub-sección "En archivos `.fitzv`" que cita las
  4 capabilities LSP de Phase 11.8. **Cap 38 (Qué sigue)
  refresh** — sección "Lo que ya bajó de especulativo a
  REALIDAD" mueve SFC + deployment + migrations desde
  "roadmap futuro" a "shipped features". **`docs/architecture.md`
  refresh** con nueva sección `view/` que describe los 7
  módulos del pipeline view (lexer/parser/expand/check/
  codegen_ssr/codegen_wasm/wasm_build), la relación 2-emit-
  branches + 1-check-pass, y la integración con el module
  loader. **Nuevo módulo del curso M9 — Frontend nativo con
  `.fitzv`** con 3 caps pedagógicos: C1 (Counter primer
  contact), C2 (Template DSL profundo — interpolación +
  directivas + composición + event wiring), C3 (Board.fitzv
  full-page migration del kanban como acceptance criterion).
  `docs/curso/index.md` y `mkdocs.yml` actualizados. **Con
  este release Phase 11 queda cerrada por completo** — solo
  Phase 11.7 (client-side dynamic capabilities + kanban SPA
  port) queda como sub-fase FUTURA schedule-TBD (SSR path
  cubre el 100% del caso Board y el 95% del caso general;
  Phase 11.7 unlockea drag&drop + offline).

- **Phase 11 Session B — LSP inside `.fitzv`** (v0.21.3,
  2026-07-18) — **Phase 11.8 CERRADO ENTERO**. El LSP ahora
  reconoce `.fitzv` como surface de primera clase con las
  cuatro capabilities core: **diagnostics + completions +
  hover + go-to-definition**. Editar un `.fitzv` en VSCode ya
  no es texto plano — la extensión bundleada muestra errores
  del view lexer/parser/expand/check en tiempo real, completa
  directivas de template (`{#if}`, `{#for}`, etc.) + state
  fields + event handlers, hover sobre state fields muestra
  tipo declarado, y go-to-def salta a la línea de declaración.
  **11.8.a diagnostics** — nuevo `check_view_source(source) ->
  Vec<FitzError>` routea via view pipeline + mapea 3 tipos de
  error a `FitzError`; `check_source_by_uri(uri, source)`
  dispatch por extensión; LSP bin `check_and_publish` routea
  `.fitzv` transparente. **11.8.b completions** — nuevo
  `completion_at_position_view` con 4 clases (directives +
  event decorators + state fields + event names); heuristic
  scan del source raw es robust to partial parses
  (unterminated `{` mid-typing). **11.8.c hover** — nuevo
  `hover_at_position_view` con markdown code fence + label
  ("state field of Component" / "event handler of Component");
  keyword filter evita false positives. **11.8.d go-to-def** —
  nuevo `definition_at_position_view` salta a `<name>: <type>`
  line en state block O `event <name>(...)` line; component
  boundary respect. Sin cambios breaking. **~620 LoC + 20
  tests nuevos**. Deudas residuales: fine-grained context
  routing (completion suggestions correctas pero pueden
  aparecer en contexts adjacentes); cross-module symbol
  lookup en hover/go-to-def; TypeInfo-based hover para complex
  expr shapes. **Solo Session C (Phase 11.9 pedagogic docs)
  + Session D (Phase 11.7 client-side reactivity) siguen
  abiertas** de las 3 Phase 11 sub-fases originales.

- **Phase 11 Session A — Small residual debts** (v0.21.2,
  2026-07-17) — 3 deudas menores del SFC pipeline cerradas
  en bloque coordinado. **S.1 Alias en imports SFC** (`from X
  import Y as Z`) — `Token::As` nuevo en el view lexer,
  `ViewImport.names: Vec<(String, Option<String>)>` mirror de
  PreF8.4, SSR emitter aplana aliases a `imported_names` (K-4)
  usando el alias local en scope; emite `from X import Y as Z`
  verbatim en el classic module. **S.2 `Map<Str, Str>` static
  props** vía `k=v,k=v` convention — `<Child meta="role=admin,
  scope=full" />` con `meta: Map<Str, Str>` coerciona a `vec![
  (...), (...)]` (checker + WASM) y a `{"role": "admin", ...}`
  (SSR); empty → `vec![]`/`{}`; restrict a `Map<Str, Str>`
  only, richer shapes vía interpolación. **S.3 Type-check
  estático del expr interpolado** — nueva `light_check_
  interpolated_prop` en `view/check.rs` catchea bare Ident
  refs con parent state field type mismatch (Str vs Int, etc.)
  en check time en vez de propagarse hasta el emitted module.
  Richer expr shapes skipping. **S.6 Cross-file `<Child />`
  composition** DIFERIDO — design decision needed, ningún
  ejemplo existente lo necesita; ataca cuando llegue el
  companion UI library con grid + forms. Sin cambios breaking.
  3719 → 3741 lib tests verde (+22 tests netos).

- **Phase 11 refinements** (v0.21.1, 2026-07-17) — K-3
  (compound props para `<Child />`) + K-4 (SSR emitter acepta
  imported top-level fn refs en templates + event bodies).
  Descubiertos + fixed durante la Board.fitzv migration en
  `fitz-liveviews`. **K-3 List<primitive>** — `<Child tags="a,b,c"
  />` con `tags: List<Str>` coerciona a `vec![...]` (checker +
  WASM) y a `[...]` (SSR); empty string → `vec![]`/`[]`; nested
  primitives recurse. **K-3 Interpolated (SSR)** — `<Child
  prop="{expr}" />` inlina la expr parseada con state-field
  rewriting del padre; nominal / compound types pasan
  naturalmente por la interpolación. **K-4 imported fn refs
  (SSR)** — el walker `format_fitz_expr_scoped` gana un
  `imported_names: &[&str]` del `ExpandedViewFile.imports`
  (§9.dd), threaded por ~30 call sites del emitter. Bare Idents
  que matchean un import emiten verbatim; el classic checker
  valida el call sobre el emitted module. Resolution order:
  `local_scope > state_field > imported_name > error con hint`.
  WASM path rechaza interpolated props citando Phase 11.7+
  (reactive plumbing). Con esto la Board.fitzv migration en
  `fitz-liveviews` queda "prolija, facil, con arquitectura
  clara" — types en `card.fitz`, helpers puros en
  `board_helpers.fitz`, SFC en `Board.fitzv`, HTTP + WS thin
  wire-up en `main.fitz`. Sin cambio breaking. 3719 lib tests +
  4 K-4 nuevos verdes.

**Próximo norte**: **atacar deudas** — inventario en
[`docs/deudas-post-5b.md`](deudas-post-5b.md). Fase 11
remaining scope (11.7 client-side dynamic capabilities +
kanban SPA port + WASM interpolated props con reactivity
plumbing, 11.8 LSP inside `.fitzv`, 11.9 pedagogic docs), Fase
13+ (orquestación distribuida, multi-tenant, deploy targets
extra `fly`/`railway`/`k8s`) según demanda real. Sin presión
inmediata.

---

## Nombre

Por el [Monte Fitz Roy](https://en.wikipedia.org/wiki/Fitz_Roy)
en El Chaltén, Patagonia, Argentina. Un nombre que no se olvida.
