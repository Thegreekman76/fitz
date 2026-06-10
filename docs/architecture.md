# Arquitectura del compilador

Este documento describe cómo está organizado el código en [src/](../src/) y
qué le pasa a un programa Fitz desde que es texto en un archivo `.fitz`
hasta que produce output (un `print`, una respuesta HTTP, un binario
nativo, o un test reportado). Es la referencia para entender el
compilador desde adentro; para aprender el lenguaje desde afuera, ver
[docs/guide.md](guide.md).

> **Estado**: este doc cubre hasta el cierre de **Fase 12 entera +
> Fase 8.b/8.c + TaskHub** (v0.15.x, junio 2026). Refresh sincronizado
> con el estado del repo. Incluye Fase 9.w MVP (auth + WS + cron) y
> sus iteraciones 2 (RBAC + persistencia de jobs), Fase 10 entera
> (driver Postgres puro + ORM + migrations), Fase 12 (healthz/readyz
> + Secret + observability OTel + Docker + deploy + @trace/@metric/
> @flag), Fase 8.b/8.c (CPython embebido + paquetes pip bundleados),
> y el CLI builder (Fase 13). Cuando avancemos a Fase 13+ o más allá,
> este doc se actualiza junto.

## Pipeline en una imagen

```mermaid
flowchart TD
    Source["archivo .fitz<br/>(texto fuente)"] --> Lexer["<b>lexer.rs</b><br/>tokenizar"]
    Lexer --> Tokens["Vec&lt;Token&gt;"]
    Tokens --> Parser["<b>parser.rs</b><br/>parsear con precedencia<br/>(usa ast.rs)"]
    Parser --> AST["Program = Vec&lt;Stmt&gt;<br/>(ast.rs)"]
    AST --> Checker["<b>types.rs</b><br/>check_program<br/>(resolver + chequear)"]
    Checker -->|errores| Abort["✗ stderr + exit 1"]
    Checker -->|OK + TypeEnv + TypeInfo + DefinitionInfo| Fork{"subcomando<br/>de fitz"}

    Fork -->|fitz check| OK["✓ sin errores de tipo"]

    Fork -->|fitz run<br/>fitz test<br/>fitz repl<br/>fitz dev| Eval["<b>evaluator.rs</b><br/>ejecutar AST<br/>(env.rs + value.rs<br/>+ cron_jobs.rs)"]
    Eval -->|rutas HTTP registradas| Http["<b>http.rs</b><br/>axum + tokio<br/>multi-thread"]
    Eval -->|rutas WS (@ws)| Ws["<b>http.rs + asyncapi.rs</b><br/>WsConn&lt;T&gt; + AsyncAPI 3.0"]
    Eval -->|jobs @cron| Cron["<b>cron_jobs.rs</b><br/>scheduler tokio::spawn<br/>+ persistencia DB opt-in"]
    Eval -->|sin rutas| StdOut["stdout / output / test report"]
    Http -->|/openapi.json + /docs| Openapi["<b>openapi.rs</b><br/>schema OpenAPI 3.1"]
    Http -->|/healthz + /readyz + /metrics| HealthMetrics["healthz/readyz<br/>+ Prometheus"]
    Http -->|spans + logs + métricas| Otel["<b>observability.rs + logging.rs</b><br/>OTLP exporter + structured JSON"]
    Http --> Server["servidor en host:port"]

    Fork -->|fitz build| Codegen["<b>codegen.rs</b><br/>AST + TypeEnv → Rust"]
    Codegen --> Project["Cargo project en<br/>target/fitz-build/&lt;stem&gt;/"]
    Project --> Cargo["cargo build --release"]
    Cargo --> Bin["binario nativo<br/>(adyacente al .fitz<br/>o en target/release/)"]
    Cargo -->|--bundle-python/--bundle-pip| Launcher["<b>launcher_template.rs<br/>+ pbs.rs</b><br/>CPython + pip embebidos"]

    Fork -->|fitz openapi| OpenApiCmd["<b>openapi.rs</b><br/>standalone schema"]
    Fork -->|fitz fmt| Fmt["<b>fmt.rs</b><br/>pretty-printer"]
    Fork -->|fitz lint| Lint["<b>lint.rs</b><br/>linter de patrones"]
    Fork -->|fitz test| Test["<b>testing.rs</b><br/>registry de @test fns"]
    Fork -->|fitz db &lt;sub&gt;| Db["<b>migrations.rs</b><br/>schema diff + introspect"]
    Db -->|diff/migrate/inspect| Driver["<b>db.rs</b><br/>driver Postgres puro<br/>(wire v3.0 + TLS + pool)"]
    Driver --> Postgres[("Postgres real")]
    Db -->|new/squash/stamp| FileSystem["archivos .sql/.fitz<br/>en migrations/"]
    Fork -->|fitz docker init/build| Docker["<b>docker.rs</b><br/>Dockerfile + compose<br/>autogenerados"]
    Fork -->|fitz deploy| Deploy["<b>deploy.rs</b><br/>thin wrapper sobre<br/>docker/compose"]
    Fork -->|fitz py-types<br/>fitz py-stubs| PyTools["<b>py_types.rs<br/>+ pyi_stub.rs</b><br/>SQLAlchemy → Fitz<br/>+ stubs .pyi"]
    Fork -->|fitz new<br/>fitz init<br/>fitz add/remove/update| Pm["<b>manifest.rs<br/>+ lockfile.rs<br/>+ git_dep.rs</b><br/>package manager"]
    Pm --> Toml["fitz.toml + fitz.lock"]

    classDef good fill:#dff5dd,stroke:#3a8a3a
    classDef bad fill:#fcdede,stroke:#a33
    classDef input fill:#e0e8ff,stroke:#446
    classDef external fill:#fff5dd,stroke:#aa8
    class Source input
    class OK,StdOut,Server,Bin,OpenApiCmd,Fmt,Lint,Test,FileSystem,Toml,Launcher,HealthMetrics,Otel,Docker,Deploy,Ws,Cron,PyTools good
    class Abort bad
    class Postgres external
```

ASCII fallback (mismo diagrama, sin colores, para terminales y editores
que no rendericen mermaid):

```
                              archivo .fitz
                                    │
                                    ▼
                              ┌──────────┐
                              │ lexer.rs │   tokenizar
                              └────┬─────┘
                                   ▼
                              Vec<Token>
                                   │
                                   ▼
                              ┌───────────┐
                              │ parser.rs │  precedencia + estructura
                              │  (ast.rs) │  (Program = Vec<Stmt>)
                              └────┬──────┘
                                   ▼
                                Program
                                   │
                                   ▼
                              ┌──────────┐
                              │ types.rs │  resolver + check
                              └────┬─────┘
                                   │ (TypeEnv + TypeInfo + DefinitionInfo)
                                   │
   ┌───────┬──────┬───────┬───────┬┴──────┬───────┬─────────┬─────────┬──────┐
   ▼       ▼      ▼       ▼       ▼       ▼       ▼         ▼         ▼      ▼
 check   run/   build  openapi  fmt     lint    test    db <sub>   docker  deploy
        test/         openapi  fmt.rs  lint.rs testing  migrations docker  deploy
        dev/           .rs                      .rs     .rs        .rs     .rs
        repl                                                                │
          │              │                                                  │
          ▼              ▼                                                  ▼
   evaluator.rs       codegen.rs                                    wrapper
   env.rs+value       Cargo project                                docker/
   cron_jobs.rs       │                                            compose
   testing.rs         ▼
        │         cargo build --release
        │             │
   ┌────┴───────────┐ ▼
   ▼    ▼   ▼      ┌──────────┐
  CLI  http  cron  │ binario  │ (+ launcher_template + pbs si
 stdout axum sched │ standalone│  --bundle-python/--bundle-pip)
       tokio  │   └──────────┘
       multi  │
       thread │
       │     │
       ▼     ▼
   servidor  jobs
   + /healthz/readyz/metrics
   + /openapi.json + /docs
   + /asyncapi.json (si hay @ws)
   + logs estructurados (logging.rs)
   + spans OTel (observability.rs)
       │
       ▼
   Postgres (db.rs driver puro: wire v3.0 + TLS + pool)
```

## Los flujos del CLI

El CLI ([main.rs](../src/main.rs)) tiene **33 sub-comandos**
(19 top-level + 10 sub-comandos `db ...` + 2 `docker ...` + 2
`deploy ...`) agrupados en **8 familias**. Todos comparten el
front-end (lexer → parser → checker) cuando trabajan con código
Fitz; después se bifurcan:

**Familia 1 — Pipeline core del lenguaje**:
- **`fitz run [archivo]`** — Chequea tipos en modo strict (flag
  `--no-typecheck` lo baja a warning) y ejecuta el AST con el
  evaluator. Sin args, busca `fitz.toml` y corre `[bin].main`.
  Si el programa registró rutas HTTP / WS / `@cron`, arranca el
  servidor / scheduler.
- **`fitz build [archivo]`** — Chequea strict (sin escape), genera
  Cargo project, invoca `cargo build --release`, copia binario.
  Sin args, manifest mode + output a `target/release/<pkg-name>`.
  Flags Fase 8.b/8.c: `--bundle-python` + `--bundle-pip <PACKAGE>`
  (repetible) + `--bundle-pip-requirements <FILE>` (repetible)
  para embeber CPython 3.14.x via python-build-standalone y
  paquetes pip preinstalados (binarios standalone que NO requieren
  Python en el destino).
- **`fitz check [archivo]`** — Lexea + parsea + chequea tipos.
  Reporta errores y termina (útil para editores / CI).
- **`fitz openapi <archivo>`** — Emite schema OpenAPI 3.1 a stdout
  (handlers HTTP descubiertos en el AST).

**Familia 2 — Package manager (Fase 9.y)**:
- **`fitz new <nombre>`** — Crea proyecto nuevo (`<nombre>/fitz.toml`
  + `src/main.fitz` + `git init`).
- **`fitz init`** — Convierte el cwd actual en un proyecto.
- **`fitz add <name> --path/--git`** — Suma dep al `fitz.toml` +
  sincroniza `fitz.lock`.
- **`fitz remove <name>`** — Quita dep + sincroniza lockfile.
- **`fitz update [name]`** — Re-resuelve deps (re-clone git deps).

**Familia 3 — DX (Fase 9.z)**:
- **`fitz fmt [archivos] [--check]`** — Pretty-printer cero config
  (preserva comments + blank lines del usuario).
- **`fitz test [filter] [--file]`** — Test runner built-in (descubre
  `@test` fns, output estilo cargo).
- **`fitz dev [--file]`** — Hot reload con file watcher (kill+respawn
  del child al cambio).
- **`fitz repl`** — REPL interactivo con env compartido entre líneas.
- **`fitz lint [archivos] [--deny <name>]`** — Linter de patrones
  (4 lints: unused_variable, unused_import, useless_match,
  string_concat).

**Familia 4 — Interop Python (Fase 8, feature `python`)**:
- **`fitz py-types <archivo.py>`** — Genera `type` Fitz desde modelos
  SQLAlchemy (introspección via duck typing sobre `__table__.columns`).
- **`fitz py-stubs <archivo.py>`** — Genera stubs `.pyi` desde Fitz.
- **Auto-pickup de stubs** (Fase 8-pyi.B, no es sub-comando): cuando
  el programa tiene `from python import foo`, el loader busca
  `<base_dir>/foo.pyi` adyacente y registra nominales declarados al
  `TypeEnv` antes del checker — el user puede escribir
  `let u: User = requests.fetch(...)?` con `User` declarado en
  `requests.pyi` y el checker lo resuelve.

**Familia 5 — DB / migrations / ORM (Fase 10 + Tier S v0.10.28-29)**:
- **`fitz db diff [--file] [--out] [--check-destructive] [--allow-destructive]`**
  — Compara schema declarado vs DB real y emite DDL. Clasifica cada
  change como Safe/Risky/Destructive (v0.10.31 Tier A.1).
- **`fitz db migrate [--dry-run] [--sql]`** — Aplica migrations
  pendientes contra la DB. Tracking idempotente via tabla
  `_fitz_migrations`.
- **`fitz db status`** — Lista migrations applied vs pending.
- **`fitz db new <name>`** — Crea archivo `.sql`/`.fitz` nuevo con
  timestamp prefix.
- **`fitz db rollback [--count N]`** — Revierte las últimas N
  migrations (usa el bloque `-- DOWN`).
- **`fitz db check [--file]`** — Valida consistencia schema +
  migrations sin tocar DB.
- **`fitz db history`** — Lista migrations applied con timestamps.
- **`fitz db squash <from> <to>`** — Combina migrations en una.
- **`fitz db stamp <version> | --all`** — Marca migration(s) como
  applied SIN ejecutar SQL (adopt legacy DB).
- **`fitz db inspect [--schema | --all-schemas | --table | --json]`**
  — Introspect del schema **real** con vista texto + JSON
  machine-readable.

**Familia 6 — Editor support (Fase 9.x, feature `lsp`)**:
- **`fitz-lsp`** (bin separado en `src/bin/fitz-lsp.rs`, no
  sub-comando) — Language server sobre tower-lsp. Consumido por la
  extensión VSCode (`editors/vscode/`).

**Familia 7 — Docker stack (Fase 12.4)**:
- **`fitz docker init [--force]`** — Genera `Dockerfile` multi-stage
  + `.dockerignore` + `docker-compose.yml` smart en el directorio
  del manifest. Detección AST-only del entry point para decidir
  `EXPOSE <port>` (si hay `@server(N)`) + service `postgres:16-alpine`
  con healthcheck (si hay `db.connect(...)`). Sub-paso 12.4.b suma
  fallback `python:3.12-slim-bookworm` cuando hay interop Python +
  `restart: unless-stopped` cuando hay `@cron`.
- **`fitz docker build [--tag <X>]`** — Thin wrapper sobre
  `docker build` que usa el Dockerfile del manifest_dir y tag-ea con
  `<package.name>:latest` (override con `--tag`).

**Familia 8 — Deploy orchestrator (Fase 12.6)**:
- **`fitz deploy docker [--tag <X>] [--no-push]`** — `docker build`
  + `docker push` con tag derivado del manifest. Targets MVP solo
  docker y compose; `fly`/`railway`/`k8s` quedan como deuda visible.
- **`fitz deploy compose [--no-detach] [--no-build]`** — Thin wrapper
  sobre `docker compose up -d --build`.

## Módulos en src/

### main.rs — entry point y CLI

Parsea argumentos con [clap](https://docs.rs/clap), enruta al
subcomando correspondiente, orquesta los pasos del pipeline. Cada
error termina con `exit(1)` y mensaje a `stderr`. Deliberadamente
delgado: no contiene lógica del lenguaje, solo coordinación. Los
sub-comandos `dev`, `repl`, `lint`, `fmt`, `test`, `add`, `remove`,
`update`, `new`, `init` tienen sus `*_cmd` adentro.

### lib.rs — crate library

Expone los módulos como `pub mod` para que los bins (`fitz` en
`src/main.rs`, `fitz-lsp` en `src/bin/fitz-lsp.rs`) los consuman vía
`use fitz::...`. Refactor introducido en Fase 9.x.1.b cuando aterrizó
el LSP — antes Fitz era bin-only. La lib también facilita tests
unitarios y futura integración con otras herramientas (por ejemplo,
un linter o formatter externo que reuse `parser` y `types`).

### lexer.rs — tokenización

Convierte el texto fuente en un `Vec<Token>`. Reconoce literales (Int,
Float, Str, Bool, Null), identificadores, palabras clave (`fn`, `if`,
`while`, `for`, `type`, `match`, `return`, `import`, `async`, `await`,
`as`, etc.), operadores, delimitadores, y decoradores (`@nombre`).
Maneja strings con interpolación a nivel léxico (detecta `{` adentro
del string); el ensamblado del `Expr::StrInterp` se completa en el
parser. Anota cada token con su posición (línea, columna).

Fase 9.z.1.b sumó **`Trivia` side-stream**: una segunda salida del
lexer con `Vec<Comment>` (line + block) y `Vec<usize>` (líneas blank).
La función `tokenize_with_trivia` la expone para el formatter; el
`tokenize` normal (parser/checker/eval) la ignora — overhead cero
para el resto.

### ast.rs — definición del AST

Tipos puros, sin lógica. Define `Program = Vec<Stmt>`, donde `Stmt`
cubre `FnDef`, `TypeDef`, `Assign`, `If`, `While`, `For`, `Loop`,
`Return`, `ReturnStatus` (HTTP status custom), `Import`, `FromImport`,
`Expr` (statement-expression), `Break`, `Continue`, `Error`
(recovery, Fase 9.0). `Expr` cubre literales, `Ident`, `BinOp`,
`UnaryOp`, `Call`, `FnExpr`, `Field`, `Index`, `Match`, `Try`
(`expr?`), `Await`, `Range`, `List`, `Map`, `StructLit`, `Ok`,
`Err`, `StrInterp`, `Error`. Aparte: `TypeExpr` (`Named`, `Generic`,
`Nullable`, `Function`) para anotaciones, `Pattern` para `match`, y
`Decorator` (`@get`/`@server`/`@middleware`/`@header`/`@test`) con
`args` + `kwargs`. `Expr` y `Stmt` llevan `span` para errores
posicionados (S1.2 + S1.codegen, post-5b).

### parser.rs — construcción del AST

Recursive descent con escalera de precedencia. Toma `Vec<Token>` y
devuelve `Result<Program, FitzError>`. Maneja todas las construcciones
del lenguaje, incluyendo anotaciones de tipo con generics anidados
(`Map<Str, List<Int>>`), sufijo nullable (`Str?`), tipo función
(`Fn(Int) -> Int`), patrones de `match`, paréntesis opcionales en
decoradores (post-9.z.2.a, necesario para `@test fn ...`), método
chain multi-línea (post-PreF8.2), aliases en imports (`from foo
import bar as b`).

Tiene también `parse_with_recovery` (Fase 9.0 / F15): variante
tolerante a errores que produce `(Program, Vec<FitzError>)` —
sintetiza nodos `Stmt::Error` / `Expr::Error` donde el parse falló y
sigue con el siguiente stmt. Usada por el LSP para que `did_change`
emita diagnostics sobre buffers en construcción.

### value.rs — valores en runtime

El enum `Value` que vive durante `fitz run`/`test`/`dev`/`repl`:
`Int`, `Float`, `Str`, `Bool`, `Null`, `List`, `Map`, `Instance`,
`Result`, `Function` (built-in o user), `Module`, `Type`, `Future`
(post-Fase 6), `HttpResponse`, `CorsConfig`, `PyObject` (feature
`python`).

`List`, `Map` e `Instance` están envueltos en
`Arc<parking_lot::Mutex<...>>` (alias `Shared<T>`) para modelar
semántica de referencia compartida y para que `Value` sea `Send` —
lo que destrabó F17.5 (eliminación del bridge HTTP) y el runtime
tokio multi-thread del server. Incluye `impl Display` que produce
el formato canónico (strings con comillas dobles adentro de
colecciones, `Float` con `.0`, etc.) que el codegen replica
bit-a-bit.

### env.rs — entornos / scopes

`Environment` con stack de scopes (`Arc<Mutex<...>>` después de F17).
Métodos `define`, `get` (busca recursivo subiendo a padres),
`assign` (sobreescribe en el scope donde fue definida), `has`,
`local_names` (para `:env` del REPL, sin recursar). Las closures
(`Value::Function { closure }`) capturan un handle del env de
definición; el evaluator agrega un child para los params al
invocar.

### types.rs — sistema de tipos y checker estático

Dos responsabilidades:

1. **Resolución**: convierte `TypeExpr` (sintáctico) a `Type`
   (resuelto, con identidad nominal). `Type` cubre primitivos,
   generics built-in (`List<T>`, `Map<K,V>`, `Result<T>`,
   `Nullable<T>`, `Future<T>`), `Nominal(TypeId)` para tipos custom,
   `Function { params, ret }`, `PyAny` (Fase 8.4) y `Any` como
   escape gradual.
2. **Checker**: `check_program(&Program)` recorre el AST con un
   `CheckCtx` (scopes, `return_stack` para `?` y `return`,
   `inferred_returns` para `FnExpr.ret`, `def_info` para `go-to-def`
   del LSP). Sintetiza tipos, valida llamadas (aridad + tipos),
   chequea exhaustividad de `match` sobre `Result`, valida métodos
   built-in con templates paramétricos. Devuelve `(TypeEnv,
   TypeInfo, DefinitionInfo, Vec<FitzError>)`.

`TypeInfo` (F16) registra el tipo sintetizado de cada `Expr`
indexado por span — consumido por el LSP `textDocument/hover` y
por `:type` del REPL. `DefinitionInfo` registra `(use_span,
def_span)` para `textDocument/definition`.

### evaluator.rs — ejecución del AST

El intérprete. Toma `Program` + `Environment` y produce efectos.
Maneja control flow (`return`, `break`, `continue`) con señales
internas. Resuelve `import` cargando archivo canonicalizado,
parseándolo recursivamente, con cache por path y detección de
ciclos por stack. Despacha métodos built-in (`xs.map`, `m.get`,
`s.upper`, etc.) por tipo del receptor. Decoradores HTTP
(`@get`/`@post`/etc.) registran rutas en el `HttpRegistry` activo;
`@test` registra en el `TestRegistry` activo (Fase 9.z.2);
`@server` configura `ServerConfig`.

Post-Fase 6.4 (Async nativo) las funciones del evaluator son
`async fn` y usan `#[async_recursion]` para que `eval_expr`/
`eval_stmt`/`eval_call` puedan recursarse a través del `.await`.
Post-F17 todas las firmas son `Send` — un solo runtime tokio
maneja tanto el evaluator como axum, los handlers HTTP corren
en paralelo entre workers.

APIs públicas que consumen los sub-comandos:
- `eval_with_base_and_deps` para `fitz run` y `fitz test`.
- `eval_program_with_env` (Fase 9.z.4) para el REPL: env
  compartido entre invocaciones + devuelve el `Value` del último
  stmt para pretty-print Python-style.
- `run_test_handler` (Fase 9.z.2) para invocar un `@test fn`
  (sync o async) y traducir el resultado a ok/FAILED.
- `build_runtime` para construir el tokio current_thread runtime
  compartido por todos los sub-comandos async.

### http.rs — runtime HTTP + WebSockets + healthz/metrics

Activa la capa HTTP nativa. Post-F17.5 ya no hay bridge
`mpsc/oneshot` ni `std::thread` separado para tokio — un solo
runtime tokio `rt-multi-thread` corre tanto el evaluator como
axum, y los handlers axum invocan al evaluator directamente sobre
`Arc<HttpRegistry>` compartido. Paralelismo real entre requests.

Componentes:
- `HttpRegistry`: tabla de rutas (`method`, `path_template`,
  `handler`, `RouteMeta`, `middlewares`, `cors`, `required_roles`,
  `is_ws`, `ws_msg_type`). Thread-local + `parking_lot::Mutex`
  para registry activo. Incluye `ws_broadcaster: Arc<WsBroadcaster>`
  para fan-out de WebSockets y `cron_registry: Arc<CronRegistry>`
  para que el lifecycle compartido entre HTTP server y scheduler
  cron reuse el mismo runtime tokio.
- `serve(registry, program, addr)`: construye `Arc<HttpRegistry>`
  + `axum::Router` + `TcpListener`, llama `axum::serve` con
  graceful shutdown (SIGTERM drain con 30s + readyz → 503).
  Precomputa el schema OpenAPI + AsyncAPI eager y los sirve en
  `/openapi.json` + Scalar UI en `/docs` + `/asyncapi.json`.
- `build_router(metas, registry, openapi_schema)`: arma el
  `Router` con un closure por ruta + CORS preflight + middleware
  chain. Auto-mount de `/healthz` + `/readyz` (Fase 12.1) y
  `/metrics` Prometheus (Fase 12.3.iter2.Tier3) con override si
  el user declara handlers homónimos.
- **WebSockets** (Fase 9.w.2): `build_ws_method_router` con
  `WebSocketUpgrade` extractor + auth pre-upgrade (401/403 sin
  abrir el socket) + heartbeat ping/pong opcional via
  `@server(ws_heartbeat_secs=N)`. `WsConn<T>` con métodos
  `recv`/`send`/`broadcast`/`close` marshaling JSON automático
  contra el `type T` declarado.
- **Auth nativa** (Fase 9.w.1): `dispatch_request` ejecuta
  `@auth_provider` antes de body parsing si la ruta tiene
  `@authenticated`/`@admin`/`@requires("role")`. Provider devuelve
  `Result<User>` → 401 si Err, 403 si admin/role no matchea.
- `MiddlewareKind::{Pre, Post}` + `MiddlewareSpec.kind`: mini-tanda
  Mw.next clasifica middlewares por aridad. Pre (1 arg) corre antes
  del handler con semántica gate-only; Post (2 args
  `(Request, Response)`) corre después del handler en reverse order.
- `parse_urlencoded_body`: parsea `application/x-www-form-urlencoded`
  body como `Value::Map<Str, Str>`. Content-Type 415 estricto.
- `value_to_json` / `json_to_value` / `json_to_instance`:
  traducen entre `Value` y `serde_json::Value`. Con schema (`type`
  declarado), `json_to_instance` valida campos / aplica defaults /
  rechaza extras (→ 400). `Value::Secret` se redacta a `"***"`.
- `parse_path_template` / `coerce_path_param`: extraen path params
  + query params (post-F7) tipados.
- `ServerConfig`: `@server(port, host, docs=Bool, api_version="X",
  observability=Bool, prometheus=Bool, ws_heartbeat_secs=N)`.
  Default `127.0.0.1:3000`.

### asyncapi.rs — generador AsyncAPI 3.0 (Fase 9.w.2.d)

Paralelo a `openapi.rs` pero para handlers `@ws("/path")`. Emite
channels + operations receive/send + securitySchemes.bearerAuth
cuando hay auth. `channels_from_registry` (runtime, consumido por
`serve`) y `pseudo_channels_from_ast` (build-time, consumido por
`codegen.rs` para embeber el schema bit-a-bit en el binario).
`BTreeMap` para orden determinístico. Sirve en `/asyncapi.json`
cuando hay handlers `@ws`.

### cron_jobs.rs — scheduler de jobs `@cron` + `@background` (Fase 9.w.3)

`CronJob` (handler + Schedule parseado + tz + retry + catch_up +
store opcional para persistencia) + `CronRegistry` (paralelo a
`HttpRegistry`, vive como `cron_registry: Arc<CronRegistry>`
adentro). `spawn_cron_scheduler` arranca un `tokio::spawn` por
job con loop `sleep_until(next_tick) → invoke_with_retry`.

Persistencia (Fase 9.w.3.iter2): cuando el job declara
`store=<db_binding>`, crea tablas `fitz_cron_jobs` + `fitz_cron_runs`
auto via `CREATE TABLE IF NOT EXISTS` al boot del scheduler;
persiste cada attempt con status `running`/`ok`/`retrying`/`failed`;
opt-in `catch_up=true` ejecuta UN run inmediato al boot si hubo
missed runs (no N — evita spam). Race condition de CREATE TABLE
serializada con `tokio::sync::OnceCell` global (`v0.15.13` fix +
paralelo en codegen v0.15.14).

`spawn(fn_call)` fire-and-forget integrado con el mismo modelo:
`tokio::spawn(invoke)` envuelve el JoinHandle en `Value::Future`,
permite `await` o discard.

### codegen.rs — transpile a Rust

Genera un Cargo project completo a partir del AST + `TypeEnv` +
`TypeInfo` (mini-tanda Hpx.2 — el codegen consulta el side-table
para inferir return types de fns sin anotar).
`generate_project(path, program, type_env, type_info, dep_registry)`
devuelve `Cargo.toml` + `src/main.rs` + módulos auxiliares.

**Helpers para inferencia** (mini-tandas Hpx.2 + 5b.1 + P2):
- `infer_return_type_from_body(body, type_info)`: walkea
  `Stmt::Return(e)` en el body, consulta `e.span()` en `TypeInfo`,
  unifica con `lub`.
- `infer_param_type_from_call_sites(program, fn_name, idx, type_info)`:
  scanea el programa por calls a `fn_name`, extrae el tipo del arg
  `idx`-ésimo.
- `has_unannotated_fn_params(program)` + `fill_inferred_param_types(program, type_info)`:
  el `build_file` usa estos para mutar el AST in-place tras la
  primera pasada del checker, llenando params inferidos. Se re-corre
  el checker para refinar `TypeInfo` y soportar la cadena 5b.1+Hpx.2
  (ambos param y return inferidos cuando el return depende del param).

**Mw.next codegen** (mini-tanda P1 + RP): `HandlerSig.mw_user_fns_post`
guarda nombres de post mws. `CodegenCtx.middleware_post_fn_names`
tracks fns marcadas como Post (clasificadas por aridad en post-scan
de `pre_register_fns`). `gen_top_fn` emite Post mws con return
`__FitzResponse` (no Option). `emit_handler_dispatch_and_response`
construye un `__FitzResponse` intermedio, corre la chain de Post mws
en reverse, y convierte a axum Response al final. Cubre handlers
con return plain T, returns_response (ReturnStatus), y Result<T>.

Mapping de tipos (post-F17.4b):
- `Int → i64`, `Float → f64`, `Str → String`, `Bool → bool`,
  `Null → ()`.
- `List<T> → Arc<Mutex<Vec<T>>>` (`std::sync::Mutex`, sin deps
  extras en el `Cargo.toml` generado).
- `Map<K,V> → Arc<Mutex<Vec<(K,V)>>>`.
- `Result<T> → Result<T, String>` (Err pinned a String).
- `type Foo { ... } → struct FooData { ... } + type Foo =
  Arc<Mutex<FooData>>`. `PartialEq` emitido manual (Mutex no impl
  PartialEq) con helper recursivo `field_eq_expr`.

State HTTP compartido pasa de `thread_local!` (pre-F17) a
`static X: LazyLock<Arc<Mutex<T>>>` + materialización
`(*X).clone()` en cada handler. Field access se emite como bloque
acotado `{ let __obj = ...; let __g = __obj.lock().unwrap();
__g.<f>.clone() }` para evitar deadlock por re-lock.

`fitz build` con HTTP emite `#[tokio::main]` default multi-thread
+ `axum::Router` + handler wrappers async. Cargo.toml condicional:
`axum`/`tokio`/`serde` solo si hay decoradores HTTP, `pyo3` solo
con feature `python`. La regla de oro: output del binario bit-a-bit
idéntico a `fitz run` para programas en el subset soportado.

Tiene su propio `ModuleLoader` que replica el del evaluator pero
AOT, y un guard `check_no_python_imports` que aborta builds con
`from python import` salvo que la feature `python` esté activa
(Fase 8.7 = deuda F19 cerrada — el binario lincado con PyO3 ahora
sí soporta interop).

### openapi.rs — generador OpenAPI 3.1

`generate_openapi(routes, program)` produce un `Value::Map` con la
estructura OpenAPI 3.1 (`openapi`, `info`, `paths`, `components`).
Recibe rutas de dos fuentes:
- `routes_from_registry`: rutas resueltas en runtime (consumido por
  `fitz run`).
- `pseudo_routes_from_ast`: rutas inferidas del AST sin evaluar
  (consumido por `fitz openapi` standalone + por el codegen para
  embeber el schema bit-a-bit en el binario).

Soporta status codes custom (post-F7), query params, headers via
`@header(name="X")`, opt-out con `@server(docs=false)`,
`api_version` configurable. Renderiza la UI Scalar via
`templates/scalar.html` cuando se sirve `/docs`.

### manifest.rs — `fitz.toml` del package manager (Fase 9.y.1)

Define `Manifest { package, bin, lib, dependencies }` + `Package`
+ `Bin` + `Lib` + `Dependency` (Detailed con `path`/`git`/`tag`/
`rev`) + `DepRegistry = HashMap<String, PathBuf>`. APIs:
- `find_manifest(start)`: walk-up cargo-style.
- `Manifest::parse(text)`: TOML → struct con `serde`.
- `resolve_dependencies(manifest, base_dir)`: resuelve cada dep a
  un `ResolvedDep` (path absoluto al `lib.entry`). Path deps
  resuelven inmediato; git deps delegan a `git_dep.rs`.
- `build_dep_registry(resolved)`: del `Vec<ResolvedDep>` al
  `DepRegistry` que consume el loader del evaluator.
- `write_manifest_with_edit` (Fase 9.y.4): preserva comments via
  `toml_edit` cuando `fitz add`/`remove` modifica el archivo.

Validación de nombre: `^[a-z][a-z0-9_-]{0,63}$` (crates.io style).

### lockfile.rs — `fitz.lock` (Fase 9.y.3.a)

`Lockfile { version: 1, packages: Vec<LockedPackage> }`. Cada
`LockedPackage` lleva `name`, `version`, y `source` opcional (para
git deps: `git+<url>#<commit>`). Path deps no llevan source.

`from_resolved(resolved)` construye el lockfile desde el resultado
de la resolución; `write_lockfile_if_changed(path, lock)` hace
short-circuit byte-a-byte para evitar tocar mtime cuando el
contenido no cambió. `fitz run`/`build`/`check` lo sincronizan
automático en cada invocación.

### git_dep.rs — git deps + cache (Fase 9.y.3.c)

Maneja `[dependencies] foo = { git = "<url>", tag = "v1" }`. Clona
a `<cache_dir>/git/<sanitized-url>@<ref>/` (default
`~/.fitz/cache/`, override con `FITZ_CACHE_DIR`). Estrategia split:
`--depth 1 --branch <tag>` para tags, full clone + checkout para
revs. Detección de commit hash exacto para el lockfile via
`git rev-parse HEAD`. Cache reuse sin re-clone automático;
`fitz update <name>` invalida el cache.

### db.rs — driver Postgres puro + ORM (Fase 10, ~3000+ LoC)

El módulo más grande del proyecto en LoC. Implementa **driver
Postgres completo en Fitz/Rust puro** (sin `tokio-postgres` /
`sqlx` / `diesel` / `libpq`):

- **Wire protocol v3.0**: `Connection::connect` hace handshake +
  SCRAM-SHA-256 auth (RFC 7677) + PBKDF2-HMAC-SHA-256 +
  StartupMessage. Simple Query y Extended Query con
  Parse/Bind/Describe/Execute/Sync. ErrorResponse parseado
  estructurado a `DbError::Server { severity, code, message }`
  con SQLSTATE codes nativos.
- **TLS strict** (v0.10.23): `sslmode=require` / `verify-ca` /
  `verify-full` via `rustls` (CA bundle del sistema o
  `sslrootcert=<path>` PEM custom). `disable` también soportado
  para dev local.
- **Pool de conexiones**: `DbPool` con `idle: Mutex<Vec<Connection>>`
  + `permits: Semaphore` (default 10, override
  `FITZ_DB_MAX_CONNS` v0.10.29 con clamp `[1, 200]`). Cache
  global `OnceLock<HashMap<URL, Arc<DbConnHandle>>>` — múltiples
  `db.connect(url)` con la misma URL devuelven el MISMO handle
  (evita el "connection pool leak" donde cada call creaba pool
  nuevo). Health check task background con `tokio::spawn` que
  cierra conns idle stale cada 30s.
- **Tipos PgValue**: 11 OIDs core (BOOL/INT/FLOAT/TEXT/BYTEA/
  DATE/TIME/UUID/JSON/JSONB/VOID) + 12 array OIDs (`int4[]`,
  `text[]`, `jsonb[]`, etc.). Marshaling bidireccional Fitz
  `Value` ↔ `PgValue` con respeto de NULL adentro de arrays
  (`List<Int?>` ↔ `int8[]` con `{a,NULL,c}` format).
- **Observabilidad** (v0.10.28-29): `FITZ_DB_LOG=1|verbose` env
  var opt-in. Mode `verbose` aplica **redaction de secrets**
  (v0.10.29) sobre params via heurística contextual sobre el
  SQL (`password`/`secret`/`token`/`api_key`/etc. → `<redacted>`).
- **Errores enriquecidos** (v0.10.29): `DbError::Server` Display
  ahora muestra `<severity> [<SQLSTATE>]: <msg>`. Las queries
  fallidas pasan por `enrich_db_error_with_context` que suma
  `[sql: <one-line truncado> params=[...]]` (con redaction).

El ORM (decoradores `@table`/`@primary`/`@column`/`@belongs_to`/
`@has_many`/`@has_one`/`@unique`/`@check_constraint`/`@index`)
NO vive en `db.rs` — los decoradores se procesan en `types.rs`
y poblan `TableMetadata`. El SQL builder de los read/write
methods vive en `evaluator.rs` (`translate_method_call_to_sql`
para `.where(...)` closures + métodos relacionados) y `codegen.rs`
(paridad para `fitz build`).

### migrations.rs — schema diff + introspect + DDL emit (Fase 10.6+)

Sistema de migrations automático paralelo a Alembic / Flyway /
TypeORM CLI, pero sin deps externas — todo Rust + el wire protocol
del driver.

- **`Schema`** = `Vec<Table>`; **`Table`** = `(name, columns,
  indexes, foreign_keys, composite_pk, check_constraints,
  schema, renamed_from)`. **`Index.using/where_clause`**
  + **`ForeignKey.references_schema`** (v0.10.29 — cross-schema
  FK transparente).
- **`schema_from_program(program, type_env)`** — Construye el
  "target" schema desde los `@table` types del AST. Resuelve
  TypeMetadata → Table con auto-naming (constraints
  `<table>_<col>_fkey`/`idx_<table>_<col>`/`chk_<table>_<idx>`),
  composite PK como `PRIMARY KEY (a, b)` table-level, FK
  qualified cross-schema.
- **`introspect_schema(conn)`** — Lee el "current" schema de la
  DB con queries contra `pg_catalog` (`pg_class`, `pg_attribute`,
  `pg_index`, `pg_constraint`, `pg_am`). Cubre columns +
  tipos + defaults + NOT NULL, indexes con WHERE clauses y
  method (gin/gist/etc.), FKs con ON DELETE.
- **`diff_schemas(current, target)`** — Devuelve `Vec<Change>`
  determinístico:
  - `CreateTable` / `DropTable` / `RenameTable`.
  - `AddColumn` / `DropColumn` / `RenameColumn` / `AlterColumnType`
    / `AlterColumnNullable` / `AlterColumnDefault`.
  - `CreateIndex` / `DropIndex` (v0.10.29: detecta cambios en
    `using`/`where_clause`/`unique`/`columns` cuando nombres
    matchean → `DROP + CREATE` para regenerar).
  - `AddForeignKey` / `DropForeignKey` (con schema qualifier
    cross-schema).
- **`changes_to_sql(changes)`** — Emit DDL ejecutable directo
  via `psql -f` o `db.exec`.
- **`format_inspection_text` / `format_inspection_json`** (v0.10.28)
  + **`format_inspection_*_all_schemas`** (v0.10.29) — Pretty-
  print del schema para `fitz db inspect`.
- **Tracking via tabla `_fitz_migrations`**: `applied_versions`,
  `record_applied`, `apply_pending_migrations`. Idempotente —
  re-correr `fitz db migrate` salta las ya aplicadas.

### testing.rs — testing built-in (Fase 9.z.2.a)

`TestRegistry { tests: Vec<TestSpec> }` con `TestSpec { name,
handler, is_async, span, source_file }`. Thread-local activo
seteado por `with_active_test_registry` (sync/async) durante el
discovery del runner.

`with_test_source` agrega un nivel: el loader de módulos lo setea
con el filename del módulo importado antes de eval, así los `@test`
declarados en módulos quedan etiquetados con su archivo real (no
con el del importer). `current_test_source` lo lee el branch
`@test` de `process_decorator` al construir el `TestSpec`.

### fmt.rs — formatter (Fase 9.z.1)

Pretty-printer cero config sobre el AST. `format_source(text)`
tokeniza con `tokenize_with_trivia` (Fase 9.z.1.b), parsea, recorre
el AST emitiendo cada nodo en su forma canónica (4 espacios indent,
comillas dobles, paréntesis obligatorios en condiciones, type defs
multi-línea, etc.), y intercala los comments del `Trivia` stream
en su posición original.

Reglas de blank lines: máximo 1 consecutiva (las múltiples se
colapsan), obligatoria entre `fn`/`type` top-level. Comments
normalizados (`//foo` → `// foo`); trailing comments con 2 espacios
de separación. Ver [docs/fmt-style.md](fmt-style.md) para la
referencia completa.

Deuda residual: bug conocido cuando un body de fn termina con
trailing comment seguido de otro bloque — inserta blank spurious
adentro del segundo body. Variante del caso edge ya documentado en
`fmt-style.md`. Tracked en `docs/deudas-post-5b.md`.

### lint.rs — linter (Fase 9.z.5)

Linter de patrones más allá de tipos. `lint_source(source,
program) -> Vec<LintFinding>` walkea el AST recolectando findings
+ aplica supresiones leyendo el source raw (busca
`// @allow(<lint>)` en la línea inmediatamente anterior).

4 lints implementados:
- `unused_variable`: `let x = ...` cuyo nombre no aparece en uses,
  skipea prefijo `_`.
- `unused_import`: `import X` / `from X import Y` con binding no
  referenciado.
- `useless_match`: `match expr { _ => body }` con un solo arm
  catch-all.
- `string_concat`: `BinOp Add` con ambos operandos `Str` literales.

Walkers `collect_uses_in_*` y `walk_exprs_in_stmt` recorren stmts
y exprs recursivos. Catálogo cerrado (sin plugins en el MVP).

### lsp.rs — Language Server (Fase 9.x, feature `lsp`)

Feature-gated detrás de `cargo build --features lsp`. Implementa
los handlers de tower-lsp:
- `textDocument/didOpen`/`didChange`/`didClose`: parsea con
  `parse_with_recovery`, corre `check_program`, emite
  `Diagnostic`s.
- `textDocument/hover`: lee el tipo del nodo bajo el cursor del
  `TypeInfo` (F16) y lo renderiza como markdown `fitz` block.
- `textDocument/definition`: lee `DefinitionInfo` y devuelve la
  `Location` del use → def_span.
- `textDocument/completion`: enumera símbolos del scope-level +
  fields/métodos after-dot.

Pipeline pure-function en `check_source_with_types(text) ->
(TypeEnv, TypeInfo, DefinitionInfo, Vec<FitzError>)`. El bin vive
en `src/bin/fitz-lsp.rs` (también feature-gated). El cliente VSCode
en `editors/vscode/` lo invoca como subprocess.

### py_interop.rs / py_types.rs / pyi_loader.rs / pyi_stub.rs — Interop Python (Fase 8, feature `python`)

Feature-gated detrás de `cargo build --features python`. Linkea
contra CPython 3.10+ via [PyO3](https://pyo3.rs) (`abi3-py310`,
`auto-initialize`).

`py_interop.rs` (Fase 8.1-8.6) expone `import_module`,
`get_attr`, `call`, `value_to_py`/`py_to_value` (Fase 8.2 marshaling
bidireccional de tipos compuestos). Las excepciones Python se
envuelven en `Value::Result(Err)` con formato
`"<ClassName>: <message>"` (Fase 8.3). Detección automática de
corutinas + bridge tokio↔asyncio via `tokio::spawn_blocking` +
`asyncio.run_until_complete` (Fase 8.6).

`py_types.rs` (Fase 8.5) implementa `fitz py-types`:
introspección de modelos SQLAlchemy via duck typing sobre
`__table__.columns`, emite `type` Fitz correspondientes.

`pyi_stub.rs` (v0.9.39) — Parser de stubs `.pyi` Python (PEP 484/561).
Scope MVP: top-level `def`/`class`/vars con anotaciones, type
expressions `int|str|float|bool|list[T]|dict[K,V]|Optional[T]`,
`Union[T, None]`, etc.

`pyi_loader.rs` (Fase 8-pyi.B, v0.9.57) — Auto-pickup de stubs
`.pyi` adyacentes al `.fitz` raíz. Cuando el programa contiene
`from python import foo`, el loader busca `<base_dir>/foo.pyi` y, si
existe, parsea con `pyi_stub::parse_stub` y registra los nominales
declarados en el `TypeEnv` ANTES del checker. Silent fallback: si
no existe, el binding sigue como `Type::PyAny` opaco.

### logging.rs — Structured logging built-in (Fase 12.3.a)

Implementa `log.info`/`log.warn`/`log.error`/`log.debug` con kwargs
heterogéneos (Int/Float/Str/Bool/Null/Secret/List/Map). Output JSON
flat a stderr (containers/CI/redirección) o pretty con ANSI colors
(TTY). Override explícito via `FITZ_LOG_FORMAT=json|pretty`. Filter
via `RUST_LOG` (default `info`). Redacción recursiva de
`Value::Secret` en List/Map. `SpanContext` (trace_id 32 hex /
span_id 16 hex) atravesando `tokio::task_local!` para correlación
multi-thread; los logs adentro de un handler HTTP heredan
automático el trace_id del request.

### observability.rs — OTLP exporter para spans HTTP (Fase 12.3.c)

Cuando `OTEL_EXPORTER_OTLP_ENDPOINT` está seteada, conecta los
spans que abre `dispatch_request` (12.3.b) a un backend OTel real
(Jaeger, Tempo, Honeycomb, Datadog). Sin esa var, `init_otel()` es
no-op silencioso — zero overhead. Env vars OTel-standard:
`OTEL_SERVICE_NAME` (default `fitz-app`),
`OTEL_TRACES_SAMPLER_ARG` (ratio `0.0..1.0`, default `1.0`).
Iter2.a (v0.12.1) derivó `trace_id`/`span_id` del span OTel
cuando está activo para correlación bit-a-bit Fitz↔OTel: el
mismo trace_id aparece en logs stderr y en el backend OTel.
Iter2.Tier3 (v0.12.2) sumó endpoint `/metrics` Prometheus via
`metrics-exporter-prometheus` con dual gate
`@server(prometheus=true)` (compile-time) o `FITZ_PROMETHEUS=1`
(runtime). Iter2.Tier2 (bridge métricas OTel) **bloqueado** por
release del crate `metrics-exporter-opentelemetry` compatible con
`opentelemetry_sdk 0.32` — Prometheus pull cubre el caso 90%
mientras tanto.

### docker.rs — `fitz docker init` + `fitz docker build` (Fase 12.4)

Sub-comando `fitz docker init` que genera 3 archivos en el
directorio del manifest: `Dockerfile` multi-stage (builder con
imagen oficial `ghcr.io/thegreekman76/fitz:<tag>` que invoca
`fitz build`, runtime `gcr.io/distroless/cc-debian12` o
`python:3.12-slim-bookworm` si hay interop Python),
`.dockerignore`, y `docker-compose.yml` smart. Detección AST-only
del entry point (`@server(N)` → `EXPOSE` + `ports:`,
`db.connect(...)` → service `postgres:16-alpine` + healthcheck,
`@cron` → `restart: unless-stopped`, `from python import` →
runtime swap). Política skip-por-default + `--force`. Bonus
`fitz docker build [--tag X]` thin wrapper sobre
`docker build -t <tag> .`.

### deploy.rs — `fitz deploy <target>` orchestrator (Fase 12.6)

Thin wrapper sobre `docker build/push` y `docker compose up`
según target. Targets MVP: `docker` (build + push con `--no-push`
opcional) y `compose` (up local con `--no-detach`/`--no-build`).
Aborta si falta Dockerfile/compose.yml (sugiere
`fitz docker init`). Targets `fly`/`railway`/`k8s` quedan como
deuda visible — para esos, correr los CLIs directo. Detección
AST-only del entry point para validar que el proyecto está listo.
NO toca codegen (no emite código Rust).

### launcher_template.rs — Launcher para `--bundle-python` (Fase 8.b)

Cuando `fitz build --bundle-python` está activo, el output NO es
el binario "real" directo, sino un **launcher** Rust standalone
que lleva embebidos:
1. El tarball PBS (CPython 3.14.x install_only_stripped) via
   `include_bytes!`.
2. El "real binary" (transpile estándar con feature `python`,
   linkea libpython).

En primer run, el launcher extrae todo a `$TMPDIR/fitz-py-<hash>/`,
setea `PYTHONHOME` + `LD_LIBRARY_PATH`/`DYLD_FALLBACK_LIBRARY_PATH`/
`PATH`, y `exec`/spawn-and-wait del real binary. Sentinel
`.extracted` marca completitud; runs subsecuentes reusan el dir.
Timing observado Windows 11 SSD: cold ~5 s, warm ~50 ms.

Sub-paso 8.c suma `--bundle-pip <PACKAGE>` (repetible) +
`--bundle-pip-requirements <FILE>`: pip packages se preinstalan
en venv temporal y se empaquetan en tarball secundario también
embebido. Cache key (FNV-1a hash sobre bundle_pip args +
requirements file contents) con sidecar `inputs_hash` para reuso.

### pbs.rs — python-build-standalone downloader (Fase 8.b)

Descarga el tarball `install_only_stripped` de la release pinned
(constante `PBS_RELEASE`, bump manual c/3-6 meses) para el triple
destino y lo guarda en `<cache>/pbs/<tarball-name>` (cache global
compartido entre builds, default `~/.fitz/cache/`, override con
`FITZ_CACHE_DIR`). Builds reproducibles (misma política que
`Cargo.lock`). CPython 3.14.x dentro del rango `abi3-py310`.

### cli.rs — CLI builder nativo (Fase 13)

`@command("name", desc="...")` sobre una fn declara un comando
CLI. El binario producido por `fitz build` parsea `std::env::args()`
y dispatcha al comando matcheante. Convención de params:
sin default → positional arg requerido (`mybin <name>`); con
default → flag opcional (`--name <value>`); Bool con default
`false` → flag bool (`--loud`). Help autogenerado con
`mybin --help` + `mybin <cmd> --help`. El intérprete `fitz run`
también puede correr CLI programs.

### format.rs — `FormatSpec` aplicado a un `Value` (mini-tanda Fm)

Implementa la semántica completa de format spec estilo Python
sobre `Value`: width, alignment, fill, sign, alternate form,
grouping (`,`/`_`), precision, type chars (`b`/`c`/`d`/`e`/`E`/
`f`/`F`/`g`/`G`/`o`/`s`/`x`/`X`/`%`). Entry point
`format_value_with_spec(value, spec) -> Result<String, String>`
con mensaje legible si tipo y spec son incompatibles. Consumido
por la sintaxis `"{x:>10.2f}"` en strings interpolados.

### error.rs — manejo de errores

`FitzError` común a todas las fases, con `kind`, `line`, `column`,
`message`, `hint` opcional. `ErrorKind` enum cubre errores de
lexer/parser/evaluator/checker. `impl Display` formatea con la
posición cuando es distinta de `0:0`. Cada fase del compilador
devuelve `Result<T, FitzError>` y `main.rs` decide cómo mostrarlos
según el sub-comando.

## Por qué este orden y no otro

**Separar lexer / parser / AST / checker / eval / codegen** es la
estructura clásica de un compilador, pero hay decisiones específicas
del proyecto que vale aclarar:

- **El checker corre antes del eval y antes del codegen, no como una
  capa opcional.** Modo strict por default en `fitz run` (y siempre en
  `fitz build`) atrapa errores temprano. La flag `--no-typecheck` está
  para diagnosticar bugs del checker, no para usuarios finales.

- **El evaluator usa el AST directamente, sin IR tipado.** Es más
  simple y suficientemente rápido para el caso de uso (servidor de
  desarrollo, scripts). El codegen también consume el AST + TypeEnv
  directamente. Cuando hizo falta info tipada por nodo (LSP hover,
  REPL `:type`), se sumó como **side-table indexado por span**
  (`TypeInfo`/`DefinitionInfo` de F16) en lugar de un IR formal —
  más barato.

- **`http.rs` está separado del evaluator** porque la interacción
  tokio/axum es lo suficientemente compleja para vivir aparte y
  porque permite que el evaluator no dependa de `tokio` para sus
  paths sync. Post-F17 ya no hay bridge entre intérprete y server;
  todo corre en un solo runtime tokio multi-thread. WebSockets
  (Fase 9.w.2) y cron jobs (Fase 9.w.3) viven en el MISMO registry
  + runtime, compartiendo el `Arc<HttpRegistry>` — un programa
  con HTTP + WS + cron arranca un único proceso con todos los
  subsistemas coordinados.

- **El codegen produce un Cargo project, no un solo `.rs` + invocación
  de `rustc`.** Los imports cross-archivo necesitan `mod`, los
  decoradores HTTP necesitan dependencias externas, y cargo cachea
  builds incrementales. Trade-off conocido: la primera compilación
  cuesta ~1-2 s extra vs `rustc` directo.

- **Paridad bit-a-bit `fitz run` ↔ `fitz build` es contrato del
  lenguaje, no implementación.** Cada feature (HTTP, WS, auth, ORM,
  cron, interop Python, observability) tiene su path tanto en
  `evaluator.rs` como en `codegen.rs`, y los tests E2E
  (`compile_e2e`) validan que el output coincide carácter por
  carácter. Las divergencias (ej. `fitz build` no soporta state
  compartido pre-F11) son errores de codegen explícitos con mensaje
  citando el sub-paso futuro, no silent fallbacks.

- **Features opcionales (`python`, `lsp`) son cargo features**, no
  binarios separados ni crates aparte. La razón: el cuerpo del
  evaluator y el codegen mantienen branches `#[cfg(feature =
  "python")]` para registrar/serializar `Value::PyObject`. Splitting
  a crates separadas implicaría re-exponer demasiada API interna. Sí
  hay un bin separado para `fitz-lsp` (`required-features = ["lsp"]`)
  para que `cargo build` default sin features no arrastre tower-lsp.

- **`--bundle-python` produce un launcher, no el binario directo**
  ([launcher_template.rs](../src/launcher_template.rs)). Embeber
  CPython en un binario standalone que no requiere Python en el
  destino exige extraer el runtime a un dir cache + setear env vars
  antes del exec — eso vive en un thin Rust binary aparte que
  envuelve el binario "real" emitido por el codegen estándar. El
  pattern es el mismo que Datasette usa para distribuir aplicaciones
  Python como CLIs portables.

- **Observability es opt-in por env vars, no por flag del lenguaje.**
  `OTEL_EXPORTER_OTLP_ENDPOINT` ausente → `init_otel()` es no-op
  silencioso. `RUST_LOG` ausente → logs solo a nivel `info`+.
  `FITZ_PROMETHEUS=1` o `@server(prometheus=true)` activan
  `/metrics`. Trade-off: opt-out de access logs HTTP automático
  requiere `@server(observability=false)` explícito (deuda residual
  ABIERTA: gating de deps OTel emitidas cuando el programa no las
  usa — hoy se linkean siempre que hay handlers HTTP).

- **Package manager, DX, Docker stack, deploy y CLI builder son
  módulos hermanos, no capas del compilador**. `manifest.rs`,
  `lockfile.rs`, `git_dep.rs`, `fmt.rs`, `testing.rs`, `lint.rs`,
  `docker.rs`, `deploy.rs`, `cli.rs` operan sobre el filesystem y
  el AST, pero no son parte del pipeline core lexer→parser→checker→
  eval/codegen. `main.rs` los invoca según sub-comando; no hay
  acoplamiento profundo entre ellos.

- **El driver Postgres es puro Rust, sin libpq.** [db.rs](../src/db.rs)
  implementa el wire protocol v3.0 + SCRAM-SHA-256 + TLS (rustls) +
  pool de conexiones desde cero, sin `tokio-postgres`/`sqlx`/`diesel`.
  Razón: control completo del marshaling Fitz `Value` ↔ `PgValue`
  (incluyendo arrays con NULL adentro, JSONB libre vs `Map<Str, T>`
  concreto, Date/DateTime/Uuid como tipos nativos), y promesa "cero
  deps externas para features intrínsecas". El ORM declarativo
  (`@table`/`@primary`/`@column`/`@belongs_to`/`@has_many`) vive
  encima — los decoradores se procesan en `types.rs` poblando
  `TableMetadata`, el SQL builder en `evaluator.rs` (`.where`
  closures translateadas a SQL parametrizado) + `codegen.rs`
  (paridad). [migrations.rs](../src/migrations.rs) consume el
  driver para introspect/diff/migrate.
