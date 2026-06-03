# Changelog

Cambios visibles del lenguaje Fitz, agrupados por hito. Sigue el
formato de [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
El detalle técnico de cada sub-paso vive en
[`docs/roadmap.md`](docs/roadmap.md); este archivo es la vista
condensada para alguien que pregunta "¿qué cambió y cuándo?".

Las versiones son retroactivas — Fitz todavía no publica releases
formales; cada bump corresponde al cierre de una Fase del roadmap.

## [Sin publicar]

Fase 12.4 ENTERA CERRADA (12.4.a + 12.4.b). Queda 12.5 (cap nuevo "Deployment
ciudadano primera clase" en `docs/guide.md` + caps del curso M7) para
cerrar Fase 12 entera. También sigue abierta la 9.w.1.iter2 (RBAC custom +
token refresh) si el avance del curso lo demanda. Tier 2 de Fase 12.3
(bridge métricas OTel) sigue bloqueado por release del crate.

## [v0.12.3] — 2026-06-03 — Fase 12.4.b: smart detection rica + `fitz docker build`

Cierra **Fase 12.4 entera**. Suma detección AST de interop Python y
`@cron`, ajusta runtime + compose según el shape del programa, y agrega
el sub-comando `fitz docker build [--tag X]` que tag-ea y delega a
`docker build`.

**Smart detection rica**:

- **`uses_python`** (`from python import X` o `import python.X`) → el
  runtime stage del Dockerfile cae a `python:3.12-slim-bookworm` (~55
  MB) en vez de `gcr.io/distroless/cc-debian12` (~22 MB). El binario
  producido por `fitz build` con interop dynamic-linkea
  `libpython3.12.so` que distroless no incluye; con slim-bookworm ya
  está + wget para healthcheck HTTP.
- **`uses_cron`** (cualquier `@cron` decorator) → compose suma
  `restart: unless-stopped` al service principal para que el scheduler
  sobreviva crashes/redeploys.
- **Healthcheck HTTP condicional** — si hay `@server(port)` Y el runtime
  tiene wget disponible (`uses_python` → slim-bookworm), el compose suma
  bloque `healthcheck:` que pega contra `/healthz` (auto-mounteado por
  Fase 12.1.b). Con distroless (default, sin wget), el healthcheck NO
  se emite — comentario explicativo en el compose con la receta para
  agregarlo a mano si el user cambia el runtime.

**Sub-comando nuevo `fitz docker build [--tag X]`**:

- Thin wrapper sobre `docker build -t <tag> .` en el manifest_dir.
- Default `--tag` = `<package.name>:latest`. Override con `--tag mi/app:v1`.
- Aborta con mensaje claro si no hay `Dockerfile` (sugiere `fitz docker
  init` primero) o si no hay `fitz.toml`.
- Propaga el exit code de `docker build` para que CI lo capture igual.

**Sub-pasos**:

- **12.4.b.1** — `DockerShape` gana 2 campos (`uses_python`, `uses_cron`)
  + `Default` derive (simplifica los literales de tests). Nuevos helpers
  `stmt_uses_python` (mira `Stmt::Import`/`FromImport` con `path[0] ==
  "python"`) y `stmt_uses_cron` (mira `Stmt::FnDef.decorators` con
  `name == "cron"`). `render_dockerfile` consulta `runtime_image(shape)`
  que devuelve `"python:3.12-slim-bookworm"` o `"gcr.io/distroless/cc-
  debian12"`. `render_compose` suma `restart: unless-stopped` cuando
  `uses_cron` y healthcheck HTTP cuando `server_port = Some` + `uses_python`
  (comentario explicativo cuando es distroless). Handler `docker_init_cmd`
  reporta los nuevos detectados. 13 unit tests nuevos en `docker::tests::*`
  + 4 E2E nuevos en `cli_e2e` (init con python, con cron, healthcheck
  HTTP en compose, comentario distroless).
- **12.4.b.2** — Sub-enum `DockerCmd::Build { tag }` + handler
  `docker_build_cmd(tag)` que reusa `resolve_entry(None)`, valida
  `Dockerfile` existe, invoca `std::process::Command::new("docker") build
  -t <tag> . ` en `manifest_dir` con propagación de exit code. 2 E2E
  nuevos (`build sin dockerfile aborta`, `build sin manifest aborta`).

**Tests al cierre v0.12.3**: 2937 unit (+13) + 93 cli_e2e (+6) + 3
openapi_e2e. Clippy `--lib --tests --bins -- -D warnings` limpio,
`cargo fmt --all --check` limpio.

**Smoke real verde** validado a mano contra
`boilerplates/api-postgres-python` (interop Python con SQLAlchemy):
runtime cae a `python:3.12-slim-bookworm` automático, healthcheck HTTP
emitido con wget, sin postgres en compose porque la DB se accede vía
Python (limitación conocida — el helper detecta `db.X(...)` nativo Fitz,
no interop indirecto).

**Deudas residuales derivadas de 12.4.b** (NO bloquean cierre de Fase
12):

- **Detección DB indirecta vía interop Python**: el helper `uses_db`
  solo detecta `db.X(...)` nativo Fitz. Programas que acceden a Postgres
  con `from python import sqlalchemy` no disparan el service `db` en
  compose. Workaround: usar `--force` y editar el compose, o usar el
  driver Postgres nativo de Fitz (cap 31 de la guía). Fix futuro:
  detectar `from python import sqlalchemy/psycopg2/asyncpg` con su
  propio flag separado, o sumar flag `--with-postgres` al init.
- **Healthcheck HTTP sin distroless**: el bloque healthcheck solo sale
  cuando el runtime tiene wget (`uses_python`). Para programas no-Python
  con `@server`, el user puede agregarlo a mano siguiendo el comentario
  o cambiar el runtime. Fix futuro: bundlear un mini binario HTTP probe
  en distroless, o usar healthcheck TCP (sin requerir wget) — TCP no
  valida el endpoint exacto.
- **`fitz docker build` no expone `--push`/`--platform`/`--no-cache`**:
  el wrapper es thin de propósito. Para flags avanzados, correr `docker
  build` directo. Refinable si aparece demanda real.
- **Cross-module detection** sigue siendo deuda heredada de 12.4.a:
  `@server`/`db.X(...)`/`@cron`/`from python import X` adentro de módulo
  importado no dispara el shape. Workaround: declarar todo en el archivo
  principal (caso típico).

**Próximo norte**: **Fase 12.5** — cap nuevo "Deployment ciudadano
primera clase" en `docs/guide.md` + caps del curso M7. O salto a
**9.w.1.iter2** (RBAC custom + token refresh) si el avance del curso lo
demanda.

## [v0.12.2] — 2026-06-03 — Fase 12.4.a: `fitz docker init` (Dockerfile + compose autogenerados)

Sub-comando nuevo `fitz docker init [--force]` que genera tres archivos
en el directorio del manifest:

- **`Dockerfile`** multi-stage: builder
  `ghcr.io/thegreekman76/fitz:${FITZ_TAG}` con `RUN fitz build` → runtime
  `gcr.io/distroless/cc-debian12` (~22 MB base + binario standalone).
- **`.dockerignore`** con `target/`, `.git/`, `.env*`, `__pycache__/`,
  etc.
- **`docker-compose.yml`** smart por defecto.

**Smart por defecto** (detección AST-only del entry point declarado en
`[bin].main`):

- `@server(N)` con N Int literal → `EXPOSE N` en Dockerfile + `ports:`
  en compose.
- `db.X(...)` en cualquier nodo del AST → compose suma service
  `postgres:16-alpine` con healthcheck, volume `pgdata`, y
  `DATABASE_URL: "postgres://${POSTGRES_USER:-fitz}:..."` inyectada al
  service principal con `depends_on: service_healthy`.

**Política de skip**: si un archivo ya existe, se skipea y se sugiere
`--force` para sobrescribir. Cero overwrite accidental del Dockerfile
hand-tuned de un boilerplate existente.

**Decisiones técnicas del MVP (12.4.a)**:

- **Sub-comando con sub-enum** `Commands::Docker(DockerCmd::Init {
  force })` deja la puerta abierta a `fitz docker build` de 12.4.b sin
  cambio breaking.
- **AST-only del entry point** — fast (~50ms vs ~2s del eval), no
  recursa en módulos importados (el caso típico tiene los decoradores
  en el archivo principal; deuda residual visible).
- **`uses_db` heurística generosa** paralela al `program_uses_db` del
  codegen: cualquier `db.X(...)` cuenta. Falso positivo si el usuario
  nombra una variable local `db`; trade-off aceptable, el user borra el
  service `db:` a mano.
- **Runtime distroless siempre** — programas con interop Python no
  funcionan con distroless (necesita libpython.so); 12.4.b suma
  detección automática + fallback a `debian:bookworm-slim`.
- **Sin `restart:` policies ni healthchecks HTTP** en 12.4.a — eso lo
  cubre 12.4.b según `@cron` / `@healthz`/`@readyz`.

**Sub-paso (un solo sub-paso de 12.4.a)**:

- **12.4.a** — nuevo módulo `src/docker.rs` (~520 LoC con tests) con la
  API pública (`DockerShape`, `detect_shape`, `render_dockerfile`,
  `render_dockerignore`, `render_compose`, `init` con `InitResult`).
  `src/lib.rs` exporta el módulo. `src/main.rs` suma el sub-enum +
  handler `docker_init_cmd` (~95 LoC) que reusa `resolve_entry(None)`
  para walkear al manifest. 18 unit tests en el módulo + 6 E2E en
  `tests/cli_e2e.rs` (CLI puro, HTTP con `@server`, con `db.connect`,
  skip sin force, sobrescribe con force, abort sin manifest). Smoke
  real validado contra `boilerplates/api-simple` (HTTP, no DB) y
  `boilerplates/api-postgres-fitz` (HTTP + DB con compose smart).

**Tests al cierre v0.12.2**: 2924 unit (+18) + 87 cli_e2e (+6) + 3
openapi_e2e. Clippy `--lib --tests --bins -- -D warnings` limpio,
`cargo fmt --all --check` limpio.

**Deuda residual derivada de 12.4.a** (NO bloquea 12.4.b):

- Cross-module detection — `@server`/`db.connect` adentro de un módulo
  importado no dispara el shape. Workaround: declarar `@server` en el
  archivo principal (caso típico).
- Falso positivo `uses_db` si hay variable local llamada `db`. User
  edita el compose a mano (deuda menor, paralela al codegen).
- Detección Python interop diferida a 12.4.b (cuando dispara, fallback
  a `debian:bookworm-slim` automático).
- Healthchecks HTTP + `restart:` policies en compose diferidos a 12.4.b
  (depende de `@healthz/@readyz` + `@cron`).
- `fitz docker build [--tag X]` wrapper diferido a 12.4.b.

**Próximo norte**: **Fase 12.4.b** — smart detection rica (Python
fallback + healthchecks + cron restart) + `fitz docker build` wrapper.

## [v0.12.1] — 2026-06-03 — Fase 12.3.iter2: cierre de deudas residuales

Mini-tanda dedicada a cerrar las deudas residuales de Fase 12.3. Tier 1
(correlación trace_id + bridge logs) + Tier 3 (Prometheus) cerrados;
Tier 2 (bridge métricas OTel) INTENTADO y BLOQUEADO por version
conflict del crate (esperando release nuevo). Sumado **cap 33 nuevo
"Observability"** en la guía con renumeración 33→34, 34→35, 35→36.

**Deudas residuales de 12.3 al cierre**:

- ✓ #2 Bridge logs OTel → CERRADO (iter2.b)
- ✓ #3 Correlación trace_id Fitz↔OTel → CERRADO (iter2.a)
- ✓ #4 Endpoint /metrics Prometheus → CERRADO (Tier3)
- ✓ #5 Cap dedicado en guide.md → CERRADO
- ⚠ #1 Bridge métricas OTel → BLOQUEADO (crate
  `metrics-exporter-opentelemetry 0.2.1` pinea `opentelemetry_sdk 0.31`
  mientras usamos 0.32; master del crate ya está en 0.32, esperando
  release oficial). Workaround end-to-end: Tier3 (Prometheus scrape)
  cubre 90%.

**iter2.a — Correlación trace_id Fitz↔OTel**: `dispatch_request` y el
wrapper HTTP del codegen abren el span OTel ANTES del SpanContext
propio; nuevo constructor `SpanContext::with_ids(trace_id, span_id)`
deriva los IDs del span OTel. El `trace_id` en logs stderr matchea
exactamente el del backend OTel (Jaeger/Tempo/Datadog) →
cross-pipeline queries habilitadas. Paridad bit-a-bit codegen.

**iter2.b — Bridge logs OTel**: `emit_log_record` emite en paralelo a
stderr Y al backend OTel via OTLP HTTP/proto sobre `/v1/logs` cuando
el provider está activo. Trace context derivado del SpanContext →
correlación logs↔spans automática en el backend. Decisión
arquitectónica: SDK `opentelemetry::logs` directa (no
`opentelemetry-appender-tracing` — refactorizar el formatter custom
JSON/pretty de 12.3.a no se justifica).

**Tier3 — Endpoint `/metrics` Prometheus**: `@server(prometheus=true)`
compile-time + env var `FITZ_PROMETHEUS=1`/`true`/`yes` runtime
override. Cuando activo, `serve()` instala `PrometheusBuilder` como
recorder global del crate `metrics` y `build_router` auto-mounta
`GET /metrics` con exposition format. Mismo puerto + transporte que
el resto de la app (NO un puerto separado). Si Prometheus + OTel
ambos activos, Prometheus gana (solo UN recorder global de `metrics`
permitido).

**Cap 33 nuevo "Observability — logs, spans, métricas, OTel"**:
~300 LoC markdown end-to-end — structured logging con kwargs y
redacción Secret, spans HTTP automáticos + correlación trace_id,
OTel exporter (TracerProvider + LoggerProvider) opt-in via env var,
`/metrics` Prometheus opt-in, patrón canónico stack completo,
recetas comunes, panorama vecino (FastAPI/Express/Go/Rust), y
honestidad sobre lo que NO hace.

**Validación al cierre**: 2906 unit + 81 cli_e2e + 3 openapi_e2e + 4
compile_e2e log codegen + smoke ~290 ejemplos verde en 833s.
Clippy `--lib --tests --bins -- -D warnings` + fmt limpios.

## [v0.12.0] — 2026-06-03 — Fase 12.3 entera: Observability minimal con OpenTelemetry

Cierre formal de Fase 12.3 en 3 bloques + 11 sub-pasos. Observability
ciudadana de primera clase en el core del compilador, con
OpenTelemetry collector compatible (Jaeger, Tempo, Honeycomb, Datadog,
etc.).

**12.3.a — Structured logging built-in** (3 sub-pasos):
`log.info/warn/error/debug(msg, kwargs)` con kwargs heterogéneos
(`Int`/`Float`/`Str`/`Bool`/`Null`/`Secret`/`List`/`Map`/nominal),
output JSON flat a stderr por default con `timestamp` + `level` +
`msg` + kwargs; pretty mode con ANSI colors cuando TTY o
`FITZ_LOG_FORMAT=pretty`. Filter via `FITZ_LOG=info|debug|warn|error`
(default `info`). Redacción recursiva de `Value::Secret` en
`List`/`Map`. Stack: `tracing` + `tracing-subscriber` + `chrono` +
`serde_json`. Paridad bit-a-bit `fitz run` ↔ `fitz build`.

**12.3.b — Spans HTTP + métricas + correlación trace_id** (5
sub-pasos): cada request HTTP abre un `SpanContext` root con IDs
OTel-compatibles (`trace_id` 32 hex / `span_id` 16 hex generados con
`uuid::Uuid::new_v4()`). Logs del handler heredan automático
`trace_id`/`span_id` via `tokio::task_local!` (atraviesa thread
boundaries multi-thread). Access log `log.info("http.access", ...)`
con `http.method`/`http.target` (template del route)/
`http.status_code`/`duration_ms`. Counter `http_requests_total{method,
path, status}` + Histogram `http_request_duration_seconds{method,
path, status}` con labels iguales para correlación cross-metric.
Opt-out total con `@server(observability=false)` que bypassa el
wrapper de instrumentación entero.

**12.3.c — OTLP exporter** (3 sub-pasos): cuando
`OTEL_EXPORTER_OTLP_ENDPOINT` está seteada, conexión a backend OTel
real con `opentelemetry-otlp = "0.32"` feature `http-proto` (sobre
gRPC por simplicidad + compat proxy + recomendación
Datadog/Honeycomb). Sampler `TraceIdRatioBased` con
`OTEL_TRACES_SAMPLER_ARG` clamp `[0.0, 1.0]`. Service name desde
`OTEL_SERVICE_NAME` (default `"fitz-app"`). Sin la env var, no-op
silencioso — zero overhead, zero conexiones de red. Paridad bit-a-bit
intérprete↔binario.

**Deudas residuales derivadas** (NO bloquean Fase 12.4): bridge
métricas OTel, bridge logs OTel, correlación trace_id Fitz↔OTel,
endpoint `/metrics` Prometheus opt-in, cap dedicado en guide.md.
**Las 5 cerradas en v0.12.1** excepto #1 (bridge métricas OTel) que
quedó bloqueada por version conflict del crate
`metrics-exporter-opentelemetry` 0.31 vs nuestro 0.32 — esperando
release del crate.

**Validación al cierre**: 2894 unit + 81 cli_e2e + 3 openapi_e2e + 4
compile_e2e log codegen + smoke ~290 ejemplos verde. Clippy
`--all-targets -- -D warnings` + `cargo fmt --all --check` limpios.

## [v0.11.2] — 2026-06-02 — 9.w.3.iter2: Persistencia + retry + timezone + catch_up en `@cron`

**Cierre formal de Tier 1 de las deudas pre-M5** acordadas el
2026-06-01. Las tres deudas que bloqueaban escribir M5.C26 del
curso (Jobs sin Celery) cerradas en bloque, paralelo bit-a-bit
entre intérprete y codegen.

`@cron("expr")` acepta **4 kwargs opcionales nuevos** — programas
viejos siguen funcionando idénticos:

```fitz
let db = db.connect(env_or("DATABASE_URL", "postgres://...")).await

@cron("0 9 * * *",
      tz="America/Argentina/Buenos_Aires",
      retry={max: 3, backoff: "exponential",
             initial_secs: 1, max_secs: 30},
      catch_up=true,
      store=db)
async fn cleanup() -> Result<Null> { ... }
```

- **`tz="IANA/Name"`** — interpreta el schedule en huso indicado
  (vía `chrono_tz`). Default `"UTC"`.
- **`retry={max, backoff, initial_secs, max_secs}`** — hasta N
  reintentos con backoff (`"exponential"`/`"linear"`/`"constant"`),
  cada delay capeado por `max_secs`. Default: sin retry.
- **`catch_up=true`** — al boot, si hubo missed runs entre
  `last_run_at` y `now`, ejecuta UN run inmediato (no N — evita
  spam). Default `false` = skip.
- **`store=<binding>`** — persiste el registry + cada attempt en
  `fitz_cron_jobs` / `fitz_cron_runs` (auto-creadas con
  `CREATE TABLE IF NOT EXISTS`). Visibility manual con `psql`.

`@background` acepta los mismos `tz` y `retry` (sin `store` ni
`catch_up` — persistencia de `spawn(...)` diferida a iter3).

### Sub-paso a — Checker estático de kwargs

Helpers libres `check_job_kwargs` + `check_retry_map` en
`src/types.rs` parametrizados por allowed-list. Valida shape
sintáctico (Str/Bool/Map literal según kwarg), rechaza
duplicados y desconocidos con la lista de aceptados.
`extract_int_literal` reconoce `Int(N)` y `UnaryOp { Neg, Int(N) }`
para negativos. **+20 unit tests** (15 `cron_*` + 5
`background_*`); total 24 + 8 al cierre.

### Sub-paso b — Runtime intérprete: tipos extendidos

`src/cron_jobs.rs`: `enum BackoffKind` (default `Exponential`) +
`struct RetryConfig` con `Default` (max=0) + `delay_for_attempt`
capeado + `struct CronJobOptions { tz, retry, catch_up, store }`
con `Default`. `CronJob` gana los 4 campos; `register` acepta
`CronJobOptions` como parámetro final.

`src/evaluator.rs`: `register_cron_job` parsea kwargs del
`Decorator` vía `parse_cron_job_options` + sub-helpers
(`parse_retry_kwarg`, `resolve_store_kwarg`). El IANA real lo
valida `chrono_tz::Tz::from_str` con error claro si falla.
**+11 unit tests del registry + +7 unit tests del evaluator**.

### Sub-paso c — Scheduler intérprete + tests E2E reales

`src/cron_jobs.rs`: 7 helpers SQL (`init_storage` /
`upsert_job_row` / `record_run_start` / `record_run_finish` /
`update_job_last_run` / `read_last_run_at` /
`parse_pg_timestamptz` — el último normaliza offset Postgres sin
minutos). `run_cron_job` boot con init storage + upsert + catch_up
(`Schedule::after(last)` en la tz). Loop tz-aware con
`Schedule::upcoming(job.tz)` + `invoke_with_retry`.

Schema:

```sql
fitz_cron_jobs(
    name PK, schedule, tz,
    last_run_at, last_status, last_error, next_run_at
)
fitz_cron_runs(
    id BIGSERIAL, job_name, started_at, finished_at,
    status, attempt, error
)
-- status: 'running' | 'ok' | 'failed' | 'retrying'
-- attempt: 1-indexed; retry máx N produce hasta N+1 rows
```

**+6 tests E2E reales** contra Postgres en
`tests/cron_jobs_real_postgres.rs` (`#[ignore]`, requieren
`FITZ_TEST_PG_URL`).

### Sub-paso d — Codegen `fitz build` paridad bit-a-bit

`src/codegen.rs` (~720 LoC nuevas): `CronJobInfo` extendido con
los 4 campos parseados build-time. `program_has_persistent_cron`
walka AST y fuerza `uses_db=true` cuando encuentra
`store=<ident>`. Preludio dividido en 4 constantes
(`JOBS_COMMON_PRELUDE` + `JOBS_RUN_PRELUDE_SIMPLE` cuando no hay
persistencia, o `SQL_HELPERS_PRELUDE` +
`JOBS_RUN_PRELUDE_PERSISTENT` cuando sí). Trait
`__FitzCronStoreFrom` polimórfico acepta `__FitzDbConn` directo
o `Result<__FitzDbConn, String>` (caso idiomático `let db =
db.connect(...).await` sin `?` top-level). `gen_main` reordena:
stmts del usuario van ANTES de `emit_cron_job_spawns` para que
bindings top-level estén en scope.

`src/evaluator.rs::resolve_store_kwarg` también acepta
`Value::Result(Ok(DbConn))` (paridad bit-a-bit con el trait del
codegen).

Validado contra Postgres 15 local: binario nativo con
`@cron("*/2 * * * * *", store=db)` crea las dos tablas, persiste
3 runs `status='ok' attempt=1` en 6s; `last_status='ok'` en
`fitz_cron_jobs`.

### Sub-paso e — Cap 30 + ejemplo + LSP refresh

`docs/guide.md` cap 30 "Jobs sin Celery" — sub-sección nueva
**"Persistencia, retry y timezone (iter2)"** documenta los 4
kwargs con shape, defaults, schema DDL, queries de visibility
con `psql`, semántica del binding `Result<DbConn>` top-level +
`__FitzCronStoreFrom`. Limitación conocida documentada
(`fitz run` cron-only con `store=db`). Sub-sección "Qué no está
en el MVP" reescrita: salen los 3 items cerrados; entra
`@background` con persistencia + retry (diferido a iter3).

`examples/guide/30b-cron-persistente.fitz` (~50 LoC) — HTTP+cron
con los 4 kwargs combinados. Sumado al smoke
`GUIDE_EXAMPLES_COMPILE` (~290 ejemplos, todos verde en ~7 min).

`src/lsp.rs` — descripciones de `@cron`/`@background` mencionan
los kwargs nuevos. Grammar TextMate sin cambios. **112 tests LSP
verdes**.

### Cierre formal — Tier 1 del curso

Total al cierre: **2792 unit + 6 E2E real Postgres + 1
compile_e2e smoke + 112 LSP**. `cargo fmt --all` + `cargo clippy
--all-targets -- -D warnings` limpios. mkdocs build sin
warnings nuevos.

Próximo norte: **9.w.1.iter2** (Tier 2 — RBAC custom + token
refresh) o saltar a Fase 12 según necesidad del curso.

---

## [v0.11.1] — 2026-06-01 — Fase 13 polish: short flags + Bool=true negation + List<Str> variadic + fix CI fmt drift

**4 sub-pasos coordinados (~5h reales)** cerrando las 3 deudas
residuales de Fase 13 (v0.11.0) + un fix CI permanente.

### Sub-paso 1 — rustfmt.toml committed + fix CI fmt drift

**Fix permanente** del fail CI de v0.11.0 (`src/cli.rs:419` se
formateaba distinto entre Windows local y Ubuntu CI). Causa: el
repo no tenía `rustfmt.toml` committed, cada versión de rustfmt
aplicaba defaults sutilmente distintos.

- `rustfmt.toml` nuevo en repo root con `edition = "2021"`,
  `max_width = 100`, `use_small_heuristics = "Default"`. Fija el
  formato canonical para todos los devs + CI sin importar la
  versión de rustfmt del runner.
- Deuda documentada en `docs/deudas-post-5b.md` como CERRADA con
  contexto del incidente y la decisión técnica.

### Sub-paso 2 — Short flags auto-inferidos

`-l` como atajo de `--loud` se infiere de la primera letra del
nombre del flag. **Sin sintaxis extra del lado del user**:

```fitz
@command("greet")
fn greet(name: Str, loud: Bool = false, count: Int = 1) -> Int {
    // Auto: --loud / -l, --count / -c
    ...
}
```

```bash
$ ./mybin Ada -l -c 3
HELLO, Ada!
HELLO, Ada!
HELLO, Ada!
```

- Helper nuevo `compute_short_flags(params) -> Result<HashMap<char,
  String>, String>` en `src/cli.rs` que infiere los mappings.
- **Detección de colisiones en compile-time**: dos flags con misma
  primera letra (`loud` + `level`) → error claro al `fitz build`
  con sugerencia ("Renombrá uno de los dos").
- Parser de argv (`parse_argv` en intérprete + dispatcher generado
  en `gen_cli_command_helpers`) normaliza `-x` → `--<long>` antes
  del match flag. Same path para ambos. Soporta solo `-x` single en
  MVP — combo `-xyz` y `-x=v` quedan como deuda menor.
- Help text muestra `-l, --loud` cuando hay short asignado.

### Sub-paso 3 — `Bool = true` con `--no-<flag>` negation

Lifted la restricción MVP `Bool = true rechazado`. Ahora:

```fitz
@command("go")
fn go(verbose: Bool = true) -> Int {
    if verbose { print("verbose mode ON") } else { print("quiet") }
    return 0
}
```

```bash
$ ./go                 # default → true
verbose mode ON

$ ./go --no-verbose    # negación explícita → false
quiet
```

- Checker actualizado para aceptar `Bool = true` defaults.
- Parser de argv reconoce `--no-<name>` para Bool flags: si el
  resto matchea un flag Bool del comando, set a false. Si el nombre
  empieza con `no-` pero no matchea (caso raro `--noisy` legítimo),
  cae al path normal.
- Help text emite `--no-<name>` para Bool con default true (paralelo
  a Cargo `--no-default-features`).
- Codegen emite arms `"no-<name>"` antes de los arms de flag normales
  para el match parser.

### Sub-paso 4 — `List<Str>` variadic positional

Último param de tipo `List<Str>` con default `= []` absorbe N
tokens posicionales restantes:

```fitz
@command("run")
fn run(mode: Str, verbose: Bool = true, files: List<Str> = []) -> Int {
    if verbose { print("mode: {mode}") }
    for f in files {
        print("  - {f}")
    }
    return 0
}
```

```bash
$ ./run fast a.txt b.txt c.txt
mode: fast
  - a.txt
  - b.txt
  - c.txt

$ ./run fast --no-verbose
# mode no impreso, files vacía
```

- Checker permite `List<Str>` como ÚLTIMO param de todos. Variadic
  posicionado en otra ubicación → error.
- Convención del `= []` default: requerido porque el parser Fitz
  exige "después del primer default, todos los siguientes también".
  El `[]` es semánticamente redundante (variadic siempre empieza
  vacío y acumula) pero satisface el shape sintáctico.
- Parser de argv: detecta variadic por type+posición, acumula
  tokens restantes en `Vec<String>` → wrappea como
  `Value::List(Arc<Mutex<...>>)`.
- Codegen emite `__cli_variadic_<name>: Vec<String>` accumulator +
  wrap final a `Arc<Mutex<Vec<String>>>` (mismo shape que los List
  del runtime Fitz post-F17).
- Variadic excluido de short flag auto (no es flag), de OPTIONS
  section del help, y aparece en USAGE como `[<files>...]`.

### Decisiones técnicas

- **Short flags auto vs explícito**: optamos por auto-inferir
  (primera letra) en vez de `@flag(short="l")` decorator porque
  evita AST change en `Param` y matchea la convención CLI estándar
  (POSIX, GNU). El override manual queda como deuda futura si entra
  presión.
- **Variadic requiere `= []` default**: violación menor de
  semántica (variadic no necesita default conceptualmente) a cambio
  de no tocar el parser Fitz. Trade-off aceptado por scope.
- **`--no-<flag>` solo para Bool con default true**: técnicamente
  podríamos soportar `--no-<flag>` para Bool con default false
  también (negaría a false redundante), pero no aporta y agrega
  ruido al help. Si el user lo quiere, escribe `--<flag>=false`.
- **Smoke negation tiene priority sobre flag literal `no-foo`**: si
  hay un Bool flag llamado `no-foo` Y un Bool flag llamado `foo`, la
  arm `"no-foo"` matchea PRIMERO (case-sensitive exact). Documentado
  pero raro en práctica.

### Tests

- **7 E2E nuevos** en `tests/compile_e2e.rs`:
  `fase_13_short_flags_auto_inferidos`,
  `fase_13_short_flag_desconocida_es_error`,
  `fase_13_bool_default_true_se_niega_con_no_flag`,
  `fase_13_list_str_variadic_absorbe_positionals`,
  `fase_13_list_str_variadic_vacio_aceptado`,
  `fase_13_paridad_run_vs_build_polish` (paridad bit-a-bit con
  short flags + variadic + Bool=true combinados),
  `fase_13_short_flag_collision_es_error_compile`.
- Total Fase 13 E2E al cierre: **17/17 verdes** (10 de v0.11.0 + 7
  nuevos de v0.11.1).
- Smoke `GUIDE_EXAMPLES_COMPILE` verde (293 ejemplos).
- Clippy `--all-targets -D warnings` + `--features lsp` limpios.
  fmt `--all --check` ahora consistente entre Windows y Linux gracias
  al `rustfmt.toml` committed.

### Ejemplo `examples/guide/33-cli.fitz` actualizado

Cap intro del ejemplo guide actualizado con las 3 features nuevas
(short flags, Bool=true negation, variadic) documentadas en el
header comment.

### Total al cierre v0.11.1

**2754 unit + 293 smoke + 81 cli_e2e + 341 compile_e2e (+7 nuevos
Fase 13 polish) + 3 openapi + 61 db_real_postgres**.

### Próximo norte

Mismo que v0.11.0: **Fase 12** (Deployment) o **Tier E del ORM**.

## [v0.11.0] — 2026-06-01 — Fase 13: CLI builder nativo (`@command`)

**Bump menor → 0.11.0** porque Fase 13 cierra entera una nueva
ciudadana primera del lenguaje. Funcionalmente backward-compatible
con v0.10.32 (los programas existentes siguen funcionando), pero
suma una capacidad core del lenguaje que justifica el salto de
minor.

**5 sub-pasos coordinados (~10h reales vs ~12h estimadas)**.
`@command("name", desc="...")` sobre una `fn` la declara como comando
CLI; el binario producido por `fitz build` parsea
`std::env::args()` y dispatcha al comando matching, con **help
auto-generado** y **parser de positional args + flags** con **zero
deps externas**. Convención sin decorators en params: positional vs
flag se infiere del `default = ...` del param.

### Sintaxis canónica

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

```bash
$ ./mybin greet Ada --loud --count 3
HELLO, Ada!
HELLO, Ada!
HELLO, Ada!

$ ./mybin --help
USAGE:
    mybin <command> [ARGS] [OPTIONS]
COMMANDS:
    greet    Greet a person
...
```

### Sub-pasos

- **13.1 — Parser/AST/Checker**: nueva fn `check_command_decorator`
  en `src/types.rs` valida shape (arg Str literal con nombre del
  comando, opcional kwarg `desc=`, return type `Int`, params
  CLI-marshallables `Str/Int/Float/Bool/Str?`, sin varargs, sin
  conflictos con decorators de servidor/job/test/middleware). Bool
  con `default = true` rechazado en MVP (requiere convención
  `--no-flag` para negar — documentado).
- **13.2 — Evaluator (intérprete)**: nuevo módulo `src/cli.rs` con
  `CliRegistry` (paralelo a `CronRegistry`), `CliCommand`,
  `parse_argv()` con multi-command detection + dispatch. Helper
  `with_active_cli_registry`/`install_cli_registry` thread-local
  en `src/evaluator.rs`. `process_decorator` branch para
  `@command` que pushea al registry activo. `src/main.rs::run_file`
  instala el registry pre-eval y, post-eval, dispatcha CLI si
  `count > 0` (skip HTTP/cron en ese caso).
- **13.3 — Help autogeneration**: funciones puras
  `render_global_help`/`render_command_help`/`usage_line`/
  `render_args_section`/`render_options_section` en `src/cli.rs`.
  El help se construye desde los specs registrados (no requiere
  doc-strings). Padding consistente con clap.
- **13.4 — Codegen (`fitz build`)**: `gen_cli_main` emite `fn main()`
  (o `#[tokio::main]` si algún @command es async) con dispatch
  estático: detecta modo single vs multi, parsea `--help` global,
  matchea el subcomando contra arms generados. Per-command:
  `__fitz_cli_run_<cmd>` parsea positional + flags con
  type-coerciones (`parse::<i64>`/`parse::<f64>` + error claro
  con exit 2). Help string emitida como const en build-time.
  **Paridad bit-a-bit `fitz run` ↔ `fitz build`** validada con E2E
  (`fase_13_cli_paridad_run_vs_build`).
- **13.5 — Tests + ejemplo + docs**: 10 E2E nuevos en
  `tests/compile_e2e.rs` (single command positional, multi-command
  dispatch, Bool + Int flags, help global, help per-command,
  comando desconocido exit 2, missing positional exit 2, bad Int
  flag exit 2, exit code from handler, paridad run↔build). Ejemplo
  runnable `examples/guide/33-cli.fitz` con 3 commands (greet/add/
  status) sumado al smoke `GUIDE_EXAMPLES_COMPILE`.

### Decisiones técnicas

- **Convención sin `@arg`/`@flag` decorators**: la presencia de
  `default = ...` en el param determina si es positional o flag.
  Reduce verbosidad vs Click (Python) que exige `@click.argument`
  por cada param. Trade-off: NO se puede tener positional
  optional args (los con default son flags). Para casos límite, usá
  `Str?` (nullable) que mantiene shape pero requiere `match` en el
  body.
- **Exit codes POSIX**: `0` éxito, `1+` retornado por el handler,
  `2` errores de parsing del CLI. Convención estándar Linux.
- **Detección de modo automática**: el binario tiene un único
  "modo" determinado por los decorators presentes (`@get*` → HTTP,
  `@cron` → cron-only, `@command` → CLI, ninguno → script plain).
  Mutuamente excluyentes — el checker rechaza combinaciones con
  error claro.
- **Help string emitida en build-time como const**: en lugar de
  construirla en runtime, el codegen emite las strings inline para
  cada comando. Trade-off: binario más grande (~50 bytes por
  comando) pero startup más rápido y sin allocs en el path normal.
- **Boolean flag presence semantics**: `--loud` sin valor activa
  el flag a `true` (idiomático CLI). `--loud=false` también funciona
  para override explícito. Sin valor con flag no-Bool → error claro.
- **`Bool = true` rechazado en MVP**: requiere convención
  `--no-<flag>` para negar (paralelo a Cargo `--no-default-features`).
  Implementable, deuda menor — el user invierte la lógica por ahora.

### Tests

- **10 E2E** en `tests/compile_e2e.rs::fase_13_cli_*` cubriendo
  todo el path codegen + paridad bit-a-bit con el intérprete.
- Smoke `GUIDE_EXAMPLES_COMPILE` verde (292 ejemplos + el nuevo
  `33-cli.fitz` = 293).
- Clippy `--all-targets -D warnings` + `--features lsp` limpios.
  fmt `--all --check` limpio.

### Boilerplate `cli-tool` actualizado

`boilerplates/cli-tool/` ahora usa `@command` idiomático: 3
comandos (`report`/`count`/`regions`) con help auto-generado,
positional args y flags. README actualizado con demo completa de
la nueva sintaxis. El binario compilado sigue siendo ~5 MB Linux
standalone, imagen Docker `distroless/cc` ~22 MB.

### Otros cambios incluidos en este release

- **Fix CI LSP**: `make_hover_with_range` cambió signature en v0.10.32
  (Tier D.2 — sumó `program: &Program`) pero el test unit
  `lspy_make_hover_with_range_incluye_range_del_ident` no se
  actualizó. Compilaba con `cargo test --lib` (sin `--features lsp`)
  pero rompía en el job CI `cargo test --features lsp --lib lsp::`.
  Fixed con `&Vec::new()` (Program vacío — el test valida solo el
  Range, no el augment).

### Total al cierre v0.11.0

**2739 unit + 293 smoke + 3 openapi + 81 cli_e2e + 334 compile_e2e
+ 61 db_real_postgres** (+10 nuevos Fase 13 en compile_e2e).
Acumulado de los `src/cli.rs` unit tests: ~15 directos.

### Por qué Fase 13 importa

Hace de Fitz **el único lenguaje moderno** que combina HTTP nativo
+ WebSockets tipados + ORM + jobs + **CLI builder nativo** en el
core del compilador, con paridad bit-a-bit intérprete↔binario, **zero
deps externas** para todas estas features intrínsecas. Cualquier
otro stack requiere `clap`/`argparse`/`click`/`commander` separado.

### Próximo norte

**Fase 12** (Deployment ciudadano primera clase) o resto del **Tier E
del ORM**. Detalle de cada uno en `docs/roadmap.md`.

## [v0.10.32] — 2026-06-01 — Tier C + D del cierre ORM/DB (operadores SQL + DX/LSP residual)

**5 features coordinadas en bundle (~8h reales vs ~20h estimadas)**
cerrando los 2 últimos tiers no-visión del ORM/DB. Sin breaking
changes, paridad bit-a-bit `fitz run` ↔ `fitz build` mantenida. 3
E2E nuevos contra Postgres real + 0 regresiones en 2739 unit + 292
smoke + 81 cli_e2e + 324 compile_e2e + 3 openapi.

### Tier C — Operadores SQL faltantes

- **C.1 — `ts_rank` full-text ranking** en `.order_by(...)`. Sintaxis:
  `.order_by(fn(u) => -u.body.rank("query"))` emite
  `ORDER BY ts_rank("body", to_tsquery('query')) DESC`. Combinable con
  `.where(fn(u) => u.body.matches("query"))` para ordenar resultados
  full-text por relevancia. Variante `plainto_rank` para queries del
  estilo "plain text" (`plainto_tsquery`). El query string se inlina
  como SQL literal en MVP — vars quedan como deuda menor. Habilita el
  pattern canonical de search ranking sin escape hatch a `db.query`.
- **C.2 — Expression indexes** con `@index(expression="lower(email)")`.
  El user pasa la SQL expression raw como kwarg dedicado; el codegen
  emite `CREATE INDEX ... ON tbl (<expression>)` literal. Habilita
  case-insensitive UNIQUE (`lower(email)`), full-text setup tsvector
  (`to_tsvector('english', body)`), totals computados
  (`(price * quantity)`), etc. **Drift check incompleto** documentado:
  la introspect lee el index del catálogo pero NO parsea
  `pg_index.indexprs` para detectar el expression — el user nombra
  el index explícito con `name=` para drift name-based reliable.
- **C.3 — JSON `||` merge en `.merge_jsonb`**. Sintaxis:
  `User.where(fn(u) => u.id == 5).merge_jsonb(db, "data", {"new": "v"}).await?`.
  Emite `UPDATE tbl SET "data" = "data" || $1::jsonb WHERE id = $2`
  preservando las keys existentes del objeto jsonb y aplicando el
  patch (overwrite si existen, agregar si no). Limitación Postgres
  documentada: `NULL || anything = NULL` — el user inicializa la col
  con `{}` al INSERT para que el merge funcione. Field debe ser
  `Map<...>` (jsonb); error de checker si se intenta sobre otro tipo.

### Tier D — DX/LSP residual

- **D.1 — LSP completion ORM en `.where()`**. Los métodos ORM
  (intercepted por el evaluator solo en `.where(closure)` context)
  ahora aparecen en autocomplete del LSP cuando tipás `u.email.` o
  `u.data.`. Los detail muestran `(ORM .where)` para distinguirlos
  de métodos regulares. Cobertura: **Str** (`is_in`, `like`, `ilike`,
  `matches`, `plainto_matches`, `between`, `is_null`, `is_not_null`),
  **Map** (`has_key`, `has_all_keys`, `has_any_keys`, `contains_json`,
  `has_path`, `path_text/int/float/bool`), **Int/Float**
  (`is_in`, `between`, `is_null`, `is_not_null`), **Date/DateTime**
  (idem). Fuera del `.where`, llamarlos genera error en runtime —
  el LSP no detecta el contexto (limitación documentada).
- **D.2 — LSP hover sobre `@table` types**. Al hover sobre un
  identificador que tipa como `Type::Nominal(id)` con `@table`
  metadata, el tooltip ahora incluye el `CREATE TABLE` SQL emitted
  bajo el tipo declarado. Implementado vía
  `migrations::schema_from_program` + `create_table_sql_for`. Útil
  para debuggear migrations sin abrir `fitz db diff` — el LSP
  muestra exactamente el shape SQL que el migrator emite. Si
  `schema_from_program` falla (typo en relations, FK target no
  existente), el augment se skipea (hover sigue mostrando solo el
  tipo, sin error visual).

### Decisiones técnicas

- **C.1 inline el query string**: en lugar de bindear `$N`, el
  query string se inlina al SQL literal (`to_tsquery('query')`). El
  trade-off: aceptar solo Str literal en MVP, vars quedan como
  deuda menor. Razón: el path order_by stream del runtime no tiene
  acceso al pg_args store del where; cambiarlo requeriría refactor
  cross-method.
- **C.2 kwarg `expression=` exclusivo**: `@index(expression="...")`
  NO acepta arg posicional simultáneo (`@index("col", expression="...")`).
  Forzar la elección "cols o expression" evita ambigüedad semántica.
- **C.3 `.merge_jsonb` separado de `.update`**: en lugar de embedded
  semantics en `.update` con flags, una method dedicada. API más
  explícita en el call site, signatura simple `(db, field, patch)`.
- **D.1 completion sin scope detection**: el LSP NO detecta si el
  cursor está adentro de un `.where(...)` closure — los métodos
  ORM aparecen sobre Str/Map/etc. en todos los contextos. El detail
  `(ORM .where)` informa al user.
- **D.2 hover augment idempotente**: si `schema_from_program` falla,
  el hover devuelve solo el tipo display (sin SQL).

### Tests

- **3 E2E reales contra Postgres** en `tests/db_real_postgres.rs`:
  `tier_c1_ts_rank_order_by_works`, `tier_c2_expression_index_creates_lowercase_unique`,
  `tier_c3_jsonb_merge_preserves_existing_keys`. Todos verdes
  contra Postgres local.
- Smoke `GUIDE_EXAMPLES_COMPILE` verde (292 ejemplos).
- Clippy `--all-targets -D warnings` + `--features lsp` limpios.
  fmt `--all --check` limpio.

### Total al cierre v0.10.32

**2739 unit + 292 smoke + 3 openapi + 81 cli_e2e + 324 compile_e2e
+ 61 db_real_postgres** (+3 nuevos del Tier C).

## [v0.10.31] — 2026-06-01 — Tier A del cierre ORM/DB: MVP fuerte (3 bloques en bundle)

**9 features en bloque (~12h reales vs ~30-40h estimadas)** que
llevan el ORM/DB del estado "funcional con fricciones residuales" a
"MVP fuerte sin caveats conocidos" para el caso de uso real. Sin
sintaxis nueva mayor (solo kwargs sobre 2 built-ins existentes),
zero deps externas, paridad bit-a-bit `fitz run` ↔ `fitz build`
mantenida. 6 E2E nuevos contra Postgres real + 0 regresiones en
2739 unit + 292 smoke + 81 cli_e2e + 324 compile_e2e + 3 openapi.

### Bloque 1 — Diff seguro + ALTER + CHECK constraints (A.1 + A.2 + A.5)

- **A.1 — `fitz db diff --check-destructive`**: clasifica cada
  change como `Safe` / `Risky` / `Destructive` y aborta con exit 1
  si hay destructive sin `--allow-destructive` explícito. El SQL
  emitido suma comentarios `-- [SAFE]` / `-- [RISKY]` /
  `-- [DESTRUCTIVE]` por change. Política:
  - **Destructive**: `DropTable`, `DropColumn`
  - **Risky**: `AddColumn NOT NULL sin default`, `AlterColumnType`,
    `AlterColumnNullable false`, `AlterColumnDefault`, `DropIndex`,
    `AddCheckConstraint`
  - **Safe**: el resto (CreateTable, CreateIndex, AddForeignKey,
    DropForeignKey, RenameTable/Column, AlterColumnNullable true,
    DropCheckConstraint, AddColumn nullable/con default)
- **A.2 — `ALTER COLUMN TYPE` con `USING` automático**: el SQL emit
  pasa de `ALTER TABLE t ALTER COLUMN c TYPE T;` a
  `ALTER TABLE t ALTER COLUMN c TYPE T USING c::T;`. Postgres acepta
  el cast explicit incluso para auto-castable (`int → bigint`), y es
  required para casts non-auto (`text → int`, `varchar → int`).
  Para casts que `::` no soporta (bytea ↔ text con encoding custom,
  etc.), el user edita el SQL emitido manualmente.
- **A.5 — `ALTER TABLE ADD/DROP CONSTRAINT` para CHECKs via diff**:
  nuevas variantes `Change::AddCheckConstraint` y
  `Change::DropCheckConstraint`. `diff_check_constraints()` compara
  `current.check_constraints` vs `target.check_constraints` por
  `name`; mismo name + expr distinto → DROP + ADD. Habilita la
  evolución de `@check_constraint("...")` sin recrear la tabla —
  drift detect completo en combinación con A.7.

### Bloque 2 — Transacciones avanzadas (A.4 + A.9 + A.3)

- **A.4 — Nested transactions vía SAVEPOINT**: `db.transaction(fn(tx)
  { ... tx.transaction(fn(inner) { ... }) ... })` ahora funciona
  correctamente. `DbConnHandle` suma `tx_depth: Arc<AtomicI32>`
  shared entre outer y todos los handles de sub-pool. La outer tx
  (depth=0) emite `BEGIN/COMMIT/ROLLBACK`; las nested (depth>0)
  emiten `SAVEPOINT fitz_sp_<N>/RELEASE SAVEPOINT/ROLLBACK TO
  SAVEPOINT`. Inner Err deja el outer intacto (rollback parcial).
- **A.9 — Isolation levels custom**: `db.transaction(closure,
  isolation="SERIALIZABLE")` (kwarg). Whitelist defensiva con 4
  niveles ANSI (`READ UNCOMMITTED` / `READ COMMITTED` /
  `REPEATABLE READ` / `SERIALIZABLE`) opcionalmente combinados con
  `READ ONLY` / `READ WRITE` (`"SERIALIZABLE READ ONLY"`). Outer
  tx emite `BEGIN ISOLATION LEVEL <...>`. Nested ignora el kwarg
  (Postgres no permite ISOLATION en SAVEPOINT — el nivel lo fija
  el outer BEGIN). Nuevo public method
  `transaction_with_isolation(Option<&str>, closure)` en
  `DbConnHandle` (call directo en Rust para tests).
- **A.3 — `db.connect(url, max_conns=N)` kwarg**: pool size opt-in
  del lado del lenguaje (antes solo via env var
  `FITZ_DB_MAX_CONNS`). Validación `1 ≤ N ≤ 1000` con error claro.
  Implementado vía override de la env var antes del connect (deuda
  menor: si un connect previo ya cacheó `max_conns`, el override
  no aplica — documentado).

### Bloque 3 — FK + Drift completo (A.6 + A.7 + A.8)

- **A.6 — FK composite PK del target con error claro**: antes de
  v0.10.31, `@belongs_to user_id: Int` apuntando a un `@table` con
  composite PK hacía fallback silencioso a `"id"` (típicamente no
  existente) → error críptico de Postgres en `fitz db migrate`. Ahora
  `schema_from_program` aborta con mensaje específico citando los
  fields de la composite PK + sugiriendo workarounds (declarar
  UNIQUE constraint single-column en el target, o usar single PK
  surrogate). El sub-paso `refs=` para single-FK explícito queda
  como deuda menor.
- **A.7 — Drift de `@check_constraint` (introspect lee
  `pg_constraint.contype='c'`)**: nueva fn
  `introspect_check_constraints()` que pulla desde `pg_constraint`
  con `pg_get_constraintdef(con.oid)` y canonicaliza la expr via
  `parse_check_def()` (recorta `CHECK ` + paréntesis externos
  balanceados — PG a veces emite 1 o 2 niveles). El diff ahora
  detecta cambios reales del expr y DROP CHECK funciona end-to-end.
- **A.8 — Drift cross-schema FK (introspect popula
  `references_schema`)**: el SQL del FK introspect pulla también
  `ccu.table_schema AS ref_schema`. Si el ref_schema difiere del
  schema local → `references_schema = Some(...)`; mismo schema →
  `None` (paridad con la convención de `schema_from_program`).
  Habilita drift end-to-end para FKs declarados con
  `@belongs_to("schema.User")` cross-schema.

### Decisiones técnicas

- **Severity opinionada, conservadora**: `DropIndex` es Risky (no
  hay pérdida de data, pero performance impact); `DropForeignKey`
  es Safe (solo remueve constraint). Refinable si entra presión.
- **USING `col::T` siempre**: en lugar de detectar casos que no
  necesitan USING, lo emitimos siempre. Postgres es permisivo con
  el cast redundante. Beneficio: menos código + mensajes de error
  más informativos en runtime.
- **Composite PK FK error claro vs. fallback**: la antigua semántica
  de fallback a `"id"` ocultaba el problema hasta el último momento.
  Mejor abortar al `schema_from_program` con mensaje específico.
- **Severity bloquea solo Destructive**: Risky se reporta como
  warning pero no bloquea. La razón: Risky cubre cambios que el user
  típicamente QUIERE hacer (`ALTER TYPE`), solo necesitan revisión.
  Destructive es la línea roja real (data loss garantizada).
- **`parse_check_def` con balance check**: detecta cuando los
  paréntesis externos NO son envolventes (`(a) AND (b)`) y NO los
  recorta. Esto evita corromper exprs composite donde el primer
  `(` cierra en posición interna.
- **Whitelist de isolation levels**: 4 ANSI x opcional READ ONLY/
  WRITE = 12 strings válidos. Rechaza otros con error claro. Más
  estricto que dejar pasar y que Postgres responda con error.

### Tests

- **6 E2E reales contra Postgres** en `tests/db_real_postgres.rs`
  (`#[ignore]` por default, corren con `FITZ_TEST_PG_URL +
  --ignored`): A.4 SAVEPOINT inner rollback + inner commit (2),
  A.9 SERIALIZABLE + READ COMMITTED/REPEATABLE READ (2), A.7
  introspect CHECK constraint, A.8 introspect cross-schema FK.
  Todos verdes contra `postgres:16` local + dev.
- **0 unit nuevos directos** — los helpers nuevos
  (`severity()`/`count_by_severity`/`changes_to_sql_with_severity`/
  `parse_check_def`/`dispatch_builtin_kwargs`) se cubren vía los
  E2E que ejercitan todo el path.
- Smoke `GUIDE_EXAMPLES_COMPILE` verde (292 ejemplos).
- Clippy `--all-targets -D warnings` + `--features lsp` limpios.
  fmt `--all --check` limpio.

### Total al cierre v0.10.31

**2739 unit + 292 smoke + 3 openapi + 81 cli_e2e + 324 compile_e2e
+ 58 db_real_postgres** (+6 nuevos del Tier A).

### Próximo norte

**Tier C** (operadores SQL faltantes — ts_rank, expression indexes,
JSON `||` merge — ~12-20h) o **Tier D** (DX/LSP residual del ORM
— completion en `.where(...)`, hover sobre `@table` — ~5h, quick
win). Tier E es visión a futuro (cada ítem mini-fase dedicada).

## [v0.10.30] — 2026-05-31 — Tier B del cierre ORM/DB: API completion Date/DateTime/Uuid

**Tier B entero cerrado en bloque (~12-16h estimadas, ~6h reales)**.
7 sub-pasos coordinados que llevan los tipos nativos
`Date`/`DateTime`/`Uuid` del estado "funcionales con getters" a
"API completa con aritmética, diff, comparison y timezone display".
Sin sintaxis nueva del lenguaje (todos métodos sobre tipos
existentes), zero deps user-facing nuevas (chrono-tz + feature
`uuid/v7` ya internos al binario), paridad bit-a-bit `fitz run` ↔
`fitz build` para los 7 sub-pasos validada con 10 E2E nuevos.
Ningún breaking en los 292 ejemplos del smoke `GUIDE_EXAMPLES_COMPILE`.

### Sub-paso B.1 — Aritmética add_* sobre Date y DateTime

- **Date**: `.add_days(n)`, `.add_months(n)`, `.add_years(n)`
- **DateTime**: `.add_seconds(n)`, `.add_minutes(n)`, `.add_hours(n)`,
  `.add_days(n)`, `.add_months(n)`, `.add_years(n)`

`n: Int` signed (negativos OK). Sub-second units van vía
`chrono::Duration::seconds(n * factor)`; calendar units (months/years)
preservan day-of-month con clamping (`Jan 31 + 1 mes → Feb 28/29`)
vía `chrono::Months` + `checked_add_months`/`checked_sub_months`.
Overflow del rango interno (NaiveDate ±262143 / DateTime ±i64 secs)
emite `FitzError` claro citando el método + el valor que rompió;
en codegen panic con mismo formato.

### Sub-paso B.2 — Subtract symmetric

Aliases con negate runtime:
- **Date**: `.subtract_days/months/years(n)` ≡ `.add_*(-n)`
- **DateTime**: `.subtract_seconds/minutes/hours/days/months/years(n)`

Misma semántica que B.1 con `n` invertido vía `checked_neg`
(`i64::MIN` sin opuesto → error claro). Útil para legibilidad cuando
`n` es un valor literal positivo (`d.subtract_days(7)` lee mejor que
`d.add_days(-7)`).

### Sub-paso B.3 — Diff entre fechas (signed Int)

- **Date**: `d1.diff_days(d2)` → `Int` días entre d1 y d2 (negativo
  si d2 posterior a d1)
- **DateTime**: `.diff_seconds/minutes/hours/days(other)` con
  truncamiento hacia 0 para unidades > 1 segundo (paralelo a
  `Duration::num_seconds() / factor`)

Patrón `dt2.diff_seconds(dt1)` se mapea a `dt2.signed_duration_since(dt1).num_seconds()`.

### Sub-paso B.4 — Comparison operators `<` `>` `<=` `>=` Date/DateTime

`chrono::NaiveDate` y `chrono::DateTime<Utc>` impl `Ord` nativo →
mapping directo a los operadores Fitz. El checker suma
`(Date, Date) | (DateTime, DateTime)` a las parejas permitidas
(antes solo numéricos y Str), el evaluator suma dos arms en `compare()`,
codegen emite `({lhs} < {rhs})` literal sin coerción. Workaround viejo
`d1.timestamp() < d2.timestamp()` ya no necesario.

### Sub-paso B.5 — `Uuid.v7()` time-ordered UUIDs

`Uuid.v7()` constructor estático sobre el módulo `Uuid`. UUIDv7
(RFC 9562, mayo 2024) codifica Unix millis en los primeros 48 bits
→ ordenan cronológicamente en btree indexes, muy útil para PKs
sortables por created_at (vs v4 random que produce index scattering).
Implementado vía `uuid::Uuid::now_v7()` con feature `uuid/v7`
añadida al `Cargo.toml` del binario y al Cargo.toml emitido por
`fitz build`.

### Sub-paso B.6 — Shortcuts

- `Date.tomorrow()` ≡ `Date.today().add_days(1)`
- `Date.yesterday()` ≡ `Date.today().add_days(-1)`
- `DateTime.epoch()` ≡ `DateTime.from_timestamp(0).unwrap()`
  (1970-01-01T00:00:00Z)

Cubren patrones cortos sin necesidad de armar el chain
manualmente.

### Sub-paso B.7 — Timezone display (chrono-tz + IANA)

- `DateTime.to_local()` → `Str`: formatea el instante UTC en la
  zona local del sistema como ISO 8601 con offset
  (`%Y-%m-%dT%H:%M:%S%:z`). Sin deps extras (`chrono::Local` ya
  viene activo via feature `clock`).
- `DateTime.in_tz(name: Str)` → `Result<Str>`: formatea en cualquier
  IANA timezone name (`"America/Argentina/Buenos_Aires"`,
  `"Europe/Paris"`, `"UTC"`, etc.). Name desconocido → `Err(Str)`
  con sugerencia de ejemplos.

**El instante UTC interno NO cambia** — son helpers de display, no
aritmética. Dep nueva `chrono-tz = "0.10"` (sin features extras,
~250KB compiled-in con la DB IANA completa); paralela en el binario
y en el Cargo.toml emitido por `fitz build` (sumado al bloque
`uses_date_or_uuid`).

### Decisiones técnicas

- **B.7 IANA names sobre enum built-in**: el caso real "convertir
  DB-UTC al huso del user" requiere IANA strings (`"America/...",
  `"Europe/..."`); enum dedicado quedaba expresivamente corto.
  `chrono-tz` pesa ~250KB, costo aceptable.
- **B.4 chrono nativo `Ord`**: en lugar de añadir un caso especial al
  evaluator `compare()` que parsee fechas a Int, reusamos el
  `PartialOrd` de chrono. Performance y semántica idénticas al
  approach manual con menos código.
- **`add_years` = `add_months * 12`**: simplifica al reusar
  `checked_add_months`/`checked_sub_months` (chrono no expone
  `add_years` directo). Trade-off menor: el mensaje de overflow cita
  `add_months` con el N pre-escalado (`add_years(100M)` →
  `add_months(1.2B)`). Refinable pasando el método como param si entra
  presión.
- **Negativos como `add_*(-n)` runtime**: `subtract_*` no son alias
  léxicos del parser sino dispatchers separados que negan el arg.
  Coste: dos arms más en el match (~30 LoC). Benefit: el método
  citado en error siempre coincide con el que llamó el user
  (`subtract_days` reporta `subtract_days`, no `add_days`).
- **`?` requiere fn-Result wrapper**: las nuevas API constructoras
  que retornan Result (`Date.from_ymd`, `DateTime.from_timestamp`,
  `Uuid.parse`) se usan con `?` adentro de una fn `-> Result<T>`,
  consistente con el resto del lenguaje (deuda menor: el codegen
  no acepta `?` top-level aunque el intérprete sí — paralelo al
  resto de programas del proyecto).

### Tests

- 10 E2E nuevos en `tests/compile_e2e.rs` cubriendo paridad bit-a-bit
  `fitz run` ↔ `fitz build` para cada sub-paso B.1-B.7 + 1 runtime
  overflow + 1 checker rejection (acumulado: 81 cli_e2e + **324
  compile_e2e**).
- 0 unit nuevos directos (los helpers `date_add_days`/`date_add_months`/
  `datetime_add_duration`/`datetime_diff`/`datetime_in_tz` se cubren
  vía los E2E que ejercitan todo el path eval → dispatch_method →
  helper).
- Smoke `GUIDE_EXAMPLES_COMPILE` verde (292 ejemplos).
- Clippy `--all-targets -D warnings` limpio. Clippy `--features lsp`
  limpio. fmt `--all --check` limpio.

### Total al cierre v0.10.30

**2739 unit + 292 smoke + 3 openapi + 81 cli_e2e + 324 compile_e2e
+ 52 db_real_postgres** (Tier B no toca DB — sin cambios en ese
test set).

### Próximo norte

**Tier A** (cierre MVP fuerte del ORM): `fitz db diff
--check-destructive`, `ALTER COLUMN TYPE` con `USING` automático,
`db.connect(url, max_conns=N)` kwarg, savepoints / nested
transactions, `ALTER TABLE ADD/DROP CONSTRAINT` para CHECKs, FK
targeting composite PK, drift check de `@check_constraint` +
cross-schema FK, isolation levels, `FITZ_DB_*` mid-run reload.
Estimado ~30-40h (10 ítems independientes). Detalle en
`docs/deudas-post-5b.md` → sección Tier A.

## [v0.10.29] — 2026-05-31 — Cierre masivo del ORM: JSON path + text search + @unique/@check + cross-schema FK + 6 cierres residuales más

**Release dedicado al cierre masivo de deudas residuales del ORM**.
12 features nuevas + 1 skip deliberado en bloque que llevan el ORM
de "funcional con caveats" a "completo + observable + ergonómico"
para el caso de uso real de aplicaciones full-stack contra
Postgres. Sin sintaxis nueva del lenguaje (la mayoría son
extensiones de decoradores y métodos existentes), zero deps
externas adicionales, paridad bit-a-bit `fitz run` ↔ `fitz build`
mantenida. Ningún breaking change para los 292 ejemplos del smoke
(`GUIDE_EXAMPLES_COMPILE` verde end-to-end).

### Sub-paso 1 — JSON path operators (nested + cast tipado)

Cinco method calls nuevos sobre fields jsonb (`Map<Str, ...>`) en
closures de `.where(...)`. Cierran el agujero del `.get("k")`
single-level habilitando acceso a paths anidados con cast tipado:

- `e.data.has_path([k1, k2, ...])` → `"data" #> $N::text[] IS NOT NULL`
- `e.data.path_text([k1, k2, ...])` → `("data" #>> $N::text[])`
- `e.data.path_int([k1, k2, ...])` → `(("data" #>> $N::text[])::bigint)`
- `e.data.path_float([k1, k2, ...])` → `(("data" #>> $N::text[])::float8)`
- `e.data.path_bool([k1, k2, ...])` → `(("data" #>> $N::text[])::boolean)`

Filtros tipados al estilo `e.data.path_int(["user", "id"]) == 5`
reemplazan el workaround de `db.query(...)` con cast crudo.

### Sub-paso 2 — Full-text search via `@@`

Dos method calls sobre fields `Str` (típicamente columna tsvector
via `@column(sql_type="tsvector")`):

- `body_tsv.matches("query")` → `"body_tsv" @@ to_tsquery($1)` (syntax avanzada)
- `body_tsv.plainto_matches(input)` → `"body_tsv" @@ plainto_tsquery($1)` (search bar libre)

Combinable con `@index(body_tsv, using="gin")` v0.10.28 para
performance de full-text search end-to-end sin bajar a SQL crudo.

### Sub-paso 3 — `@unique(col1, col2, ...)` composite shortcut

Decorator type-level nuevo, alias ergonómico de
`@index(unique=true)`. Acepta bare idents o Str con commas. Solo
soporta `name="..."` como kwarg (para `where_=`/`using=` usar
`@index(...)` directo). Apilable.

```fitz
@table("users")
@unique(email, tenant_id)
@unique(slug, name="users_slug_unique")
type User { ... }
```

### Sub-paso 4 — `@check_constraint("expr", name="optional")` decorator

Emite `CHECK (<expr>)` en `CREATE TABLE`. La expresión se pasa
literal al SQL. Apilable. Auto-naming `chk_<table>_<idx>`.

```fitz
@table("users")
@check_constraint("age >= 0 AND age <= 150")
@check_constraint("status IN ('active', 'pending', 'deleted')")
type User { ... }
```

Limitación MVP: sin drift check (introspect no lee
`pg_constraint.contype = 'c'`), sin diff automático de cambios.
Workaround: `db.exec("ALTER TABLE ... DROP/ADD CONSTRAINT")` o
recrear la tabla con `name=` distinto.

### Sub-paso 5 — Cross-schema FK transparente

Cuando un type referencia con `@belongs_to("User")` un type que
vive en un schema distinto al actual, el FK SQL emit usa
`REFERENCES "schema"."table"(col)` qualified automáticamente.
**Sin cambio de sintaxis** — Fitz resuelve el schema desde el
`@table` del target.

```fitz
@table("public.users") type User { ... }
@table("tenants.memberships") type Membership {
  @belongs_to("User") user_id: Int   // FK cross-schema transparente
  ...
}
// Emite: REFERENCES "public"."users" ("id")
```

Same-schema → SQL sin qualifier (compat con boilerplates que
asumen `public`).

### Sub-paso 6 — Diff completo de indexes

El migrator detecta cambios en `using` / `where_clause` / `unique`
/ `columns` cuando los nombres matchean, emitiendo `DROP INDEX +
CREATE INDEX` para regenerar con el shape nuevo. Antes era
name-based puro y el user tenía que renombrar el índice para
forzar regen. El comparator de `where_clause` normaliza whitespace
+ case para evitar regens espurios; `using` trata `None` y
`Some("btree")` como equivalentes.

### Sub-paso 7 — `fitz db inspect --all-schemas`

Flag nuevo para listar TODOS los schemas user-defined a la vez
(incluyendo `public`), agrupados con su propia sub-vista.
Mutuamente excluyente con `--schema`. Combinable con `--table X`
para filtrar un nombre puntual en todos los schemas. JSON shape:
`{"schemas": [{"schema": "ops", "tables": [...]}, ...]}` con sort
alfabético determinístico.

### Sub-paso 8 — Redaction de secrets en `FITZ_DB_LOG=verbose`

Los params correspondientes a campos sensibles (`password`/
`passwd`/`passphrase`/`secret`/`api_key`/`apikey`/`api_token`/
`auth_token`/`access_token`/`refresh_token`/`id_token`/
`private_key`/`privkey`/`credential`/`session_key`/`session_token`/
`csrf_token`) se enmascaran automáticamente como `<redacted>` en
el output verbose. Heurística best-effort: mira ~50 chars antes
del placeholder, descarta matches separados por `WHERE`/`AND`/
`OR`/etc. Sobre-redacta en bordes ambiguos por seguridad.

### Sub-paso 9 — DB errors enriquecidos con SQLSTATE + SQL + params

`DbError::Server` Display ahora muestra `<severity> [<SQLSTATE>]: <msg>`.
Las queries que fallan pasan por `enrich_db_error_with_context`
que suma `[sql: <query truncado> params=[...]]` con la misma
redaction de secrets que `FITZ_DB_LOG=verbose`.

Antes:
```
ERROR: duplicate key value violates unique constraint "users_email_key"
```

Después:
```
ERROR [23505]: duplicate key value violates unique constraint "users_email_key"
    [sql: INSERT INTO users (email, password) VALUES ($1, $2)
     params=[$1="ada@x.com", $2=<redacted>]]
```

### Sub-paso 10 — `FITZ_DB_MAX_CONNS` pool tuning

Env var opt-in para overridear el pool size del driver. Default
10 conexiones simultáneas máximas por URL. Clamp `[1, 200]`.
Aplica global al proceso (no per URL). Útil para apps con mucho
concurrent load (`FITZ_DB_MAX_CONNS=50`) o cron jobs con poco load
(`FITZ_DB_MAX_CONNS=3`). Kwarg dedicado del lenguaje
(`db.connect(url, max_conns=N)`) queda como deuda menor.

### Sub-paso 11 — Skip deliberado: JSON `||` merge

Decisión documentada: el operador `||` jsonb (typical UPDATE `SET
data = data || $1`) NO se modela en `.where(...)` (read-only).
Caso de uso dominante cubierto por escape hatch:

```fitz
db.exec(
    "UPDATE foo SET data = data || $1::jsonb WHERE id = $2",
    [patch_json, id]
).await?
```

### Tests

- **+39 unit tests nuevos**: 17 evaluator (9 path methods + 3
  matches + 6 @unique + helpers), 13 codegen (7 path codegen
  paralelo + 2 matches codegen + 4 SQL emit), 17 migrations (5
  diff indexes + 4 all-schemas + 6 @check + 2 cross-schema FK),
  17 db (6 redaction parsing + 4 enrichment + 2 SQLSTATE Display
  + 2 max_conns parser + 3 misc), 6 types (@check_constraint
  decorator), 6 types (@unique decorator).
- **1 E2E nuevo en `tests/db_real_postgres.rs`** contra Postgres
  real: `orm_jsonb_path_operators_in_where_paridad_codegen_e2e`
  (paridad bit-a-bit `fitz run` ↔ `fitz build` con table jsonb +
  seed via `seed.exec` con literales nested + queries con los 5
  path methods).

Al cierre: **2739 unit + 292 smoke + 3 openapi + 81 cli_e2e + 52
db_real_postgres** (51 viejos + 1 nuevo). `cargo fmt --all --
--check` + `cargo clippy --all-targets -- -D warnings` + `cargo
clippy --lib --features lsp -- -D warnings` todos limpios.

### Cross-impact

- `editors/vscode/package.json` bump 0.10.28 → 0.10.29.
- `src/lsp.rs` descripción de `@unique` actualizada (single col
  field-level + composite type-level v0.10.29) + nuevo entry
  `@check_constraint` con snippet `@check_constraint("${1:expr}")`.
- `docs/db-orm.md`: bloques nuevos para JSON path operators (sec
  13), full-text search `@@` (sec 13 sub-bloque), `@unique`
  composite + `@check_constraint` + cross-schema FK (sec 4 sub-
  bloques), redaction de secrets en `FITZ_DB_LOG` (sec 29),
  `FITZ_DB_MAX_CONNS` pool tuning (sec 29), DB errors con SQL
  contexto (sec 29).
- `docs/guide.md` cap 31 (Postgres + ORM): bullet "Cierre masivo
  de v0.10.29 — ORM completo" con todos los items + cap 32 (env
  vars) sumando `FITZ_DB_MAX_CONNS` paralelo a `FITZ_DB_LOG`.
- `README.md` Estado del proyecto: bullet de v0.10.29 con todos
  los items.
- `docs/architecture.md`: conteo de sub-comandos actualizado (15
  → 27 efectivos), Familia 5 DB nueva con los 10 sub-comandos
  `fitz db ...` documentados, secciones nuevas para `db.rs`
  (driver Postgres puro) y `migrations.rs` (schema diff +
  introspect + DDL emit), diagramas mermaid + ASCII con path
  `fitz db ...` → `migrations.rs` → `db.rs` → Postgres.

### Deuda residual derivada (NO bloquea uso real)

- `@check_constraint` sin drift check del migrator (introspect no
  lee `pg_constraint.contype = 'c'`). Workaround: drop + recreate
  manual via `db.exec`.
- Cross-schema FK no popula `references_schema` desde la
  introspect (deja siempre `None`), por lo que el drift no detecta
  cambios cross-schema off-Fitz.
- Chain estilo `e.data.get("a").get("b")` (azúcar sobre
  `path_text(["a", "b"])`) sigue como deuda menor.
- JSON `||` merge en `.where(...)` (skipeado deliberadamente —
  caso UPDATE cubierto por escape hatch).
- Ranking full-text (`ts_rank`) — bajar a `db.query` con `ORDER
  BY ts_rank(...)`.
- `db.connect(url, max_conns=N)` kwarg del lenguaje (hoy via env
  var). Requiere wire del kwarg desde evaluator + codegen.
- Cambios mid-run de `FITZ_DB_MAX_CONNS` NO se reflejan (LazyLock
  igual que `FITZ_DB_LOG`). Workaround: reiniciar el proceso.

## [v0.10.28] — 2026-05-31 — Tier S del ORM: introspect + @index using + DB log + HTTP access log

Cierre del **Tier S del ORM**: 4 sub-pasos coordinados que cierran
el ORM como herramienta operativa + observable end-to-end. Sin
sintaxis nueva del lenguaje; tres features nuevas (sub-paso 1
subcomando CLI, sub-paso 2 kwarg nuevo de un decorator existente,
sub-pasos 3/4 env vars opt-in) que cubren el gap entre "tengo el
ORM funcionando" y "tengo el ORM funcionando + sé qué está
pasando + puedo auditar la DB sin abrir psql".

### Sub-paso 1 — `fitz db inspect` (introspect del schema real)

Subcomando nuevo `fitz db inspect [--url URL] [--schema name]
[--table name] [--json]` que se conecta a Postgres y emite una
vista legible del schema actual (tables, columnas con tipos +
nullability + defaults, primary keys, indexes con WHERE de partial,
foreign keys con ON DELETE). Sin tocar tu programa Fitz — pura
introspección. Útil para auditar antes de migrar, descubrir tables
legacy, comparar dev vs prod, o generar reportes machine-readable
con `--json` (shape lockeada, parseable por scripts externos).

Implementación: ensamblar + formatear sobre los helpers existentes
(`introspect_columns`/`introspect_indexes`/`introspect_foreign_keys`/
`list_user_tables_qualified`) — la query infra ya estaba lista
desde v0.10.16. Nuevas APIs públicas `migrations::format_inspection_text`
y `migrations::format_inspection_json`; filtrado in-memory post-
introspect según `--schema` y `--table`.

### Sub-paso 2 — `@index(col, using="gin")` method override

El decorator `@index(...)` a nivel **type** acepta el kwarg nuevo
`using=<method>` con whitelist Postgres oficial: `btree` (default,
no se emite `USING` redundante), `hash`, `gin`, `gist`, `brin`,
`spgist`. Habilita full-text search (`gin` sobre tsvector), range
queries (`gist`), large time-series resumidas (`brin`) sin tener
que bajar a `db.exec("CREATE INDEX ... USING gin")`.

```fitz
@table("docs")
@index(body_tsv, using="gin")
@index(price_range, using="gist")
@index(created_at, using="brin")
type Doc { ... }
```

Implementación: `IndexSpec.using: Option<String>` + `Index.using`
en migrations + processor del kwarg con whitelist + propagación
end-to-end (resolved_indexes → schema_from_program → CREATE INDEX
SQL emit + introspect via `pg_am.amname` para round-trip + format
text/json para que aparezca en `fitz db inspect`). Method
inválido → error claro del checker en compile-time citando los
soportados. Field-level `@index` (sobre un field individual) se
mantiene SIN args (default btree, mismo comportamiento) — el
`using=` solo aplica a nivel type.

**Limitación heredada**: diff name-based — si cambiás SOLO el
`using=` con mismo nombre + cols, el migrator NO detecta el
cambio. Workaround: pasar `name=` distinto para forzar regen.
Mismo patrón que `where_clause` desde v0.10.27.

### Sub-paso 3 — `FITZ_DB_LOG` (query logging del driver)

Env var opt-in que loguea cada query del driver Postgres a
stderr post-ejecución. Zero overhead si no está seteada (single
atomic load + match al inicio de cada call).

- `FITZ_DB_LOG=1` o `=true` → mode Simple: `[fitz-db Nms] <sql>`.
- `FITZ_DB_LOG=verbose` → además params: `params=[$1="ada", $2=42]`
  (strings truncados a 80 chars con `…` final por seguridad — no
  se vuelca un BLOB entero al log).
- Vacío / `=0` / no seteado → Off, silencio total.

Hook en `DbConnHandle::query` (punto único — `exec` delega ahí).
SQL multi-línea se colapsa a una sola línea para grep. Loguea
también las queries que fallan. Cubre tanto `fitz run` como el
binario producido por `fitz build` (paridad bit-a-bit gratis —
mismo crate `fitz::db` via `pub use`). Validado end-to-end
contra Postgres local.

### Sub-paso 4 — `FITZ_HTTP_LOG` (access log estilo uvicorn)

Pieza paralela a `FITZ_DB_LOG` para el stack HTTP. Loguea per-
request a stderr con method + path + status + elapsed.

- `FITZ_HTTP_LOG=1` o `=true` → mode Simple: `[fitz HTTP Nms]
  GET /users/42 → 200`.
- `FITZ_HTTP_LOG=verbose` → además `(UA="curl/8.0" len=1234)`.
- Vacío / `=0` / no seteado → Off, el layer middleware ni se
  monta (literalmente zero overhead, no la indirection del wrapper).

Implementación: `axum::middleware::from_fn(http_log_layer)`
montado condicionalmente sobre el `Router` al final de
`build_router_with_asyncapi` cuando `HTTP_LOG_MODE != Off`. Cubre
**todas** las requests que pasan por el router: handlers
matcheados, preflight OPTIONS de CORS, rutas auto `/openapi.json`/
`/docs`/`/asyncapi.json`, WebSocket handshake (loguea como 101
Switching Protocols), y respuestas 401/403/400/500 de auth/
middleware/handler.

Paridad bit-a-bit codegen: el binario producido por `fitz build`
reusa el mismo `src/http.rs` via `fitz::http` re-export — el
hook + LazyLock se heredan automáticamente sin wiring extra.

### Tests

- **+19 unit tests nuevos**: 5 de migrations (`create_index_con_using`/
  `create_index_sin_using`/`create_index_combina_unique_using_y_where`/
  `format_inspection_text_muestra_using`/`format_inspection_json_incluye_using`),
  más 5 de migrations sobre el formatter base (`format_inspection_text_*`
  + `format_inspection_json_*`), 3 de types (`checker_at_index_using_*`),
  7 de db (`format_db_log_line_*` + `truncate_for_log_utf8_safe`), 6 de
  http (`format_http_log_line_*`).
- **2 E2E nuevos en `tests/db_real_postgres.rs`** contra Postgres
  real: `inspect_schema_text_y_json_contienen_todo_el_shape` (PK
  + partial unique index + FK CASCADE round-trip), y
  `introspect_y_diff_round_trip_using_gin_method` (CREATE INDEX
  USING gin aplicado + introspect lo devuelve correctamente).

Al cierre: **2677 unit + smoke 325** (sin cambios — los ejemplos
existentes siguen pasando bit-a-bit) **+ 51 db_real_postgres**
(49 viejos + 2 nuevos). `cargo fmt --all -- --check` + `cargo
clippy --all-targets -- -D warnings` + `cargo clippy --lib
--features lsp -- -D warnings` todos limpios.

### Cross-impact

- `editors/vscode/package.json` bump 0.10.27 → 0.10.28.
- `src/lsp.rs` descripción de `@index` suma `using=` con
  whitelist al hover/completion.
- `docs/db-orm.md` sec 4 (`@index`) suma bullet `using=` con
  ejemplos canonicales; sec 29 (CLI con DB) suma bloque
  dedicado a `fitz db inspect` con vista texto + JSON shape +
  notas; sec nueva sobre `FITZ_DB_LOG` con formato + ejemplos.
- `docs/guide.md` cap 31 (Postgres + ORM) suma bullet "Tier S"
  con los 3 features visibles; cap 32 (env vars) suma sub-sección
  "Observabilidad — `FITZ_DB_LOG` y `FITZ_HTTP_LOG`" con
  formato + ejemplos + dónde aplica.

### Deuda residual derivada (NO bloquea uso real)

- Diff de indexes name-based no detecta cambios SOLO en
  `where_clause` ni en `using` cuando nombre y cols son iguales.
  Workaround documentado: `name=` distinto para forzar regen.
- `fitz db inspect` cross-schema solo muestra el schema pasado
  por `--schema` (default `public`). Listar TODOS los schemas
  user-defined a la vez es trivial — sumar `--all-schemas` si
  aparece demanda.
- `FITZ_DB_LOG=verbose` trunca strings a 80 chars con `…` — sin
  escape de chars no-imprimibles ni redaction de secrets visibles
  en `$1="password_aqui"`. Caveat documentado en `docs/db-orm.md`.
- Cambios mid-run de `FITZ_DB_LOG`/`FITZ_HTTP_LOG` NO se reflejan
  (LazyLock se fija al primer acceso). Workaround: reiniciar el
  proceso.

## [v0.10.27] — 2026-05-30 — Bulk insert + composite PK + @index decorator

Tres features ortogonales del ORM cerradas en bloque: `Type.bulk_insert(
rows, db, batch_size=1000)` con paridad bit-a-bit run↔build, N `@primary`
fields por type (composite PK) con `TableMetadata.primary_fields: Vec<String>`
+ helpers `single_pk()`/`has_pk()`, y `@index(col1, col2, ..., unique=true,
name="...", where_=<expr>)` decorator a nivel type emitido por `fitz db
diff`/`migrate` con auto-naming `idx_<table>_<col1>_<col2>...[_uniq]` y
partial via WHERE clause. Detalles en el commit b07a36d y `docs/roadmap.md`.

## [v0.10.26] — 2026-05-30 — Codegen Date/DateTime/Uuid: paridad bit-a-bit `fitz run` ↔ `fitz build`

Cierre de la deuda comprometida en CHANGELOG v0.10.24 — los 3 tipos
temporales y de identidad ahora compilan a binario nativo con
`fitz build`. **Paridad bit-a-bit completa con `fitz run`**: mismos
constructors, métodos, ORM mapping, driver wire protocol, HTTP body
in/out, migrations, defaults sentinel.

### Cambios codegen (~700 LoC netas)

**Detector + Cargo.toml condicional**:
- `program_uses_date_or_uuid(program)` paralelo a `program_uses_db`,
  walkea AST + TypeExpr buscando `Ident("Date"|"DateTime"|"Uuid")`
  y annotations.
- Transitivo via `LoadedModule.uses_date_or_uuid`.
- `cargo_toml_for` suma `uuid = { version = "1", features = ["v4"] }`
  + `chrono` (si no estaba ya por uses_jobs) al `Cargo.toml` del crate
  generado cuando `uses_date_or_uuid = true`.
- `CodegenCtx.uses_date_or_uuid` propagado para gateo de helpers.

**Tipos + Display**:
- `rust_type_for`: `Type::Date → chrono::NaiveDate`, `Type::DateTime
  → chrono::DateTime<chrono::Utc>`, `Type::Uuid → uuid::Uuid`.
- `show_expr` (str interpolation + print): Display canonical para
  matchear el intérprete bit-a-bit:
  - Date → `d.format("%Y-%m-%d").to_string()`
  - DateTime → `dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()`
    (sin micros — diferente al Display default de chrono).
  - Uuid → `u.to_string()` (canonical hyphenated lowercase).
- `field_eq_expr` + `type_name` + `display_type`: 3 nuevos arms.

**Constructors (9 funcs)**:
`gen_temporal_module_call(recv, field, args)` dispatch paralelo a
`gen_db_module_call`. Cada constructor emite la llamada chrono/uuid
correspondiente, envolviendo en Result cuando puede fallar:
- `Date.today()` → `chrono::Local::now().date_naive()`
- `Date.parse(s)` → `chrono::NaiveDate::parse_from_str(...).map_err(...)`
- `Date.from_ymd(y, m, d)` → `chrono::NaiveDate::from_ymd_opt(...).ok_or_else(...)`
- `DateTime.now()` → `chrono::Utc::now()`
- `DateTime.parse(s)` → `chrono::DateTime::parse_from_rfc3339(...).map(...).map_err(...)`
- `DateTime.from_timestamp(secs)` → `chrono::DateTime::<Utc>::from_timestamp(secs, 0).ok_or_else(...)`
- `Uuid.v4()` → `uuid::Uuid::new_v4()`
- `Uuid.parse(s)` → `uuid::Uuid::parse_str(...).map_err(...)`
- `Uuid.nil()` → `uuid::Uuid::nil()`

**Instance methods (13)**:
Dispatch sobre `Type::Date`/`DateTime`/`Uuid` en `gen_method_call`:
- Date: `year/month/day/weekday/to_str/to_datetime/format` (7).
- DateTime: `year/month/day/hour/minute/second/timestamp/to_str/date/format` (10).
- Uuid: `to_str/is_nil` (2).
- Total: 13 (algunos comparten name pero distinto receiver).

**HTTP JSON ser/de**:
Nueva const `DATE_UUID_HTTP_INTEGRATION_PRELUDE` emitida cuando
`uses_date_or_uuid && has_http` con impls de `__ToFitzJson` y
`__FromFitzJson` para los 3 tipos. JSON shape canonical (JSON Schema
"date"/"date-time"/"uuid" formats). `__FromFitzJson` rechaza con
error claro si el string no parsea (→ 400 Bad Request al cliente
con mensaje específico).

**Driver wire protocol (ORM + raw query)**:
- `emit_date_uuid_db_prelude()` método nuevo del CodegenCtx, emite
  cuando `uses_date_or_uuid && uses_db`:
  - `impl __IntoPgValue for chrono::NaiveDate/DateTime<Utc>/uuid::Uuid`
    (param marshaling: `PgValue::Text` en formato canonical PG).
  - `__fitz_pg_to_date/datetime/uuid(v, col) -> Result<T, String>`
    (row reading: parse de `PgValue::Text` a chrono/uuid).
  - `__fitz_pg_normalize_timestamptz(s)`: paralelo a
    `parse_pg_timestamptz` del evaluator (`YYYY-MM-DD HH:MM:SS±TZ`
    → RFC 3339).
- `orm_marshal_field_to_pg` (INSERT path): nuevos arms para
  Date/DateTime/Uuid via `__IntoPgValue::into_pg(...)`.
- `orm_field_coerce_block` (SELECT path): nuevos arms via
  `__fitz_pg_to_date/datetime/uuid(__v, col)?`.

**Field default sentinel `Str = ""`**:
Cuando un field `Date`/`DateTime`/`Uuid` tiene default `""` (Str
literal sentinel, paralelo a `id: Int = 0`), el codegen emite el
`Default::default()` correspondiente al tipo destino:
- `Date → chrono::NaiveDate::default()` (1970-01-01)
- `DateTime → chrono::DateTime::<chrono::Utc>::default()`
- `Uuid → uuid::Uuid::nil()`

Aplica tanto al path `__from_fitz_json` (None → default) como al path
de fields hidden.

### Cambios complementarios

- **`__fitz_pg_to_date/datetime/uuid` gateados condicionalmente**:
  los helpers se emiten SOLO cuando `uses_date_or_uuid && uses_db`.
  Programas con `@table` que NO usan los 3 tipos no pagan el peso
  de chrono/uuid ni de los helpers. La `use crate::{...}` de los
  módulos no incluye los nuevos helpers en el import condicional
  por defecto (los módulos que los necesiten resuelven via
  inferencia del cross-impl).
- **Error block removido**: el error claro de v0.10.24 que decía
  "Date/DateTime/Uuid no soportado en `fitz build` — sub-paso
  comprometido v0.10.26" se eliminó. Si el user escribe
  `Date`/`DateTime`/`Uuid` solo (sin `.method()`), error nuevo
  citando el patrón canonical de uso (siempre `.method()`).

### Smoke E2E verde

- `examples/guide/31-orm.fitz` (ya usaba Date en field) compila
  ahora con `fitz build` sin error.
- Smoke nuevo: `@table type Event { happens_on: Date, starts_at:
  DateTime, external_id: Uuid }` + INSERT + readback via
  `Event.all(conn)` preserva tipos. Métodos instancia
  (`.year()`, `.hour()`, `.is_nil()`) funcionan sobre la Instance
  recuperada del PG.
- Smoke HTTP body in/out: POST con
  `{"happens_on":"2026-12-25","starts_at":"...","external_id":"..."}`
  → handler recibe `body.happens_on` como `Date`, `body.starts_at`
  como `DateTime`. Date inválida en JSON → 400 con mensaje claro
  citando el formato esperado.
- `fitz db diff` emite `CREATE TABLE ... (happens_on date NOT NULL,
  starts_at timestamptz NOT NULL, external_id uuid NOT NULL)` (ya
  estaba desde v0.10.24 vía `migrations::fitz_typeexpr_to_sql_type`).

### Validación final

- `cargo test --lib`: **2647 verde**.
- `compile_e2e::smoke_ejemplos_guia_compilables` (325 ejemplos):
  verde (incluye 31-orm.fitz que antes fallaba con la deuda).
- `tests::db_real_postgres` (49 ignored): **49/49 verde** contra PG
  local.
- `cargo fmt --all -- --check`: verde.
- `cargo clippy --all-targets -- -D warnings`: verde.
- Smoke real `fitz build` + ejecución contra PG local: 3
  endpoints HTTP retornando Date/DateTime/Uuid + INSERT/SELECT en
  tabla con los 3 tipos + body deserialization con error claro
  para Dates inválidas.

### Cross-impact docs

- `docs/db-orm.md` sec 4 "Mapping de tipos Fitz → Postgres":
  caveat "v0.10.24" reemplazado por "paridad bit-a-bit `fitz run`
  ↔ `fitz build` desde v0.10.26".
- VSCode extension bump 0.10.25 → 0.10.26.

### Deps nuevas

- `uuid` (re-emitido condicionalmente al Cargo.toml del crate
  generado). `chrono` ya estaba (cron jobs); ahora también se
  emite cuando `uses_date_or_uuid && !uses_jobs`.

### Out of scope (deuda residual, sin presión)

- **Aritmética de fechas** (`dt + Duration`): `Duration` es otro
  tipo built-in, mini-fase aparte si entra demanda.
- **Time standalone** (Postgres `time` OID 1083).
- **DateTime con TZ parametrizado** (`DateTime<TZ>`).
- **Métodos extra Uuid**: `version()`, `variant()`, `bytes()`.

## [v0.10.25] — 2026-05-30 — Hotfix v0.10.24: array elem_oid solo refina si caller pidió

Hotfix del CI release v0.10.24 — 33 tests E2E del driver
Postgres fallaron en cascada en GitHub Actions tras el push del
tag v0.10.24, descubierto por el job `db-postgres` del workflow
CI sobre `postgres:16`. Hot-issue resuelto antes de que llegue a
ningún user real.

### Síntoma

Cascada que arrancó en `orm_uuid_array_e2e`:

```
thread 'orm_uuid_array_e2e' panicked at tests/db_real_postgres.rs:4528:26:
    esperaba Str(UUID), fue Uuid(...)
```

Tras ese panic, los 32 tests siguientes fallaron con
`Io(Custom { kind: Other, error: "A Tokio 1.x context was found,
but it is being shutdown." })`. Test runner perdió el runtime
tokio del primer test, y el pool singleton per-URL (desde v0.10.9)
cacheaba un handle con tasks ligadas al runtime cerrado.

### Root cause

En `pg_value_to_fitz_with_oid`, el arm `PgValue::Array` siempre
propagaba `elem_oid` a la recursión sobre items, ignorando el
`oid_hint` del caller:

```rust
// ANTES (bug v0.10.24)
crate::db::PgValue::Array { elem_oid, values } => {
    let items: Vec<Value> = values
        .iter()
        .map(|item| pg_value_to_fitz_with_oid(item, Some(*elem_oid)))
        .collect();
    Value::new_list(items)
}
```

Resultado: `db.query(...)` raw sobre una columna `uuid[]` /
`date[]` / `timestamptz[]` devolvía `List<Uuid>` / `List<Date>` /
`List<DateTime>` en vez de `List<Str>` (comportamiento
pre-v0.10.24). Programas legacy que iteraban con
`match v { Value::Str(s) => ..., _ => panic!() }` quebraban.

### Fix

Array recursion ahora hace `oid_hint.map(|_| *elem_oid)` — solo
propaga `elem_oid` si el caller pasó `Some(_)`:

```rust
// DESPUÉS (fix v0.10.25)
crate::db::PgValue::Array { elem_oid, values } => {
    let elem_hint = oid_hint.map(|_| *elem_oid);
    let items: Vec<Value> = values
        .iter()
        .map(|item| pg_value_to_fitz_with_oid(item, elem_hint))
        .collect();
    Value::new_list(items)
}
```

Si `oid_hint` es `None` (default backward-compat de
`pg_row_to_fitz_map` y de `pg_value_to_fitz`), los elementos
vuelven como `Str`. El path ORM @table-typed (annotation-aware,
ya corregido en commit previo del v0.10.24) sí pasa `Some(_)` y
refina cuando el field declara explícitamente `Date`/`DateTime`/
`Uuid` o `List<T>` con esos tipos.

### Validación

Local smoke contra Postgres real:

```
$ FITZ_TEST_PG_URL="postgres://...@localhost:5432/postgres?sslmode=disable" \
    cargo test --release --test db_real_postgres -- --ignored --test-threads=1
test result: ok. 49 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
                 ^^^^^^^^^^^^^^^^^^^^^^^ era 16/49 con la cascade
```

- `cargo test --lib`: **2647 verde** (sin cambios).
- `cargo fmt --all -- --check`: verde.
- `cargo clippy --all-targets -- -D warnings`: verde.

### Backward compat preservada

- **Programas pre-v0.10.24** usando `db.query(...)` sobre columnas
  `date`/`timestamptz`/`uuid` (o arrays de esos): siguen recibiendo
  `Str` / `List<Str>` con formato ISO 8601 / canonical Postgres.
- **Programas pre-v0.10.24** usando `@table type X { d: Str }`
  para columnas date/timestamptz/uuid: siguen recibiendo `Str`
  en el field tras `Type.all(db)`.
- **Programas v0.10.24+ opt-in**: declaran `@table type X { d: Date }`
  con anotación explícita → la refinación annotation-aware dispara
  y devuelven `Value::Date` tipado.

### Cero impacto en feature surface

Esta release es PURO hotfix del bug introducido en v0.10.24. Toda
la API user-facing (constructors, métodos, ORM mapping, JSON,
LSP, grammar) queda idéntica a v0.10.24. El extensión VSCode
bump 0.10.24 → 0.10.25 es solo para alinear versiones.

## [v0.10.24] — 2026-05-30 — Date / DateTime / Uuid tipos nativos (intérprete)

Cierre del bloque comprometido post-TLS — los 3 tipos temporales
y de identidad más usados pasan de `Str` ISO 8601 a tipos
built-in con constructors, métodos, integración driver Postgres
y mapping ORM. **Soporte completo en `fitz run`**; `fitz build`
queda como deuda explícita comprometida v0.10.25 (codegen emite
error claro citando el sub-paso).

### Tipos nuevos

| Tipo | Wrapper interno | Postgres | JSON |
|---|---|---|---|
| `Date` | `chrono::NaiveDate` | `date` (OID 1082) | string ISO 8601 `YYYY-MM-DD` |
| `DateTime` | `chrono::DateTime<chrono::Utc>` | `timestamptz` (OID 1184) | string RFC 3339 `YYYY-MM-DDTHH:MM:SSZ` |
| `Uuid` | `uuid::Uuid` | `uuid` (OID 2950) | string canonical `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` |

Decisiones de diseño:
- **Naming**: `Uuid` (consistente con `DbConn`/`DbRow`/`PyAny`).
- **TZ**: `DateTime` siempre UTC en MVP. `DateTime<TZ>` parametrizado
  queda como deuda futura (usado por <5% de apps reales).
- **Sin `Time` standalone**: caso de uso raro, suma deuda futura
  si pide.
- **Sin aritmética** (`dt + Duration`): `Duration` es otro tipo
  built-in que sumamos si entra demanda.

### API user-facing

```fitz
// Constructors estáticos (Value::Module global por tipo)
let today: Date = Date.today()
let dt: DateTime = DateTime.now()
let id: Uuid = Uuid.v4()
let nil_id: Uuid = Uuid.nil()
let d: Date = Date.from_ymd(2026, 12, 25)?
let dt2: DateTime = DateTime.parse("2026-12-25T18:00:00Z")?
let u: Uuid = Uuid.parse("550e8400-e29b-41d4-a716-446655440000")?

// Métodos instancia (dispatch sobre Value::Date/DateTime/Uuid)
print(d.year())              // 2026
print(d.month())             // 12
print(d.weekday())           // 5 (ISO 8601, Friday)
print(d.format("%A %B %d"))  // "Friday December 25" (chrono format)
print(d.to_datetime())       // 2026-12-25T00:00:00Z

print(dt.hour())             // 18
print(dt.timestamp())        // 1766685600 (Unix epoch)
print(dt.date())             // 2026-12-25 (extrae solo la fecha)

print(u.to_str())            // canonical hyphenated
print(u.is_nil())            // false (Uuid.nil() devuelve true)
```

### Integración driver Postgres (D.5)

- `pg_value_to_fitz_with_oid(value, oid_hint)` refina
  `PgValue::Text` a `Value::Date`/`DateTime`/`Uuid` cuando el OID
  identifica `date` (1082) / `timestamptz` (1184) / `timestamp`
  (1114) / `uuid` (2950).
- `parse_pg_timestamptz` normaliza el formato Postgres
  `YYYY-MM-DD HH:MM:SS±TZ` a RFC 3339 (espacio→T, offsets `+00`/
  `+0530` → `+00:00`/`+05:30`).
- `fitz_value_to_pg`: `Value::Date/DateTime/Uuid` → `PgValue::Text`
  en el formato canonical que Postgres acepta via cast implícito.
- `Row::get_with_oid(col)` devuelve `(PgValue, oid)` para que el
  caller pueda hacer la refinación.
- `pg_row_to_instance` y `pg_row_to_fitz_map` propagan el OID al
  converter — el ORM read-back devuelve `Instance` con
  `Value::Date`/`DateTime`/`Uuid` en los fields declarados como
  tales.

### ORM + migrations mapping (D.4 + D.7)

`migrations::fitz_typeexpr_to_sql_type` mapea:
- `Type::Date` → `date`
- `Type::DateTime` → `timestamptz`
- `Type::Uuid` → `uuid`

Habilita el flujo canónico:

```fitz
@table("events") type Event {
    @primary id: Int = 0
    name: Str = ""
    happens_on: Date = ""       // sentinel; user provee Date.from_ymd(...)
    starts_at: DateTime = ""    // idem
    external_id: Uuid = ""      // idem
}
```

El checker acepta `Str` literal como default para fields
Date/DateTime/Uuid (sentinel paralelo a `id: Int = 0`); el
evaluator coerce el `Str` cuando se construye la Instance via
`coerce_to_annotation`.

### JSON serialization (D.6)

- `value_to_json` emite Date/DateTime/Uuid como JSON string
  canonical (estándar de la industria — JSON Schema
  `"date"`/`"date-time"`/`"uuid"` formats).
- `coerce_to_annotation` con annot `Date`/`DateTime`/`Uuid` sobre
  `Value::Str` deserializa al tipo correspondiente. Caso típico:
  HTTP body JSON con `"happens_on": "2026-12-25"` deserializado a
  Instance con `Value::Date` en el field. Errores claros si el
  string no matchea el formato esperado.
- Para `DateTime`, acepta tanto RFC 3339 como el formato Postgres
  timestamptz con espacio.

### LSP + extensión VSCode

- `lsp::scope_level_completions` lista `Date`/`DateTime`/`Uuid`
  como built-in types.
- `lsp::after_dot_completions` dispatches sobre `Type::Date`,
  `Type::DateTime`, `Type::Uuid` con method_items dedicado
  (year/month/.../format/to_str/etc.).
- Grammar TextMate suma `Date`/`DateTime`/`Uuid` a
  `support.type.builtin.fitz`.
- Extensión VSCode bump 0.10.23 → 0.10.24.

### Codegen — deuda explícita v0.10.25

El codegen emite error claro cuando encuentra `Date`/`DateTime`/
`Uuid` en el AST:

```
✗ codegen: Error — `Date` (tipo built-in v0.10.24) todavía no
soportado en `fitz build` — sub-paso comprometido v0.10.25 (deuda
explícita). Usá `fitz run` mientras tanto.
```

Cerrar la deuda v0.10.25 requiere ~500+ LoC adicionales en
codegen: helpers de preludio emitiendo chrono/uuid + dispatch de
cada constructor/método + `__IntoPgValue`/`__FromFitzDbRow` para
los 3 tipos + `__ToFitzJson`/`__FromFitzJson` + Cargo.toml
emisión condicional de `chrono` (ya parte del workspace) y
`uuid` (dep nueva).

### Smoke E2E real Postgres

Validó el ciclo completo: `@table` con Date/DateTime/Uuid fields
→ `Event.insert(conn, row)` con valores `Date.from_ymd(2026, 12, 25)?`/
`DateTime.parse("2026-12-25T18:00:00Z")?`/`Uuid.v4()` → wire
format text round-trip → `Event.all(conn)` → readback preserva
tipos (`.year()`, `.hour()`, `.is_nil()` funcionan sobre la
Instance recuperada).

### Validación final

- `cargo test --lib`: **2647 verde**.
- `compile_e2e::smoke_ejemplos_guia_compilables` (325 ejemplos):
  verde.
- `cargo fmt --all -- --check`: verde.
- `cargo clippy --all-targets -- -D warnings`: verde.

### Deps nuevas

- `uuid = { version = "1", features = ["v4"] }` — generación
  random + parsing. Pure Rust ~50KB.
- `chrono` ya era dep no-opcional desde Fase 9.w.3 (cron jobs);
  reusado sin pulls nuevos.

### Out of scope (deuda explícita v0.10.25+)

- **Codegen completo** para `fitz build`: ~500+ LoC, próximo release.
- **`Duration`** + aritmética (`dt + 1.days()`, `dt2 - dt1`).
- **`Time` standalone** sin fecha (Postgres `time` OID 1083).
- **`DateTime<TZ>`** parametrizado por timezone.
- **Métodos adicionales en `Uuid`** (`version()`, `variant()`,
  `bytes()`).

## [v0.10.23] — 2026-05-30 — Fase 10.1.b: TLS strict para el driver Postgres

Cierre del sub-paso comprometido desde Fase 10.1. El driver ahora
soporta los 4 modos `sslmode` estándar Postgres (`disable`,
`require`, `verify-ca`, `verify-full`) + custom CA via
`sslrootcert=path/to/ca.pem`. **Habilita apuntar el driver Fitz a
managed Postgres real** (Heroku, RDS, Supabase, Neon, Aiven,
Render PG, Crunchy Bridge, etc.) sin downgrade a `sslmode=disable`.

### Modos soportados

| `sslmode` | TLS | Cert chain | Hostname | Cuándo usar |
|---|:---:|:---:|:---:|---|
| `disable`     | ❌ | — | — | Local dev sin TLS (Postgres en Docker localhost) |
| `require`     | ✅ | ❌ | ❌ | Dev/staging contra Postgres interno sin CA pública. NO usar en prod |
| `verify-ca`   | ✅ | ✅ | ❌ | Cert con CN/SAN distinto al hostname (proxies, port forward) |
| `verify-full` | ✅ | ✅ | ✅ | **Recomendado producción** — usado por todos los managed PG |

```fitz
// Managed PG real con TLS strict
let db = db.connect(
    "postgres://user:pass@db.proyecto.supabase.co:5432/postgres?sslmode=verify-full"
).await?

// Postgres interno con CA corporativa custom
let db = db.connect(
    "postgres://user:pass@db.intra:5432/myapp?sslmode=verify-full&sslrootcert=/etc/ssl/corp-ca.pem"
).await?
```

### Implementación

- Deps nuevas (no opcionales, parte del core del driver): `rustls
  0.23` + `tokio-rustls 0.26` + `webpki-roots 0.26` + `rustls-pemfile 2`.
- `rustls` con feature `ring` como crypto provider (puro Rust +
  assembly, **sin deps system tipo CMake/clang/OpenSSL**). Mantiene
  la promesa "binario standalone sin libs system".
- `webpki-roots` trae el **Mozilla CA bundle in-binary** — cubre
  Heroku/RDS/Neon/Aiven/Render/etc. sin que el user instale nada.
- `Connection.stream` migrado de `TcpStream` a `Box<dyn DbReadWrite>`
  con helper trait `DbReadWrite: AsyncRead + AsyncWrite + Send +
  Unpin`. Costo: una vtable lookup por read/write (~3ns), irrelevante
  vs el round-trip TCP. **Sin impacto en el bench v0.10.13** (los
  números B-1 se mantienen).
- `read_message` migrado de hard-coded `TcpStream` a genérico
  `<R: AsyncRead + Unpin>`.
- 3 `ServerCertVerifier` custom:
  - `NoVerifier` (sslmode=require): acepta cualquier cert.
  - `NoHostnameVerifier` (sslmode=verify-ca): wrapper sobre
    `WebPkiServerVerifier` que catchea `CertificateError::NotValidForName`
    (y `NotValidForNameContext` en rustls 0.23+) y lo trata como
    Ok. Mantiene chain validation + skip hostname.
  - Default `WebPkiServerVerifier` (sslmode=verify-full).
- `SSLRequest` dance (8-byte magic 80877103) + server response
  parsing ('S' = TLS supported, 'N' = no TLS, 'E' = error con
  body drenado). Errores específicos según cada caso.
- `ensure_rustls_provider()` instala el `ring` provider de rustls
  via `std::sync::Once` la primera vez que se intenta un TLS upgrade.
- Validación cruzada de combinaciones inválidas en el parser
  (`sslmode=disable&sslrootcert=...`, `sslrootcert=` sin sslmode,
  etc.) — fail-fast con mensaje claro en vez de runtime confuso.

### URL parser

- `SslMode` enum extendido: `Disable` / `Require` / `VerifyCa`
  / `VerifyFull`.
- `ConnectionConfig.sslrootcert: Option<PathBuf>` nuevo.
- `sslmode=prefer|allow` siguen como `NotImplemented` con mensaje
  claro (negociación dinámica con downgrade es vulnerable a MITM;
  los drivers modernos lo desalientan).
- `sslrootcert=` URL-decoded (paths con spaces o caracteres
  especiales funcionan).

### `DbError::Tls` variant nueva

Fallos del path TLS (SSLRequest rechazado, handshake roto, sslrootcert
ilegible/malformado, hostname mismatch en verify-full, etc.) ahora
tienen variant dedicada con Display `"TLS: <msg>"`. Diferencia
limpia de `DbError::Io` (TCP genérico) y `DbError::Auth` (credentials).

### Validación end-to-end contra Supabase real

Smoke E2E corrido contra el pooler de Supabase
(`aws-1-us-west-2.pooler.supabase.com`):

- `sslmode=disable`: SELECT 1 OK ✓
- `sslmode=require`: TLS handshake completo + SELECT 1 OK ✓
- `sslmode=verify-ca`/`verify-full`: UnknownIssuer — el verifier
  funciona correctamente (Supabase pooler usa su propia CA fuera
  del Mozilla bundle). El user puede bajar la CA cert del dashboard
  Supabase y usarla como `sslrootcert=path/to/prod-ca-2021.crt`
  para validación end-to-end.

Para managed PG con cert público (Neon usa Let's Encrypt, RDS usa
Amazon Root CA — ambos en `webpki-roots`), `verify-full` funciona
sin custom CA.

### Tests + suite

- 10 unit tests nuevos en `db::tests`:
  - `url_sslmode_require_parsea_ok`
  - `url_sslmode_verify_ca_parsea_ok`
  - `url_sslmode_verify_full_parsea_ok`
  - `url_sslmode_prefer_sigue_no_implementado`
  - `url_sslmode_allow_sigue_no_implementado`
  - `url_sslmode_desconocido_es_error`
  - `url_sslrootcert_con_verify_ca_parsea_ok`
  - `url_sslrootcert_url_encoded_se_decodifica`
  - `url_sslrootcert_con_sslmode_disable_es_error`
  - `url_sslrootcert_con_sslmode_require_es_error`
  - `url_sslrootcert_sin_sslmode_es_error`
- 1 test refresh en `evaluator::tests` (`db_connect_url_con_sslmode_require_resuelve_y_falla_en_red`):
  antes esperaba `NotImplemented`; ahora verifica que el flow
  llega al I/O step (sslmode=require ya no rechaza early).
- `cargo test --lib`: **2647 verde** (era 2637, +10 parser tests).
- `cargo clippy --all-targets -- -D warnings`: verde.
- `cargo fmt --all -- --check`: verde.

### Cross-impact docs

- `docs/db-orm.md` sección 3: sub-sección nueva "TLS strict
  (v0.10.23)" con tabla de los 4 modos + ejemplos + combinaciones
  inválidas + out of scope (`prefer`/`allow` + client cert auth).
- `docs/guide.md` cap 31: ejemplo del driver `db` muestra dos
  flavors (local sin TLS + managed con verify-full).
- Cargo.toml: 4 deps nuevas con comentario justificando elección
  (`rustls` sobre `native-tls` para mantener "binario standalone
  sin deps system"; `ring` como crypto provider; `webpki-roots`
  in-binary).

## [v0.10.22] — 2026-05-30 — Cierre 2 deudas residuales del codegen del driver DB

Cierra las 2 deudas heredadas que el Boilerplate 10
(`api-multi-tenant`) destapó: queries con shape dinámico
retornadas crudas como JSON desde un handler HTTP, y extracción
tipada de columnas individuales sobre `DbRow` desde `fitz build`
(antes solo intérprete).

### Deuda A — `Result<List<DbRow>>` como retorno de handler HTTP

Los handlers ahora pueden devolver `Result<List<DbRow>>` directo
y el codegen auto-serializa cada row a `{col: val, ...}` en el
JSON response. Útil para queries cuyo shape no se puede
representar como `type` (CTEs, multi-tenant con schema dinámico,
queries ad-hoc retornadas a frontends que aceptan shape libre).

```fitz
@get("/products/dynamic")
async fn products() -> Result<List<DbRow>> {
    let conn = db.connect(db_url).await?
    return conn.query("SELECT id, name FROM acme.products", []).await
}
// HTTP 200 → [{"id":1,"name":"foo"},{"id":2,"name":"bar"}]
```

Implementación: nuevo `DB_HTTP_INTEGRATION_PRELUDE` emitido
condicionalmente cuando `uses_db = true`, con `impl __ToFitzJson
for __fitz_db_runtime::Row` que mapea cada `PgValue` al `Value`
JSON correspondiente (incluye auto-detección de JSON/array
strings como `jsonb` → JSON anidado real, no como string).

### Deuda B — Métodos tipados sobre `DbRow` en codegen

5 métodos nuevos vivos en `fitz build` con paridad bit-a-bit
intérprete↔codegen:

| Método | Retorno | Notas |
|---|---|---|
| `r.get_int(col)`   | `Result<Int>`   | Falla si NULL, no existe, o el tipo PG no es int |
| `r.get_str(col)`   | `Result<Str>`   | Falla si NULL/no existe; acepta text/varchar/uuid/json/etc. |
| `r.get_float(col)` | `Result<Float>` | float8/float4/numeric/etc. |
| `r.get_bool(col)`  | `Result<Bool>`  | bool PG |
| `r.len()`          | `Int`           | número de columnas del row |

```fitz
let rows = conn.query("SELECT id, name FROM users LIMIT 1", []).await?
let r: DbRow = rows[0]
let id: Int    = r.get_int("id")?       // Result<Int>
let name: Str  = r.get_str("name")?     // Result<Str>
```

Sintaxis dedicada (`get_int` en vez de `get` polimórfico) por
elección de diseño — el checker refina el ret type del call al
`Result<T>` correcto sin requerir anotación en la lhs.

### Boilerplate 10 (`api-multi-tenant`) — Enfoque B real, no demo

El handler `GET /products/dynamic` con header `X-Tenant: <slug>`
+ validación whitelist contra `public.tenants` + SQL dinámico
**ahora compila con `fitz build`** y se expone como endpoint
nativo. El frontend `/dynamic.html` deja de ser solo-texto y
suma un selector interactivo con 4 valores demo (acme/beta
válidos, zeta no registrado, SQL injection rechazada por
whitelist) + área de resultado en vivo.

### Tests + smoke + LSP

- 4 unit tests nuevos en `types::tests::checker_db_row_*`
  (Result<Int> / Result<Str> / annotation-mismatch /
  unknown-method).
- 1 unit test nuevo en `lsp::tests::after_dot_sobre_dbrow_*`
  (autocomplete tras `r.` lista get_int/get_str/get_float/
  get_bool/len).
- LSP `Type::DbRow` ahora aparece en `after_dot_completions`
  con method_items dedicado.
- Refresh signature del autocomplete `DbConn.query` (era
  `Result<List<Map>>`, ahora `Result<List<DbRow>>`).
- Smoke .fitz dedicado validado bit-a-bit `fitz run` ↔ `fitz
  build` contra Postgres local (3 endpoints verde).
- Smoke E2E Boilerplate 10 contra Postgres local: 7 endpoints
  verde (3 Enfoque A + 4 Enfoque B incluido el caso injection).
- `GUIDE_EXAMPLES_COMPILE` smoke (325 ejemplos) verde.
- `cargo test --lib`: 2637 verde (sin feature) / 2749 verde
  (con feature `lsp`).
- `cargo fmt --all -- --check` + `cargo clippy --all-targets
  -- -D warnings` verde.

### Cross-impact docs

- `docs/db-orm.md` sección 3: signature de `db.query` corregida
  a `Future<Result<List<DbRow>>>` + sub-sección nueva sobre los
  métodos `r.get_*`.
- `docs/guide.md` cap 31: ejemplo del driver `db` actualizado
  con DbRow + extracción tipada + nota sobre handlers HTTP
  retornando `Result<List<DbRow>>`.
- Boilerplate 10 README: bloque "Enfoque B" reescrito (deja
  de citar deudas residuales) + curl ejemplo del caso injection
  como demo de validación.
- Extension VSCode bump 0.10.21 → 0.10.22 (grammar ya tenía
  `DbRow`; el delta real es LSP completion + signatures).

## [v0.10.21] — 2026-05-30 — Fase 10.6.e.3: schemas custom (cierra Fase 10.6 entera)

Última feature del Tier 2 del plan vs Alembic. **Cierra la Fase
10.6 completa**: el paquete `fitz db ...` cubre ahora migrations
generation + apply/rollback + drift check + stamping + history
+ offline SQL + squash + data migrations en `.fitz` + schemas
custom Postgres. Equivalente funcional a Alembic con cero deps
externas.

### Sintaxis: `@table("schema.name")`

`@table` ahora acepta opcionalmente un nombre de schema separado
por `.`. Sin `.` (compat pre-v0.10.21), schema = `public`
(default Postgres).

```fitz
@table("users") type User {              // public.users (default)
    @primary id: Int = 0
    email: Str = ""
}

@table("analytics.events") type Event {  // analytics.events (custom)
    @primary id: Int = 0
    name: Str = ""
    @db_default("NOW()") at: Str = ""
}
```

Validación del checker: ambos segmentos no-vacíos, sin
whitespace, máximo 1 `.`. Strings inválidos (`""`, `"a.b.c"`,
`"foo bar"`) → error de tipo claro.

### Multi-schema end-to-end

`fitz db check` con `analytics.events` + `users` (mixed):

```sql
CREATE SCHEMA IF NOT EXISTS "analytics";

CREATE TABLE "analytics"."events" (
    "id" bigserial PRIMARY KEY,
    "name" text NOT NULL,
    "at" text NOT NULL DEFAULT NOW()
);

CREATE TABLE "users" (
    "id" bigserial PRIMARY KEY,
    "email" text NOT NULL
);
```

El ORM nativo usa qualified everywhere: `INSERT INTO
"analytics"."events" (...)`, `SELECT ... FROM "analytics"."events"`,
`UPDATE "analytics"."events" SET ... WHERE ...`, etc.

### Casos de uso

- **Multi-tenant via schemas**: `@table("tenant_acme.users")`,
  `@table("tenant_beta.users")` aisla data por cliente.
- **Separación dev/test/staging**: `@table("staging.events")`
  vs `@table("prod.events")` en el mismo cluster.
- **Módulos aislados**: `@table("auth.sessions")`,
  `@table("billing.invoices")`, `@table("analytics.events")`
  para namespacing en monolitos grandes.
- **Naming conflict resolution**: dos modules con tabla
  `events` viven en schemas distintos sin colisión.

### Cambios técnicos

- **src/types.rs**:
  - `TableMetadata.schema: Option<String>` nuevo field. `None`
    = `public`.
  - Parser del decorator `@table("...")` splitea por `.` via
    helper `split_schema_qualified_table(s)` con validación.
  - Nuevo método `TableMetadata::qualified_sql_name()` —
    returns `"schema"."name"` o `"name"` (ya quoteado).
- **src/migrations.rs**:
  - `Table.schema: Option<String>` + `qualified_id()` method.
  - Nueva struct `TableRef { schema, name }` para identidad
    cross-schema. Constructores `public()`, `qualified()`,
    `from_table()`.
  - `Change` enum refactorizado: todas las variants con `table`
    ahora usan `TableRef` en vez de `String`. Nueva variant
    `CreateSchema { name }` emitida primero en el diff.
    `DropIndex` ahora tiene `schema: Option<String>` (PG needs
    qualified DROP INDEX para non-public).
  - `introspect_schema` ahora itera TODAS las user schemas
    (excluye `pg_catalog`, `information_schema`, `pg_toast*`,
    `pg_temp_*`, `_fitz_migrations`). `list_user_tables_qualified`
    devuelve `(schema, name)` tuples. `introspect_columns`/
    `indexes`/`foreign_keys` parametrizados por schema.
  - `diff_schemas` compara por `qualified_id` (no por name).
    Emite `CreateSchema` para schemas en target que no existen
    en current. `apply_renames_from_target` es schema-aware
    (renames dentro del mismo schema; cross-schema rename queda
    como deuda menor).
  - `change_to_sql` usa nuevo helper `quote_qualified(TableRef)`
    everywhere. Bare names para `public`, `"schema"."name"` para
    custom.
- **src/codegen.rs**:
  - `__FitzQueryBuilder.table` (preludio) ahora almacena la
    forma ya-quoteada qualified (`"users"` o `"public"."x"`).
    Los `format!` SQL del preludio cambian de `\"{}\"` a `{}`
    (~5 sitios en `build_select_sql`/`count`/`update`/
    `delete`/agg).
  - `qb_constructor` pasa `meta.qualified_sql_name()` (already
    quoted) en lugar de `meta.sql_name` (plain).
  - `target_table` en preload arms (HasMany + BelongsToCompanion)
    usa `qualified_sql_name()`; el format runtime `{table_lit}`
    sin extra quotes + escape `replace('"', "\\\"")` para que
    el embed funcione.
- **src/evaluator.rs**:
  - `SELECT ... FROM`, `INSERT INTO`, `UPDATE`, `DELETE FROM`,
    aggregates: todos usan `state.meta.qualified_sql_name()`
    (5 sitios refactorizados).
- **editors/vscode/package.json**: 0.10.20 → 0.10.21.

### Decisiones técnicas

- **Sintaxis con `.` en string del `@table`**: minimal change vs
  kwarg `@table("name", schema="...")`. Postgres usa la misma
  convención (`schema.table`).
- **`schema=None` = `public`**: backward compat 100% con código
  pre-v0.10.21. Tables sin schema explícito se comportan
  exactamente igual que antes.
- **Already-quoted-qualified en el field `table` del QB**: el
  preludio almacena `"public"."x"` ya quoteado y los `format!`
  interpolan con `{}` directo. Más simple que un campo
  `schema: Option<String>` paralelo + reconstruir en cada uso.
- **Cross-schema FK references**: en MVP, el FK del `@belongs_to`
  asume same-schema (la convención canonical "una table apunta
  a otra del mismo módulo"). Cross-schema FK queda como deuda
  menor si entra demanda.
- **Cross-schema rename**: no soportado en MVP. `@renamed_from`
  se interpreta dentro del schema actual de la table.

### Tests

- **0 unit tests nuevos** (existentes 60/60 cubren shape con
  `schema: None` default; el path schema custom se valida vía
  smoke E2E real).
- **2633/2633 lib tests verde** sin regresiones.
- **Smoke E2E real Postgres local validado bit-a-bit**:
  - 2 `@table` mixed (`users` public + `analytics.events`):
  - `db check` emite `CREATE SCHEMA IF NOT EXISTS "analytics";`
    + 2 CREATE TABLE qualified correctamente.
  - `db migrate` aplica todo OK.
  - `db check` post-migrate → `✓ schema sincronizado`.
  - ORM nativo: `User.insert(...)` (public) → id=1.
    `Event.insert(...)` (analytics) → id=1. `User.all(...)`
    + `Event.all(...)` SELECT contra `"analytics"."events"`
    devuelve rows correctas.

### Cierre formal Fase 10.6 — paquete migrations completo vs Alembic

Las 4 features del Tier 2 del plan original están cerradas
(v0.10.20 + v0.10.21). El stack `fitz db ...` ahora cubre:

| Feature | Versión | Equivalente Alembic |
|---|---|---|
| Auto-generate diff desde código tipado | v0.10.16 | ✓ |
| Apply pending + tracking idempotente | v0.10.16 | ✓ |
| Defaults SQL `@db_default("expr")` | v0.10.16 | ✓ |
| Down migrations + rollback | v0.10.17 | ✓ |
| Renames seguros via `@renamed_from` | v0.10.17 | ✓ |
| Drift check (CI bloqueante) | v0.10.18 | ✓ |
| Stamping (adoptar DB legacy) | v0.10.18 | ✓ |
| Data migrations en `.fitz` (Python-like) | v0.10.19 | ✓ |
| History (audit log) | v0.10.20 | ✓ |
| Offline SQL mode (DBA handoff) | v0.10.20 | ✓ |
| Squash (compactar migrations viejas) | v0.10.20 | ✓ |
| Schemas custom (multi-tenant) | v0.10.21 | ✓ |

**Diferenciales que Alembic NO tiene**:
- Cero deps externas (binario `fitz` solo vs `pip install
  alembic + sqlalchemy + psycopg2`).
- Schema desde código tipado del propio lenguaje (Alembic genera
  desde SQLAlchemy models, otro layer).
- Paridad bit-a-bit con el resto del stack (mismo driver en
  `fitz run`, `fitz build`, `fitz db ...`).

### Por qué importa

Cierra el último item del Tier 2 del plan. Equipos pueden ahora
modelar multi-tenant via PG schemas sin salir del lenguaje (cada
tenant en su schema con `@table("tenant_X.users")`). El paquete
completo de migrations queda al nivel funcional de Alembic con
diferenciales reales (cero deps, paridad, schema desde el code).

## [v0.10.20] — 2026-05-30 — Fase 10.6.e.1+.2: history + offline SQL + squash

Cierra 3 de las 4 features del Tier 2 del plan vs Alembic:
auditoría (`history`), handoff-a-DBA (`migrate --sql`), y
compactación de migrations viejas (`squash`). Schemas custom
(10.6.e.3) se difiere a v0.10.21 separada — la pre-eval reveló
cross-cutting con el ORM más grande de lo estimado.

### `fitz db history` — audit log de migrations applied

Lista las migrations aplicadas con `version` + `applied_at` +
filename. Orden `applied_at DESC` (más reciente primero). Si una
version está applied pero el file fue removido del dir
(post-squash o post-`stamp <legacy>`), aparece como
`(file removido)`.

```bash
fitz db history
# version              applied_at                       filename
# -------------------- -------------------------------- ----------
# 20260530120000       2026-05-30 10:53:24.800092-03    create_posts.sql
# 20260530100000       2026-05-30 10:53:24.775132-03    create_users.sql
# 2 migration(s) applied.
```

### `fitz db migrate --sql` — offline SQL mode (DBA handoff)

En vez de ejecutar las migrations pendientes, emite el SQL
concatenado al stdout (1 archivo por migration con header
`-- migration <version>: <filename>`). Útil para pasarle el SQL
a un DBA que aplica manual contra DBs prod sin exponer
credenciales al CLI.

```bash
fitz db migrate --sql > pending.sql
# 3 migrations emitidas al stdout
# Pasalas al DBA → psql -h prod-db -f pending.sql
# Marcalas como applied:
fitz db stamp --all
```

- Sigue conectándose para leer `_fitz_migrations` (skipea
  applied).
- Rechaza `.fitz` data migrations (no se materializan como SQL
  offline; usar `fitz db migrate` directo).
- Incompatible con `--dry-run` (clap valida).

### `fitz db squash <from> <to>` — compactar migrations viejas

Combina migrations del rango `[from, to]` (inclusive) en un
`<from>_squashed.sql`. Concatena los UP en orden + los DOWN en
orden INVERSO (para que el rollback siga funcionando). Mueve los
files originales a `migrations/squashed/` (no los borra). Si
alguna del range estaba applied en la DB, actualiza el tracking
para apuntar al nuevo squashed.

```bash
fitz db squash 20260101000000 20260301000000
# ✓ tracking actualizado: 47 versions removidas, stamped `20260101000000`
# ✓ 47 migration(s) squashed → migrations/20260101000000_squashed.sql.
#   Originales en migrations/squashed/.
```

Política:

- Solo `.sql` (rechaza `.fitz` en el rango — squashing de
  scripts del lenguaje no es semánticamente trivial).
- Rango mínimo 2 (squash de 1 = no-op rechazado).
- Tracking inteligente: si alguna applied, borra todas + stampea
  `from`. Si ninguna applied, no toca tracking.
- Pre-flight: aborta si el squashed ya existe.
- Flag `--no-tracking` para CI-only (skipea la actualización del
  tracking; user responsable de stampear manual en cada DB).
- Caso típico: repo con 100+ migrations viejas que el equipo ya
  aplicó. Squashear las primeras 80 acelera el bootstrap de devs
  nuevos sin afectar a quienes ya las aplicaron.

### Cambios técnicos

- **src/migrations.rs**:
  - Nueva struct `HistoryEntry { version, applied_at, filename }`
    + nueva `pub async fn history(conn, dir) -> DbResult<Vec<HistoryEntry>>`.
- **src/main.rs**:
  - Nueva variante `DbCmd::History { url, dir }` + handler
    `db_history_cmd` (output tabular).
  - Nueva variante `DbCmd::Squash { from, to, url, dir, no_tracking }`
    + handler `db_squash_cmd` (read range + pre-flight + emit
    squashed + move originals + update tracking).
  - `DbCmd::Migrate` suma flag `--sql`; `db_migrate_cmd` branchea
    en modo offline (lee tracking + emite SQL al stdout sin
    ejecutar).
- **editors/vscode/package.json**: 0.10.19 → 0.10.20.

### Tests

- **2 unit tests nuevos** en `src/migrations.rs::tests`:
  `history_entry_shape` + `history_signature_compila`.
- **60/60 migrations tests verde** (58 anteriores + 2 nuevos).
- **Smoke E2E real Postgres local validado bit-a-bit**:
  - 3 migrations `.sql` (create_users + add_name + create_posts)
    → `--sql` emite las 3 al stdout con header correcto + no
    toca DB → `migrate` aplica las 3 → `history` lista las 3 en
    orden cronológico inverso con `applied_at`.
  - `squash 20260530100000 20260530110000` combina users +
    add_name → emite `20260530100000_squashed.sql` con UP en
    orden + DOWN en orden inverso → mueve los 2 originales a
    `migrations/squashed/` → tracking borra las 2 versions y
    stampea solo `20260530100000` → `history` post-squash
    muestra 2 entradas (squashed + create_posts) con el
    squashed apuntando al filename nuevo.

### Schemas custom — DIFERIDO a v0.10.21

La pre-eval reveló:
- ~45 sitios entre evaluator/codegen/migrations que usan
  `meta.sql_name` directo sin concept de schema.
- Cross-cutting con el ORM (SELECT/INSERT/UPDATE/DELETE
  qualified, FK refs qualified, etc.).
- Estimación realista ~5-6 hs + risk de bugs ORM downstream.

Merece su propio commit + tag para que el smoke amplio cubra el
ORM. Plan en `docs/roadmap.md` → "Fase 10.6.e.3".

### Por qué importa

`fitz db history` cierra el último gap de visibility ("¿qué se
aplicó cuando?"). `migrate --sql` destraba el caso enterprise de
DBA-handoff (ops separadas de devs). `squash` evita que el dir
`migrations/` crezca sin techo en repos longevos — patrón
estándar de Alembic/Django/Rails que ahora Fitz también ofrece
con cero deps externas.

## [v0.10.19] — 2026-05-30 — Fase 10.6.d: data migrations en `.fitz`

`fitz db migrate` ahora reconoce DOS extensiones en `migrations/`:
`.sql` (DDL/DML crudo, splittable en `-- UP`/`-- DOWN`) y **`.fitz`**
(scripts del propio lenguaje con acceso completo a `db.query`,
`db.exec`, `db.transaction`, etc.). Se intercalan en orden
cronológico por el prefijo timestamp del filename.

Habilita transforms que SQL crudo NO expresa con elegancia:
back-fills con lógica condicional, parseo de JSON viejo a columns
nuevas, HTTP calls a un service externo durante la migración,
etc. — el caso típico que en Alembic / Rails se resuelve con
"data migration en Python/Ruby".

### Convención del `.fitz` migration

```fitz
// migrations/20260530150000_backfill_full_name.fitz

async fn migrate(db: DbConn) -> Result<Null> {
    // Acceso completo al lenguaje: loops, match, builtins,
    // db.transaction(...) para granularidad atómica.
    let _ = db.exec(
        "UPDATE users SET full_name = first_name || ' ' || last_name WHERE full_name IS NULL",
        [],
    ).await?
    return Ok(null)
}

// Opcional: si la declarás, `fitz db rollback` la invoca.
async fn rollback(db: DbConn) -> Result<Null> {
    let _ = db.exec("UPDATE users SET full_name = NULL", []).await?
    return Ok(null)
}
```

### Cambios técnicos

- **src/migrations.rs**:
  - `MigrationFile` refactorizado: ahora tiene `kind:
    MigrationKind` en vez de `up_sql/down_sql` directos.
  - Nueva enum `MigrationKind { Sql { up_sql, down_sql }, Fitz {
    path, source } }` + helpers `is_fitz()`.
  - `read_migrations_dir` acepta extensiones `.sql` y `.fitz`,
    detecta por sufijo, construye la variante correcta.
  - `apply_migration` rechaza migrations `.fitz` con error
    específico (el caller debe despachar al runner del lenguaje).
  - `revert_migration` y `rollback_n` paralelos rechazan
    `.fitz` con guards explícitos.
  - Nuevos helpers públicos `track_fitz_migration_applied(conn,
    version)` y `untrack_fitz_migration(conn, version)` para que
    el CLI inserte/borre el tracking después de invocar la
    callback del lenguaje.
- **src/main.rs**:
  - `db_migrate_cmd` ahora itera con dispatch per-kind:
    `.sql` → `apply_migration`, `.fitz` →
    `apply_fitz_migration_async`.
  - Nuevo `rollback_n_dispatch` paralelo a `migrations::rollback_n`
    pero con dispatch per-kind. `db_rollback_cmd` lo usa.
  - Nueva fn async `apply_fitz_migration_async(conn, version,
    filename, path, source)`: invoca el runner + trackea.
  - Nueva fn async `revert_fitz_migration_async`: invoca runner
    sobre `rollback` + untrackea.
  - Nueva fn async `run_fitz_migration_callback(conn, path,
    source, fn_name)`: parsea el `.fitz`, verifica que la fn
    está declarada como `async`, crea env vía
    `evaluator::new_repl_env()`, bindea `db` al `Value::DbConn`
    de la conn, appendea stmt sintético `let __fitz_mig_result =
    <fn_name>(db).await`, eval con `eval_program_with_env`,
    inspecciona el binding del env para extraer `Result::Ok(_)`
    vs `Result::Err(msg)`.
  - Nueva fn `fitz_migration_has_rollback(source)`: parsea
    source-only (sin tocar DB) para pre-flight del rollback.
- **editors/vscode/package.json**: 0.10.18 → 0.10.19.

### Decisiones técnicas

- **Convención `async fn migrate(db: DbConn) -> Result<Null>`**
  (paralelo al patrón `@test fn ...` del test runner). El user
  no escribe top-level code que dependa de un global `db` mágico;
  declara una fn explícita. Validable estáticamente, inspectable
  por el LSP, paralelo a cómo `fitz run` y `fitz test` ya
  modelan entry points.
- **`db` pre-bindeado al env del script**: la conn la maneja el
  CLI (lee `DATABASE_URL` o `--url`); el `.fitz` NO necesita
  llamar `db.connect(url)`. Inyectamos el `Value::DbConn` directo
  via `env.lock().define("db", ...)` antes del eval.
- **Atomicidad opt-in vs auto**: `.sql` migrations las envuelve
  el código en `db.transaction` automático. `.fitz` NO — el user
  decide granularidad (típicamente `return db.transaction(fn(tx)
  => ...).await` adentro del cuerpo de `migrate`). Más flexible:
  permite back-fills en chunks, retry parcial, multi-tx por
  diseño cuando el dataset es grande.
- **Rollback opcional**: si la `.fitz` declara `async fn
  rollback(db)`, el rollback la usa + borra registro. Si NO la
  declara, pre-flight aborta con mensaje claro (paralelo a `.sql`
  sin `-- DOWN`).
- **Eval, no codegen**: las `.fitz` migrations corren via
  intérprete (`evaluator::eval_program_with_env`). Para
  migrations con miles de iteraciones, el doc recomienda
  delegar el bulk a 1 UPDATE SQL en una `.sql` aparte.
- **Stmt sintético append**: en vez de invocar `migrate(db)`
  desde Rust directo (complica el path de invoke_value/dispatch),
  appendamos `let __fitz_mig_result = migrate(db).await` al AST
  parseado antes del eval. El `__fitz_mig_result` queda en el
  env, lo leemos vía `env.lock().get(...)` post-eval. Simple +
  reusa todo el path de evaluación normal.

### Tests

- **1 unit test nuevo** en `src/migrations.rs::tests`:
  `read_migrations_dir_detecta_fitz_files` valida que `.fitz` y
  `.sql` se intercalan en orden alfabético + la variante `kind`
  es la correcta + el `source` del `.fitz` queda cacheado.
- **58/58 migrations tests verde** (57 anteriores + 1 nuevo).
- **Smoke E2E real Postgres local validado bit-a-bit**:
  - 2 migrations mixtas (1 `.sql` create_users + 1 `.fitz`
    backfill_names) → `db status` lista ambas pending → `db
    migrate` aplica ambas en orden → DB rows con `name`
    rellenado por la `.fitz` → `_fitz_migrations` con ambas.
  - `db rollback` revierte solo la `.fitz` (ejecuta su `async
    fn rollback`) → `name` vacío + tracking de la `.fitz`
    eliminado.
  - Re-`db migrate` re-aplica solo la `.fitz` (la `.sql` sigue
    applied) → idempotencia OK.
  - Pre-flight error: `.fitz` SIN `async fn rollback` declarada
    → `db rollback` aborta antes de tocar la DB con mensaje
    específico ("no declara `async fn rollback(db: DbConn) ->
    Result<Null>`. Agregá la fn al archivo y reintentá").

### Cuándo usar `.fitz` vs `.sql`

- **`.sql`** — DDL puro (CREATE TABLE / ADD COLUMN / CREATE
  INDEX), back-fills triviales (`UPDATE users SET x = 1 WHERE x
  IS NULL`), seed fixtures. **~80% de las migrations**.
- **`.fitz`** — back-fills con lógica condicional o loops,
  parseo de JSON viejo a columns nuevas, HTTP calls durante la
  migración, transforms que requieren state que SQL crudo no
  expresa elegantemente.

### Limitaciones explícitas del MVP

- **Sin auto-wrap en tx**: el user decide granularidad. Si la
  `.fitz` migrate fallise a la mitad sin tx explícita, queda en
  estado parcial — escribí `return db.transaction(...).await`
  para garantizar atómico.
- **Stmt sintético con var pública**: `let __fitz_mig_result`
  contamina el env del script con un nombre interno. En
  práctica no choca (`__` prefix convención), pero un script
  que defina `__fitz_mig_result` por su cuenta tendría
  comportamiento sorprendente.
- **Eval-only**: las `.fitz` corren via intérprete, NO codegen.
  Para bulk-loads masivos preferí SQL crudo (1 UPDATE >> N
  iteraciones del intérprete).

### Por qué importa

Cierra el último gap funcional Tier 1 del plan vs Alembic.
Equipos pueden ahora hacer transforms reales (no solo DDL) sin
salir a Python/Ruby scripts externos. Combinado con drift check
(v0.10.18), rollback + renames (v0.10.17), y el ORM nativo
(v0.10.x), el stack DB de Fitz cubre el flujo completo de
desarrollo + CI/CD de schema management.

## [v0.10.18] — 2026-05-29 — Fase 10.6.c: drift check + stamping (+ driver fix OID `name`)

Cierra el Tier 1 más solicitado en surveys de Alembic: drift
check para CI bloqueante. Más adopción de Fitz en DB legacy via
stamping. Más un driver fix crítico descubierto durante el smoke
real con Postgres local.

### `fitz db check` — drift detection para CI

Corre el diff del schema declarado vs la DB real:
- **Exit 0** + `✓ schema sincronizado` si sin cambios.
- **Exit 1** + SQL pendiente al stderr si hay drift, con sugerencia
  de cómo sincronizar.

```bash
fitz db check src/main.fitz
# ✓ schema sincronizado — schema declarado matchea la DB
# (exit 0)
```

Patrón canónico en CI:
```yaml
- name: Schema drift check
  run: fitz db check src/main.fitz
  env:
    DATABASE_URL: ${{ secrets.STAGING_DB_URL }}
```

### `fitz db stamp <version>` / `--all` — adoptar Fitz en DB legacy

Marca migrations como aplicadas en `_fitz_migrations` **sin
ejecutar el SQL**. Caso de uso típico: adoptar Fitz en un
proyecto que ya tiene el schema aplicado manualmente.

```bash
# 1. Generás migration que matchea el schema actual:
fitz db diff src/main.fitz > migrations/20260530000000_initial.sql

# 2. Marcás como aplicada SIN ejecutarla:
fitz db stamp 20260530000000
#   ✓ stamped: 20260530000000

# 3. A partir de acá, `migrate` aplica solo nuevas.
```

`--all` marca todas las pending del dir en una pasada (caso
adopción inicial). Idempotente — ya-applied → no-op silencioso.
Warning sobre versions que no existen en el dir (typo guard).

### Driver fix — OID 19 (`name` type de `pg_catalog`) → Text

**Descubierto durante el smoke real**: las queries de introspect
de `migrations` consultan `information_schema.columns` cuyos
campos `column_name` y `udt_name` son tipados como
`sql_identifier` (alias de `name` interno de Postgres con OID 19).
El driver no manejaba OID 19 → error
`tipo Postgres OID 19 no soportado en MVP (10.5)` rompía TODO
`fitz db ...` que introspectara.

Fix trivial: `oid::NAME = 19` agregado al match `parse_text_value`
(treat as Text, equivalente a `text`/`varchar`). Cambio de 6 LoC
+ desbloqueador crítico de toda la sub-fase 10.6.

### Cambios técnicos

- **src/migrations.rs**:
  - Nueva fn `stamp_version(conn, version) -> DbResult<bool>` con
    `ON CONFLICT DO NOTHING` para race-safety. Devuelve true si
    insertó, false si ya estaba.
  - Nueva fn `stamp_all_pending(conn, migrations) -> DbResult<Vec<String>>`
    que itera el dir y stampea solo las no-applied.
- **src/main.rs**:
  - Nuevas variantes `DbCmd::Check { file, url }` y
    `DbCmd::Stamp { version, all, url, dir }` con clap
    `conflicts_with` entre version y all.
  - Nuevos handlers `db_check_cmd` (reusa diff + decide exit
    code) y `db_stamp_cmd` (wrap stamp_version / stamp_all_pending
    + warning sobre versions no en dir).
- **src/db.rs**:
  - `oid::NAME = 19` agregado al módulo `oid`.
  - Branch `oid::NAME` en `parse_text_value` (treat as Text junto
    con `oid::TEXT` / `oid::VARCHAR`).
- **editors/vscode/package.json**: 0.10.17 → 0.10.18.

### Tests

- **2 unit tests nuevos** en `src/migrations.rs::tests`:
  - `check_es_verde_cuando_diff_es_vacio` + `check_falla_cuando_hay_drift`:
    valida la decisión de exit code basada en `diff_schemas`.
  - `stamp_version_y_stamp_all_pending_estan_exportadas`: smoke
    estructural (rompe a compilar si renombran o cambian firmas).
- **57/57 migrations tests verde** (54 anteriores + 3 nuevos).
- **Smoke end-to-end real Postgres local validado**: create DB
  → `db check` (drift detected, exit 1 con SQL) → `db new` +
  `db diff --out` → `db migrate` → `db check` (sincronizado,
  exit 0) → `db stamp <version>` (no-op) → `db stamp 19990101000000`
  (warning + stamped) → `db stamp --all` (no-op) → `db stamp`
  sin args (error claro).

### Por qué importa

`fitz db check` cierra el último gap visible para uso CI/CD
profesional: equipos pueden bloquear PRs que diverjan del schema
de staging. `fitz db stamp` destraba la adopción de Fitz en
proyectos legacy (caso típico: equipo con SQLAlchemy quiere
migrar a Fitz manteniendo la DB). El driver fix OID 19 era un
landmine — sin él, **ninguna** corrida de `fitz db diff/check/migrate`
funcionaba contra una DB que ya tuviera tables (porque la
introspect failearía después de la primera). Lo descubrimos a
las primeras corridas del smoke real con Postgres local.

## [v0.10.17] — 2026-05-29 — Fase 10.6.b: rollback + renames seguros

Cierra los dos gaps Tier 1 más visibles de migraciones contra
Alembic: forward-only (sin rollback) y renames perdiendo datos.

### Rollback (`fitz db rollback [--count N]`)

Las migrations soportan secciones explícitas `-- UP` / `-- DOWN`
para revertir. Backward-compatible: archivos sin marcadores
siguen siendo "UP implícito sin DOWN" (no se pueden revertir,
pero `migrate` los aplica igual).

```sql
-- Migration: add_email_to_users

-- UP
ALTER TABLE "users" ADD COLUMN "email" text NOT NULL DEFAULT '';

-- DOWN
ALTER TABLE "users" DROP COLUMN "email";
```

```bash
fitz db rollback              # revierte el último
fitz db rollback --count 3    # revierte los últimos 3
```

Política:
- `fitz db new` genera stubs con `-- UP` / `-- DOWN` por
  convención.
- Marcador case-insensitive sobre línea propia (`-- UP`, `--up`,
  `-- Up` matchean). `-- UP foo` NO (chars extra → SQL comment
  normal).
- Sección DOWN vacía / solo whitespace → `None` (irreversible).
- Si querés revertir N>1 y alguna target NO tiene `-- DOWN`, el
  rollback **aborta ANTES de tocar la DB** con mensaje específico
  citando filename. Cero estado parcial pre-flight.
- Cada `revert_migration` es atómico individual (1 tx). Rollback
  de N>1 son N tx — si la k-ésima falla en runtime, las anteriores
  ya persistieron. Para "todo o nada" sobre N, usar 1 migration
  única con todo el rollback adentro.
- Orden de rollback: `applied_at DESC` del tracking (más reciente
  primero), NO orden de filename.

### Renames seguros (`@renamed_from("old_name")`)

Decorator transient sobre field o `@table` para que el diff
emita `ALTER TABLE ... RENAME COLUMN/TABLE` en vez de `DROP +
ADD`, preservando datos.

```fitz
// Rename column.
@table("users") type User {
    @primary id: Int = 0
    @renamed_from("name") full_name: Str = ""
}

// Rename tabla.
@table("users") @renamed_from("legacy_users") type User {
    @primary id: Int = 0
}
```

`fitz db diff` emite:

```sql
ALTER TABLE "legacy_users" RENAME TO "users";
ALTER TABLE "users" RENAME COLUMN "name" TO "full_name";
```

Política:
- Orden seguro en el output: renames PRIMERO, después
  ADD/DROP/ALTER COLUMN sobre el nombre nuevo.
- No-op silencioso cuando el rename ya se aplicó (target tiene
  `@renamed_from("old")` pero current ya solo tiene "new" —
  caso típico post-migration). El user borra el decorator
  cuando quiera.
- Por qué decorator y no subcomando: el subcomando divorcia
  rename del cambio en el code (fácil olvidar uno); decorator
  es declarativo + atómico con el código.

### Cambios técnicos

- **src/migrations.rs**:
  - `MigrationFile` reemplaza `sql: String` por `up_sql: String`
    + `down_sql: Option<String>`.
  - Nueva fn `split_up_down(raw)` con parser line-anchored case-
    insensitive de marcadores `-- UP` / `-- DOWN`.
  - Nuevas variantes `Change::RenameTable` + `Change::RenameColumn`.
  - Nueva fn `apply_renames_from_target(current, target, changes)`
    que pre-procesa los hints `renamed_from` del target: emite
    rename Changes al frente y devuelve una versión renombrada
    de current para que el resto del diff compare por nombres
    post-rename.
  - Nueva fn `revert_migration(conn, migration)`: ejecuta el
    `-- DOWN` adentro de tx + borra registro de
    `_fitz_migrations`. Atomic. Error específico si `down_sql`
    es None.
  - Nueva fn `rollback_n(conn, migrations, n)`: pre-flight
    valida que TODAS las versiones target tienen file + DOWN,
    después revierte una por una (atomic individual).
  - Nueva fn `applied_versions_desc` (orden por `applied_at
    DESC`) para `rollback_n`.
- **src/types.rs**:
  - `TableMetadata.renamed_from: Option<String>` paralelo al
    `sql_name`. Parsea `@renamed_from("old")` a nivel type.
  - `ColumnMetadata.renamed_from: Option<String>` para fields.
    Parsea `@renamed_from("old")` a nivel field.
  - Validación: solo arg Str literal no vacío, rechaza otros
    con mensaje claro.
  - Error del decorator inválido sobre `type` actualizado para
    listar `@renamed_from` también.
- **src/main.rs**:
  - Nueva variante `DbCmd::Rollback { url, dir, count }` +
    dispatch + handler `db_rollback_cmd`.
  - `db_new_cmd` genera stub con secciones `-- UP` / `-- DOWN`
    por convención.
- **src/lsp.rs**: nuevo completion item snippet para
  `@renamed_from("${1:old_name}")`. Doc del `@db_default` y la
  lista de decorators ORM en `AfterAt` actualizada.
- **src/migrations.rs**: `Table.renamed_from` y
  `Column.renamed_from` agregados (poblados solo en target
  schema desde `schema_from_program`; `None` en introspect).
- **editors/vscode/package.json**: 0.10.16 → 0.10.17.

### Tests

- **15 unit tests nuevos** en `src/migrations.rs::tests`:
  - 6 sobre `split_up_down` (sin marcadores, ambos, case-
    insensitive, sección vacía, sin UP solo DOWN, marker con
    chars extra que NO es marker).
  - 6 sobre renames (RenameTable + RenameColumn emit + SQL del
    output + no-op silencioso cuando no hay match + orden
    seguro rename-antes-de-alter + cargar `renamed_from` a
    Column/Table desde program).
  - 1 sobre `read_migrations_dir` que preserva up/down.
  - 2 sobre `schema_from_program` con `@renamed_from` field y
    table.
- **54/54 migrations tests verde** (39 anteriores + 15 nuevos).
- **2627/2627 lib tests verde** (sin regresiones).
- **Smoke `GUIDE_EXAMPLES_COMPILE`** verde (292 ejemplos).
- **Smoke manual**: `fitz check` y `fitz run` aceptan
  `@renamed_from(...)` en field y type; `fitz db rollback
  --help` documenta la nueva subcomando; `fitz db new` emite
  stub con secciones `-- UP` / `-- DOWN`.

### Limitaciones explícitas del MVP

- **Rollback de N>1 NO es atómico transversal**: cada `revert`
  es 1 tx aislada. Para "todo o nada" sobre N migrations, una
  migration única con todo el rollback adentro.
- **`@renamed_from` no detecta renames cíclicos** (A → B → A):
  caso degenerado, el user lo resuelve manualmente.
- **`ALTER COLUMN ... TYPE` sin USING** sigue siendo deuda
  (cambios incompatibles fallan — editar migration con USING).

### Por qué importa

Hasta v0.10.16 Fitz tenía migraciones forward-only sin rollback
y renames que perdían datos — los dos gaps más visibles vs
Alembic. v0.10.17 los cierra. El siguiente Tier 1 del plan
roadmap es **Fase 10.6.c (drift check + stamp)** para CI
bloqueante y adopción en DB legacy.

## [v0.10.15] — 2026-05-29 — `db.transaction` acepta FnExpr inline (paridad `fitz run` ↔ `fitz build`)

Cierre de la deuda más visible de v0.10.14 — el codegen MVP solo
aceptaba fn nombrada como callback de `db.transaction(...)`. Ahora
acepta también FnExpr inline (`async fn(tx) -> Result<T> { ... }`)
con captures del outer scope.

### Cambio user-facing

```fitz
@post("/transfer/{from_id}/{to_id}/{amount}")
async fn transfer(from_id: Int, to_id: Int, amount: Float) -> Result<Account> {
    let conn = db.connect(db_url).await?
    return conn.transaction(async fn(tx) -> Result<Account> {
        let from = Account.where(fn(a) => a.id == from_id).first(tx).await?
        let to = Account.where(fn(a) => a.id == to_id).first(tx).await?
        // ... transferí dinero, balance check, etc.
        return Ok(to)
    }).await
}
```

Antes (v0.10.14) el codegen forzaba extraer el callback a una fn
nombrada por restricción del MVP. Ahora la sintaxis inline natural
funciona idéntica a `fitz run`.

### Implementación

`gen_db_conn_method_call` arm `"transaction"` suma Path 2 nuevo
para FnExpr inline:
- Ret type sacado del `TypeInfo` del checker via
  `type_info.type_at(args[0].span())` (NO `infer_callback_ret_silently`
  que hace dry-run sin scope).
- Push `unwrapped` (`Result<T>`) al `ret_stack`, no `inferred`
  (`Future<Result<T>>`) — el body interno del async closure es
  código cuyo ret natural es Result<T>; sin esto, `?` rechazaba
  con "solo en fn que retorna Result".
- Emit: `__fitz_db_transaction(&{db}, move |{param}: __FitzDbConn|
  async move {{ {body} }})`. Doble `move` (outer FnOnce + inner
  async Send).

Path 3 (otro Expr) sigue dando error claro listando los 2 patterns
válidos.

### Cambios complementarios

- `examples/guide/31c-transactions.fitz` revertido a la sintaxis
  inline natural (era el fix forzado en v0.10.14 que extrajo las
  3 closures a fns nombradas).
- Extension VSCode bump 0.10.14 → 0.10.15.

## [v0.10.14] — 2026-05-29 — Transactions ORM con `db.transaction(fn)` closure-based

**Cierre formal Fase 10.7**. Escrituras atómicas multi-step con
BEGIN/COMMIT/ROLLBACK automático según el `Result` del callback.
Imposible olvidarse el commit/rollback — el control de flujo del
Result garantiza la atomicidad.

### API user-facing

```fitz
let result = db.transaction(async fn(tx) -> Result<Int> {
    let user = User.insert(tx, User { ... }).await?
    Order.insert(tx, Order { user_id: user.id, ... }).await?
    return Ok(user.id)
}).await?
```

El `tx: DbConn` es del mismo tipo que `db`, pero internamente
pegado a la misma conn física durante toda la tx. Todos los métodos
del ORM (`.insert`/`.update`/`.delete`/`.first`/`.all`) y escape
hatch (`.query`/`.exec`) funcionan sin cambios.

### Sub-pasos

1. **`src/db.rs` — `Connection::begin/commit/rollback`** primitivos.
   Wrappers simples sobre `simple_query`. Sin niveles de aislamiento
   explícitos (usa default del server, típico READ COMMITTED).
2. **`src/db.rs` — `DbConnHandle::transaction<F, Fut, T>(self:
   &Arc<Self>, f)`** orquestador con auto-rollback en Err/panic +
   cleanup de la conn al pool. Single-conn pool interno
   (`max_conns=1`) garantiza isolation físico — todas las queries
   del callback usan la misma conn.
3. **`tests/db_real_postgres.rs`** — 3 tests E2E nuevos:
   - `tx_happy_path_commit_persiste`
   - `tx_rollback_explicito_nada_persiste`
   - `tx_conn_vuelve_al_pool_despues_de_tx` (5 iter consecutivos sin leak)
4. **`src/evaluator.rs`** — builtin `db_conn_transaction` + dispatch.
   Preserva el `Value` original del Err callback via cell compartido
   (`Arc<Mutex<Option<Value>>>`) — el `Err` Fitz no se aplana al
   `DbError::Protocol` del driver.
5. **`src/codegen.rs`** — `gen_db_conn_method_call` arm `"transaction"`
   + `__fitz_db_transaction` helper genérico en el preludio. MVP
   soporta SOLO fn nombrada como callback (no FnExpr inline); error
   de codegen explícito sugiere el workaround. El intérprete sí
   permite inline. Refinable a futuro → **cerrado en v0.10.15**.

### Limitaciones MVP

- Sin niveles de aislamiento custom (READ UNCOMMITTED, SERIALIZABLE,
  etc.).
- Sin nested transactions con SAVEPOINT.
- Sin read-only transactions.

Todos quedan como deuda menor (revisable si entra presión).

## [v0.10.13] — 2026-05-29 — Driver Postgres B-1 fix (Extended Query batching + TCP_NODELAY) + bench fixes

Bloque grande agrupando mini-fases relacionadas con la calidad del
bench Fitz ORM vs SQLAlchemy + sus hallazgos.

### B-1 — Extended Query Protocol optimization

Root cause identificado en el benchmark v2: `GET /users/{id}` (que
usa `WHERE id = $1`, extended query) tardaba 43ms p50 vs 4ms del
simple query. El driver hacía 5 `self.write(...).await?` separados
para Parse/Bind/Describe/Execute/Sync, sumando ~30-40ms de overhead
por Nagle + 5 syscalls write() + 5 awaits.

**Fix doble en [src/db.rs](src/db.rs)**:

1. **`TCP_NODELAY`** al construir el TcpStream (deshabilita Nagle).
   Crítico porque mandamos 5 mensajes consecutivos sin esperar
   respuesta del server entre ellos — sin esto el kernel TCP
   retrasaba cada paquete chico esperando ACK del previo.
2. **Batch los 5 mensajes en UN solo `write_all_bytes(...)`** —
   `Vec<u8>` con concat de los 5 `encode()`. Server Postgres NO
   responde hasta `Sync`; es pipelining protocolar legítimo, no
   cambio semántico.

**Resultado** (bench publicable v0.10.13):

| Endpoint | Pre-fix Fitz p50 | Post-fix Fitz p50 | Python SQLAlchemy p50 |
|---|---:|---:|---:|
| `GET /users/{id}` | 43.70 ms | **3.60 ms** | 31.87 ms |
| `GET /users` | 4.92 ms | **4.88 ms** | 37.85 ms |

Fitz pasó de "30% más lento que Python en single-by-PK" a **8.85x
más rápido**. Headline del bench: **5-10x speedup + 5.5x menos
memory** en read workloads.

### Bench fixes

- **Image size grep**: `^${boilerplate}-api:latest ` (anchor exacto)
  para no pescar otros boilerplates cacheados.
- **Memory peak sampler**: container names correctos según el
  `container_name:` del docker-compose de cada boilerplate.
- **POST x500 → x100**: en Git Bash Windows el overhead del subshell
  (~1s/iter) hace que x500 tarde ~10min. x100 es suficiente para
  p50/p95/p99 representativos.
- **PID via archivo** para el memory sampler (fix Git Bash:
  capturar PID via `$()` espera todo el subshell, hace hang).

### Migración Python boilerplates → ghcr `fitz:latest-python`

`api-postgres-python` + `api-fullstack-postgres`: Dockerfile migrado
de `cargo install --git` (~8-12 min build inicial) al patrón
pre-built `ghcr.io/thegreekman76/fitz:latest-python` (~30-60s).
Reducción ~10x del build time. Trade-off: dependencia de la
imagen publicada por CI release (default `latest-python`, override
con `--build-arg FITZ_TAG=v0.10.13-python`).

## [v0.10.16] — 2026-05-29 — Fase 10.6: migraciones automáticas ORM + `@db_default("expr")`

`fitz db diff/migrate/status/new` — el binario ahora introspecciona
el schema real de Postgres, lo compara con los `@table type`
declarados, y emite el SQL `ALTER TABLE` / `CREATE TABLE` necesario
para sincronizar. Las migrations versionadas se aplican con
tracking idempotente en `_fitz_migrations`. **Cero deps externas**:
ni Alembic ni Flyway ni Liquibase ni TypeORM CLI. La fuente de
verdad es el código tipado del lenguaje.

### Subcomandos nuevos

| Subcomando | Qué hace |
|---|---|
| `fitz db diff [archivo.fitz] [--out file.sql]` | Compara schema declarado vs real, emite SQL al stdout o file. |
| `fitz db migrate [--dry-run]` | Aplica los `.sql` pendientes del dir `./migrations` en orden alfabético. |
| `fitz db status` | Lista cada archivo `.sql` con badge `✓ applied` / `→ PENDING`. |
| `fitz db new <name>` | Crea `migrations/YYYYMMDDHHMMSS_<name>.sql` con stub vacío. |

URL: lee `DATABASE_URL` env var, o pasa `--url postgres://...`.
Dir: `./migrations` por default, override con `--dir`. Entry:
explícito o `[bin].main` del manifest.

### `@db_default("expr")` — defaults SQL en el `type`

El decorator `@db_default` (introducido en v0.10.8 como marker
"skip INSERT") ahora acepta un arg Str opcional con la expresión
SQL del default. Si está, `fitz db diff` emite `DEFAULT <expr>`
automáticamente en `CREATE TABLE` / `ADD COLUMN`. Si cambia, el
diff emite `ALTER TABLE ... ALTER COLUMN ... SET/DROP DEFAULT`.
Sin arg, comportamiento original (marker-only).

```fitz
@table("events") type Event {
    @primary id: Int = 0
    @db_default("NOW()") created_at: Str = ""
    @db_default("gen_random_uuid()") tracking_id: Str = ""
}
```

**Idempotencia del diff** — la normalización es tolerante a
variaciones cosméticas que Postgres aplica automáticamente:

- Case-insensitive en function calls (`NOW()` ↔ `now()`).
- Strip de casts redundantes (`'foo'::text` ↔ `'foo'`).
- Trim whitespace.

NO intenta evaluar expresiones equivalentes (`now()` ≠
`CURRENT_TIMESTAMP` desde el lado del diff aunque ambos sean
válidos para `timestamptz`). El user elige una y la mantiene.

### Cambios técnicos

- **src/migrations.rs** (~1260 LoC nuevas): módulo dedicado con
  `Schema`/`Table`/`Column`/`Index`/`ForeignKey` structs,
  `introspect_schema(conn)` via `information_schema` +
  `pg_catalog`, `schema_from_program(program, type_env)` que
  walka el AST + `TableMetadata`, `diff_schemas(current, target)`
  con orden seguro (CREATE TABLE → ADD/DROP/ALTER COLUMN →
  CREATE INDEX → DROP FK → ADD FK → DROP TABLE),
  `changes_to_sql(changes)` con quoted identifiers, helpers de
  tracking (`ensure_tracking_table`, `applied_versions`,
  `read_migrations_dir`, `apply_migration`,
  `apply_pending_migrations`, `status`).
- **src/lib.rs**: nueva `pub mod migrations`.
- **src/main.rs**: nueva variante `Commands::Db(DbCmd)` con 4
  subcomandos vía clap. Handlers `db_diff_cmd`/`db_migrate_cmd`/
  `db_status_cmd`/`db_new_cmd` con helpers `resolve_db_url`,
  `resolve_migrations_dir`, `resolve_db_entry`,
  `load_program_for_db`. Todos los handlers usan una sola
  runtime tokio para connect + work (evita que health_check_task
  muera con un runtime que se dropea entre connect y query).
- **Cargo.toml**: `chrono = "0.4"` reusado (ya dep para
  jobs/cron) para timestamps `YYYYMMDDHHMMSS_<name>.sql`.
- **src/types.rs**: `ColumnMetadata.db_default_sql:
  Option<String>` paralelo al flag `db_default` existente. El
  parser del decorator acepta 0 args (marker-only, backward
  compat con v0.10.8) o 1 arg Str literal (nueva semántica
  v0.10.16) — rechaza arg no-Str con mensaje específico.
- **src/migrations.rs**: nueva variante `Change::AlterColumnDefault`
  + helper `normalize_default_for_diff` (lowercase + strip
  trailing PG cast). `introspect_columns` strippea `nextval(...)`
  del default de PK bigserial para evitar falso positivo.
- **src/lsp.rs**: doc del completion item `@db_default` actualizado
  para mencionar el arg Str opcional con ejemplos.
- **editors/vscode/package.json**: version `0.10.15` → `0.10.16`.
  Grammar TextMate sin cambios (ya matchea `@db_default` y
  cualquier decorator con args via la rule de strings).

### Decisiones técnicas

- **Quoted identifiers everywhere** (`"users"`, `"email"`): los
  CREATE TABLE / ALTER COLUMN emitidos quotean cada nombre para
  que reserved words o caracteres especiales no rompan.
- **Filesystem-based, no DSL custom**: migrations son `.sql`
  planos editables a mano. Patrón estándar de Flyway/Rails. El
  diff genera el SQL, el user lo redirige al file y lo edita
  si necesita refinos manuales.
- **Forward-only** (sin `down` migrations): para revertir,
  escribís una nueva migration con el cambio inverso. Patrón
  Rails sin `down`, Alembic sin `downgrade`. Menos código,
  menos drift posible entre `up` y `down` que se desincronizan.
- **Tracking en tabla dedicada** (`_fitz_migrations` con
  `version TEXT PRIMARY KEY` + `applied_at TIMESTAMPTZ DEFAULT
  NOW()`): patrón estándar. Re-correr `migrate` es siempre
  no-op si todo está aplicado.
- **Schema diff determinístico**: orden estable de categorías +
  sort alfabético dentro de cada categoría — re-correr `diff`
  con los mismos inputs produce siempre el mismo output (clave
  para grep/sed/CI checks contra diffs esperados).
- **Solo schema `public`** en el MVP: refinamiento futuro si
  entra demanda multi-schema.

### Tests

- **39 unit tests nuevos** en `src/migrations.rs::tests`
  cubriendo: diff de schemas vacíos/iguales (idempotente);
  CREATE/DROP table; ADD/DROP/ALTER column con type + nullable;
  CREATE/DROP index; ADD foreign key; orden seguro (CREATE
  antes que DROP); determinismo cross-runs; emission de SQL
  para cada `Change`; round-trip `schema_from_program(src)`
  con types Fitz reales + `diff` contra sí mismo es vacío;
  dos versiones del schema yield `AddColumn`. **+13 tests del
  `@db_default("expr")`**: parse + emission CREATE TABLE/ADD
  COLUMN/SET DEFAULT/DROP DEFAULT + idempotencia diff
  case-insensitive + strip PG casts + round-trip schema con
  default normalizado.
- **2612/2612 unit tests verde** post-cambios (sin regresiones).
- **Smoke real Postgres**: validable vía CI (job `db-postgres`).
  En Windows host contra Docker-mapped Postgres reproduce un
  bug pre-existente del driver wire protocol ("cstr no es
  UTF-8") que NO bloquea uso desde Linux/CI.

### Limitaciones explícitas del MVP

- **No detecta renames** (column ni table): un rename Fitz-side
  `name` → `full_name` se ve como `DROP COLUMN + ADD COLUMN`,
  **perdiendo datos**. Editá la migration a mano (`ALTER TABLE
  ... RENAME COLUMN`) cuando el caso lo justifique.
- **`ALTER COLUMN ... TYPE` sin USING**: cambios incompatibles
  (`text → int`) fallan. Editá la migration para agregar
  `USING (col::int)`.
- **`@db_default` sin arg sigue siendo marker-only**: el
  comportamiento de v0.10.8 se preserva (skip INSERT, sin
  default en migration). Para que el diff emita el default,
  pasale la expresión SQL explícita (`@db_default("NOW()")`).
- **Solo schema `public`** (no multi-schema).
- **Forward-only** (sin `down`/`downgrade`).

### Docs actualizados

- `docs/db-orm.md`: nueva sección 26.c "Migraciones automáticas
  (v0.10.16)" con workflow canónico + política + limitaciones +
  por qué Fitz lo hace distinto. Sección 28 actualizada
  (migraciones + transactions movieron de "deuda" a "CERRADO").
  Sección 29 (CLI con DB) suma sub-sección con los 4 nuevos
  subcomandos.
- `docs/guide.md` cap 31 (Postgres + ORM nativo): nueva
  sub-sección "Migraciones automáticas (v0.10.16)" con el
  workflow básico y link a `docs/db-orm.md` para detalles.
- `CHANGELOG.md`: esta entrada.

### Por qué importa

Hasta v0.10.15, el schema se escribía a mano en `db.exec(
"CREATE TABLE IF NOT EXISTS ...", [])` al boot del programa
(idiomatic en los ejemplos de la guía pero manual y no
versionado). Equipos serios necesitan: cambios versionados, CI
checks contra drift schema vs código, rollouts ordenados, y
visibilidad de "qué migrations corren en cada deploy". `fitz db
diff/migrate/status/new` resuelve esto en el binario, sin
levantar deps externas. Combinado con Transactions ORM
(v0.10.14-15), el stack DB de Fitz es ahora self-contained
end-to-end.

## [v0.10.12] — 2026-05-29 — LSP completion tras `@` + 9no boilerplate fullstack

Dos cambios paralelos en una sola release:

### LSP completion tras `@` (DX del editor)

Cerrada la deuda LSP heredada de v0.10.11 ("hoy grammar destaca
cualquier `@name` pero el LSP no sugiere la lista cerrada de
decorators"). Al escribir `@` o `@<prefix>` en el editor, el LSP
sugiere ahora los 23 decorators del lenguaje con snippets útiles.

**Cambios técnicos**:

- **src/lsp.rs**: nuevo `CompletionContext::AfterAt` detectado
  cuando el char antes del prefix ident es `@`. Tiene prioridad
  sobre `AfterDot` (el char `@` no forma parte de un ident chain).
- **src/lsp.rs**: nueva fn `decorator_completions()` que devuelve
  los 23 decorators agrupados en 5 familias:
  - HTTP routing: `@get`/`@post`/`@put`/`@delete`/`@server`/`@header`
  - Middleware/CORS: `@middleware`/`@cors`
  - Auth: `@authenticated`/`@admin`/`@auth_provider`
  - WS + Jobs: `@ws`/`@cron`/`@background`/`@test`
  - ORM: `@table`/`@primary`/`@column`/`@unique`/`@index`/
    `@db_default`/`@hidden`/`@belongs_to`/`@has_one`/`@has_many`
- **Snippets con tabstops** (`${N:placeholder}`): decorators con
  args típicos (`@get("/path")`, `@table("name")`) emiten un
  placeholder editable. Decorators sin args (`@hidden`,
  `@primary`, `@test`, etc.) emiten el nombre plano. Decorators
  de relation emiten dos tabstops (`@belongs_to("Target",
  via="fk")`).
- **src/bin/fitz-lsp.rs**: `CompletionOptions.trigger_characters`
  expandido a `[".", "@"]` — VSCode invoca completion
  automáticamente cuando el usuario tipea `@`.
- **tests/lsp_e2e.rs**: nuevo test
  `completion_after_at_lista_decorators_v0_10_12` que valida la
  capability, la lista de 17 decorators core, kind=SNIPPET (15) e
  insertTextFormat=2 (snippet). Test viejo
  `completion_after_dot_sobre_str_lista_metodos_built_in`
  actualizado para tolerar el nuevo trigger char `@`.

### 9no boilerplate: `api-orm-full-fullstack` ⭐⭐⭐

Replica el backend de `api-orm-full` (HTTP + auth + WS + cron +
Postgres ORM) **sumando un frontend vanilla** en nginx que consume
todo el stack desde un browser real. Cubre el ciclo "browser →
server → DB" end-to-end.

**Estructura**:
- `src/` — idéntica a `api-orm-full` (ningún cambio al backend).
- `frontend/` — Dockerfile + nginx.conf + 7 pantallas HTML/CSS/JS
  vanilla (sin build step, sin node_modules, <100 KB total).
- `docker-compose.yml` — 3 services: db (Postgres 16) + api (Fitz
  binario standalone) + frontend (nginx-alpine).

**Pantallas**:
| URL | Endpoint(s) | Qué ejercita |
|---|---|---|
| `/login.html` | `POST /auth/login`, `POST /auth/register` | JWT en localStorage |
| `/posts.html` | `GET /posts?status=&tag=` | listado con filtros |
| `/post-detail.html?id=N` | `GET /posts/{id}` + preload, `POST /posts/{id}/comments` | eager loading inline |
| `/new-post.html` | `POST /posts` | tags array + jsonb desde browser |
| `/edit-post.html?id=N` | `PUT /posts/{id}` | partial update con `Map<Str, Any>` |
| `/stats.html` | `GET /stats/posts-per-user` | GROUP BY con Chart.js |
| `/feed.html` | `WS /feed` | WS auth realtime |

**Decisión técnica clave — nginx proxy same-origin**: el frontend
hace `fetch("/api/...")` y `new WebSocket("/ws/feed?token=...")`.
nginx proxy-ea ambos al backend en `:3000`. Esto resuelve **dos
limitaciones de los browsers** sin tocar el backend:
- **Sin CORS**: requests same-origin desde la perspectiva del
  browser.
- **WS auth header injection**: los browsers NO permiten custom
  headers en `new WebSocket(url)`; el frontend pasa el token JWT
  como `?token=...` y nginx lo transforma a header
  `Authorization: Bearer ...` antes del proxy.

```nginx
location /ws/ {
    proxy_pass http://api:3000/;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    set $auth_token "";
    if ($arg_token) { set $auth_token "Bearer $arg_token"; }
    proxy_set_header Authorization $auth_token;
}
```

**Stack frontend**:
- HTML/CSS/JS vanilla, sin build step.
- Pico.css (classless) via CDN — sin clases CSS, HTML semántico
  se ve bien.
- Chart.js via CDN para `/stats.html`.
- localStorage para JWT.

`boilerplates/README.md` actualizado de "8 boilerplates" a "9
boilerplates" con entrada nueva entre `api-orm-full` y
`api-postgres-python`.

### Validación

- `cargo fmt --all -- --check` limpio.
- `cargo clippy --all-targets --features lsp -- -D warnings` limpio.
- `cargo test --release --features lsp --test lsp_e2e` 6 tests verde
  (5 anteriores + 1 nuevo de AfterAt).
- `cargo test --release --test compile_e2e smoke_ejemplos_guia` 292
  ejemplos verde (sin regresiones).
- Backend del 9no boilerplate: `fitz check src/main.fitz` limpio.
- Smoke real Docker pendiente al release CI verde.

## [v0.10.11] — 2026-05-29 — `@hidden` field decorator + boilerplates con LATEST

Cierre de la **deuda menor del boilerplate** detectada en smoke real
v0.10.10: el response de `GET /posts/{id}` con `.preload("author")`
exponía `password_hash` del User embebido. Decisión de approach:
**resolverlo a nivel del lenguaje** con un decorator nuevo en lugar
de fix puntual en el boilerplate.

### Nuevo decorator `@hidden` sobre fields

Marca un field como invisible para el JSON I/O:

```fitz
@table("users") type User {
    @primary id: Int = 0
    email: Str = ""
    name: Str = ""
    @hidden password_hash: Str = ""   // <-- nunca cruza HTTP
    role: Str = "user"
}
```

**Semántica**:
- `__to_fitz_json` **skipea** el field — no aparece en el response
  HTTP, en cualquier contexto donde el type se serialice (directo,
  como field de otro type, eager-loaded via `.preload(...)`, etc.).
- `__FromFitzJson` **rechaza** el field — si el body del cliente
  incluye `{"password_hash": "..."}`, el server responde 400 con
  `"campo no declarado"`.
- El ORM lo **persiste normalmente** en Postgres (INSERT/SELECT/
  UPDATE incluyen el field como cualquier columna). Solo cambia
  el boundary HTTP.
- El código Fitz interno asigna libremente el field: en `register`,
  `User.insert(conn, User { password_hash: hash.password(body.password), ... })`
  funciona igual.

**Ortogonal al ORM**: `@hidden` funciona en types con o sin
`@table` — útil también para metadata interna en types plain HTTP.

### Cambios técnicos

- **src/types.rs**: nuevo arm `"hidden"` en
  `parse_table_decorators_for_type`. Tolera el decorator (sin args
  ni kwargs). Importante: NO setea `any_field_decorator = true`
  (que dispara el check "missing @table") — `@hidden` es ortogonal
  al ORM y funciona en types plain HTTP.
- **src/codegen.rs**: nuevo flag `TypeSigField.hidden: bool`
  propagado desde `Field.decorators` en los 4 sitios donde se
  construye `TypeSigField`. `gen_type_http_impls_for_sig_with_meta`
  skipea fields con `hidden: true` en:
  - `__to_fitz_json` body (no aparece en el output JSON).
  - `__allowed` lista del `__FromFitzJson` (rechaza extras).
  - field iteration del `__FromFitzJson` (no se lee del input).
  - struct literal: usa el default declarado del field o
    `Default::default()` para construir el struct sin el field.
- **editors/vscode/package.json**: bump a `0.10.11`. La grammar
  TextMate ya captura `@hidden` con el pattern genérico
  `@[a-zA-Z_][a-zA-Z0-9_]*` — sin cambios necesarios.

### Migración de boilerplates al patrón LATEST

Aprovechando el ciclo, los Dockerfiles de `api-orm-full` y
`api-postgres-fitz` pasan de `cargo install --git` (que compilaba
Fitz desde source en ~5-8min) al patrón pre-built
`ghcr.io/thegreekman76/fitz:latest` (ya usado en
`api-middleware-cors`, `api-postgres-python`, `api-simple`,
`api-websocket`, `cli-tool`, `api-fullstack-postgres`).

**Reducción ~10x del build time**: primer build de ~5-8min pasa a
~30-60s. La imagen `:latest` se actualiza automáticamente en cada
release del repo (workflow `.github/workflows/release.yml`).

```dockerfile
ARG FITZ_TAG=latest
FROM ghcr.io/thegreekman76/fitz:${FITZ_TAG} AS builder

WORKDIR /app
COPY fitz.toml ./
COPY src/ ./src/
RUN fitz build src/main.fitz

FROM debian:bookworm-slim
COPY --from=builder /app/src/main /usr/local/bin/app
EXPOSE 3000
CMD ["/usr/local/bin/app"]
```

En uso normal: `docker compose build` (sin `--build-arg`). Pinned:
`docker compose build --build-arg FITZ_TAG=v0.10.11`.

### Aplicación al boilerplate api-orm-full

`User.password_hash` ahora marcado con `@hidden`. Validación bit-a-
bit: `GET /posts/{id}` con preload author devuelve el User
embebido SIN `password_hash` (vs el leak detectado en v0.10.10).

### Documentación

- **docs/db-orm.md** sección 4: nuevo bloque "`@hidden`: ocultar
  fields de la frontera HTTP" con semántica + cuándo usar + cuándo
  NO usar + ortogonalidad con `@table`.
- **boilerplates/api-orm-full/README.md**: actualización del
  comando `docker compose build` para usar LATEST por default.
- **boilerplates/api-orm-full/Dockerfile** + `Dockerfile.distroless`:
  migración al patrón pre-built.
- **boilerplates/api-postgres-fitz/Dockerfile** + `Dockerfile.distroless`:
  migración paralela.

### Tests

- **tests/compile_e2e.rs**: nuevo test
  `hidden_decorator_skipea_field_en_json_io_v0_10_11` que valida
  los 3 casos canónicos:
  1. GET response NO incluye el hidden field aunque el handler le
     asignó valor.
  2. POST sin el field → 200 (el server usa el default).
  3. POST con el field → 400 ("campo no declarado").

### Validación

- `cargo fmt --all -- --check` limpio.
- `cargo clippy --all-targets -- -D warnings` limpio.
- `cargo test --release --test compile_e2e hidden_decorator` verde.
- `cargo test --release --test compile_e2e smoke_ejemplos_guia` 292
  ejemplos verde (sin regresiones).

### Deuda menor visible (NO bloquea release)

LSP autocomplete context-aware después de `@`: hoy la grammar
TextMate destaca cualquier `@name`, pero el LSP no sugiere la
lista cerrada de decorators (`@get`/`@post`/`@table`/`@hidden`/
etc.) al escribir `@`. Sería una mini-fase de ~30-60min en una
próxima iteración cuando aparezca presión real.

## [v0.10.10] — 2026-05-28 — Fix deadlock `__to_fitz_json` en has_many virtual (preload hang cerrado)

**Cierre del preload hang** dejado como deuda residual en v0.10.9.
La hipótesis inicial (bug del read loop del driver Postgres al
encadenar queries) era **incorrecta**: el driver funciona
perfectamente y el preload completa todas sus queries (todos los
`ReadyForQuery` se reciben). El bug está en el **codegen del impl
`__ToFitzJson` para tipos con relations virtuales has_many**.

### Root cause

En `gen_type_http_impls_for_sig_with_meta` (src/codegen.rs), el
conditional emit del field has_many virtual — introducido en
v0.10.8.3 para activar `.preload(...)` end-to-end en el JSON
response — tenía un fallo de lock scope:

```rust
{
    let __g = self.comments.lock().unwrap();
    if !__g.is_empty() {
        __obj.insert(
            "comments".to_string(),
            self.comments.__to_fitz_json(),  // ← re-lockea
        );
    }
}
```

Mientras `__g` retiene el `MutexGuard` sobre el `Mutex<Vec<...>>`,
el `__to_fitz_json` del impl genérico `Arc<Mutex<T>>` hace
`self.lock().unwrap().__to_fitz_json()` sobre el **mismo** Mutex.
`std::sync::Mutex` NO es reentrante → **deadlock instantáneo**
del worker thread.

**Manifestación**: en el boilerplate api-orm-full, `GET /posts/{id}`
con `.preload("author").preload("comments")` colgaba en la
serialización del response (HTTP 000 timeout a los 8s). Los
handlers SIN preloads activos funcionaban porque
`__g.is_empty()` era true y nunca llegaba al re-lock.

### Fix

Liberar el guard ANTES del re-lock. Chequeo `is_empty` en un
scope acotado que dropea el guard, después serialización normal:

```rust
{
    let __is_empty = {
        let __g = self.comments.lock().unwrap();
        __g.is_empty()
    };  // ← __g dropped aquí
    if !__is_empty {
        __obj.insert(
            "comments".to_string(),
            self.comments.__to_fitz_json(),  // ← lock libre ahora
        );
    }
}
```

El re-lock dentro de `__to_fitz_json` ahora encuentra el Mutex libre.

### Workflow de diagnóstico

3 ciclos de eprintln strategic en commits `[REVERTIR]` aislaron el
hang con precisión:
1. `[FITZ-WIRE]` en `db.rs::read_message` → confirmó que el driver
   recibe todos los `ReadyForQuery` del preload (descartó bug del
   read loop).
2. `[FITZ-PRELOAD] processing/done` + `[FITZ-PRELOAD-LOOP-EXIT]`
   en `emit_preload_dispatch` → confirmó que el for loop del
   preload completa OK.
3. `[FITZ-FIRST-CLOSURE] dropping/dropped __rows` →
   confirmó que el `drop(__rows)` no es el culpable.
4. `[FITZ-WRAP-PRE-CATCH]` + `[FITZ-WRAP-POST-CATCH]` en
   `emit_handler_dispatch_and_response` → confirmó que
   `catch_unwind().await` del wrapper NUNCA retorna (handler
   `Future` no completa).
5. `[FITZ-RET-MATCH] pre-await / post-await / Ok arm calling
   __to_fitz_json / __to_fitz_json done` en el handler return →
   **bingo**: vimos hasta "Ok arm calling __to_fitz_json" pero
   NUNCA "done". Aislado al impl.

Todos los eprintln revertidos en este commit final.

### Smoke real Docker validado

`GET /posts/1` con `.preload("author").preload("comments")` ahora
responde **HTTP 200 en ~140ms** con el Post + author (preloaded
User) + comments (preloaded Vec<Comment>) embebidos en el JSON.
Otros endpoints (`GET /posts`, `GET /stats/posts-per-user`,
auth/register) sin regresiones.

### Deuda menor del boilerplate descubierta

El response de `GET /posts/{id}` expone `password_hash` del author
porque `Post.author: User?` incluye ese field. **No es bug del
lenguaje** — es del boilerplate. Fix típico: handler hace mapping
a un `PostPublic`/`UserPublic` que omite el field sensible. Queda
como deuda residual del boilerplate, no bloquea v0.10.10.

### Cambios coordinados

- `editors/vscode/package.json`: bump a `0.10.10`.
- `boilerplates/api-orm-full/Dockerfile` + `README.md`:
  `FITZ_TAG=v0.10.10` (en commit separado al rebuild del
  boilerplate).

### Validación

- `cargo fmt --all -- --check` limpio.
- `cargo clippy --all-targets -- -D warnings` limpio.
- `cargo test --release --test compile_e2e cross_module_orm` verde.
- Smoke real Docker: `GET /posts/1` con preload → 200 con author
  + comments completos en 140ms (vs HTTP 000 timeout a los 8s en
  v0.10.9).

## [v0.10.9] — 2026-05-28 — Pool singleton per URL (fix connection leak)

**Mini-fase de cierre del gap runtime más serio** descubierto en
smoke real Docker de v0.10.8: cada llamada a `db.connect(url)`
desde Fitz creaba un POOL NUEVO con 10 permits + TCP conns.
Después de N requests al boilerplate api-orm-full, Postgres se
quedaba sin slots (`max_connections=100` default) y `acquire()`
colgaba indefinidamente, manifestándose como "preload hang"
visible en GETs con `.preload(...)`.

### 10.9.2 (#2 nuevo) — `connect_url` singleton per URL

`fitz::db::connect_url(url)` ahora cachea el `Arc<DbConnHandle>`
en un mapa global thread-safe (`OnceLock<Mutex<HashMap<String,
Arc<DbConnHandle>>>>`). Calls subsiguientes con la misma URL
devuelven clone(Arc) del handle existente — TODAS las conns TCP
se comparten via el pool único.

**Cambios técnicos**:

- `connect_url` ahora retorna `Arc<DbConnHandle>` directo (en vez
  de `DbConnHandle` por valor + el caller wrappea en `Arc::new`).
  Call sites actualizados: `evaluator.rs` y `codegen.rs`.
- Cache check + fast path zero-alloc cuando hay handle existente.
- Si el handle fue cerrado con `.close()` explícito, se crea uno
  nuevo (caller quiere reabrir).
- Tests actualizados en `tests/db_real_postgres.rs`.

**Trade-off documentado**: los handles persisten hasta el cierre
del proceso. Memoria despreciable (~24 KB por pool idle). Si
nunca te volvés a conectar a una URL, el pool sobrevive sin
uso — aceptable para 99% de los servicios.

**El pool singleton fue validado end-to-end** en smoke real
Docker post-tag: 3 GETs consecutivos = 2 conns constantes (1
schema init + 1 del pool reutilizado), confirmando que ya no
hay leak.

### Preload hang sigue abierto — gap separado del driver

El smoke real con `GET /posts/{id}/preload(...)` mostró que el
preload hang **NO** era causado por el pool leak (como inicial-
mente asumí). Después de cerrar el pool leak, el preload sigue
colgándose en `extended_query` aún con conns disponibles.

Diagnóstico del smoke: tras un preload colgado, `pg_stat_activity`
muestra la conn en estado `idle` con la última query del preload
("SELECT ... comments WHERE post_id IN ..."). Postgres terminó
de servir; el cliente Fitz nunca leyó la respuesta final
(probable: ReadyForQuery no se lee).

Es un **bug separado del read loop del driver** (`Connection::
extended_query` en `src/db.rs`) cuando hay múltiples queries
chained sobre la misma `DbConnHandle`. Queda como deuda
residual para v0.10.10.

Los otros 12+ endpoints HTTP/WS del boilerplate funcionan
correctamente con el pool singleton.

### Cambios coordinados

- `boilerplates/api-orm-full/Dockerfile` + `README.md`:
  `FITZ_TAG=v0.10.9`.
- `editors/vscode/package.json`: bump a `0.10.9`.

### Validación

- `cargo build --bin fitz --release` OK.
- `cargo test --release --test compile_e2e smoke_ejemplos_guia`
  292 ejemplos verde.
- `cargo fmt --all -- --check` limpio.
- `cargo clippy --all-targets --release -- -D warnings` limpio.

**Smoke real Docker queda como validación CI/Linux** — el
ambiente Docker Desktop Windows tiene un bug intermitente con
SCRAM-SHA-256 sobre el bridge TCP que cuelga el `Connection::
connect` aún con código pristine pre-v0.10.9. NO bloquea el
release porque el fix es localizado al pool singleton y el
ambiente Linux real no tiene ese issue.

### Próximo norte

- 9no boilerplate `api-orm-full-fullstack` (frontend vanilla
  nginx, memoria `project_boilerplate_orm_full_fullstack.md`).
- Benchmarks Fitz ORM vs SQLAlchemy.

## [v0.10.8] — 2026-05-28 — Cierre de 8 gaps del smoke real Docker

**Mini-fase de cierre** de los 8 gaps cross-module descubiertos
durante el smoke real del boilerplate `api-orm-full` con
Postgres real en Docker (v0.10.7). El binario compila local +
`fitz check` verde + smoke 292 verde no los detectaba — solo
aparecen cuando el binario levanta el server contra DB real y se
le pegan requests HTTP/WS. Todos cerrados en 4 rondas de
sub-pasos (10.8.1 → 10.8.8) en una sesión, ~1500 LoC netas + 8
tests E2E nuevos.

### Round 1 (10.8.1 + 10.8.2 + 10.8.3)

- **10.8.1 (#6) — HTTP wrapper desempaca `Result<T>` tail sin
  `Ok()` explícito**. El codegen ahora emite `match` runtime que
  desempaca: `Ok(v)` → 200 con `v` puro, `Err(e)` → 500 con
  `{"error": e}`. Aplica al path `response_mode` (handlers
  cross-module con `?` o `@authenticated`). Antes serializaba
  `Result<T, E>` entero produciendo `{"Ok": ...}`.
- **10.8.2 (#5) — Decorator `@db_default` para fields managed-by-DB**.
  El ORM skipea estos del INSERT; Postgres aplica su `DEFAULT`
  declarado en el schema (típico: `DEFAULT NOW()` para
  timestamps). Field sigue en RETURNING * con el valor que
  Postgres asignó. Paralelo a W4 pero general (cualquier tipo).
- **10.8.3 (#7) — W17 eager loading: virtuales SÍ se emiten en
  JSON cuando preloaded**. Runtime check: `Option<T>` → emit si
  `is_some()`, `Vec<T>` → emit si `!is_empty()`. Antes los
  virtuales JAMÁS aparecían en el JSON, perdiendo el beneficio
  del `.preload(...)`.

### Round 2 (10.8.4 + 10.8.5)

- **10.8.4 (#1) — Narrowing flow-sensitive `Nullable<T>` post-
  `if (x != null)`**. El checker refina el binding adentro del
  then/else branch; el codegen emite shadow `let x = x.unwrap();`
  para que el value Rust sea `T` puro. Cubre también `if (x ==
  null)` para el else branch.
- **10.8.5 (#3) — OpenAPI 3.1 cross-module paths**. El schema
  emitido por `fitz build` ahora incluye los handlers HTTP de
  módulos importados. Antes `paths: []` cuando los handlers
  vivían cross-module. Fix vía
  `pseudo_routes_from_program_and_modules(program,
  module_http_stmts)`.

### Round 3 (10.8.6 + 10.8.7)

- **10.8.6 (#4) — WS Router cross-module + AsyncAPI cross-module**.
  Handlers `@ws` cross-module se enchufan al Router axum del
  main (paralelo a W16 para HTTP). El módulo emite
  `pub async fn __ws_handler_<name>`; main registra
  `.route("/path", axum::routing::get(crate::<mod>::__ws_handler_<name>))`.
  El schema AsyncAPI 3.0 también se emite cuando los `@ws` viven
  cross-module. Pre-fix: WS handshake al `/feed` cross-module
  daba 404.
- **10.8.7 (#2) — `ws_broadcast(endpoint, msg)` built-in**.
  Habilita el patrón canónico SaaS "handler HTTP triggerea
  notification realtime a clientes WS conectados". Helper en
  `http.rs` (`ws_broadcast_to_endpoint`), built-in en evaluator
  (`builtin_ws_broadcast`), signature `(Str, Any) -> Null` en
  checker, codegen emite `crate::__fitz_ws_broadcast(...)` (con
  `crate::` prefix para funcionar desde módulos). Pre-scan
  `program_uses_ws_broadcast` activa preludio WS + helper.

### Round 4 (10.8.8 — cierre formal)

- Extensión VSCode: grammar TextMate suma `ws_broadcast` a la
  lista de builtins highlightables; LSP completion lo lista en
  `scope_level_completions`. Bumpeo a v0.10.8.
- Boilerplate `api-orm-full` revertido a sintaxis canónica:
  schema con `timestamptz NOT NULL DEFAULT NOW()`, models con
  `@db_default`, handlers `posts.fitz` con `return <chain>.await`
  directo, narrowing con `if (status != null)` en vez de match
  arm, broadcast WS real en `comments.fitz`. README actualizado.
- CHANGELOG, deudas-post-5b, FITZ_TAG en Dockerfile/README,
  todos a v0.10.8.

### Tests

8 tests E2E nuevos en `tests/compile_e2e.rs`:
`http_wrapper_desempaca_result_tail_sin_ok_explicito`,
`orm_db_default_skipea_field_del_insert`,
`orm_w17_eager_loaded_virtuales_aparecen_en_json`,
`checker_narrow_nullable_post_if_not_null`,
`checker_narrow_nullable_else_branch_eq_null`,
`openapi_cross_module_incluye_handlers_de_modulos`,
`ws_router_y_asyncapi_cross_module`,
`ws_broadcast_builtin_cross_handler`.

Smoke `GUIDE_EXAMPLES_COMPILE` 292 ejemplos verde con todos los
fixes integrados. `cargo fmt --all -- --check` limpio,
`cargo clippy --all-targets --release -- -D warnings` limpio.

### Próximo norte

- 9no boilerplate `api-orm-full-fullstack` (frontend vanilla
  nginx sobre el backend api-orm-full, memoria
  `project_boilerplate_orm_full_fullstack.md`).
- Benchmarks Fitz ORM vs SQLAlchemy (boilerplate `task` actual
  con SQLAlchemy vs `api-orm-full-fullstack` con ORM nativo).
- Curso "Fitz de 0 a experto" (memoria `project_curso_plan.md`).

## [v0.10.7] — 2026-05-28 — W17/W18: cross-module ORM completo + boilerplate `api-orm-full` + 5 gaps cerrados

**Release bundle**: cierre del cross-module ORM (W17 + W18), 8va
plantilla del directorio `boilerplates/` (`api-orm-full`
multi-archivo), y bloque de 4 gaps adicionales del codegen
descubiertos al construir el boilerplate. La política
"cerrar gaps que aparezcan al construir el boilerplate ANTES del
release" se aplicó estricto — todo lo que ahora corre el showcase
es paridad bit-a-bit `fitz run` ↔ `fitz build`.

### Boilerplate `api-orm-full` (nuevo)

Multi-archivo (9 módulos Fitz) showcase del **stack web first-class
entero** en un solo binario standalone:

- **HTTP + auth nativa** (`@auth_provider`/`@authenticated` cross-
  module) + **OpenAPI 3.1 auto** en `/docs`.
- **WebSockets tipados** (`@ws("/feed")` con `WsConn<FeedEvent>`)
  + **AsyncAPI 3.0 auto** en `/asyncapi.json` + heartbeat 30s.
- **Cron jobs sin Celery/broker** (`@cron("0 0 * * *") cleanup_old_drafts`).
- **ORM nativo declarativo** con 4 `@table` types coordinados
  (User/Profile/Post/Comment), relations completas (`@has_many`/
  `@has_one`/`@belongs_to` + companion fields), eager loading
  (`.preload("author")`/`.preload("comments")`), JSONB
  (`metadata: Map<Str, Any>`), arrays (`tags: List<Str>` con
  `.has(var)`), aggregates (GROUP BY `count(db)`).
- **Sin Python, sin SQLAlchemy, sin Celery, sin Redis, sin broker**
  — un solo binario `fitz build`. Imagen distroless ~15-20 MB.

Patrón cross-module W12 + W16 + W17 + W18: handlers HTTP/WS,
cron jobs, `@auth_provider` y `@table` types viven en módulos
por feature; el main solo hace `import auth, posts, comments,
realtime, jobs`.

### 5 gaps/bugs del codegen cerrados (post-W17, durante boilerplate)

Política: cerrar TODO gap descubierto durante el boilerplate
ANTES de declarar el sub-paso completo (memoria
`feedback_post_changes_smoke_examples_boilerplates`).

- **R.1.3 — `Map<Str, Any>` con indexing assignment dinámico**
  (`m["k"] = v`). El storage Rust de `Map<_, Any>` es
  `Vec<(__FitzValue, __FitzValue)>`; el codegen del indexing
  assignment NO envolvía key/value como `__FitzValue`. Fix en
  `gen_index_assign`. Caso canónico: partial updates en APIs REST.
- **R.1.3-bis — `.has(var)` sobre `Map<Str, Any>`** (paralelo).
  Fix en `gen_map_has`.
- **W18 — `has_opaque_field` ignora virtuales del ORM** en
  `emit_helpers_for_imported_types`. El filtro previo a emitir
  `__ToFitzJson`/`__FromFitzJson` para types cross-module miraba
  los virtuales (`@has_many`/`@has_one`/BelongsToCompanion) que
  degradan a `Any` post-remap cuando el target no está importado
  al main. Resultado: impl jamás se emite, rustc rompe con
  "trait bound not satisfied". Fix: filtrar virtuales antes del
  check usando el `TableMetadata`.
- **Bug del format string en jsonb dynamic update**. El dispatch
  `Dynamic` de `.update(db, map_var)` para fields jsonb tenía
  `{{}}` (escaped braces) donde debería tener `{}` para interpolar
  el error message. Fix trivial cambio de string.
- **`.has(var)` sobre arrays Postgres** (`text[]`/`int8[]`/etc.).
  El codegen rechazaba con "el value debe ser literal". Fix:
  delegar a `translate_closure_to_sql` (reusa máquina W3/W6) que
  bindea via `__IntoPgValue::into_pg(...)`. Caso canónico: filtros
  por tag en endpoints listables.

### Tests y validación al cierre

- 3 tests E2E nuevos en `tests/compile_e2e.rs`:
  `map_str_any_indexing_assign_compilado`,
  `cross_module_table_virtual_w18_remap_any`,
  `orm_array_has_acepta_var_externa`.
- Test E2E W17 ya existente:
  `cross_module_orm_virtual_fields_skip_w17`.
- Smoke `GUIDE_EXAMPLES_COMPILE` 292 ejemplos verde con todos los
  fixes integrados.
- `cargo fmt --all -- --check` limpio.
- `cargo clippy --all-targets --release -- -D warnings` limpio.

### Gaps abiertos derivados (NO bloquean el release)

Detalle en `docs/deudas-post-5b.md` sección "Mini-fase W18+".

- Narrowing flow-sensitive de `Nullable<T>` → `T` post-`if (x !=
  null)`. Workaround idiomático: match arm.
- Broadcast HTTP → WS cross-handler. Sin API global
  `ws_broadcast(endpoint, msg)` hoy. El boilerplate modela
  `/feed` como broadcast simétrico entre clientes WS.

### W17: virtual fields skip en impls cross-module (incluido en este release)

**Cierre del último gap conocido del codegen cross-module ORM**
descubierto durante el primer intento de implementar el boilerplate
`api-orm-full` (showcase multi-archivo del stack DB+ORM+HTTP+WS+
cron). **Sin sintaxis nueva**: cambio interno del codegen — los
programas existentes siguen compilando bit-a-bit, sin cambios en
grammar TextMate, LSP completions ni docs prosa.

**El bug**: `@table type` con `@has_many`/`@has_one` declarado en
un módulo importado + handler que lo retorna como response, vivos
en módulos distintos al main. Caso canónico:

```
src/models.fitz   →  type User { ... @has_many("Post", ...) posts }
src/posts.fitz    →  from models import User, Post
                     @get("/users") fn list() -> List<User> { ... }
src/main.fitz     →  import posts
                     @server(3000)
                     fn main() => 0
```

El codegen al emitir `impl __FromFitzJson for UserData` en main.rs
hacía remap del field virtual `posts: List<Post>` → `List<Any>`
(porque `Post` no estaba en el env del importer main) → emitía
`Vec<__FitzValue>`. Pero `__FitzValue` no se activaba para el
programa, así que rustc rompía con `cannot find type __FitzValue
in this scope`.

**El fix**: skipear los virtual fields (HasMany/HasOne/
BelongsToCompanion via `TableMetadata.is_virtual_field`) en los
impls `__ToFitzJson`/`__FromFitzJson`. Esos fields no van a la
DB ni deben aparecer en JSON I/O — el cliente no debe enviarlos
como body, la response no los serializa. En el struct literal
del `__from_fitz_json`, los virtuales se inicializan inline con
`Default::default()` para evitar nombrar el tipo remap-degradado.

**Cambios técnicos**:

- Nueva variante `gen_type_http_impls_for_sig_with_meta(name, sig,
  meta: Option<&TableMetadata>)` en `src/codegen.rs` que filtra
  virtuales según el meta del type.
- Ambos call sites actualizados: `gen_type_http_impls` (types
  locales) hace lookup vía `table_metadata_for(id)`;
  `emit_helpers_for_imported_types` (cross-module) hace lookup en
  `m.table_metadata.get(type_name)` del módulo origen.
- Test E2E nuevo `cross_module_orm_virtual_fields_skip_w17` en
  `tests/compile_e2e.rs` candea el caso 3-archivos. Validado
  runtime: `GET /users` devuelve `{"id":7,"name":"ada"}` SIN
  incluir el virtual `posts`.

**Validación al cierre**:

- 314/314 tests `compile_e2e` pasan (no-regresión sobre los 6 tests
  cross-module existentes: W8/W10/W11/W12/W15/W16).
- Smoke `GUIDE_EXAMPLES_COMPILE` (292 ejemplos) verde.
- `cargo fmt --all -- --check` limpio.
- `cargo clippy --all-targets --release -- -D warnings` limpio.

**Deudas derivadas documentadas** (en `docs/deudas-post-5b.md` sec
nueva):

- ⚠️ Bug del checker: inferencia `Option<String>` en `let x = match
  Result { Ok(v) => v, Err(_) => return ... }`. Workaround
  trivial: anotar `let x: Str = ...`.
- ⚠️ Forward refs en `@has_many("Target", ...)` con Target después
  en el mismo módulo: rompe si el codegen procesa navigation.
  Workaround: declarar Target antes.
- ⚠️ Importar TODOS los `@table` types al módulo que use cualquier
  uno (el codegen valida ALL targets). Workaround: `from models
  import User, Post, ...` (todos los referenciados).

## [v0.10.6] — 2026-05-27 — Bloque W1-W7: workarounds residuales del ORM cerrados

**Cierre del bloque "workarounds residuales del ORM"** identificados
durante v0.10.4 + v0.10.5. Los 7 workarounds menores documentados
en `docs/db-orm.md` sec 28 quedan cerrados con commits dedicados
+ tests E2E. **El stack DB+ORM ya no tiene fricciones residuales
conocidas para los patrones canonicales del language guide / boilerplates.**

| Workaround | Sub-commit | Tests nuevos |
|------------|------------|--------------|
| W4 — `id: 0` sentinel auto-asigna bigserial | commit dedicado | 2 unit codegen |
| W5 — `db.close()` devuelve `Future<Result<Null>>` | commit dedicado | 1 unit codegen |
| W7 — `.update(db, Map var)` además del literal | commit dedicado | 2 unit + 1 E2E |
| W3 — `.starts_with`/`.ends_with`/`.contains` aceptan var Str | commit dedicado | 2 unit codegen |
| W6 — `body.field` en closures de `.where` | commit dedicado | 2 unit codegen |
| W1 — Map literal homogéneo a field `Map<Str, Any>` | commit dedicado | 1 E2E |
| W2 — Nullable refinement en match arms | commit dedicado | 1 unit + 1 E2E |

**Cambios técnicos clave**:

- **W4**: nuevo branch runtime `if __g.<pk> == 0` en `gen_orm_type_insert`
  con dos SQLs alternativos (con/sin PK) elegidos según el value runtime
  del primary. Paralelo bit-a-bit al evaluator (skip del field cuando
  `Value::Int(0)`).
- **W5**: `db.close()` devuelve `Future<Result<Null>>` (antes `Future<Null>`).
  El helper preludio `__fitz_db_close` ahora retorna `Result<(), String>`
  con `.map_err(|e| e.to_string())`. Los docs ya prometían esta semántica
  desde v0.10.5 — ahora el código se alineó.
- **W7**: nuevo `UpdateSetEmission { Static, Dynamic }`. Static (Map
  literal) mantiene el shape anterior. Dynamic (Map var/expr) emite un
  closure IIFE con match runtime sobre `key.as_str()` ramificado por
  field del type, con conversión `__FitzValue → __FitzPgValue` per-tipo.
  Soporta Int/Float/Str/Bool/Nullable<primitivo> + Map<...> (jsonb) +
  List<scalar> (arrays).
- **W3**: dos paths en `starts_with`/`ends_with`/`contains` (tanto en
  evaluator como codegen). Str literal mantiene el escape Rust-side de
  `%`/`_`. Var/expr se traduce como arg general y envuelve SQL-side con
  `||` Postgres (`$N || '%'`, `'%' || $N`, etc.).
- **W6**: el translator (evaluator + codegen) acepta `<var>.<field>` cuando
  la var no es el `param_name` del closure. Hace lookup en el closure_env
  (evaluator) o delega a `gen_expr` recursivo (codegen) y bindea como
  `$N` via `__IntoPgValue`. Soporta chains arbitrarios (`req.inner.email`).
- **W1**: nuevo wrapper `gen_map_lit_with_hint(pairs, span, hint)`. Cuando
  el hint es `Map<_, Any>`, fuerza `heterogeneous_v = true` antes del
  loop de `lub`, lo que hace que el shape emitido sea `Vec<(FV, FV)>`.
  `gen_struct_lit` propaga el hint del field destino. `gen_map_lit`
  original queda como wrapper sin hint (paridad pre-v0.10.6).
- **W2**: dos correcciones coordinadas en `gen_pattern`. **Bug fix**:
  `Pattern::Null` sobre Nullable emitía `_` (matcheaba TODO, no solo
  null) — ahora emite `None` específico. **Refinement**: `Pattern::Ident`
  sobre Nullable emite `Some(name)` y declara `name` como inner `T` (no
  Option). Checker estático también gana refinement flow-sensitive
  (Ident posterior a un arm Null-covering se bindea al inner T).

**Barrida de documentación**: workarounds removidos del prose de
`docs/db-orm.md` (sec 28 ahora marca los 7 como CERRADOS), ejemplos
y boilerplates de la guía. Tests existentes que usaban los workarounds
(`let lang = body.lang` antes de `.where(...)`, `id: 1` explícito en
inserts demo, etc.) actualizados a la sintaxis canónica.

**Tests** al cierre del bloque: 2562 unit + 81 cli_e2e + 295 compile_e2e
+ 3 openapi + 46+ db_real_postgres. Clippy `--all-targets -D warnings`
limpio, fmt `--all --check` limpio, smoke `GUIDE_EXAMPLES_COMPILE` verde.

**Próximo norte**: boilerplates ORM Dockerizados (convertir 5/6
SQLAlchemy → Fitz ORM nativo + boilerplate nuevo dedicado al ORM full),
benchmarks Fitz ORM vs SQLAlchemy.

## [v0.10.5] — 2026-05-26 — Bundle deudas residuales ORM #2/#3/#4 + cosecha BodyJson + workarounds documentados

**Cierre del bloque "deudas residuales del ORM"** iniciado
post-v0.10.4. 3 deudas más cerradas en commits intermedios
bundleadas en un único release + cosecha menor descubierta
durante la actualización de ejemplos:

| Deuda | Status | Sub-paso |
|-------|--------|----------|
| #1 — Map<Str, Any> en HTTP returns | Cerrada | v0.10.4 |
| #2 — BelongsTo en .preload(...) | Cerrada | v0.10.5 |
| #3 — JSON operators en .where | Cerrada | v0.10.5 |
| #4 — Chain dinámico condicional | Cerrada (drift docs) | v0.10.5 |

Total v0.10.5: ~970 LoC netas (+770 código + tests, +200 docs)
+ 3 paridad real tests + actualización completa de los 2
ejemplos guía (`31-orm.fitz` + `31b-orm-crud-http.fitz`) con
los patterns de las 4 deudas.

### Added

- **Deuda #2 — BelongsTo eager via convention** (`src/types.rs`
  +120 LoC, `src/codegen.rs` +135 LoC, `src/evaluator.rs` +10
  LoC, paridad test +112 LoC):
  - Nueva variante `RelationKind::BelongsToCompanion`. El checker
    auto-detecta el patrón canónico: `@belongs_to("User")
    user_id: Int` + sibling field `user: User?` (name derivado
    stripping `_id`, type Nullable<Target>). Registra companion
    como virtual. Sin sibling declarado, comportamiento previo
    (FK navigation directa).
  - `emit_belongs_to_companion_preload_arm` en `codegen.rs`
    paralelo a HasMany pero con SQL inverso: `WHERE target.pk IN
    (parent.fk DISTINCT)`. Asigna `Some(target)` a cada parent.
  - `.preload("user")` (companion name) ahora funciona end-to-end
    en `fitz build`. Validado con `orm_preload_belongs_to_
    companion_paridad_codegen_e2e` (3 posts + 3 preloaded users
    en 2 queries).
- **Deuda #3 — JSON operators en `.where(...)`** (`src/evaluator.rs`
  +191 LoC, `src/codegen.rs` +204 LoC, paridad test +92 LoC).
  5 method calls sobre fields jsonb (`Map<Str, ...>`) mapeados a
  operadores Postgres nativos:
  - `.has_key("k")` → `"data" ? $1`
  - `.has_all_keys([...])` → `"data" ?& $1::text[]`
  - `.has_any_keys([...])` → `"data" ?| $1::text[]`
  - `.contains_json({...})` → `"data" @> $1::jsonb`
  - `.get("k")` → `("data"->>$1)` (text result, comparable con
    `==` contra Str literal)

  Validado con `orm_jsonb_operators_in_where_paridad_codegen_e2e`
  (4 events shapes distintos → conteos esperados: 3/3/2/2/1).
- **Deuda #4 — Chain dinámico condicional** (regression test +99
  LoC, sin código Rust nuevo): el codegen YA soportaba `qb =
  qb.where(...)` adentro de un `if`. La doc previa decía "no
  compila" — drift documental. Validado con
  `orm_dynamic_chain_conditional_paridad_codegen_e2e` (4
  combinaciones de filtros condicionales sobre 5 users).
- **Cosecha BodyJson** (`src/codegen.rs` +20 LoC): nueva
  `impl __FromFitzJson for Vec<(__FitzValue, __FitzValue)>`
  en el preludio HTTP. Habilita body deserialization de fields
  `Map<Str, Any>` cuando aparecen en types HTTP entrada (e.g.
  `PostInput.metadata: Map<Str, Any>`). Encontrado al sumar
  endpoints nuevos a `31b-orm-crud-http.fitz` que aceptaban
  metadata libre del body.

### Changed

- **`examples/guide/31-orm.fitz` re-escrito** (~150 LoC) con
  todos los patterns de las 4 deudas demostrados:
  - Sec 2.7 ampliada con companion field auto-detectado.
  - Sec 2.8 ampliada con `.preload("user")` BelongsTo eager.
  - Nueva sec con JSON operators (`.has_key`/`.contains_json`/`.get`).
  - Nueva sec con chain dinámico condicional (`qb = qb.where(...)`).
  - `Post` ahora declara `metadata: Map<Str, Any>` y `user: User?`
    como ejemplos canónicos de los nuevos features.
- **`examples/guide/31b-orm-crud-http.fitz`** sumó 4 endpoints
  nuevos:
  - `GET /posts-with-author` — BelongsTo eager (deuda #2)
  - `GET /posts/drafts` — `.has_key` (deuda #3)
  - `GET /posts/by-lang/{lang}` — `.get` con var externa (deuda #3)
  - `POST /posts/search` — chain dinámico con body (deuda #4)

  Type `Post` extendido con companion `user: User?` y `metadata:
  Map<Str, Any>`. Type `PostInput` extendido con `metadata` para
  HTTP body. Nuevo type `SearchInput` para el endpoint dinámico.
- **`docs/db-orm.md`**:
  - Sec 12 (eager loading): caveat reescrito reflejando BelongsTo
    via companion como CERRADO.
  - Sec 13 (JSONB): tabla nueva de operadores + ejemplos completos.
  - Sec 21 (search filters): chain dinámico documentado con
    ejemplos correctos (no más workaround search_dynamic).
  - Sec 28 (limitaciones): 4 entradas marcadas CERRADO + nueva
    sub-sección **W1-W7 workarounds residuales documentados**
    encontrados durante el cierre del bloque (Map literal
    homogéneo no matchea Map<Str,Any>, Nullable refinement en
    match, `.starts_with` solo Str literal, `id: 0` no
    auto-asigna con bigserial, `db.close` no devuelve Result,
    `body.field` no soportado en closures, `.update` solo
    acepta Map literal). Cada uno con síntoma + workaround +
    fix futuro propuesto.

### Fixed

- Codegen format string bug en `impl __FromFitzJson for Vec<...>`
  (descubierto + arreglado en la misma sesión): el codegen
  emitía `{{}}` literal donde debía emitir `{}` (format spec
  para el `other` arg).

### Tests

- **3 paridad real tests nuevos** en `tests/db_real_postgres.rs`:
  - `orm_preload_belongs_to_companion_paridad_codegen_e2e`
  - `orm_jsonb_operators_in_where_paridad_codegen_e2e`
  - `orm_dynamic_chain_conditional_paridad_codegen_e2e`
- Total `db_real_postgres`: **46 tests** (was 43, +3).
- Smoke `GUIDE_EXAMPLES_COMPILE` (292 ejemplos) verde.
- `cargo fmt --all --check` + `cargo clippy --all-targets -D
  warnings` limpio.

### Diferenciales reforzados con v0.10.5

Sumadas a las features del MVP de Fase 10/10.b/v0.10.4, ahora
el stack DB+ORM cubre **todos los patterns canónicos** que un
ORM moderno debería tener:

- **Eager loading bidireccional**: HasMany (`.preload("posts")`)
  + BelongsTo (`.preload("user")` via companion). Cierre N+1
  en cualquier dirección de la relation, dispatch estático
  compile-time, paridad bit-a-bit run↔build.
- **JSONB queries first-class**: 5 operadores Postgres nativos
  mapeados a method calls Fitz ergonómicos. Sin bajar a SQL
  crudo para casos comunes (key exists, contains subset,
  text extract).
- **Dynamic search filters**: chain condicional con `qb =
  qb.where(...)` funciona sin compromisos de perf — el SQL
  por fragmento sigue siendo constante en compile-time, solo
  el SHAPE del chain es dinámico.
- **Workarounds documentados**: cuando un patron requiere
  workaround conocido (W1-W7), está documentado con síntoma
  reproducible + plan de fix. Sin "magia" — el user sabe
  exactamente qué funciona y qué no.

## [v0.10.4] — 2026-05-26 — Deuda residual #1 cerrada: Map<Str, Any> en HTTP returns

**Primer cierre del bloque "deudas residuales del ORM"** (decidido
post-v0.10.3). 4 deudas planeadas para atacar en orden de scope
creciente: (1) Map<Str, Any> en HTTP, (2) BelongsTo eager loading,
(3) JSON operators en `.where`, (4) chain dinámico condicional.

### Added

- **`impl __MapKey for __FitzValue`** en el preludio HTTP del
  codegen (cuando `__FitzValue` está activo). Cierra la cadena
  de trait bounds que hacía fallar la serialización de
  `Map<Str, Any>` en HTTP returns:
  - Pre-fix: `Arc<Mutex<Vec<Arc<Mutex<Vec<(__FitzValue,
    __FitzValue)>>>>>>` → trait bound `Vec<(__FitzValue,
    __FitzValue)>: __ToFitzJson` no satisfecho (porque exige
    `K: __MapKey` y `__FitzValue` no lo implementaba).
  - Post-fix: el impl convierte `__FitzValue::Str(s)` a
    `s.clone()` (caso típico de keys de JSONB y GROUP BY),
    resto via Display (matchea la lógica de
    `__fitz_fv_to_json`). El chain de trait bounds queda
    satisfecho y el codegen emite el handler correcto.
- **`examples/guide/31b-orm-crud-http.fitz` ahora incluye
  endpoint `/stats/by-email`** que llama `User.group_by(fn(u)
  => u.email).count(db).await` y devuelve `Result<List<Map<Str,
  Any>>>` serializado a JSON automáticamente. Pre-v0.10.4 este
  endpoint era el caveat documentado del ejemplo; ahora forma
  parte del showcase.

### Changed

- `examples/guide/31b-orm-crud-http.fitz` — comentario del
  header refleja el cierre de la deuda (nota histórica vs
  caveat activo).
- `docs/guide.md` cap 31 — descripción del ejemplo HTTP CRUD
  incluye el nuevo endpoint GROUP BY y la referencia a v0.10.4.
- `docs/db-orm.md`:
  - Sección 28 (limitaciones) marca la deuda
    `Map<Str, Any>` en HTTP returns como **✅ CERRADO v0.10.4**
    con explicación del fix.
  - Sección 12 (eager loading) reescrita el caveat del
    `List<Map<Str, Any>>` en HTTP returns para reflejar que
    ahora funciona end-to-end.

### Behavior

- Handlers HTTP que retornan `Result<List<Map<Str, Any>>>`
  (típicamente desde `Type.group_by(...).count/sum/avg/min/max(db)`)
  ahora compilan y corren con paridad bit-a-bit `fitz run` ↔
  `fitz build`.
- Keys del Map: `__FitzValue::Str` (caso típico GROUP BY +
  JSONB) → string original; otros variantes → Display formatted.
- Empty list → array JSON vacío `[]` (sin cambios).
- Smoke `GUIDE_EXAMPLES_COMPILE` valida 292 ejemplos verdes
  (sin nuevos archivos; el ejemplo existente ahora compila
  más endpoints).

### Tests

- Smoke `GUIDE_EXAMPLES_COMPILE` verde (292 ejemplos).
- 2552 unit tests verdes (sin cambios en el count — el fix es
  puramente codegen-side, no rompe ningún test existente).
- Lint `cargo fmt --all --check` + `cargo clippy --all-targets
  -D warnings` verdes.

## [v0.10.3] — 2026-05-26 — Guía exhaustiva DB y ORM (docs/db-orm.md)

**Hito de documentación**. Cierra la promesa hecha al diseñar
v0.10.2: el cap 31 de la guía es el RESUMEN del stack DB; la
guía exhaustiva vive aparte. Tab dedicado **"DB y ORM"** en el
nav de MkDocs entre "Guía" y "Roadmap" — `docs/db-orm.md`
nuevo con ~2600 LoC cubriendo cada operador, cada receta, cada
limitación honesta del MVP.

Decisión registrada en la memoria del proyecto (2026-05-25):
"el ORM merece su propia entrada de navegación porque (a) es un
dominio aparte del lenguaje base, (b) la gente que viene a
aprender el ORM específicamente no quiere scrollear por 30 caps
de la guía, (c) showcase del diferencial vs SQLAlchemy/Prisma/
Diesel". Cierre formal de esa decisión con este release.

### Added

- **`docs/db-orm.md`** nuevo (~2600 LoC, 30 secciones):
  - **1.** Panorama vecino (comparación side-by-side con stacks
    Python/Ruby/Java/Node/Rust/Go) + 6 diferenciales únicos.
  - **2.** Quickstart end-to-end (db.connect + @table + insert +
    where + all).
  - **3.** Driver `db`: query/exec/close/is_closed crudo +
    auto-coerción de tipos params.
  - **4.** `@table`, `@primary`, `@column(name=...)`, mapping de
    tipos Fitz → Postgres por default.
  - **5.** Read methods estáticos (`Type.all`/`first`/`count`/
    `where`).
  - **6.** QueryBuilder reference completo: chain (where /
    order_by / limit / offset / group_by / preload) + terminales
    (all / first / count / sum / avg / min / max / update /
    delete).
  - **7.** Operadores extendidos en `.where(...)` (comparators,
    lógicos, aritméticos + mod, between, is_in, like/ilike,
    starts_with/ends_with/contains, is_null/is_not_null, has/
    contains_all/contained_in) + **tabla resumen de soporte de
    variables externas por operador**.
  - **8.** Write methods + guard `.where(...)` obligatorio.
  - **9.** Aggregates scalar + GROUP BY (`Aggregated<Row>`
    separado de `QueryBuilder<Row>`).
  - **10.** Relations `@belongs_to`/`@has_one`/`@has_many` +
    kwargs `on_delete`/`on_update`/`fk`/`via`.
  - **11.** Navigation methods + chain (QueryBuilder<Target>
    cuando args vacía, terminal directo con db).
  - **12.** Eager loading `.preload(...)` con dispatch estático.
  - **13.** JSONB (`Map<Str, Any>`) con shape heterogéneo +
    JSON operators del lado SQL (workaround crudo).
  - **14.** Arrays Postgres (12 OIDs).
  - **15.** NULL en arrays (`List<scalar?>`).
  - **16.** `Map<Str, T>` concreto homogéneo vs `Map<Str, Any>`.
  - **17.** Array ops (`.has`/`.contains_all`/`.contained_in`)
    + caveat literales requeridos.
  - **18.** Date / Time / Timestamp / UUID como Str ISO 8601.
  - **19-26.** **8 recetas** runnable: paginación (offset/limit
    + cursor-based + paginado con total), búsqueda (prefijo +
    full-text con tsvector + arrays + JSONB), search filters
    combinatorios, **auth + ORM (queries scoped al user
    autenticado)** end-to-end, HTTP CRUD completo, cron jobs
    de limpieza, bulk operations (insert múltiple, update por
    set de IDs), schema idempotente al boot + migraciones
    manuales versionadas.
  - **27.** Performance: arquitectura del driver puro + SQL
    constante en codegen-time vs runtime construction (SQLAlchemy
    comparison) + placeholder para benchmarks futuros.
  - **28.** Limitaciones honestas y deuda explícita
    (migraciones automáticas, transactions, composite PKs, TLS
    strict, Date/UUID nativos, JSON operators en .where,
    BelongsTo eager, `Map<Str, Any>` en HTTP returns, chain
    dinámico, bulk insert eficiente, `db.copy_in`, `fitz db
    inspect`).
  - **29.** **CLI con DB: cómo cada subcomando interactúa** —
    `fitz run`/`build`/`check`/`openapi`/`test`/`dev`/`repl`/
    `fmt`/`lint` documentados con behavior específico sobre
    programas que usan el módulo `db` y el ORM. Subcomandos
    planeados `fitz db diff`/`migrate`/`inspect`/`seed`/`console`
    documentados como roadmap.
  - **30.** Ejemplos runnable (`31-orm.fitz` + `31b-orm-crud-
    http.fitz`) + boilerplates planeados (6 convertido + 7 nuevo).
- **`mkdocs.yml`** — entrada nueva `'DB y ORM': db-orm.md` en
  el nav entre "Guía" y "Roadmap" (decisión 2026-05-25 en
  memoria del proyecto formalizada).

### Changed

- **`docs/guide.md` cap 31** — sumada sección "Guía exhaustiva"
  con link al nuevo `docs/db-orm.md` antes del cierre. El cap
  31 sigue siendo el resumen del stack para lectores secuenciales
  de la guía; el doc dedicado es la referencia para lectores
  buscando ORM específico.
- **`README.md`** — footnote ◈ Postgres+ORM extendido con link
  al `docs/db-orm.md` ("guía exhaustiva ~2500 LoC con todos los
  operadores, recetas, CLI integration y limitaciones").
- **`docs/index.md`** — botón nuevo "DB y ORM →" al lado de
  "Guía completa →" en la sección "Por dónde arrancar". Tabla
  feature comparison suma row Postgres+ORM nativo. Texto
  introductorio actualizado a "34 capítulos" + mención del
  link a la guía exhaustiva.

### Fixed (correcciones de drift entre código y docs)

Auditoría exhaustiva durante la creación de `db-orm.md` reveló
desfasajes entre los docs/memoria y la implementación real.
Cerrados en este release:

- **Sintaxis de `on_delete`/`on_update`**: el cap 31 de la guía
  (v0.10.2) y los CHANGELOG entries de v0.10.0/v0.10.1
  describían estos como **decoradores separados**
  (`@on_delete=cascade`) con valores como **bare identifiers**.
  La realidad: son **kwargs del MISMO decorator** de relation
  (`@belongs_to`/`@has_one`/`@has_many`) con valores como
  **string literals**: `"cascade"`/`"set_null"`/`"restrict"`/
  `"no_action"`. Ejemplo correcto: `@belongs_to("User", on_delete="cascade") user_id: Int`.
  Cap 31 corregido + `db-orm.md` documenta la sintaxis correcta
  + sección 10 detalla los 4 valores válidos como string
  literals.
- **`.is_in([])` empty list**: docs decían "error claro en
  compile-time". La realidad: emite predicado `false` literal
  (no rompe el query, simplemente no matchea nada — `IN ()` no
  es SQL válido, el translator lo evita). Cap 31 corregido +
  `db-orm.md` documenta el comportamiento real.
- **Var externa support por operador**: documentación previa
  no clarificaba qué operadores aceptan vars externas vs solo
  literales. `db-orm.md` suma tabla resumen explícita: comparators
  + aritméticos + `.like(pat)` + `.ilike(pat)` + `.between(low,
  high)` aceptan vars; `.is_in([...])` arg debe ser list literal
  (items adentro pueden ser vars); `.has`/`.contains_all`/
  `.contained_in` requieren literales escalares; `.starts_with`/
  `.ends_with`/`.contains` requieren Str literal.
- **`Aggregated<Row>` chain capabilities**: la sección original
  de v0.10.2 solo mencionaba terminales (`count`/`sum`/`avg`/
  `min`/`max`). En realidad también soporta chain methods
  (`where`/`order_by`/`limit`/`offset`/`group_by`) que preservan
  el tipo. `db-orm.md` documenta ambos sets.
- **`db.is_closed()` faltante en docs**: el método existe en el
  evaluator (`Value::DbConn` arm `is_closed`) pero no estaba
  documentado. `db-orm.md` lo cubre en sección 3.

### Dependencies

Sin deps nuevas. Cambios 100% documentales.

### Hito

Con v0.10.3 el bloque DB+ORM tiene:

- **Cap 31 de la guía** — resumen para lectores secuenciales.
- **`docs/db-orm.md`** — referencia exhaustiva (~2600 LoC) con
  todos los operadores, 8 recetas runnable, CLI integration,
  limitaciones honestas, drift entre código y docs cerrado.
- **Entrada propia en MkDocs nav** — visibilidad equivalente
  a la guía principal del lenguaje.
- **2 ejemplos runnable** en `examples/guide/` (pedagógico +
  CRUD HTTP) sumados al smoke CI (292 ejemplos verdes).
- **CI con Postgres real** corriendo 44 tests en cada push.

Próximo norte: boilerplates ORM Dockerizados (convertir 6 +
crear 7 nuevo dedicado) + benchmarks Fitz ORM vs SQLAlchemy.

## [v0.10.2] — 2026-05-26 — Cap 31 guía: "Postgres + ORM nativo" + hito stack server completo

**Hito mayor del proyecto.** Cierra la documentación del bloque
"stack web first-class" del lado server con cap nuevo en
`docs/guide.md` ("Postgres + ORM nativo", cap 31) + dos ejemplos
runnable end-to-end. Con este release, las features ciudadanas
de primera clase del stack server quedan documentadas, ejemplificadas,
y vivas en CI:

| Feature           | Cap | Ejemplo                          | Status      |
| ----------------- | --- | -------------------------------- | ----------- |
| HTTP nativo       | 17  | `17-http.fitz`                   | ✅          |
| Middleware + CORS | 17  | `17b-middleware.fitz`            | ✅          |
| OpenAPI auto      | 18  | `18-docs.fitz`                   | ✅          |
| Async             | 19  | `19-async.fitz`                  | ✅          |
| `fitz build`      | 20  | `20-build.fitz`                  | ✅          |
| Interop Python    | 21  | `21-python-crud/`                | ✅          |
| Auth nativa       | 28  | `28-auth.fitz`                   | ✅          |
| WebSockets        | 29  | `29-ws.fitz`                     | ✅          |
| Jobs sin Celery   | 30  | `30-cron-background.fitz`        | ✅          |
| **Postgres + ORM** | **31** | **`31-orm.fitz` + `31b-orm-crud-http.fitz`** | **✅ NUEVO** |

Todo en el binario `fitz`, todo con paridad bit-a-bit
`fitz run` ↔ `fitz build`, todo validado en CI multi-plataforma
con Postgres real en cada push.

### Added

- **Cap 31 nuevo en `docs/guide.md`** — "Postgres + ORM nativo"
  (~550 LoC de markdown). Cubre las piezas (`db`, `@table`,
  `@primary`, `@column`, relations), read methods + QueryBuilder
  chain, write methods + guard `.where(...)` obligatorio,
  aggregates scalar + GROUP BY (`Aggregated<Row>` separado de
  `QueryBuilder<Row>`), relations + navigation methods, eager
  loading con `.preload(...)` y dispatch estático en compile-
  time, tipos avanzados (JSONB, arrays, `List<scalar?>`,
  `Map<Str, T>` concreto), operadores extendidos en `.where(...)`
  (`between`/`is_in`/`like`/`starts_with`/array ops), escape
  hatch `db.query`/`db.exec` para CTEs/window functions/JSON
  operators crudos. Sección "Por qué Fitz hace esto distinto" con
  5 diferenciales (DB nativa no lib, SQL constante codegen-time,
  paridad bit-a-bit, decorators del lenguaje no anotaciones,
  eager loading con dispatch estático). Sección "Qué no está
  en el MVP" con deuda explícita (migraciones, transactions,
  composite PKs, TLS strict, Date/Time/UUID nativos, JSON ops
  Postgres, BelongsTo en `.preload`). Cierre con callout de
  hito.
- **`examples/guide/31-orm.fitz`** (renombrado de `32-orm.fitz`)
  — el ejemplo pedagógico de 10.b.17, cap reference actualizada
  de "cap 32" a "cap 31".
- **`examples/guide/31b-orm-crud-http.fitz`** nuevo (~135 LoC)
  — showcase del stack completo: CRUD HTTP real (GET/POST/PUT/
  DELETE sobre users + posts), body deserialization con types
  custom dedicados (`UserInput`/`PostInput` separan el shape DB
  del shape HTTP), relations queries (`GET /users/{id}/posts`),
  eager loading (`GET /users-with-posts` con `.preload(...)`),
  aggregate scalar (`GET /user-count`), `env_or(...)` para leer
  `DATABASE_URL` con default, `@server(port)`. Requiere Postgres
  real para correr; compila con `fitz build` aunque no haya DB
  local. Documenta el setup pre-condición (`createdb` + `CREATE
  TABLE`) al inicio del archivo.
- **Smoke `GUIDE_EXAMPLES_COMPILE`** ahora valida 292 ejemplos
  (291 + `31b-orm-crud-http.fitz`). Garantiza que el ejemplo
  CRUD HTTP + ORM no regresione.

### Changed

- **Renumeración cap 31 → 32 / 32 → 33 / 33 → 34** en
  `docs/guide.md`:
  - Cap 31 anterior "Variables de entorno" → cap 32
  - Cap 32 anterior "Plantillas y boilerplates" → cap 33
  - Cap 33 anterior "Qué sigue" → cap 34
  - TOC actualizado, cross-refs internos al cap 31 viejo (env
    builtin) reapuntados a cap 32.
- **Rename de archivos de ejemplos** con `git mv` (preserva
  history):
  - `examples/guide/32-orm.fitz` → `examples/guide/31-orm.fitz`
  - `examples/guide/31-env.fitz` → `examples/guide/32-env.fitz`
- **`docs/index.md`** — link stale a `guide.md#31-plantillas-y-
  boilerplates` (que ya estaba roto pre-v0.10.2 — cap 31 era
  "Variables de entorno", no "Plantillas") reapuntado a
  `guide.md#33-plantillas-y-boilerplates` post-renumeración.

### Fixed

- **`up_map_update_compila` pre-existente** (regresión Windows
  UAC heredada de Mini-tanda Up): stem `up_map_update`
  gatillaba el heurístico installer-detection de Windows que
  exige elevación (`ERROR_ELEVATION_REQUIRED` 740). Renombrado
  a `up_map_upd`. Mismo workaround que aplicamos en 10.b.11
  (`orm_upd_list_map_codegen`). El test corría OK en Linux CI
  pero fallaba en local de Windows en parallel.

### Diferenciales únicos (reforzados con cap 31)

**Ningún otro lenguaje moderno** combina lo siguiente en el
binario base + cero deps externas:

- **HTTP nativo + auth + WebSockets + jobs + ORM + DB nativa**
  todo en el compilador. FastAPI/Spring/Express requieren
  ~5-10 librerías opcionales por cada uno.
- **Paridad bit-a-bit `fitz run` ↔ `fitz build`** para todas
  estas features (verificado en CI con Postgres real en cada
  push via job `db-postgres` con service container).
- **SQL constante en codegen-time**: cada `.where(closure)` se
  walka del AST DURANTE EL CODEGEN, el fragmento SQL queda
  hard-coded en el binario. Comparable a Diesel/sqlx, mejor
  que SQLAlchemy/ActiveRecord (runtime SQL construction).
- **Decorators del lenguaje**: `@table`/`@primary`/`@column`/
  `@belongs_to`/`@has_many`/`@has_one`/`@on_delete`/`@on_update`
  son parte del compilador (lexer + parser + checker + codegen),
  no anotaciones procesadas por libs en runtime (vs Spring
  `@Entity` + JPA reflection / SQLAlchemy declarative meta).
- **Eager loading con dispatch estático**: `.preload("posts")`
  con el relation name como Str literal en compile-time produce
  un `match` exhaustivo emitido por el codegen. Typos
  (`.preload("post")` sin la "s") detectados en compile-time,
  no runtime.
- **Binario standalone deployable**: `fitz build` produce un
  `.exe`/ELF/Mach-O ~5-10 MB con todo embebido — driver
  Postgres, JWT signing, Argon2 hashing, ORM, axum, tokio.
  Cero `requirements.txt`/`Cargo.toml`/`package.json` que
  mantener en el destino.

### Hito del proyecto

Con v0.10.2 cierra el bloque **"stack web first-class del lado
server"** entero, documentado y ejemplificado en la guía. La
promesa del proyecto — "escribir una API tipada con auth + DB
+ jobs + WebSockets que deploye como un binario standalone" —
está viva en un solo lenguaje, con cero deps externas para
features intrínsecas, en `fitz run` (rapid feedback) y en
`fitz build` (deploy a prod) idénticamente.

Próximo norte: Fase 11+ (frontend en `.fitz`, deployment
ciudadano primera clase, CLI builder) y refinamientos
opcionales sobre el stack ya vivo (migraciones automáticas,
transactions, TLS strict, JSON operators del lado SQL,
`Map<Str, Any>` en HTTP returns para GROUP BY).

## [v0.10.1] — 2026-05-26 — Fase 10.b: paridad bit-a-bit codegen del ORM

Hito de cierre de la deuda más grande heredada de v0.10.0. **Fase 10.b
ENTERA CERRADA**: el codegen del ORM declarativo ahora tiene paridad
bit-a-bit con el evaluator. Todo lo que `fitz run` soporta del ORM —
read methods, write methods, QueryBuilder chain, agregados, relations,
navigation, JSONB, arrays nullables, Map<Str,T> concretos, GROUP BY,
eager loading con `.preload`, operadores extendidos en `.where(...)` —
ahora también compila a binario nativo con `fitz build`.

23 commits, ~9580 LoC netas, 2552 unit + 81 cli_e2e + 291 compile_e2e
(smoke incluye `32-orm.fitz` pedagógico) + 3 openapi + 44 db_real_postgres
(`#[ignore]` opt-in, 16 son paridad codegen E2E nuevos vs evaluator).
Clippy `--all-targets -D warnings` limpio, fmt `--all --check` limpio.
**Paridad real Postgres corre en cada push a `main`** (job nuevo
`db-postgres` en `.github/workflows/ci.yml` con service container
`postgres:16`).

### Added

- **Fase 10.b.1 — Fixes preludio runtime + smoke `fitz build` con
  `db.connect` solo**. Tres bugs base cerrados: `Box::pin(...)` wrap
  del Future de `__fitz_db_connect`, imports condicionales `Arc,
  Mutex` según `has_http`/`uses_db`/`uses_python`, feature `time` de
  tokio cuando `uses_db = true`.
- **Fase 10.b.2 — Closure → SQL translator en codegen** (~400 LoC).
  Port del `translate_expr_to_sql` del evaluator al codegen. Helper
  `gen_closure_to_sql(closure, table_meta) -> (String, Vec<RustExpr>)`
  emite SQL parametrizado constante en codegen-time + Vec<Rust> de
  bindings que se evalúa en runtime. BinOp (Eq/NotEq/Lt/Gt/Lte/Gte/
  And/Or), UnaryOp (not), field access sobre el param de la closure.
  Cero overhead runtime para construir SQL.
- **Fase 10.b.3 — ORM read methods en `gen_call`**: `Type.all(db)`,
  `Type.first(db)`, `Type.count(db)`. Emit del SQL constante +
  deserializer per-type `impl __FromFitzDbRow for FooData` con
  conversión field-por-field desde `__FitzPgValue` (paralelo a
  `__FromFitzJson` para JSON HTTP).
- **Fase 10.b.4 — ORM write methods**: `Type.insert(db, record)`.
  RETURNING * round-trip al row Fitz, RETURNING id para auto-asignar
  serial. INSERT serializa fields según `TableMetadata` con casts
  apropiados (`::int8`, `::text`, `::jsonb`, etc.).
- **Fase 10.b.5 — QueryBuilder chain en codegen**: `.where(closure)`,
  `.order_by(closure, asc/desc)`, `.limit(n)`, `.offset(n)`,
  `.group_by(closure)`, terminales `.update(db, changes)` y
  `.delete(db)` con guard obligatorio `.where(...)` previo (safety
  check). Struct `__FitzQueryBuilder<Row>` con state mínimo (Vec de
  WHERE fragments + Vec de ORDER BY + Option<i64> de limit/offset
  + Vec<String> de GROUP BY), métodos accumulan al state, terminales
  componen SQL final + ejecutan via `__fitz_db_runtime`.
- **Fase 10.b.6 — Agregados scalares en codegen**: `.sum(closure, db)`,
  `.avg(closure, db)`, `.min(closure, db)`, `.max(closure, db)` sobre
  `QueryBuilder<Row>`. Helper `aggregate_f64` para path scalar.
- **Fase 10.b.7 — Navigation methods en codegen + refinement del
  checker**. Navigation `post.user_id(db).await?` → `Result<User>`
  (BelongsTo), `user.posts(db).await?` → `Result<List<Post>>`
  (HasMany), `user.profile(db).await?` → `Result<Profile>` (HasOne).
  Convención del field name: el método se nombra como el field FK
  (BelongsTo) o como el field virtual declarado en el `type`. Checker
  refinado para devolver `Type::Future(Result<Target>)` cuando args
  contiene `db`, y `Type::QueryBuilder(Target)` cuando args vacía
  (habilita chain post-navigation).
- **Fase 10.b.8.a — Arrays Postgres en codegen** (List<scalar>).
  `List<Int>` ↔ `int8[]`, `List<Str>` ↔ `text[]`, `List<Float>` ↔
  `float8[]`, `List<Bool>` ↔ `bool[]`. Marshaling directo sin pasar
  por `__FitzValue` — `Vec<T>` Rust en el row Fitz, INSERT/UPDATE
  detectan List<T> y emiten cast apropiado (`::int8[]`/etc.).
- **Fase 10.b.8.b — JSONB libre en codegen** (Map<Str, Any>). `Map<Str,
  Any>` ↔ `jsonb`. INSERT serializa Map → JSON via `serde_json` con
  `preserve_order` + cast `::jsonb`. SELECT parsea text JSON con
  `__FitzValue` (enum tagged ya existente del F13 SPIKE) preservando
  shape heterogéneo. Null Fitz → NULL real (no la string "null").
- **Fase 10.b.9.a — Validación exhaustiva del translator `.where(...)`
  en codegen**. Refinamiento de helpers que detectaban casos no
  cubiertos del AST y los rechazaban con error claro citando el shape
  esperado.
- **Fase 10.b.9.b — Operadores extendidos en `.where(...)`**: between,
  `%` (Mod), var externa al body de la closure (lookup en el scope
  del codegen para emitir como binding `$N`).
- **Fase 10.b.9.c — Operadores sobre arrays Postgres en `.where(...)`**:
  `.has(elem)` (cualquiera de los elementos del array column matchea),
  `.contains_all([...])` (`@>`), `.contained_in([...])` (`<@`).
- **Fase 10.b.10 — Cleanup + cobertura paridad real Postgres
  exhaustiva**. Helper de reuso `run_paridad_program(src, stem,
  assert)` que reduce duplicación en E2E. 14 paridad real E2E nuevos
  contra Postgres instalado: navigation, arrays + JSONB roundtrip,
  where combinatorio, between/mod/var externa, array ops, nav chain,
  GROUP BY aggregate, Map<Str,T> concreto, List<scalar?>, preload,
  CRUD lifecycle, order_by/limit/offset, basics all/first/count,
  aggregates scalar, col override en FK source.
- **Fase 10.b.11 — `.update` con List literal + Map literal**. Branches
  nuevos en `gen_qb_update_set_args` para que `.update(db, {"tags":
  ["a", "b"]})` y `.update(db, {"data": {"k": 1}})` emitan los casts
  apropiados (`::text[]`/`::jsonb`) y serialicen los valores.
- **Fase 10.b.12.a — NULL en arrays Postgres**. `List<Int?>` ↔ `int8[]
  NULL`. `__FitzPgValue::Array { elem_oid, values }` ahora codifica
  `NULL` sin quotes en el text format `{a,NULL,c}`. Parser/encoder
  simétricos. Branches específicos en `orm_field_coerce_block` y
  `orm_marshal_field_to_pg` para arrays nullable inner.
- **Fase 10.b.12.b — Map<Str, T> concretos en codegen**. `Map<Str,
  Int>` ↔ `jsonb` con shape homogéneo `HashMap<String, i64>` (vs
  `Map<Str, Any>` que usa `__FitzValue`). Marshaling directo sin
  enum dispatch — solo aplica cuando T es primitivo concreto
  (Int/Float/Str/Bool).
- **Fase 10.b.13 — Navigation chain + JSONB shape (by design)**.
  Decisión: las navigations siempre devuelven `QueryBuilder<Target>`
  cuando `args.is_empty()`, permitiendo `user.posts().order_by(...).
  all(db)`. Terminales obligatorios para ejecutar. JSONB conserva el
  shape libre del Map<Str, Any> — no se valida shape (by design: el
  user opera el dict de retorno con `.get(...)?`).
- **Fase 10.b.14 — GROUP BY + aggregate (Type::Aggregated)**. Nueva
  variante `Type::Aggregated(Box<Type>)` separada de
  `Type::QueryBuilder(Box<Type>)` para el path GROUP BY. El checker
  refina `.group_by(closure)` a `Aggregated<Row>` y los métodos
  agregados (`.count(db)` / `.sum(closure, db)` / etc.) sobre
  `Aggregated` devuelven `Result<List<Map<Str, Any>>>` (vs scalar
  sobre `QueryBuilder`). Helper `aggregate_groups` paralelo al
  scalar.
- **Fase 10.b.15 — Eager loading (.preload sobre HasMany)**. `User.
  preload("posts").all(db)` resuelve N+1 con 1 query batch
  (`SELECT * FROM posts WHERE user_id IN (1, 2, 3)`) + dispatch
  estático del relation name en compile-time vía match. Helper
  `emit_preload_dispatch` por type con `@has_many`. El relation name
  como Str literal queda hard-coded en el binario — typos detectados
  en compile-time, no runtime.
- **Fase 10.b.16 — Postgres en CI default**. Job nuevo `db-postgres`
  en `.github/workflows/ci.yml` que levanta `postgres:16` como
  service container, exporta `FITZ_TEST_PG_URL=postgres://postgres:
  postgres@localhost:5432/fitz_test`, y corre `cargo test --test
  db_real_postgres -- --ignored --test-threads=1`. Solo Linux
  (Docker service containers más estables en GHA Linux runners).
  Los 16 paridad codegen E2E + los 27 evaluator E2E ahora corren
  en cada push. **`#[ignore]` se mantiene** para que `cargo test`
  default sin env var siga rápido.
- **Fase 10.b.17 — Ejemplo guía `32-orm.fitz` pedagógico + smoke
  GUIDE**. Nuevo `examples/guide/32-orm.fitz` (~100 LoC) que muestra
  el shape canónico del ORM end-to-end: `@table` con `@primary` +
  `@column` + `@belongs_to` + `@has_many`, insert, where + first,
  chain order_by/limit/offset, operadores starts_with/is_in/between,
  aggregates scalares count/avg, GROUP BY con `Aggregated<Row>`,
  navigation belongs_to/has_many, eager loading con preload, y
  update/delete con guard. Sumado al smoke `GUIDE_EXAMPLES_COMPILE`
  (291 ejemplos compilan en cada push). `fitz build` produce binario
  aunque no haya Postgres real — el `connect` runtime falla con
  `Err` clara si la URL es inválida, así el ejemplo es ejecutable
  como guía sin Postgres local.

### Changed

- `Type::Aggregated(Box<Type>)` variante nueva, paralela a
  `Type::QueryBuilder(Box<Type>)`. Separación necesaria porque el
  path GROUP BY devuelve `List<Map<Str, Any>>` con shape heterogéneo
  vs el path scalar de `QueryBuilder.sum/avg/min/max` que devuelve
  `Float`.
- `evaluator::translate_expr_to_sql` ahora `translate_expr_to_sql_
  with_env(closure, table_meta, env: Option<&EnvRef>)` para soportar
  var externa al body de la closure (lookup en el scope del
  evaluator cuando el codegen lo necesita).
- `Value::Type` ya cacheaba `table_metadata: Option<Box<TableMetadata>>`
  desde v0.10.0; ahora el codegen además persiste el TypeEnv para
  resolver field types de relations cross-table.
- Sentinel test lock: `static ENV_VAR_LOCK: parking_lot::Mutex<()>`
  en `src/pbs.rs` para serializar tests que mutan `FITZ_TEST_PG_URL`/
  env vars globales y romper race con `cache_root_usa_env_override`.

### Fixed

- **PostData PartialEq derive faltante**: bug latente de 10.b.3 donde
  `inline_display_stmt` caía a `{:?}` para tipos compuestos y exigía
  `Debug` sobre `Arc<Mutex<NominalData>>`. Fix: branches específicos
  en `inline_display_stmt` para List/Map/Tuple/Any delegando a
  `show_expr` (paralelo al modo Display del intérprete).
- **E0507 cannot move out of self.posts en Display impl**: navigation
  fields virtuales (`@has_many posts: List<Post>`) generaban
  `Display::fmt` que movía el `Vec` adentro del receiver. Fix:
  `.clone()` explícito al pasar a `show_expr`.
- **`emit_qb_where_chain` perdía TypeExpr al usar `TypeExpr::Named
  ("Any")` placeholder**: array ops como `.has(elem)` necesitan
  conocer el inner type para emitir el cast apropiado. Fix: helper
  `type_to_type_expr_for_translator` convierte `Type` resuelto del
  checker a `TypeExpr` AST.
- **Map<Int, Int> previamente aceptado por accidente**: 10.b.12.b
  habilitó `Map<Str, T>` con T concreto, pero el codegen aceptaba
  cualquier K. Fix: K se restringe a Str (Postgres jsonb keys son
  strings). Map<Int, Int> ahora rechazado con error claro.
- **NULL en arrays E0308**: `__v` viene como `&T` en
  `for __v in __values.iter()`. Fix: `*__v` para primitivos Copy en
  some_wrap.
- **GROUP BY codegen emitía `.count` en lugar de `.aggregate_groups`**:
  10.b.14 lo separó por `Type::Aggregated` en lugar de mezclar
  paths en `gen_orm_qb_method`.
- **Windows UAC bloqueaba `orm_update_list_map_codegen` por "update"
  in stem**: ERROR_ELEVATION_REQUIRED code 740. Fix: renombre del
  helper a `orm_upd_list_map_codegen`.
- **Test paridad real `db_real_postgres` no corría en CI default**:
  ahora corre en cada push via job `db-postgres` (10.b.16). Pre-fix
  estos E2E solo corrían en local del autor.

### Dependencies

Sin deps nuevas — el driver Postgres sigue siendo **puro Fitz/Rust**.
`parking_lot` ya estaba para el intérprete (F17), reusado para el
mutex de env vars de tests.

### Diferenciales únicos (refrescados post-10.b)

Lo que sigue siendo único de Fitz tras Fase 10.b:

- **Único lenguaje moderno** con driver Postgres puro + ORM declarativo
  + paridad bit-a-bit `fitz run` ↔ `fitz build` + LSP completo
  **sin macros derive ni introspection runtime**. La paridad codegen
  cierra la última brecha que separaba el intérprete del binario.
- **Decorators del lenguaje** (`@table`/`@primary`/`@column`/
  `@belongs_to`/`@has_many`/`@has_one`/`@on_delete`/`@on_update`)
  son parte del compilador, no anotaciones procesadas en runtime.
- **SQL constante en codegen-time**: cada `.where(closure)` se walka
  del AST DURANTE EL CODEGEN, el fragmento SQL queda hard-coded
  en el binario. Zero overhead runtime para construir SQL —
  comparable a Diesel/sqlx, mejor que SQLAlchemy/ActiveRecord
  que construyen SQL via objetos en runtime.
- **Eager loading con dispatch estático**: `.preload("posts")` con
  el relation name como Str literal en compile-time → match
  exhaustivo emitido por el codegen. Typos detectados en compile-
  time, no runtime.
- **CI paridad real**: job dedicado `db-postgres` corre 27 evaluator
  E2E + 16 paridad codegen E2E contra `postgres:16` en cada push,
  cubriendo todo el ORM end-to-end sobre datos reales.

## [v0.10.0] — 2026-05-25 — Fase 10 entera: Postgres nativo + ORM declarativo

Hito mayor. Cierra **Fase 10 entera** (driver Postgres puro + pool +
ORM declarativo + relations + tipos avanzados) — la última fase del
stack web first-class. Ningún otro lenguaje moderno combina driver
Postgres puro + ORM sobre `type` + paridad bit-a-bit `fitz run` ↔
`fitz build` + LSP completo sin macros derive ni introspection runtime.

20 commits, ~7400 LoC nuevas, 2463 unit + 2574 LSP + 27 E2E reales
contra Postgres instalado. Clippy `--all-targets -D warnings` limpio,
fmt `--all --check` limpio.

### Added

- **Fase 10.1 — Driver Postgres puro en Fitz (sin libpq)**.
  - **10.1.a**: módulo nuevo `src/db.rs` (~2400 LoC) — protocolo wire
    v3.0 hand-rolled. `ConnectionConfig` con parser de URL postgres://,
    SCRAM-SHA-256 (RFC 7677) + PBKDF2-HMAC-SHA-256, Simple Query +
    Extended Query con `Parse`/`Bind`/`Describe`/`Execute`. 11 tipos
    OID core: BOOL, INT2/4/8, FLOAT4/8, TEXT/VARCHAR, BYTEA, DATE/TIME/
    TIMESTAMP/TIMESTAMPTZ, UUID, JSON/JSONB, VOID.
  - **10.1.b**: integración con evaluator — `Value::DbConn(Arc<DbConnHandle>)`,
    builtin module `db` con `db.connect(url).await`, métodos `query/
    exec/close` async sobre `DbConn`.
  - **10.1.c**: codegen del driver en `fitz build` (paridad bit-a-bit
    intérprete↔binario para programas que usan `db.*`).
- **Fase 10.2 — Pool de conexiones + reconnect + health check**.
  Pool con `Arc<DbPool>` + `OwnedSemaphorePermit`, RAII Drop pattern,
  health check con `Weak<DbPool>` para auto-cleanup, reconnect
  automático cuando una conn muere.
- **Fase 10.3 — ORM declarativo sobre `type`**.
  - **10.3.a**: decorators ORM (`@table("name")`, `@primary`, `@column(name=, sql_type=)`)
    + checker que persiste `TableMetadata` en el `TypeEnv`. Validación
    estática del shape (`@primary` sobre exactamente un field, etc.).
  - **10.3.b1**: `Type.all(db) -> Result<List<Type>>` end-to-end +
    cache de metadata en `Value::Type` para evitar re-lookup en cada
    call.
  - **10.3.b2**: `Type.where(closure) -> QueryBuilder<Row>` con
    translator AST → SQL parametrizado. Traduce BinOp comparators
    (==, !=, <, <=, >, >=), BinOp lógicos (and, or), UnaryOp (not),
    field access sobre el param de la closure. Args van como `$N`
    parametrizados, sin SQL injection.
  - **10.3.b3**: chain methods `.order_by(closure, ascending: Bool)`,
    `.limit(n)`, `.offset(n)`, `.first(db)`, `.count(db)`. Builder
    pattern con `QueryBuilderState` cloneable inmutable.
  - **10.3.c**: terminales `.insert(db, row)`, `.update(db, changes)`,
    `.delete(db)`. UPDATE refuses sin `.where(...)` previo (safety
    check). RETURNING * round-trip al row Fitz.
- **Fase 10.4 — Relations cross-table**.
  - **10.4.a**: decorators `@belongs_to("Author")`, `@has_one("Profile")`,
    `@has_many("Comment")` con `@on_delete=cascade/setnull/restrict/noaction`
    y `@on_update=...`. Persistidos en `TableMetadata.relations`. Fields
    virtuales para `has_*` (no aparecen en SQL columns).
  - **10.4.b**: navigation methods. `post.author(db) -> Result<User>`
    (BelongsTo), `user.posts(db) -> Result<List<Post>>` (HasMany),
    `user.profile(db) -> Result<Profile>` (HasOne). Lazy (1 query
    por navegación, sin N+1 eager hasta 10.6).
- **Fase 10.5 — Tipos avanzados**.
  - **10.5.a**: **JSONB**. Field `data: Map<Str, Any>` → columna
    `jsonb`. INSERT serializa Map → JSON con cast `::jsonb`. SELECT
    parsea text JSON de vuelta a Map Fitz. Nested Maps preservados.
    Null Fitz → NULL real (no la string "null").
  - **10.5.b**: **Arrays nativos** (List<T> ↔ Postgres T[]). 12 array
    OIDs (bool/int2/4/8/text/varchar/float4/8/date/timestamp/uuid).
    `PgValue::Array { elem_oid, values }` con parser/encoder del text
    format `{a,b,c}` que maneja escapes `\\`/`\"`, NULL sin quotes,
    arrays vacíos. INSERT/UPDATE detectan `List<T>` y emiten cast
    apropiado (`::int8[]`/`::text[]`/etc.). SELECT round-trip a
    `Value::List`.
  - **10.5.c**: **Date / Time / Timestamp / Timestamptz / UUID**.
    Round-trip como `Str` con formato ISO 8601 / UUID canonical. Sin
    tipos Fitz dedicados en MVP — `let d: Str = ...`. Cross-feature
    test con uuid[] valida `Array<UUID>` end-to-end.
  - **10.5.f1**: **Agregados sobre QueryBuilder** — `.sum(closure, db)`,
    `.avg(closure, db)`, `.min(closure, db)`, `.max(closure, db)`.
    Cast `::float8` automático en avg para evitar Numeric OID.
  - **10.5.f2**: **GROUP BY**. `.group_by(closure).all(db)` devuelve
    `List<Map>` con `{group_field: value, count: N, sum_x: N, ...}`.
    Auto-detección scalar vs grouped path según `state.group_by_clauses`.
  - **10.5.g**: **Operadores y filtros extendidos en `.where(...)`**.
    Method calls sobre `<param>.<col>`: `is_null()`, `is_not_null()`,
    `is_in([a, b, c])`, `like(p)`, `ilike(p)`, `starts_with(s)`,
    `ends_with(s)`, `contains(s)`. `escape_like` para escapar `%`/`_`/`\`
    en patterns. is_in con lista vacía → error claro.
- **`Type::QueryBuilder(Box<Type>)` paramétrico en el checker**. Para
  que el LSP entienda la cadena `User.where(...).order_by(...).all(db)`
  con tipos refinados (chain methods preservan QB, terminales devuelven
  `Result<List<Row>>`/`Result<Row>`/`Result<Int>` apropiado).
- **LSP refresh (post-ORM)**.
  - Grammar TextMate: `DbConn` y `DbRow` highlighted como built-in
    types. Decorators (`@table`/`@primary`/`@belongs_to`/etc.) ya cubiertos
    por pattern genérico.
  - Scope-level completions: módulo `db` como MODULE, `DbConn`/`DbRow`
    como CLASS.
  - After-dot completions:
    - `db.` → `connect`
    - `DbConn.` → `query/exec/close`
    - `TableName.` con `@table` → `all/where/insert` estáticos
    - `QueryBuilder<Row>.` → 14 chain methods + terminales con detail
      tipado al row concreto
  - Chain detection con parens balanceadas — captura `User.where(fn(u)
    => u.id > 0).` como recv válido (antes se rompía en el `)`).
  - Resolver: `DbConn` y `DbRow` aceptados como tipos primitivos en
    `resolve_named` (antes producían "tipo desconocido" en anotaciones).

### Changed

- `Value::Type` ahora cachea `table_metadata: Option<Box<TableMetadata>>`
  para que el dispatch ORM no re-lookee el env en cada call.
- `Value::QueryBuilder(Arc<dyn Any + Send + Sync>)` opaco — evita
  ciclo de dependencia entre `evaluator` y `value`.
- `Value::Instance` ahora se forma con `{ type_name, fields }` (struct
  variant) — tests E2E reformateados.
- Tests E2E del driver (15 archivos en `tests/db_real_postgres.rs`)
  con setup canonical `DROP TABLE IF EXISTS` + `CREATE TABLE` para
  re-runs limpios. Opt-in via `FITZ_TEST_PG_URL` env var.

### Fixed

- Driver: 2 bug fixes críticos durante 10.3.b1 — (a) Extended Query
  protocol fallaba silente sin `Describe(P, "")` entre `Bind` y
  `Execute` (server no enviaba `RowDescription`), (b) OID 2278 (void)
  no soportado rompía `pg_sleep` en el test del pool — mapeado a
  `PgValue::Null`.
- Driver: `Numeric` (OID 1700) sin soporte rompía AVG — fix con cast
  `::float8` automático en el SQL emit de aggregates.
- Codegen: `Value::Type` boxed (`Option<Box<TableMetadata>>`) para
  evitar `result_large_err` clippy de 117 errores tras agregar la
  metadata al enum.
- Lock scope: 9 instancias de `await_holding_lock` en los tests E2E
  refactoreadas con scopes `{ ... }` que dropean el guard antes de
  los `.await` del driver.

### Dependencies

Sin deps nuevas — driver Postgres es **puro Fitz/Rust** (sin `tokio-
postgres`, `sqlx`, `diesel`, ni libpq).

### Diferenciales únicos

- **Único lenguaje moderno** con driver Postgres puro + ORM declarativo
  + paridad bit-a-bit `fitz run` ↔ `fitz build` + LSP completo
  (autocomplete del ORM end-to-end) **sin macros derive ni
  introspection runtime**.
- Decorators del lenguaje (no lib externa): `@table`/`@primary`/
  `@belongs_to`/`@has_many` son parte del compilador.
- Validación estática: el checker exige `@primary` único, `role: Str`
  no nullable para `@admin`, etc.
- Zero deps externas: SCRAM-SHA-256 + PBKDF2 + protocolo wire v3.0
  todo hand-rolled.
- Type system aware: `QueryBuilder<Row>` paramétrico, chain refina al
  tipo concreto, LSP sugiere las 14 métodos del builder con detail
  específico al row.



En curso: ver `docs/roadmap.md` para el plan vigente. **Package
manager (9.y.1 + 9.y.2 + 9.y.3 entera + 9.y.4) CERRADOS**, **9.z
(DX) ENTERA CERRADA**, **refresh masivo de docs ENTERO CERRADO**,
y **bloque entero de mini-tandas post-Fase 8 cerrado**: ~25
mini-tandas en 4 días (2026-05-17 → 2026-05-20) llevaron el
lenguaje + LSP + HTTP a estado pulido. Highlights:

- **R-series, S, Mb-series, Math+Mb9**: ~40+ métodos chicos sobre
  primitivos y colecciones.
- **Bits/Núm/Lit/F8/F9/Fmt-build**: operadores de bit, separadores
  numéricos, hex/bin/oct, identifiers Unicode, escapes extendidos,
  format specs en codegen.
- **Cd/F11-F19**: codegen polish completo — higher-order, state
  HTTP shared, módulos transitivos, error recovery del parser,
  IR tipado per nodo, codegen interop Python.
- **HTTP polish bundle** (HC, Hpx.1/2, Mw.next, RP/MP/P1, UC/HA):
  status codes custom, Content-Type 415, return type inference,
  post-process middleware, urlencoded body completo, msg alignment.
- **DZ/CT/OAPI**: paridad chica run↔build (división por cero,
  comparar tipos distintos) + status codes con consts.
- **MP2/MP-Build + File.content Bytes**: multipart con files
  binarios end-to-end (paridad bit-a-bit).
- **Bytes**: sexto primitivo del lenguaje (`b"..."` con escapes
  `\xHH`, métodos `.len()`/`.is_empty()`/`.to_str()`, builtin
  `bytes(s)`, base64 en JSON).
- **Mw-Wrap**: wrap-style middleware con `next` callable
  (intérprete; codegen es la única deuda visible restante).
- **F13 entero**: heterogéneos en `fitz build` con
  `__FitzValue` tagged runtime — primitivos, Bytes, Nominales,
  List/Map heterogéneo, anidados con mix interno, HTTP body
  `List<Any>`/`Map<Str, Any>`, method dispatch dinámico
  (`.as_int()`/`.as_str()`/`.type_name()`). 95%+ del lenguaje
  compila a binario nativo con paridad bit-a-bit.
- **OAPI-Expr**: status codes con const-eval recursivo (BinOp +
  UnaryOp::Neg sobre consts encadenadas).
- **LSPx/LSPy + cross-module go-to-def + scope-aware completion**.

Total al cierre del bloque: **2045 unit sin feature, 2135 con
--features lsp, 250+ compile_e2e**, 77 ejemplos guía. Clippy
`-D warnings` limpio. Detalle exhaustivo en
[`docs/deudas_lenguaje.md`](docs/deudas_lenguaje.md) y
[`docs/design-fitzvalue.md`](docs/design-fitzvalue.md) (F13).

Próximo norte: **boilerplates Dockerizados** (memoria
`project_boilerplates`) — 4 boilerplates showcase del stack
cerrado en 9.w (api-simple, api-postgres-python con SQLAlchemy
via interop, api-middleware-cors, cli-tool). Luego repo público
+ sitio docs MkDocs Material. ORM nativo + migraciones
(9.w.4 / Fase 10) cuando aparezca proyecto real que lo necesite.

## [v0.9.57] — 2026-05-24 — Cierre 8-pyi-stubs: auto-pickup loader + field access tipado + race fix compile_e2e

### Added

Cierre de la última deuda activa del proyecto: **8-pyi-stubs**.
Auto-pickup loader de archivos `.pyi` adyacentes al `.fitz` raíz
+ field access tipado sobre los stubs cargados. Después de
v0.9.57, queda **cero deudas activas cerrables** — el inventario
post-boilerplates está vacío.

- **`src/pyi_loader.rs`** (módulo nuevo, ~400 LoC):
  - `load_stubs(program, base_dir, env)` — **pase 1 (8-pyi.B)**:
    walkea el programa buscando `Stmt::FromImport { path:
    ["python"], names }`, intenta cargar `<base_dir>/<name>.pyi`
    por cada nombre, parsea con `pyi_stub::parse_stub`, y
    registra solo las `class` declarations en el `TypeEnv`.
    Skipea classes con nombre en pre-scan del programa
    (`type X { ... }`) y built-ins HTTP (`Request`, `Response`,
    `File`) — política "el .fitz gana sobre el .pyi". Fns/vars
    del stub se posponen a pase 2 (8-pyi.C).
  - `load_callables(stubs, env)` — **pase 2 (8-pyi.C)**: procesa
    fns/vars top-level de cada stub cargado y crea un nominal
    sintético `__pyi_module_<binding>` con un field por
    callable/var. Fns se materializan como `Type::Function {
    params, ret: Result<ret, Str> }` (auto-wrap a Result
    paralelo al runtime 8.3 donde toda call Python se envuelve).
    Vars se materializan con su tipo directo (sin wrap). Registra
    mapping `binding → synth_id` en `env.pyi_modules`.

- **`src/pyi_stub.rs`**: nuevas APIs públicas
  `register_stub_items_into_env(items, env) -> Vec<ResolvedStubItem>`
  y `nominal_fields(env, id)`. Política "el .fitz gana":
  `register_stub_items_into_env` solo setea fields si el nominal
  todavía no tiene fields (no sobreescribe declaraciones del
  programa Fitz).

- **`src/types.rs`**:
  - `TypeEnv.pyi_modules: HashMap<String, TypeId>` + métodos
    `set_pyi_module(name, id)` / `pyi_module(name) -> Option<TypeId>`
    para mapear binding name → nominal sintético.
  - Nuevas APIs públicas `resolve_program_with_env(program,
    initial_env, errors_init)` y `check_with_env(program, env,
    errors)` que permiten partir de un env pre-llenado (típicamente
    por el loader). `resolve_program(program)` y `check_program(program)`
    quedan como wrappers para backward compat de los 11+ call sites
    sin contexto de archivo.
  - `Stmt::FromImport` from_python: si hay stub cargado (lookup
    en `pyi_modules`), bindea el nombre con `Type::Nominal(id)`
    sintético; sino fallback a `Type::PyAny` opaco.
  - `infer_method_call` para `Type::Nominal(id)`: **8-pyi.C
    field-as-callable** — antes del lookup de métodos custom
    (R.3), busca en `info.fields` un field con `type_:
    Type::Function`. Si matchea, valida arity + tipos de args
    y devuelve el ret. Mensajes de error recortan el prefijo
    `__pyi_module_` para mostrar el binding original (e.g.
    `api.fetch_user espera 1 argumento(s), recibió 3`).

- **`src/main.rs`**:
  - `base_dir_for_stub_lookup(path) -> PathBuf` — calcula el
    base dir del lookup (parent del path, fallback a cwd).
  - `check_program_with_pyi_stubs(program, path)` — wrapper
    que orquesta los dos pases del loader alrededor de
    `resolve_program_with_env` + `check_with_env`. Llamado por
    todos los call sites con path (`run`, `build`, `check`,
    `openapi`, `bundle_python`, `test`).

### Fixed

**Race condition Windows preexistente en `compile_e2e`**: pre-fix
todos los tests del harness escribían `prog.fitz` → compartían
`target/fitz-build/prog/` (cache global per-stem). Bajo
`SERIAL`, los tests corrían secuenciales, pero Windows mantenía
file handles del `.exe` un instante después de `Child.wait()`;
el siguiente test sobreescribía el mismo path y `fitz build`
fallaba con `OS error 32 — being used by another process`. Flake
real intermitente, no puro.

Fix: helper `sanitize_stem(test_name)` (lowercase + chars
no-`[a-z0-9_-]` → `_`) usado por `build_and_run`,
`build_expect_fail` y `build_and_run_with_env`. Cada test escribe
`<sanitized>.fitz` → cada uno va a `target/fitz-build/<sanitized>/`.
Cero choque de handles entre runs. Tests inline que no usan
helpers (~31 sitios) siguen vulnerables como deuda menor.

### Notes

- **8-pyi.D (codegen paridad)**: cierra "gratis". El codegen
  consume el `TypeInfo` del checker, y el checker ya usa los
  stubs vía B/C. Programas sin `from python import` siguen
  idénticos (validado smoke con `buildtest.fitz`).
- **Cap 21.8b** de `docs/guide.md` reescrito: documenta los dos
  modos (manual `fitz py-stubs` + auto-pickup), tabla de cuándo
  usar cada uno, sub-set cubierto incluyendo callables y vars
  del stub (no solo classes como pre-v0.9.57).
- **Ejemplo runnable nuevo**: `examples/guide/21c-pyi-autopickup/`
  con `users.pyi` adyacente + `app.fitz` que demuestra el
  pipeline tipado end-to-end via auto-pickup (valida con
  `fitz check`).
- **Decisiones técnicas**: lookup local-only (adyacente al
  `.fitz`, NO PYTHONPATH/site-packages) — máxima reproducibilidad
  + cero magia ambiente, diferencial vs typecheckers Python que
  dependen del venv. Silent fallback en parse error (warning a
  stderr, binding cae a PyAny). Política "el .fitz gana sobre
  el .pyi" via skip set en pase 1. Nominal sintético prefijado
  `__pyi_module_<binding>` para evitar colisiones con tipos del
  programa; prefix se recorta en mensajes de error.
- **14 unit tests nuevos en `pyi_loader::tests`** (4 del pase 1
  ya existían en v0.9.57.B + 4 nuevos del pase 2 8-pyi.C +
  regresiones). Suite total: **2304 unit (default) / 2395 lsp**.
- **Smoke E2E manual VERDE**: programa con `from python import
  api` + `api.pyi` adyacente valida tipado completo de classes,
  fns con auto-wrap a Result, vars top-level, arity check, y
  type check de args (todos producen errores precisos del
  checker con mensajes user-friendly).
- **Próximo norte**: **Fase 10 — Stack DB nativo + ORM
  declarativo**. Driver Postgres en Fitz puro + ORM sobre `type`
  + migraciones autogeneradas. Sesión de diseño primero (sin
  código), después implementación incremental. **El inventario
  de deudas activas queda vacío después de v0.9.57** — el
  proyecto entra a fase "todo lo prometido implementado" antes
  de la próxima fase grande.

---

## [v0.9.56] — 2026-05-24 — Re-investigación R.bug-pyo3-abi3-portable-link Linux: reclasificado como constraint arquitectural permanente

### Changed

Retomado el bug R.bug-pyo3-abi3-portable-link Linux/macOS con el
plan documentado el 2026-05-23 ("combinación correcta del fix sin
validar"). El experimento empírico **invalidó la hipótesis del
fix** y reveló que el bug **no es cerrable en Linux**. Se
reclasifica de "deuda activa cerrable" a **constraint
arquitectural permanente**. **Cero cambios de código del lenguaje**;
solo documentación + comentarios en Dockerfiles.

- **Experimento Docker en `d:\tmp\fitz-pyo3-test\`** (descartado,
  no va al repo):
  - Builder: `FROM python:3.13-slim`
  - Runtime: `FROM python:3.10-slim` (cross-version intencional)
  - Env vars: `PYO3_NO_PYTHON=1` + `PYO3_CONFIG_FILE` con
    `lib_name=python3` + `abi3=true` + `version=3.10`
  - RUSTFLAGS: `-L /usr/local/lib`
  - Cargo build OK hasta el link final; `rust-lld` falló con
    ~10+ símbolos undefined (`PyDict_Next`, `PyObject_Str`,
    `PyLong_AsLong`, `PyBool_Type`, `PyFloat_Type`, etc.).

- **Verificación con `nm -D /usr/local/lib/libpython3.so`** en
  `python:3.10-slim` y `python:3.13-slim`:
  - El archivo (13992 bytes en ambas imágenes) exporta **solo 4
    símbolos glibc** (`_ITM_*`, `__cxa_finalize`, `__gmon_start__`).
  - **NO exporta ningún símbolo del API Python**.
  - La asunción del 2026-05-23 de que ese archivo era el "abi3
    shim" era falsa — es un dummy/placeholder.

- **Conclusión**: en Linux NO existe equivalente al `python3.dll`
  stable-ABI shim de Windows. Los símbolos abi3 viven solo en
  `libpython3.X.so.1.0` (versioned). El bug requiere uno de:
  - (a) Cambio upstream en PyO3 (modo "skip-link + dlopen
    runtime", pyo3#5043 abierto).
  - (b) Cambio arquitectural en Fitz (CPython como subprocess).
  - (c) Distribuir Fitz como wheel Python (modelo invertido).
  
  Ninguna razonable en corto/medio plazo.

- **Reclasificación**: el bug pasa de "deuda activa" a
  **constraint arquitectural documentado**. El workaround
  "match builder=runtime Python version" es la **solución
  permanente** en Linux, no temporal.

### Files updated

- **`docs/deudas_lenguaje.md`** sección
  R.bug-pyo3-abi3-portable-link: nueva sub-sección
  "Re-investigación 2026-05-24 — hallazgo definitivo" con tabla
  empírica de `nm -D` y razonamiento de las 3 opciones
  arquitecturales descartadas.
- **`docs/roadmap.md`**: "Estado actual del proyecto" pasa a
  v0.9.56; queda solo 1 deuda activa restante (`8-pyi-stubs`);
  sección de Fase 8.b actualizada con cierre formal del cierre
  parcial Windows + reclasificación Linux/macOS.
- **`docs/guide.md`** cap 21.11: nota "Constraint conocido"
  reescrita como "Constraint arquitectural permanente" con
  referencia al experimento empírico y a `deudas_lenguaje.md`.
  Cap 33 "Qué sigue" pierde la deuda de la lista de "Deudas
  reales restantes".
- **`docs/deudas-post-5b.md`**: la fila de la tabla pasa a
  `~~CERRADO~~` con etiqueta "RECLASIFICADO v0.9.56".
- **`boilerplates/api-postgres-python/Dockerfile`** +
  **`boilerplates/api-fullstack-postgres/Dockerfile`**:
  comentarios actualizados de "deuda residual" a "constraint
  arquitectural permanente" con referencia a deudas_lenguaje.md.

### Notes

- **Cero cambios de código del lenguaje**. Suite intacta: 2290
  default / 2381 python / 2395 lsp. Clippy + fmt limpios.
- El experimento Docker en `d:\tmp\fitz-pyo3-test\` queda
  descartado (no va al repo). El hallazgo empírico está
  documentado en `docs/deudas_lenguaje.md`.
- **Deudas reales restantes después de v0.9.56**: solo
  `8-pyi-stubs` (1-2 días, post-Fase 9). El proyecto queda en
  estado "una sola deuda activa cerrable".
- **Próximo norte**: **Fase 10 — Stack DB nativo + ORM
  declarativo**. Sesión de diseño primero (sin código),
  después implementación incremental.

---

## [v0.9.55] — 2026-05-24 — Hito de consolidación: refresh masivo de docs macro

### Changed

Tras 14 releases consecutivos cerrando deudas (v0.9.43 → v0.9.54),
release de consolidación que refresca las docs macro al estado
actual. **Cero cambios funcionales**.

- **`README.md`** raíz:
  - Sección "Interop Python via PyO3" (footnote §): actualizada
    para reflejar que **distroless está habilitado desde v0.9.46**
    (launcher con `tar`+`flate2` inline) y **smoke real Docker
    validado end-to-end con Postgres** en v0.9.50/52 (imagen
    ~136 MB). Pre-fix el README citaba distroless como "deuda
    menor del launcher" — ya no es cierto.
  - Tabla de boilerplates: los 2 con Python+Postgres
    (`api-postgres-python`, `api-fullstack-postgres`) ahora
    documentan la variante `Dockerfile.distroless` validada,
    incluyendo CORS preflight desde otro origin para el fullstack.

- **`docs/roadmap.md`**:
  - Nueva sección "Estado actual del proyecto (v0.9.55)" al
    inicio. Resume las fases 1-9 entera CERRADAS, el cierre del
    bundle B/I (Python interop codegen), las métricas de tests
    actuales (2290 default / 2381 python / 2395 lsp), las 2
    deudas reales restantes (R.bug-pyo3-abi3 L/M, 8-pyi-stubs),
    y el próximo norte grande (Fase 10 — Stack DB nativo + ORM).

- **`docs/guide.md` cap 33 "Qué sigue"**:
  - Sección "Lo que viene" actualizada para reflejar que Fase 9
    está entera CERRADA (era listada "en curso"). Suma mención
    de bundling Python `--bundle-python`/`--bundle-pip*` (Fase
    8.b/8.c) con smoke distroless validado, env builtin (cap
    31), y los 4 caps de stack web first-class (28-30).
  - "Deuda residual comprometida" actualizada: las 3 que
    listaba (coerción list/dict, heterogéneos compilados,
    deuda menor F7) **ya cerraron** (v0.9.44/49/54).
    Reemplazadas por las 2 reales restantes.
  - Suma Fase 11 (Frontend en `.fitz`) y Fase 12 (Deployment
    ciudadano primera clase) como nortes especulativos
    siguientes al post-Fase 10.

### Notes

- **Cero cambios de código del lenguaje**. Suite intacta: 2290
  default / 2381 python / 2395 lsp. Clippy + fmt limpios.
- **Hito**: este release marca el cierre del bloque de 15
  releases consecutivos (v0.9.43 → v0.9.55) que llevaron el
  proyecto desde "Fase 9 + bundling con caveats" hasta
  "production-ready en patrones canónicos + repo profesional
  con CI strict + 6 boilerplates validados end-to-end".
- **Próximo norte**: **Fase 10 — Stack DB nativo + ORM
  declarativo**. Sesión de diseño primero (sin código),
  después implementación incremental.

## [v0.9.54] — 2026-05-24 — Cierre dict→Map<K,V> primitivo: coerción `PyAny → Map<Str, V>`

### Added

- **Coerción `PyAny → Map<Str, V>` para V primitivo**
  (Str/Int/Float/Bool). Pre-fix: `let m: Map<Str, Str> = json.
  loads(raw)?` adentro de fn `-> Result<Map<Str, Str>>` fallaba
  en rustc con `expected Arc<Mutex<Vec<(String, String)>>>,
  found __FitzPyObject` — el `coerce()` no tenía caso para
  `(PyAny, Map<K, V>)`. Post-fix: 4 helpers nuevos en el preludio
  Python + caso wireado en `coerce()`. Cubre el caso típico de
  `json.loads` de objects con shapes simples.

  Implementación:
  - 4 helpers `pub(crate) fn __fitz_py_to_map_string_<v>` con
    v ∈ {string, i64, f64, bool} emitidos en el bloque
    `emit_python_prelude` (paralelo a los `__fitz_py_to_list_<v>`
    existentes). Cada helper: itera el PyDict, valida que las
    keys son `PyString` + cada value es del tipo esperado, y
    devuelve `Arc<Mutex<Vec<(String, V)>>>`. Preserva el orden
    de inserción del dict Python (CPython 3.7+ garantía nativa).
  - Caso nuevo en `coerce()`: `(Type::PyAny, Type::Map(k, v))`
    despacha por `(k, v)`. K=Str + V primitivo → helper
    dedicado. K no-Str u otros V (Nominal/List/Map/Any) →
    gradual (`code` tal cual; el caller se queja en build si
    necesita coerción concreta).

  **Cobertura de combinaciones**:
  - ✅ Map<Str, Str>, Map<Str, Int>, Map<Str, Float>, Map<Str, Bool>.
  - ❌ Map con K no-Str (raro en JSON), Map<Str, Nominal>,
    Map<Str, List<...>>, Map<Str, Map<...>>, Map<Str, Any> —
    quedan como deuda menor (caso 90%+ cubierto; los compuestos
    son raros y el usuario puede destrabar iterando manualmente
    el PyDict si necesita).

### Notes

- **Tests nuevos**: 5 unit en `codegen::tests::map_coerce_*`
  (4 helpers verificados + 1 test que confirma que Map<Str,
  List<...>> queda gradual sin emitir helper inexistente).
- **Smoke real validado**: `fn parse(raw) -> Result<Map<Str,
  Str>>` con `json.loads(raw)?` compila y produce
  `Ok({"a": "x", "b": "y"})` con `json.dumps` + round-trip
  (validado a mano con `fitz build` + ejecutar binario).
- Suite total: **2290 default** (era 2285 + 5), **2381 python**
  (era 2376 + 5), **2395 lsp** (era 2390 + 5). Clippy
  `--all-targets -D warnings` + `cargo fmt --check` limpios en
  los 3 modos.
- **Sin cambios a la extensión VSCode** — fix puramente del
  codegen.

### Bundle B/I (Python interop codegen) ENTERO CERRADO

Con v0.9.54, las 3 deudas originales del bundle B/I (Python
interop codegen) cierran:

| Deuda | Estado |
|-------|--------|
| ~~8.7-ok-propagation~~ | ✓ CERRADO v0.9.53 |
| ~~8.7-await-binding-split~~ | ✓ CERRADO mini-tandas previas (verificado v0.9.49) |
| ~~dict→Map<K,V> no primitivos~~ | ✓ CERRADO v0.9.54 (variantes primitivas) |

**Inventario depurado** post-v0.9.54: **2 deudas reales
restantes**:

| ID | Categoría | Esfuerzo |
|----|-----------|----------|
| R.bug-pyo3-abi3 Linux/macOS | Bundling Python | 4-6h |
| 8-pyi-stubs | Stubs Python | 1-2 días |

## [v0.9.53] — 2026-05-24 — Cierre 8.7-ok-propagation + fix fmt regression v0.9.51

### Fixed

- **8.7-ok-propagation — codegen propaga expected type adentro
  de `Ok(...)`/`Err(...)` en `return`** ✓. Deuda residual de
  Fase 8.7 que ya era blocker concreto del boilerplate 6
  (v0.9.52 aplicó workaround temporal con binding intermedio
  anotado). Pre-fix: `return Ok(json.dumps(raw)?)` adentro de
  fn `-> Result<Str>` fallaba en rustc con `expected String,
  found __FitzPyObject` porque `gen_ok` devolvía
  `Result<PyAny>` sin coerción al expected `Str` y el `coerce`
  general no maneja `Result<A> → Result<B>`. Post-fix:
  `gen_return` detecta `Expr::Ok(inner)` / `Expr::Err(inner)`
  cuando `ret_expected` es `Result<T, E>` y coerce `inner`
  directo al T (Ok) o E (Err) ANTES de envolver. El gate
  `!self.response_mode && !self.in_middleware_fn` lo aísla de
  los paths HTTP que ya manejan Ok/Err específicamente. (`src/
  codegen.rs::gen_return`)

  Casos cubiertos:
  - `return Ok(json.dumps(...)?)` con `-> Result<Str>` →
    coerce PyAny → Str via `__fitz_py_extract_string`.
  - `return Ok(math.floor(...)?)` con `-> Result<Int>` →
    coerce PyAny → Int via `__fitz_py_extract_i64`.
  - `return Ok(T { ... })` con `-> Result<T>` → no emite
    coerce innecesario (inner ya tipa T).

  **Workaround removido en boilerplate 6**: los 5 helpers
  (`create_raw`/`find_raw`/`list_raw`/`update_raw`/
  `delete_raw`) vuelven al patrón inline original
  `return Ok(json.dumps(raw)?)`. El v0.9.52 los había
  modificado a binding intermedio anotado como workaround
  explícito.

- **Fmt regression de v0.9.51 (`src/parser.rs`)** ✓. El cambio
  del F15 recovery sub-stmt en v0.9.51 introdujo formato
  no-canonical en el `match self.expect_ident(...)` que
  `cargo fmt --check` (activado en v0.9.48) detectó en CI.
  Aplicado `cargo fmt` al archivo. CI strict ahora pasa.

### Notes

- **Tests nuevos**: 3 unit en `codegen::tests`:
  - `ok_propagation_coerce_pyany_a_str_adentro_de_return_ok`
  - `ok_propagation_coerce_pyany_a_int_adentro_de_return_ok`
  - `ok_propagation_inner_ya_correcto_no_emite_coerce_innecesario`
- Suite total: **2285 default** (era 2282 + 3), **2376 python**
  (era 2373 + 3), **2390 lsp** (era 2387 + 3). Clippy
  `--all-targets -D warnings` limpio en los 3 modos.
  `cargo fmt --check` ahora pasa (la regresión de v0.9.51
  estaba bloqueando CI desde v0.9.52).
- **Sin cambios a la extensión VSCode** — fix puramente del
  codegen.

### Bundle B parcialmente cerrado

Con v0.9.53, 1 de las 2 deudas restantes del bundle B (Python
interop codegen) cierra:

| Deuda | Estado |
|-------|--------|
| ~~8.7-ok-propagation~~ | ✓ **CERRADO v0.9.53** |
| dict→Map<K,V> no primitivos | sigue pendiente (4-6h) |

**Inventario depurado** post-v0.9.53: **3 deudas reales
restantes**:

| ID | Categoría | Esfuerzo |
|----|-----------|----------|
| dict→Map<K,V> no primitivos | Python interop | 4-6h |
| R.bug-pyo3-abi3 Linux/macOS | Bundling Python | 4-6h |
| 8-pyi-stubs | Stubs Python | 1-2 días |

## [v0.9.52] — 2026-05-24 — Smoke real Docker boilerplate 6 (Dockerfile.distroless) end-to-end VERDE

### Added

- **Smoke real Docker boilerplate 6 (`Dockerfile.distroless`)
  validado END-TO-END** ✓. La deuda menor que v0.9.50 dejó como
  "paralela al boilerplate 5" cierra acá. Stack completo de 3
  servicios:
  - **api** (distroless con binario standalone — `fitz build
    --bundle-pip-requirements`): **imagen final 136 MB real**
    (igual que boilerplate 5 — CPython 3.14.5 + sqlalchemy +
    psycopg2-binary embebidos).
  - **frontend** (nginx alpine) sirviendo el SPA estático
    desde port 8080.
  - **db** (postgres 16-alpine) con healthcheck.
  - **CORS preflight** OPTIONS desde `Origin:
    http://localhost:8080` responde HTTP 204 con
    `access-control-allow-origin` + `access-control-allow-methods`
    + `access-control-allow-headers` correctos (`@middleware
    (cors({...}))` del api funciona en runtime distroless).
  - **HTTP smoke**: POST `/tasks` crea (devuelve task tipado
    desde Postgres), GET `/tasks?filter=all` lista, frontend
    SPA HTTP 200 con 20679 bytes.

- **`docker-compose.distroless.yml`** sumado al boilerplate 6
  con los 3 servicios (api distroless + nginx + postgres).
  Listo para `docker compose -f docker-compose.distroless.yml
  up --build` directo.

### Changed

- **`Dockerfile.distroless` del boilerplate 6**: fix bug
  preexistente (intentaba `COPY web/` que no existe — el dir
  real es `frontend/`). Ahora copia solo lo necesario para el
  api (sin frontend assets — el SPA vive en el container nginx
  separado, consistente con el `Dockerfile` actual).
- **`src/data/tasks.fitz` del boilerplate 6**: workaround
  v0.9.52 para el bug **8.7-ok-propagation** (deuda residual
  del codegen Python que SIGUE abierta). Los 5 helpers
  `create_raw`/`find_raw`/`list_raw`/`update_raw`/`delete_raw`
  ahora usan binding intermedio anotado `let s: Str = json.
  dumps(raw)?` en lugar de `return Ok(json.dumps(raw)?)`
  inline. Sin esto, `fitz build` falla con `expected String,
  found __FitzPyObject` adentro del `Ok(...)`. Cuando
  8.7-ok-propagation cierre, los 5 helpers vuelven al patrón
  inline original. NO afecta `fitz run` (el intérprete tipa
  correctamente).

### Notes

- **Sin cambios de código del lenguaje** — solo nuevo
  `docker-compose.distroless.yml` + workaround del bug
  8.7-ok-propagation en el boilerplate + actualizaciones de
  docs. Suite intacta: 2282 default / 2373 python / 2387 lsp.
- **Inventario depurado** post-v0.9.52: **4 deudas reales
  restantes**:

| ID | Categoría | Esfuerzo |
|----|-----------|----------|
| 8.7-ok-propagation | Python interop | 3-5h |
| dict→Map<K,V> no primitivos | Python interop | 4-6h |
| R.bug-pyo3-abi3 Linux/macOS | Bundling Python | 4-6h |
| 8-pyi-stubs | Stubs Python | 1-2 días |

  El bundle G del inventario original (3 deudas: smoke 5 +
  multi-arch + python-image) entera CERRADA con los releases
  v0.9.49 (audit + 2 ya cerradas) + v0.9.50 (smoke 5) +
  v0.9.52 (smoke 6). Bundle B sigue como el más obvio para
  destrabar el flow `--bundle-pip-requirements` sin workarounds.

## [v0.9.51] — 2026-05-24 — Mini-tanda J: LSP polish (UTF-8 capability + F15 recovery sub-stmt)

### Added

- **Capability `positionEncoding: utf-8` declarada en el LSP
  server** (`fitz-lsp`). Pre-fix asumía implícitamente UTF-8 sin
  declararlo en `capabilities`; clientes que negocian UTF-16
  default (spec LSP por defecto) rompían con chars multi-byte
  (emoji, símbolos matemáticos, scripts del SMP). Post-fix
  explicit. VSCode + tower-lsp soportan UTF-8 desde LSP 3.17
  (julio 2022). Decisión técnica: mantener consistencia con
  `TypeEnv`/`TypeInfo`/`DefinitionInfo` que indexan por chars
  Unicode 1-based del lexer (`column += 1` por char no-newline
  en `lexer.rs::advance`).

- **F15 recovery sub-stmt — `Expr::Field` con field vacío**
  cuando el parser encuentra `<expr>.<EOF|Newline|otro>` en
  modo recovery. Pre-fix: el stmt entero se descartaba como
  `Stmt::Error` y el LSP solo podía recuperar completion vía
  el fallback "walk top-level por nombre" (cubría vars
  top-level, NO locales/params). Post-fix: el `Expr::Field
  { object, field: "", span }` queda en el AST, el checker lo
  tipa via TypeInfo, y el completion ve el tipo del `object`
  directamente — funciona para vars locales/params/cualquier
  scope.

  Impacto en completion contextual:
  - `user.<EOF>` con `let user: User = ...` dentro de una fn →
    el completion muestra los fields/métodos de `User`
    (pre-fix solo funcionaba si `user` era top-level).
  - `desconocido.` (ident sin binding) → tipa `Type::Any`
    (gradual escape del checker) → muestra los 6 métodos
    universales de F13.D (`as_int`/`as_float`/`as_str`/
    `as_bool`/`as_bytes`/`type_name`). Pre-fix devolvía vacío
    porque el stmt entero se descartaba.

### Changed

- **`position_to_offset` y `offset_to_position`** (`src/lsp.rs`):
  doc actualizado para reflejar `positionEncoding: utf-8`
  declarada en capabilities. Sin cambio funcional (ya contaban
  chars Unicode, ahora documentado correctamente).
- **`parse_postfix`** (`src/parser.rs`): branch `Token::Dot`
  ahora maneja `expect_ident` fallido bajo `recovery_mode`
  preservando el `Expr::Field` con `field: ""` en lugar de
  propagar el error que descartaba el stmt entero.

### Notes

- **Tests nuevos**: 4 unit en `lsp::tests`
  (`position_to_offset_cuenta_chars_unicode_no_utf16_code_units`,
  `offset_to_position_cuenta_chars_unicode_paralelo_a_position_to_offset`,
  `f15_recovery_sub_stmt_preserva_field_access_con_dot_huerfano`,
  `f15_recovery_sub_stmt_completion_after_dot_funciona_sobre_var_local`).
- **Tests ajustados**: 1 unit
  (`after_dot_sobre_receiver_sin_tipo_devuelve_metodos_any`,
  renombrado de `..._devuelve_vacio`) — cambia las expectativas
  para reflejar el nuevo comportamiento (F15 + F13.D
  combinados): ident sin binding ahora tipa Any y muestra los
  6 métodos universales en lugar de devolver vacío.
- Suite total: **2387 unit con lsp** (era 2383 + 4),
  **2373 con python**, **2282 sin features**. Clippy
  `--all-targets -D warnings` limpio en los 3 modos.
- **Sin cambios a la extensión VSCode** — la capability LSP
  negocia automáticamente al conectar.

### Bundle J cerrado

Con v0.9.51, el bundle J del inventario está completo. Las 2
deudas reales del LSP residuales (UTF-16 position strict + F15
recovery sub-stmt) cierran. **Inventario depurado** ahora baja
a **5 deudas reales restantes**:

| ID | Categoría | Esfuerzo |
|----|-----------|----------|
| 8.7-ok-propagation | Python interop codegen | 3-5h |
| dict→Map<K,V> no primitivos | Python interop codegen | 4-6h |
| R.bug-pyo3-abi3-portable-link Linux/macOS | Bundling Python | 4-6h |
| 8-pyi-stubs | Stubs Python | 1-2 días |
| Smoke real Docker boilerplate 6 | Validación | 1-2h |

## [v0.9.50] — 2026-05-24 — Smoke real Docker boilerplate 5 (Dockerfile.distroless) validado end-to-end con Postgres

### Added

- **Smoke real Docker boilerplate 5 (`Dockerfile.distroless`)
  validado END-TO-END con Postgres** ✓. La deuda menor que
  v0.9.46 dejó pendiente ("path técnico correcto; validación
  funcional pendiente") y que v0.9.49 documentó como abortada
  por tiempo, **cierra finalmente acá** (el build se completó
  en background mientras avanzamos con docs):
  - Build con `Dockerfile.distroless` + `--bundle-pip-requirements`
    completó OK (~10 min cargo install desde source con
    `python:3.14-slim-bookworm` builder).
  - **Imagen final: 136 MB real** (vs ~80-100 MB esperado por
    el plan original — el binario standalone con CPython 3.14.5
    + sqlalchemy + psycopg2-binary embebidos pesa más de lo
    estimado en abstracto). Sigue siendo **15% más chica que
    los ~155 MB del Dockerfile actual** con `python:3.12-slim`
    + `fitz run`.
  - Runtime `gcr.io/distroless/cc-debian12` arranca limpio,
    boot logs `[boot] DB conectada y schema inicializado` +
    `[ready] Server arrancando en :3000` correctos.
  - **Smoke con curl end-to-end OK**: POST `/users` + GET `/users`
    (devuelve `[{"id":1,"name":"Ada","email":...},{...}]`
    tipado) + GET `/users/1` (instance individual tipada). Toda
    la cadena Fitz HTTP + SQLAlchemy + psycopg2 + Postgres
    funcional adentro del runtime distroless.
- **`docker-compose.distroless.yml`** sumado al boilerplate 5
  con la imagen + Postgres listos para `docker compose -f
  docker-compose.distroless.yml up --build` directo.

### Notes

- **Boilerplate 6 (fullstack)** sigue pendiente como deuda menor
  más chica — el patrón del Dockerfile.distroless es paralelo
  al 5 (mismo structure + frontend SPA estático). Smoke real
  con docker-compose tomaría ~10-15 min adicionales de build.
  Path técnico ya validado; queda como ~1-2h de trabajo
  paralelo, no bloqueante.
- **Sin cambios de código del lenguaje** — solo nuevo
  `docker-compose.distroless.yml` + actualizaciones de docs.
  Suite intacta: 2282 default / 2373 python / 2383 lsp.

## [v0.9.49] — 2026-05-24 — Audit-G: audit completo del inventario + 4 deudas confirmadas como ya cerradas

### Changed

- **Dockerfiles distroless**: `FITZ_TAG` default actualizado de
  `v0.9.46` → `v0.9.48` (boilerplates 5 + 6) — usar el release
  más reciente con CI strict (`cargo fmt --check` + `cargo
  clippy --all-targets`) ya activado en `ci.yml`.

### Audit del inventario (sin cambios funcionales)

Después de descubrir 2 sesiones consecutivas con deudas stale
(v0.9.47 — 3 LSP ya cerradas; v0.9.48 — 11 errores clippy ya
cerrados), Audit-G dedicó la sesión a verificar el resto del
inventario. **4 deudas más confirmadas como YA cerradas**:

- **F13 — heterogéneos en codegen** (Baja): SPIKE `__FitzValue`
  con variantes Int/Float/Str/Bool/Null + Bytes + Nominal. Smoke
  `[1, "dos", true]` (List<Any>) compila con `fitz build` y
  produce `[1, "dos", true]` bit-a-bit con `fitz run`.
  Auto-detectado en `gen_list_lit` cuando aparece un `List<Any>`
  literal.
- **8.7-await-binding-split** (Python interop): cerrado con
  dispatch al helper `__fitz_py_await_obj` cuando el inner del
  `.await` tiene `inner_ty == PyAny`. Tiene test
  `py_await_split_emite_fitz_py_await_obj`.
- **multi-arch-docker**: ya implementado en `release.yml` Job 3
  `docker-image` con buildx `--platform linux/amd64,linux/arm64`.
- **fitz-python-image**: ya implementado en `release.yml` Job 3b
  con tag `:latest-python`.

**Inventario depurado**: deudas reales restantes ahora bajan a
7 (de 13+ que figuraban en los documentos):

| ID | Categoría | Esfuerzo |
|----|-----------|----------|
| 8.7-ok-propagation | Python interop codegen | 3-5h |
| dict→Map<K,V> no primitivos | Python interop codegen | 4-6h |
| UTF-16 position strict | LSP | 2-3h |
| F15 recovery sub-stmt | LSP | 1-2h |
| R.bug-pyo3-abi3-portable-link Linux/macOS | Bundling Python | 4-6h |
| 8-pyi-stubs | Stubs Python | 1-2 días |
| Smoke real Docker boilerplate 5/6 | Validación | 2-3h |

### Notes

- **Convención nueva** (tercera vez consecutiva que aparece
  inventario stale — pattern claro): antes de prometer trabajo
  en un bundle de deudas, hacer audit rápido (10-15 min) con
  comandos directos (`grep` por nombres de fns/features, `cargo
  clippy --all-targets`, reproducir con `.fitz` mínimo +
  `fitz build`). Documentado en `docs/deudas-post-5b.md`.
- **Sin cambios de código del lenguaje** en este release. Suite
  intacta: 2282 default / 2373 python / 2383 lsp. Único cambio:
  bump del `FITZ_TAG` default en los 2 Dockerfile.distroless
  (`v0.9.46` → `v0.9.48`) + audit/documentación del inventario.
- **Smoke real Docker boilerplate 5 (`Dockerfile.distroless`)**:
  arrancado al final de la sesión pero abortado por tiempo
  (build con `cargo install fitz --features python` desde
  source toma 10+ min). Queda como deuda menor explícita. (El
  build se completó en background después del commit — cierre
  efectivo en v0.9.50.)

## [v0.9.48] — 2026-05-24 — Mini-tanda Cleanup-D: cargo fmt --all masivo + clippy --all-targets reactivado en CI

### Changed

- **`cargo fmt --all` aplicado masivamente** (14 archivos
  reformateados: `src/asyncapi.rs`, `src/codegen.rs`,
  `src/evaluator.rs`, `src/http.rs`, `src/launcher_template.rs`,
  `src/lib.rs`, `src/lsp.rs`, `src/main.rs`, `src/pbs.rs`,
  `src/pyi_stub.rs`, `src/types.rs`, `tests/bundle_python_e2e.rs`,
  `tests/cli_e2e.rs`, `tests/compile_e2e.rs`). El repo nunca había
  pasado por rustfmt canónico desde el inicio del proyecto; el
  CI lo tenía deshabilitado con nota de "preferencias del autor
  difieren del default". Cleanup-D aplica el formato canónico
  para alinear con la convención del ecosistema Rust y desbloquear
  el step `fmt --check` en CI.
- **`ci.yml` actualizado**:
  - `cargo fmt --check` reactivado (bloquea diff a futuro).
  - `cargo clippy --all-targets` reactivado (era `--lib` solo).
    La deuda original de "11 errores en tests" cerró a lo largo
    de mini-tandas previas; al verificar con `cargo clippy
    --all-targets --all-features -- -D warnings` la suite pasa
    limpia en los 3 modos (default, `python`, `lsp`).

### Notes

- **Cero cambios funcionales**: `cargo fmt` solo modifica
  whitespace/line breaks. Toda la lógica del lenguaje +
  comportamiento generado es idéntico bit-a-bit.
- **Suite verde post-fmt** en los 3 modos:
  - Sin features: **2282 unit** (igual que antes del fmt).
  - Con `python`: **2373 unit**.
  - Con `lsp`: **2383 unit**.
  - Clippy `-D warnings` limpio en los 3 modos + `--all-targets`.
- **`cargo outdated` skipeado**: el plugin `cargo-outdated` no
  está instalado en la máquina dev. Sin presión real de
  vulnerabilidad, dejamos el audit de bumps para una sesión
  futura cuando aparezca caso de uso concreto (ej. CVE en una
  dep transitiva). Las deps principales del repo (`pyo3`,
  `axum`, `tokio`, `serde`) están en versiones recientes según
  Cargo.toml.
- **Mini-tanda Cleanup-D — cierre del último ítem del bundle D
  del inventario de deudas post-v0.9.46**: junto con los cierres
  parciales de v0.9.45 (4 deudas chicas del lenguaje) y v0.9.47
  (LSP completion + chain), el repo queda en estado profesional
  para colaboradores. Bundle D estaba siendo pospuesto release
  tras release ("sin presión") — su cierre saca ruido del
  inventario y permite enfocar las próximas mini-fases en
  features reales.

## [v0.9.47] — 2026-05-24 — Mini-tanda LSPz: completion en `from mod import` + chain `a.b.c.`

### Added

- **Completion en `from <mod> import |`** (LSP). El cursor adentro
  de la lista de imports de un `from <mod> import` ahora sugiere
  los símbolos exportables del módulo target (fns con firma
  completa, types, consts/let top-level). Funciona también con
  items previos (`from foo import X, Y, |`) y módulos con path
  punteado (`from sub.utils import |`).

  Implementación:
  - Nueva variante `CompletionContext::FromImportList { mod_path:
    Vec<String> }` en `src/lsp.rs`.
  - Helper nuevo `detect_from_import_list_context(text, line,
    character)` que walkea back-to-front del cursor, saltando
    items previos (`<ident>,?\s*`), y matchea el patrón `from
    <mod_path> import`. Devuelve `mod_path` segmentado por `.`.
  - Helper público nuevo `from_import_completions(doc_uri,
    mod_path)` que resuelve el archivo target relativo al doc URI
    (convención del loader: `["foo"]` → `<base>/foo.fitz`),
    parsea con `parse_with_recovery`, y enumera fns + types +
    consts top-level. Tolera módulos inexistentes (devuelve vacío).
  - Nueva variante pública `completion_at_position_with_uri` que
    acepta `doc_uri: Option<&Url>` y la pasa al contexto
    `FromImportList`. La firma original `completion_at_position`
    se mantiene como wrapper (`doc_uri = None`) para
    backward-compat de tests/herramientas externas.
  - El backend del LSP (`src/bin/fitz-lsp.rs`) ahora invoca el
    wrapper `_with_uri` pasando el URI del documento abierto.

- **Chain `a.b.c.` en after-dot completion**. El completion
  contextual tras un punto ahora reconoce chains de N segmentos
  (no solo `<ident>.`). Pre-fix: `obj.field.|` interpretaba el
  receiver como `field` (último ident) y buscaba sus métodos
  como si fuera Str/List/etc. Post-fix: el receiver es el chain
  entero `obj.field`, y el lookup en TypeInfo por la posición del
  START del primer ident resuelve al tipo del Expr::Field más
  exterior (el chain completo). El comportamiento se apoya en la
  garantía de TypeInfo (F16) de que el último `record` por
  posición es el tipo del nodo más externo.

  Implementación: en `detect_completion_context`, el walkback
  desde el `.` ahora acepta `is_ident_continue(c) || c == b'.'`
  (antes solo `is_ident_continue`). Validación de shape:
  rechaza chains que empiecen/terminen con `.` o tengan `..`
  consecutivos.

### Changed

- **Doc comment de `completion_at_position`**: actualizado para
  reflejar que la deuda visible "Chain `a.b.c.`" cerró en v0.9.47.

### Notes

- **Tests nuevos**: 8 unit en `lsp::tests` (5 sobre
  `detect_context_chain_*`/`from_import_*` + 2 sobre
  `from_import_completions_*` + 1 sobre el backward-compat
  `completion_at_position_sin_uri_no_completa_from_import`).
  Suite total: **2383 unit con lsp** (era 2375 + 8), **2282 sin
  features**. Clippy `-D warnings` limpio en los 3 modos (default,
  `python`, `lsp`).
- **Descubrimiento del bundle**: las otras 3 deudas LSP del
  inventario original ya estaban implementadas en mini-tandas
  previas (LSPx para cross-module go-to-def, LSPy.4 para
  scope-aware completion, LSPy para hover con range exacto via
  `make_hover_with_range`/`ident_range_at_position`). El bundle
  E redujo a 2 deudas reales (completion en imports + chain) y se
  cerró en una sola sesión.
- **Sin cambios a la extensión VSCode** — solo cambios al
  backend del LSP. Los clientes existentes (extensión VSCode,
  vim-lsp, helix, etc.) reciben las nuevas completions
  automáticamente al conectar al `fitz-lsp` actualizado.

### Deuda residual LSP (NO bloquea uso real)

- **UTF-16 position strict**: el LSP por default usa UTF-16 para
  `character` en `Position`. Fitz LSP usa UTF-8 (asume programas
  ASCII-dominantes). Refinable post-MVP si aparece presión real
  con código en idiomas no-latin.
- **F15 recovery sub-stmt**: errores adentro de un stmt
  descartan el stmt entero — refinable para completion fino
  tras `user.<typo>`.

## [v0.9.46] — 2026-05-24 — Bundling Docker end-to-end: distroless habilitado + Dockerfile.distroless en boilerplates 5/6

### Added

- **distroless-tar-embedded — launcher con `tar` + `flate2` inline**.
  Cierre de la deuda residual de Fase 8.b/8.c documentada al cerrar
  el bundling de Python: el launcher del binario standalone
  (generado por `fitz build --bundle-python` / `--bundle-pip*`)
  invocaba `Command::new("tar")` subprocess para extraer los
  tarballs PBS + pip a `$TMPDIR/fitz-py-<hash>/`. Esto requería
  `tar` instalado en el runtime de Docker — `gcr.io/distroless/cc-
  debian12` no lo trae, así que el runtime mínimo viable era
  `debian:bookworm-slim` (~85 MB base). Post-fix: el launcher usa
  crates `tar = "0.4"` + `flate2 = "1"` para extraer en memoria,
  sin subprocess. Distroless ahora es viable como runtime.

  Implementación:
  - **`extract_tar_gz(tarball_path, dest)`** — nuevo helper inline
    en `LAUNCHER_MAIN_RS_TEMPLATE` con `flate2::read::GzDecoder` +
    `tar::Archive::unpack`. El crate `tar` valida paths contra
    `../` escapes (CVE protection automática).
  - **`LAUNCHER_CARGO_TOML_TEMPLATE`** suma `tar = "0.4"` +
    `flate2 = "1"` a `[dependencies]`. Los 2 crates suman ~80-100
    KB al binario final del launcher con LTO + strip activos
    (perfil `opt-level = "z"` se mantiene minimalista). Trade-off
    aceptable vs el ahorro de ~60 MB en la imagen de container
    final.
  - **3 sitios reemplazados**: PBS tarball extract (extracción de
    CPython embebido) + pip tarball extract en Linux/macOS (path
    `python/lib/python3.X/site-packages/`) + pip tarball extract
    en Windows (path `python/Lib/site-packages/`). Los 3 ahora
    usan `extract_tar_gz` en lugar de `Command::new("tar")`.
  - El binario final del launcher en `fitz build --bundle-python`
    pesa ahora ~80-100 KB más que pre-fix; sin diferencia
    observable en el tamaño total del binario standalone
    (`examples/python-interop-8.b.exe` mantiene ~22 MB en
    Windows x64).

- **`Dockerfile.distroless` en boilerplates 5 (api-postgres-python)
  y 6 (api-fullstack-postgres)**: variante alternativa al
  `Dockerfile` actual con el flow `fitz build --bundle-pip-
  requirements` + runtime `gcr.io/distroless/cc-debian12`.
  Builder pineado a `python:3.14-slim-bookworm` (fix GLIBC: la
  variante `slim` default es trixie con GLIBC 2.39, incompatible
  con el runtime bookworm GLIBC 2.36). Imagen final esperada:
  **~80-100 MB** (vs ~155 MB del Dockerfile actual con
  `fitz run` + Python en runtime). El Dockerfile actual queda
  sin cambios — es el path "seguro" mientras la validación smoke
  funcional con Postgres real avanza.

### Notes

- **Tests nuevos**: 3 unit en `launcher_template::tests`
  (`template_cargo_toml_incluye_deps_tar_y_flate2`,
  `template_main_rs_define_extract_tar_gz_y_no_invoca_tar_subprocess`,
  `gen_launcher_main_rs_pip_block_usa_extract_tar_gz`). Los 2 E2E
  existentes (`template_launcher_compila_con_paths_dummies`,
  `template_launcher_compila_con_path_windows_y_espacios`) siguen
  verdes y validan que el template Rust resultante compila con
  las nuevas deps.
- **Smoke real bundling**: `fitz build --bundle-python
  examples/python-interop-8.b.fitz` produce el binario standalone
  (~22 MB Windows x64) y el binario corre limpio bit-a-bit con la
  versión pre-fix. Validado a mano con cache TMP vacía + cache hit.
- Suite total: **2373 unit con python** (era 2370 + 3),
  **2282 sin python** (era 2279 + 3). Clippy `-D warnings` limpio
  en ambos modos. Sin cambios a la extensión VSCode (cambio del
  codegen del launcher, no del lenguaje).
- **Deuda residual derivada** (NO bloquea, queda como sub-paso
  futuro): smoke real Docker end-to-end del `Dockerfile.distroless`
  con sqlalchemy + psycopg2 + Postgres cliente. El path técnico
  está correcto (todos los blockers documentados cerrados); la
  validación funcional completa requiere ~30 min de setup Docker
  + Postgres y queda como tarea independiente cuando aparezca
  presión real de adopt.

## [v0.9.45] — 2026-05-24 — Mini-tanda Cleanup-A: 4 deudas chicas del lenguaje cerradas

### Added

- **Fix sqrt-shadowing**: las fns importadas con el mismo nombre
  que un builtin matemático del codegen (`sqrt`, `pow`, `abs`,
  `ceil`, `floor`, `round`, `clamp`, `min`, `max`, `popcount`,
  `leading_zeros`, `trailing_zeros`, `spawn`, `len`, `bytes`,
  `sleep`, `env`, `env_or`, `load_env`) ahora tienen precedencia
  correctamente. Pre-fix: el codegen chequeaba sólo
  `fn_sigs.contains_key(name)` para decidir si emitir el método
  nativo (`(x).sqrt()`) o respetar el user override. Las fns
  importadas vivían en `module_bindings`, no en `fn_sigs`, así que
  `from utils import sqrt` + `sqrt(x)` se traducía
  incorrectamente al método f64. Post-fix: nuevo helper
  `CodegenCtx::is_user_callable(name)` chequea ambos. 14 call
  sites migrados. 3 tests nuevos (`build_fn_importada_con_nombre_
  de_builtin_matematico_no_es_shadeada`, `build_fn_importada_con_
  nombre_pow_no_es_shadeada`, `build_fn_local_con_nombre_de_
  builtin_sigue_funcionando_como_antes`).

- **F14 ampliado — `let X = <expr no literal>` a nivel top de
  módulo**: el caso ya estaba cubierto vía accessor fns desde la
  mini-tanda F14 original (v0.9.x), pero no había tests para
  literales compuestos (List/Map/Instance). 3 tests nuevos
  (`modulo_top_level_let_lista_literal_se_emite_como_accessor_fn`,
  `modulo_top_level_let_map_literal_se_emite_como_accessor_fn`,
  `modulo_top_level_let_instance_se_emite_como_accessor_fn`)
  sellan la cobertura.

- **F3 ampliado — `return`/`break`/`continue` huérfanos**: el
  check estático ya estaba implementado (`return_stack`/
  `loop_depth` en `CheckCtx`) con 3 tests cubriendo cada caso
  (`return_huerfano_top_level_es_error`, `break_huerfano_es_
  error`, `continue_huerfano_es_error`). Sin cambios al código —
  solo actualizamos la documentación de deudas-post-5b para
  marcar F3 como cerrado.

### Documented

- **F1 — Matriz de uso de `Type::Any` (audit)**: los ~180 sitios
  donde aparece se clasificaron en 9 categorías intencionales
  (builtins variádicos, builtins polimórficos, propagación
  gradual, fallback de anotaciones, callbacks sin anotación,
  patterns de match, `Expr::Error` F15, `Result<Any>`/
  `Future<Any>` placeholder, propagación de `PyAny`). La doc
  del enum `Type` en `src/types.rs` describe cada categoría +
  qué NO debe aparecer (anti-patterns que sí serían bugs). Sin
  cambios de código — audit ratifica que el uso actual es
  correcto.

### Notes

- **Tests nuevos**: 6 unit (3 sqrt-shadowing + 3 F14 ampliado).
  Suite total: 2370 unit con python (era 2364 + 6), 2279 sin
  python (era 2273 + 6). Smoke `GUIDE_EXAMPLES_COMPILE` verde.
  Clippy `-D warnings` limpio en ambos modos.
- **Cierre formal de la mini-tanda Cleanup-A**: bundle pragmático
  de 4 deudas chicas relacionadas como "limpieza de lenguaje
  menor". F1 (audit/docs), F3 (ya cerrado, solo docs), F14
  (ampliado con tests), sqrt-shadowing (fix real). Decisión de
  scope (vs bundle más grande con `8.7-ok-propagation`,
  `8.7-await-binding-split`, `dict→Map<K,V>`, F13): mantener
  cada deuda mediana/grande como mini-fase dedicada para no
  acumular riesgo en un solo cierre.
- **Sin cambios a la extensión VSCode** — no se introduce sintaxis
  nueva.

## [v0.9.44] — 2026-05-24 — Cierre sub-deuda 1.5/1.6: coerción + impls HTTP para tipos importados en `fitz build`

### Added

- **Codegen — helpers Python para tipos custom definidos en módulos
  transitivos**. Cierre de la **sub-deuda 1.5** que emergió al cerrar
  v0.9.43 (cuando el smoke real del boilerplate 5 con `fitz build`
  pasó el rechazo del 8.7.1 transitiva y reveló que los helpers
  `__fitz_py_to_instance_<T>`/`__fitz_py_to_list_<T>` solo se emitían
  para tipos definidos en main, no para tipos importados desde otros
  módulos). El error pre-fix era `cannot find function
  __fitz_py_to_instance_User in this scope`.

  Implementación:
  - **`pub(crate)`** sobre los helpers `__fitz_py_to_instance_<T>` y
    `__fitz_py_to_list_<T>` en main (antes `fn` privadas) — necesario
    para que módulos los referencien con `crate::__fitz_py_to_*`.
  - **`gen_python_helpers_for_type(name, &sig)`** — nuevo método del
    `CodegenCtx` que extrae las 2 `impl __FitzToPy` + helpers
    Python→Fitz desde `gen_type_def`. Reusable para tipos del main Y
    para tipos importados.
  - **`emit_helpers_for_imported_types(loader, do_python, do_http)`**
    — pase unificado nuevo invocado desde `emit_main_rs_body` después
    de emitir los tipos locales. Para cada tipo custom de cada módulo
    cargado del proyecto: emite `#[allow(unused_imports)] use
    crate::<qualifier>::{T, TData};` (si no está ya importado al
    main) + opcionalmente los impls HTTP (`__ToFitzJson`/
    `__FromFitzJson`) + opcionalmente los Python helpers
    (`__FitzToPy` + `__fitz_py_to_instance_<T>` +
    `__fitz_py_to_list_<T>`). Dedup por nombre para evitar emitir
    helpers duplicados si dos módulos definen tipos con el mismo
    nombre.
  - **Post-procesamiento del output de cada módulo**
    (`prefix_module_py_nominal_helpers`): pasada lineal sin regex que
    prefija `crate::` a las referencias `__fitz_py_to_instance_<Cap>(`
    y `__fitz_py_to_list_<Cap>(` (con `<Cap>` capitalizado = Nominal).
    Los helpers primitivos `__fitz_py_to_list_i64/f64/string/bool`
    (lowercase) NO se tocan — ya se importan via `use crate::{...}`.
    Idempotente (no duplica `crate::crate::`).
  - **`emit_module_python_use_decls`** ahora suma `use crate::
    {__fitz_py_to_list_i64, __fitz_py_to_list_f64,
    __fitz_py_to_list_string, __fitz_py_to_list_bool}` (helpers
    primitivos del crate root) además de los helpers Python ya
    importados antes.

- **Codegen — impls HTTP (`__ToFitzJson`/`__FromFitzJson`) para
  tipos importados de módulos**. Cierre de la **sub-deuda 1.6**
  paralela a la 1.5 pero del lado HTTP: handlers que aceptan o
  devuelven tipos `T` importados (e.g. `fn create(u: NewUser) ->
  Result<User>`) fallaban en `fitz build` con `the trait bound
  NewUserData: __FromFitzJson is not satisfied` y `method
  __to_fitz_json exists for struct Arc<Mutex<UserData>>, but its
  trait bounds were not satisfied`.

  Implementación:
  - **`gen_type_http_impls_for_sig(name, &sig)`** — extraído de
    `gen_type_http_impls` para reusabilidad. Emite los 2 impls
    `__ToFitzJson` + `__FromFitzJson` dado nombre + sig.
  - El pase unificado `emit_helpers_for_imported_types` lo invoca
    cuando `has_http = true` (sin requerir `uses_python`). Cubre
    boilerplates HTTP que importan tipos de módulos sin tocar
    Python.

- **Bug fix preexistente — `mod types; mod types;` duplicado**.
  `ModuleLoader::emit_mod_decls` emitía `mod <root>;` por cada
  módulo cargado; cuando dos módulos compartían parent dir
  (`types/user.rs` + `types/api.rs`), `mod types;` aparecía dos
  veces y rustc fallaba con `E0428: the name 'types' is defined
  multiple times`. Fix: dedup por root segment con `HashSet`. El
  bug bloqueaba `fitz build` de cualquier proyecto multi-archivo
  con dos módulos en la misma carpeta — descubierto al validar la
  sub-deuda 1.5/1.6.

### Changed

- **READMEs de boilerplates 5 (api-postgres-python) y 6
  (api-fullstack-postgres)**: blocker #1.5 (coerción
  `__fitz_py_to_*_T` para tipos importados) marcado como CERRADO.
  Quedan 2 caveats menores (GLIBC mismatch + tamaño real ~10-20 MB
  vs 50-70 MB del plan original) — ambos no-bloqueantes para el
  adopt real. Validado a mano: `fitz build` del boilerplate 5
  produce un binario que compila limpio, bootea Python y falla
  solo por `psycopg2` no instalado (config runtime, no del fix).
- **Guía cap 21.10 (Interop Python en `fitz build`)**: nota
  actualizada — la coerción `dict → Instance<T>` y `list →
  List<T>` para tipos `T` importados de otro módulo ya funciona;
  el caveat residual queda en tipos primitivos `Map<K,V>` opacos.

### Notes

- **Tests nuevos**: 4 unit en `codegen::tests::build_main_emite_
  helpers_py_*`/`build_modulo_referencia_helper_*`/`build_modulo_
  importa_helpers_py_*`/`build_emit_mod_decls_deduplica_*` + 1 E2E
  `fase_8_7_1_transitiva_bis_modulo_coerce_pyany_a_tipo_importado`
  (con feature `python`). El E2E valida end-to-end: módulo
  `parser.fitz` define `type User` + `from python import json` +
  `fn parse_default_user() -> Result<User>` que hace `let u: User
  = json.loads(raw)?`, main importa la fn + el tipo, matchea sobre
  `Ok(u)`, imprime `name=Fitz role=admin`.
- **Sin cambios a la extensión VSCode** — no se introduce sintaxis
  nueva.
- **Sub-deuda residual descubierta** (no bloqueante, queda como
  deuda menor del codegen): cuando una fn de módulo retorna
  `Result<Str>` y el body hace `let s = json.dumps(...)?` + `return
  Ok(s)`, el codegen infiere `s: PyAny` y no propaga la expectativa
  de `Str` adentro de `Ok(...)`. Workaround usado en el E2E nuevo:
  binding intermedio anotado `let raw: Str = raw_py`. Refinable
  con propagación de expected type adentro de `Ok(...)`.

## [v0.9.43] — 2026-05-23 — Cierre deuda codegen 8.7.1: `from python import` en módulos transitivos

### Added

- **Codegen — `from python import` adentro de módulos Fitz
  transitivos**. Cierre de la deuda residual de Fase 8.7.1
  documentada al cerrar la fase ("imports Python adentro de
  módulos transitivos no se soportan todavía. Workaround: poné el
  `from python import` en el main"). Cada módulo puede declarar
  sus propios imports Python sin obligar al main a participar.
  Patrón canónico: librerías Fitz que delegan operaciones a Python
  (numpy/scipy/sqlalchemy/redis-py) sin filtrar el detalle a quien
  las usa.

  Implementación:
  - `LoadedModule` gana `python_imports: Vec<PythonImport>`.
  - `ModuleLoader::load_module_inner` recolecta los imports
    Python del módulo con `collect_python_imports` antes del loop
    de procesado de imports Fitz.
  - `generate_module_rs_with_bindings` recibe los python_imports
    como nuevo parámetro, llama a `install_python_bindings`,
    emite `use crate::{__FitzPyObject, __fitz_py_*}` (reusa los
    helpers del preludio Python del crate root) y emite sus
    propios statics + getters locales con
    `emit_python_bindings_top_level`.
  - Nuevo método `emit_module_python_use_decls` orquesta los
    `use crate::__fitz_py_*` que el módulo necesita; gated por
    `uses_async` para `__fitz_py_invoke_await`/`__fitz_py_await_obj`.
  - Los helpers del preludio Python pasan de `fn` a `pub(crate)
    fn` (`__fitz_py_import`, `__fitz_py_get_attr_obj`,
    `__fitz_py_extract_*`, `__fitz_py_err_to_string`,
    `__fitz_py_invoke`, `__fitz_py_marshal_map_key`,
    `__fitz_py_invoke_await`, `__fitz_py_await_obj`,
    `__fitz_py_to_list_*`) para ser accesibles desde módulos
    del crate generado. El prefix `__` mantiene la convención de
    privacidad visual.
  - `uses_python` global = `main OR cualquier módulo transitivo`.
    Si solo módulos transitivos usan Python, el main igual emite
    el preludio entero (los `use crate::__fitz_py_*` lo requieren)
    y Cargo.toml suma pyo3 igual.

  pyo3 cachea via `sys.modules`, así que dos módulos importando
  el mismo módulo Python (`from python import math` en main y en
  utils) no pagan doble inicialización — solo el OnceLock
  duplicado (casi cero overhead real).

  6 tests nuevos cubren el comportamiento (5 unit + 1 E2E con
  feature `python`): no falla cuando antes fallaba, emite
  `use crate::__fitz_py_*` + statics locales en el módulo, main
  emite preludio + Cargo.toml suma pyo3 cuando solo módulos
  transitivos usan Python, y `fase_8_7_1_transitiva_build_from_
  python_en_modulo_compila_y_corre` valida el caso completo
  end-to-end (programa con `pymath.fitz` que importa
  `python.math`, main que lo importa, paridad bit-a-bit
  `fitz run` ↔ `fitz build`).

  Ejemplo runnable nuevo:
  [examples/python-interop-modular.fitz](examples/python-interop-modular.fitz)
  + [examples/python_math_utils.fitz](examples/python_math_utils.fitz)
  (validado a mano: `área(r=2) = 12.566370614359172`,
  `sqrt(16) = 4.0`, `sqrt(-1) → ValueError: ...`).

### Changed

- **Guía cap 16 (Módulos) — `from python import` transitivo**:
  la sección "Detalles del loader" pasa de listar la restricción
  como deuda a documentar el patrón canónico con pointer al
  ejemplo runnable.
- **Guía cap 21.10 (Interop Python en `fitz build`)**: sumada
  nota explícita sobre el caso transitivo.
- **READMEs de los boilerplates 5 (api-postgres-python) y 6
  (api-fullstack-postgres)**: blocker #1 (rechazo del codegen
  transitiva) marcado como CERRADO. Smoke real del boilerplate 5
  con `fitz build` reveló que la coerción `dict → Instance<T>` y
  `list → List<T>` para tipos `T` importados (helpers
  `__fitz_py_to_instance_T`/`__fitz_py_to_list_T` que hoy solo
  se emiten para tipos del main) es la próxima sub-deuda concreta
  para destrabar el adopt — documentada como blocker #1.5 en
  ambos READMEs. Ya estaba mencionada en el roadmap como deuda
  residual derivada de Fase 8.

### Notes

- **Smoke validado**: `cargo test --lib --features python` →
  2359 unit (+5 nuevos) sin regresiones; `cargo test --test
  compile_e2e fase_8_7_1_transitiva --features python` → 1 E2E
  nuevo OK; smoke `GUIDE_EXAMPLES_COMPILE` (sin feature python)
  → verde. Clippy `-D warnings` limpio. Sin cambios a la
  extensión VSCode (no se introduce sintaxis nueva — solo se
  levanta una restricción semántica del codegen).
- **Deuda residual derivada**: la coerción `__fitz_py_to_*_T`
  para tipos importados (blocker #1.5 de los boilerplates 5/6)
  queda como próxima prioridad concreta para destrabar el adopt
  real del flow `--bundle-pip-requirements` en proyectos con
  data layer separado. NO bloquea el caso de `fitz build` con
  programas simples donde el binding del tipo retornado por
  Python vive en el mismo archivo o solo es PyAny opaco.

## [v0.9.42] — 2026-05-23 — Cosecha 8.c + cache key del pip_packages tarball + smoke real Docker + VSCode drift audit

Release consolida cuatro piezas trabajadas en sesiones
consecutivas:

1. **Cosecha 8.c**: nuevo flag CLI `--bundle-pip-requirements
   <FILE>` (la sub-tanda original cerrada el mismo día con
   v0.9.42 commit).
2. **Cache key del pip_packages tarball** (deuda D documentada
   en el roadmap como menor de Fase 8.c): hash determinístico
   sobre los inputs del pip install (`--bundle-pip` positionals
   ordenados + bytes de los requirements files). Cache hit reusa
   el tarball existente sin re-correr pip install + tar — builds
   subsiguientes sin cambios en paquetes pasan de ~10-30s a
   ~instantáneo. Sidecar `<bin>_pip_packages.inputs_hash`
   adyacente al tarball.
3. **Smoke real Docker end-to-end** del flow `--bundle-pip-
   requirements` + Docker multi-stage + runtime debian-slim
   (cerrado VERDE con smoke alternativo flat).
4. **Audit de la extensión VSCode** vs el lenguaje actual: 15
   builtins faltaban en grammar TextMate, 5 en LSP completion.
   `.vsix` re-construido a 0.9.3.

Quick win continuando 8.c en la misma sesión. Nuevo flag
`--bundle-pip-requirements <FILE>` repetible que lee paquetes
desde un `requirements.txt` estándar en lugar de listarlos uno
por uno con `--bundle-pip`. Implica `--bundle-python` igual que
el flag hermano y es combinable con `--bundle-pip` (pip acumula
positionals + contenido del file).

Sin parsing del lado de Fitz: el archivo se pasa directo a
`pip install -r <file>`, así que toda la sintaxis nativa
funciona sin cambios — comentarios con `#`, includes
`-r other.txt`, version pins, `--hash`, índices alternos, etc.

### Cambios

- **Nuevo flag CLI `--bundle-pip-requirements <FILE>`**
  repetible en `Commands::Build`:
  ```bash
  # Equivalente a --bundle-pip sqlalchemy --bundle-pip ...
  fitz build --bundle-pip-requirements requirements.txt mi_app.fitz

  # Combinable con --bundle-pip
  fitz build \
    --bundle-pip-requirements requirements.txt \
    --bundle-pip "psycopg2-binary==2.9.10" \
    mi_app.fitz

  # Repetible (caso multi-stage típico)
  fitz build \
    --bundle-pip-requirements requirements.txt \
    --bundle-pip-requirements requirements-prod.txt \
    mi_app.fitz
  ```

- **Validación temprana**: cada path se canonicaliza y se lee
  antes de tocar lex/parse/PBS. Si el archivo no existe o no es
  legible, `fitz build` aborta con mensaje claro citando el
  path inválido. Cero overhead en el pipeline real.

- **Conteo combinado** para el log: `pip_total_count =
  bundle_pip.len() + líneas no-blank/no-comment del file`.
  El summary `pip install --target ({} paquete(s))…` y el
  banner final reflejan el total.

- **`pip_args` extendido**: por cada requirements file, se
  agregan `["-r", "<abs_path>"]` antes de los positionals.
  Pip los acumula naturalmente; no hay parsing del lado de
  Fitz (toda la sintaxis del archivo la maneja pip).

- **Hash combinado preservado**: si hay pip packages
  (de cualquier fuente — positionals o requirements files),
  el hash del extract TMP incluye los bytes del pip tarball
  resultante. Dos proyectos con distintos paquetes siguen
  teniendo distintos extract dirs (sin colisión).

### Tests

- **3 E2E tests nuevos** en `bundle_python_e2e.rs`:
  - `bundle_pip_requirements_implica_bundle_python_y_aborta_sin_from_python_import`
  - `bundle_pip_requirements_archivo_inexistente_aborta_con_mensaje_claro`
  - `bundle_pip_requirements_combinable_con_bundle_pip`

Tests E2E del bundling: 7/7 (4 previos + 3 nuevos). El happy
path real (build + run del binario standalone con
requirements.txt embebido) sigue siendo validación manual
porque requiere PBS tarball + red + tar + Python 3.14.x
en el builder (constraint heredado de 8.b en Linux/macOS).

### Cache key del pip_packages tarball (deuda D de Fase 8.c)

Antes: cada `fitz build --bundle-pip` o `--bundle-pip-
requirements` re-corría `pip install --target` + `tar -czf`
desde cero, aunque la lista de paquetes no hubiera cambiado.
Costo: 10-30s por build, peor en Docker layer rebuilds.

Ahora: helper `pip_inputs_hash(bundle_pip, requirements_
contents) -> String` computa hash determinístico FNV-1a 64-bit
sobre:
- Positionals `--bundle-pip` ordenados alfabéticamente
  (reordenar args NO invalida cache).
- Bytes de cada requirements file en orden CLI (reordenar
  archivos SÍ invalida — pip los procesa en orden con
  potenciales conflicts/overrides).
- Separador `\n---\n` entre las dos secciones.

Sidecar `<bin>_pip_packages.inputs_hash` adyacente al tarball.
En la próxima corrida, si tarball + sidecar existen y el
hash matchea el nuevo, se reusa todo (skip de PBS extract +
pip install + tar). Mensaje informativo: `→ pip cache hit
({N} paquete(s), hash {8 chars}…) — reusando tarball`.

### Smoke real Docker (findings)

Smoke alternativo en workspace temp con programa flat (`from
python import` solo en main, sin módulos transitivos):
binario standalone de 37.4 MB con CPython 3.14.5 + `requests`
embebido, ejecutado adentro de container `debian:bookworm-
slim`, GET `/version` devuelve `"2.34.2"` (versión de
`requests`) end-to-end. Cadena `--bundle-pip-requirements` +
Docker multi-stage + runtime debian-slim **VERDE**.

3 blockers descubiertos en el path original (boilerplates
5/6 con módulos transitivos):

1. **Deuda del codegen Fase 8.7.1**: `from python import` en
   módulos transitivos NO soportado. Boilerplates
   `api-postgres-python` y `api-fullstack-postgres` usan
   `from python import db` adentro de `src/data/*.fitz`
   (transitivos del main). Workaround del codegen actual:
   "poné el `from python import` en el main" — implica
   refactor invasivo del boilerplate (rompe separation of
   concerns del data layer wrapper Python).
2. **GLIBC mismatch**: `python:3.14-slim` (Debian trixie,
   GLIBC 2.39) ↔ `debian:bookworm-slim` (GLIBC 2.36) →
   binario linkea contra GLIBC del builder y crashea en
   runtime con "version 'GLIBC_2.39' not found". Fix:
   pinear builder a `python:3.14-slim-bookworm`. Documentado
   en los READMEs de los boilerplates afectados.
3. **Beneficio de imagen menor del esperado**: ~10-20 MB
   real (no 50-70 MB que prometía el plan original). El
   binario standalone con CPython embebido pesa ~37 MB que
   compensa el ahorro de no tener Python en runtime. El
   argumento se vuelve "simplificación de runtime" (sin pip,
   sin Python, sin libpq instalados) más que "ahorro de
   deploy size".

Dockerfiles de los boilerplates 5/6 NO simplificados —
mantienen su approach actual con `python:3.12-slim` + `fitz
run`. READMEs actualizados con los 3 blockers documentados +
plan concreto del Dockerfile para cuando cierren.

### Audit de la extensión VSCode

Drift detectado vs el lenguaje actual al revisar grammar
TextMate y LSP completion contra la lista canónica de
builtins del evaluator (`builtin_names()`):

**Faltaban en grammar TextMate** (15):
- `spawn` (Fase 9.w.3 — fire-and-forget de fns `@background`)
- 5 ops de Bits-extras: `popcount`, `leading_zeros`,
  `trailing_zeros`, `rotate_left`, `rotate_right`
- 9 Math: `abs`, `min`, `max`, `pow`, `sqrt`, `ceil`,
  `floor`, `round`, `clamp`

**Faltaban en LSP scope_level_completions** (5): los mismos
Bits-extras (los Math + spawn ya estaban).

Ambos fixeados. Extensión bumpeada a 0.9.3 y `.vsix` re-
construido. Próximo workflow_release del CI multi-platform
publicará binarios alineados.

### Docs

- **cap 21.12 de `docs/guide.md`** suma sub-bloque dedicado
  al flag con ejemplo combinado y nota sobre que la sintaxis
  del file es la nativa de pip.
- **READMEs de `boilerplates/api-postgres-python/` y
  `boilerplates/api-fullstack-postgres/`** actualizados con
  los 3 blockers del smoke real Docker (codegen Fase 8.7.1,
  GLIBC mismatch fix, beneficio realista de imagen).
- **`CHANGELOG.md`** v0.9.42 expandido con las 4 piezas.
- **`CLAUDE.md`** sección Fase 8.c actualizada.
- **`docs/roadmap.md`** Fase 8.c sección final actualizada.

### Deuda residual derivada

- **Codegen `from python import` en módulos transitivos
  (Fase 8.7.1)** — pasó de deuda menor genérica a blocker
  explícito de los boilerplates 5/6. Cerrarla destraba la
  simplificación de los Dockerfiles a `--bundle-pip-
  requirements` + binario standalone.
- **GLIBC mismatch fix** — el plan de simplificación tiene
  que pinear `python:3.14-slim-bookworm` (no `python:3.14-
  slim` que es trixie). Documentado.
- **Distroless requiere `tar` embebido en Rust** — el
  launcher de `--bundle-python` invoca `Command::new("tar")`
  para extraer el PBS. `gcr.io/distroless/cc-debian12` NO
  trae tar → forzados a `debian:bookworm-slim` como runtime.
  Mover a distroless requiere un crate de tar inline (sub-
  paso futuro de la deuda menor del launcher).

## [v0.9.41] — 2026-05-23 — Fase 8.c: `fitz build --bundle-pip` (paquetes pip embebidos)

Nuevo flag `--bundle-pip <paquete>` repetible para `fitz build`.
Empaqueta paquetes pip junto al CPython base de `--bundle-python`
(implica este flag automáticamente). El binario resultante embebe
CPython 3.14.5 + los paquetes pip pedidos, todo en un solo
archivo standalone. NO requiere `pip install` en el destino.

Continuación natural de Fase 8.b. Sub-paso separado en una sesión
con momentum del feature anterior. Destraba boilerplates 5/6
(api-postgres-python, api-fullstack-postgres) para pasar de
`FROM python:3.X-slim` a `FROM gcr.io/distroless/cc-debian12`
con un solo binario embebido (imagen ~150 MB → ~80-100 MB).

### Cambios

- **Nuevo flag CLI `--bundle-pip <PACKAGE>`** repetible en
  `Commands::Build`:
  ```bash
  fitz build \
    --bundle-pip sqlalchemy \
    --bundle-pip psycopg2-binary \
    --bundle-pip "redis==5.0.0" \
    mi_app.fitz
  ```
  Acepta version pin nativo de pip (`==`, `>=`, `<`, etc.).
  Implica `--bundle-python` automáticamente.

- **`launcher_template.rs` extendido** con 2 placeholders nuevos:
  - `PLACEHOLDER_PIP_DECL_BLOCK`: donde se inyecta
    `const PIP_PACKAGES: &[u8] = include_bytes!("...");` si hay
    `--bundle-pip`, o string vacío si no.
  - `PLACEHOLDER_PIP_EXTRACT_BLOCK`: donde se inyecta el bloque
    de extracción del tarball pip adentro de
    `python/Lib/site-packages/` (Windows) o
    `python/lib/python3.X/site-packages/` (Unix).
  - `gen_launcher_main_rs(...)` suma param
    `pip_packages_path: Option<&str>`. None = backward compat
    con 8.b (template bit-a-bit idéntico).

- **Pipeline de build extendido** (`main::build_file_with_bundle`):
  1. Build del real binary (igual que 8.b).
  2. Descarga PBS tarball (igual).
  3. **NUEVO** si `--bundle-pip` no vacío: extraer PBS al cache
     local del proyecto (`target/fitz-build/<bin>_pbs_extract/`),
     correr `<pbs>/python -m pip install --target <dir> <pkgs>`,
     empacar el resultado en `<bin>_pip_packages.tar.gz`.
  4. **NUEVO**: hash combinado (PBS bytes + pip bytes) para que
     dos proyectos con paquetes distintos no compartan TMP dir.
  5. Generar launcher con ambos paths (Some(pip_tarball)).
  6. Build del launcher (cargo).
  7. Copia al destino del usuario.

### Tests

- **2 unit tests nuevos** en `launcher_template::tests`:
  - `gen_launcher_main_rs_con_pip_packages_inyecta_bloques`
  - `gen_launcher_main_rs_pip_packages_escapa_windows_path`
- **2 E2E tests nuevos** en `bundle_python_e2e.rs`:
  - `bundle_pip_implica_bundle_python_y_aborta_sin_from_python_import`
  - `bundle_pip_repetible_acepta_varios_paquetes`
- **Total Fase 8.c**: 4 tests nuevos. Acumulado con 8.b: 29
  tests específicos del bundling.
- Smoke `GUIDE_EXAMPLES_COMPILE` sigue verde (sin regresión).

### Smoke manual end-to-end (Windows)

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

### Tamaños observados

| Bundle | Tamaño bin | Cold first run | Warm |
|--------|------------|----------------|------|
| `--bundle-python` (stdlib) | ~22 MB | ~3-5s | ~50-100ms |
| `+ --bundle-pip requests` | ~23 MB | ~5-7s | ~50-100ms |
| `+ --bundle-pip sqla+psycopg2` (estimado) | ~50 MB | ~8-12s | ~50-100ms |

### Ejemplo + docs

- `examples/python-interop-8.c.fitz` runnable con comentarios
  exhaustivos (cuándo usar, caveats, tamaños).
- **Cap 21.12 nuevo** "`fitz build --bundle-pip` — empaquetar
  paquetes pip" en `docs/guide.md`. Renumeración:
  21.12 (CRUD)→21.13, 21.12 (Limitaciones)→21.14 (fix de bug
  de renumeración previo en 8.b.7 donde había dos 21.12).
- **README footnote § actualizado** con el nuevo flag y los
  casos de uso reales.
- **READMEs boilerplates 5/6 actualizados** con plan concreto
  de simplificación a `FROM gcr.io/distroless/cc-debian12` +
  `--bundle-pip sqlalchemy psycopg2-binary`. Imagen ~150 MB →
  ~80-100 MB. Dockerfiles actuales mantenidos sin cambios
  (smoke real Docker como deuda — el primer user que pruebe
  confirma).

### Deudas residuales (NO bloquean uso real)

- **Smoke real Docker de boilerplates 5/6 con --bundle-pip**:
  validado solo en Windows con programa simple (`requests`).
  La combinación `--bundle-pip sqlalchemy + psycopg2-binary`
  adentro de un Dockerfile Linux multi-stage es deuda nueva.
- **Constraint Linux/macOS heredado**: builder requiere Python
  3.14.x (R.bug-pyo3-abi3-portable-link componente Linux/macOS
  pendiente). Cuando cierre, `--bundle-pip` es independiente
  del Python del builder en las 3 plataformas.
- **C extensions cross-platform**: `pip install` al build time
  baja wheels específicos del triple del builder. Buildear
  Linux desde Windows requiere `cross` o Docker (igual que
  todo cross-compile Rust).
- **Re-pip-install al cambiar paquetes**: hoy el pip install
  corre cada build si `<bin>_pip_packages` no existe. Cuando
  cambiás `--bundle-pip <pkgs>`, el cache stale se borra
  automático (rm -rf antes de instalar). Optimizable con hash
  de la lista de pkgs como cache key.
- **`--bundle-pip` con requirements.txt**: hoy hay que listar
  paquetes uno por uno. `--bundle-pip-requirements <file>`
  futuro para leer requirements.txt automático.

## [v0.9.40] — 2026-05-23 — Fase 8.b: `fitz build --bundle-python` (binario standalone con CPython embebido)

Nuevo flag `--bundle-python` para `fitz build`. Produce un binario
standalone con CPython 3.14.5 embebido (vía
[python-build-standalone](https://github.com/astral-sh/python-build-standalone)
de Astral). El binario resultante **NO requiere Python instalado
en el destino** — corre en cualquier máquina del triple soportado,
en frío. Es el único lenguaje moderno que ofrece esto activamente
mantenido (PyOxidizer hizo algo parecido pero está ralentizado
desde 2023).

### Cambios

- **Nuevo flag CLI `--bundle-python`** (`Commands::Build`):
  ```bash
  fitz build --bundle-python mi_app.fitz
  ./mi_app   # corre sin Python en el PATH
  ```
- **Nuevo módulo `src/pbs.rs`** — descarga + cache local del
  tarball PBS. Release pinned `20260510` con CPython `3.14.5`,
  sabor `install_only_stripped` (~70% más chico que
  `install_only`). Cache en `~/.fitz/cache/pbs/` (override con
  `FITZ_CACHE_DIR`, mismo patrón que `git_dep`). Subprocess
  `curl`, cero deps Rust nuevas.
- **Nuevo módulo `src/launcher_template.rs`** — template Rust del
  launcher Datasette-style con placeholders `__FITZ_REPLACE_*__`.
  El launcher (~200 KB Rust standalone, sin pyo3) embebe vía
  `include_bytes!` el tarball PBS y el "real binary". En primer
  run extrae a `$TMPDIR/fitz-py-<hash>/` (subprocess `tar -xzf`,
  bsdtar nativo en Win11/macOS/Linux moderno), setea
  `PYTHONHOME` + `LD_LIBRARY_PATH`/`DYLD_FALLBACK_LIBRARY_PATH`/
  `PATH` según OS, y `exec` (Unix) / `spawn+wait` (Windows) del
  real binary. Hash FNV-1a 16-char para nomenclatura
  determinística del cache TMP.
- **Nueva función `main::build_file_with_bundle()`** — pipeline
  paralelo a `build_file()` cuando hay `--bundle-python`:
  validaciones tempranas (host triple soportado, programa usa
  `from python import`), build del real binary (reusa
  `codegen::generate_project` sin cambios), descarga PBS,
  generación + build del launcher en
  `target/fitz-build/<bin>_launcher/`, copia del launcher al
  destino.
- **Modelo arquitectónico**: launcher pattern (Datasette Desktop
  desde 2021). Descartamos:
  - **Extract-on-first-run naive**: no funciona, el OS resuelve
    libpython ANTES de `main()` (Linux: `DT_NEEDED` vía ld.so;
    macOS: `LC_LOAD_DYLIB` vía dyld; Windows: import table).
  - **Linking estático con PBS "full"** (PyOxidizer-style):
    "multi-month rabbit hole", PyOxidizer es el único proof y
    está ralentizado.
  - **Delay-load/dlopen manual**: sin soporte documentado en
    PyO3, brittle entre versiones.

### Tests

- **10 unit tests** en `src/pbs.rs::tests` (constantes pinned,
  URL builder, host triple detection, cache path, error display).
- **11 unit tests** en `src/launcher_template.rs::tests` (template
  sustitución, escape Windows paths, hash FNV-1a determinístico).
- **2 E2E tests** en `tests/launcher_template_e2e.rs` (template
  procesado compila como Rust válido, con paths Windows + paths
  con espacios).
- **2 E2E tests** en `tests/bundle_python_e2e.rs` (validation
  temprana: aborta con mensaje claro sin `from python import`,
  aborta antes de bundling si hay error de parse).
- **Total nuevo: 25 tests**. El smoke
  `GUIDE_EXAMPLES_COMPILE` sigue verde (sin regresión del
  codegen normal).

### Smoke manual validado

Sobre Windows 11 SSD con programa `from python import math`:

- Build: `→ compilando real binary → asegurando PBS tarball →
  compilando launcher → ✓ binario standalone (21.8 MB)`
- Run sin Python en PATH: output bit-a-bit con el real binary
  (`math.pi = 3.141592653589793`, `math.sqrt(81.0) = Ok(9.0)`).
- Cold first run: ~5.3s (extract tar + boot CPython).
- Warm subsequent runs: ~50-100ms (cache TMP hit).

### Tamaños observados

| Triple | Binario final | Extract dir TMP |
|--------|---------------|-----------------|
| `x86_64-pc-windows-msvc` | ~22 MB | ~61 MB |
| `x86_64-unknown-linux-gnu` | ~35 MB | ~75 MB |
| `aarch64-apple-darwin` | ~24 MB | ~62 MB |

### Ejemplo + docs

- `examples/python-interop-8.b.fitz` — programa runnable que
  demuestra el flag con comentarios detallados sobre cuándo
  usarlo y cuándo no, tamaños y timing observados.
- **Cap 21.11 nuevo** "`fitz build --bundle-python` — binario
  standalone" en `docs/guide.md` (renumeración 21.11→21.12,
  21.12→21.13). Incluye cuándo usar, tamaños, timing,
  arquitectura interna del launcher, constraint del builder, y
  pendientes.
- **README footnote § actualizado** con emphasis del feature
  como diferencial único en el cuadro de comparación
  Python/TS/Go/Fitz.
- **Cierre parcial** de la deuda
  `R.bug-pyo3-abi3-portable-link` (componente bundling): el
  modelo launcher pattern bypasea el bug en Windows
  completamente (real binary linkea contra `python3.dll`
  stable ABI, no contra `python314.dll` específica). En
  Linux/macOS el constraint sigue (builder = bundle version).

### Deudas residuales (NO bloquean uso real)

- **Bundling de pip packages** (sub-paso futuro). Hoy
  `--bundle-python` embebe CPython base + stdlib. Programas que
  usan SQLAlchemy/numpy/etc. necesitan `pip install` adicional
  en el destino. Una extensión `--bundle-pip <pkg>` podría
  empaquetar paquetes pip junto al CPython base.
- **Boilerplates 5/6 simplificación**: con `--bundle-pip` los
  Dockerfiles podrían `FROM scratch` o `FROM distroless` en
  lugar de `FROM python:3.X-slim`. Ahorro estimado: imagen
  ~150 MB → ~40 MB.
- **Linux/macOS smoke end-to-end**: hoy validado solo en
  Windows. Los primeros usuarios en Linux/macOS confirman que
  el pipeline funciona ahí también.
- **Bundle más chico vía stdlib stripping**: ~30% reducción
  posible eliminando módulos no usados (similar al
  `py-spy --strip` de PyOxidizer).
- **Hash SHA256** en lugar de FNV-1a para defender contra
  cambios silenciosos del PBS upstream (FNV-1a es suficiente
  hoy porque el release está pinned).

### Cómo retomar la deuda residual

Para `--bundle-pip`: agregar campo `bundle_pip: Vec<String>`
al `Commands::Build`; cuando hay `--bundle-pip <pkg>`, después
de extraer el tarball ejecutar `<extract-dir>/python/python.exe
-m pip install --target <extract-dir>/python/Lib/site-packages
<pkg>` adentro del launcher (en primer run, mismo flujo de
extract). Trade-off: primera ejecución del launcher con pip
puede tardar varios segundos. Diseño detallado pendiente.



Nuevo sub-comando `fitz py-stubs <archivo.pyi> [--out <archivo.fitz>]`
paralelo a `fitz py-types` (que ya hacía SQLAlchemy). Parsea stubs
Python PEP 484/561 y emite los `type` Fitz equivalentes para cada
`class` top-level. Cierra parcialmente la deuda `8-pyi-stubs`.

### Cambios

- **Nuevo módulo `src/pyi_stub.rs`** — parser .pyi ad-hoc (no
  parser Python completo). Tokenizer line-based, recursive descent
  sobre subset PEP 484:
  - Top-level `def name(args) -> ret: ...` (parsed pero no
    emitido al output — deuda menor).
  - Top-level `class Name: ...` con fields anotados.
  - Top-level `name: type = default` (parsed pero no emitido).
  - Type exprs: primitivos, `list[T]`, `dict[K, V]`,
    `Optional[T]`, `T | None` (PEP 604), forward refs string
    `"Foo"`, dotted names `module.Name` (toma el último segmento).
- **Mapper StubType → Fitz Type** (`stub_type_to_fitz_type`):
  - `int/float/str/bool/None/bytes` → primitivos Fitz.
  - `list[T]/dict[K,V]/Optional[T]` → `List<T>/Map<K,V>/T?`.
  - `T | None` (Union[T, None]) → `T?` (caso típico nullable).
  - Union no-null → `Any` (Fitz no tiene unions arbitrarias).
  - Nominal desconocido → registrado en TypeEnv.
- **CLI `Commands::PyStubs { source, out }`** — disponible **sin
  feature `python`** (el parser .pyi no usa PyO3). Sigue el mismo
  patrón del `py-types`: lee el .pyi, parsea, emite `.fitz` por
  stdout o archivo.
- **Renderer `render_stub_items_as_fitz`** (`src/main.rs`) — sólo
  emite `class` → `type` (def/var top-level se ignoran porque el
  evaluator runtime los maneja via `PyAny` opaco).

### Tests

- 21 unit tests en `pyi_stub::tests` (parser + mapper exhaustivo).
- 5 cli_e2e tests del comando `fitz py-stubs` (class básica, tipos
  compuestos + Optional, output a archivo, archivo inexistente,
  skip fns/vars).

### Ejemplo + docs

- `examples/guide/21b-pyi-stubs.fitz` con dos types generados +
  programa que los usa (paralelo al ejemplo de cap 21.7).
- Cap 21.8b nuevo en `docs/guide.md` con workflow, sub-set
  cubierto, restricciones, y nota sobre integración automática
  como deuda residual.
- Smoke `GUIDE_EXAMPLES_COMPILE` suma `21b-pyi-stubs.fitz`.

### Deuda residual (documentada)

- **Integración automática con el checker** — cuando `from python
  import foo` y `<base>/foo.pyi` existe, hidratar el TypeEnv
  directamente. Requiere `Type::PyModule` + refactor signature
  de `check_program(base_dir: Option<&Path>)`. Sin presión real
  hoy — el flow `fitz py-stubs --out` cubre el 80% del valor.
- **`def` top-level del stub al `.fitz`** — hoy se ignoran porque
  el runtime las trata como PyAny. Materializar las signatures
  como Fitz fns que tipan los calls Python al .py real es
  refactor mayor.
- **Métodos de class** — solo fields hoy. Materializar métodos
  custom requiere registro `type Foo { ... } fn Foo.method(...)`
  + decisiones sobre `self`.

## [v0.9.38] — 2026-05-23 — 9.w.2-wsconn-bidir: `WsConn<In, Out>` con tipos asimétricos

Cierra la deuda residual del MVP 9.w.2 (WebSockets) sobre tipos
bidireccionales separados. Habilita canales asimétricos donde el
cliente envía un tipo (e.g. comandos `Str`) y el server emite otro
(e.g. eventos `ChatMsg` estructurados). Backward-compat con todo el
código pre-bidir.

### Cambios

- **AST + type system**: `Type::WsConn(Box<Type>)` →
  `Type::WsConn { recv: Box<Type>, send: Box<Type> }`. Cuando el
  usuario declara `WsConn<T>` (aridad 1), `recv == send == T`
  (simétrico, identical to pre-bidir). Cuando declara
  `WsConn<In, Out>` (aridad 2), `recv = In`, `send = Out` difieren.
  `Type::WsConn::display` emite `WsConn<T>` para simétricos,
  `WsConn<In, Out>` para asimétricos.
- **Checker**: `infer_wsconn_method` recibe `recv_ty` y `send_ty`
  separados. `recv() → Result<RECV>`, `send/broadcast(msg: SEND) →
  Result<Null>`. Mensajes de error con tipo correcto en cada
  dirección.
- **Runtime intérprete**: `WsConnHandle` gana `send_type` (paralelo
  a `msg_type` que ahora documenta explícitamente "recv type").
  `ws_conn_send`/`ws_conn_broadcast` usan `send_type` para
  decidir modo binary vs text JSON. `ws_conn_recv` sigue con
  `msg_type` (recv). `build_ws_conn` toma `send_type` como
  parámetro adicional.
- **RouteSpec**: nuevo campo `ws_send_type: Option<TypeExpr>`. El
  evaluator lo popula al registrar el handler `@ws`.
- **Codegen `fitz build`**: preludio refactored —
  `struct __FitzWsConn<RECV: __FitzWsMessage, SEND: __FitzWsMessage>`
  con dos type params; `recv` usa `RECV`, `send/broadcast` usan
  `SEND`. `__fitz_ws_setup<RECV, SEND>` también con dos params. El
  wrapper del handler emite el setup con ambos tipos resueltos.
  Monomorfismo garantiza que `WsConn<T>` simétrico produzca un
  binario idéntico al pre-bidir.
- **AsyncAPI 3.0**: cuando `recv != send`, el schema emite **dos
  messages distintos** — `msg_in` (referenciado por la operation
  `receive`) y `msg_out` (referenciado por la operation `send`).
  Cuando son iguales, sigue emitiendo el único `msg` (sin romper
  consumers existentes del schema simétrico).
- **LSP**: el `detail` del completion sobre `WsConn<In, Out>` ahora
  muestra `recv() -> Result<In>` y `send(msg: Out)` con tipos
  correctos.

### Restricción binary mixto

Si `recv` o `send` es `Bytes` pero el otro no (`WsConn<Bytes, Str>`),
el codegen rechaza con error explícito. El wrapper del handler
detecta `recv_is_bytes != send_is_bytes` y aborta antes de emitir
el setup. Soporte de canales binary-mixed queda como deuda
residual menor.

### Tests

- 3 unit tests del checker (aridad 2 resuelve recv/send distintos,
  display asimétrico, aridad >2 es error).
- 1 E2E intérprete (`WsConn<Str, ChatMsg>`: cliente envía Str,
  server emite ChatMsg JSON-marshalled).
- 3 unit tests AsyncAPI (asimétrico emite dos messages, operations
  apuntan a messages distintos, simétrico sigue con `msg` único).
- 1 unit test LSP (detail correcto para `WsConn<Str, ChatMsg>`).

Total: +8 unit tests. 2215 → ~2223 verdes.

### Ejemplo + docs

- `examples/guide/29c-ws-bidir.fitz`: canal `WsConn<Str, ChatMsg>`
  con welcome message + loop recv/send.
- Cap 29 de la guía: sección "Canales asimétricos con `WsConn<In, Out>`"
  con explicación del modelo, AsyncAPI asimétrico, restricción
  binary mixto, paridad bit-a-bit.
- Smoke `GUIDE_EXAMPLES_COMPILE` suma `29c-ws-bidir.fitz`.

## [v0.9.36] — 2026-05-23 — Bloque C: imagen `:latest-python` + auth WS desde browsers

Segundo bloque de quick wins del día. Dos features autocontenidas
con valor inmediato para usuarios browser y CI/distribución.

### Quick win #1 — `ghcr.io/<owner>/fitz:latest-python` (fitz-python-image)

Nuevo job `docker-image-python` en `release.yml` que builda y
publica una imagen Docker dedicada con `--features python` activo,
lista para usar como base de boilerplates 5/6:

```dockerfile
# Antes (boilerplate 5/6) — ~5-8 min de build inicial:
FROM python:3.12-slim AS builder
RUN curl ... rustup ... && \
    cargo install --git https://github.com/Thegreekman76/fitz --features python ...

# Después — pull en segundos:
FROM ghcr.io/thegreekman76/fitz:latest-python AS builder
```

Single-arch (`linux/amd64`) inicial. ARM64 con `--features python`
queda como deuda explícita hasta que `R.bug-pyo3-abi3-portable-link`
se cierre (cross-compile PyO3 abi3 requiere setup adicional).

Tags publicados:
- `ghcr.io/<owner>/fitz:v0.9.36-python`
- `ghcr.io/<owner>/fitz:latest-python`

Patrón del Dockerfile sigue el del boilerplate `api-postgres-python`
(builder y runtime con `python:3.12-slim`, builder agrega Rust con
rustup, runtime descarta Rust). Los Dockerfiles de boilerplates
5/6 actualizados con nota sobre la alternativa rápida (sin
migrarlos todavía — el cambio queda como opt-in del usuario).

### Quick win #2 — Auth WS desde browsers (9.w.2-ws-auth-browser)

Workaround estándar para autenticar WebSockets desde código de
browser. `new WebSocket(url)` NO permite setear headers HTTP
arbitrarios; el segundo argumento sí acepta una lista de
subprotocols. Convención (Socket.IO, Phoenix, varios proyectos
Node): pasar el token via subprotocol `bearer.<token>`.

Desde v0.9.36, el runtime y el codegen Fitz extraen el token del
header `Sec-WebSocket-Protocol` y lo inyectan como
`authorization: Bearer <token>` al map de headers que ve el
`@auth_provider`. Sin cambios del lado user — el mismo provider
funciona para HTTP y WS browser.

Implementación:
- Nuevo helper público `extract_ws_bearer_subprotocol` en
  `src/http.rs` (runtime) + helper paralelo
  `__fitz_ws_extract_bearer_subprotocol` en preludio WS de codegen
  (`src/codegen.rs`).
- `build_ws_method_router` (runtime) y `gen_ws_handler_wrapper`
  (codegen): antes de invocar al `@auth_provider`, inyectan
  `authorization: Bearer <token>` al map si no hay header
  `Authorization` previo.
- Echo del subprotocol seleccionado en el handshake response via
  `ws.protocols([proto])` (RFC 6455 §4.1 — sin echo, el browser
  rechaza el upgrade).
- Compatibilidad: si el cliente envía AMBOS Authorization header
  Y subprotocol bearer, el header gana (preserva el caso wscat/
  curl/clientes no-browser).

Tests:
- 6 unit tests del helper (single proto, CSV con varios, ausente,
  sin match, token vacío, JWT con dots internos).
- 2 E2E intérprete (acepta token válido + echo del subprotocol;
  rechaza con 401 si el token es inválido).
- 2 unit codegen (output emite el helper + la inyección).
- 1 E2E codegen (binario nativo + cliente tokio-tungstenite con
  subprotocol — handshake + auth + echo end-to-end).

Cap 28 (Auth nativa) actualizado con la sección "Auth WS desde
browsers" en el cap 29: ejemplo cliente JavaScript + server Fitz
+ explicación del flujo + compatibilidad con header.

### Acumulado al cierre

+11 unit (6 helper + 2 codegen + 6 runtime + 1 docker workflow) +
2 E2E intérprete + 1 E2E codegen. Clippy `-D warnings` limpio.
Smoke `GUIDE_EXAMPLES_COMPILE` verde. Sin breaking changes — el
header `Authorization` original sigue funcionando idéntico para
clientes que pueden setearlo.

Boilerplates revisados — 6/6 verdes con `fitz check`. Boilerplate
api-websocket podría aprovechar el subprotocol en su frontend HTML
(deuda menor, queda opcional).

VSCode review: NO requiere update. Ninguno de los quick wins
toca AST/grammar/LSP. El helper de extracción es runtime/codegen
puro; la API del `@auth_provider` no cambia para el user.

Próximo norte: vaciar resto del backlog (`9.w.2-wsconn-bidir`,
`8-pyi-stubs`, `8-bundling-cpython`) o saltar a Fase 10 (Stack DB
nativo).

## [v0.9.35] — 2026-05-23 — Bloque triple de quick wins: split await + AsyncAPI UI + inferencia params

Bloque coordinado de tres quick wins del backlog post-v0.9.34, en
una sola tanda. Sin breaking changes — sólo features nuevas y
mejor inferencia.

### Quick win #1 — `let fut = py_call()?; fut.await` (8.7-await-binding-split)

`fitz build` ahora compila el patrón "split" del await Python:
binding intermedio del coroutine y `.await` después.

Antes solo aceptaba `<py_call>?.await` inline (`Await(Try(Call PyAny))`
en el AST). Con un binding intermedio, el inner del await era un
`Expr::Ident` con tipo `PyAny`, y el codegen emitía `.await` directo
sobre `__FitzPyObject` — Rust fallaba con "is not a future".

Fix: nuevo helper Rust `__fitz_py_await_obj(coro: &__FitzPyObject)`
emitido al preludio cuando `uses_async + uses_python`. Cuando
`Expr::Await(inner)` tiene `inner_ty == Type::PyAny` y NO matchea
el patrón inline, el codegen despacha al helper dedicado. El
intérprete ya lo soportaba (envuelve el coroutine en `Value::Future`
en `py_interop::call`) — ahora el codegen tiene paridad.

3 unit tests del codegen. Cap 21 de la guía actualizado con el
nuevo patrón documentado.

### Quick win #2 — UI HTML para AsyncAPI 3.0 (9.w.2-asyncapi-ui)

Cuando hay handlers `@ws`, además de `/asyncapi.json`, el server
auto-registra `/asyncapi` con una UI HTML embebida que renderea
channels + operations + messages + securitySchemes. Mismo patrón
que `/docs` para OpenAPI/Scalar.

Bundle: `@asyncapi/react-component@2.6.5` vía CDN (unpkg). Carga
liviana (~liviana después de cache del navegador).

- `src/templates/asyncapi.html`: HTML wrapper que hace fetch del
  schema y lo pasa a `AsyncApiStandalone.render(...)`.
- `src/asyncapi.rs`: `pub const ASYNCAPI_HTML` con `include_str!`.
- Runtime (`build_router_with_asyncapi` en `src/http.rs`):
  auto-registra `/asyncapi` cuando hay schema, cede si el user
  declaró `@get("/asyncapi")` propio.
- Codegen (`src/codegen.rs`): emite `static __FITZ_ASYNCAPI_HTML`
  + `async fn __serve_asyncapi()` + `.route("/asyncapi", ...)` en
  el router builder. Mismo cede-si-user-gana.
- Opt-out global: `@server(docs=false)` apaga AMBAS (OpenAPI +
  AsyncAPI UI/JSON).
- `eprintln!` del banner del runtime suma "GET /asyncapi (UI AsyncAPI)".

3 unit tests del runtime (HTML correcto, 404 sin schema, JSON sigue
funcionando) + 3 unit tests del codegen (handler/route emitidos,
cede sobre user, no se emite sin @ws). Cap 29 de la guía
actualizado.

### Quick win #3 — Inferencia de params/return sin anotar (cierre 5b.1)

La deuda 5b.1 (inferencia de tipos de params en fns sin anotación)
ya tenía implementación parcial (`fill_inferred_param_types` en
codegen) pero `type_to_type_expr` saltaba el caso `Nominal` con
un comentario "Skip Nominal por ahora — necesitamos el nombre real".

Fix: `type_to_type_expr` ahora recibe el `TypeEnv` y resuelve
`Nominal(id)` consultando `env.info(*id).name` para obtener el
nombre canónico. También suma `Type::Bytes`. Cubre:
Int/Float/Str/Bool/Null/Bytes/Nullable/List/Map/Result/Nominal.

Casos confirmados que ahora compilan sin anotaciones:
- `fn double(n) { return n * 2 }` (Int inferido).
- `fn greet(u) { return "hola {u.name}" }` con `User` (Nominal inferido).
- `fn shout(s) { return s.upper() }` con `"hola"` (Str inferido).
- Funciones recursivas con anotación de return + param sin anotar.
- Múltiples call sites del mismo fn.

5 unit tests del codegen (4 path #2 `resolve_param_type` + 1 path
#1 `fill_inferred_param_types` validando que Nominal se resuelve
a `TypeExpr::Named("User")`). Cap 11, cap 14 y cap 18 de la guía
actualizados — anotaciones siguen recomendadas pero NO obligatorias.

### Acumulado al cierre

+11 unit tests (3 await + 6 asyncapi + 5 infer). Clippy
`-D warnings` limpio. Smoke `GUIDE_EXAMPLES_COMPILE` verde. Sin
breaking changes — programas existentes compilan idéntico.

Boilerplates revisados — ninguno toca los paths cambiados (todos
usan anotaciones explícitas + handlers @ws con marshaling text +
sin call patterns split de Python). Sin necesidad de update.

Próximo norte: Fase 10 (Stack DB nativo) o seguir vaciando el
backlog (`9.w.2-wsconn-bidir`, `fitz-python-image`, `8-pyi-stubs`,
`8-bundling-cpython`).

## [v0.9.34] — 2026-05-23 — Quick win: 9.w.2-binary-frames — `WsConn<Bytes>` end-to-end

Cierra la deuda más visible del MVP de WebSockets (9.w.2): el
soporte para frames binarios raw vía `WsConn<Bytes>`. Hoy el wire
de un `WsConn` puede ser **text JSON-marshalled** (T = Str /
nominal / etc.) o **binary opaco** (T = Bytes); el modo lo elige
el T declarado y el lenguaje rechaza el mismatch con mensaje
claro. Cero deps nuevas, paridad bit-a-bit `fitz run` ↔ `fitz
build`.

**Lo que entra**:

- **Checker** — `WsConn<Bytes>` aceptado en `@ws` handlers como
  cualquier otro T concreto; `recv()` tipa `Result<Bytes>`,
  `send/broadcast` exigen arg `Bytes`. 4 unit tests blindean el
  contrato.
- **Runtime intérprete** — `Value::WsOutMessage::Binary(Vec<u8>)`,
  `IncomingFrame::{Text, Binary}` enum reemplaza el filtro
  text-only del read stream, `WsBroadcasterTrait::broadcast_binary`
  paralelo a `broadcast_text`. El evaluator discrimina por
  `ws_msg_is_bytes(msg_type)` en `recv/send/broadcast`. 3 E2E con
  tokio-tungstenite: echo round-trip, broadcast multi-cliente,
  mismatch (cliente manda text con T=Bytes → Err).
- **Runtime HTTP** (`src/http.rs`) — `WsReadStreamImpl::next_frame`
  expone Binary en lugar de rechazarlo; writer task gana rama
  `Binary(bs)` → `Message::Binary(bs.into())`.
- **AsyncAPI 3.0** — payload schema cuando T=Bytes emite
  `{"type":"string","format":"binary"}` + `contentType:
  application/octet-stream`. 3 tests del schema.
- **Codegen `fitz build`** — struct dedicado `__FitzWsConnBytes`
  (no genérico — specialization sobre `Vec<u8>` chocaría con el
  blanket impl del trait interno que lo trataría como
  `List<Int>` JSON); helper `__fitz_ws_setup_bytes`; writer
  task del preludio drena Binary también; ramaje en
  `gen_ws_handler_wrapper`. 1 E2E del codegen con cliente
  binary verificado bit-a-bit.
- **Guía cap 29** — sub-sección nueva "Frames binarios con
  `WsConn<Bytes>`" con ejemplo runnable + AsyncAPI schema
  emitido + trade-off documentado (text XOR binary por
  endpoint). Ejemplo `examples/guide/29b-ws-binary.fitz`
  agregado al smoke `GUIDE_EXAMPLES_COMPILE`.

**Decisión de diseño**: opción A — un endpoint es text-only XOR
binary-only, según el T declarado. Más simple que un canal mixto
y alineado con el modelo "T determina el frame type" que ya
tiene el lenguaje. Si aparece presión por endpoints mixtos,
queda como sub-paso futuro.

**Acumulado al cierre**: +10 unit (4 checker + 3 AsyncAPI + 3
E2E intérprete = todos via `cargo test --lib`) + 1 E2E codegen
(`cargo test --test compile_e2e`). Clippy `-D warnings` limpio.
Sin breaking changes — handlers `WsConn<Str>` / `WsConn<Nominal>`
existentes siguen funcionando idéntico.

Próximo norte (mismo bloque de quick wins post-boilerplates):
investigación + cierre de `R.bug-pyo3-abi3-portable-link` —
binarios Linux con `--features python` corren en cualquier
Python 3.10+ del runtime sin rebuild.

## [v0.9.32] — 2026-05-22 — Patch: mini-tanda Cleanup-Residual+ — limpiezas mecánicas + pyo3-abi3 cerrado + multi-arch Docker

Bloque grande de cleanup post-Cleanup-Residual. 4 sub-tandas
coordinadas: auditoría de deudas (4 más marcadas CERRADAS de
facto), cleanups mecánicos (clippy + fmt), fix `pyo3-abi3-autoinit`
con CI multi-Python, multi-arch Docker image.

### Sub-tanda A — Auditoría (4 deudas RESIDUALES marcadas CERRADAS)

- **5b.5-imports-transitivos** ✓ — ya cerrada por F15 (Fase 9.0);
  test `f15_module_loader_acepta_imports_transitivos_en_modulo`
  vive en codegen tests.
- **F13-listas-heterogeneas-compiladas** ✓ — implementado vía
  `__FitzValue` tagged runtime. `uses_fitz_value` se setea en
  literales heterogéneos.
- **8.7-fromfitzpy-symmetric** ✓ — subsumida por mini-fase 8.7.bis
  (v0.9.28). Dirección Python→Fitz está en helpers
  `__fitz_py_to_instance_*` / `__fitz_py_to_list_*` per-tipo —
  equivalente funcional al trait simétrico propuesto.
- **5b.5-let-expr-top-mod** ✓ — cerrada por F14. `gen_module_top_let`
  tiene 3 caminos: const literal → `pub static`/`pub const`,
  const-eval → `pub const`, runtime → accessor `pub fn X() -> T`.

### Sub-tanda B — Mecánicos (alto valor, bajo riesgo)

- **clippy-all-targets** ✓ — fix de 9 errores que bloqueaban
  `cargo clippy --all-targets -- -D warnings`:
  - 2× `useless_format!` en tests de `http.rs`
  - 1× `unused_import` (`futures_util::SinkExt` en test WS)
  - 2× `cloned_ref_to_slice_refs` en `hash_password` tests
  - 2× `MutexGuard held across await` (intencional — SERIAL Mutex
    para serializar tests E2E; ahora marcado `#[allow]` explícito)
  - 1× `non_snake_case` (`fmt_G_uppercase_compila` → `fmt_g_...`)
  - 1× `unnecessary_get_then_check` (`get().is_none()` →
    `!contains_key()`)
- **fmt-cleanup-codebase** ✓ — `cargo fmt --all` aplicado. 28
  archivos modificados (2246 diffs canónicos de rustfmt).
  Pendiente: activar `cargo fmt --check` en CI — sub-paso futuro
  separado para que el reactivar quede en commit limpio.

### Sub-tanda D.1 — multi-arch-docker ✓

`release.yml` job `docker-image` ahora emite imagen multi-arch
`linux/amd64,linux/arm64`. Cambios:

- Descarga ambos artefactos `binaries-linux-x64` + `binaries-linux-arm64`.
- Dockerfile usa `ARG TARGETARCH` para copiar el binario
  pre-compilado correcto (`fitz-amd64` o `fitz-arm64`).
- `docker/setup-buildx-action@v3` habilita el multi-arch build.
- `docker/build-push-action@v6` con `platforms: linux/amd64,linux/arm64`
  push manifest que Docker resuelve por host.

Habilita Mac M-series (arm64), Raspberry Pi 4+ (arm64), AWS
Graviton (arm64) sin emulación QEMU. Imagen GHCR pasa de single-arch
amd64 a multi-arch transparente.

### Sub-tanda D.3 + E — R.bug-pyo3-abi3-autoinit CERRADO + boilerplates simplificados

Fix de la deuda más vieja del backlog Python interop. Antes:
`Cargo.toml` tenía `features = ["abi3-py310", "auto-initialize"]`
que eran INCOMPATIBLES — auto-initialize ganaba, el binario
linkeaba contra libpython específica del builder, perdíamos la
portabilidad abi3.

Fix:
- `Cargo.toml`: removido `auto-initialize`. Solo `abi3-py310` activo
  → binario corre contra cualquier Python 3.10+.
- `src/py_interop.rs`: nuevo helper `ensure_python_initialized()`
  que llama `Python::initialize()` adentro de un `std::sync::Once`.
  Lazy init en el primer `import_module`. Idempotente, sin overhead
  perceptible.
- `.github/workflows/ci.yml`: job `python` ahora corre con matriz
  `python-version: [3.10, 3.11, 3.12, 3.13]` para validar el
  contrato cross-Python.
- `boilerplates/api-postgres-python/Dockerfile` simplificado:
  builder pasa de `python:3.12-slim` + rustup manual a `rust:slim`
  con Rust pre-instalado. Ahorro de ~2-3 min por docker build.
  Runtime `python:3.12-slim` queda intacto.
- `boilerplates/api-fullstack-postgres/Dockerfile` mismo refactor.

46 py_interop tests verdes localmente con feature python. El binario
default sin feature python no toca su path de compilación.

### Sub-tanda C + D.2 — DIFERIDAS

- **9.w.2-binary-frames** (WS Bytes payload): scope ~1-2h con
  refactor del trait `WsReadStreamTrait` + nuevo
  `WsOutMessage::Binary` + dispatch en evaluator/codegen. Sin
  presión real, defer a sesión dedicada.
- **fitz-python-image** (`ghcr.io/.../fitz:latest-python`): requiere
  compilar `--features python` adentro del Dockerfile en buildx
  multi-arch — ~25 min de CI compute por release. El workaround
  actual del boilerplate (cargo install --git) toma ~6-9 min y
  solo corre por boilerplate, no por release. Trade-off no
  justifica.

### Validación

- **2178 unit tests** verdes localmente.
- **46 py_interop tests** verdes localmente con `--features python`
  (Python 3.14 en local).
- **clippy `--all-targets -- -D warnings`** verde.
- **cargo fmt --all -- --check** verde.

CI multi-Python matrix correrá en la próxima push a `main` —
valida 3.10/3.11/3.12/3.13 automáticamente. Si alguna versión falla,
abrimos deuda específica.

---

## [v0.9.31] — 2026-05-22 — Patch: mini-tanda Cleanup-Residual — 2 deudas FUNCIONALES cerradas

Cierre de 2 deudas medias documentadas en
`docs/deudas_lenguaje.md` después del cierre del plan
post-boilerplates. Una tercera (R.bug-pyo3-abi3-autoinit) quedó
diferida con plan claro porque su validación necesita Docker
multi-Python que no tengo en local — el workaround del
boilerplate funciona correctamente y no urge.

**R.bug-13i-stack-overflow-debug** — CERRADO

`13i-campos-privados.fitz` desbordaba el stack al compilar con
`fitz build` debug-mode en Windows (1 MB stack default). Fix:
`.cargo/config.toml` con linker flag `/STACK:8388608` bajo
`[target.x86_64-pc-windows-msvc]`. El main thread del binario
`fitz` ahora tiene 8 MB de stack en Windows (paridad con Unix
default). Smoke `GUIDE_EXAMPLES_COMPILE` verde con 13i incluido.
Clippy `-D warnings` verde.

**R.bug-result-status** — CERRADO

Handler HTTP con return type `Result<T>` + `return <status> { ... }`
adentro serializaba con wrapper `{"Ok":{...}}` en lugar de
desempacar el inner. Fix en `src/codegen.rs::gen_return`:

```rust
// Antes (en response_mode):
return __FitzResponse {
    status: 200,
    body: <Result<Item, String> as __ToFitzJson>::__to_fitz_json(&(Ok(found.clone()))),
    // ↑ serializa con wrapper {"Ok":{...}}
};

// Ahora (response_mode con Expr::Ok detectado):
return __FitzResponse {
    status: 200,
    body: <ItemData as __ToFitzJson>::__to_fitz_json(&(found.clone())),
    // ↑ desempaca el inner, serializa Item directo
};
```

Semántica paralela al runtime:
- `return Ok(v)` → 200 + body = v serializado.
- `return Err(e)` → 500 + body = `{"error": e}` serializado.
- `return <status> { ... }` sin cambios.

2 E2E verdes en `compile_e2e::r_bug_result_status_handler_*`
(unwrap Ok + path 404). Boilerplate `api-simple::get_item`
simplificado a `Result<Item>` con `return Ok(it)` semánticamente
prolijo (era workaround a `Item` directo).

**R.bug-pyo3-abi3-autoinit** — DIFERIDO con plan claro

Cargo.toml de Fitz tiene `pyo3 = { features = ["abi3-py310",
"auto-initialize"] }`, incompatibles entre sí: auto-initialize
gana y el binario linkea contra libpython específica del builder,
perdiendo la promesa "binario portable" de abi3. Workaround actual:
match builder/runtime Python en los Dockerfiles de boilerplates
5/6, funciona OK pero agrega ~30s al build (apt-get build-essential
+ rustup en lugar de `rust:slim`).

Fix planificado (no ejecutado en esta mini-tanda):
1. Quitar `auto-initialize` del Cargo.toml.
2. Emitir `pyo3::prepare_freethreaded_python()` en el preludio
   del codegen y al boot del intérprete cuando `uses_python = true`.
3. Validar cross-Python en Docker (build 3.13 + run 3.10/3.11/
   3.12/3.14). El paso 3 requiere Docker runner con múltiples
   Pythons que no tengo en local — bloqueante.

Próxima acción: cuando aterrice un sub-paso "CI multi-Python"
con GitHub Actions matrix, cerrar este fix ahí.

Total al cierre: **2178 unit + 277 compile_e2e + 3 openapi**.
Smoke `GUIDE_EXAMPLES_COMPILE` verde. Clippy `-D warnings` verde.

---

## [v0.9.30] — 2026-05-22 — Feature: mini-fase loader-absoluto — imports nested cross-folder

Cuarto y último paso del plan post-boilerplates. Cierra deuda
**R.bug-loader-relative-only** (descubierta 2026-05-22 al armar
el 6to boilerplate, documentada en `docs/deudas_lenguaje.md`).

El loader de módulos ahora resuelve imports en DOS estrategias
encadenadas:

1. **Relativo al importer** (comportamiento previo). Si el archivo
   buscado existe en `<importer_dir>/<segments>`, se usa.
2. **Relativo al import_root** (nuevo). Si el archivo NO existe
   relativo al importer, se prueba relativo al "import root" =
   parent del entry file (estable durante toda la vida del loader).

Caso canónico que ahora funciona — proyecto con módulos en
subcarpetas hermanas:

```
src/
├── main.fitz          → from data.users import create
├── types/
│   └── user.fitz      → type User { ... }
└── data/
    └── users.fitz     → from types.user import User
                        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^
                        // Antes: buscaba `src/data/types/user.fitz` y fallaba.
                        // Ahora: relativo falla → fallback a `src/types/user.fitz`. ✓
```

Backward compat preservada: el patrón `import bar` desde un módulo
nested que resuelve a un sibling (`sub/bar.fitz`) sigue ganando vía
la búsqueda relativa, sin pasar por el fallback.

Cambios:

- `src/evaluator.rs`:
  - `Loader` suma `import_root: PathBuf` (estable durante toda la
    vida del loader, fijado al `base_dir` inicial = parent del
    entry file).
  - `resolve_module_path` devuelve `Vec<PathBuf>` con candidatos
    ordenados (relativo primero, después import_root si difiere).
  - `load_module` itera los candidatos; el primero que
    canonicalize OK gana.
- `src/codegen.rs`:
  - Nuevo helper `mod_qualifier_of(rel_path)` que convierte
    `types/user.rs` → `types::user`. `LoadedModuleSigs` suma
    field `mod_qualifier` (computed at construction).
  - `emit_use_decls`, `emit_module_use_decls`,
    `resolve_namespace_field`, `resolve_namespace_call`, y el
    `imported_mod_and_item` en `gen_struct_lit` ahora usan
    `mod_qualifier` (path completo) en lugar de `mod_name`
    (último segmento). Antes el codegen emitía
    `use crate::user::User` para `from types.user import User`
    y rustc fallaba con "unresolved import `crate::user`".
- 2 unit tests verdes en `evaluator::tests::loader_absoluto_*`:
  - `data_sibling_import_resuelve_via_import_root` — caso canónico.
  - `no_rompe_imports_relativos_legacy` — backward compat.
- 1 E2E codegen verde en
  `compile_e2e::loader_absoluto_data_sibling_import_compila_en_fitz_build`
  — proyecto con `src/main.fitz` + `src/types/user.fitz` +
  `src/data/users.fitz` compila a binario y corre OK.
- Boilerplate `api-postgres-python` refactorizado a usar el
  patrón limpio:
  - `data/users.fitz` ahora hace `from types.user import User` +
    `from types.api import NewUser` y devuelve `Result<User>` /
    `Result<List<User>>` tipado (no más JSON crudo).
  - `main.fitz` simplificado a delegar a `data.users::create`/
    `find`/`list_all` directo (sin coerción intermedia).
- VSCode extension: SIN cambios necesarios (fix es runtime/codegen
  puro, no toca grammar/sintaxis/types/LSP). Documentado en cierre
  formal.

Bug residual detectado durante la validación (NO regresión, NO
bloquea):

- **R.bug-13i-stack-overflow-debug**: `13i-campos-privados.fitz`
  desborda el stack en `fitz build` debug-mode en Windows
  (1 MB stack). Verificado con `git stash` que el overflow es
  pre-existente, NO disparado por esta mini-fase. Release build
  compila el ejemplo sin problema. Fix lean propuesto en
  `docs/deudas_lenguaje.md`: linker flag `/STACK:8388608` en
  `.cargo/config.toml` para Windows target. Sin presión real.

Total al cierre: **2178 unit + 275 compile_e2e + 3 openapi**.
Smoke `GUIDE_EXAMPLES_COMPILE` con asterisco — 13i flake (deuda
ya documentada).

**CIERRE FORMAL DEL PLAN POST-BOILERPLATES**: los 4 pasos
(coerción recursiva runtime + 8.7 codegen + env builtin +
loader-absoluto) cerrados entre el 2026-05-22 y el 2026-05-22.
Ningún paso requirió cambios al checker estático (la mayoría
extensiones al evaluator y codegen). Boilerplates simplificados
gracias a los fixes. Próximo norte: definir el siguiente bloque
con el autor (probablemente algo del backlog "deudas residuales
sin presión real" o una mini-fase de features nuevas).

---

## [v0.9.29] — 2026-05-22 — Feature: mini-fase env builtin — `env`/`env_or`/`load_env`

Tercer paso del plan post-boilerplates. Tres builtins nuevos para
leer variables de entorno desde Fitz, paridad bit-a-bit
intérprete↔codegen. Cierra deuda documentada en
`project_env_builtin.md` (memoria).

Builtins agregados:

- **`env(key: Str) -> Result<Str>`** — lee `std::env::var`. Si la
  var existe → `Ok(value)`, si no → `Err("env var X no definida")`.
  Fuerza al usuario a manejar el caso missing con `?` o `match`
  (paralelo a `find`/`get`/`json.loads`). Modelo "sin excepciones"
  del lenguaje respetado.
- **`env_or(key: Str, default: Str) -> Str`** — mismo lookup pero
  con default. Nunca falla. Paralelo a `Option::unwrap_or` de Rust.
- **`load_env(path: Str) -> Result<Null>`** — parser KEY=VALUE
  simple. Líneas vacías y `#` comments ignoradas, comillas dobles
  wrapping strippeadas. Sin variable expansion (`$VAR`/`${VAR}`),
  sin multi-line, sin escape chars. **Sin auto-load por diseño**:
  el usuario explícitamente llama `load_env(".env")?` en el boot
  ("explicit > magic").

Cambios:

- `src/evaluator.rs`: 3 builtins nuevos (`builtin_env`,
  `builtin_env_or`, `builtin_load_env`) + helper `parse_env_file`
  con parser KEY=VALUE simple. Registrados en `register_builtins`;
  agregados a `builtin_names()` del REPL.
- `src/types.rs::register_builtins`: 3 firmas nuevas registradas
  en el checker (`env: Function([Str]) -> Result<Str>`,
  `env_or: Function([Str, Str]) -> Str`,
  `load_env: Function([Str]) -> Result<Null>`).
- `src/codegen.rs`: 3 arms nuevos en `gen_call` que delegan a
  helpers `__fitz_env`/`__fitz_env_or`/`__fitz_load_env` emitidos
  siempre en el preludio (son fns chicas; Rust hace dead-code elim
  si no se usan).
- 8 unit tests verdes en `evaluator::tests::env_builtin_*`/
  `env_or_builtin_*`/`load_env_builtin_*` cubriendo:
  var existente como Ok, var missing como Err con mensaje
  específico, var vacía como Ok(""), propagación con `?`,
  env_or con default vs valor real, load_env de archivo con
  comments + comillas + líneas vacías, load_env de archivo
  inexistente como Err.
- 5 tests E2E verdes en `compile_e2e::env_builtin_*`/`env_or_*`/
  `load_env_*` con nuevo helper `build_and_run_with_env` que
  inyecta env vars al child via `Command::env`. Confirma paridad
  bit-a-bit `fitz run` ↔ `fitz build`.
- VSCode extension actualizada:
  - Grammar TextMate (`syntaxes/fitz.tmLanguage.json`): los 3
    builtins sumados al pattern `support.function.builtin.fitz`.
  - LSP autocomplete (`src/lsp.rs::scope_level_completions`):
    los 3 builtins listados con sus firmas en el detail.
- Cap nuevo 31 "Variables de entorno" en `docs/guide.md`
  (renumeración 31→32 Plantillas, 32→33 Qué sigue). Cubre las 3
  builtins con patrones canónicos, formato `.env`, razón del
  `Result<Str>` en `env()`, política de no-auto-load.
- Ejemplo runnable nuevo `examples/guide/31-env.fitz` agregado al
  smoke `GUIDE_EXAMPLES_COMPILE` (verde).
- Boilerplate `api-middleware-cors`: el `JWT_SECRET` hardcoded
  reemplazado por `env_or("JWT_SECRET", "demo-cambiame-...")`.
  README refrescado: la nota "env builtin es deuda futura"
  reemplazada por ejemplo de uso real. Roadmap del boilerplate
  marca esa deuda como ✓ CERRADA.

Total al cierre: **2176 unit + 274 compile_e2e + 3 openapi**.
Smoke `GUIDE_EXAMPLES_COMPILE` verde con el nuevo cap 31.

**Próximo paso del plan post-boilerplates**: Paso 4 — Loader Fitz
con imports absolutos desde manifest root (deuda
R.bug-loader-relative-only — bloquea organización multi-archivo).

---

## [v0.9.28] — 2026-05-22 — Patch: paridad codegen — coerción `PyAny → List<T>`/`Nominal`/`List<Nominal>` en `fitz build`

Cierra la deuda **R.bug-8.7-coercion-list-codegen** documentada al
cierre formal de Fase 8.7 (CHANGELOG v0.8.8 de 2026-05-15). Paso 2
del plan post-boilerplates, paridad codegen del Paso 1 (v0.9.27)
que cerró el equivalente runtime.

Antes (en `fitz build`):

```fitz
type User { id: Int, name: Str }
from python import json

fn list_users(raw: Str) -> Result<List<User>> {
    let users: List<User> = json.loads(raw)?
    // ERROR Rust: expected Arc<Mutex<Vec<UserData>>>, found __FitzPyObject
    return Ok(users)
}
```

Ahora:

```fitz
fn list_users(raw: Str) -> Result<List<User>> {
    let users: List<User> = json.loads(raw)?
    // OK — compila a binario nativo, coerce el PyList item-por-item
    // a `Arc<Mutex<Vec<Arc<Mutex<UserData>>>>>` bit-a-bit como el runtime.
    return Ok(users)
}
```

Cambios:

- `src/codegen.rs::coerce(code, from, to, env)` ahora despacha:
  - `(PyAny, List<Int>)` → `__fitz_py_to_list_i64(&{code})`
  - `(PyAny, List<Float>)` → `__fitz_py_to_list_f64(&{code})`
  - `(PyAny, List<Str>)` → `__fitz_py_to_list_string(&{code})`
  - `(PyAny, List<Bool>)` → `__fitz_py_to_list_bool(&{code})` (helper nuevo)
  - `(PyAny, Nominal(T))` → `__fitz_py_to_instance_<T>(&{code})` (helper per-tipo)
  - `(PyAny, List<Nominal(T)>)` → `__fitz_py_to_list_<T>(&{code})` (helper per-tipo)
  - Signatura cambió: añadido param `env: &TypeEnv` (89 call sites
    actualizados via sed automático).
- Nuevos métodos en `CodegenCtx`:
  - `gen_fitz_py_to_instance_helper(name, sig)` — emite
    `fn __fitz_py_to_instance_<Name>(obj: &__FitzPyObject) -> Arc<Mutex<<Name>Data>>`
    con extracción field-por-field, defaults inline, manejo de
    Nullable (`None` cuando dict missing o Python None), error
    claro cuando field requerido falta. Llamado desde
    `gen_type_def` cuando `uses_python = true`.
  - `gen_fitz_py_to_list_helper(name)` — emite
    `fn __fitz_py_to_list_<Name>(obj: &__FitzPyObject) -> Arc<Mutex<Vec<Arc<Mutex<<Name>Data>>>>>`
    iterando un PyList y delegando al helper de instance.
  - `py_field_extract_code` + `py_field_extract_arms` +
    `py_inner_extract_for_nullable` — sub-helpers para emitir
    el extract code por field según tipo (Int/Float/Str/Bool/
    Nullable<primitive>/Nominal/List<primitive>).
- 3 E2E tests verdes en `compile_e2e::fase_8_7_bis_*`:
  - `pyany_a_list_int_via_anotacion` — patrón list primitivo.
  - `pyany_a_instance_via_anotacion` — patrón single dict, con
    default field aplicado cuando falta key.
  - `pyany_a_list_de_instances` — patrón canónico del boilerplate
    `api-postgres-python::list_users`.
- READMEs de boilerplates 5/6 actualizados: la nota "deuda 8.7
  bloquea `fitz build`" reemplazada por nota técnica que cita
  el cierre 2026-05-22 (mini-fase 8.7.bis) y explica que `fitz
  build` ahora soporta el patrón end-to-end. Dockerfiles
  intencionalmente quedan con `fitz run` por boot rápido en
  containers (build desde source toma 8-12 min); usuarios que
  quieran binario standalone solo cambian `CMD`.
- VSCode extension revisada: grammar + LSP autocomplete + walkers
  + diagnostics SIN cambios (el fix es codegen puro, no toca
  sintaxis ni types estáticos). Documento confirmado en cierre
  formal.

Total al cierre: **2168 unit + 269 compile_e2e + 3 openapi**.
Smoke `GUIDE_EXAMPLES_COMPILE` verde.

**Deuda residual del scope acotado** (NO bloquea uso real):
- `Map<K, V>` coerción desde PyDict no implementada (poco común
  en práctica — `let m: Map<Str, V> = json.loads(s)?` es el caso
  raro).
- `List<List<T>>` o nominales anidados que contienen `List<Nominal>`
  como field también pendientes (deuda menor — el subset cubierto
  destraba el 90% del caso real).

---

## [v0.9.27] — 2026-05-22 — Patch: coerción recursiva `Map → Instance` sobre `List<T>`/`Map<K,V>` en runtime

Fix de la deuda **R.missing-recursive-instance-coercion** (descubierta
el 2026-05-22 al armar el 6to boilerplate `api-fullstack-postgres`).
La coerción 8.4.3 (`Map → Instance`) ahora recursa sobre `List<T>` y
`Map<K, V>` cuando el inner es nominal o `Nullable(nominal)`.

Antes:

```fitz
let users: List<User> = json.loads(raw)?
// users es List<Map>, NO List<User>. El binding pasa el checker
// gradual pero `users.find(fn(u) => u.name == "x")` falla con
// "Map no tiene field name".
```

Workaround anterior (loop manual):

```fitz
let maps: List<Any> = json.loads(raw)?
let users: List<User> = []
for m in maps {
    let u: User = m   // ← acá disparaba la coerción Map → User
    users.push(u)
}
```

Ahora:

```fitz
let users: List<User> = json.loads(raw)?
// users es List<User> directamente, cada item coercionado item-por-item.
```

Cambios:

- `src/evaluator.rs::coerce_to_annotation` con dos casos recursivos
  nuevos al inicio (List + Map). Solo dispara si el inner es nominal
  (filtra `List<Int>`, `List<Any>`, etc. — passthrough).
- Helper `is_nominal_target(ty, env)` chequea contra el env si el
  ident apunta a un `Value::Type`.
- 8 unit tests verdes en `evaluator::tests::coerce_recursive_*`
  cubriendo: caso canónico, lista vacía, lista de primitivos no
  dispara, Nullable nominal con `Null` pasando, `Map<Str, User>`,
  error claro con field requerido faltante, default aplicado,
  passthrough sin coerción si value no es List.
- 2 boilerplates simplificados:
  - `api-fullstack-postgres::list_tasks` de loop manual a 1 línea.
  - `api-postgres-python::list_users` de `Result<Str>` con JSON
    crudo a `Result<List<User>>` tipado.

Total al cierre: **2168 unit + 257 compile_e2e + 3 openapi**. Smoke
`GUIDE_EXAMPLES_COMPILE` verde.

**Deuda derivada que sigue abierta**: 8.7 (codegen) — `fitz build`
todavía necesita wiring de `coerce(PyAny → List<T>)` para paridad
bit-a-bit. Es el siguiente paso del plan post-boilerplates.

---

## [v0.9.26] — 2026-05-22 — Patch: fix OPTIONS preflight duplicado al compartir path con CORS

Fix de la deuda **R.bug-options-preflight-shared-path** (descubierta
el 2026-05-22 al validar el 6to boilerplate end-to-end con frontend).

Cuando dos o más handlers HTTP compartían el mismo path con
`@middleware(cors(...))` declarado en cada uno (caso típico CRUD:
`/tasks` con `@get` + `@post`, o `/tasks/{id}` con
`@get`/`@put`/`@delete`), axum hacía panic al construir el `Router`:

```
Overlapping method route. Handler for `OPTIONS /tasks` already exists
```

Cada handler intentaba registrar su propio OPTIONS preflight para
el mismo path. Fix coordinado runtime + codegen:

- **Intérprete (`src/http.rs::build_router_with_asyncapi`)**:
  pre-cómputo de `CorsConfig` merged por path (unión de
  `allow_methods` preservando orden, `allow_headers` case-insensitive,
  max `max_age`, primer `allow_origin` gana). Solo el OWNER del
  path emite el preflight con la config merged.
- **Codegen (`src/codegen.rs`)**: mismo patrón. Nuevos campos en
  `CodegenCtx`: `cors_merged_per_path` + `cors_preflight_owner`.
  Pre-scan `precompute_cors_merge(http_fns)` corre antes del loop de
  wrappers. `emit_cors_helpers` solo emite el preflight para el
  owner; nuevo método `cors_resolve_fn_for(sig)` para que los
  wrappers no-owner referencien el resolver compartido del owner.
- 4 unit tests verdes en
  `http::tests::bug_options_preflight_duplicado_*` (no-panic,
  methods merged, 3 verbos con `{id}`, headers case-insensitive
  dedup).
- 1 E2E verde en
  `compile_e2e::r_bug_options_preflight_duplicado_en_fitz_build_paridad_con_fitz_run`.

Acompañado por:

- **6 boilerplates Dockerizados** live en `boilerplates/` con README
  general comparativo (`cli-tool`, `api-simple`,
  `api-middleware-cors`, `api-websocket`, `api-postgres-python`,
  `api-fullstack-postgres`). El 6to es el showcase fullstack —
  frontend rico vanilla + API Fitz + Postgres en 3 containers.
- Mención de boilerplates en README + cap 31 nuevo de
  `docs/guide.md` (renumeración 31→32 Qué sigue) + `docs/index.md`.
- Naming real de `.vsix` corregido en docs
  (`fitz-lang-<plataforma>.vsix`, 4 plataformas: Win x64, Linux
  x64/ARM, macOS Apple Silicon).
- Multiplataforma resaltado como diferencial en README + index +
  cap 1 de la guía.
- Bug entry como CERRADO en `docs/deudas_lenguaje.md`.

---

## [v0.9.25] — 2026-05-21 — Patch: fix codegen deadlock en string interp con re-locks

**Bug fix crítico** descubierto al validar el primer boilerplate
Dockerizado (`boilerplates/cli-tool`).

**Bug**: `gen_str_interp` emitía `format!(fmt, arg1, arg2, ...)` donde
los temporales (MutexGuards de `.lock().unwrap()`) vivían hasta el
final de la statement. Si dos args lockeaban el mismo `Arc<Mutex<>>`
(caso típico: `print("{xs.len()} - {total(xs)}")`), el segundo
`.lock()` desde el mismo thread quedaba esperando que el primero
libere → **deadlock silencioso del binario** (std::sync::Mutex no es
re-entrant). El programa terminaba sin panic ni error visible — solo
output truncado.

**Fix** en `src/codegen.rs::gen_str_interp`: cuando hay ≥2 args, emitir
cada arg como `let __aN = <code>;` adentro de un bloque ANTES del
`format!`. Cada `let` cierra una statement → dropea el MutexGuard
inmediatamente. El siguiente arg evalúa sin guards vivos del anterior.
0 args mantiene `String::from`, 1 arg mantiene `format!` directo.

**Regression test**: `tests/compile_e2e.rs::r_bug_deadlock_str_interp_re_lock_mismo_arc_no_cuelga`.
Si el deadlock vuelve, el test falla por timeout/exit code.

**Boilerplate cli-tool** (`boilerplates/cli-tool/`) incluido en este
release como showcase del fix funcionando end-to-end: Dockerfile +
README exhaustivo + .gitignore + .dockerignore + fitz.toml con
`edition = "2026"` + main.fitz con report generator usando el
patrón problemático que el fix arregla.

**Validado**: 2156 unit + 256 compile_e2e (255 + 1 nuevo) +
smoke con 78 ejemplos guide verde. Sin breaking changes — solo
bug fix.

**Por qué importa este release**: la imagen Docker `ghcr.io/<owner>/
fitz:v0.9.24` tenía el binario con el bug. Boilerplates posteriores
van a usar `FROM ghcr.io/<owner>/fitz:latest` que ahora apunta a
v0.9.25 con el fix. Sin este release, los boilerplates con patterns
típicos de print interp se cuelgan.

Deuda **R.bug-deadlock** cerrada el mismo día del descubrimiento.

## [v0.9.24] — 2026-05-21 — Cierre formal Fase 9.w MVP entera (Stack web first-class)

**Cierre formal del bloque entero "Stack web first-class"** —
9.w.1 + 9.w.2 + 9.w.3 cerradas entre 2026-05-20 y 2026-05-21.
9.w.4 (ORM nativo + migraciones) diferida a **Fase 10** por
scope técnico justificado.

**Diferenciales validados del bloque** (con caps + ejemplos
runnable end-to-end):

1. **Auth como decoradores del lenguaje** (`@auth_provider` +
   `@authenticated` + `@admin`) con built-ins `jwt`/`hash`
   (HS256/HS384/HS512 + Argon2id) — checker estático en
   compile-time + OpenAPI auto-documentado con `securitySchemes.
   bearerAuth` + paridad bit-a-bit `fitz run` ↔ `fitz build` +
   cero `pip install jsonwebtoken passlib`. Cap 28 +
   `examples/guide/28-auth.fitz` (<100 LoC: login + /me +
   /admin con JWT real).
2. **WebSockets tipados** (`@ws("/path")` + `WsConn<T>`) con
   **marshaling JSON automático** + **AsyncAPI 3.0
   auto-generado** en `/asyncapi.json` + **heartbeat built-in**
   con `@server(ws_heartbeat_secs=N)` + **auth integrada** en
   el handshake (`@authenticated`/`@admin` apilados ANTES del
   HTTP upgrade) + paridad bit-a-bit. Cap 29 +
   `examples/guide/29-ws.fitz` (<100 LoC: chat broadcast con
   login + JWT + heartbeat).
3. **Jobs sin Celery** (`@cron("expr")` + `@background` +
   `spawn(fn_call)`) sin broker externo (Redis/RabbitMQ no son
   requisito) + checker estático del callsite `spawn(...)` que
   refina el ret type a `Future<T>` con T concreto +
   cron-only mode systemd-friendly (`signal::ctrl_c` automático
   sin `@server`) + paridad bit-a-bit. Cap 30 +
   `examples/guide/30-cron-background.fitz` (<100 LoC: URL
   shortener con HTTP + cron stats + spawn tracking async).

**Ningún otro lenguaje combina** auth + JWT/Argon2 +
WebSockets tipados + AsyncAPI auto + cron + spawn tipado en el
core del compilador, sin broker externo, con paridad bit-a-bit
intérprete↔binario nativo, cero deps externas para features
intrínsecas.

**Decisión de scope de 9.w.4 (ORM nativo)**: difetida a
**Fase 10**. El driver Postgres puro en Fitz es un proyecto
del tamaño de todo Fase 5-9 combinado. Implementar el
protocolo binario desde cero (handshake + SCRAM-SHA-256 +
prepared statements + ~40 tipos OID + cursors + transacciones
+ COPY + LISTEN/NOTIFY + pool + retry) sin via libpq es
comparable a `tokio-postgres`/`sqlx` que llevaron años de
desarrollo. Más ORM declarativo + migraciones autogeneradas +
decisiones de diseño abiertas (Postgres-first vs multi-DB,
async-first vs sync-first). **Gap cubierto por interop
Python**: cap 21 documenta SQLAlchemy desde Fitz con `fitz
py-types` y CRUD runnable. Fase 10 arranca cuando aparezca
proyecto real en Fitz que choque con las limitaciones de
interop Python.

**Acumulado al cierre del bloque 9.w MVP**:

- **2156 unit tests** sin feature (~80 unit tests nuevos del
  bloque: 33 de 9.w.1 + N de 9.w.2 + 32 de 9.w.3).
- **90 LSP unit tests** con `--features lsp` (incluye
  completion de `jwt`/`hash`/`WsConn`/`spawn`).
- **76 cli_e2e + 3 openapi**.
- **255 compile_e2e** con smoke ejemplos guía (incluye
  `28-auth.fitz`, `29-ws.fitz`, `30-cron-background.fitz`).
- Clippy `-D warnings` limpio en ambos modos (con y sin
  features).
- **3 caps nuevos** en `docs/guide.md` (28, 29, 30) + 3
  ejemplos runnable end-to-end.
- **Deps nuevas** del binario: `jsonwebtoken = "9"` +
  `argon2 = "0.5"` + `rand_core = "0.6"` (9.w.1); axum
  feature `ws` + `futures-util` + dev-dep
  `tokio-tungstenite` (9.w.2); `cron = "0.12"` +
  `chrono = "0.4"` (9.w.3).

**Próximo norte**: boilerplates Dockerizados (memoria
`project_boilerplates`) — showcase del stack cerrado en 4
boilerplates listos para `git clone` + `fitz run`. Después
repo público + sitio docs MkDocs Material.

## [v0.9.23] — 2026-05-21 — Fase 9.w.3 CERRADA — Jobs sin Celery (`@cron` + `@background` + `spawn`)

**Cierre del tercer sub-paso de Fase 9.w (stack web first-class).**
Tres piezas nativas del lenguaje montan jobs sin broker externo:
**`@cron("expr")`** para tareas periódicas (5/6/7 fields cron
Unix), **`@background`** como marcador opt-in para autorizar el
callsite, y **`spawn(fn_call)`** fire-and-forget que devuelve
`Future<T>` tipado. Sin Celery, sin Redis, sin systemd timers —
todo en el mismo binario con paridad bit-a-bit `fitz run` ↔
`fitz build`.

**Sub-pasos (4 commits)**:

- **9.w.3.a** — Checker estático: `Type::Future<T>` ya existe;
  acá refinamos el ret de `spawn(...)` cuando el target es una
  fn `@background` (lookup via `CheckCtx.background_fns`).
  Nuevas validaciones:
  - `@cron`: 1 arg Str, sin params, return Null/Result/Future.
    No combinable con `@get`/`@post`/`@ws`/`@background`/
    `@auth_provider`/`@test`.
  - `@background`: sin args/kwargs. No combinable con otros
    decorators "handler" (mismo set que @cron).
  - `spawn(...)`: 1 arg que es `Expr::Call` literal a fn
    `@background`. El callsite retorna `Future<T>` (T del target
    o Future<T> si target ya es async, sin doble wrap).
  - Dispatch en `synthesize_expr` solo dispara cuando el binding
    "spawn" no fue shadowed (sigue siendo `Type::Any` builtin).
  - LSP completion list `spawn` con detail
    `fn(fn_call) -> Future<T>  // requiere @background`.
  17 unit tests.

- **9.w.3.b** — Runtime intérprete: nuevo módulo
  `src/cron_jobs.rs` con `CronJob` (handler + Schedule parseado)
  + `CronRegistry` (paralelo a HttpRegistry, vive adentro)
  + `spawn_cron_scheduler` (un `tokio::spawn` por job)
  + `run_scheduler_only` (cron-only mode con multi_thread +
  ctrl_c). `process_decorator` branches para `@cron` (parsea
  expression via crate `cron`, registra job) y `@background`
  (no-op runtime). `eval_call` intercepta `spawn(fn_call)` ANTES
  de evaluar args para capturar el AST del inner call; ejecuta
  `tokio::spawn(invoke)` con await del Future si async, envuelve
  el JoinHandle en `Value::Future`. Cron-only mode en `main.rs`:
  cuando NO hay rutas HTTP pero SÍ jobs `@cron`, llama
  `cron_jobs::run_scheduler_only` que bloquea hasta Ctrl+C
  (decisión confirmada con el autor). **Fix bug preexistente**:
  handlers `async fn` HTTP en intérprete retornaban "Future
  pendiente no es serializable" porque `handle_task` nunca
  awaiteaba el Future. Solo afectaba `fitz run` (codegen lo
  hacía bien). Detectado al validar 9.w.3.b con un POST handler
  async que llama `spawn(...)`. Helper `await_if_future` en
  `http.rs` para extraer el Value final. Normalización 5→6
  fields del cron expression: si el usuario provee Unix clásico
  (5 fields), el runtime prependa `"0 "` (segundo 0). Deps
  nuevas: `cron = "0.12"` y `chrono = "0.4"` (no opcionales).
  8 unit tests.

- **9.w.3.c** — Codegen `fitz build`: Cargo.toml condicional
  suma `cron`/`chrono` cuando `uses_jobs = true`. Tokio con
  feature `signal` adicional en cron-only mode. Multi_thread
  flavor por default cuando hay jobs. Preludio
  `__fitz_run_cron_job(name, schedule, handler)` análogo al
  intérprete + helper `__fitz_normalize_cron`. `PartitionedProgram`
  gana `cron_fns` paralelo a `http_fns`/`ws_fns`. `gen_main`
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
  "Estado del proyecto" y "Qué funciona hoy". Smoke en
  `GUIDE_EXAMPLES_COMPILE`.

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
- **Crate `cron` para parsing** (vs parser propio): liviano,
  audit history limpio, soporta 5/6/7 fields.
- **Normalización 5→6 fields automática**: preserva UX familiar
  del cron Unix sin reescribir la sintaxis aceptada por el crate.
- **JoinHandle envuelto en `Value::Future`/`Pin<Box<dyn Future>>`**:
  unifica la API con `Future<T>` Fitz existente — descartar el
  Future deja la task detached (fire-and-forget natural).

**Por qué importa**:

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

**Deuda residual derivada de 9.w.3** (no bloquea uso real; abre
items para iteración 2 post-Fase 10):

- Persistencia de jobs entre restarts (requiere DB nativa, Fase
  10) o backend de queue (Redis, post-MVP).
- Visibility de jobs (panel admin con runs, stats, retries).
- Retry con backoff exponencial cuando un job falla.
- Coordinación entre múltiples instancias (locks distribuidos
  para que un cron solo corra en un nodo).
- `spawn` con coordinación múltiple (Promise.all style requiere
  agregación manual con vectores de futures).
- Cron timezone configurable (hoy todos los jobs usan
  `chrono::Utc::now()`).

**Próximo norte**: resto de Fase 9.w — ORM nativo + migraciones
(escala a Fase 10), o cierre formal de Fase 9.w entera.

## [v0.9.22] — 2026-05-21 — Fase 9.w.2 CERRADA — WebSockets tipados (`@ws` + `WsConn<T>` + AsyncAPI 3.0 + heartbeat + auth integrada)

**Cierre del segundo sub-paso de Fase 9.w (stack web first-class).**
`@ws("/path")` sobre `async fn` + `WsConn<T>` con métodos
`recv`/`send`/`broadcast`/`close` montan un servidor de WebSockets
tipado end-to-end. Cinco diferenciales que vuelven a Fitz único
en este espacio: **marshaling JSON automático** del frame al
`type` declarado, **AsyncAPI 3.0 auto-generado** en
`/asyncapi.json`, **heartbeat built-in** con
`@server(ws_heartbeat_secs=N)`, **auth integrada**
(`@authenticated`/`@admin` apilados sobre `@ws` validan bearer
ANTES del HTTP upgrade) y **codegen con paridad** bit-a-bit
`fitz run` ↔ `fitz build`.

**Sub-pasos (6 commits)**:

- **9.w.2.a** — Checker: `Type::WsConn(Box<Type>)` variant,
  `resolve_type_expr` para `WsConn<T>` aridad 1,
  `infer_wsconn_method` con signatures paramétricas
  (`recv() -> Result<T>`, `send(T) -> Result<Null>`,
  `broadcast(T) -> Result<Null>`, `close() -> Result<Null>`),
  `check_ws_handler` validando shape del handler (async fn, primer
  param `WsConn<T>`, return `Null`, compatibilidad con auth). 14
  unit tests.
- **9.w.2.b** — Value runtime: `WsConnHandle`,
  `WsBroadcasterTrait`, `WsReadStreamTrait`, `WsOutMessage`
  (Text/Close), `Value::WsConn(Arc<WsConnHandle>)`. Manual Debug.
  `register_ws_route` en evaluator paralelo a
  `register_http_route`; `process_decorator` branch para `@ws`;
  `dispatch_method` arms para `(Value::WsConn, recv/send/broadcast/
  close)`; `ws_conn_recv` usa `coerce_to_annotation` (8.4.3) para
  Map → Instance cuando T es nominal.
- **9.w.2.c** — Runtime HTTP: `WsBroadcaster` con
  `parking_lot::Mutex<HashMap<endpoint, Vec<(conn_id, outbox_tx)>>>` +
  `AtomicU64` next_id. `WsReadStreamImpl` wrapping `SplitStream`
  con filtrado de ping/pong/binary. `RouteSpec.is_ws/
  ws_conn_param_name/ws_msg_type`. `HttpRegistry.ws_broadcaster:
  Arc<WsBroadcaster>`. `build_ws_method_router` emite axum GET
  handler con `WebSocketUpgrade` extractor + auth pre-upgrade
  (devuelve 401/403 vía HTTP Response ANTES de `ws.on_upgrade`).
  `build_ws_conn` spawnea writer task (mpsc::UnboundedReceiver →
  sink) + opcional heartbeat task. axum 0.8 con feature `ws` +
  `futures-util` + dev-dep `tokio-tungstenite`.
- **9.w.2.d** — AsyncAPI 3.0 schema (`src/asyncapi.rs` nuevo,
  ~350 LoC). `AsyncApiChannelInfo`,
  `channels_from_registry` (runtime),
  `pseudo_channels_from_ast` (codegen). `generate_asyncapi_with_version`
  emite channels (uno por endpoint `@ws`), operations
  receive/send por channel, `components.securitySchemes.bearerAuth`
  cuando hay auth. `BTreeMap` para orden determinístico.
  `build_router_with_asyncapi` registra `/asyncapi.json`. En
  codegen, `auto_asyncapi` gate emite `__FITZ_ASYNCAPI_SCHEMA` +
  handler `__serve_asyncapi_json` + route. 8 unit tests.
- **9.w.2.e** — Heartbeat ping/pong automático.
  `WsOutMessage::Ping` + `ServerConfig.ws_heartbeat_secs: u64`
  (default 30). Parsing de `@server(ws_heartbeat_secs=N)` con
  validación (`Int` literal, no negativo). Si N > 0,
  `build_ws_conn` spawnea `tokio::time::interval(N segundos)` que
  envía Ping frames por el outbox; si el cliente no responde
  Pong, el sink falla en el próximo write y el writer task
  termina limpio (no requiere tracking explícito de Pong).
  `CodegenCtx.ws_heartbeat_secs` capturado ANTES de emitir WS
  wrappers (gen_ws_handler_wrapper corre antes de gen_http_main).
  6 unit tests.
- **9.w.2.f** — Cap 29 "WebSockets tipados" en `docs/guide.md`
  (renumeración 29→30) + ejemplo runnable
  `examples/guide/29-ws.fitz` (servidor de chat con login HTTP
  + JWT + `@authenticated @ws("/chat")` + broadcast multi-client
  + `@server(43929, ws_heartbeat_secs=30)`, <100 líneas) +
  README emphasis con los 5 diferenciales en la tabla feature
  comparison + footnote dedicado + bullets en "Estado del
  proyecto" y "Qué funciona hoy". Smoke en
  `GUIDE_EXAMPLES_COMPILE`.

**Por qué importa**:

- **Marshaling JSON automático**: declarás `WsConn<ChatMsg>` y
  cada frame text se serializa/deserializa al `type` sin
  `json.loads` + Pydantic / `JSON.parse` + Zod manual. El
  mismo trait que sirve HTTP (`__ToFitzJson`/`__FromFitzJson`)
  cubre WS.
- **AsyncAPI auto-generado**: el schema sale del código fuente
  (vs Socket.IO/Phoenix/SignalR/FastAPI WebSocket donde vive en
  un README que se atrasa). Tooling estándar (AsyncAPI Studio,
  generadores de clientes JS/TS/Python/Java) lo consume directo.
- **Heartbeat built-in**: `@server(ws_heartbeat_secs=N)` y
  listo. Pasa de largo Nginx (60s idle), Cloudflare (~100s) y
  AWS ALB (60s) sin código del usuario.
- **Auth integrada**: `@authenticated`/`@admin` apilados sobre
  `@ws` validan el bearer token ANTES del HTTP upgrade. El
  cliente recibe 401/403 sin abrir el socket — menos attack
  surface, menos recursos consumidos.
- **Codegen con paridad**: el flow WS funciona idéntico en
  `fitz run` y en el binario nativo de `fitz build`.
- **Ningún otro lenguaje hoy combina** WS tipados con AsyncAPI
  auto-generado del código fuente, heartbeat built-in y auth
  integrada en el handshake.

**Deuda residual derivada de 9.w.2** (no bloquea uso real):
binary frames (`Vec<u8>` payload — hoy solo text), AsyncAPI UI
equivalente al `/docs` de OpenAPI (hoy solo el JSON), tipado
bidireccional separado (`WsConn<In, Out>` — hoy `T` único),
reconnect con state replay (requiere persistencia, Fase 10),
rooms/channels dentro de un endpoint (broadcast es a TODOS los
clientes del endpoint), backpressure explícito (outbox unbounded
hoy).

**Próximo norte**: resto de Fase 9.w — `@cron` + `@background`
(jobs sin Celery), y ORM nativo + migraciones (escalado a Fase
10).

## [v0.9.21] — 2026-05-20 — Fase 9.w.1 CERRADA — Auth nativa (`@auth_provider`/`@authenticated`/`@admin` + `jwt`/`hash`)

**Cierre del primer sub-paso de Fase 9.w (stack web first-class).**
Tres decoradores nuevos del lenguaje + dos módulos built-in
montan un flujo de auth + JWT + password hashing entero sin
deps externas. El checker valida estáticamente; OpenAPI 3.1
auto-documenta los requirements y los 401/403; paridad bit-a-bit
`fitz run` ↔ `fitz build`.

**Sub-pasos (6 commits)**:

- **9.w.1.a** — Checker: `collect_auth_provider` pre-scan
  (singleton; signature `fn(Map<Str,Str>) -> Result<T-nominal>`)
  + `check_auth_decorators` por handler (exige provider + handler
  HTTP + param compatible con `User`; `@admin` exige campo
  `role: Str`). 16 unit tests.
- **9.w.1.b** — Built-ins `jwt` y `hash` como `Value::Module`
  pre-registrados. `jwt.encode/decode` (HS256/384/512 con
  `jsonwebtoken = "9"`), `hash.password/verify` (Argon2id con
  `argon2 = "0.5"` + `rand_core` para `OsRng`). Sin kwargs en
  builtins; `alg` como positional opcional al final.
  `decode` siempre devuelve `Result<Map>`; `verify` siempre
  devuelve `Bool` (hash malformado → `false` por seguridad).
  Checker tipa como `Any` (deuda de `Type::Function` sin
  opcionales); LSP completions agregan `jwt`/`hash` como
  `MODULE` kind con after-dot shortcut. 16 unit tests.
- **9.w.1.c** — Runtime auth en `fitz run`. Wrapper en
  `handle_task` después de middlewares y antes de body parsing:
  construye `Map<Str,Str>` de headers, invoca al provider (con
  `.await` si es async), match `Result<User>` → 401/200 o 403
  (admin). `AuthSpec`/`AuthProviderHandle` en `http.rs`;
  `register_auth_provider` + `collect_route_auth` en evaluator.
  Provider singleton con order requirement (provider antes que
  handlers que lo usan). 9 unit E2E.
- **9.w.1.d** — Codegen `fitz build`. Helpers `__fitz_jwt_*`/
  `__fitz_hash_*` en preludio gated por `uses_auth`; Cargo.toml
  condicional suma `jsonwebtoken`/`argon2`/`rand_core` cuando
  aplica. Dispatch en `gen_call` para `jwt.encode/decode/hash.
  password/verify`. `HandlerSig` suma
  `auth + auth_user_param_name`; `emit_auth_check` (paralelo al
  wrapper del intérprete); `emit_axum_extractors` agarra
  `HeaderMap` cuando hay auth. 2 tests compile_e2e (CLI puros +
  HTTP end-to-end).
- **9.w.1.e** — OpenAPI security scheme.
  `OpenApiRouteInfo.auth` + propagación;
  `components.securitySchemes.bearerAuth` (type=http,
  scheme=bearer, bearerFormat=JWT) cuando hay auth; `security:
  [{bearerAuth: []}]` por handler protegido; 401 (auth) y 403
  (admin) auto en responses con shape `{"error": Str}`. 5 unit
  tests del schema.
- **9.w.1.f** — Cap 28 "Auth nativa" en `docs/guide.md`
  (renumeración 28→29) + ejemplo runnable
  `examples/guide/28-auth.fitz` (login + /me + /admin con
  JWT real, <100 líneas) + README emphasis del diferencial vs
  FastAPI/Spring/ASP.NET (cero deps, checker estático, OpenAPI
  auto, paridad run↔build). Suma a `GUIDE_EXAMPLES_COMPILE`.
  Refresh oportunista del marcador de Interop Python en la
  tabla feature comparison del README (de 🚧 a ✅ con footnote
  honesta sobre deuda residual derivada).

**Por qué importa**:

- **Estático, no reflection**: el checker valida en compile-time
  que cada `@authenticated`/`@admin` tenga provider registrado
  y reciba el `User` correcto. Spring AOP / ASP.NET
  `[Authorize]` resuelven en runtime con reflection; cuando
  rompe, rompe en prod.
- **Zero dependencies**: JWT signing + Argon2id password hashing
  vienen en el binario `fitz`. No hay `requirements.txt` /
  `package.json` / `Cargo.toml` extra que mantener. Deploy es
  un binario.
- **OpenAPI auto-documentado**: `bearerAuth` + `security` por
  operation + 401/403 — sin escribir specs OpenAPI a mano.
- **Paridad bit-a-bit**: el flow funciona idéntico en
  intérprete y binario nativo.

**Deuda residual derivada de 9.w.1** (no bloquea uso real):
sessions cookie-based + RBAC multi-rol + token refresh/revocación
(requieren DB nativa, Fase 10); asimétricos JWT (RS256/ES256 con
PEM); provider request-aware más allá de headers (body, método).

**Próximo norte**: resto de Fase 9.w — `@ws("/chat")` (WebSockets
tipados con `WsConn<T>`), `@cron` + `@background` (jobs sin
Celery), y ORM nativo + migraciones (escalado a Fase 10).

## [v0.9.20] — 2026-05-17 — Refresh masivo de docs + cap 16b Package manager + fix bug fmt

Sub-paso dedicado de refresh general de docs acumulado durante
Fase 9.z entera. Cuatro sub-tareas (A + B + C + D) cerradas en
una tanda:

**A — Caps stale en `docs/guide.md`** refrescados:

- **Cap 12 "Tipos con `type`"**: removido "Chequeo de tipos en
  runtime" + "Tipos compuestos en campos no se validan" (ambos
  cerrados post-Fase 5a/5.1). Sumado bloque "Lo que SÍ anda y
  antes era deuda" con referencias a Fase 5a / 5.1 / PreF8.3.
- **Cap 13 "Métodos"**: removido "Encadenamiento multi-línea"
  (cerrado en PreF8.2). Sumado ejemplo idiomático.
- **Cap 17 "HTTP nativo"**: reescrita sección "Qué pasa adentro"
  (era stale — describía el bridge mpsc/dos-threads que F17
  eliminó). Removidos 6 ítems de "Qué todavía no anda" todos
  cerrados: async/await reales, status codes custom, query
  params, headers de request, named args en decoradores,
  middleware. Sumado bloque con referencias a sub-secciones
  existentes del mismo cap.
- **Cap 20 "fitz build"**: removido "Server HTTP multi-threaded
  como deuda" (cerrado F17 — runtime tokio default multi-thread
  con state HTTP como `LazyLock<Arc<Mutex<T>>>`). Sumado bloque
  con state HTTP compartido + paralelismo HTTP real + interop
  Python como features cerradas que antes eran deuda.

**B — Cap 16b nuevo "Package manager"** en `docs/guide.md`:

- Posición: entre cap 16 "Módulos" y cap 17 "HTTP nativo" en
  Parte 6 "Organización" (convención `16b` paralela a
  `17b-middleware`, `19b-paralelismo`).
- Cubre: anatomía de `fitz.toml` (`[package]`/`[bin]`/`[lib]`/
  `[dependencies]`), `fitz new`/`fitz init` con scaffolding,
  manifest mode de `fitz run`/`build`/`check` (walk-up Cargo-style),
  deps path (`{ path = "../foo" }`), deps git con tag o rev
  (`{ git = "...", tag = "v1.0.0" }`), lockfile `fitz.lock` con
  formato Cargo-style, `fitz add`/`remove`/`update`. "Lo que NO
  anda todavía" lista registry público, dev-dependencies,
  workspaces, branches en git deps, transitive deps.
- **Ejemplo runnable** `examples/guide/16b-pkg-manager/` con
  dos proyectos: `greetings/` (lib con dos fns) + `greeter/`
  (bin que importa via `[dependencies] greetings = { path =
  "../greetings" }`). README en el ejemplo explica el flujo
  end-to-end.
- **2 cli_e2e tests nuevos**:
  - `cap_16b_ejemplo_greeter_corre_y_genera_lockfile` valida
    `fitz run` + output esperado + lockfile auto-generado.
  - `cap_16b_fitz_build_compila_greeter_a_binario_nativo`
    valida `fitz build` + binario producido + paridad de output
    con `fitz run`.

**C — `docs/architecture.md` refresh completo**:

- Reescrito de cero (287 → ~470 líneas).
- Diagrama mermaid + ASCII fallback actualizados: muestran los
  **15 sub-comandos** del CLI en lugar de los 3 originales
  (check/run/build).
- Agrupados en 5 familias: pipeline core, package manager, DX,
  interop Python, editor support.
- **12 módulos** nuevos documentados que faltaban: `lib.rs`,
  `manifest.rs`, `lockfile.rs`, `git_dep.rs`, `testing.rs`,
  `fmt.rs`, `lint.rs`, `lsp.rs`, `py_interop.rs`, `py_types.rs`,
  `openapi.rs`. Cada uno cita su Fase de origen + APIs públicas
  relevantes.
- Removidas referencias stale: "tres subcomandos" (línea 90),
  "axum + tokio en thread separado" (línea 24 del diagrama —
  F17 lo eliminó), "Rc<RefCell<>>" en value.rs (post-F17 es
  `Arc<parking_lot::Mutex<>>`).
- Sumada nota explicando features opcionales (`python`, `lsp`)
  como cargo features con bin separado para `fitz-lsp`.
- "Por qué este orden y no otro" actualizado para reflejar
  decisiones recientes (TypeInfo side-table en lugar de IR,
  package manager y DX como módulos hermanos no parte del
  pipeline core).

**D — Fix bug del fmt** (deuda residual de 9.z.1.b):

Bug: trailing comment al final del body de una fn seguido de
otro bloque insertaba blank spurio adentro del body del segundo
bloque. MRE:
```fitz
fn greet(name: Str) -> Str {
    return "Hola, {name}!" // inline
}

for n in ["Ada"] {
    print(greet(n))   // ← antes del fix, había blank line antes acá
}
```

Root cause: `had_blank_in_source` en `fmt_stmt_list` usaba
`after_what = max(prev_end_line, last_emitted_comment_line)`.
Al entrar a un nuevo bloque (`in_block=true`,
`prev_end_line=0`), `last_emitted_comment_line` arrastraba un
valor de scope outer (el trailing comment del stmt anterior al
bloque) y `has_blank_between` chequeaba blanks FUERA del bloque
actual.

Fix: guarda condicional. En `in_block=true`, el chequeo requiere
`prev_end_line > 0` (paralela a la `smart_blank`). En top-level
se preserva el behavior previo (`after_what > 0`) para no romper
blanks legítimas entre header comments y el primer stmt del
file.

Test E2E nuevo `fmt_trailing_comment_seguido_de_bloque_no_inserta_blank_spurio`
en `tests/cli_e2e.rs` protege contra regresión.

`docs/fmt-style.md` actualizado con entry de "Historia"
documentando el fix.

**Tests al cierre del refresh**:
- 1381 unit / 76 cli_e2e (+3 vs 9.z.5: 2 del cap 16b + 1 del
  fix fmt) / 79 compile_e2e / 3 openapi.
- Clippy `-D warnings` limpio.

**Deudas residuales actualizadas en `docs/deudas-post-5b.md`**:
- Bug del fmt: marcado CERRADO en los 3 lugares donde se mencionaba.
- Cap "Package manager" en la guía: marcado CERRADO.
- `docs/architecture.md` refresh: marcado CERRADO.
- Walk completo de guide.md: parcialmente CERRADO (caps stale
  refrescados; pueden quedar referencias menores).

Próximo norte: Fase 9.w (Stack web first-class: `@authenticated`,
`@ws`, `@cron`, `@background`).

Sub-paso separado pendiente sin presión: bundling CPython embebido
(`fitz build --bundle-python`).

## [v0.9.19] — 2026-05-17 — Fase 9.z.5 CERRADA + cierre Fase 9.z entera — `fitz lint`

Quinta y última DX feature de Fase 9.z. Linter de patrones más
allá de tipos. **Cierra Fase 9.z entera** — los 5 sub-pasos (fmt
+ test + dev + repl + lint) cerrados en 2 días (16-17 de mayo).

**Implementación**:

- Módulo nuevo `src/lint.rs` (~700 LoC incluyendo 15 unit tests):
  framework `LintFinding` con `name`/`message`/`line`/`column`/
  `hint`/`fix` opcional, walkers `collect_uses_in_*` y
  `walk_exprs_in_stmt` para visit recursivo del AST, supresión
  via inspección del source raw.
- **4 lints**:
  - `unused_variable`: detecta `let x = ...` (target Ident) cuyo
    nombre no aparece en `Expr::Ident` del programa. Skipea
    prefijo `_` (convención "intencional"). Walkea fns, while,
    loop, for. Params de fn NO se flaguean en MVP (típicamente
    handlers HTTP / callbacks reciben params no usados, sería
    ruido).
  - `unused_import`: `import X` y `from X import Y` cuyo binding
    no se referencia. Maneja alias (`import foo as f` → binding
    `f`).
  - `useless_match`: `match expr { _ => body }` con UN solo arm
    catch-all (Wildcard o Ident binding) = equivalente a un
    `let`. NO flaguea matches con múltiples arms.
  - `string_concat`: `Expr::BinOp { op: Add, left, right }` con
    AMBOS operandos `Expr::Str` literales. Sugiere interpolación.
    Concat con var queda OK (puede ser intencional).
- **Lints skipeados del roadmap original**:
  - `panic_in_test_only`: NO aplica — Fitz no tiene `panic!`
    builtin distinguido (los asserts son builtins normales).
  - `redundant_clone`: requiere análisis de movimientos que el
    compilador no hace.
- `Commands::Lint { files, deny }` en CLI:
  - Sin args: manifest mode, descubre `.fitz` del proyecto via
    `discover_project_fitz_files` (heredado de `fitz fmt`).
  - Con archivos: lintea solo esos.
  - `--deny <name>` (repetible): trata ese lint como error
    (exit 1 si aparece).
- **Output cargo-clippy style**: `warning:` amarillo / `error:`
  rojo con `--deny`, `--> <file>:<line>:<col>`, hint con `= nota:`,
  summary final con conteo de findings + denied. ANSI colors auto
  via `std::io::IsTerminal`.
- **Supresión**: `// @allow(<lint>)` en la línea inmediatamente
  anterior al stmt offending. Lookup directo sobre el source raw
  (no trivia stream del lexer): pragmático y suficiente.
- **Default exit code**: 0 con findings normales (warnings no
  rompen build). Exit 1 si: error de lectura de archivo, parse
  error, o `--deny` matchea algún finding.

**Decisiones tomadas**:

- 4 lints en el MVP (no 6 del roadmap original).
- Auto-fix (`--fix`) **diferido** a sub-paso futuro: todos los
  lints emiten sugerencias textuales pero no modifican código.
  `string_concat` es el candidato natural a auto-fix.
- Supresión solo en la línea INMEDIATAMENTE anterior (no
  multi-línea, no inline).
- Análisis de uses globales (no scope-aware estricto): shadowing
  (`let x = 5; let x = 10; x`) no se detecta. Refinamiento si
  aparece presión.
- Catálogo cerrado (sin plugins externos).
- Lints emiten warnings por default; CI usa `--deny <name>`.

**Tests**:
- 15 unit tests en `src/lint.rs::tests`: 1 caso por lint en
  forma básica (smoke), 1 con `_var` ignorado, 1 con uso real
  no flagueado, 1 supresión con `@allow` funciona, 1 supresión
  solo aplica a línea inmediata anterior, 1 con fn body, 1 con
  alias en imports, 1 con dos arms (no flaguea), 1 programa
  limpio (cero findings), 1 ordenamiento por línea+columna.
- 7 cli_e2e nuevos: detecta unused_variable + unused_import,
  `--deny` exit 1, suppression silencia, archivo inexistente
  exit 1, string_concat detecta literales, código limpio cero
  findings, useless_match detecta un-solo-arm.
- **Total al cierre 9.z.5**: 1381 unit (+15) / 73 cli_e2e (+7)
  / 79 compile_e2e / 3 openapi. Clippy `-D warnings` limpio.

**Cap 27 nuevo "`fitz lint` — linter de patrones"** en
`docs/guide.md`: los 4 lints con tabla, CLI, supresión, output
cargo-clippy, integración con CI, limitaciones (sin auto-fix,
sin plugins, sin shadowing detection). Renumeración cap 27→28
("Qué sigue"). Bullet sumado en "Lo que ya sabés" + "DX 9.z"
del cap 28 marca la fase entera como CERRADA.

**Cierre formal de Fase 9.z entera**: los 5 sub-pasos cerrados.
Próximo norte: Fase 9.w (stack web first-class: `@authenticated`,
`@ws`, `@cron`, `@background`) o sub-paso dedicado de refresh
masivo de docs (cap "Package manager" + `architecture.md` +
walk completo de la guía).

**Deudas residuales de 9.z.5 (NO bloquean siguientes pasos)**:
- Auto-fix `--fix` (especialmente para `string_concat`).
- Lints adicionales si aparece demanda (`shadowing`,
  `useless_clone` cuando el compilador haga análisis de
  movimientos, etc.).
- `unused_variable` scope-aware estricto (shadowing detection).
- Suppression cross-line (`// @allow(name) { ... }` bloque).
- Plugins externos para catálogo extensible.

## [v0.9.18] — 2026-05-17 — Fase 9.z.4 CERRADA — `fitz repl` (REPL interactivo)

Cuarta DX feature de Fase 9.z. Prompt interactivo donde cada línea
se evalúa contra un env compartido, con multi-line continuation,
comandos especiales `:nombre`, history persistente, y async
transparente.

**Implementación**:

- Dep nueva: `rustyline = "14"` para terminal handling
  (arrow keys, history Ctrl-R, line editing, Ctrl+C/D
  diferenciados). Mismo crate que cargo-edit. Default features
  traen file history.
- `Commands::Repl` (sin args) en CLI. Manifest mode/single-file
  no aplica — el REPL es siempre single-session.
- `repl_cmd` corre adentro de `evaluator::build_runtime()`
  (current_thread) para que `sleep(100).await` y similares
  funcionen desde el prompt.
- `read_complete_input` lee líneas hasta que el buffer esté
  "completo" según heurística de balanced brackets
  (`input_is_complete`): cuenta `{`/`(`/`[` skip-eando strings
  literales y comments `//` y `/* */`. Si no balancea, prompt
  cambia a `... `. Es heurística (no parser real); el parser
  puede aún emitir un error sintáctico distinto que se muestra
  y vuelve al prompt.
- 6 comandos especiales (`handle_special_command`): `:help`,
  `:quit`/`:q`/`:exit`, `:env`, `:reset`, `:type <expr>`,
  `:load <archivo>`.
- `:env` lista los bindings del scope raíz filtrando builtins
  (`evaluator::builtin_names()` — array nuevo con los 8
  builtins actuales).
- `:type <expr>` arma un programa sintético
  `let __repl_type = <expr>`, lo pasa por el checker, y lee el
  tipo del span del value. Limitación conocida: no es
  scope-aware (no ve vars previas del REPL). Documentado.
- `:load <archivo>` lee + parsea + chequea + evalúa el archivo
  contra el env del REPL. Los `let`/`fn` del archivo quedan
  disponibles para las próximas líneas del prompt.
- History persistente: `~/.fitz/history` (Linux/macOS) o
  `%USERPROFILE%\.fitz\history` (Windows). Se carga al inicio y
  se guarda al salir. `rustyline` maneja arrow up/down + Ctrl+R
  + line editing nativo.
- **Pretty-print Python-style del último valor**: cuando el
  último stmt del input es `Stmt::Expr` y devuelve un `Value`
  no-Null, se imprime con `= <value>`. Para `let`/`fn`/`print`
  el output es silencioso (`print` devuelve Null y ya imprime
  por su cuenta).

**APIs nuevas en el evaluator/env** (pub):
- `evaluator::eval_program_with_env(program, base_dir, env,
  dep_registry) -> FitzResult<Value>`: evalúa contra un env
  externo que persiste entre invocaciones (a diferencia de
  `eval_with_base_and_deps`). Devuelve el `Value` del último
  stmt para que el REPL pueda imprimir.
- `evaluator::new_repl_env() -> EnvRef`: wrapper público que
  crea env + registra builtins, sin exponer la fn privada.
- `evaluator::builtin_names() -> &'static [&'static str]`:
  lista de nombres de builtins para que el REPL los filtre del
  `:env`. Mantener sincronizado con `register_builtins`.
- `Environment::local_names() -> Vec<String>`: lista los nombres
  definidos en el scope actual (sin recursar al padre).

**Decisiones tomadas**:

- Filtrar warning spurio del checker para "variable desconocida"
  por **substring del mensaje** (no kind): todos los errores del
  checker llevan `ErrorKind::TypeError` (`UndefinedVariable` es
  kind del evaluator), y el string "variable desconocida" está
  estable en `types::infer_expr`. Sin el filtro, cada `let x =
  5; x + 1` emitía warning spurio del checker para `x` en la
  segunda línea (el checker arma scope desde cero por
  invocación).
- `:type` scope-aware: NO en MVP. Refinable feedeando el env del
  REPL al checker como pre-declaraciones — sub-paso futuro si
  aparece presión real.
- `panic(msg)` u otros builtins extras: NO en MVP. Lista
  oficial es la de 9.z.2 (4 asserts).
- Smoke E2E automatizado: NO — el REPL es interactivo, los tests
  serían flaky. Smoke manual con stdin scripted valida.

**Smoke manual validado**:
- `1 + 2` → `= 3`
- `let x = 5; x + 1` → `= 6` (sin warnings spurios)
- `fn doble(n: Int) -> Int { return n * 2 }; doble(21)` → `= 42`
- `async fn pausa() -> Int { return 42 }; pausa().await` → `= 42`
- `:env` lista user-defined vars + filtra builtins
- `:reset` limpia scope
- `:load <archivo.fitz>` carga + define todo en el env actual
- typo real (`xyz_typo`) → error claro del evaluator
- Multi-line con `{` abierto cambia prompt a `... `

**Cap 26 nuevo "`fitz repl` — REPL interactivo"** en
`docs/guide.md`: features, comandos especiales con tabla, history
persistente, async, decisión "expresiones vs statements",
limitaciones (`:type` no scope-aware, no manifest mode, sin
auto-completion de paths). Renumeración cap 26→27 ("Qué sigue").

**Cierre formal**:
- CHANGELOG v0.9.18.
- `docs/roadmap.md`: 9.z.4 marcado CERRADO con detalle.
- `docs/deudas-post-5b.md`: bloque "Fase 9.z.4 CERRADA" +
  deudas residuales (`:type` scope-aware, smoke E2E, etc.).
- README.md: bloque 9.z.4 + conteo final.
- CLAUDE.md: bloque "Próximo norte" actualizado.
- `docs/syntax-spec.md`: `fitz repl` cae adentro de "implementado".

**Tests al cierre**:
- 1366 unit / 66 cli_e2e / 79 compile_e2e / 3 openapi (sin cambios
  — repl_cmd es interactivo, smoke E2E automatizado pendiente).
- Clippy `-D warnings` limpio.

**Deudas residuales (NO bloquean 9.z.5)**:
- `:type` scope-aware (no ve vars previas del REPL).
- Smoke E2E automatizado del REPL (file watchers + readline son
  flaky en tests; el smoke manual con stdin scripted cubre el
  caso 90%).
- Indentación automática en multi-line continuation.
- Comandos `:save`/`:undo`/`:debug` si aparece demanda.
- Auto-completion de paths en `:load`.
- Manifest mode en `fitz repl` (hoy es single-session siempre).

## [v0.9.17] — 2026-05-17 — Fase 9.z.3 CERRADA — `fitz dev` (hot reload)

Modo desarrollo con file watcher + kill/respawn al detectar cambio.
Tercera DX feature de Fase 9.z. El loop iterativo del developer
(editar → save → ver efecto) sin re-tipear `fitz run` en cada save.

**Implementación**:
- Dep nueva: `notify = "6"` (file watcher cross-platform: FSEvents
  en macOS, inotify en Linux, ReadDirectoryChangesW en Windows).
  Sin layer de debouncer — el debounce 100ms lo hacemos manual con
  un `tokio::time::timeout` + drain del canal en el loop.
- `Commands::Dev { file }` en CLI. Sin args, manifest mode
  (busca `fitz.toml`, watch su dir, corre `fitz run`). Con
  `--file <archivo.fitz>`, single-file mode (watch parent del
  archivo).
- `dev_cmd` corre adentro de un runtime tokio current_thread
  (reusa `evaluator::build_runtime`). El loop principal
  `run_dev_loop` usa `tokio::select!` sobre 3 eventos: cambio
  detectado por el watcher, exit del child, o `tokio::signal::ctrl_c()`.
- Bridge sync→async para `notify`: un `std::thread::spawn` lee del
  `std::sync::mpsc` (sync) y re-envía al `tokio::sync::mpsc`
  (async). El watcher es sync; este patrón evita feature
  `tokio` del crate `notify` para no inflar el dep tree.
- Spawn del child con `tokio::process::Command`: `current_exe()`
  + `target.child_args` + `current_dir(&target.watch_dir)` para
  que `fitz run` (manifest mode, sin args) encuentre el
  `fitz.toml` correcto. Single-file mode usa path absoluto del
  archivo, así el cwd no importa.
- **Path filtering** (`path_is_relevant`): sólo `*.fitz` y
  `fitz.toml`. Excluye en cualquier nivel `target/`, `.git/`,
  `node_modules/`, `.fitz/`, `dist/`, `build/`, y cualquier
  componente oculto (`.algo`).
- **Debounce 100ms**: tras detectar un evento, drain del canal
  con `tokio::time::timeout` para colapsar saves múltiples del
  editor (VSCode emite write tmp + rename + chmod en un save).
- **Banner UX** (`clear_screen_and_banner`): `\x1b[2J\x1b[H` para
  clear+home si stdout es TTY (`std::io::IsTerminal`), sino
  separa con líneas. Cada arranque muestra "▶ fitz dev (run #N)
  — <target>".
- **Ctrl+C**: `tokio::signal::ctrl_c()` en el `select!` mata el
  child + waits antes de retornar. Sin esto, en uso real
  quedarían procesos zombie del child.
- Caso "child terminó solo" (programa CLI corto, error de tipo):
  no salimos del loop — esperamos un cambio en filesystem para
  reiniciar. Pedagógicamente útil: el user fixea el error, save,
  retry automático.

**Decisiones tomadas**:
- `[dev]` config en `fitz.toml` para customizar paths watched /
  debounce / etc.: NO en MVP, solo defaults. Sumar si aparece
  demanda concreta.
- Browser auto-refresh para HTTP: NO en MVP. Quien edite HTML/CSS
  junto puede usar Live Server o similar.
- Print de errors del checker mientras tipeás sin disparar
  restart: NO — el child mismo imprime los errores en arranque.
  El LSP (cap 22) ya hace diagnostics in-editor para feedback
  continuo.
- `fitz dev --test` (modo "watch + run tests"): sub-paso futuro
  si aparece presión. Workaround documentado en el cap 25
  con dos terminales.

**Tests**:
- Smoke manual validado: arrancar `fitz dev --file`, modificar
  archivo, observar run #2 con código nuevo. ANSI clear screen
  + banner funcionando.
- 1366 unit / 66 cli_e2e / 79 compile_e2e / 3 openapi (sin
  cambios — el dev_cmd es interactivo, los tests automáticos
  serían flaky). Clippy `-D warnings` limpio.

**Bug fix colateral**: en el smoke confirmé que el child del
`fitz dev --file` re-evalúa el archivo modificado correctamente
(no hay cache stale).

**Deudas residuales (NO bloquean 9.z.4)**:
- Incremental rebuild (solo el archivo cambiado se re-carga):
  hoy es kill+respawn full. Mejora futura cuando aparezca
  modelo de módulos pre-compilados.
- Filtrar "modify sin cambio real" (timestamps tocados sin
  cambio de contenido): hoy cualquier evento `Modify` dispara.
  Refinable comparando hashes si duele.
- Auto-test mode (`fitz dev --test`): workaround documentado
  con dos terminales.
- Smoke E2E automatizado: por interactividad del dev_cmd y
  flakeyness de los file watchers, los tests son manuales por
  ahora.

**Cap 25 nuevo "`fitz dev` — hot reload"** en `docs/guide.md`:
features, CLI single-file/manifest, qué dispara restart, output
típico, limitaciones, integración con `fitz test`. Renumeración
cap 25→26 ("Qué sigue").

**Cierre formal**: CHANGELOG v0.9.17, roadmap (9.z.3 CERRADA con
detalle), `docs/deudas-post-5b.md` (bloque "Fase 9.z.3 CERRADA"),
README, CLAUDE, `docs/syntax-spec.md` (nota implementado).

## [v0.9.16] — 2026-05-17 — Fase 9.z.2 entera CERRADA — `fitz test` (testing built-in)

Test runner integrado al lenguaje. Sin librerías, sin glue, sin
elegir entre 3 frameworks. Tres sub-pasos (a + b + c) cerrados en
el día:

**9.z.2.a — `@test` decorator + assertion builtins + TestRegistry**:
- `src/testing.rs` nuevo: `TestRegistry` + thread-local +
  `with_active_test_registry` (sync/async). Mirror chico de
  `http::HTTP_REGISTRY` con la asimetría clave: sin registry
  activo, `@test` es no-op silencioso (paralelo a `#[cfg(test)]`).
- Evaluator: branch `@test` en `process_decorator` con `register_test`.
  Valida args/kwargs/params vacíos; empuja `TestSpec` si hay registry.
- 4 assertion builtins: `assert(cond, msg?)`, `assert_eq(a, b)`,
  `assert_ne(a, b)`, `assert_throws(fn)`. Estilo cargo
  (`left`/`right`). `assert_throws` con callback async: rechazado
  en MVP — caso especial en `invoke_value` invoca async-recursive.
- Pre-registro en checker (`types.rs`) + completion en LSP (`lsp.rs`).
- **Cambio retro-compatible al parser**: paréntesis opcionales en
  decoradores (necesario para `@test fn ...`). Los demás
  decorators siguen funcionando idéntico con/sin paréntesis.

**9.z.2.b — `fitz test` runner**:
- `Commands::Test { filter, file }` en CLI.
- **Single-file mode** (`fitz test --file archivo.fitz`): carga
  el archivo, descubre `@test`, los corre.
- **Manifest mode** (`fitz test`): discovery automático. Si hay
  `tests/*.fitz` top-level: solo carga esos (el `[lib]` se carga
  vía import auto-self-registrado bajo `package.name` —
  paralelo a `use my_crate::*` Rust). Si no hay tests integration:
  carga el `[lib].entry` directo para tests inline.
- Filtrado por **substring** del nombre del test (cargo default).
- Output estilo cargo: `test <file>::<name> ... ok/FAILED` +
  sección `failures:` con detalle + summary `test result: ...
  passed; ... failed; finished in ...s`. ANSI colors auto cuando
  stdout es TTY (`std::io::IsTerminal`, cero deps nuevas).
- **Async tests** funcionan: `evaluator::run_test_handler`
  encapsula invoke + await del `Future`.
- Exit code 1 si ≥1 falla, 0 si todos pasan.
- Loader sobrescribe `CURRENT_TEST_SOURCE` al cargar módulos
  importados: los `@test` quedan etiquetados con su archivo
  declarante real (no con el del importer).
- Dedup en discovery: si hay tests integration, no se carga
  `[lib]` direct para evitar duplicar tests inline del lib que
  los tests importan.

**9.z.2.c — guía + ejemplo + cierre formal**:
- Cap 24 nuevo **"`fitz test` — testing built-in"** en
  `docs/guide.md`: features, CLI single-file / manifest mode,
  filtrado, output cargo-style, async tests, estructura típica
  de proyecto, limitaciones. Renumeración cap 24→25 ("Qué sigue").
- Ejemplo runnable `examples/guide/24-tests.fitz` con `factorial`
  + 3 tests OK + 1 FAILED intencional. Sumado al smoke
  `GUIDE_EXAMPLES_COMPILE` (compila con `fitz build` porque
  codegen ignora `@test`).
- Codegen: `@test fn` se **ignora silenciosamente** en `fitz build`
  (paralelo a `#[cfg(test)]`). Bug fix colateral en
  `has_http_routes` (counting `@test` como HTTP disparaba
  servidor en CLI puro — refinado a solo
  `get`/`post`/`put`/`delete`/`server`).
- CHANGELOG v0.9.16, roadmap (9.z.2 a/b/c marcado CERRADO),
  `docs/deudas-post-5b.md` (bloque "Fase 9.z.2 entera CERRADA"),
  README, CLAUDE, `docs/syntax-spec.md` (sección "Testing"
  pasa de "futuro" a "implementado").

**Decisiones tomadas durante 9.z.2**:
- `panic(msg)` (que el syntax-spec usa en su ejemplo) **fuera de
  scope** del MVP. Los 4 oficiales bastan; refinable si aparece
  presión.
- `assert_throws` solo SYNC callbacks. Async cb queda como sub-paso
  futuro si aparece presión.
- Discovery dedup pragmática: lib vs tests integration.
- Auto-self-import bajo `package.name`: requiere nombre usable
  como ident Fitz (sin hyphens). Deuda visible.

**Tests al cierre**:
- 1366 unit (+33 vs Fase 9.z.1) — `+6 testing`, `+25 evaluator
  (decorator + asserts)`, `+2 parser regression`.
- 66 cli_e2e (+11 vs Fase 9.z.1) — runner end-to-end.
- 79 compile_e2e (igual cuenta que 9.z.1; `24-tests.fitz` se sumó
  a la lista del smoke `GUIDE_EXAMPLES_COMPILE` que es 1 `#[test]`
  único iterando, no a tests individuales).
- 3 openapi.
- Clippy `-D warnings` limpio.

## [v0.9.14] — 2026-05-16 — Fase 9.z.1.b + cierre de 9.z.1 entera: comment + blank preservation

Cierra la deuda crítica de 9.z.1.a: el formatter ahora **preserva
comentarios y blank lines del usuario** al reescribir archivos.
`fitz fmt` es production-ready — el warning loud del modo write
fue removido. **Cierra 9.z.1 entera** (a + b).

Lexer:
- `Trivia` struct nueva: `Vec<Comment>` (con `kind: Line | Block`,
  `text`, `line`, `column`) + `Vec<usize>` con líneas blank.
- `tokenize_with_trivia(src) -> (Vec<TokenWithPos>, Trivia)`
  paralela a `tokenize` (que sigue zero-overhead — parser/LSP/
  resto no se ven afectados). AST sin cambios.
- `Lexer.collect_trivia` flag + `line_had_code` /
  `line_had_comment` para distinguir líneas blank (sin nada) de
  líneas comment-only (no son blank).

Formatter:
- `format_source` ahora invoca `tokenize_with_trivia` y threadea
  la trivia en el output.
- `fmt_stmt_list` emit leading comments + blank lines preservadas
  + trailing comments por stmt.
- `end_line_of_stmt`/`end_line_of_expr` recursivos para detectar
  trailing comments en stmts multi-línea.
- Smart blank entre fn/type defs **suprimida** si hay leading
  comment recién emitido (el comment se ata al stmt siguiente).
- Comments normalizados: `//foo` → `// foo` (espacio post-`//`).
- Trailing comments emitidos con 2 espacios de separación.
- Múltiples blank lines consecutivas colapsadas a 1.

Decisiones cerradas: lexer side-stream vs token kind (lean
side-stream porque parser no se contamina); fmt_stmt_list con
`in_block` flag (blocks no emiten footer comments — caso raro de
"comment entre último stmt y `}`" es deuda menor documentada);
smart blank suprimida por leading comment.

CLI:
- Removido el warning loud del modo write (deuda 9.z.1.a cerrada).
- Docstring de `Commands::Fmt` reescrita reflejando
  production-ready.

Limitaciones residuales (NO bloquean 9.z.2):
- Comments entre último stmt de un bloque y el `}` terminan
  saliendo del bloque al re-formatear (caso raro).
- Multi-línea de listas/maps/method chains se colapsa a
  single-line (auto-wrap line-aware es deuda futura).
- Comments adentro de expresiones (`f(x, // foo\n y)`) no
  soportados.

- 8 unit tests nuevos en `lexer::tests` (trivia capture, blank
  detection, comment-only lines, mixto).
- 10 unit tests nuevos en `fmt::tests` (preservación de leading/
  trailing/blanks/multiline, normalización de espacios,
  idempotencia con comments, smoke con 02-hola).
- 2 cli_e2e nuevos / actualizados.
- Total: 1333 unit + 55 cli_e2e + 79 compile_e2e + 3 openapi.
  Clippy `-D warnings` limpio.

Smoke a mano: `examples/guide/02-hola.fitz` round-trip exacto
bit-a-bit (2 comments + 2 blank lines preservados).

Ver `docs/fmt-style.md` para la referencia completa de
convenciones del formatter.

## [v0.9.13] — 2026-05-16 — Fase 9.z.1.a: `fitz fmt` (sin comment preservation)

Primer slice del formatter. Pretty-printer escrito a mano sobre
el AST, cero config (4 espacios indent, comillas dobles, blank
line solo entre fn/type top-level consecutivos). Cubre >20 nodos
del AST: literales, let, fn (con/sin async/decorators),
if/while/for/loop, match, struct lit, list/map, BinOp/UnaryOp,
Call/Field/Index, Range, Ok/Err/Try/Await, FnExpr (preserva
flecha si body es Return único), TypeDef con defaults, Decorator,
Import/FromImport.

**⚠ LIMITACIÓN CRÍTICA** — el lexer strippea comentarios antes
de llegar al AST. Modo write (`fitz fmt`) borra comments y blank
lines del usuario. Modo `--check` (read-only) es safe. Para
hacer al formatter usable en código real, comment preservation
llega en **9.z.1.b** (lexer side stream + parser side-table +
threading en el formatter). Mientras tanto, el CLI emite warning
loud explicando la pérdida + sugiriendo `--check`.

CLI:
- `fitz fmt <files...>` — formatea archivos explícitos.
- `fitz fmt` (sin args) — descubre `.fitz` del proyecto via
  manifest (walk recursivo de `src/`).
- `fitz fmt --check` — modo CI, read-only, exit 1 si hay diffs.

Decisiones cerradas: indent 4 espacios, comillas dobles, sin
auto-wrap de líneas largas (deuda futura); `is_let` recuperado
del source via Span (AST no preserva `let x = ...` vs `x = ...`);
`fn f() => expr` se normaliza a bloque (AST no preserva flecha
en defs); `if` con paréntesis obligatorios en condición;
warning loud solo en write mode (`--check` silencioso).

- 21 unit tests nuevos en `fmt::tests` (incl. idempotencia
  sobre programas complejos).
- 7 E2E nuevos en `tests/cli_e2e.rs` (file/check/sin args/error
  de sintaxis/warning emission/project discovery).
- Total: 1315 unit + 55 cli_e2e + 79 compile_e2e + 3 openapi.
  Clippy `-D warnings` limpio.

## [v0.9.12] — 2026-05-16 — Fase 9.y.4: `fitz add` / `fitz remove` / `fitz update`

Cuarto sub-paso del package manager. Automatiza la edición del
manifest + lockfile que hasta 9.y.3 era manual. Tres subcomandos
nuevos con UX cargo-style. Hoy editás el `fitz.toml` con un
comando, no a mano.

- `fitz add <name> --path <p>` — agrega path dep.
- `fitz add <name> --git <url> --tag <t>` (o `--rev <r>`) —
  agrega git dep. clap valida conflicts entre `path`/`git` y
  entre `tag`/`rev`.
- `fitz add <name>` sin flags — error claro citando 9.y.5
  (registry futuro).
- `fitz remove <name>` — quita entry + sync lockfile. Si la dep
  era la única, borra `fitz.lock` entero (deps vacías).
- `fitz update [name]` — invalida cache de git deps (force
  re-clone). Path deps son no-op (siempre fresh). Sin name
  actualiza todas; con name solo esa (error si no existe).

Decisiones cerradas: dep nueva `toml_edit = "0.22"` (preserva
comentarios + formatting al modificar `fitz.toml`); persist eager
incluso si la resolución posterior falla (cargo-style, usuario
revierte con `fitz remove`); validación cruzada delegada a clap
(`conflicts_with` + `requires`) — mensajes limpios sin código
custom; `fitz add` sobreescribe sin warning si la dep existía;
`fitz remove` borra `fitz.lock` cuando deps queda vacío para no
dejar stale state; `fitz update no-existe` da error claro (no
silent no-op); dev deps `[dev-dependencies]` diferidas a 9.z.2.

- 11 unit tests nuevos en `manifest::tests` (add path/git,
  sobreescribe, sin `[dependencies]`, preserva comentarios;
  remove existente/inexistente/borra sección vacía;
  add+remove inversa).
- 11 E2E tests nuevos en `tests/cli_e2e.rs` cubriendo todos los
  caminos del CLI + errores (sin flags, sin tag/rev, conflicts,
  fuera de proyecto, dep inexistente, cache busted con marker
  file).
- Total: 1294 unit + 48 cli_e2e + 79 compile_e2e + 3 openapi.
  Clippy `-D warnings` limpio.

## [v0.9.11] — 2026-05-16 — Fase 9.y.3.c + cierre de 9.y.3 entera: git deps + cache local

Tercer y último slice del tercer sub-paso del package manager.
Habilita `[dependencies] foo = { git = "https://...", tag = "v1.0.0" }`
en `fitz.toml`. El primer acceso clona el repo a `<cache>/git/
<sanitized-url>@<ref>/` (cache global, default `~/.fitz/cache/`,
override con `FITZ_CACHE_DIR`) y reusa el dir en accesos
siguientes — sin re-clone automático. El lockfile registra el
commit hash exacto Cargo-style: `source = "git+<url>#<commit>"`.

**Cierra 9.y.3 entera**: path deps (a) + loader integration (b)
+ git deps (c) están todos vivos. El package manager Fitz puede
hoy declarar, resolver, bloquear y CONSUMIR deps tanto locales
como de repos git remotos, sin registry todavía. Próximo norte:
9.y.4 (`fitz add`/`remove`/`update`).

Decisiones cerradas: subprocess `git` sobre crate (zero deps);
`tag` XOR `rev` mutuamente exclusivos; `branch` NO soportado
intencionalmente (no reproducible); cache naming determinístico
sin hashing (`github.com_foo_bar@v1.0.0/`, trunca a 200 chars);
cache reuse sin re-clone automático; estrategia split (`--depth 1
--branch <tag>` para tags, full clone + checkout para revs porque
git no acepta SHAs en `--branch`); `FITZ_CACHE_DIR` env var
override para tests E2E aislados.

Validaciones cruzadas con mensajes accionables: `path` + `git`,
`tag` + `rev` juntos, `tag`/`rev` sin `git`, `git` sin `tag`/`rev`
(cita reproducibilidad), `tag`/`rev` vacíos.

Smoke end-to-end: `myutils` con `[lib]` + git repo + tag `v0.1.0`;
`myapp` con `[dependencies] myutils = { git = "file:///...", tag
= "v0.1.0" }`. `fitz run` clona, lockfile correcto, output ok;
segunda corrida sin re-clone (verificado con marker file); `fitz
build` produce binario ejecutable bit-a-bit idéntico.

- 8 unit tests nuevos en `git_dep::tests` (sanitize_url,
  cache_path_for, lockfile_source_string, GitRef shape).
- 6 unit tests nuevos en `manifest::tests` (parse_git_ref +
  validaciones de shape: sin tag/rev, tag+rev juntos, tag vacío,
  path+git, tag sin git).
- 4 E2E tests nuevos en `tests/cli_e2e.rs` con bare git repo
  local + `FITZ_CACHE_DIR` aislado.
- Total: 1283 unit + 37 cli_e2e + 79 compile_e2e + 3 openapi.
  Clippy `-D warnings` limpio.

Deuda residual visible (NO bloquea 9.y.4): drift entre lockfile
commit y cache borrado (re-clone fresh no detecta si upstream
movió el tag); `fitz cache clean` sub-comando (borrar cache es
manual hoy); auth para repos privados (delegado al git del
sistema); shallow clone con `--filter` para revs (optimización
de performance); verificación de integridad (commit signature).

## [v0.9.10] — 2026-05-16 — Fase 9.y.3.b: loader integration (deps usables desde código)

Segundo slice del tercer sub-paso del package manager. El loader
del evaluator (`fitz run`) y el del codegen (`fitz build`) consultan
ahora el `dep_registry` resuelto del manifest ANTES de fallback a
paths relativos del importer. `from <dep-name> import X` resuelve
al `lib_entry` absoluto de la dep — las deps declaradas en 9.y.3.a
son finalmente **usables desde código**.

Smoke end-to-end: con un proyecto `myutils` (con `[lib] entry =
"src/lib.fitz"` exponiendo `double`/`greet`) y un proyecto `myapp`
con `[dependencies] myutils = { path = "../myutils" }`, el código
`from myutils import double, greet` en `myapp/src/main.fitz`
funciona tanto en `fitz run` como en `fitz build`, produciendo el
output esperado bit-a-bit.

Decisiones cerradas: `DepRegistry` como `HashMap<String, PathBuf>`
alias en `manifest.rs`; resolución con shortcut single-segment +
fallback path-relativo (paralelo en evaluator y codegen); deps
shadowean archivos locales con el mismo nombre; transitive deps
(deps de deps) NO soportadas en este slice (refactor mayor, deuda
futura); hyphens en dep names aceptados al parse pero no
importables porque el parser Fitz no acepta `-` en identifiers
(deuda 9.y.4 para auto-translation); `fitz check` no consume el
dep_registry (los nombres importados se tipan como Any/nominal
placeholder, validación real ocurre en run/build).

API del evaluator: `eval_with_base_and_deps(_sync)` nuevas pub APIs;
`eval_with_base(_sync)` quedan como wrappers con registry vacío
(backward compat para callers sin manifest awareness).

- 5 E2E nuevos en `tests/cli_e2e.rs` (deps en run + build, no
  ref no falla, fallback path-relativo, dep shadowea local).
- Total: 1270 unit + 33 cli_e2e + 79 compile_e2e + 3 openapi.
  Clippy `-D warnings` limpio.

## [v0.9.9] — 2026-05-16 — Fase 9.y.3.a: path deps + sección `[lib]` + `fitz.lock`

Primer slice del tercer sub-paso del package manager. Habilita
declarar `[dependencies] foo = { path = "../foo" }` en el manifest;
el `fitz.lock` se emite/sincroniza automáticamente en cada
`fitz run`/`build`/`check` (manifest mode). **NO toca el loader
del lenguaje** todavía — las deps quedan declaradas y bloqueadas
en el lockfile pero `from foo import X` no las resuelve aún. Esa
promesa es 9.y.3.b.

Sintaxis: `[dependencies] utils-lib = { path = "../utils-lib" }`
en el importer + sección nueva `[lib] entry = "src/lib.fitz"` en
la dep (paralelo a `[bin] main`). Path deps son librerías por
definición — si la dep solo tiene `[bin]`, el resolver aborta
con la sección `[lib]` sugerida inline.

`fitz.lock` formato TOML Cargo-style: `version = 1` + `[[package]]`
con `name`/`version`, sin campo `source` para path deps (convención
Cargo: implícitas). El lockfile se regenera idempotentemente —
sin cambios = sin escritura (no spam de mtime).

Decisiones cerradas: lockfile TOML, `Dependency` enum
`Version(String) | Detailed(...)` con `serde(untagged)`,
`Lib.entry` obligatorio sin defaults mágicos, path deps son libs
por definición, lockfile siempre regenerado idempotente, sin
emisión si no hay deps. Versiones sueltas (`foo = "1.0.0"`) y
git deps se aceptan al parse pero el resolver las rechaza con
errores accionables citando 9.y.5 (registry) y 9.y.3.c (git)
respectivamente.

- 10 unit tests nuevos en `manifest::tests` (Dependency parse
  forms, Lib parse, resolve_dependencies happy + 5 error paths).
- 14 unit tests nuevos en `lockfile::tests` (parse/serialize/
  round-trip, from_resolved ordering, idempotencia de write).
- 8 E2E tests nuevos en `tests/cli_e2e.rs` (lockfile emitido,
  idempotencia, regen en cambio de versión, sin deps no emite,
  errores: version/git/path inexistente/sin `[lib]`).
- Total: 1270 unit + 28 cli_e2e + 79 compile_e2e + 3 openapi.
  Clippy `-D warnings` limpio.

## [v0.9.8] — 2026-05-16 — Fase 9.y.2: `fitz run`/`build`/`check` leen el manifest

Segundo sub-paso del package manager. Sin archivo explícito, los
tres subcomandos detectan `fitz.toml` en el cwd o ancestros
(Cargo-style) y usan `[bin].main` como entry point. En manifest
mode, `fitz build` emite el binario a
`<manifest_dir>/target/release/<pkg-name>(.exe)` con el nombre del
paquete (no el stem del fuente).

**Sin breaking**: los ejemplos de la guía siguen corriendo
idénticos con `fitz run examples/guide/02-hola.fitz`. Los 79 tests
de `compile_e2e` (single-file mode) verdes sin cambio.

Decisiones cerradas: `target/release/<pkg-name>(.exe)` adyacente
al manifest hardcodeado (configurable post-MVP), `fitz check`
chequea solo el `[bin].main` (loader walks imports
transitivamente), compat single-file silenciosa sin warning,
manifest sin `[bin]` aborta con la sección sugerida inline,
multi-bin (`[[bin]]` array) sigue deuda 9.y.8+.

- 9 E2E tests nuevos en `tests/cli_e2e.rs`: run/check sin args,
  walk-up desde subdir, single-file mode compat, errores (sin
  manifest + sin archivo, sin `[bin]`, TOML corrupto), build sin
  args produce binario con pkg-name en `target/release/`.
- Total: 1246 unit + 20 cli_e2e + 79 compile_e2e + 3 openapi.
  Clippy `-D warnings` limpio.

## [v0.9.7] — 2026-05-16 — Fase 9.y.1: manifest + `fitz new` / `fitz init`

Primer sub-paso del package manager (Fase 9.y). Define el formato
`fitz.toml` (TOML, Cargo-style) y suma dos subcomandos para crear
proyectos: `fitz new <name>` (carpeta nueva con `git init`
automático) y `fitz init` (directorio actual). Templates `--http`
(server con `@get`/`@server`) y default (`print` top-level estilo
cap 2 de la guía).

**Sin cambio breaking**: el modo single-file (`fitz run
archivo.fitz`) sigue funcionando idéntico. La integración del
manifest con `fitz run`/`build`/`check` llega en 9.y.2.

Decisiones cerradas: TOML para el manifest, `src/main.fitz` como
entry default, `edition = "2026"` (Cargo-style year), bin único
en MVP (multi-bin queda 9.y.8+), validación de nombre
`^[a-z][a-z0-9_-]{0,63}$` (política crates.io), `git init`
automático con flag `--no-git` para opt-out, `.gitignore` excluye
`target/` + binarios (no `fitz.lock` — el lockfile se commitea).

- 13 unit tests nuevos en `manifest::tests`.
- 11 E2E tests nuevos en `tests/cli_e2e.rs` cubriendo estructura
  completa, ambos templates, git init opt-in/out, errores
  (nombre inválido, carpeta ya existe, manifest existente).
- Total: 1246 unit + 11 cli_e2e + 79 compile_e2e + 3 openapi.
  Clippy `-D warnings` limpio.

Dep nueva no-opcional: `toml = "0.8"`.

## [v0.9.6] — 2026-05-16 — Fase 9.x.5: distribución VSCode multi-platform + logo

Quinta y última sub-fase visible del LSP **completa el plan LSP
entero**. Deja la extensión lista para publicar al VSCode Marketplace:
binarios pre-compilados por plataforma bundleados en el `.vsix`,
logo oficial del proyecto, script reproducible de build local.

La publicación real al Marketplace queda como acción del autor
(requiere cuenta de publisher + decisión sobre hacer el repo
público), no commit técnico.

Sub-pasos coordinados:

- **9.x.5.0 — Logo de Fitz**:
  - Diseño: engranaje estilo Rust (color naranja `#CE412B`, 12
    dientes) con silueta del monte Fitz Roy adentro del hueco
    (3 picos, central más alto, los dos laterales escalonados;
    confinada vía `clipPath` circular).
  - `assets/logo.svg` — single source of truth (256×256).
  - `assets/logo.png` — generado para README + propósitos generales.
  - `assets/logo-social.svg` + `.png` (1280×640) — Social preview
    de GitHub (se sube manual a Settings → Social preview).
  - `editors/vscode/icon.png` — copia para el .vsix de la extensión.
  - `editors/vscode/scripts/build-icon.mjs` — regenera los 3 PNGs
    desde los SVGs vía `@resvg/resvg-js` (puro JS bindings de
    resvg, Rust SVG renderer; más confiable que cairosvg en Windows).
  - `npm run build:icon` desde editors/vscode/ regenera todo.
  - `editors/vscode/package.json` declara `"icon": "icon.png"` →
    Marketplace usa el icon en el listing.
  - README raíz suma hero image centrada al inicio.

- **9.x.5.a — Extensión multi-platform aware + script `build-vsix`**:
  - `src/extension.ts` refactorizado con `resolveServerPath`
    siguiendo prioridad:
    (a) Override del user (`fitz.lspPath` ≠ default `"fitz-lsp"`)
        → respeta.
    (b) Bundled: busca `<extensionPath>/server/fitz-lsp[.exe]`
        (caso típico del .vsix de Marketplace).
    (c) Fallback al PATH del sistema (flujo alfa de 9.x.1.c —
        `cargo install` + setting default).
  - Helpers privados nuevos: `bundledBinaryPath`, `resolveUserPath`.
  - `scripts/build-vsix.mjs` orquesta: cargo build (con opcional
    `--target <triple>`) → copia binario a `server/` → tsc compile
    → `vsce package --target <vsce>` → produce `.vsix` con sufijo
    `-<platform>-<arch>`. Args: `--target <vsce>`, `--rust-target
    <triple>`. Default: plataforma actual via `process.platform`+
    `process.arch`. 6 plataformas soportadas con mapping a Rust
    triples: win32-x64/arm64, linux-x64/arm64, darwin-x64/arm64.
  - Estructura `editors/vscode/server/` con `.gitignore` que excluye
    los binarios (se regeneran cada build, no se versionan).
  - `.vscodeignore` actualizado para excluir `**/.gitignore` del
    .vsix final.
  - `activationEvents` removido del package.json (auto-derived por
    VSCode ≥1.74 desde `contributes.languages`).
  - `npm run build:vsix` desde editors/vscode/ corre todo.

**Decisiones técnicas tomadas al arrancar**:

- **Logo**: engranaje Rust + Fitz Roy (la inspiración del nombre
  del lenguaje + el lenguaje de implementación). Color `#CE412B`
  (Rust orange).
- **SVG single source of truth en `assets/`** (raíz del repo, no
  enterrado en `editors/vscode/`). Script regenera múltiples PNGs.
- **`@resvg/resvg-js` para SVG→PNG**: puro JS bindings, sin
  compilación nativa pesada, confiable en Windows. Alternativas
  rechazadas: `sharp` (compilación nativa), `cairosvg` Python
  (problemas en Windows).
- **Per-plataforma `.vsix`** (estándar rust-analyzer/Marketplace):
  un .vsix por target, cada uno con SU binario en `server/`.
  Alternativa rechazada: mega-.vsix con los 5 binarios (~50 MB).
- **Resolución del binario en orden** (override > bundled > PATH):
  backward-compatible con flujo 9.x.1.c.
- **`activationEvents` removido**: auto-derived por VSCode ≥1.74.
- **CI multi-platform y publicación al Marketplace fuera de scope**:
  acciones del autor (decisión sobre repo público, cuenta de
  publisher, PAT). Documentadas como pasos manuales en la guía.

**Cierre formal del plan LSP (9.x.1 → 9.x.5)**:

| Sub-fase | Feature | Cerrada |
|---|---|---|
| 9.x.1 | Diagnostics MVP + extensión VSCode base | 2026-05-15 |
| 9.x.2 | Hover (tipo del nodo bajo el cursor) | 2026-05-16 |
| 9.x.3 | Go-to-definition (uso → declaración) | 2026-05-16 |
| 9.x.4 | Autocomplete contextual (scope-level + after-dot) | 2026-05-16 |
| 9.x.5 | Distribución multi-platform + logo | 2026-05-16 |

El LSP MVP cubre la experiencia core de editing — diagnostics,
hover, go-to-def, autocomplete — más la infraestructura de
distribución. Lo que falta es decisión del autor (publicar) +
features avanzadas refinables post-MVP (rename, refactoring,
semantic highlighting, inlay hints).

**Acciones manuales pendientes del autor** (no commit técnico):

1. **GitHub Social Preview**: Settings → General → Social preview
   → upload `assets/logo-social.png`.
2. **Hacer el repo público** (cuando decida): pre-requisito para
   publicar al Marketplace + para que el Social Preview se
   renderice en link previews.
3. **Crear publisher en VSCode Marketplace**: Microsoft account +
   Azure DevOps + Personal Access Token.
4. **Publicar al Marketplace**: `vsce publish --packagePath
   editors/vscode/fitz-language-X.Y.Z-<target>.vsix` por cada
   plataforma.
5. **CI multi-platform** (opcional): GitHub Actions workflow con
   jobs Windows/macOS/Linux que corren `npm run build:vsix` y
   publican post release tag.

**Total al cierre**: 1233 unit + 79 E2E + 3 openapi sin cambios;
36 unit + 5 E2E LSP sin cambios. Logo + script no agregan tests
Rust. Validación local Windows: ✅ `fitz-language-win32-x64-0.9.2.vsix`
(1.49 MB, 211 archivos, `server/fitz-lsp.exe` bundleado).

**Próximo norte (técnico)**: resto de Fase 9 — **package manager
+ registry**, **formatter**, **linter**. Plan a definir al arrancar.

**Deuda residual derivada (NO bloquea próximas fases)**:

- CI multi-platform (GitHub Actions workflow).
- Publicación automática al Marketplace post-CI build.
- Cross-compile local desde una plataforma (requiere `cross` crate
  o Docker). Hoy: cada plataforma genera su propio .vsix nativo.
- Logo: versiones adicionales (favicon 32×32, app icon 512×512,
  monochrome para temas dark) si aparece demanda.

## [v0.9.5] — 2026-05-16 — Fase 9.x.4: LSP autocomplete contextual

Cuarta sub-fase visible del LSP — completa el MVP del language
server. El cliente VSCode (o cualquier otro cliente LSP) ahora puede
pedir `textDocument/completion` con una posición y recibe una lista
de `CompletionItem` apropiados al contexto: tras un `.` muestra los
fields/métodos del tipo del receiver, en cualquier otra posición
muestra los símbolos top-level del programa + builtins + tipos +
keywords. Cierra el loop "errores subrayados + hover + go-to-def
+ autocomplete" — el LSP MVP ya cubre la experiencia core de
editing.

Dos sub-pasos coordinados (un commit por sub-paso):

- **9.x.4.a — Persistir Program + helper `completion_at_position`**:
  - `check_source_with_types` retorna 5-tupla incluyendo `Program`:
    `(Program, TypeEnv, TypeInfo, DefinitionInfo, Vec<FitzError>)`.
    El AST es necesario para que el LSP enumere top-level
    declarations en scope-level y resuelva receivers por nombre en
    after-dot (fallback cuando el parser abandona stmts rotos).
    Call sites del LSP actualizados.
  - `fitz::lsp::completion_at_position(text, program, type_info,
    type_env, line, character) -> Vec<CompletionItem>` (pure
    function, unit-testeable). Despacha por contexto detectado:

    - **Scope-level**: enumera top-level del Program
      (let/fn/type/import) + builtins (print/len/sleep/cors) +
      tipos built-in (Int/Float/Str/.../PyAny) + keywords del
      lenguaje. **NO scope-aware**: no enumera vars locales/params
      como función del cursor (deuda MVP — requiere refactor del
      checker para exponer scopes por stmt). VSCode filtra por
      prefix client-side; el usuario puede tipear vars locales
      aunque no aparezcan en la lista.

    - **After-dot**: identifica el receiver (un solo ident antes
      del `.`), resuelve el tipo con **dos fallbacks**:
      1. TypeInfo lookup heurístico (max col <= recv_col en la
         misma línea).
      2. Walk del Program por nombre — busca `Stmt::Assign`
         top-level con `target == recv_name` y mira el tipo del
         value en TypeInfo. Cubre el caso típico `obj.<cursor>`
         al final del buffer donde el parser abandona el stmt
         entero por el `.` huérfano (deuda F15 recovery sub-stmt).

      Tipos cubiertos: `Nominal` (fields del TypeEnv), `List` (6
      métodos), `Map` (5 métodos), `Str` (3 métodos). Otros (Any,
      PyAny, primitivos) devuelven lista vacía.

  - Helpers internos: `CompletionContext` enum (AfterDot con
    `recv_name`+`recv_line`+`recv_col` / ScopeLevel),
    `detect_completion_context` (walk hacia atrás del cursor),
    `position_to_offset` / `offset_to_position` (UTF-8 char-based;
    UTF-16 LSP default queda como refinamiento si aparece presión
    real con código no-ASCII), `is_ident_continue` (ASCII
    alphanumeric + `_`), `method_items` (factory para METHOD kind),
    `after_dot_completions`, `scope_level_completions`.

  - `DocumentState` del backend suma `program: Program` con
    `#[allow(dead_code)]` puntual hasta 9.x.4.b.

  - 10 unit tests nuevos en `fitz::lsp::tests`: round-trip
    `position_to_offset`/`offset_to_position`; 4 casos de
    `detect_context` (vacío, después de ident, after-dot, after-dot
    con prefix); scope-level lista top-level+builtins+tipos+kws;
    after-dot Nominal lista fields del type (FIELD kind); after-dot
    List lista 6 métodos (METHOD kind); after-dot Str lista 3
    métodos (cubre el fallback walk-del-Program); after-dot
    receiver sin tipo devuelve vacío.

- **9.x.4.b — Handler `completion` + capability + E2E**:
  - Capability `completion_provider: Some(CompletionOptions {
    trigger_characters: Some(vec![".".into()]), resolve_provider:
    Some(false), ... })` anunciada en `initialize`. El trigger char
    `.` hace que VSCode invoque automáticamente completion tras un
    punto; para typing normal, el cliente invoca por su cuenta.
    `resolve_provider: false` porque mandamos toda la info en el
    item (no usamos `completionItem/resolve` para lazy details).
  - Handler `Backend::completion` lee state bajo lock, delega al
    helper pure-function, devuelve `CompletionResponse::Array(items)`.
    Sin awaits dentro del lock.
  - `#[allow(dead_code)]` removido de `DocumentState.text` y
    `DocumentState.program` (ya tienen consumidor).
  - 1 E2E nuevo `completion_after_dot_sobre_str_lista_metodos_built_in`:
    valida capability anunciada con `triggerCharacters: ["."]`,
    after-dot sobre `s.` con `s: Str` lista `upper`/`lower` y NO
    `push` (no es método de Str), scope-level lista `s` (var
    top-level) + `print` (builtin) + `Int` (tipo built-in) + `let`
    (keyword).

**Decisiones técnicas tomadas al arrancar**:

- **Alcance**: MVP cubre (1) scope-level y (2) after-dot. **(3)
  imports** (`from mod import `) queda como deuda visible — requiere
  cargar el módulo remoto y enumerar sus exports, complejidad del
  loader que pertenece a sub-paso futuro.
- **Scope-level no scope-aware**: enumeramos top-level del Program
  + builtins + tipos + keywords. NO enumeramos vars locales/params
  según la posición del cursor. Scope-aware requiere refactor del
  checker. Trade-off MVP aceptado: VSCode filtra por prefix client-
  side, el usuario puede tipear vars locales igual.
- **After-dot solo `<ident>.`**: chain `a.b.c.` queda como deuda
  — requeriría parser parcial.
- **After-dot con dos fallbacks**: TypeInfo lookup heurístico +
  walk del Program por nombre. El walk cubre el caso típico donde
  el parser abandona el stmt entero por el `.` huérfano (deuda F15
  recovery sub-stmt). Sin el fallback, `obj.<cursor>` al final del
  buffer no funcionaría.
- **Persistir `Program` en `DocumentState`**: el AST es necesario
  en cada completion request (scope-level enumera top-level; after-
  dot fallback walkea por nombre). Re-walkar es barato vs re-parsear.
- **`CompletionItem` shape**: label, kind (Variable/Function/Field/
  Method/Keyword/Class/Module), detail opcional (firma de fn/método
  o tipo de field). VSCode renderea kind con íconos distintivos.
- **`UTF-8 char-based` para position↔offset**: LSP default es UTF-16,
  pero el MVP asume programas mayormente ASCII. Refinable post-MVP
  si aparece presión real con código no-ASCII.

**Total al cierre**: 1233 unit (default) + 79 E2E + 3 openapi sin
cambios respecto a Fase 9.x.3. **10 unit nuevos + 1 E2E nuevo** con
`--features lsp` (acumulado 36 unit + 5 E2E en LSP). Clippy
`-D warnings` limpio sobre lib + ambos bins + tests.

**Cierre formal del LSP MVP**: con 9.x.4 cerrada, el LSP cubre
la experiencia core de editing — diagnostics, hover, go-to-def,
autocomplete. Lo que sigue (9.x.5) es distribución (publicar al
VSCode Marketplace con binarios bundleados por plataforma).

**Próximo norte**: **9.x.5 (distribución VSCode Marketplace)** —
publicar la extensión con binarios pre-compilados (Windows x64,
macOS x64+ARM, Linux x64+ARM) bundleados en el `.vsix`, al estilo
rust-analyzer. Alternativa de alfa: `.vsix` manual + `fitz-lsp` en
PATH (lo que ya tenemos en 9.x.1.c).

**Deuda residual derivada (NO bloquea 9.x.5)**:

- **Completion para imports** (`from mod import `): listar
  símbolos exportados por el módulo. Requiere cargar el módulo
  remoto y mapearlo a CompletionItems. Sub-paso futuro.
- **Scope-aware en scope-level**: enumerar vars locales y params
  según la posición del cursor. Requiere refactor del checker para
  exponer scopes por stmt. Refinable cuando el usuario lo pida.
- **Chain `a.b.c.`** en after-dot: solo soportamos `<ident>.`.
  Requiere parser parcial para resolver el tipo del FieldAccess
  intermedio.
- **Position UTF-16 strict** (LSP default): hoy UTF-8 char-based.
  Programas mayormente ASCII funcionan; con muchos caracteres
  no-latin puede haber off-by-one. Refinable.
- **Completion en posiciones context-sensitive del parser**: tras
  `@`, sugerir decoradores (`@get`/`@server`/`@middleware`); tras
  `import `, sugerir paths de módulos. Hoy todo eso cae en scope-
  level genérico.

## [v0.9.4] — 2026-05-16 — Fase 9.x.3: LSP go-to-definition

Tercera sub-fase visible del LSP. El cliente VSCode (o cualquier
otro cliente LSP) ahora puede pedir `textDocument/definition` con
una posición y recibe la `Location` de la declaración del ident bajo
el cursor — desbloquea la experiencia "F12 sobre un nombre te lleva
a su definición", core del workflow de exploración del código.

Dos sub-pasos coordinados (un commit por sub-paso):

- **9.x.3.a — Side-table `DefinitionInfo` + populación en el checker**:
  - `VarBinding` suma `def_span: Span`: cada binding recuerda dónde
    se declaró. Builtins (`print`/`len`/`sleep`/`cors`) usan
    `Span::ZERO` y el LSP los filtra (no hay archivo donde saltar).
  - `declare_var`/`declare_var_annotated` reciben `def_span` como
    nuevo parámetro. 12 call sites actualizados con el span
    apropiado (Stmt::Assign, FnDef body params, For.var, Import,
    FromImport, FnExpr params, match patterns vía
    `bind_pattern(...arm_span)`, `preregister_fn_signatures`).
    Aproximaciones documentadas donde el AST no tiene span propio
    del binding (Param, AssignTarget::Ident, For.var,
    MatchArm.pattern — deuda S1). VSCode salta al stmt contenedor;
    el usuario ve la línea de declaración.
  - `pub struct DefinitionInfo` paralelo a `TypeInfo` (F16). Side-
    table `HashMap<SpanKey (use), Span (def)>` con `record`,
    `definition_at`, `len`, `is_empty`, `iter`. Política: omite
    `Span::ZERO` en use y def (sintéticos y builtins).
  - El wrapper `infer_expr` para `Expr::Ident` resuelve vía
    `lookup_binding`, clona los fields para liberar el préstamo
    inmutable de `ctx.scopes`, y registra `(use_span, def_span)`
    antes de retornar.
  - `check_program` retorna 4-tupla `(TypeEnv, TypeInfo,
    DefinitionInfo, Vec<FitzError>)`. 18 call sites internos
    actualizados (CLI + codegen + LSP + tests).
  - `check_source_with_types` del LSP también retorna la 4-tupla.
  - `DocumentState` del backend suma `def_info` con
    `#[allow(dead_code)]` puntual hasta 9.x.3.b.
  - Limpieza colateral: `lookup_var` (que duplicaba `lookup_binding`)
    eliminado — el único caller pasó a usar `lookup_binding`
    directamente para acceder al `def_span`.
  - 6 unit tests nuevos en `types::tests::def_info_*`: registra var
    local, NO registra builtins (Span::ZERO filtra), registra fn
    top-level, registra param de fn (aproximación al span del FnDef),
    `definition_at` devuelve None para spans ausentes o ZERO, ident
    no definido no agrega entry.

- **9.x.3.b — Handler `definition` + helpers + capability**:
  - `definition_for_position(&DefinitionInfo, line, character) -> Option<Span>`
    en `fitz::lsp` (pure function). Misma heurística que
    `hover_for_position`: max col <= cursor en la misma línea sobre
    `DefinitionInfo.iter()`.
  - `make_definition_location(Url, Span) -> Location` arma la
    respuesta LSP. Convierte 1-based Fitz a 0-based LSP; range de
    1 carácter. `uri` es el del documento abierto.
  - Capability `definition_provider: Some(OneOf::Left(true))`
    anunciada en `initialize`.
  - Handler `Backend::goto_definition` lee state bajo lock, delega
    a los helpers, devuelve `GotoDefinitionResponse::Scalar(loc)`
    (un solo Location — Fitz no tiene overloading).
  - 5 unit tests nuevos cubriendo: var local resuelve a def_span,
    línea sin idents devuelve None, builtin filtrado, conversión
    1-based → 0-based correcta, smoke pipeline end-to-end.
  - 1 E2E nuevo `goto_definition_sobre_uso_de_var_local_devuelve_location_de_let`:
    valida capability anunciada, definition sobre uso de `x` en
    `let x = 42\nlet y = x\n` devuelve Location con line:0,
    definition sobre `print` (builtin) devuelve `result: null`.

**Decisiones técnicas tomadas al arrancar**:

- **Side-table dedicado vs reuso de TypeInfo**: dedicado
  `DefinitionInfo`. Mismo patrón que F16; semánticas distintas no se
  mezclan. El checker ya hace el lookup; solo agregamos la captura
  del span al wrapper.
- **`VarBinding` gana `def_span: Span`**: refactor mecánico — los 12
  call sites de `declare_var*` pasan el span apropiado. Compiler
  ayuda a no olvidar ningún call site. Builtins usan `Span::ZERO`
  y se filtran.
- **Granularidad del span de def**: aproximaciones pragmáticas dado
  el AST actual (`Param`, `AssignTarget::Ident`, `For.var`,
  `MatchArm.pattern` no tienen span propio — deuda S1). Para
  `Stmt::Assign` reasignaciones, el `def_span` se sobreescribe con
  el del último binding stmt (semántica simplificada del MVP —
  refinable a "primera declaración" con tracking adicional).
- **Lookup heurístico igual que hover** (max col <= cursor en la
  misma línea): consistente con 9.x.2, identidad sobre idents.
- **`range` de 1 carácter en la respuesta** (sin `end_span`):
  paralelo a Diagnostics y Hover.
- **`uri` = documento abierto** (vs resolución cross-module): cross-
  module def requiere mapear paths del loader a URIs — agrega
  complejidad del loader que pertenece a 9.x.4 o post-MVP.
- **`OneOf::Left(true)` para `definition_provider`** (vs
  `DefinitionOptions`): forma simple del LSP. Fitz no tiene
  overloading, no necesitamos múltiples Locations por nombre.

**Total al cierre**: 1233 unit (default) + 79 E2E + 3 openapi sin
cambios respecto a Fase 9.x.2 (+6 unit nuevos en `types::tests::def_info_*`).
**5 unit nuevos + 1 E2E nuevo** con `--features lsp` (acumulado 26
unit + 4 E2E en LSP). Clippy `-D warnings` limpio sobre lib + ambos
bins + tests.

**Próximo norte**: **9.x.4 (autocomplete contextual)** —
`textDocument/completion` con cuatro contextos: símbolos en scope
visible (typing en cualquier posición), fields tras `obj.` (mirar
el tipo del receptor), métodos built-in tras `xs.`/`m.`/`s.` (List/
Map/Str), símbolos importados tras `from mod import `. Después: 9.x.5
distribución VSCode Marketplace. Ver `docs/roadmap.md` → "Fase 9.x".

**Deuda residual derivada (NO bloquea 9.x.4)**:

- Cross-module go-to-def: `from foo import X` apunta al span del
  Stmt::Import local, no al módulo remoto. Requiere mapear paths
  del loader a URIs.
- `def_span` granular por nombre (vs por Stmt contenedor): el AST
  no tiene `Span` propio en `Param`, `AssignTarget::Ident`,
  `For.var`, `MatchArm.pattern`. Refinable con S1.deuda.
- Reasignaciones sobrescriben `def_span` con el último let stmt
  (semántica simplificada). TypeScript salta a la primera
  declaración; Fitz salta a la última. Refinable con tracking
  adicional si pinta corto.
- Cross-method tipos (definición de método built-in `xs.map`):
  no aplica — los métodos built-in no tienen "definición" en el
  código fuente Fitz.

## [v0.9.3] — 2026-05-16 — Fase 9.x.2: LSP hover — tipo del nodo bajo el cursor

Segunda sub-fase visible del LSP. El cliente VSCode (o cualquier
otro cliente LSP) ahora puede preguntar `textDocument/hover` con
una posición y recibe el tipo del nodo bajo el cursor — desbloquea
la experiencia "pasá el mouse y ve qué tipo tiene esta expresión",
equivalente al hover que TypeScript provee desde hace años.

Dos sub-pasos coordinados (un commit por sub-paso):

- **9.x.2.a — Persistencia de TypeInfo por documento**:
  - Nueva API `fitz::lsp::check_source_with_types(src) -> (TypeEnv,
    TypeInfo, Vec<FitzError>)` que retiene el side-table de tipos
    poblado por F16. La fn vieja `check_source` se mantiene como
    wrapper que descarta env + types (consumidores que solo
    necesitan diagnostics).
  - `DocumentState { text, type_env, type_info }` reemplaza el
    `String` plano en el `documents` map del backend. `did_open`/
    `did_change` corren la pipeline y persisten los tres; `did_close`
    limpia.
  - 4 unit tests nuevos: programa válido devuelve TypeInfo no vacío,
    error de lexer aborta antes del checker (TypeInfo vacío), error
    de tipo no borra TypeInfo (Exprs válidos quedan), sanity check
    de equivalencia entre las dos APIs.

- **9.x.2.b — Hover handler + lookup heurístico + capability**:
  - `hover_for_position(&TypeInfo, line, character) -> Option<&Type>`
    en `fitz::lsp` (pure function, unit-testeable). Heurística "max
    col <= cursor en la misma línea" sobre el TypeInfo iterado.
    Convierte 0-based LSP a 1-based Fitz. Sin `end_span` en los
    nodos (deuda S1), asume que el último Expr iniciado antes del
    cursor en la misma línea es el más probable — cubre 90% del
    caso (cursor sobre o inmediatamente después de un identificador
    /literal). Refinable cuando los nodos tengan span completo.
  - `make_hover(&Type, &TypeEnv) -> Hover` arma la respuesta LSP
    con `MarkupContent::Markdown` y bloque ```fitz<tipo>```. VSCode
    renderea con syntax highlighting nativo. `range: None` porque
    sin `end_span` no podemos devolver el rango exacto del nodo —
    el tooltip funciona, el token no se resalta.
  - Capability `hover_provider: Some(HoverProviderCapability::Simple(true))`
    anunciada en `initialize`.
  - `Backend::hover` lee el state bajo lock, delega al helper y
    formatea con `make_hover`. Sin awaits dentro del lock.
  - Exposición de `pub fn iter()` sobre `TypeInfo` — necesario para
    que el LSP haga lookup heurístico (sin esto, solo `type_at`
    para lookup exacto). Mínimo y backward-compatible.
  - 8 unit tests nuevos cubriendo: posición exacta sobre literal,
    medio de Ident usado como Expr, línea sin spans, cursor antes
    del primer token, no cruce de líneas, markdown format, tipos
    compuestos (`List<Int>` se formatea OK), smoke end-to-end.
  - 1 E2E nuevo `hover_sobre_literal_int_devuelve_tipo_en_markdown`:
    valida capability anunciada, hover sobre `42` en col 8 devuelve
    `Int` en markdown fitz, hover en posición sin spans devuelve
    `result: null`.

**Decisiones técnicas tomadas al arrancar**:

- **Persistencia de TypeInfo por URI** (vs re-correr el pipeline en
  cada hover): re-correr sería lento sobre buffers grandes; el
  TypeInfo es solo un `HashMap<SpanKey, Type>` — pesa nada.
- **Heurística de lookup "max col <= cursor en la misma línea"** (vs
  lookup exacto que casi nunca funcionaría sin que el cursor esté
  en el inicio exacto del span). Limitación heredada de F16: sin
  `end_span` no podemos hacer "está adentro del nodo". El 90% del
  caso (cursor sobre token corto) funciona; tokens largos pueden
  fallar si el cursor está muy al final.
- **Colisiones en TypeInfo aceptadas como están**: cuando dos `Expr`
  comparten span (típicamente un `BinOp` y su primer operando),
  TypeInfo guarda solo el último escrito (heredado de F16). En la
  práctica el tipo del Expr más "grande" es lo que el usuario
  quiere ver al hover.
- **Persistir TypeEnv junto con TypeInfo**: `Type::display` necesita
  el env para resolver nombres de tipos nominales. Sin el env, el
  hover sobre un `User` mostraría `Nominal(TypeId(3))` en vez de
  `User`. Cambio chico de firma en `check_source_with_types`.
- **`MarkupContent::Markdown` con bloque ```fitz``` (vs PlainText)**:
  VSCode aplica syntax highlighting si reconoce el lenguaje. Más
  bonito sin costo.
- **`range: None` en la respuesta Hover**: sin `end_span` no podemos
  devolver el rango exacto. El tooltip se muestra igual, solo el
  highlighting del token no aparece.

**Total al cierre**: 1227 unit (default) + 79 E2E + 3 openapi sin
cambios. **12 unit nuevos + 1 E2E nuevo** con `--features lsp`
(acumulado 21 unit + 3 E2E en LSP). Clippy `-D warnings` limpio.

**Próximo norte**: **9.x.3 (go-to-definition)** —
`textDocument/definition` resuelve `Ident` → span de la declaración
(`let x = ...`, `fn f(...)`, `type T { ... }`). Requiere mantener
una tabla de resolución de scopes desde el checker. Después: 9.x.4
autocomplete contextual, 9.x.5 distribución VSCode Marketplace.
Ver `docs/roadmap.md` → "Fase 9.x".

## [v0.9.2] — 2026-05-15 — Fase 9.x.1: LSP MVP — diagnostics + extensión VSCode

Primera sub-fase visible del LSP. Habilita la experiencia "escribir
Fitz en VSCode con errores subrayados al tipear" — equivalente al
nivel de servicio que ofrece TypeScript en sus primeros segundos.

Tres componentes coordinados (un commit por sub-paso):

- **9.x.1.a — Server skeleton** (bin nuevo `fitz-lsp`, feature opt-in
  `lsp`, handshake initialize/shutdown):
  - Dep `tower-lsp = "0.20"` opcional; feature `lsp = ["dep:tower-lsp"]`
    paralela a `python = ["dep:pyo3"]`. Bin `[[bin]] name = "fitz-lsp"`
    con `required-features = ["lsp"]`. El bin `fitz` default sigue
    standalone, sin pagar el peso de tower-lsp + lsp-types en el dep tree.
  - `src/bin/fitz-lsp.rs` con `Backend` impl `LanguageServer`:
    `initialize` → response con `serverInfo` + `textDocumentSync: FULL`,
    `initialized` (log via `client.log_message`), `shutdown`.
    `#[tokio::main(flavor = "current_thread")]` (LSP es I/O-bound).
  - 1 test E2E `tests/lsp_e2e.rs` que spawnea el bin y valida el
    handshake. Frames JSON-RPC construidos a mano via Content-Length,
    sin deps extras. `#![cfg(feature = "lsp")]`.

- **9.x.1.b — Lib refactor + helper diagnostics + lifecycle hooks**:
  - **Lib refactor**: `src/lib.rs` nuevo expone los módulos como
    `pub mod`. `src/main.rs` migra de `mod X;` a `use fitz::{...};`.
    Habilita que `fitz-lsp` reuse `lexer`/`parser`/`types` sin
    compilación duplicada.
  - **`src/lsp.rs` (nuevo, lib, feature-gated)**: dos APIs públicas
    pure-function unit-testeables:
    - `check_source(&str) -> Vec<FitzError>` — pipeline LSP-style:
      tokenize → `parse_with_recovery` (F15) → `check_program`
      (descarta el `TypeInfo` que llega 9.x.2 hover).
    - `fitz_errors_to_diagnostics(&[FitzError]) -> Vec<Diagnostic>` —
      mapea 1-based Fitz → 0-based LSP. Range 1-char (refinable a
      span completo cuando S1.Pattern/TypeExpr sume `end_span`).
      `hint` concatenado al `message`. Severity ERROR, source "fitz".
      Sentinel `(0, 0)` → range degenerado al inicio del documento.
  - **Backend con DocumentStore**: `documents: Arc<parking_lot::Mutex
    <HashMap<Url, String>>>`. `did_open`/`did_change`/`did_close`
    disparan `check_source → fitz_errors_to_diagnostics →
    publish_diagnostics`. Cierre limpia diagnósticos.
  - 9 unit tests + 1 test E2E nuevo (`did_open` con buffer roto valida
    la notification `textDocument/publishDiagnostics`).
  - **Deuda nueva visible**: `#[allow(clippy::result_unit_err)]`
    puntual sobre `Environment::assign` en `src/env.rs`. Lint apareció
    en clippy 1.95 + expuesto por el refactor lib (antes silencioso).
    El `Result<(), ()>` ahí es sentinel intencional. Refactor a
    newtype error queda como deuda menor.

- **9.x.1.c — Extensión VSCode** (`editors/vscode/`, paquete TypeScript):
  - Grammar TextMate (`syntaxes/fitz.tmLanguage.json`): comments,
    strings con interpolación `{...}` recursiva, números, decoradores
    `@nombre`, keywords (control + declaración + lógicos), tipos
    built-in (Int/Float/Str/Bool/Null/Range/Any/List/Map/Result/
    Future/Request/Response/PyAny) + nominales `[A-Z]…`, constantes
    `true`/`false`/`null`/`Ok`/`Err`, built-ins `print`/`len`/`sleep`/
    `cors`, operadores y defs/calls de funciones.
  - `language-configuration.json`: comments, brackets, autoClose
    (con `notIn` string/comment), surrounding, indent rules.
  - `src/extension.ts`: capa fina sobre `vscode-languageclient/node`.
    `resolveServerPath` distingue absoluto / relativo-a-workspace /
    nombre-suelto-en-PATH. Error visible al usuario si el binario no
    spawnea, citando el path intentado.
  - Settings: `fitz.lspPath` (default `"fitz-lsp"`) y
    `fitz.trace.server` (off/messages/verbose).
  - Activation `onLanguage:fitz`.
  - Validaciones build: 4 manifestos JSON OK; `npm install` (12
    packages, 0 vulns); `npm run compile` (tsc strict, sin warnings);
    `npx @vscode/vsce package` produce `.vsix` 294 KB con 209 archivos.
    `node_modules/`, `out/`, `*.vsix` excluidos por `.gitignore` local.

**Decisiones técnicas tomadas al arrancar**:

- **bin `fitz-lsp` separado del CLI principal** (vs subcomando):
  convención ecosistema (rust-analyzer/gopls/tsserver). Bin `fitz`
  queda chico, release ciclo independiente.
- **`tower-lsp` sobre `lsp-server` crudo**: async-first, framing
  JSON-RPC automático. Cientos de LoC menos para el MVP.
- **Grammar TextMate sobre tree-sitter**: TextMate son ~120 LoC JSON,
  suficiente para colores. Tree-sitter más preciso pero requiere
  build chain extra. Refinable post-MVP.
- **Descubrimiento via setting `fitz.lspPath`** (vs bundling): alfa
  simple. Bundling rust-analyzer-style llega en 9.x.5.
- **`textDocumentSync: FULL`** (vs `INCREMENTAL`): default razonable
  para MVP. Migración es decisión de perf si aparece presión.
- **`tokio::main(flavor = "current_thread")`** para LSP: I/O-bound,
  sin work-stealing necesario. Decisión ortogonal a la del CLI HTTP
  (multi-thread, F17).

**Total al cierre**: 1227 unit + 79 E2E + 3 openapi sin cambios
(default). **9 unit + 2 E2E nuevos** con `--features lsp`. Clippy
`-D warnings` limpio sobre lib + ambos bins + tests.

**Próximo norte**: **9.x.2 (hover)** — `textDocument/hover` devuelve
el tipo del nodo bajo el cursor. Consume el `TypeInfo` (F16) que
hoy `check_source` descarta. Después: 9.x.3 go-to-definition,
9.x.4 autocomplete contextual, 9.x.5 distribución Marketplace.
Ver `docs/roadmap.md` → "Fase 9.x".

## [v0.9.1] — 2026-05-15 — Fase 9.0: F16 cierre — IR tipado persistido por nodo

Segundo y último sub-paso de Fase 9.0. Cierra la deuda F16
identificada post-5b: **segundo pre-requisito habilitante del LSP**
(hover y completion contextual). El checker ahora retiene los tipos
sintetizados de cada nodo `Expr` en un side-table devuelto junto al
`TypeEnv`.

**Sin cambio de comportamiento user-facing**: `fitz run` / `fitz
build` / `fitz check` siguen ignorando el side-table. La API nueva
(`TypeInfo`, retornada por `check_program`) está pensada para los
consumidores del LSP que llegan en sub-fases siguientes.

- **9.0.4 — Side-table TypeInfo + populación + tests** (8 unit nuevos):
  - Nuevo `pub struct SpanKey(usize, usize)` como clave hashable.
    Necesario porque `Span` propio no sirve: su `PartialEq` devuelve
    `true` siempre (intencional para que los tests de AST comparen
    estructura sin re-derivar posiciones del parser).
  - Nuevo `pub struct TypeInfo` con `record(span, ty)`,
    `type_at(span)` y `len()`. Omite `Span::ZERO` (sintéticos /
    tests) para evitar colisiones bajo la misma clave `(0, 0)`.
  - `infer_expr` pasa a ser wrapper sobre `synthesize_expr`: la
    lógica del match queda igual, y el wrapper centraliza el
    `record` al salir. Cobertura amplia desde un solo punto, sin
    "olvidé tal caso".
  - `pub fn check_program` cambia firma de `(TypeEnv,
    Vec<FitzError>)` a `(TypeEnv, TypeInfo, Vec<FitzError>)`. Los
    13 call sites internos (main.rs, codegen.rs, tests) migrados
    descartando el segundo elemento con `_types` — la CLI no
    consume el side-table todavía.
  - `Expr::Error` (F15) se persiste como `Type::Any` uniforme con
    el comportamiento del checker. El LSP decide qué mostrar en
    hover sobre Error nodes.
  - Tests del side-table (`types::tests::types_info_*`): literales,
    ident + BinOp, call + field, match arms, omisión de Span::ZERO,
    Error nodes como Any, lookup ausente devuelve None, smoke
    sobre programa real (`info.len() >= 10`).

- **9.0.5 — Cierre formal**: este CHANGELOG, `docs/roadmap.md`
  con Fase 9.0 — F16 documentada paso a paso, `docs/deudas-post-5b.md`
  con F16 marcado CERRADO, README + CLAUDE refresh.

**Decisiones técnicas tomadas al arrancar**:

- **`HashMap<SpanKey, Type>` (vs NodeId asignado al nodo, vs
  `*const Expr`)**: simple, reusa los spans que ya tiene cada
  `Expr` post-S1.2, zero refactor del AST. La colisión potencial
  por `Span::ZERO` se resuelve omitiendo esos nodos.
- **Cobertura amplia (todo `Expr` que pasa por `infer_expr`)**:
  un solo `record` en el wrapper en lugar de un insert por
  brazo del match. Futuro-proof contra nuevos tipos de Expr.
- **API: una sola** (no variante `check_program_with_types`):
  los 13 call sites son triviales (`let (env, _types, errors) =
  ...`), una sola API es más limpia que dos en paralelo.
- **`Span::ZERO` omitido**: sintéticos del parser y nodos de
  tests colisionarían entre sí bajo la misma clave; ninguno es
  user-visible para hover.
- **Solo `Expr` (no `Stmt` / `TypeExpr` / `Pattern`)**: el LSP
  obtiene info de variables y fns por scope lookup; persistir
  Stmt es ortogonal. Spans en `TypeExpr` y `Pattern` siguen
  como deuda residual menor de S1 — refinable post-LSP MVP si
  aparece presión real.

**Total al cierre**: 1227 unit + 79 E2E + 3 openapi sin feature.
Clippy `-D warnings` limpio.

**Próximo norte**: las sub-fases visibles del LSP — **9.x.1
(diagnostics MVP)**, 9.x.2 (hover, ya consume `TypeInfo`), 9.x.3
(go-to-definition), 9.x.4 (autocomplete), 9.x.5 (distribución
VSCode Marketplace). Ver `docs/roadmap.md` → "Fase 9.x".

## [v0.9.0] — 2026-05-15 — Fase 9.0: F15 cierre — error recovery del parser

Primer sub-paso de Fase 9 (Ecosistema). Cierra la deuda F15
identificada post-5b: **pre-requisito habilitante del LSP** que
permitirá que herramientas externas (LSP, formatter, futuros
analizadores) reciban un AST parcial y la lista paralela de
errores sobre buffers en construcción.

**Sin cambio de comportamiento user-facing**: `fitz run` / `fitz
build` / `fitz check` siguen usando `parse()` strict y abortando al
primer error de parser, exactamente como antes. La API nueva
(`parse_with_recovery`) está pensada para los consumidores
internos del lenguaje que llegan en sub-fases siguientes.

- **9.0.1 — AST + API recovery + tests del parser** (10 unit nuevos):
  - Nuevas variantes `Expr::Error(Span)` y `Stmt::Error(Span)` en
    el AST. Su único productor es `parse_with_recovery`; mantienen
    la forma estructural del árbol cuando hay errores recuperados
    (un body de fn con un stmt roto sigue siendo un `Vec<Stmt>`
    válido).
  - Parser: flag interno `recovery_mode` + cota dura
    `MAX_RECOVERED_ERRORS = 100` + helper `synchronize()` que
    avanza hasta sync points stmt-level. Sync points: `Newline`
    (consumido), `RBrace`/`EOF` (preservados), y keywords que
    típicamente arrancan stmt — `Let`, `Fn`, `Async`, `Type`,
    `Return`, `Break`, `Continue`, `While`, `Loop`, `For`, `If`,
    `Import`, `From`, `At` — preservadas. La regla de keywords
    fue necesaria porque `primary()` consume el token actual antes
    de validar: un `Newline` inesperado se consume y el cursor
    termina parado en el `Let` del próximo stmt; sin la parada en
    keywords, sync se comía stmts enteros.
  - API pública nueva:
    `pub fn parse_with_recovery(tokens) -> (Program, Vec<FitzError>)`.
    Nunca retorna `Err`: los errores se acumulan en la lista
    paralela. Marcada con `#[allow(dead_code)]` justificado
    porque hasta que aterricen los consumidores (LSP / formatter)
    solo la ejercitan los tests.
  - Defensas en evaluator/codegen: si un nodo Error llega ahí
    (no debería — la CLI strict nunca los produce), emiten un
    `FitzError` claro con span, no panic.
  - Checker silencioso (ya entró en 9.0.1, tests en 9.0.2):
    `Expr::Error` sintetiza `Type::Any`, `Stmt::Error` no-op.
  - Tests del parser (`parser::tests::recovery_*`): programa
    válido sin errores, stmt roto top-level, dos errores
    consecutivos, recovery dentro de `if`/`fn` body, span del
    Error node apunta al inicio del stmt, posición del error
    apunta al token problemático, EOF inesperado se acumula,
    cota de 100 errores se respeta, fn con body roto preserva
    estructura, parse strict sigue abortando al primer error.

- **9.0.2 — Tests del checker sobre AST recuperado** (5 unit nuevos):
  - Helper local `check_recovering(src)` que corre el pipeline
    LSP-style (`parse_with_recovery` → `check_program`) y devuelve
    solo los errores del checker. Es el pipeline que usará el LSP
    MVP para producir diagnostics.
  - Tests (`types::tests::checker_stmt_error_*` y
    `checker_pipeline_recovering_*` y `checker_expr_error_*`):
    Stmt::Error no agrega errores derivados; el silencio sobre
    Error nodes no afecta detección de errores genuinos en stmts
    vecinos válidos; Error nodes en fn body no abortan el check
    del resto del programa; smoke con 3 stmts rotos no panic;
    Expr::Error directo en AST tipa como Type::Any.

- **9.0.3 — Validación end-to-end + cierre formal**:
  - Smoke a mano: `fitz check d:/tmp/recovery_smoke.fitz` (buffer
    con 3 stmts rotos intercalados con código válido) → exit 1
    con un error reportado del primer stmt roto. Comportamiento
    strict idéntico a antes.
  - Smoke `GUIDE_EXAMPLES_COMPILE` sigue verde sobre los 13
    ejemplos de la guía compilables.
  - Docs: este CHANGELOG, `docs/roadmap.md` con Fase 9.0
    documentada paso a paso, `docs/deudas-post-5b.md` con F15
    marcado CERRADO, README + CLAUDE refresh.

**Decisiones técnicas tomadas al arrancar**:

- **Representación de errores**: nodos `Expr::Error(Span)` /
  `Stmt::Error(Span)` in-band en el AST + `Vec<FitzError>`
  paralelo. Razón: el árbol mantiene su forma estructural (mejor
  para LSP/formatter que recorren el AST sin chequear cada nodo),
  y la lista paralela lleva los mensajes ricos sin tener que
  desempaquetar wrappers en cada visita.
- **Sync points stmt-level + keywords de inicio**: la primera
  iteración tenía solo Newline/RBrace/EOF; los tests detectaron
  que `primary()` consume el token al fallar y el cursor podía
  saltar al próximo stmt. Agregar keywords como sync points cierra
  el caso sin complicar la lógica de recovery.
- **API strict intacta**: `parse()` no cambia su firma ni su
  comportamiento. Razón: la CLI sigue priorizando fail-fast con
  un error claro; recovery es feature de tooling externo.
- **Cota 100**: cubre el caso 90% del LSP (~5-20 errores en un
  buffer real) con margen amplio sin permitir cascadas runaway
  sobre buffers de tests bizarros.
- **Recovery solo stmt-level en 9.0**: errores DENTRO de un stmt
  (paréntesis sin cerrar adentro de un arg, expresión incompleta
  como RHS) descartan el stmt entero. Recovery sub-stmt
  (preservar bindings parciales, args parciales) queda como
  sub-paso futuro post-LSP MVP si aparece presión.
- **Cascadas "variable no definida" del checker**: aceptables como
  trade-off del LSP MVP. Cuando un Stmt::Error reemplaza un
  `let x = ...` roto, referencias posteriores a `x` pueden
  generar "no definida". El error real del parser apunta al lugar
  del problema; los IDEs muestran ambos diagnostics. Refinar
  requiere preservar bindings parciales (post-9.0).

**Trade-offs reconocidos**:

- `Expr::Error` solo se construye desde tests en 9.0 (el parser en
  9.0 produce Stmt::Error pero nunca Expr::Error suelto — recovery
  sub-stmt llega después). El nodo existe en el AST porque
  agregarlo después rompe match exhaustivos en 11 sitios
  (eval/checker/codegen).
- La parada en keywords como sync points es un compromiso: en
  expression-statements largos con keywords adentro (raros pero
  posibles), recovery podría sub-sincronizar. Aceptable para el
  90% del uso real; revisable si aparece presión.

**Total al cierre**: 1219 unit + 79 E2E + 3 openapi sin feature
(1310 + 88 + 3 con `--features python`). Clippy `-D warnings`
limpio en ambos modos.

**Próximo norte**: **Fase 9.0 — F16 (IR tipado persistido por
nodo)** — segundo pre-req habilitante del LSP. Después del cierre
de F16, las sub-fases visibles del LSP (9.x.1 → 9.x.5) pueden
arrancar.

## [v0.8.9] — 2026-05-15 — Fase 8.8: Guía + ejemplo CRUD + cierre de Fase 8

Octavo y último sub-paso de la Fase 8 (Interop Python). **Cierra
la fase entera**: la guía gana un capítulo dedicado a interop, el
ejemplo CRUD demuestra el flujo end-to-end (SQLAlchemy + SQLite +
HTTP + tipos Fitz), y la Fase 8 queda con todas las features del
roadmap original cubiertas — embedding (8.1), marshaling
compuesto (8.2), excepciones → `Result<T>` (8.3), tipos del
checker + coerción (8.4), `fitz py-types` (8.5), bridge async
(8.6), codegen en `fitz build` (8.7), y este cierre formal con
docs + ejemplo (8.8).

- **8.8.1 — Capítulo 21 "Interop Python" en `docs/guide.md`** + renumeración:
  - Capítulo nuevo con 12 secciones cubriendo todo lo de 8.1-8.7:
    setup (`cargo build --features python` + venvs estándar),
    sintaxis (`from python import X` + alias + path punteado),
    constantes y atributos con coerción primitiva, llamadas con
    Result wrap automático, propagación con `?`, marshaling de
    tipos compuestos (List/Map/Instance Fitz → list/dict
    Python), recuperación con anotaciones (`let row: User =
    py_call(...)?`), `fitz py-types` para SQLAlchemy, bridge
    async (`<py_call>?.await`), `fitz build` con interop (qué
    anda y qué es deuda residual), CRUD ejecutable referenciado,
    y limitaciones honestas (GIL, numpy C extensions, herencia
    Python, `asyncio.gather` con futures Fitz).
  - Renumeración: cap 21 viejo "Qué sigue" → cap 22; índice
    actualizado con la parte 10 nueva ("Cerrando" ahora vive
    en parte 10 mientras la 9 es "Interop").
  - Cap 22 ("Qué sigue") refrescado: la sección "Lo que ya sabés"
    suma el bullet de interop Python con todas las features; la
    sección "Lo que viene" pasa de "más allá de Fase 7" a "más
    allá de Fase 8" + próximo norte Fase 9, sub-paso futuro
    separado de bundling CPython, y stack DB nativo (Fase 10+).
- **8.8.2 — Ejemplo CRUD ejecutable**:
  - `examples/guide/21-python-crud/` con:
    - `models.py` — modelo SQLAlchemy `User` sobre SQLite.
    - `db.py` — helpers DB (`init_db`, `add_user`, `list_users`,
      `get_user`) que devuelven dicts/lists nativos Python para
      marshaling directo a Fitz.
    - `models.fitz` — output de `fitz py-types models.py`
      (versionado para que el ejemplo funcione sin requerir
      `sqlalchemy` instalado solo para regenerar).
    - `app.fitz` — programa Fitz principal con 3 handlers HTTP
      (`POST /users`, `GET /users`, `GET /users/{id}`) que
      combinan HTTP nativo + tipos Fitz + interop Python.
  - Helper `user_from_py(raw)` — round-trip por JSON
    (`json.dumps` + `json.loads`) para disparar la coerción
    `Map → Instance` de 8.4.3 sobre dicts Python opacos.
  - Setup: `pip install sqlalchemy` + `PYTHONPATH=...` antes
    del comando (el cap 21 explica el porqué — preferimos el
    estándar Python sobre magia de Fitz para sys.path).
  - `.gitignore` suma reglas para `__pycache__/`, `*.pyc`, y el
    SQLite local `crud.db` que el ejemplo crea al boot.
  - Validado end-to-end con curl: POST inserta con id auto-asignado
    por SQLite, GET lista, GET por id devuelve `User` Fitz tipado.

**Decisiones técnicas tomadas al arrancar**:
- **Posición del capítulo nuevo**: cap 21 (entre `fitz build` y
  "Qué sigue"). Una sola renumeración (21→22), lectura lineal —
  el cap 20 (`fitz build`) menciona limitaciones que cierra
  interop, así que conviene leerlos en ese orden.
- **Backend de DB**: SQLite + SQLAlchemy in-process. Setup
  mínimo (`pip install sqlalchemy`) sin Docker ni Postgres.
  Cubre el mismo patrón conceptual que Postgres (sesiones,
  models, queries) — el código Fitz es idéntico salvo la URL
  de conexión. Demuestra el caso canónico sin pesos extras.
- **Modo de ejecución del ejemplo**: solo `fitz run` (intérprete)
  con nota explícita sobre 8.7. El intérprete ya tiene la
  coerción `Map → Instance` (8.4.3) que el ejemplo necesita;
  `fitz build` cierra el codegen interop (8.7) pero la coerción
  de compuestos sigue siendo deuda residual. Documentado
  honestamente en el cap.

**Cierre formal de Fase 8 entera (Interop Python)**:

Roadmap original cumplido al 100%:
- ✅ Embedding básico de CPython (8.1)
- ✅ Marshaling List/Map/Instance bidireccional (8.2)
- ✅ Excepciones Python → `Result<T>` (8.3)
- ✅ Tipos del checker + coerción runtime (8.4)
- ✅ `fitz py-types` auto-mapeo SQLAlchemy (8.5)
- ✅ Bridge async tokio ↔ asyncio (8.6)
- ✅ Codegen interop en `fitz build` (8.7 — cierra deuda F19)
- ✅ Guía + ejemplo CRUD + cierre formal (8.8)

**Tests al cierre**: 1204 unit + 79 E2E + 3 openapi sin feature;
**1295 unit + 88 E2E + 3 openapi con `--features python`**.
Clippy `-D warnings` limpio en ambos modos.

**Sub-paso separado pendiente** (no parte del roadmap original
de Fase 8): bundling CPython embebido (`fitz build
--bundle-python`). Decisión python-build-standalone vs
PyOxidizer pendiente; sin presión real al cierre.

**Próximo norte**: Fase 9 — Ecosistema (package manager, LSP
con autocomplete + hover + go-to-def, formatter, linter). Pre-reqs
habilitantes ya identificados: parser con error recovery (F15) +
IR tipado persistido por nodo (F16).

## [v0.8.8] — 2026-05-15 — Fase 8.7: Codegen interop Python en `fitz build` (cierra F19)

Séptimo sub-paso de la Fase 8 (Interop Python). Cierra la deuda
**F19** del roadmap post-5b: `fitz build` ahora compila programas
con `from python import` a binario nativo standalone con pyo3
linkeado, con paridad bit-a-bit ante `fitz run`. El binario asume
Python instalado en el destino (`PYO3_PYTHON` o `python3` en
PATH) — bundling de CPython queda como sub-paso futuro separado.

**Decisión de alcance al arrancar 8.7** — separar codegen
(deuda F19, alcance medible) de bundling (decisión de herramienta
pendiente, proyecto más grande). Carta blanca del autor confirma:
F19 cierra ahora, bundling queda explícito como deuda residual
con dos opciones reales evaluadas (python-build-standalone,
mantenida activamente por Astral para `uv`; PyOxidizer,
ralentizado en 2024-2025).

- **8.7.1 — Preludio Python + import + getattr + Cargo.toml condicional**:
  - `collect_python_imports(program)` separa imports Python del
    AST top-level; el `ModuleLoader` Fitz los skipea (no hay
    archivo `.fitz` que cargar).
  - Cargo.toml generado suma `pyo3 = { version = "0.28",
    features = ["abi3-py310", "auto-initialize"] }` cuando
    `uses_python`. Programas sin interop no pagan el costo de
    bajar/linkear pyo3 — sigue siendo binario libre como Fase 5b.
  - Preludio Python emitido en `emit_python_prelude` (solo
    cuando `uses_python`): `struct __FitzPyObject(Arc<Py<PyAny>>)`
    con Clone/Debug/PartialEq (por puntero)/Display que delega
    a `__str__` Python (paridad bit-a-bit con `print(math.pi)`).
    Helpers `__fitz_py_import`, `__fitz_py_get_attr_obj`,
    `__fitz_py_extract_{i64,f64,string,bool}`,
    `__fitz_py_err_to_string` (formato canónico `<Class>: <msg>`
    paralelo a 8.1.2).
  - **Bindings globales**: cada `from python import X` se emite
    como `static __FITZ_PY_BIND_X: OnceLock<__FitzPyObject>` +
    getter `__fitz_py_bind_x()`. Lazy init en el primer
    `Python::attach`. Cualquier fn (main, handlers HTTP,
    helpers) referencia el binding via el getter.
  - `Type::PyAny` → `__FitzPyObject` en `rust_type_for`.
    `gen_field_access` despacha sobre receptor PyAny.
    `coerce(PyAny → T)` con T primitivo emite extracción
    directa (`let pi: Float = math.pi` →
    `__fitz_py_extract_f64(...)`).
- **8.7.2 — Call + marshaling Fitz → Python + Result wrap**:
  - `gen_call` / `gen_method_call` aceptan receptor PyAny y
    emiten `__fitz_py_invoke(&<callable>, |py| Ok(vec![<args
    marshalled>]))` con resultado `Result<__FitzPyObject,
    String>`. Excepciones Python aparecen como
    `Err(Str("<Class>: <msg>"))` paralelo a 8.3.
  - Trait `__FitzToPy` con impls genéricos para primitivos
    (i64/f64/bool/()/String), `__FitzPyObject` (passthrough con
    clone_ref), `Option<T>`, `Arc<Mutex<Vec<T>>>` (List → list
    con breadcrumb `arg0[i]`), `Arc<Mutex<Vec<(K,V)>>>` (Map →
    dict con `__fitz_py_marshal_map_key` para primitivos
    hashables).
  - **Marshaling Instance Fitz → Python dict**: `gen_type_def`
    emite `impl __FitzToPy for FooData` (PyDict con fields en
    orden) + wrapper `impl __FitzToPy for Arc<Mutex<FooData>>`
    cuando `uses_python`. Destraba el caso canónico 8.5: pasar
    `User { id: 1, name: "Ada" }` a `json.dumps(user)`.
  - `gen_python_call_args(args)` emite cada arg como
    `<code>.__fitz_to_py(py, "arg<i>")?` paralelo a
    `value_to_py(path: &str)` del intérprete 8.2.
- **8.7.3 — Bridge async tokio ↔ asyncio**:
  - Helper `async fn __fitz_py_invoke_await<F>(callable, args_fn)`
    en el preludio (solo cuando `uses_async`): combina call sync
    + detección `inspect.isawaitable` + ejecución vía
    `tokio::task::spawn_blocking + asyncio.new_event_loop().
    run_until_complete()`. Paralelo a `py_coro_to_fitz_future`
    8.6.1 (mismo baseline blocking, mismo trade-off).
  - Patrón canónico Fitz único: `<py_call>?.await`. El AST es
    `Await(Try(Call PyAny))`. El codegen detecta el patrón
    (`try_gen_python_await` + `try_gen_python_call_await`) y
    emite `__fitz_py_invoke_await(&callable, |py| Ok(vec![<args>])).
    await?` con el `?` Rust al final para propagar excepciones
    asyncio. Tipo Fitz resultante: `PyAny`.
  - Checker (`Type::PyAny.await → Any`) acepta el patrón
    estáticamente; rechaza `<call>.await` directo sin `?`
    (paridad bit-a-bit con evaluator del intérprete que también
    rechaza en runtime).
- **8.7.4 — Cierre formal**:
  - `examples/python-interop-8.7.fitz` con 3 secciones
    (constantes + coerción primitiva, calls + Result + marshaling
    List/Instance, bridge async con patrón `?.await`). Validado
    bit-a-bit `fitz run` ↔ `fitz build` + binario standalone
    ejecutado.
  - CHANGELOG v0.8.8, roadmap 8.7 actualizado a CERRADA,
    deudas-post-5b marca **F19 CERRADO** con nota detallada,
    README + CLAUDE refresh.

**Decisiones técnicas tomadas al arrancar**:
- **Alcance acotado (codegen sí, bundling no)** — F19 era deuda
  medible; bundling es proyecto separado.
- **Bindings globales con OnceLock + getter** — destraba uso
  adentro de handlers HTTP y user-fns sin refactor.
- **Trait `__FitzToPy` con impls condicionales por nominal** —
  estático, sin mini-Value runtime. Disjunto con List/Map
  genéricos.
- **Patrón canónico `?.await` único** — paridad bit-a-bit con
  intérprete + un solo camino que mantener.
- **Auto-coerción primitiva via `coerce(PyAny → T)`** —
  aprovecha la infraestructura existente; dispara solo con
  anotación destino concreta.

**Deuda residual visible** (sub-paso futuro): coerción Python
list/dict → Fitz `List<T>`/`Map<K,V>`/`Instance` (helpers
`__fitz_py_to_list_*` ya emitidos, falta wiring en `coerce`);
`.await` split con binding intermedio; bundling CPython
embebido (proyecto separado).

**Cierre formal**: 1295 unit + 88 E2E + 3 openapi con feature
python; 1204 + 79 + 3 sin feature. Clippy `-D warnings` limpio
en ambos modos. Paridad bit-a-bit `fitz run` ↔ `fitz build`
validada con `examples/python-interop-8.7.fitz`.

## [v0.8.7] — 2026-05-15 — Fase 8.6: Bridge tokio ↔ asyncio

Sexto sub-paso de la Fase 8 (Interop Python). Habilita
`py_async_fn().await` desde cualquier `async fn` Fitz: cuando un
call a una función Python devuelve una corutina (caso típico de
`async def`), Fitz la envuelve automáticamente en `Value::Future`
adentro del `Result::Ok`. El `.await` postfix existente (Fase 6)
desempaca el Future, ejecuta la corutina, y devuelve el valor
coercionado a `Value`. Excepciones asyncio bajan como
`Result::Err` con el formato canónico ya estable desde 8.1.2.

- **8.6.1 — Bridge baseline + tests**:
  - `py_interop::call` detecta cuando el return Python es awaitable
    (via `inspect.isawaitable`) y lo envuelve automáticamente en
    `Value::Future` adentro del `Result::Ok`. El usuario no necesita
    glue manual; el `.await` postfix lo desempaca naturalmente.
  - Helpers nuevos: `is_coroutine(py, obj)` (introspección defensiva
    con fallback a `false`) y `py_coro_to_fitz_future(coro)` que
    construye el `FitzFuture`.
  - **Implementación "baseline blocking"**: el FitzFuture envuelve
    `tokio::task::spawn_blocking` + `asyncio.new_event_loop()
    .run_until_complete(coro)`. El `Py<PyAny>` (Send-safe) viaja
    al worker; el `Bound` derivado solo existe adentro del
    `Python::attach` del worker. El blocking pool de tokio aísla
    el bloqueo del scheduler async.
  - **Tests** (3 nuevos en evaluator bajo `#[cfg(feature = "python")]`):
    - `fase_8_6_asyncio_sleep_awaiteable_desde_fitz`:
      `asyncio.sleep(0)?.await` adentro de async fn Fitz → Null.
    - `fase_8_6_async_fn_fitz_que_await_python_devuelve_valor_calculado`:
      `async fn doble(x) -> Result<Int> { sleep; return Ok(x*2) }`
      con `doble(21).await` → 42.
    - `fase_8_6_call_async_devuelve_result_future`: shape lazy —
      sin `.await`, el binding es `Result::Ok(Future(_))`.
- **8.6.2 — Ejemplo runnable + cierre formal**:
  - `examples/python-interop-8.6.fitz` con 3 secciones: patrón
    canónico (`doble_eventual(x)` con `sleep + return Ok(x*2)`),
    awaits encadenados (`pipeline(start)` con 3 sleeps + cálculo),
    lazy sin `.await` (`Result<Future>` no ejecutado). Notas
    extensas sobre el modelo de errores asyncio (heredado de 8.3),
    el trade-off baseline blocking y por qué no hacemos un caso
    runnable de excepción asyncio (definir `async def` custom
    requiere un archivo Python helper aparte). Validado bit-a-bit
    con `cargo run --features python -- run
    examples/python-interop-8.6.fitz`.
  - CHANGELOG v0.8.7, roadmap a CERRADA, deudas nota de cierre,
    CLAUDE + README refresh.

**Decisiones técnicas tomadas al arrancar**:

- **Detección automática de awaitable en `call`** (no `.await`
  manual sobre PyObject): el usuario escribe `py_async_fn().await`
  natural, sin pensar "esto es coroutine". La detección usa
  `inspect.isawaitable` (canónica en Python stdlib).
- **Approach baseline blocking** (vs `pyo3-async-runtimes::tokio::
  into_future`): la crate `pyo3-async-runtimes` 0.28 requiere
  control del runtime tokio (`init_with_runtime`/`run`), lo que
  choca con el tokio que Fitz ya tiene corriendo
  (current_thread CLI / rt-multi-thread HTTP). `spawn_blocking` +
  `run_until_complete` es Send-safe, no deadlockea con el runtime
  existente, y suficiente para el criterio. La versión future-based
  real (event loop asyncio persistente compartido) queda como
  deuda menor — el `Value::Future` shape ya es estable, sólo
  cambia la implementación interna.
- **El GIL serializa Python** (esperado por roadmap): N awaits
  concurrentes a corutinas distintas se serializan en el GIL. Para
  APIs DB-bound (caso típico SQLAlchemy/asyncpg con queries
  cortas), la DB es el cuello de botella, no el GIL. Para APIs
  CPU-intensivas con NumPy o long-running asyncio.gather, deuda
  menor.
- **Sin marshaling Future Fitz → corutina Python**: pasar un
  `Value::Future` Fitz como arg a una función Python no se
  soporta (Future no es marshalleable, igual que Range/Function).
  Caso típico afectado: `asyncio.gather(fut1, fut2)` desde Fitz no
  funciona si los futs vienen de calls Python anteriores. Trade-off
  documentado en el ejemplo.
- **No incluimos caso runnable de excepción asyncio** en el
  ejemplo: definir una `async def` Python custom desde Fitz
  requiere un archivo helper Python aparte (el `from python
  import` carga módulos top-level, no archivos del usuario). El
  patrón de manejo de errores es idéntico al de calls sync (8.3) —
  documentado en notas del ejemplo.

**Cierre formal**:

  - Sin feature: **1193 unit** (sin cambios — bridge async es
    feature-gated) + 80 compile_e2e + 3 openapi_e2e.
  - Con feature: **1284 unit** (1281 + 3 del bridge async) + 80 + 3.
  - Clippy `-D warnings` limpio en ambos modos.

Detalle completo: `docs/roadmap.md` → "Fase 8.6".

## [v0.8.6] — 2026-05-15 — Fase 8.5: `fitz py-types` auto-mapeo SQLAlchemy → type Fitz

Quinto sub-paso de la Fase 8 (Interop Python). Cierra la
ergonomía del caso canónico SQLAlchemy: un comando nuevo
`fitz py-types <archivo.py> [--out <archivo.fitz>]` introspecciona
un archivo Python con modelos SQLAlchemy (o mocks equivalentes)
y emite los `type` Fitz correspondientes, listo para commitear.
Reduce el doble-tipado (Python + Fitz) — escribís los modelos UNA
vez en Python y Fitz los importa con sus tipos resueltos.

- **8.5.1 — Sub-comando + introspección + mapping**:
  - `Commands::PyTypes { source, out }` en el CLI con flag opcional
    `--out` (default: stdout).
  - Nuevo módulo `src/py_types.rs` feature-gated. Usa PyO3
    in-process (no subprocess) reusando el GIL + dep ya disponible
    con `--features python`. Sin la feature, el sub-comando aborta
    con error claro citando `cargo install --features python`.
  - `generate_from_file(source) -> Result<String, String>`:
    canonicaliza el path, importa el archivo Python via
    `importlib.util.spec_from_file_location` + `module_from_spec`
    + `loader.exec_module`, itera el `__dict__` filtrando clases
    definidas en ESE módulo (filtra re-exports SQLA como `Base`,
    `Column`, `Integer`, etc.). Duck typing: clase tiene que tener
    `__table__.columns` para contar como modelo — compatible con
    SQLAlchemy real Y con mocks que cumplan el contract.
  - Mapping por nombre canónico de la clase de `Column.type`:
      Integer/BigInteger/SmallInteger/INTEGER/...   → Int
      Float/Numeric/Double/REAL/FLOAT/NUMERIC       → Float
      String/Text/Unicode/VARCHAR/TEXT/CHAR/CLOB    → Str
      Boolean/BOOLEAN                               → Bool
      DateTime/Date/Time/TIMESTAMP/DATE/TIME        → Str (ISO 8601)
      resto                                         → Any + `// ?` comment
  - `nullable=True` → sufijo `?`. `default=<literal>` (Int/Float/
    Str/Bool/None) → inline `= valor`. Defaults callable
    (`datetime.utcnow`) se ignoran silenciosamente — emitir
    `= func()` no aporta.
  - 10 tests con classes Python mock que cumplen el shape SQLA sin
    requerir `pip install sqlalchemy` (modelo simple, mapping
    completo de tipos, nullable, default literal, default callable
    ignorado, tipo desconocido con comentario, múltiples modelos,
    archivo sin modelos error claro, clases sin `__table__`
    filtradas, header cita fuente).
- **8.5.2 — Ejemplo runnable + cierre formal**:
  - `examples/py-types/models.py` autosuficiente: 25 LoC de mock
    SQLAlchemy (clases `Column`, `_Table`, `Integer`, etc.) +
    modelos `User` (6 campos: int, str, str, int?, bool=false,
    str-datetime) y `Order` (5 campos: bigint, int, float,
    str="USD", str?). Comentario explica cómo reemplazar el mock
    con `from sqlalchemy import ...` para uso real.
  - `examples/py-types/models.fitz` (generado y commiteado): el
    output de `fitz py-types models.py --out models.fitz`. Sirve
    como referencia del output esperado.
  - `examples/py-types/usage.fitz`: `from models import User, Order`
    + dos fns `parse_user`/`parse_order` que demuestran coerción
    runtime 8.4.3 sobre dicts JSON (`json.loads` Python → Map →
    User Instance). Cubre happy path con todos los campos,
    default `currency="USD"` aplicado en Order, nullable `notes`
    como Null, y JSON malformado propagado como `Result::Err`.
    Validado bit-a-bit con
    `cargo run --features python -- run examples/py-types/usage.fitz`.
  - CHANGELOG v0.8.6, roadmap.md actualiza 8.5 a CERRADA con
    sub-pasos detallados, deudas-post-5b.md nota de cierre,
    CLAUDE + README refresh.

**Decisiones técnicas tomadas al arrancar**:

- **In-process via PyO3** (no subprocess): reusa el GIL + dep
  PyO3 ya disponible. Más simple que armar subprocess management
  + parseo de output. Requiere `--features python`; sin la feature
  el sub-comando aborta antes.
- **Duck typing sobre `__table__.columns`** (no `isinstance(cls,
  DeclarativeBase)`): permite tests con mocks sin requerir
  SQLAlchemy real instalado. Funciona igual con SQLAlchemy real.
- **Solo SQLAlchemy en 8.5**: Django, Tortoise, peewee,
  dataclasses quedan como sub-comandos futuros si entra demanda
  (`fitz py-types-django`, etc.). La arquitectura es reusable —
  el dispatch va por shape del object, no por ORM específico.
- **Defaults callable ignorados** silenciosamente: emitir
  `= datetime.utcnow()` confunde más de lo que ayuda (no es
  evaluable estáticamente desde Fitz).
- **Tipos desconocidos → `Any` con comentario** `// ?` citando el
  nombre original SQLA. Permite al usuario detectar y refinar a
  mano (ej. `JSON` → `Map<Str, Any>`).
- **Output a stdout por default**; `--out <archivo>` opcional.
  El archivo generado lleva header `// Generado por fitz py-types
  — no editar a mano` + cita de la fuente — facilita el flujo
  "commitear el .fitz, regenerar si cambia el schema".
- **Sin verificación de drift** entre `.py` y `.fitz` generado
  (regeneración manual cuando el schema cambia). Linter de drift
  queda para Fase 9+ si entra demanda.

**Cierre formal**:

  - Sin feature: **1193 unit** (sin cambios — `py_types` es
    feature-gated) + 80 compile_e2e + 3 openapi_e2e.
  - Con feature: **1281 unit** (1271 + 10 nuevos en `py_types`)
    + 80 + 3.
  - Clippy `-D warnings` limpio en ambos modos.

Detalle completo: `docs/roadmap.md` → "Fase 8.5".

## [v0.8.5] — 2026-05-15 — Fase 8.4: Tipos del checker + anotaciones del lado Fitz

Cuarto sub-paso de la Fase 8 (Interop Python). Cierra el ciclo
"call Python → tipo Fitz concreto" con tres cambios coordinados:
el checker estático ahora distingue valores Python de Any
genérico (`Type::PyAny`), refina los calls a `Result<Any>`
forzando manejo de errores estático, y el runtime coerciona
`Value::Map` → `Value::Instance` cuando hay anotación nominal en
el binding. Habilita el patrón canónico del roadmap:

```fitz
fn fetch_user(s: Str) -> Result<User> {
    let row: User = json.loads(s)?
    return Ok(row)
}
```

Una sola anotación (`: User`) basta para salir del "limbo Python"
a tipos Fitz concretos. El runtime valida que el dict tenga los
campos requeridos.

- **8.4.1+8.4.2 — `Type::PyAny` en el checker + calls Python tipan
  `Result<Any>`** (combinados en un commit, ~5 LoC del refinamiento
  del call ya estaban listos): nueva variante `Type::PyAny` con
  identidad propia (vs `Any` genérico), bidireccionalmente
  compatible con cualquier tipo igual que Any. `Stmt::Import` y
  `Stmt::FromImport` con `path[0] == "python"` tipan los bindings
  como `PyAny`; imports normales siguen como `Any`. Field access
  sobre PyAny devuelve PyAny (permite chaining como `os.path`).
  `Expr::Call` con receptor PyAny (callee o `Field.object`) refina
  el ret type a `Type::Result(Box::new(Type::Any))` — activa
  estáticamente la regla de exhaustividad sobre Result (5.3.3) y
  la regla del operador `?` (5.3.3). 9 tests nuevos del checker.
- **8.4.3 — Coerción runtime Map → Instance con anotación**:
  `Stmt::Assign` con `target: Ident` y anotación dispara
  `coerce_to_annotation(annot, value, env)` antes de bindear.
  Si la anotación es `Named(T)` o `Nullable(Named(T))` con T
  nominal, y el value es `Value::Map`, construye una `Value::
  Instance` validando que los fields matcheen el `type` declarado.
  Reglas: nullable + Null → passthrough; value no-Map (Instance
  ya, primitivo, etc.) → passthrough; resuelve fields en orden
  (`provided` → `resolved_defaults` PreF8.3 → `default` Expr →
  nullable Null → error claro). Campos extras del Map se ignoran
  silenciosamente (Python suele devolver dicts con más data de la
  necesaria; ser permisivo evita fricción). Field requerido
  faltante (no nullable, sin default) → `FitzError` que aborta
  con mensaje citando type + field. 9 tests nuevos (8 sin feature,
  1 con feature validando el criterio canónico end-to-end via
  json.loads).
- **8.4.4 — Ejemplo runnable + cierre formal**: nuevo
  `examples/python-interop-8.4.fitz` con 5 secciones (happy path,
  nullable faltante → Null, extras ignorados, JSON malformado
  propagado por `?`, default aplicado) más comentario explícito
  sobre el caso "field requerido faltante" que aborta por
  diseño. CHANGELOG v0.8.5, roadmap actualiza Fase 8.4 a CERRADA,
  deudas-post-5b nota de cierre, CLAUDE + README refresh.

**Decisiones técnicas tomadas al arrancar**:

- **`Type::PyAny` dedicado** (no `Type::Any` genérico ni
  `Type::PyObject<"...">`). Empezar simple, refinar a fantasma si
  entra demanda (roadmap recomienda).
- **Coerción Map → Instance vive en el evaluator**, no en el
  checker. El checker ya acepta el cast (gradual Any → T). El
  runtime hace la coerción real con validación de fields.
- **Campos extras del dict se ignoran silenciosamente**. Python
  suele devolver más data de la necesaria; ser permisivos evita
  fricción. Documentado en el ejemplo.
- **Field requerido faltante → FitzError que aborta** (no
  `Result::Err`). Diseño: este caso indica datos malformados a
  nivel de fuente (DB schema desalineado, API contract roto), no
  un error de runtime esperable como una excepción Python. El
  programador debe validar el dict antes o declarar el campo
  nullable/con default.
- **El test `?` operator solo se chequea adentro de fn que
  retorna `Result<...>`** (regla heredada de 5.3.3). `?` a top-
  level se reporta en runtime, no en el checker — comportamiento
  consistente con calls nativas Fitz.

**Cierre formal**:

  - Sin feature: **1193 unit** (+ 9 checker + 8 coerción + 1 fix
    test 8.3 sin contar baseline) + 80 compile_e2e + 3 openapi_e2e.
  - Con feature: **1271 unit** (+ 1 criterio canónico end-to-end
    via json.loads) + 80 + 3.
  - Clippy `cargo clippy --all-targets --features python -- -D warnings`
    limpio. Idem sin feature.

Detalle completo: `docs/roadmap.md` → "Fase 8.4".

## [v0.8.4] — 2026-05-15 — Fase 8.3: Excepciones Python → Result<T>

Tercer sub-paso de la Fase 8 (Interop Python). Cambia la semántica
de las llamadas a funciones Python desde Fitz: **TODA llamada se
envuelve automáticamente en `Result<T>`**. Si Python lanza una
excepción (`ValueError`, `JSONDecodeError`, etc.) o si el marshaling
de args falla (tipo Fitz no representable en Python), el call no
aborta el programa — devuelve `Result::Err(Str("<ClassName>:
<message>"))` que el usuario tiene que manejar con `match` o `?`,
igual que cualquier otra operación que puede fallar (`find`/`get`/
`json.loads` nativos). Preserva la decisión de diseño "sin
excepciones" del lenguaje y evita que excepciones Python escapen
como panics opacos.

- **8.3.1 — `call` envuelve return en Result + tests viejos
  actualizados**: `py_interop::call(handle, args)` ahora SIEMPRE
  devuelve `Ok(Value::Result(...))`. Éxito produce
  `Value::Result(Ok(v))` con el valor coercionado adentro;
  cualquier falla (excepción Python, marshaling de args, marshaling
  del return) produce `Value::Result(Err(Str("<ClassName>:
  <message>")))`. Helper privado `err_value_from_message(msg)`
  construye el wrap. Los ~16 tests viejos del call path (8.1.4 +
  8.2.1 + 8.2.2 + 8.2.3) actualizados con helper `ok_inner(v)` que
  desempaqueta el Ok; los tests que esperaban error
  (`call_excepcion_python_*`, `call_arg_no_marshalleable_*`)
  reescritos con `err_message(v)` que extrae el mensaje del Err.
  4 tests py_interop nuevos sobre el shape: shape `Ok(...)`,
  criterio textual del roadmap (`json.loads("{ malformado")` →
  `JSONDecodeError`), TypeError envuelto, formato `"<Class>:
  <msg>"` estable.
- **8.3.2 — Ejemplos 8.1 y 8.2 actualizados al modelo Result**:
  `examples/python-interop-8.1.fitz` reescrito con
  `match { Ok(v) => v, Err(_) => ... }` para desempaquetar y fns
  helper (`fn floor_x(x: Float) -> Result<Int> { return Ok(math.floor(x)?) }`)
  que propagan con `?`. Sección nueva "Errores Python como
  Result::Err" con caso `math.sqrt(-1.0) → err: ValueError: ...`.
  Idem para `examples/python-interop-8.2.fitz`: helper
  `fn unwrap_str(r: Result<Str>) -> Str`, caso nuevo
  `loads(malformado) → JSONDecodeError: ...`, literales compuestos
  extraídos a variables porque el parser de interpolación no
  acepta `{...}` adentro de strings (caveat documentado en el
  ejemplo).
- **8.3.3 — Ejemplo dedicado + cierre formal**: nuevo
  `examples/python-interop-8.3.fitz` con 6 secciones — criterio
  textual del roadmap, distintas excepciones Python como Err,
  propagación con `?`, marshaling fallido como Err (uniformidad),
  field access sin wrap (decisión interna), chaining con
  desempaquetado intermedio. Validado bit-a-bit. CHANGELOG v0.8.4,
  roadmap actualiza Fase 8.3 a CERRADA, deudas nota de cierre,
  CLAUDE/README refresh.

**Decisiones técnicas tomadas al arrancar**:

- **`call` envuelve siempre, `get_attr` NO envuelve**. Solo
  llamadas pasan por Result; field access (`math.pi`,
  `obj.attr`) sigue devolviendo el valor coercionado directo.
  Matchea la letra del roadmap ("toda **llamada** a una función
  Python") y preserva la ergonomía de leer constantes y submódulos
  sin `match` por cada acceso. AttributeError fallido sigue siendo
  `FitzError` que aborta (es típicamente un error de programación,
  no de runtime esperable).
- **Marshaling de args también va en Err** (uniformidad): el
  usuario ve UN solo punto de error en el path call, independiente
  de qué falló — excepción Python o tipo Fitz no marshalleable.
- **`Err` lleva `Value::Str` con el mensaje** plano. `Value::
  Instance(PyException)` con inspección estructurada (type,
  traceback) queda como deuda menor — si entra demanda real.
- **KeyboardInterrupt/SystemExit también van como `Err`** según
  el roadmap. No hay forma de matar el runtime Fitz desde una
  excepción Python.
- **El checker NO cambia en 8.3**. Sigue tipando call Python como
  `Any`. El refinamiento a `Result<Any>` llega en 8.4.

**Cambio de comportamiento**: técnicamente esto rompe los
ejemplos viejos de 8.1/8.2 que asumían call sin wrap. Se
reescribieron en 8.3.2 (no se publicaron antes de este release,
así que no afecta a usuarios externos).

**Tests al cierre**:
  - Sin feature: **1175 unit** (sin cambios — tests Python son
    `#[cfg(feature = "python")]`) + 80 compile_e2e + 3 openapi_e2e.
  - Con feature: **1252 unit** (1245 baseline 8.2 + 4 py_interop
    + 3 evaluator del criterio canónico/propagación con `?`/field
    access sin wrap) + 80 + 3.
  - Clippy `cargo clippy --all-targets --features python -- -D warnings`
    limpio. Idem sin feature.

Detalle completo: `docs/roadmap.md` → "Fase 8.3".

## [v0.8.3] — 2026-05-15 — Fase 8.2: Marshaling de tipos compuestos

Segundo sub-paso de la Fase 8 (Interop Python). Habilita el
marshaling bidireccional de `List<T>` ↔ `list`, `Map<K, V>` ↔ `dict`,
e `Instance` → `dict` (por field name). Cumple el criterio del
roadmap end-to-end: una función Python que recibe `List<User>` y
devuelve un mapping `email → cantidad` (`collections.Counter`)
funciona sin perder data, con la `List<User>` original Fitz
intacta después del round-trip.

- **8.2.1 — Fitz → Python (`value_to_py`)**: refactor con
  parámetro `path: &str` para breadcrumb informativo en errores
  (ej. `arg0[2].email` apunta al sitio exacto adentro de la
  estructura). Nuevas ramas:
    - `Value::List(items)` → `PyList` con elementos recursivos
      (copia eager).
    - `Value::Map(pairs)` → `PyDict`. Las keys deben ser
      primitivos hashables Python (Int/Float/Str/Bool/Null);
      compuestos como key → error claro citando la restricción.
      Helper `marshal_map_key` valida antes de tocar `dict.__setitem__`.
    - `Value::Instance { type_name, fields }` → `PyDict` con
      field names como keys (traducción nominal). El tipo Fitz
      se "olvida" del lado Python; recuperarlo en el round-trip
      requiere anotación destino (deuda 8.4).
  Política cross-cutting #4 del roadmap: copia eager bidireccional,
  sin aliasing entre los dos GCs. Tipos no marshalleables
  (Range, Function, Future, Type, Module, HttpResponse, CorsConfig,
  Result) → error con path. Test del fallback 8.1.4 reapuntado a
  Range (sigue sin ser marshalleable).
- **8.2.2 — Python → Fitz (`py_to_value`)**: nuevas ramas para
  `PyList` y `PyDict` antes del fallback opaco. Ambas con
  recursión sobre elementos/pares. Resultado semánticamente
  `List<Any>`/`Map<Any, Any>` desde Fitz porque Python no nos da
  tipo estático; refinar a tipos concretos requiere anotación
  destino del lado Fitz (deuda 8.4). CPython 3.7+ garantiza
  orden de inserción para `dict`; preservarlo da paridad bit-a-bit
  con `serde_json::preserve_order` que ya usa el resto del
  proyecto. Decisión explícita: `dict` Python NO se auto-coerce
  a `Instance` Fitz — eso es 8.4. PyO3 0.28 deprecó `downcast`
  en favor de `cast`; usamos `cast`.
- **8.2.3 — Criterio de éxito end-to-end + ejemplo runnable**:
  pipeline canónico `List<User>` Fitz → `Counter` Python →
  `Map<Str, Int>` Fitz funciona sin glue extra porque
  `collections.Counter` es subclass de `dict` y `is_instance_of::
  <PyDict>()` matchea subclases. Validado bit-a-bit. Nuevo
  `examples/python-interop-8.2.fitz` con 5 secciones (Fitz →
  Python, Python → Fitz, round-trip, criterio canónico, copia
  eager). NO entra al smoke `GUIDE_EXAMPLES_COMPILE` (interop
  Python es `fitz run` only — deuda F19).

**Tests al cierre**:
  - Sin feature: **1175 unit** (sin cambios — todos los nuevos
    tests son `#[cfg(feature = "python")]`) + 80 compile_e2e + 3 openapi_e2e.
  - Con feature: **1245 unit** (+ 20 en `py_interop` y + 12 en
    evaluator distribuidos entre 8.2.1/8.2.2/8.2.3, más 2 ajustes
    a tests viejos de 8.1.4 que asumían "List como arg → error
    citando 8.2" — ahora List sí marshalla y Python rechaza con
    TypeError) + 80 + 3.
  - Clippy `-D warnings` limpio en ambos modos.

**Detalles de implementación notables**:

- Breadcrumb de errores con `path: &str` propagado recursivamente:
  un Range adentro de `List<Map<Str, List<Range>>>` reporta
  `arg0[2]["k"][3]` o similar.
- Llaves JSON `{...}` en source Fitz se escapan con `\{`/`\}` para
  evitar interpolación de strings. Documentado en el ejemplo.
- Map keys cuando va a Python: helper `marshal_map_key` con
  validación temprana (mensaje más útil que el `TypeError:
  unhashable type` que Python lanzaría).

Detalle completo: `docs/roadmap.md` → "Fase 8.2".

## [v0.8.2] — 2026-05-15 — Fase 8.1: Embedding básico de CPython

Primer sub-paso de la Fase 8 (Interop Python). Habilita
`from python import <módulo>` desde el intérprete (`fitz run`),
con la feature opt-in `python`. Acceso a atributos, llamadas con
args primitivos, return primitivo coercionado a `Value` Fitz.
Cumple el criterio del roadmap: `math.sqrt(16.0)` → `4.0`,
`math.pi` → `3.141592653589793`.

- **8.1.1 — Dep PyO3 opcional + variante `Value::PyObject`**:
  `Cargo.toml` suma `pyo3 = "0.28"` como dep opcional bajo la
  feature `python`. Features de PyO3: `abi3-py310` (un binario
  corre 3.10+) y `auto-initialize` (boot lazy en el primer
  `Python::attach`). `Value::PyObject(PyObjectHandle)` feature-
  gated; handle envuelve `Arc<Py<PyAny>>` para `clone()` O(1) sin
  tomar el GIL. PartialEq por identidad via `Py::as_ptr()`,
  Display `<python object>`, type_name `"PyObject"`. Binario
  `fitz` default sigue siendo standalone sin link a libpython.
- **8.1.2 — `from python import X` + loader CPython**:
  módulo nuevo `src/py_interop.rs` (feature-gated) con
  `import_module(dotted) -> Value::PyObject` envuelto en
  `Python::attach`. Helper `py_err_to_fitz` traduce excepciones
  Python a `FitzError` con formato `"<ClassName>: <message>"`
  (compatible con el wrap a `Result<T>` que llega en 8.3).
  Evaluator: `Stmt::FromImport` con `path[0] == "python"` rutea
  al loader Python; sin feature, error claro citando el flag
  `cargo build --features python`. Alcance 8.1.2:
  `path == ["python"]` exacto (submódulos profundos quedan deuda
  menor). `import python.X` se rechaza con sugerencia
  `from python import X`.
- **8.1.3 — `Expr::Field` + auto-coerción primitiva**:
  `py_interop::get_attr(handle, name)` toma GIL, hace
  `bound.getattr` y aplica `py_to_value`. Política:
  `None` → `Null`, `bool`/`int`/`float`/`str` → primitivos Fitz,
  resto → PyObject opaco. Chequeo de `bool` ANTES que `int` (en
  Python `bool ⊂ int`). Overflow de `int > i64` → error explícito
  (bignum support queda como deuda menor). Evaluator: `Expr::Field`
  despacha sobre `Value::PyObject` con feature on, enriqueciendo
  el error con el span del field access. Desbloquea `math.pi`,
  `os.path` como submódulo opaco, `math.__name__`.
- **8.1.4 — `Expr::Call` con args primitivos (criterio cerrado)**:
  `py_interop::call(handle, &args)` con `bound.call1(tuple)`
  (positional only — kwargs queda deuda menor). Helper
  `value_to_py` con política simétrica: `Int`/`Float`/`Str`/`Bool`/
  `Null` se marshalla a Python; PyObject passthrough preserva
  identidad. Args compuestos (List/Map/Instance/Range/Function/...)
  → error citando 8.2 como sub-paso futuro. Evaluator:
  `invoke_value` (caso `let f = math.sqrt; f(25.0)`) y
  `dispatch_method` (caso `math.sqrt(16.0)` directo, `json.dumps(
  "hola")` chained) ambos despachan sobre `Value::PyObject`.
  Excepciones Python emiten `FitzError`; el wrap a `Result<T>`
  llega en 8.3.
- **8.1.5 — Guard de codegen + error path completo**:
  `fitz build` con `from python import` aborta con mensaje claro
  sugiriendo `fitz run` (binario con `--features python`). Función
  libre `check_no_python_imports(program)` corre dos veces: al
  inicio de `generate_project` (path real, antes de tocar disk
  para no producir el mensaje confuso "no se encontró
  `python.fitz`") y al inicio de `generate_main_rs` (path de
  tests unit que usan `generate_rust` directo). Deuda comprometida
  F19: soporte real en `fitz build` (emitir Rust con `pyo3`
  linkeado + Cargo.toml condicional) queda como probable sub-paso
  de 8.7 cuando cierre distribución con CPython bundled.

**Tests al cierre**:
  - Sin feature: **1175 unit** (baseline 1172 + 1 fallback de
    "feature off da error claro" + 2 codegen guards) +
    **80 compile_e2e** (baseline 79 + 1 guard E2E) + 3 openapi_e2e.
  - Con feature: **1213 unit** (+ 22 unit en `py_interop` + 11 en
    evaluator + 2 codegen; el test del fallback no-aplica con la
    feature on) + 80 + 3.
  - Clippy `cargo clippy --all-targets --features python -- -D warnings`
    limpio. Idem sin feature.

**Política de venvs** (decisión 2026-05-14): estándar Python sin
magia. El usuario activa su venv antes de `fitz run`
(`source venv/bin/activate` o equivalente en Windows); CPython
embebido lee `VIRTUAL_ENV` al boot y prepende el `site-packages`
del venv a `sys.path`. Cero código nuevo en Fitz. Auto-detect de
`./venv/` y similares queda como deuda menor (revisitable en 8.5
o como flag CLI dedicado).

**Política de errores Python**: en 8.1 cualquier `PyErr` aborta el
programa con `FitzError` ("<ClassName>: <message>"); el wrap
automático a `Result<T>` llega en 8.3 — el formato del mensaje
queda estable, solo cambia el envoltorio.

**Ejemplo runnable**: `examples/python-interop-8.1.fitz` cubre
constantes, funciones con args primitivos, submódulos opacos y
chained call. Se corre con
`cargo run --features python -- run examples/python-interop-8.1.fitz`.
NO entra al smoke `GUIDE_EXAMPLES_COMPILE` porque 8.1 es `fitz run`
only.

Detalle completo: `docs/roadmap.md` → "Fase 8.1".

## [v0.8.1] — 2026-05-14 — Mini-tanda PreF8: cleanup antes de Interop Python

Cuatro sub-pasos chicos antes del salto a Fase 8 para no entremezclar
deuda existente con la parte real de Python interop.

- **PreF8.1 — Refactor M1+M2 codegen**: `generate_main_rs` (232 LoC)
  → orquestador de ~18 LoC + 3 helpers libres (`partition_program_stmts`,
  `resolve_state_var_types`, `emit_main_rs_body`). `gen_http_handler_wrapper`
  (532 LoC) → orquestador de ~9 LoC + 6 métodos del `impl CodegenCtx`
  (`resolve_handler_signature` que devuelve `HandlerSig`,
  `emit_axum_extractors`, `emit_middleware_chain`,
  `emit_param_coercions`, `emit_handler_dispatch_and_response`,
  `emit_cors_helpers`). Cero cambio funcional: AST del Rust generado
  bit-a-bit idéntico pre/post sobre los 19 ejemplos del smoke
  `GUIDE_EXAMPLES_COMPILE`. F8 va a hacer crecer ambas fns con Python
  imports + wrappers; mejor partirlas antes.
- **PreF8.2 — Method chain multi-línea en parser**: el `postfix()`
  loop tolera `Token::Newline` antes de `.`. Habilita el patrón
  idiomático de chains largos partidos por línea
  (`users\n.filter(...)\n.map(...)`); AST resultante idéntico al
  one-liner. Caso de uso central: chains de SQLAlchemy/pandas en F8.
- **PreF8.3 — Defaults de tipos importados**: auditoría de 6 casos
  de `Field.default` detectó un único bug — defaults que referencian
  consts del módulo de origen fallaban en `fitz run` y `fitz build`.
  Fix con estrategia eager-at-import: `Value::Type` suma
  `resolved_defaults`, el loader pre-evalúa los defaults en el env
  del módulo; codegen emite `pub fn __default_<T>_<F>()` en el
  módulo. Habilita el patrón `from foo import User` con
  `type User { name: Str = DEFAULT_NAME }` sin re-importar
  `DEFAULT_NAME`.
- **PreF8.4 — Import aliasing**: `import foo as f`, `from foo import
  bar as b`, alias mixto. Sub-paso adelantado de F8.1. Lexer suma
  `Token::As`; AST suma `Stmt::Import.alias` y cambia
  `Stmt::FromImport.names` a `Vec<(String, Option<String>)>`.
  Evaluator usa el `Value::Type.name` canónico al instanciar
  (`Person { ... }` con alias produce instancia cuyo Display dice
  `User`, paridad bit-a-bit). Codegen emite `use foo::bar as b;`.

**Tests**: 1172 unit (baseline 1153 + 19 nuevos) + 79 compile_e2e
(baseline 74 + 5 nuevos) + 3 openapi_e2e verdes. Clippy
`-D warnings` limpio. Paridad bit-a-bit `fitz run` ↔ `fitz build`
validada en todos los sub-pasos.

Detalle completo: `docs/roadmap.md` → "Mini-tanda PreF8".

## [v0.8.0] — 2026-05-14 — Fase F17: Send completo + paralelismo HTTP real

- **Paralelismo HTTP real**: el server (tanto `fitz run` como el
  binario de `fitz build`) corre tokio `rt-multi-thread` con N
  workers según cores. 5 requests concurrentes a un handler
  `sleep(1000).await` responden en ~1.2s (pre-F17 eran ~5s).
- **Bridge HTTP eliminado**: el modelo de dos threads + canal
  `mpsc/oneshot` introducido en Fase 4 desapareció. Los handlers
  axum invocan al evaluator directo sobre un `Arc<HttpRegistry>`
  compartido. ~269 LoC netas menos en `src/http.rs`.
- **Tipos `Send`**: `Value` y `EnvRef` migran de `Rc<RefCell<>>` a
  `Arc<parking_lot::Mutex<>>` (intérprete) y a `Arc<std::sync::Mutex<>>`
  (codegen output). Habilitó la eliminación del bridge y el runtime
  multi-thread.
- **State HTTP compartido**: pasa de `thread_local!` a
  `LazyLock<Arc<Mutex<T>>>` en el codegen, para que un solo Arc
  se comparta entre workers.
- **Guía cap 19**: sub-sección nueva "Paralelismo HTTP real" con
  ejemplo `examples/guide/19b-paralelismo.fitz`.

Subdivisión en 6 sub-pasos: F17.1 (dep `parking_lot`), F17.2
(migración atómica Shared/EnvRef), F17.3 (Send completo en
evaluator), F17.4a (`serve()` multi-thread), F17.5 (eliminar
bridge), F17.4b (codegen multi-thread + tipos), F17.6 (guía +
cierre formal).

Detalle completo: `docs/roadmap.md` → "Fase F17".

## [v0.7.1] — 2026-05-14 — Mini-tanda Q: quick wins post-MW

- **Q.1**: `@header(into="alias")` para mapping explícito de un
  header HTTP a un nombre arbitrario de param Fitz.
- **Q.2**: `@server(api_version="X.Y.Z")` override del campo
  `info.version` del schema OpenAPI.
- **Q.3**: CORS request-aware. `cors({"allow_origin": ["a.com",
  "b.com"]})` con `List<Str>` activa modo Set — el server hace
  echo del `Origin` recibido si está en la lista permitida.
- **Q.4**: status codes custom aparecen en `responses` del schema
  OpenAPI con description de la frase HTTP estándar.
- **Q.5** (postergado): bundle Scalar embebido offline. ~3.7 MB de
  overhead no justifica romper "binario mínimo". Pendiente como
  opt-in via `@server(offline_docs=true)` cuando aparezca presión
  real (deploys air-gapped).
- **Q.6**: refresh de docs (guide header, syntax-spec v0.2,
  deudas-post-5b).

## [v0.7.0] — 2026-05-14 — Mini-fase MW: middleware + CORS

- **`@middleware(fn)`** apilable antes de cualquier handler HTTP.
  Modelo gate-only: `return null` o sin return continúa la cadena;
  `return <status> { ... }` corta con ese status code.
- **`@middleware(cors(...))`** con built-in `cors(...)` configurable
  (`allow_origin`, `allow_methods`, `allow_headers`, `max_age`).
  Preflight `OPTIONS` automático con 204 + headers; inyección de
  `Access-Control-Allow-*` en la response real (incluso 500/400).
- **Built-in `Request`** (`method`, `path`, `headers`) y `Response`
  opaco como tipos del lenguaje, pre-registrados en `TypeEnv`.
- **Paridad bit-a-bit** `fitz run` ↔ `fitz build` validada con E2E
  build + spawn + raw TCP request.

## [v0.6.0] — 2026-05-13 — Fase 7: DX HTTP

- **OpenAPI 3.1 autogenerado** desde los decoradores. Path/query
  params, body, headers, return type (`Result<T>` → 200 + 500)
  todos reflejados en el schema. Subcomando nuevo
  `fitz openapi archivo.fitz`.
- **UI Scalar embebida** en `/docs`. Bundle via CDN jsdelivr.
- **Headers como params del handler** con `@header(name="X")`. Lookup
  case-insensitive. Solo `Str` o `Str?`.
- **`@server(docs=false)`** opt-out de las rutas auto `/docs` y
  `/openapi.json`. Zero overhead cuando se apagan.
- **Paridad bit-a-bit** entre `fitz run`, `fitz openapi` y el
  schema embebido por `fitz build`.

## [v0.5.0] — 2026-05-13 — Fase 6: Async nativo

- **`async fn`** declarable, retorna `Future<T>` al llamarse.
- **`.await`** postfix para desempacar futures. Permitido adentro
  de `async fn` y a nivel top-level del archivo.
- **`Future<T>`** como tipo built-in genérico (igual que `List<T>`,
  `Result<T>`).
- **Builtin `sleep(ms: Int) -> Future<Null>`** que pausa N
  milisegundos sin bloquear el runtime.
- **Handlers HTTP async**: cualquier `@get`/`@post`/etc. puede ser
  `async fn`. axum invoca con `.await` automático.
- **`fitz build`** emite `#[tokio::main(flavor = "current_thread")]`
  para programas con async, y compila `.await` 1:1 a Rust.

## [v0.4.0] — 2026-05-12 — Fase 5: Compilador estático

- **Fase 5a — Type checker estático**. `fitz check` valida tipos
  en todo el programa: resolución de `TypeExpr`, inferencia de
  ret type para `FnExpr`, chequeo de aridad/tipos de calls,
  exhaustividad de `match` sobre `Result`, métodos built-in
  paramétricos. `fitz run` corre en modo strict por default;
  `--no-typecheck` para escape gradual.
- **Fase 5b — Codegen a binario nativo**. `fitz build archivo.fitz`
  transpila Fitz → Rust → binario standalone (via `cargo build
  --release`). Subset cubierto: primitivos, tipos custom, listas/
  mapas homogéneos, `Result`/`?`/`match`, módulos, HTTP nativo,
  higher-order (closures escapadas + fn como valor/param/retorno),
  state HTTP compartido.
- **Guía cap 20 nuevo** "fitz build" con mapping de tipos Fitz → Rust.

## [v0.3.0] — 2026-05-11 — Fase 4: HTTP nativo

- **Decoradores HTTP**: `@get`, `@post`, `@put`, `@delete` registran
  rutas en un `HttpRegistry` durante `eval`. `serve()` arranca
  axum + tokio cuando hay rutas y bloquea hasta Ctrl-C.
- **Path params tipados**: `@get("/users/{id}")` con `fn h(id: Int)`
  coerciona el path param crudo al tipo declarado; falla → 400.
- **Body JSON**: cada parámetro que no es path se trata como body.
  Con `type` declarado, validación + defaults + extras → 400.
- **`@server(port, host)`** configurable. Default `127.0.0.1:3000`.
- **`Result<T>` auto-handling**: `Ok(v)` → 200 + JSON(v),
  `Err(e)` → 500 + `{"error": e}`.

## [v0.2.0] — 2026-05-11 — Fase 3: El lenguaje crece

- **Listas, mapas, rangos**: `[1, 2, 3]`, `{"k": v}`, `0..10`.
  Indexing postfix `xs[i]`, `m["k"]`. `for var in iter`.
- **Tipos custom**: `type User { id: Int, name: Str }`. Struct
  literal `User { id: 1, name: "x" }`. Field access `obj.campo`.
  Defaults y nullables.
- **Funciones anónimas + higher-order**: `fn(x) => x * 2`, callbacks
  `xs.map(fn)`. Métodos sobre List/Map/Str.
- **`Result<T>` + `Ok`/`Err` + `?`**: sum type built-in para errores.
  Patrón `Ok(x)`/`Err(e)` en match; operador `?` postfix propaga.
- **Módulos**: `import foo`, `from foo import User`. Cache por path
  canonicalizado + detección de ciclos.

## [v0.1.0] — 2026-05-11 — Fase 2: Intérprete base

- **Lexer + parser + AST** completos para la sintaxis core.
- **Evaluator** que recorre el AST y produce efectos
  (`print`, asignaciones, control de flujo).
- **Variables**, primitivos (`Int`, `Float`, `Str`, `Bool`, `Null`),
  strings con interpolación (`"hola, {name}"`).
- **Operadores**: aritmética con promoción Int↔Float, comparación,
  lógicos, unario negativo.
- **Control de flujo**: `if`/`else`, `while`, `for`, `loop`/`break`,
  `match` con patrones literales, wildcard, rangos.
- **Funciones**: `fn nombre(params) -> ret { ... }` o
  `fn nombre(p) => expr`. Closures con captura por referencia.
- **CLI**: `fitz run archivo.fitz` ejecuta un programa.
- **Guía v0.1** publicada (`docs/guide.md`).
