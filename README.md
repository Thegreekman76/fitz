<p align="center">
  <img src="assets/logo.png" alt="Fitz logo — engranaje de Rust con la silueta del Fitz Roy adentro" width="160" />
</p>

<p align="center">
  <em>Engranaje de Rust, Fitz Roy adentro: construido con Rust, nacido en una montaña.<br/>
  Más sobre el porqué del logo en <a href="docs/vision.md#el-logo">docs/vision.md → El logo</a>.</em>
</p>

# Fitz

> Un lenguaje de programación moderno, compilado y orientado a servicios web.
> Nacido en la Patagonia. Construido con Rust.

```fitz
// Un servicio HTTP, compilado a binario nativo, cero dependencias.

type User { id: Int, name: Str, email: Str? }

@get("/users/{id}")
async fn get_user(id: Int) -> User {
    return User { id: id, name: "ada", email: null }
}

@server(3000)
fn main() => 0
```

```bash
fitz run mi_app.fitz       # intérprete + checker estático
fitz build mi_app.fitz     # binario nativo standalone (~5 MB)
./mi_app                   # corre sin Fitz ni Rust en el destino
```

📖 **Documentación completa:** [thegreekman76.github.io/fitz](https://thegreekman76.github.io/fitz/)

## Por qué Fitz

Los lenguajes actuales te obligan a elegir entre ergonomía y performance:

- **Python** — hermoso, pero lento. Deployar es un dolor.
- **TypeScript** — tipado opcional de mentira, arrastra el bagaje de JS.
- **Go** — compilado y rápido, pero sintaxis verborrágica.
- **Rust** — perfecto por dentro, demasiado complejo para APIs.

**Fitz toma lo mejor de cada uno:**

| Feature                | Python | TypeScript | Go  | Fitz  |
| ---------------------- | ------ | ---------- | --- | ----- |
| Sintaxis limpia        | ✅     | ⚠️         | ❌  | ✅    |
| Tipado gradual         | ❌     | ✅         | ❌  | ✅ \* |
| Compilado nativo       | ❌     | ❌         | ✅  | ✅ †  |
| **Multiplataforma**    | ⚠️     | ⚠️         | ✅  | ✅ ✱  |
| HTTP en el core        | ❌     | ❌         | ❌  | ✅    |
| Async nativo           | ⚠️     | ✅         | ✅  | ✅ ‡  |
| Docs HTTP automáticas  | ⚠️     | ❌         | ❌  | ✅ ◊  |
| **Auth nativa**        | ❌     | ❌         | ❌  | ✅ ♦  |
| **WebSockets tipados** | ⚠️     | ⚠️         | ⚠️  | ✅ ♣  |
| **Jobs sin Celery**    | ⚠️     | ⚠️         | ⚠️  | ✅ ♠  |
| **Postgres + ORM nativo** | ⚠️  | ⚠️         | ⚠️  | ✅ ◈  |
| **Observability OTel** | ⚠️     | ⚠️         | ⚠️  | ✅ ◉  |
| **HTTP client built-in** | ⚠️    | ⚠️         | ⚠️  | ✅ ♥  |
| **SMTP built-in**      | ⚠️     | ⚠️         | ⚠️  | ✅ ✉  |
| **Response built-in**  | ⚠️     | ⚠️         | ⚠️  | ✅ ◄  |
| Interop Python         | ✅     | ❌         | ❌  | ✅ §  |

\* **Tipado gradual con chequeo estático**. `fitz check` valida anotaciones en compile time; sin anotación se infiere o se trata como `Any`. → [cap 15 de la guía](https://thegreekman76.github.io/fitz/guide/#15-errores-y-mensajes).

† **Compilado nativo via transpile-a-Rust + Cargo**. Binario standalone ~5 MB, sin runtime de Fitz en el destino. Cross-compile gratis vía targets de rustc. → [cap 20](https://thegreekman76.github.io/fitz/guide/#20-fitz-build--compilar-a-binario-nativo).

✱ **Multiplataforma de verdad**. Cada release publica binarios + extensión VSCode + imagen Docker para **4 plataformas** (Windows x64, Linux x64, Linux ARM64, macOS Apple Silicon). El mismo programa Fitz corre en cualquiera de las cuatro — y compilás desde una a las otras sin instalar toolchains extras (cross-compile gratis vía rustc targets). Imagen Docker `ghcr.io/thegreekman76/fitz:latest` lista para `FROM` en boilerplates Dockerizados. Variante `ghcr.io/thegreekman76/fitz:latest-python` (desde v0.9.36) con `--features python` activo, ahorra a los boilerplates 5/6 ~5-8 min de build inicial.

‡ **Async nativo + paralelismo HTTP real**. `async fn` y `.await` postfix, `Future<T>` built-in, evaluator async sobre tokio multi-thread. 5 requests concurrentes en ~1.2s en vez de ~5s en serie. → [cap 19](https://thegreekman76.github.io/fitz/guide/#19-async-y-concurrencia).

◊ **OpenAPI 3.1 + UI Scalar automáticos** desde los decoradores. Schema bit-a-bit idéntico entre `fitz run`, `fitz openapi` y `fitz build`. → [cap 18](https://thegreekman76.github.io/fitz/guide/#18-docs-automáticas).

♦ **Auth nativa** con `@auth_provider`/`@authenticated`/`@admin` + built-ins `jwt` (HS256/384/512) y `hash` (Argon2id). Validación en compile-time, 401/403 automáticos en OpenAPI. Cero deps externas. → [cap 28](https://thegreekman76.github.io/fitz/guide/#28-auth-nativa).

♣ **WebSockets tipados** con `@ws("/path")` + `WsConn<T>`. Marshaling JSON automático para T text (Str / nominal / etc.), frames binarios raw con `WsConn<Bytes>` (sin re-encoding ni base64), AsyncAPI 3.0 auto-generado en `/asyncapi.json` (incluye `contentType: application/octet-stream` cuando T=Bytes), heartbeat built-in, auth integrada en el handshake. → [cap 29](https://thegreekman76.github.io/fitz/guide/#29-websockets-tipados).

♠ **Jobs sin Celery** con `@cron("expr")` + `@background` + `spawn(fn_call)`. Sin broker externo (Redis/RabbitMQ no son requisito). Cron-only mode systemd-friendly. **v0.11.2 (iter2)**: kwargs opcionales `tz=` (IANA timezones), `retry={max, backoff, ...}` (3 backoffs), `catch_up=true|false` (missed runs al boot), `store=db` (persiste a `fitz_cron_jobs` + `fitz_cron_runs`). → [cap 30](https://thegreekman76.github.io/fitz/guide/#30-jobs-sin-celery).

▣ **CLI builder nativo (v0.11.0)** con `@command("name", desc="...")`. El binario producido por `fitz build` parsea `std::env::args()`, dispatcha al comando matching y emite `--help` autogenerado **sin `clap`/`argparse`/`click`** (zero deps externas). Convención de params sin decorators extras: positional vs flag se infiere del `default = ...`. Multi-comando con dispatch automático. Exit codes tipados (return `Int`). Paridad bit-a-bit `fitz run` ↔ `fitz build`. **Único lenguaje moderno** que combina HTTP nativo + WebSockets tipados + ORM + jobs + **CLI builder en el core del compilador**, con zero deps externas para estas features intrínsecas. → [cap 34](https://thegreekman76.github.io/fitz/guide/#34-cli-builder-nativo-command) + boilerplate [`cli-tool`](boilerplates/cli-tool/) con 3 subcomandos.

◈ **Postgres + ORM nativo**. **Hito del proyecto (v0.10.0 → v0.10.6)**: driver Postgres puro escrito en Fitz/Rust (~2400 LoC en `src/db.rs`, sin libpq, sin `tokio-postgres`/`sqlx`/`diesel`) + ORM declarativo sobre `type` con decoradores nativos del lenguaje (`@table`/`@primary`/`@column`/`@belongs_to`/`@has_many`/`@has_one`). SQL constante en codegen-time (cada `.where(closure)` se walka del AST DURANTE EL CODEGEN, fragmento SQL hard-coded en el binario — comparable a Diesel/sqlx, mejor que SQLAlchemy/ActiveRecord que construyen SQL via objetos en runtime). Eager loading con dispatch estático (`.preload("posts")` con relation name como Str literal compila a match exhaustivo en compile-time — typos detectados antes de correr). Tipos avanzados nativos: JSONB ↔ `Map<Str, Any>`, arrays Postgres ↔ `List<scalar>` (incluyendo `List<Int?>` con NULL en arrays), `Map<Str, T>` concreto homogéneo. Aggregates scalar + GROUP BY con `Aggregated<Row>` separado de `QueryBuilder<Row>`. **Transactions ORM (v0.10.14-15)** closure-based con `db.transaction(fn(tx) -> Result<T> { ... })` — auto-rollback en `Err`, fn nombrada o FnExpr inline con captures (paridad bit-a-bit). **Migraciones automáticas (v0.10.16)**: `fitz db diff/migrate/status/new` introspecciona el schema real, lo compara con los `@table` types, emite SQL DDL automático con tracking idempotente — cero deps (ni Alembic ni Flyway). `@db_default("NOW()")` opcional para emitir defaults SQL en el CREATE TABLE / ADD COLUMN. **v0.10.6** cierra 7 fricciones residuales del codegen ORM en bloque: `id: 0` auto-asigna bigserial (W4), `db.close().await?` propaga errores como `Result<Null>` (W5), `.update(db, body.changes)` acepta Map var (W7), `.starts_with(prefix)` acepta var Str (W3), `body.field` en closures de `.where` (W6), Map literal `{"k": 1}` en field `Map<Str, Any>` (W1), `match user { null => x, u => u.name }` con refinement Nullable (W2). Paridad bit-a-bit `fitz run` ↔ `fitz build` validada en CI multi-plataforma con `postgres:16` service container corriendo 16 paridad codegen E2E + 27 evaluator E2E en cada push. **Único lenguaje moderno** que combina driver Postgres puro + ORM declarativo + paridad bit-a-bit intérprete↔binario nativo + LSP completo (autocomplete del ORM end-to-end con tipos refinados) **sin macros derive ni introspection runtime**. **v0.10.27** suma `Type.bulk_insert(rows, db, batch_size=1000)` + composite PK (N `@primary` fields por type) + `@index(col1, col2, ..., unique=true, name="...", where_=<expr>)` decorator a nivel type. **v0.10.28 — Tier S del ORM**: `fitz db inspect` (introspect del schema real con vista texto + `--json` machine-readable) + `@index(col, using="gin"|"gist"|"brin"|"hash"|"spgist")` method override (full-text/range/large-tables sin bajar a `db.exec`) + `FITZ_DB_LOG=1|verbose` (query log a stderr, zero overhead default) + `FITZ_HTTP_LOG=1|verbose` (access log estilo uvicorn — paralelo, ambos via env var opt-in). **v0.10.29 — Cierre masivo del ORM**: JSON path operators (`has_path`/`path_text`/`path_int`/`path_float`/`path_bool`) con cast tipado para nested jsonb, full-text search `@@` (`matches`/`plainto_matches`), `@unique(col1, col2, ...)` composite shortcut, `@check_constraint("expr")` para CHECK constraints declarativos, cross-schema FK transparente (`@belongs_to("User")` desde un type en otro schema emite `REFERENCES "public"."users"(id)` automáticamente), diff completo de indexes (detecta cambios en `using`/`where_clause`/`unique`/`columns`), `fitz db inspect --all-schemas`, redaction automática de secrets en `FITZ_DB_LOG=verbose`, errores del driver enriquecidos con SQLSTATE + SQL + params (también redactados), `FITZ_DB_MAX_CONNS` env var para pool tuning. **v0.10.30 — Tier B (API completion Date/DateTime/Uuid)**: aritmética nativa sobre los 3 tipos built-in (`.add_days/months/years` + `.subtract_*` symétrico; DateTime también `.add_seconds/minutes/hours`), diff entre fechas (`d1.diff_days(d2)`, `dt2.diff_seconds(dt1)` — signed Int), comparison operators (`<`/`>`/`<=`/`>=`) entre Date-Date y DateTime-DateTime, `Uuid.v7()` time-ordered (RFC 9562 — PKs sortables por created_at en btree), shortcuts (`Date.tomorrow/yesterday`, `DateTime.epoch`), timezone display (`DateTime.to_local()` en TZ del sistema + `.in_tz("America/Argentina/Buenos_Aires")` IANA via `chrono-tz`). 10 E2E nuevos validan paridad bit-a-bit `fitz run` ↔ `fitz build`. **v0.10.31 — Tier A (MVP fuerte del ORM)**: `fitz db diff --check-destructive` con clasificación Safe/Risky/Destructive + abort sin `--allow-destructive`; `ALTER COLUMN TYPE` con `USING col::T` automático; `ALTER TABLE ADD/DROP CONSTRAINT` para CHECKs nuevos via diff; **nested transactions** con SAVEPOINT (`db.transaction(fn(tx){ tx.transaction(fn(inner){...}) })` — inner Err rollback parcial); **isolation levels** custom (`db.transaction(closure, isolation="SERIALIZABLE")` con whitelist 4 niveles ANSI + READ ONLY/WRITE); `db.connect(url, max_conns=N)` kwarg; drift completo de `@check_constraint` (introspect lee `pg_constraint.contype='c'`) + cross-schema FK (introspect popula `references_schema`); FK targeting composite PK del target → error claro pre-DDL en lugar de fallback silencioso. 6 E2E reales contra Postgres validan los caminos del intérprete. **v0.10.32 — Tier C + D (operadores SQL + DX/LSP residual)**: `ts_rank` full-text ranking en `.order_by(fn(u) => -u.body.rank("q"))` (combinable con `.matches("q")` para search ordering); expression indexes con `@index(expression="lower(email)")` (kwarg dedicado para case-insensitive UNIQUE, FTS setup, totals computados); `qb.merge_jsonb(db, field, patch)` para JSON `||` merge preservando keys existentes; LSP autocomplete de operadores ORM en `.where()` (`is_in`/`like`/`matches`/`has_key`/`path_text`/etc. con detail `(ORM .where)` distintivo); LSP hover sobre `@table` types muestra el `CREATE TABLE` emitted bajo el tipo (debug migrations sin abrir `fitz db diff`). 3 E2E reales contra Postgres validan los caminos del runtime. → [cap 31 (resumen)](https://thegreekman76.github.io/fitz/guide/#31-postgres--orm-nativo) y [guía exhaustiva DB y ORM](https://thegreekman76.github.io/fitz/db-orm/) (~2500 LoC con todos los operadores, recetas, CLI integration y limitaciones).

◇ **Fase 12 Tier 2 (v0.13.0)** — **`fitz deploy` + `@trace`/`@metric` + `@flag` cerrados**. Tres sub-fases coordinadas que cierran el Tier 2 de Fase 12: (1) **`fitz deploy <target>`** (12.6) — sub-comando con thin wrappers sobre `docker build`/`compose up`; soporta `docker` (con `--no-push` opt-out) y `compose` (`--no-detach`/`--no-build` opt-outs). Aborta con sugerencia clara si falta Dockerfile/compose.yml. (2) **`@trace(name="X")` y `@metric(name="X")`** (12.7) — decorators apilables sobre fns user (rechazados sobre HTTP/WS — auto-instrumentation Fase 12.3 cubre). `@trace` abre `tracing::info_span!`; `@metric` registra `<name>_duration_seconds` (histogram) + `<name>_calls_total` (counter) al Drop del scope via guard RAII. Kwarg `name=` opcional. Paridad bit-a-bit `fitz run` (no-op honesto) ↔ `fitz build` (instrumentación real). (3) **`@flag("name")` + `flag(name) -> Bool` + módulo `flags`** (12.8) — feature flags built-in con dos fuentes: sección `[flags]` en `fitz.toml` (defaults compile-time, baked-in al binario) + env vars `FITZ_FLAG_<UPPERCASE>` (override runtime). Default `false` (fail-safe, features opt-in). `@flag` sobre HTTP/WS → 404 si flag off (gate hot path antes de middlewares/auth). `flags.is_enabled(name)` alias funcional; `flags.list()` enumera flags conocidos. Paridad bit-a-bit con registry estático `OnceLock` + cache lookup. → [cap 33.5 (@trace/@metric)](https://thegreekman76.github.io/fitz/guide/#335-trace-y-metric--instrumentación-manual-fase-127) + [cap 33.11 (feature flags)](https://thegreekman76.github.io/fitz/guide/#3311-feature-flags-con-flag-flag-y-flags-fase-128).

◉ **Observability minimal con OpenTelemetry (v0.12.0)** — **Fase 12.3 CERRADA**. Tres piezas nativas montadas sobre el stack web first-class: (1) **Structured logging built-in** con `log.info/warn/error/debug(msg, k: v, ...)` que emite a `tracing` + `tracing-subscriber` con dos formatos (JSON estructurado por default, pretty con ANSI colors cuando `FITZ_LOG_FORMAT=pretty`), level configurable con `FITZ_LOG=info|debug|warn|error`. (2) **Spans HTTP automáticos** sobre cada request con OTel span `HTTP <method> <path>` + atributos `http.method`/`http.target`/`http.status_code` + métricas `http.server.requests_total{method,path,status}` (Counter) y `http.server.request_duration_seconds{method,path}` (Histogram) — todo via `metrics = "0.24"` crate. Correlación de logs con `trace_id` (32 hex) + `span_id` (16 hex) propagados via `tokio::task_local!` a TODOS los `log.*` adentro del request handler (Fitz, evaluator, OTel SDK). (3) **OTLP exporter** opt-in via env vars: `OTEL_EXPORTER_OTLP_ENDPOINT=http://collector:4318` activa el exporter HTTP/proto, `OTEL_SERVICE_NAME=mi-app` setea `service.name`, `OTEL_TRACES_SAMPLER_ARG=0.1` configura sampling (default 1.0 = todo, clamp `[0.0, 1.0]`). Sin env var → exporter no se instala (zero overhead default). Opt-out por handler con `@server(observability=false)` que bypassa todo el wrapper (handlers bare-metal). Paridad bit-a-bit `fitz run` ↔ `fitz build`. **Único lenguaje moderno** que combina HTTP nativo + WebSockets + Jobs + ORM + **observability ciudadana en el core del compilador**, con OpenTelemetry collector compatible (Jaeger, Tempo, Honeycomb, Datadog, etc.) — sin `pip install opentelemetry-*`, sin `cargo add tracing-opentelemetry`, sin glue manual. → [cap 33 de la guía](https://thegreekman76.github.io/fitz/guide/#33-observability--logs-spans-métricas-otel).

### Benchmark Fitz ORM vs SQLAlchemy

Para validar la promesa "binario nativo sin overhead" del ORM,
mantenemos un **bench reproducible cabeza-a-cabeza** entre los dos
boilerplates equivalentes
([`api-postgres-fitz`](boilerplates/api-postgres-fitz/) vs
[`api-postgres-python`](boilerplates/api-postgres-python/)) — mismo
Postgres, mismos endpoints, misma firma. **Headline numbers en v0.10.13**
(Intel Core Ultra 7 155H, Docker 29.2.1, sustained 30s c=10):

| Métrica | Fitz ORM | Python+SQLAlchemy | Speedup |
|---|---:|---:|---:|
| Memory peak | **9.2 MB** | 51 MB | **5.5x más eficiente** |
| GET /users p50 | **4.88 ms** | 37.85 ms | **7.76x** |
| GET /users RPS | **1944** | 246 | **7.91x** |
| GET /users/{id} p50 | **3.60 ms** | 31.87 ms | **8.85x** |
| GET /users/{id} RPS | **2604** | 296 | **8.80x** |
| Cold start | **0.14 s** | 0.22 s | 1.57x |
| Image size | 131 MB | 258 MB | 2x más liviano |

**Reproducí los números** con [`bash benchmarks/orm-vs-sqlalchemy/run.sh`](benchmarks/orm-vs-sqlalchemy/)
(~5-8 min con cache Docker caliente; requiere `oha` + `jq`). Resultados
publicables y detalle técnico en
[`benchmarks/orm-vs-sqlalchemy/README.md`](benchmarks/orm-vs-sqlalchemy/README.md).

#### Bench complementario — Mixed workload (Fitz vs Python+SQLAlchemy vs Node+Prisma)

Bajo carga peak realista (60% reads + 40% writes, VUs rampeando
10→100→50 sobre 3 min via [`k6`](https://k6.io/) con dominio
`users + posts` con FK), comparando los tres stacks side-by-side:

| Métrica | Fitz | Python+SQLAlchemy | Node+Prisma | Fitz vs Python | Fitz vs Node |
|---|---:|---:|---:|---:|---:|
| Memory peak | **14 MB** | 61 MB | 163 MB | **4.4x** | **11.7x** |
| Mixed p50 | **4.58 ms** | 165 ms | 14.7 ms | **36.2x** | **3.2x** |
| Mixed p95 | **11.07 ms** | 502 ms | 69.3 ms | **45.4x** | **6.3x** |
| Mixed p99.9 | **45.22 ms** | 839 ms | 173 ms | **18.6x** | **3.8x** |
| Mixed RPS | **463.1** | 164.0 | 392.6 | **2.82x** | 1.18x |
| Writes-only p95 | **12.5 ms** | 392 ms | 69.2 ms | **31.3x** | **5.5x** |
| Writes-only RPS | **846.9** | 169.6 | 577.4 | **4.99x** | 1.47x |
| Cold start | **0.15 s** | 0.81 s | 2.22 s | 5.4x | **14.8x** |

Bajo el peak de 100 VUs, **Python+SQLAlchemy satura** (p95 = 503 ms,
cola de medio segundo por requests CRUD triviales — sin timeouts
pero con cola). **Fitz mantiene <50 ms hasta el p99.9** y **Node+Prisma
queda en el medio** con 11.7x más memoria. **Reproducí** con
[`bash benchmarks/mixed-workload/run.sh`](benchmarks/mixed-workload/)
(~25-35 min; requiere `k6` + `jq`). Detalle reproducible en
[`benchmarks/mixed-workload/README.md`](benchmarks/mixed-workload/README.md).

♥ **HTTP client outbound built-in (mini-tanda 2026-06-18)** — módulo `http` con 6 builtins async (`http.get/head/post/put/delete/request`) que devuelven `Future<Result<HttpClientResponse>>`. Body shapes `Str` / `Map<Str, Any>` (auto-JSON con `Content-Type: application/json`) / `Bytes`. Tipo built-in nuevo `HttpClientResponse { status, body, headers, duration_ms }` paralelo a `Request`/`Response` del HTTP server-side. Cinco diferenciales que vuelven a Fitz único acá: (1) **built-in del lenguaje** (parte del binario `fitz`, no `pip install requests` / `npm install axios` / `cargo add reqwest` / import del std); (2) **paridad bit-a-bit `fitz run` ↔ `fitz build`** — el binario standalone tiene el cliente HTTP linkeado estático con `rustls-tls` (sin openssl en el host); (3) **async ciudadano de primera** — se integra natural con `@cron`/`@background`/handlers HTTP/`spawn(...)`; (4) **`Result<T>` automático** — errores de transporte como valores, status 4xx/5xx NO son Err (el user mira `r.status`), el `?` propaga, el checker exige manejo (regla 5.3.3); (5) **sin deps externas en el host** — `reqwest` queda estático, `rustls-tls` no exige openssl. Ningún lenguaje moderno del cuadro provee HTTP client outbound como builtin del lenguaje con esta combinación. Plan de implementación en [`docs/http-client-roadmap.md`](docs/http-client-roadmap.md). → [cap 17 de la guía](https://thegreekman76.github.io/fitz/guide/#17-http-nativo) (sub-sección "HTTP client outbound") + ejemplos runnable [`17e`](examples/guide/17e-http-client-basico.fitz)/[`17f`](examples/guide/17f-http-client-errores.fitz)/[`17g`](examples/guide/17g-http-client-webhook.fitz)/[`17h`](examples/guide/17h-http-client-health-checker.fitz).

✉ **SMTP outbound built-in (mini-tanda 2026-06-19)** — módulo `smtp` con `smtp.send(opts) -> Future<Result<SmtpResult>>` async, paralelo bit-a-bit al HTTP client. Required keys `to`/`subject` + al menos uno de `body`/`body_text`/`body_html`; `from` opcional si `SMTP_FROM` env var seteada. Texto plano + HTML juntos → **multipart/alternative automático** (cliente moderno gana HTML, fallback texto). Tipo built-in nuevo `SmtpResult { delivered: Bool, message_id: Str, duration_ms: Int }`. Configuración por env vars al boot (`SMTP_HOST`/`SMTP_PORT`/`SMTP_USER`/`SMTP_PASSWORD`/`SMTP_FROM`/`SMTP_TLS`), pool de conexiones TCP/TLS reusable adentro de lettre. Modelo de errores paralelo al HTTP client: `Result::Err(Str)` con prefijo `"smtp: "` para fallos de transporte (DNS, conexión, auth, TLS handshake, server reject); status 5xx del SMTP server cuenta como Err (lettre acepta el relay solo con 250). Cinco diferenciales (paralelo a HTTP client): (1) **built-in del lenguaje** (no `pip install yagmail` / `npm install nodemailer` / `cargo add lettre` / Maven `JavaMail`); (2) **paridad bit-a-bit `fitz run` ↔ `fitz build`** — `lettre` linkeado estático con `tokio1-rustls-tls`, sin openssl en el host; (3) **async ciudadano de primera** — se integra natural con `@cron` (digests/reportes), `@background`+`spawn(...)` (welcome emails sin bloquear la signup response), y handlers HTTP server-side (magic-link auth); (4) **`Result<T>` automático** — el `?` propaga errores de transporte, el `match` clasifica, el checker exige manejarlos; (5) **sin deps externas en el host** — el binario standalone NO requiere SMTP libraries del SO. **Ningún lenguaje moderno del cuadro provee SMTP outbound como builtin del lenguaje con esta combinación** — Python necesita `pip install yagmail` (`smtplib` stdlib es RFC 5321 low-level), JS/Node `npm install nodemailer`, Rust `cargo add lettre`, Java `JavaMail` con Maven; solo Go tiene `net/smtp` en stdlib pero es low-level. → [cap 17 de la guía](https://thegreekman76.github.io/fitz/guide/#17-http-nativo) (sub-sección "SMTP outbound") + ejemplos runnable [`17i`](examples/guide/17i-smtp-basico.fitz)/[`17j`](examples/guide/17j-smtp-errores.fitz)/[`17k`](examples/guide/17k-smtp-magic-link.fitz) contra MailHog local.

‼ **Cross-module `@middleware(fn)` (v0.19.5, 2026-06-27)** — el patrón canónico de SaaS HTTP "módulo dedicado a cross-cutting concerns + handlers en módulos separados" ahora compila a binario nativo. Antes, una fn `mw_strict(req: Request)` viviendo en `rate_limit.fitz` con `return 429 { ... }` aplicada como `@middleware(mw_strict)` desde `auth.fitz` rompía `fitz build` con 3 errores distintos del codegen (checker del loader rechazaba `return <status>` cross-module, módulo emitido sin `use crate::{Request, RequestData}`, main rechazaba la ident imported). El fix propaga el set GLOBAL de `@middleware` references al checker y codegen de cada módulo (paralelo a W12 `@auth_provider` + B10 `@background`), acepta imports en el build-time check de main, y emite las `use crate::{...}` que faltaban. Bonus: async middleware fns cross-module funcionan también (`emit_middleware_chain` detecta `Future<_>` ret y emite `.await` suffix). **Único patrón cross-module HTTP que faltaba**: el stack web first-class del codegen ahora soporta middleware modular completo, eliminando los workarounds inline que encarecían proyectos Fitz HTTP reales. → [cap 17 sub-sección "Middleware y CORS" (cross-module)](https://thegreekman76.github.io/fitz/guide/#middleware-y-cors).

◄ **`Response { ... }` built-in (v0.19.0, 2026-06-21)** — type built-in con 5 fields (`status: Int = 200` / `content_type: Str = "application/json"` / `headers: Map<Str, Str> = {}` / `body: Str = ""` / `body_bytes: Bytes? = null`) que cubre el caso entero de respuestas HTTP **non-JSON**: RSS, Atom, plain text, HTML estático, CSV exports, SVG inline, **PDF binarios**, ZIP downloads. El handler retorna `Response { content_type: "application/rss+xml", body: rss_xml }` directamente y el wrapper detecta el tipo estática y emite la response axum con `Content-Type` custom + headers + body crudo (sin JSON-wrap). Para payloads NO UTF-8 (PDF, ZIP, imágenes), `body_bytes: Bytes?` reemplaza `body` (validación XOR build-time si seteás ambos literales). Cinco diferenciales: (1) **built-in del lenguaje** validado por el checker estático, cero `pip install` (FastAPI) / `npm install` (Express) / `import` (Flask, Spring); (2) **paridad bit-a-bit `fitz run` ↔ `fitz build`** validada a mano con `curl` + `xxd` sobre los 4 casos canónicos (RSS, plain text, SVG, PDF binario); (3) **OpenAPI 3.1 auto-documentado** — `responses.200.content.<media_type>` automático con `format: binary` cuando hay `body_bytes`, cero esfuerzo del user — herramientas como Scalar / Swagger UI rendean los endpoints con sus media types reales; (4) **tipo built-in con 5 fields explícitos**, sin builders mutables ni reflection runtime; (5) **validación XOR build-time** rechaza `body` literal no-vacío + `body_bytes` antes del runtime con mensaje claro citando workarounds, UX mejor que esperar el 500. **Ningún lenguaje moderno del cuadro provee respuestas HTTP non-JSON como tipo built-in del lenguaje con esta combinación** — FastAPI lo cubre con `Response(content=..., media_type=...)` pero requiere `from fastapi.responses import Response`; Express lo cubre con `res.type().send()` encadenamiento mutable sin tipo; Flask con `Response(..., mimetype=...)` igual que FastAPI; Spring con `ResponseEntity.ok().contentType(...).body(...)` builder verbose. → [cap 17 de la guía](https://thegreekman76.github.io/fitz/guide/#17-http-nativo) (sub-sección "Respuestas con Content-Type custom (Response {...})") + ejemplo runnable [`17l`](examples/guide/17l-response-custom.fitz) con los 4 casos canónicos + endpoint /health JSON normal para regresión.

§ **Interop Python via PyO3**. Marshaling bidireccional `List`/`Map`/`Instance` ↔ `list`/`dict`, excepciones Python → `Result<T>`, bridge async tokio ↔ asyncio, `fitz py-types` auto-mapeo SQLAlchemy. Opt-in con feature `python`. **`fitz build --bundle-python` produce un binario standalone con CPython 3.14.5 embebido** (~22-35 MB según OS) — corre en cualquier máquina del triple destino sin Python instalado, sin `pip install`, sin runtime externo. **Con `--bundle-pip <paquete>` repetible** (Fase 8.c) o **`--bundle-pip-requirements <FILE>`** (cosecha 8.c v0.9.42 — lee del `requirements.txt` estándar), también empaqueta paquetes pip adentro: `fitz build --bundle-pip-requirements requirements.txt mi_app.fitz` produce un binario único de ~50 MB que incluye CPython + las deps + tu código. **Desde v0.9.46 el launcher usa crates `tar`+`flate2` inline (sin subprocess `tar`)**, habilitando runtimes minimalistas tipo `gcr.io/distroless/cc-debian12` (~22 MB base). **Smoke real Docker validado end-to-end con Postgres** (v0.9.50/52) en los boilerplates 5/6 — imagen final ~136 MB con sqlalchemy + psycopg2-binary embebidos. **En el destino: cero Python, cero pip, cero venv**. (Para el dev que buildea sí hace falta Python local — cualquier 3.10+ en Windows, 3.14.x en Linux/macOS hasta cerrar `R.bug-pyo3-abi3-portable-link`. Tabla completa de matices en el cap 21.12 de la guía.) **El único lenguaje del cuadro que hace esto** (Python necesita Python + venv + `pip install`, FastAPI necesita uvicorn + venv, Spring necesita JVM, Express necesita Node + npm install, Go no tiene interop Python). PyOxidizer hizo algo parecido para Python puro pero está ralentizado desde 2023; Fitz reimplementa el patrón sobre [python-build-standalone de Astral](https://github.com/astral-sh/python-build-standalone). → [cap 21](https://thegreekman76.github.io/fitz/guide/#21-interop-python).

## Estabilidad

Fitz está construido sobre Rust, que tiene un compromiso de estabilidad fuerte desde 2015: código que compila en una versión estable sigue compilando en versiones futuras, y los cambios que podrían romper se aíslan en _editions_ opt-in.

Encima de eso, en este repo:

- `rust-toolchain.toml` pinea la versión exacta de Rust con la que Fitz se construye. Cloná el repo y `rustup` baja esa versión sola — no importa qué Rust tengas instalado globalmente.
- `rust-version` en `Cargo.toml` documenta la versión mínima soportada. Cargo da un error claro si alguien intenta con una más vieja.
- `Cargo.lock` fija las versiones exactas de todas las dependencias transitivas, así que builds reproducibles entre máquinas y en el tiempo.

En la práctica: un cambio en Rust o en una dependencia no rompe Fitz hasta que vos decidas subir las versiones de manera explícita.

El estado actual del proyecto (fases cerradas, próximo norte, deudas comprometidas) vive en [`docs/roadmap.md`](docs/roadmap.md) y en el [CHANGELOG](CHANGELOG.md).

## Cómo empezar

**1. Instalar Fitz.** One-liner para Linux / macOS / Windows:

```bash
# Linux / macOS
curl -sSf https://thegreekman76.github.io/fitz/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://thegreekman76.github.io/fitz/install.ps1 | iex
```

El installer baja el último release, extrae los binarios a `~/.fitz/bin/` (o `%USERPROFILE%\.fitz\bin\` en Windows) y te dice cómo agregar el dir al `PATH`. Soporta `--version vX.Y.Z`, `--prefix <ruta>` y `--uninstall`.

Alternativas: bajar el binario manual desde [releases](https://github.com/Thegreekman76/fitz/releases/latest) (Linux x64/ARM, macOS ARM, Windows x64) o compilar desde fuente:

```bash
git clone https://github.com/Thegreekman76/fitz.git
cd fitz
cargo build --release
# El binario queda en target/release/fitz
```

Detalle completo (incluyendo VSCode + extensión + troubleshooting) en el [cap C1 del curso](https://thegreekman76.github.io/fitz/curso/m1-setup/c1-instalacion/).

**2. Tu primer programa.**

```fitz
// hola.fitz
print("Hola desde Fitz 🏔️")

name = "Patagonia"
print("Hola, {name}!")
```

```bash
fitz run hola.fitz
```

**3. Seguir aprendiendo.** Tres puertas de entrada según con qué venís:

- **¿Desde cero, querés que te lleven paso a paso?** El [**curso `Fitz de 0 a experto`**](https://thegreekman76.github.io/fitz/curso/) arranca con la instalación y crece capítulo a capítulo hasta una app real con Postgres + ORM + Docker.
- **¿Sabés programar, querés referencia feature por feature?** La [**guía del lenguaje**](https://thegreekman76.github.io/fitz/guide/) cubre todo lo implementado en español, con ejemplos ejecutables. Empezá por el [cap 2 — Tu primer programa](https://thegreekman76.github.io/fitz/guide/#2-tu-primer-programa).
- **¿Buscás la especificación completa** (incluye features futuras)? Ver [docs/syntax-spec.md](docs/syntax-spec.md).

## Boilerplates

Plantillas Dockerizadas listas para arrancar proyectos reales. Cada una tiene README exhaustivo con paso a paso, troubleshooting y plan de producción. Detalle completo en [`boilerplates/README.md`](boilerplates/README.md).

| Boilerplate | Qué demuestra |
|-------------|---------------|
| [`cli-tool`](boilerplates/cli-tool/)                           | CLI puro — sales report con métodos funcionales. Binario nativo distroless ~30 MB.                          |
| [`api-simple`](boilerplates/api-simple/)                       | REST API tipada + OpenAPI 3.1 + UI Scalar autogenerados.                                                    |
| [`api-middleware-cors`](boilerplates/api-middleware-cors/)     | Auth nativa JWT + Argon2 + middleware encadenado + CORS cross-origin + frontend vanilla.                    |
| [`api-websocket`](boilerplates/api-websocket/)                 | WebSockets tipados (`WsConn<T>`) con broadcast + heartbeat + frontend chat vanilla.                         |
| [`api-postgres-python`](boilerplates/api-postgres-python/)     | CRUD multi-archivo con SQLAlchemy + Postgres (compose 2 servicios). **Variante `Dockerfile.distroless` validada v0.9.50** — imagen ~136 MB. |
| [`api-fullstack-postgres`](boilerplates/api-fullstack-postgres/) | **Showcase fullstack** — API + frontend rico (tabla, edit inline, filtros) + Postgres (compose 3 servicios). **Variante distroless validada v0.9.52** — incluye CORS preflight desde otro origin. |

Quickstart genérico:

```bash
cd boilerplates/<nombre>
cp .env.example .env   # si existe
docker compose up --build   # o `docker build .`
```

## CLI

Verificá la instalación con `fitz --version` y `fitz --help`.

**Lenguaje — intérprete y compilador**

| Comando                       | Qué hace                                                                       |
| ----------------------------- | ------------------------------------------------------------------------------ |
| `fitz run [archivo]`          | Ejecuta el archivo (o el `[bin].main` del `fitz.toml`) con checker strict.     |
| `fitz build [archivo]`        | Compila a binario nativo standalone (~5 MB, sin runtime de Fitz en el destino). |
| `fitz check [archivo]`        | Valida tipos y sintaxis sin ejecutar. Exit 1 si hay errores.                   |
| `fitz openapi <archivo>`      | Emite el schema OpenAPI 3.1 del programa a stdout. Útil para CI.               |

**Package manager**

| Comando                       | Qué hace                                                                       |
| ----------------------------- | ------------------------------------------------------------------------------ |
| `fitz new <nombre>`           | Crea un proyecto Fitz nuevo en una carpeta. `--http` para template HTTP.       |
| `fitz init`                   | Inicializa un proyecto en el directorio actual.                                |
| `fitz add <dep> --path/--git` | Agrega una dep al `fitz.toml` y sincroniza el `fitz.lock`.                     |
| `fitz remove <dep>`           | Quita una dep del `fitz.toml`.                                                 |
| `fitz update [dep]`           | Re-resuelve deps. Para git deps invalida cache y re-clona.                     |

**Developer experience**

| Comando                       | Qué hace                                                                       |
| ----------------------------- | ------------------------------------------------------------------------------ |
| `fitz fmt [archivos]`         | Formatea código Fitz a estilo canónico, cero config. `--check` para CI.        |
| `fitz test [filter]`          | Corre fns con `@test`. Output estilo cargo (ok/FAILED + summary + exit code).  |
| `fitz dev [--file]`           | Hot reload — re-arranca el programa cuando un `.fitz` o `fitz.toml` cambia.    |
| `fitz repl`                   | REPL interactivo con env persistente, multi-line, history, comandos `:type`/`:env`/`:load`. |
| `fitz lint [archivos]`        | Linter de patrones (unused_variable, unused_import, useless_match, string_concat). |

**Interop Python** (requiere `cargo build --release --features python`)

| Comando                       | Qué hace                                                                       |
| ----------------------------- | ------------------------------------------------------------------------------ |
| `fitz py-types <archivo.py>`  | Genera `type` Fitz desde modelos SQLAlchemy. Output a stdout o `--out`.        |

**Deployment** (Fase 12.4)

| Comando                       | Qué hace                                                                       |
| ----------------------------- | ------------------------------------------------------------------------------ |
| `fitz docker init [--force]`  | Genera `Dockerfile` + `.dockerignore` + `docker-compose.yml` smart por defecto. Detecta `@server(port)`, `db.X(...)`, `from python import X`, `@cron` y adapta runtime + compose. |
| `fitz docker build [--tag X]` | Thin wrapper sobre `docker build -t <tag> .`. Default `<package.name>:latest`. |

## Extensión VSCode

Highlighting + LSP (diagnostics, hover, go-to-definition, autocomplete contextual) con el binario `fitz-lsp` bundleado en cada `.vsix` por plataforma.

**Instalar desde releases (recomendado)**:

1. Bajá el `.vsix` correspondiente a tu OS/arquitectura de [releases](https://github.com/Thegreekman76/fitz/releases/latest):
   - **Windows x64**: `fitz-lang-win32-x64.vsix`
   - **Linux x64**: `fitz-lang-linux-x64.vsix`
   - **Linux ARM64**: `fitz-lang-linux-arm64.vsix`
   - **macOS Apple Silicon (M1/M2/M3)**: `fitz-lang-darwin-arm64.vsix`
2. En VSCode: `Ctrl+Shift+P` → "Extensions: Install from VSIX..." → seleccioná el archivo bajado.
3. Listo — abrí cualquier `.fitz` y vas a tener errores subrayados al tipear, hover con tipos, F12 para go-to-definition, y autocomplete contextual.

> Nota: las plataformas en la matriz de release son las 4 listadas arriba. macOS Intel (`darwin-x64`) y Windows ARM64 (`win32-arm64`) no están — el primero por escasez crónica de runners macos-13 en GitHub Actions, el segundo porque axum aún no compila estable en ese target. Si necesitás alguna de esas, build local (próxima sección) funciona idéntico.
>
> Cuando se cree la cuenta de publisher en el VSCode Marketplace, la extensión va a estar instalable en un clic desde la UI de Extensions buscando `fitz`. Por ahora releases en GitHub es el camino canónico.

**Build local** (alternativa, si querés trackear `main` o no encontrás tu plataforma en releases):

```bash
cd editors/vscode
npm install
npm run build:vsix     # produce un `.vsix` para tu plataforma actual
```

Detalle completo en [cap 22 de la guía](https://thegreekman76.github.io/fitz/guide/#22-soporte-para-editores).

## Nombre

**Fitz** por el Fitz Roy — la montaña más icónica de la Patagonia, en El Chaltén, Argentina. Un nombre que no se olvida.

## Autor

Desarrollado en El Chaltén, Santa Cruz, Argentina 🇦🇷
Por un developer independiente que quería un lenguaje que no tuviera que disculparse por nada.

TheGreekMan (Palopoli Martín)

### Cómo se construye

Una nota honesta sobre el proceso, porque me parece más
interesante que esconderlo:

La idea, el roadmap, las decisiones de diseño y la dirección
del lenguaje son mías. Soy dev full-stack (Python, FastAPI,
Vue, Docker, Postgres) y aprendí Rust específicamente para
construir Fitz. Cada decisión del lenguaje — por qué HTTP es
ciudadano de primera, por qué `Result` y no excepciones, por
qué `@table` se valida en compile-time — sale de frustraciones
concretas de mi trabajo diario.

El día a día de la implementación es colaborativo con Claude
(Anthropic). El flujo típico: yo describo lo que quiero,
discutimos trade-offs, escribo o me sugiere código, lo reviso,
itero. Cuando me trabo con un sub-paso complejo, a veces caigo
en lo que la comunidad llama *vibe coding* — describir el
problema en voz alta y refinar juntos hasta que sale. Otras
veces es revisión de código pura, o debugging compartido.

El SHA del commit es mío, la responsabilidad técnica es mía,
pero el proceso real es de a dos. Lo digo porque me parece más
honesto que vender una historia de "lo hice solo a la
medianoche" que no es como funcionó.

Si te resuena el modelo, los commits cuentan la historia fina:
qué pensé, qué probé, qué descarté.

## Licencia

MIT
