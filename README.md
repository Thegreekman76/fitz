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
// Ejemplo aspiracional (sintaxis del syntax-spec). `async fn` y
// `.await` ya están implementados (Fase 6 cerrada); falta sólo
// el driver de DB `db.find(id).await`.
@get("/users/{id}")
async fn get_user(id: Int) -> Str {
    let _ = sleep(0).await
    return "user #{id}"
}
```

Para ver un ejemplo **que corre hoy end-to-end con `fitz run`**,
mirá [`examples/server.fitz`](examples/server.fitz) — un CRUD
completo con `Result + ?`, body JSON y `@server(...)`. Para un
ejemplo **compilado a binario nativo con `fitz build`**, mirá
[`examples/guide/20-build.fitz`](examples/guide/20-build.fitz) —
server HTTP sin state compartido, compilable end-to-end. Para
async, [`examples/guide/19-async.fitz`](examples/guide/19-async.fitz).
Para **docs autogeneradas** (OpenAPI 3.1 + UI Scalar en `/docs`),
[`examples/guide/18-docs.fitz`](examples/guide/18-docs.fitz).

## Por qué Fitz

Los lenguajes actuales te obligan a elegir entre ergonomía y performance:

- **Python** — hermoso, pero lento. Deployar es un dolor.
- **TypeScript** — tipado opcional de mentira, arrastra el bagaje de JS.
- **Go** — compilado y rápido, pero sintaxis verborrágica.
- **Rust** — perfecto por dentro, demasiado complejo para APIs.

**Fitz toma lo mejor de cada uno:**

| Feature                | Python | TypeScript | Go | Fitz  |
| ---------------------- | ------ | ---------- | -- | ----- |
| Sintaxis limpia        | ✅     | ⚠️       | ❌ | ✅    |
| Tipado gradual         | ❌     | ✅         | ❌ | ✅ *  |
| Compilado nativo       | ❌     | ❌         | ✅ | ✅ † |
| HTTP en el core        | ❌     | ❌         | ❌ | ✅    |
| Async nativo           | ⚠️   | ✅         | ✅ | ✅ ‡ |
| Docs HTTP automáticas | ⚠️   | ❌         | ❌ | ✅ ◊ |
| **Auth nativa**        | ❌     | ❌         | ❌ | ✅ ♦ |
| **WebSockets tipados** | ⚠️   | ⚠️       | ⚠️ | ✅ ♣ |
| **Jobs sin Celery**    | ⚠️   | ⚠️       | ⚠️ | ✅ ♠ |
| Interop Python         | ✅     | ❌         | ❌ | ✅ § |

\* **Tipado gradual con chequeo estático** — Fase 5a completada.
`fitz check` y `fitz run` validan anotaciones en compile time;
sin anotación, se infiere o se trata como `Any`.

† **Compilado nativo** — Fase 5b completada. Backend:
transpile-a-Rust + Cargo. `fitz build` compila primitivos,
tipos custom, listas/mapas, `Result`/`?`/`match`, módulos,
HTTP y async a binario standalone. Ver
[cap 20 de la guía](docs/guide.md#20-fitz-build--compilar-a-binario-nativo)
para el detalle del subset soportado y de la deuda residual.

‡ **Async nativo + paralelismo HTTP real** — Fase 6 + F17
completadas. `async fn` y `.await` postfix reales en el
lenguaje. `Future<T>` como tipo built-in, builtin `sleep`,
evaluator async sobre tokio multi-thread, handlers HTTP async,
codegen `async fn` Rust. El server HTTP corre N workers en
paralelo (sin bridge `mpsc/oneshot`): 5 requests concurrentes a
un handler `sleep(1000)` responden en ~1.2s, no en ~5s. Ver
[cap 19 de la guía](docs/guide.md#19-async-y-concurrencia) y
el ejemplo
[`examples/guide/19b-paralelismo.fitz`](examples/guide/19b-paralelismo.fitz).

◊ **Docs HTTP automáticas** — Fase 7 completada. OpenAPI 3.1
autogenerado desde los decoradores (path/query/body/headers
y `Result<T>` en return), UI Scalar embebida en `/docs`,
`@header(name="X")` para headers como params, opt-out con
`@server(docs=false)`. Schema bit-a-bit idéntico entre `fitz run`, `fitz openapi archivo.fitz` y `fitz build`. Ver
[cap 18 de la guía](docs/guide.md#18-docs-automáticas).

♦ **Auth nativa** — Fase 9.w.1 completada. Tres decoradores —
`@auth_provider` (singleton que valida headers y devuelve un
`User`), `@authenticated` (handler protegido por bearer JWT con
401 automático), `@admin` (shorthand de auth + check
`user.role == "admin"` con 403 automático) — más dos módulos
built-in **`jwt`** (encode/decode HS256/384/512) y **`hash`**
(Argon2id password hashing, recomendación OWASP). El checker
valida en compile-time que cada handler protegido tenga el
provider registrado y reciba el `User` correcto. El esquema
OpenAPI 3.1 auto-agrega `securitySchemes.bearerAuth` +
`security` por handler + 401/403 en responses. Paridad
bit-a-bit `fitz run` ↔ `fitz build`. **Cero `cargo add` /
`pip install` / `npm install`** — todo viene en el binario
`fitz`. Vs FastAPI (5 deps + middleware manual), Spring AOP
(reflection en runtime), ASP.NET `[Authorize]` (framework +
reflection), Fitz es el único lenguaje donde auth + JWT +
password hashing son ciudadanos de primera clase del compilador.
Ver [cap 28 de la guía](docs/guide.md#28-auth-nativa) y el
ejemplo completo
[`examples/guide/28-auth.fitz`](examples/guide/28-auth.fitz)
(login + /me + /admin con JWT real, < 100 líneas).

♣ **WebSockets tipados** — Fase 9.w.2 completada. `@ws("/path")`
sobre `async fn` + `WsConn<T>` con métodos
`recv`/`send`/`broadcast`/`close`. Cinco diferenciales que
vuelven a Fitz único en este espacio: **(1) marshaling JSON
automático** — declarás `WsConn<ChatMsg>` y cada frame text se
serializa/deserializa al `type` declarado sin un `json.loads` +
Pydantic / `JSON.parse` + Zod manual; **(2) AsyncAPI 3.0
auto-generado** en `/asyncapi.json` del código fuente (la spec
hermana de OpenAPI 3.1 para event-driven APIs), consumible por
tooling estándar (AsyncAPI Studio, generadores de clientes para
JS/TS/Python/Java); **(3) heartbeat built-in** con
`@server(ws_heartbeat_secs=N)` — Ping frames automáticos que
pasan de largo Nginx (60s default idle), Cloudflare (~100s) y
AWS ALB (60s); **(4) auth integrada** en el handshake
(`@authenticated`/`@admin` apilados sobre `@ws` validan el
bearer token ANTES del HTTP upgrade, devolviendo 401/403 sin
abrir el socket); **(5) codegen con paridad** — el flow WS
funciona idéntico en `fitz run` y en el binario nativo de
`fitz build`. **Ningún otro lenguaje hoy combina WS tipados con
AsyncAPI auto-generado del código fuente, heartbeat built-in y
auth integrada en el handshake**. FastAPI WebSocket te da
Pydantic y schema manual; Socket.IO te da eventos sin schema;
Phoenix Channels te da pattern matching tipado pero solo en
Elixir; SignalR te da proxies tipados solo en C# y solo en .NET.
Cero `cargo add tokio-tungstenite` o `pip install websockets`.
Ver [cap 29 de la guía](docs/guide.md#29-websockets-tipados) y
el ejemplo completo
[`examples/guide/29-ws.fitz`](examples/guide/29-ws.fitz)
(servidor de chat con login HTTP + JWT + broadcast multi-client
+ heartbeat configurado, < 100 líneas).

♠ **Jobs sin Celery** — Fase 9.w.3 completada. Tres piezas
nativas del lenguaje: **`@cron("expr")`** para tareas periódicas
(5/6/7 fields, cron Unix clásico + seconds + year), **`@background`**
como marcador opt-in para autorizar el callsite, y
**`spawn(fn_call)`** fire-and-forget que devuelve `Future<T>`
tipado. **Sin broker externo** (Redis/RabbitMQ no son requisito);
los jobs viven en memoria del proceso, suficiente para 90% de
servicios reales (tareas de mantenimiento, scripts periódicos,
fire-and-forget de notificaciones). El **checker estático** valida
en compile-time que `spawn(...)` apunte a una fn `@background` y
refina el ret type a `Future<T>` con T concreto (vs `tokio::spawn`
sin marcador, `asyncio.create_task` sin tipos, Celery con string-
based task names). **Cron-only mode** (programas sin `@server` ni
handlers HTTP) quedan vivos bloqueantes con `signal::ctrl_c`
automático — modo systemd-friendly drop-in. **Paridad bit-a-bit
`fitz run` ↔ `fitz build`** con `cron = "0.12"` + `chrono = "0.4"`
linkeados condicionalmente en el binario. **Cero `pip install
celery` / `cargo add tokio-cron-scheduler` / docker-compose con
Redis**. Vs Celery+Redis (Python con broker externo, lib opcional),
Bull/BullMQ (Node con Redis), Spring `@Scheduled` (reflection en
runtime), Fitz es el único lenguaje donde **cron + background
workers + spawn tipado** son ciudadanos de primera clase del
compilador, sin broker externo, con paridad intérprete↔binario.
Ver [cap 30 de la guía](docs/guide.md#30-jobs-sin-celery) y el
ejemplo completo
[`examples/guide/30-cron-background.fitz`](examples/guide/30-cron-background.fitz)
(URL shortener con HTTP + cron stats + spawn fire-and-forget de
tracking de clicks, < 100 líneas).

§ **Interop Python via PyO3** — Fase 8 cerrada al 100% del roadmap
original. Embedding básico de CPython (8.1), marshaling bidireccional
`List`/`Map`/`Instance` ↔ `list`/`dict` (8.2), excepciones Python →
`Result<T>` (8.3), tipos del checker + coerción runtime (8.4),
`fitz py-types` auto-mapeo SQLAlchemy → `type` Fitz (8.5), bridge
tokio ↔ asyncio (8.6), codegen interop en `fitz build` (8.7), guía +
ejemplo CRUD (8.8). Opt-in con la feature `python` al build del
binario `fitz`. **Deuda residual derivada** (no bloquea uso real
end-to-end, sí refinamientos): coerción `PyAny → List<T>/Map<K,V>/
Instance` en `fitz build` (helpers emitidos en el preludio; falta
wiring en `coerce` — anotaciones nominales sobre dicts ya andan en
intérprete vía 8.4, sólo el path codegen tiene este gap); `.await`
con binding intermedio split (`let f = py_call()?; f.await` — hoy
sólo el patrón canónico `<py_call>?.await`); stubs `.pyi` parseados
(pospuesto post-9); bundling CPython embebido (`fitz build
--bundle-python` para no requerir Python instalado en el destino —
sub-paso futuro separado, decisión python-build-standalone vs
PyOxidizer pendiente). Ver
[cap 21 de la guía](docs/guide.md#21-interop-python) y detalle
en "Estado del proyecto" abajo.

## Ejemplo aspiracional

Esto es lo que Fitz va a ser. Lo que **corre hoy** está abajo en
"Qué funciona hoy" y en [`examples/`](examples/).

```fitz
// main.fitz — un servicio completo, un archivo, cero dependencias

type User {
    id: Int
    name: Str
    email: Str?
}

@get("/")
async fn index() -> Str {
    return "Fitz corriendo 🏔️"
}

@get("/users/{id}")
async fn get_user(id: Int) -> User {
    let user = db.find(id).await
    match user {
        Ok(u)  => return u
        Err(e) => return 404 { message: e }
    }
}
```

Hoy mismo, todo lo de arriba funciona — incluyendo `async fn` con
`.await` (cap 19 de la guía) y status codes custom
(`return 404 { ... }`, cap 17). Lo único que falta es el driver
de DB:

```bash
fitz run examples/server.fitz
# Servidor en http://127.0.0.1:3000 (CRUD completo)
# Además: /openapi.json (schema 3.1) y /docs (UI Scalar) gratis.
```

Un server HTTP **compilado a binario nativo**:

```bash
fitz build examples/guide/20-build.fitz
./examples/guide/20-build      # Linux/macOS
# o:
.\examples\guide\20-build.exe  # Windows
```

Y un programa CLI con `async fn` + `.await`:

```bash
fitz build examples/guide/19-async.fitz
./examples/guide/19-async      # Linux/macOS
```

## Estado del proyecto

🏔️ **Fase 8 (Interop Python) cerrada entera — el roadmap original
está cumplido al 100%.** Fitz puede importar módulos Python,
llamar funciones, marshalar tipos en ambas direcciones, manejar
excepciones como `Result<T>`, generar `type` Fitz desde modelos
SQLAlchemy, `await` corutinas, y compilar todo a binario nativo
con pyo3 linkeado:

```fitz
from python import math
from python import json
from python import asyncio

type User { id: Int, name: Str }

// Coerción primitiva con anotación destino.
let pi: Float = math.pi  // 3.141592653589793

// Call con args + Result wrap. Excepciones Python → Err.
match math.sqrt(16.0) { Ok(v) => print(v), Err(_) => print("err") }

// Marshaling Instance Fitz → Python dict.
let u = User { id: 1, name: "Ada" }
match json.dumps(u) { Ok(s) => print(s), Err(_) => print("err") }
// → {"id": 1, "name": "Ada"}

// Recuperar tipos Fitz desde Python con anotaciones.
fn parse_user(s: Str) -> Result<User> {
    let row: User = json.loads(s)?
    return Ok(row)
}

// Bridge async con patrón canónico `?.await`.
async fn run() -> Result<Str> {
    let _ = asyncio.sleep(0.001)?.await
    return Ok("done")
}
```

El binario `fitz build` con interop linkea pyo3 con `abi3-py310 + auto-initialize` y asume Python instalado en el destino.
Paridad bit-a-bit `fitz run` ↔ `fitz build` validada en los
ejemplos. Programas SIN interop Python siguen produciendo
binarios libres como Fase 5b (pyo3 solo se incluye cuando
`uses_python = true`).

La guía del lenguaje gana un capítulo dedicado (cap 21 "Interop
Python") con 12 sub-secciones cubriendo setup, sintaxis, marshaling,
coerciones, `fitz py-types`, async, `fitz build`, y limitaciones
honestas. El ejemplo CRUD completo en
[`examples/guide/21-python-crud/`](examples/guide/21-python-crud/)
combina SQLAlchemy + SQLite + HTTP nativo Fitz + tipos:

```bash
pip install sqlalchemy
PYTHONPATH=examples/guide/21-python-crud \
  cargo run --features python -- run examples/guide/21-python-crud/app.fitz
# luego: curl http://localhost:3000/users
```

**Sub-paso separado pendiente** (no parte del roadmap original):
bundling CPython embebido con `fitz build --bundle-python` para
producir un binario standalone que NO requiera Python en el
destino. Decisión de herramienta pendiente (python-build-standalone
vs PyOxidizer).

**Fase 9.0 (pre-reqs habilitantes del LSP) CERRADA** — los dos
sub-pasos cerrados el 2026-05-15. **F15 (error recovery del parser)**
introduce `parse_with_recovery(tokens) -> (Program, Vec<FitzError>)`
para tooling externo que necesita un AST parcial sobre buffers en
construcción. **F16 (IR tipado persistido por nodo)** suma un
side-table `TypeInfo` que retiene el tipo sintetizado de cada
nodo `Expr`, accesible vía la nueva firma de `check_program`.
**Sin cambio user-facing**: `fitz run` / `fitz build` / `fitz check`
siguen usando `parse()` strict y descartando el side-table. Próximo
norte: las sub-fases visibles del LSP (9.x.1 diagnostics → 9.x.5
distribución VSCode Marketplace). Ver el
[roadmap](docs/roadmap.md) para el plan completo.

**Fase 9.w.1 (Auth nativa) CERRADA** — el MVP del stack web
first-class arrancó por auth. **Tres decoradores nuevos del
lenguaje** (`@auth_provider` singleton, `@authenticated`,
`@admin`) + **dos módulos built-in** (`jwt` con HS256/384/512,
`hash` con Argon2id) construyen un flujo de login + JWT + password
hashing entero sin dependencias externas. El **checker estático**
valida que cada handler protegido reciba un `User` del tipo
correcto en compile-time (no reflection en runtime como Spring/
ASP.NET). El **schema OpenAPI 3.1** auto-agrega
`securitySchemes.bearerAuth` + `security` por handler + 401/403
en responses — sin tocar el spec a mano. Paridad bit-a-bit
`fitz run` ↔ `fitz build`. El ejemplo completo
[`examples/guide/28-auth.fitz`](examples/guide/28-auth.fitz)
arma `POST /login` + `GET /me` (`@authenticated`) + `GET
/admin/users` (`@admin`) en menos de 100 líneas, validado
end-to-end con curl contra el binario nativo. Ver
[cap 28 de la guía](docs/guide.md#28-auth-nativa).

**Fase 9.w.2 (WebSockets tipados) CERRADA** — el segundo
sub-paso del stack web first-class. `@ws("/path")` sobre
`async fn` + `WsConn<T>` con métodos
`recv`/`send`/`broadcast`/`close`. Cinco diferenciales que
vuelven a Fitz único en este espacio: **marshaling JSON
automático** (cada frame text se serializa/deserializa al
`type` declarado, sin `json.loads` + Pydantic / `JSON.parse` +
Zod manual), **AsyncAPI 3.0 auto-generado** en
`/asyncapi.json` (la spec hermana de OpenAPI 3.1 para
event-driven APIs, consumible por AsyncAPI Studio +
generadores de clientes), **heartbeat built-in** con
`@server(ws_heartbeat_secs=N)` (Ping frames automáticos que
pasan de largo Nginx 60s default idle / Cloudflare ~100s /
AWS ALB 60s), **auth integrada** en el handshake
(`@authenticated`/`@admin` apilados sobre `@ws` validan el
bearer token ANTES del HTTP upgrade, devolviendo 401/403 sin
abrir el socket) y **codegen con paridad** bit-a-bit `fitz
run` ↔ `fitz build`. **Ningún otro lenguaje hoy combina WS
tipados con AsyncAPI auto-generado del código fuente,
heartbeat built-in y auth integrada en el handshake** —
FastAPI WebSocket te da Pydantic y schema manual; Socket.IO te
da eventos sin schema; Phoenix Channels te da pattern matching
tipado pero solo en Elixir; SignalR te da proxies tipados solo
en C# y solo en .NET. Cero `cargo add tokio-tungstenite` o
`pip install websockets`. El ejemplo completo
[`examples/guide/29-ws.fitz`](examples/guide/29-ws.fitz)
arma un servidor de chat con login HTTP + JWT + broadcast
multi-client + heartbeat configurado, en menos de 100 líneas,
validado end-to-end (incluido el binario nativo de `fitz
build`). Ver [cap 29 de la guía](docs/guide.md#29-websockets-tipados).

**Fase 9.w.3 (Jobs sin Celery) CERRADA** — el tercer sub-paso del
stack web first-class. Tres piezas nativas del lenguaje:
**`@cron("expr")`** (5/6/7 fields, cron Unix clásico),
**`@background`** como marcador opt-in para spawn, y
**`spawn(fn_call)`** fire-and-forget tipado. **Sin broker externo**
— los jobs viven en memoria del proceso, suficiente para tareas
de mantenimiento + scripts periódicos + fire-and-forget de
notificaciones (90% de servicios reales). El **checker estático**
valida en compile-time que `spawn(...)` apunte a una fn
`@background` y refina el ret type a `Future<T>` con T concreto
(vs `tokio::spawn` sin marcador, `asyncio.create_task` sin tipos,
Celery con string-based task names). **Cron-only mode**
(programas sin `@server`) quedan vivos bloqueantes con
`signal::ctrl_c` automático — modo systemd-friendly drop-in.
**Paridad bit-a-bit `fitz run` ↔ `fitz build`** con `cron`/
`chrono` linkeados condicionalmente en el binario. El ejemplo
completo
[`examples/guide/30-cron-background.fitz`](examples/guide/30-cron-background.fitz)
arma un URL shortener con HTTP + cron stats + spawn fire-and-
forget de tracking de clicks en menos de 100 líneas, validado
end-to-end (incluido el binario nativo). **Ningún otro lenguaje
combina cron + background workers + spawn tipado en el core sin
broker externo y con paridad intérprete↔binario**. Ver
[cap 30 de la guía](docs/guide.md#30-jobs-sin-celery).

**Plan LSP entero (Fase 9.x.1 → 9.x.5) CERRADO — 2026-05-15/16** —
las cinco sub-fases del LSP MVP. Habilitan la experiencia
"escribir Fitz en VSCode con errores subrayados al tipear" + "pasá
el mouse y ve qué tipo tiene" + "F12 sobre un nombre te lleva a su
declaración" + "autocomplete contextual con tipos" + **distribución
multi-platform con binario bundleado en el .vsix**. Tres componentes
coordinados:
**bin nuevo `fitz-lsp`** (opt-in con `--features lsp`,
`cargo build --release --features lsp`) que implementa el protocolo
LSP estándar (JSON-RPC sobre stdio, tower-lsp 0.20); **módulo
`fitz::lsp`** en la lib que expone el pipeline LSP-style
(`parse_with_recovery + check_program`) y el helper `FitzError → Diagnostic`; **extensión VSCode** en
[`editors/vscode/`](editors/vscode/) con grammar TextMate y cliente
LSP que spawnea `fitz-lsp` (configurable via setting `fitz.lspPath`).
La publicación real al Marketplace queda como acción del autor
(requiere cuenta de publisher + decisión sobre hacer el repo
público), no commit técnico. Ver
[cap 22 de la guía](docs/guide.md#22-soporte-para-editores) para
instalación (bundled vs manual) + settings.

Las fases cerradas:

- **Fase 2 — Intérprete base**: lexer, parser, AST, evaluador con
  funciones, closures, control de flujo, manejo unificado de errores.
- **Fase 3 — El lenguaje crece**: listas/mapas/rangos con `for ... in`,
  tipos custom (`type`) instanciables con field access y mutación,
  `Result` + `Ok`/`Err` + `?`, funciones anónimas + method calls,
  módulos / `import` / `from import`.
- **Fase 4 — HTTP nativo**: `@get`/`@post`/`@put`/`@delete` en el
  lenguaje, path params tipados, body JSON deserializado contra
  `type`, `@server(port, host)` configurable, serialización JSON
  automática (incluyendo `Result` auto-handling: `Ok(v)`→200,
  `Err(e)`→500).
- **Fase 5a — Type checker estático**: `fitz check` valida
  anotaciones, llamadas, returns, operador `?`, exhaustividad de
  `match` sobre `Result`, métodos built-in paramétricos, índices,
  FnExpr.ret inferido. `fitz run` aborta en modo strict por
  default; `--no-typecheck` lo salta.
- **Fase 5b — Codegen a binario nativo**: `fitz build` compila a
  un Cargo project + invoca `cargo build --release` para producir
  un ejecutable standalone. Subset: primitivos, control de flujo,
  tipos custom (con defaults/nullables/igualdad/aliasing), listas
  y mapas homogéneos, `Result`/`?`/`match` exhaustivo, módulos
  (`import`/`from import`), y HTTP nativo (`@get`/`@post`/`@put`/
  `@delete` + `@server` + path params + body JSON contra `type`
  custom). El binario producido es ~5 MB y no necesita Fitz ni
  Rust instalados en la máquina destino.
- **Fase 6 — Async nativo**: `async fn`, `.await` postfix,
  `Future<T>` como tipo built-in, builtin `sleep(ms)`, evaluator
  async sobre tokio `current_thread`, handlers HTTP async y
  codegen `async fn` Rust + `tokio::time::sleep` para `fitz build`. Cumple la promesa de "HTTP nativo" a nivel de ejecución.
- **Fase 7 — DX HTTP**: schema OpenAPI 3.1 autogenerado desde
  los decoradores (path/query/body/headers y `Result<T>` en
  return); UI Scalar embebida en `/docs`; `@header(name="X")`
  como decorator stackable para headers como params del handler;
  subcomando `fitz openapi archivo.fitz`; opt-out con
  `@server(docs=false)`; paridad bit-a-bit entre `fitz run`,
  `fitz openapi` y `fitz build` (el binario nativo embebe el
  schema en build-time).
- **Mini-fase MW — Middleware y CORS**: decorator
  `@middleware(fn)` apilable sobre handlers HTTP (modelo gate-only:
  `return null` o sin return → continúa la chain; `return <status> { ... }` → short-circuit). Built-in `Request` (method/path/headers)
  y `Response` opaco. Built-in `cors(...)` configurable con kwargs
  via Map literal — preflight OPTIONS automático y headers
  `Access-Control-Allow-*` inyectados en la response real
  (incluso 500/400). Paridad bit-a-bit `fitz run` ↔ `fitz build`.
- **Mini-tanda Q (post-MW)**: 4 quick wins menores — `@header(into=)`
  para mapping explícito de header a param Fitz, `@server(api_version=)`
  override del schema OpenAPI, CORS request-aware con `List<Str>`
  haciendo echo del Origin, status codes custom apareciendo en
  schema OpenAPI.
- **Fase F17 — Send completo + paralelismo HTTP real**: la deuda
  más grande del proyecto cerrada. `Value` y `EnvRef` migran a
  `Arc<parking_lot::Mutex<T>>`, runtime tokio multi-thread, bridge
  HTTP `mpsc/oneshot` eliminado (~269 LoC netas menos en `http.rs`).
  Codegen output migra paralelo (`Arc<std::sync::Mutex>`, F12
  closures con `+ Send + Sync`, state HTTP `LazyLock<Arc<Mutex<T>>>`).
  5 requests concurrentes en 1.2s vs 5.3s en serie (validado a mano).
- **Mini-tanda PreF8 — Cleanup pre-Fase 8**: 4 sub-pasos antes del
  salto a Python interop. PreF8.1 refactor M1+M2 del codegen (AST
  output bit-a-bit idéntico), PreF8.2 method chain multi-línea en
  parser, PreF8.3 audit de defaults de tipos importados (fix de
  eager-at-import), PreF8.4 import aliasing (`as`).
- **Fase 8.1 — Embedding básico de CPython**: `from python import X` desde el intérprete (`fitz run --features python`). PyO3 0.28
  + ABI3-py310. Acceso a atributos, llamadas con args primitivos,
    return primitivo coercionado a `Value` Fitz. Sub-pasos: 8.1.1
    dep PyO3 opcional + `Value::PyObject` feature-gated, 8.1.2 loader
  + `from python import X`, 8.1.3 `Expr::Field` + auto-coerción
    primitiva, 8.1.4 `Expr::Call` con args primitivos (cumple el
    criterio del roadmap end-to-end), 8.1.5 guard de codegen
    (`fitz build` aborta con mensaje claro — deuda F19 comprometida
    para sub-paso futuro).
- **Fase 8.2 — Marshaling de tipos compuestos**: `List<T>` ↔
  `list`, `Map<K, V>` ↔ `dict`, e `Instance` → `dict` (por field
  name) entre los dos runtimes. Copia eager bidireccional, sin
  aliasing entre los dos GCs. Errores con breadcrumb informativo
  (`arg0[2].email`) para localizar tipos no marshalleables adentro
  de estructuras compuestas. Sub-pasos: 8.2.1 `value_to_py` con
  parámetro `path` y nuevas ramas List/Map/Instance, 8.2.2
  `py_to_value` con ramas PyList/PyDict antes del fallback opaco,
  8.2.3 criterio canónico end-to-end (`List<User>` →
  `collections.Counter` → `Map<Str, Int>`) + ejemplo runnable
  `examples/python-interop-8.2.fitz`.
- **Fase 8.3 — Excepciones Python → `Result<T>`**: toda llamada
  a una función Python desde Fitz se envuelve automáticamente. El
  programa Fitz no aborta — el usuario maneja la falla con `match`
  o `?`, igual que `find`/`get`/`json.loads` nativos. Preserva el
  modelo "sin excepciones" del lenguaje. Decisión asimétrica:
  `call` envuelve y `get_attr` no (`math.pi` sigue siendo Float
  directo, `math.sqrt(16.0)` es `Ok(4.0)`). Marshaling de args
  fallido también va en `Err` (uniformidad). Mensaje canónico
  `"<ClassName>: <message>"` estable desde 8.1.2. Sub-pasos:
  8.3.1 `py_interop::call` envuelve siempre + tests viejos
  actualizados con helpers `ok_inner`/`err_message` + 7 tests
  nuevos del shape y criterio; 8.3.2 ejemplos 8.1/8.2 reescritos
  al nuevo modelo (con caveat del parser de interpolación con
  `{...}` documentado); 8.3.3 ejemplo dedicado
  `examples/python-interop-8.3.fitz` con 6 secciones (criterio
  textual del roadmap, excepciones como Err, propagación con `?`,
  marshaling fallido con breadcrumb, field access sin wrap,
  chaining con desempaquetado intermedio).
- **Fase 8.4 — Tipos del checker + anotaciones del lado Fitz +
  coerción runtime Map → Instance**: cierra el ciclo "call Python
  → tipo Fitz concreto" con tres cambios coordinados. (a) El
  checker distingue valores Python de Any genérico
  (`Type::PyAny`); imports `from python import X` tipan como
  PyAny vs Any. (b) Calls Python refinan al ret type
  `Result<Any>`, activando estáticamente la regla de
  exhaustividad sobre Result y la regla del operador `?` (5.3.3)
  — el usuario es forzado a manejar el error sin gradual escape.
  (c) En runtime, `Stmt::Assign` con anotación nominal
  (`let row: User = ...`) coerciona `Value::Map` →
  `Value::Instance`, iterando los fields declarados en orden
  (provided → resolved_defaults → default Expr → nullable Null
  → error claro). Habilita el patrón canónico
  `let row: User = py_call(...)?` con UNA sola anotación.
  Sub-pasos: 8.4.1+8.4.2 PyAny + call refinado + 9 tests checker;
  8.4.3 coerción runtime + 9 tests evaluator; 8.4.4 ejemplo
  runnable `examples/python-interop-8.4.fitz` con 5 secciones
  (happy path, nullable faltante, extras ignorados, JSON
  malformado propagado, default aplicado).
- **Fase 8.5 — `fitz py-types` auto-mapeo SQLAlchemy → `type`
  Fitz**: sub-comando nuevo que introspecciona un archivo
  Python con modelos SQLAlchemy (o mocks con el mismo shape) y
  emite los `type` Fitz correspondientes. Reduce el
  doble-tipado en proyectos que usan SQLAlchemy. Introspección
  por duck typing (`__table__.columns`) — funciona con
  SQLAlchemy real y con mocks. Mapeo: Integer/BigInteger →
  `Int`, Float/Numeric → `Float`, String/Text → `Str`, Boolean
  → `Bool`, DateTime → `Str` (ISO 8601), `nullable=True` → `?`,
  default literal inline, callable ignorado. Tipos desconocidos
  → `Any` con comentario `// ?`. In-process via PyO3 (no
  subprocess), requiere `--features python`. Sub-pasos: 8.5.1
  comando + introspección + mapping + 10 tests; 8.5.2 ejemplo
  runnable `examples/py-types/` con `models.py` (mock SQLA
  autosuficiente) + `models.fitz` (generado) + `usage.fitz`
  (`from models import` + coerción 8.4.3 sobre dicts JSON).
- **Fase 8.6 — Bridge tokio ↔ asyncio**: `py_async_fn().await`
  desde cualquier `async fn` Fitz. Cuando un call Python
  devuelve una corutina, Fitz la detecta via
  `inspect.isawaitable` y la envuelve automáticamente en
  `Value::Future` adentro del `Result::Ok` — el usuario escribe
  `.await` natural sin glue manual. Implementación "baseline
  blocking" con `tokio::task::spawn_blocking` +
  `asyncio.new_event_loop().run_until_complete(coro)` (Send-safe,
  no deadlockea con el runtime tokio existing). El GIL serializa
  Python (esperado por roadmap, funcional para APIs DB-bound).
  Sin marshaling Future Fitz → corutina Python (Future no
  marshalleable; `asyncio.gather` requiere helper Python externo).
  Sub-pasos: 8.6.1 detección + bridge + 3 tests; 8.6.2 ejemplo
  runnable `examples/python-interop-8.6.fitz` con 3 secciones
  (patrón canónico, awaits encadenados, lazy sin .await).
- **Fase 8.7 — Codegen interop Python en `fitz build`**: cierra la
  deuda F19 del post-5b. `fitz build` compila programas con
  `from python import X` a binario nativo standalone con pyo3
  linkeado (Cargo.toml condicional, preludio `__FitzPyObject` +
  helpers, bindings globales con `OnceLock` + getter). Cubre
  getattr opaco/primitivo, call con args primitivos + List/Map/
  Instance via trait `__FitzToPy`, Result wrap automático,
  bridge async tokio ↔ asyncio (patrón canónico `<py_call>?.await`).
  Paridad bit-a-bit `fitz run` ↔ `fitz build`. Sub-pasos: 8.7.1
  preludio + import + getattr + Cargo.toml, 8.7.2 call + marshaling
  Fitz → Python + Result, 8.7.3 bridge async (baseline blocking
  paralelo a 8.6.1), 8.7.4 cierre formal con ejemplo
  `examples/python-interop-8.7.fitz`. **Bundling de CPython
  embebido queda como sub-paso futuro separado** — el binario
  asume Python instalado en el destino.
- **Fase 8.8 — Guía + ejemplo CRUD + cierre formal de Fase 8**:
  cierra la Fase 8 entera con docs y un ejemplo ejecutable. Cap
  21 nuevo "Interop Python" en `docs/guide.md` con 12 sub-secciones
  cubriendo setup, sintaxis, marshaling, coerciones, `fitz py-types`, async, `fitz build`, y limitaciones honestas
  (renumeración cap 21→22). Ejemplo
  `examples/guide/21-python-crud/` (SQLAlchemy + SQLite + handlers
  HTTP) validado end-to-end con curl. Sub-pasos: 8.8.1 cap 21
  + renumeración; 8.8.2 ejemplo CRUD; 8.8.3 cierre formal
    (CHANGELOG, roadmap, deudas, README, CLAUDE). Decisiones de
    scope: cap 21 (una renumeración), SQLite (sin Docker), solo
    `fitz run` con nota explícita sobre deuda residual de 8.7.

**Cierre formal de Fase 8 entera (Interop Python)** — roadmap
original cumplido al 100%: embedding (8.1), marshaling (8.2),
excepciones → Result (8.3), tipos del checker (8.4), `fitz py-types` (8.5), bridge async (8.6), codegen (8.7), y docs +
CRUD (8.8).

**1310 tests pasando con `--features python`** (1310 unit + 88 E2E
con `fitz build` + 3 openapi_e2e). **1219 + 79 + 3** sin feature.
Clippy `-D warnings` limpio en ambos modos.

**Mini-tandas post-Fase 8 (polish del lenguaje base, 2026-05-17/20)**:
una serie de bundles cerrados consecutivamente que llevaron al
lenguaje + LSP + HTTP a un estado pulido antes de Fase 9.w (Stack
web first-class). Incluye: **R.1/R.2/R.3** (sintaxis polish, métodos
custom sobre `type`), **S/Mb-series/Math+Mb9** (~40+ métodos chicos
sobre `Str`/`List`/`Map`/`Range`/`Int`/`Float`), **It/Cmp+/Up/Ex**
(iteradores + comprehensions + tuple destruct + Map.update),
**Bits/Núm/Lit/F8/F9/Fmt-build** (operadores de bit, separadores en
números, hex/bin/oct, identifiers Unicode, escapes extendidos, format
specs), **Cd/F11-F19** (codegen polish: higher-order completo, state
HTTP shared, módulos transitivos, identifiers Unicode, error
recovery, IR tipado, codegen interop Python), **Fp+Sp/Fp.2/Fp.3/Sp.2**
(default params, varargs, named args, return en match arm), **HC/LSPx**
(HTTP polish + LSP cross-module go-to-def), **LSPy** (Range exacto +
scope-aware autocomplete), **Hpx.1/Hpx.2** (Content-Type 415 + return
type inference), **Mw.next/5b.1/P2** (post-process middleware + param
type inference + chained fix), **RP/MP** (Result+post mws codegen +
urlencoded body), **P1** (Mw.next codegen), **UC/HA** (urlencoded
codegen + 415 msg alignment), **DZ/CT/OAPI** (división por cero +
comparar tipos distintos + status codes con consts), **MP2/MP-Build**
(multipart en intérprete + paridad bit-a-bit en codegen),
**Bytes** (sexto primitivo del lenguaje con literal `b"..."`),
**Mw-Wrap** (wrap-style middleware con `next` callable, intérprete),
**F13 SPIKE/A/B/C/D/E** (heterogéneos completos en `fitz build`:
primitivos + Bytes + Nominales + Map heterogéneo + anidados con mix
interno + HTTP body `List<Any>`/`Map<Str, Any>` + method dispatch
dinámico `.as_int()`/`.type_name()`), **OAPI-Expr** (status codes
con const-eval recursivo), **File.content Bytes** (uploads binarios
con multipart). **2045 unit + 250+ compile_e2e** sin feature, **2135
unit con --features lsp** al cierre. Detalle en
[docs/deudas_lenguaje.md](docs/deudas_lenguaje.md) y
[docs/design-fitzvalue.md](docs/design-fitzvalue.md) (F13 design).

**Estado del bloque post-Fase-8**: 95%+ del lenguaje compila a
binario nativo con paridad bit-a-bit `fitz run` ↔ `fitz build`.
**Única deuda residual visible**: Mw-Wrap codegen (wrap-style
middleware con `next` callable en `fitz build`) — `fitz run` ya lo
cubre end-to-end; el codegen rechaza con msg claro citando
`fitz run` como workaround. Implementarlo en codegen requiere
emitir cierres Rust con tipos `Arc<dyn Fn() -> Pin<Box<dyn Future
+ Send>>>` con captura recursiva (~2-3h dedicados). Sin presión
real hoy.

**Fase 9 (Ecosistema) — pre-reqs LSP (F15 + F16) y LSP MVP entero
CERRADOS (2026-05-15/16)**: error recovery del parser, side-table
`TypeInfo` por nodo, server `fitz-lsp` (tower-lsp), extensión
VSCode con grammar TextMate + cliente LSP + diagnostics en vivo +
hover con tipo del nodo + go-to-definition + autocomplete
contextual + distribución multi-platform con binario bundleado en
el `.vsix` por plataforma. 36 unit + 5 E2E nuevos con
`--features lsp` (acumulado del plan LSP). Total al cierre de
9.x.5: 1233 unit + 79 E2E + 3 openapi sin features.

**Próximo norte — tres bloques planificados con detalle alto en
[`docs/roadmap.md`](docs/roadmap.md)**:

- **9.y — Package manager + registry 📦** (en curso) — `fitz.toml`,
  `fitz new`/`init`, `fitz add`/`remove`/`update`, resolución +
  lockfile, registry HTTP escrito en Fitz mismo, `fitz publish` +
  auth. 7 sub-pasos. **9.y.1 + 9.y.2 + 9.y.3 entera (a/b/c) +
  9.y.4 CERRADOS (2026-05-16)**:
  - **9.y.1** — formato manifest TOML + `fitz new <nombre>` y
    `fitz init` con templates default (`print` top-level) y
    `--http` (`@get`/`@server`), `git init` automático con
    `--no-git`, validación de nombre estilo crates.io.
  - **9.y.2** — `fitz run`/`build`/`check` sin args leen el
    manifest del cwd/ancestros (Cargo-style walk-up). `fitz build`
    en manifest mode emite a `<manifest>/target/release/<pkg-name>(.exe)`
    con el nombre del paquete. **Sin breaking**: los ejemplos de la
    guía siguen corriendo idénticos con `fitz run examples/guide/X.fitz`.
  - **9.y.3.a** — path deps + sección `[lib]` + lockfile `fitz.lock`.
    `[dependencies] foo = { path = "../foo" }` en el importer + sección
    `[lib] entry = "src/lib.fitz"` en la dep. Lockfile TOML Cargo-style,
    emitido/sincronizado automáticamente, idempotente.
  - **9.y.3.b** — **loader integration**: el loader del evaluator
    (`fitz run`) y del codegen (`fitz build`) consultan el
    `dep_registry` resuelto del manifest ANTES de fallback a paths
    relativos. `from <dep-name> import X` resuelve al `lib_entry`
    absoluto de la dep. **Las deps declaradas en 9.y.3.a son
    finalmente usables desde código.**
  - **9.y.3.c** — **git deps + cache local**. Habilita
    `[dependencies] foo = { git = "https://...", tag = "v1.0.0" }`
    en `fitz.toml`. El primer acceso clona a `<cache>/git/
    <sanitized-url>@<ref>/` (default `~/.fitz/cache/`, override
    con `FITZ_CACHE_DIR`) y reusa el dir en accesos siguientes.
    El lockfile registra el commit hash exacto Cargo-style:
    `source = "git+<url>#<commit>"`. `tag` XOR `rev`; `branch` NO
    soportado (no reproducible). Subprocess `git` sobre crate
    (zero deps). Smoke validado: dep git con `file://` URL +
    `fitz run` + `fitz build` + binario bit-a-bit idéntico.
  - **9.y.3 entera CERRADA**: el package manager Fitz puede hoy
    declarar, resolver, bloquear y CONSUMIR deps tanto locales
    como de repos git remotos, sin registry todavía.
  - **9.y.4** — **`fitz add` / `fitz remove` / `fitz update`**.
    Automatiza la edición del manifest + lockfile. `fitz add
    <name> --path <p>` agrega path dep; `fitz add <name> --git
    <url> --tag <t>` (o `--rev`) agrega git dep; `fitz remove
    <name>` quita entry + sync lockfile; `fitz update [name]`
    invalida cache de git deps y fuerza re-clone. Sobreescribe
    si la dep ya existía. `toml_edit` preserva comentarios y
    formatting del usuario al modificar `fitz.toml`. Smoke
    validado: add path + git + remove + update + casos de error.
  - Total al cierre 9.y.4: 1294 unit + 48 cli_e2e + 79
    compile_e2e + 3 openapi. Clippy `-D warnings` limpio.
  - **9.y.5** (registry): **diferido** — path + git deps cubren
    el 90% del caso real; el registry implica decisiones de
    hosting + infra que dejamos para cuando aparezca demanda
    concreta. Saltamos a 9.z.
- **9.z — DX completo ✨** (en curso) — `fitz fmt`, `fitz test`,
  `fitz dev`, `fitz repl`, `fitz lint`. 5 sub-pasos.
  - **9.z.1 entera CERRADA (2026-05-16)** — formatter
    pretty-printer sobre el AST, cero config (4 espacios indent,
    comillas dobles, blank line solo entre fn/type top-level).
    Cubre >20 nodos. CLI con `--check` (read-only, CI mode) y
    write mode **production-ready** (preserva comments + blank
    lines del usuario; comments normalizados `//foo`→`// foo`;
    trailing comments con 2 espacios). Lexer side-stream `Trivia`
    nuevo — el parser/LSP/resto siguen zero overhead. Ver
    [`docs/fmt-style.md`](docs/fmt-style.md) para la referencia
    completa de convenciones.
  - Total al cierre 9.z.1: 1333 unit + 55 cli_e2e + 79 compile_e2e
    + 3 openapi. Clippy `-D warnings` limpio.
  - **9.z.2 entera CERRADA (2026-05-17)** — test runner built-in.
    Decorator `@test` sobre fns sin args + 4 assertion builtins
    (`assert`, `assert_eq`, `assert_ne`, `assert_throws`) +
    sub-comando `fitz test` con discovery automático
    (single-file mode + manifest mode con `tests/*.fitz` +
    auto-self-import bajo `package.name`) + filtrado por
    substring + async tests + output cargo-style (ok/FAILED +
    failures + summary + exit code 1 si falla) + ANSI colors
    auto cuando stdout es TTY. Codegen ignora `@test`
    silenciosamente (paralelo a `#[cfg(test)]` Rust). Tres
    sub-pasos cerrados (a + b + c): infraestructura del
    lenguaje + runner CLI + cap 24 nuevo en `docs/guide.md` con
    ejemplo runnable `examples/guide/24-tests.fitz`.
  - Total al cierre 9.z.2: **1366 unit + 66 cli_e2e + 79
    compile_e2e + 3 openapi**. Clippy `-D warnings` limpio.
  - **9.z.3 CERRADA (2026-05-17)** — `fitz dev` con hot reload.
    File watcher cross-platform (crate `notify`) + kill/respawn
    del child al detectar cambio en `.fitz` o `fitz.toml`.
    Debounce 100ms para colapsar saves múltiples del editor.
    Excluye `target/`/`.git/`/`node_modules/`/archivos ocultos.
    Banner ANSI con clear screen + run number entre runs.
    `tokio::signal::ctrl_c()` mata el child antes de salir
    (evita procesos zombie). Single-file mode
    (`fitz dev --file archivo.fitz`) y manifest mode (`fitz dev`
    desde el proyecto). Cap 25 nuevo en `docs/guide.md`.
  - Total al cierre 9.z.3: **1366 unit + 66 cli_e2e + 79
    compile_e2e + 3 openapi** (sin cambios — dev_cmd es
    interactivo, smoke E2E pendiente). Clippy `-D warnings`
    limpio.
  - **9.z.4 CERRADA (2026-05-17)** — `fitz repl` interactivo.
    Prompt `fitz> ` con env compartido entre líneas, multi-line
    continuation automática (`... `) via balanced brackets,
    6 comandos especiales (`:help`/`:quit`/`:env`/`:reset`/
    `:type`/`:load`), history persistente en `~/.fitz/history`
    con arrow up/down + Ctrl+R via crate `rustyline = "14"`,
    pretty-print Python-style del último valor cuando es expr
    top-level, async transparente (`sleep(100).await` funciona),
    filtro de warning spurio del checker para vars previas
    (substring "variable desconocida"). APIs nuevas públicas
    `evaluator::eval_program_with_env` + `new_repl_env` +
    `builtin_names` + `Environment::local_names`. Cap 26 nuevo
    en `docs/guide.md`.
  - Total al cierre 9.z.4: **1366 unit + 66 cli_e2e + 79
    compile_e2e + 3 openapi** (sin cambios — repl_cmd es
    interactivo, smoke E2E pendiente). Clippy `-D warnings`
    limpio.
  - **9.z.5 CERRADA (2026-05-17) — CIERRE FASE 9.z ENTERA**.
    `fitz lint` con 4 lints: `unused_variable`,
    `unused_import`, `useless_match`, `string_concat`.
    Default warnings + exit 0; `--deny <lint>` (repetible)
    promueve a error + exit 1 para CI. Supresión por
    `// @allow(<lint>)` en la línea anterior. Output estilo
    cargo-clippy (`warning:` amarillo / `error:` rojo con ANSI
    auto via `IsTerminal`). Lints skipeados del roadmap:
    `panic_in_test_only` (no aplica — sin `panic!` builtin) y
    `redundant_clone` (sin análisis de movimientos). Auto-fix
    `--fix` DIFERIDO como sub-paso futuro. Módulo nuevo
    `src/lint.rs` (~700 LoC con 15 unit tests). Cap 27 nuevo
    en `docs/guide.md`.
  - Total al cierre 9.z.5: **1381 unit + 73 cli_e2e + 79
    compile_e2e + 3 openapi** (+15 unit + 7 cli_e2e vs 9.z.4).
    Clippy `-D warnings` limpio.
  - **CIERRE FORMAL FASE 9.z ENTERA**: los 5 sub-pasos (fmt +
    test + dev + repl + lint) cerrados en 2 días consecutivos
    (16-17 de mayo). 5 capítulos nuevos en `docs/guide.md`
    (23-27).
  - **Refresh masivo de docs (2026-05-17)** — sub-paso dedicado
    posterior a 9.z entera. Cuatro sub-tareas: (A) refresh de
    caps stale en `docs/guide.md` (caps 12/13/17/20 con menciones
    a deuda ya cerrada — async/await, status codes, query params,
    headers, named args, middleware, chequeo de tipos en runtime,
    tipos compuestos en campos, encadenamiento multi-línea,
    server HTTP single-thread); (B) **cap 16b "Package manager"**
    nuevo entre cap 16 Módulos y cap 17 HTTP, con ejemplo
    runnable `examples/guide/16b-pkg-manager/` (greetings lib +
    greeter bin que importa via path dep) + 2 cli_e2e nuevos;
    (C) **`docs/architecture.md` refresh completo** (de 287 a
    ~470 líneas, 15 sub-comandos en lugar de 3, 12 módulos nuevos
    documentados que faltaban — `lib.rs`/`manifest.rs`/
    `lockfile.rs`/`git_dep.rs`/`testing.rs`/`fmt.rs`/`lint.rs`/
    `lsp.rs`/`py_interop.rs`/`py_types.rs`/`openapi.rs`);
    (D) **fix del bug fmt** (trailing comment al final de body
    seguido de otro bloque insertaba blank spurio) con test E2E
    de regresión.
  - Total al cierre del refresh: **1381 unit + 76 cli_e2e + 79
    compile_e2e + 3 openapi**. Clippy `-D warnings` limpio.
  - Próximo norte: Fase 9.w (Stack web first-class).
- **9.w — Stack web first-class 🌐** — **MVP CERRADO ENTERO
  (2026-05-21)**. `@authenticated`/`@admin` (auth nativo
  JWT-based, **9.w.1**), `@ws` (WebSockets tipados con
  `WsConn<T>` + AsyncAPI 3.0 + heartbeat built-in + auth
  integrada, **9.w.2**), `@cron` + `@background` + `spawn`
  (jobs sin Celery, sin broker externo, **9.w.3**). ORM nativo
  + migraciones (9.w.4) diferido a Fase 10 por scope (driver
  Postgres puro es comparable a todo Fase 5-9 combinado).
  Próximo norte: boilerplates Dockerizados showcase del stack
  cerrado.

**Visión post-Fase 9 (Fase 10+)** — especulativo, norte
direccional: Fase 10 stack DB nativo + ORM declarativo, Fase 11
frontend en `.fitz` (SFC + SSR — la apuesta más ambiciosa), Fase
12 deployment ciudadano primera clase (`fitz deploy`, observability
nativa), Fase 13 CLI builder (`@command`/`@arg`/`@flag`).

**Sub-paso separado pendiente sin presión**: bundling CPython
embebido (`fitz build --bundle-python`) con dos opciones evaluadas
(python-build-standalone — mantenida activamente por Astral;
PyOxidizer — ralentizada 2024-2025).

**Deudas comprometidas que siguen**: coerción Python list/dict →
Fitz `List<T>`/`Map<K,V>`/`Instance` en `fitz build` (helpers ya
emitidos, falta wiring en `coerce`), `.await` con binding
intermedio split, stubs `.pyi` parseados (pospuesto a Fase 9+),
descripciones via doc-strings sobre handlers (OpenAPI enrichment),
modelo wrap de middleware (post-process) si aparece presión real,
event loop asyncio persistente (paralelismo I/O real en interop
async), marshaling Future↔Coroutine.

## Qué funciona hoy

- **Sintaxis completa** (Fases 2-3): variables, aritmética con
  coerción Int↔Float, strings con interpolación, control de flujo
  (`if`/`while`/`for`/`loop`/`match`), funciones (bloque y flecha),
  closures, listas/mapas/rangos, tipos custom con defaults y
  campos nullables, `Result` + `?`, módulos.
- **HTTP nativo** (Fase 4): handlers con decoradores
  `@get`/`@post`/`@put`/`@delete`, path params tipados, body JSON
  con validación contra `type`, `@server(port, host)`.
- **Type checker estático** (Fase 5a): `fitz check` valida
  anotaciones de tipo. Reporta typos en variables, mismatches
  en asignación y argumentos, return contra return_type,
  exhaustividad de `match` sobre `Result`, métodos inexistentes
  sobre built-ins, índices con tipo de clave incompatible, y más.
- **Compilación a binario nativo** (Fase 5b): `fitz build` compila
  CLI y servidores HTTP a ejecutables standalone. Ver el
  [cap 20 de la guía](docs/guide.md#20-fitz-build--compilar-a-binario-nativo)
  para el subset cubierto y las limitaciones residuales.
- **Async nativo** (Fase 6): `async fn`, `.await` postfix,
  `Future<T>`, builtin `sleep`. Compatible con CLI y handlers
  HTTP. Ver [cap 19 de la guía](docs/guide.md#19-async-y-concurrencia).
- **Docs HTTP automáticas** (Fase 7): OpenAPI 3.1 + UI Scalar
  autogenerados desde los decoradores. `/openapi.json`, `/docs` y
  `fitz openapi archivo.fitz` gratis. `@header(name="X")` para
  headers como params, opt-out con `@server(docs=false)`. Ver
  [cap 18 de la guía](docs/guide.md#18-docs-automáticas).
- **Soporte para editores** (Fase 9.x.1 → 9.x.5, MVP completo):
  bin `fitz-lsp` (LSP server sobre tower-lsp) + extensión VSCode
  con highlighting, diagnostics en vivo, hover, go-to-definition,
  autocomplete contextual, y **distribución multi-platform** con
  binario bundleado en el `.vsix` per-plataforma (script
  reproducible `npm run build:vsix`). Errores del lexer/parser/
  checker subrayados al tipear; mouse sobre una expresión muestra
  su tipo; F12 te lleva a su declaración; tras `.` aparecen los
  métodos del tipo. Ver
  [cap 22 de la guía](docs/guide.md#22-soporte-para-editores).
- **Auth nativa** (Fase 9.w.1): `@auth_provider` + `@authenticated`
  + `@admin` como decoradores del lenguaje, con built-ins `jwt`
  (HS256/384/512) y `hash` (Argon2id). El checker valida
  estáticamente que cada handler protegido reciba un `User` del
  tipo correcto. OpenAPI auto-agrega `securitySchemes.bearerAuth`
  + `security` por handler + 401/403 en responses. Paridad
  bit-a-bit `fitz run` ↔ `fitz build`. Cero deps externas. Ver
  [cap 28 de la guía](docs/guide.md#28-auth-nativa) y el ejemplo
  [`examples/guide/28-auth.fitz`](examples/guide/28-auth.fitz).
- **WebSockets tipados** (Fase 9.w.2): `@ws("/path")` sobre
  `async fn` + `WsConn<T>` con métodos
  `recv`/`send`/`broadcast`/`close`. **Marshaling JSON automático**
  de cada frame text al `type` declarado, **AsyncAPI 3.0
  auto-generado** en `/asyncapi.json`, **heartbeat built-in** con
  `@server(ws_heartbeat_secs=N)`, **auth integrada** en el
  handshake (`@authenticated`/`@admin` apilados ANTES del HTTP
  upgrade), **codegen con paridad** bit-a-bit `fitz run` ↔ `fitz
  build`. **Ningún otro lenguaje hoy combina WS tipados con
  AsyncAPI auto-generado del código fuente, heartbeat built-in y
  auth integrada en el handshake**. Ver
  [cap 29 de la guía](docs/guide.md#29-websockets-tipados) y el
  ejemplo [`examples/guide/29-ws.fitz`](examples/guide/29-ws.fitz)
  (servidor de chat con login HTTP + JWT + broadcast multi-client).
- **Jobs sin Celery** (Fase 9.w.3): tres piezas — **`@cron("expr")`**
  para tareas periódicas (5/6/7 fields, cron Unix clásico),
  **`@background`** como marcador opt-in para autorizar
  `spawn(...)`, y **`spawn(fn_call)`** fire-and-forget tipado.
  **Sin broker externo** (Redis/RabbitMQ no son requisito), los
  jobs viven en memoria del proceso. El checker valida en
  compile-time que `spawn(...)` apunte a una fn `@background` y
  refina el ret type a `Future<T>` con T concreto.
  **Cron-only mode** (sin `@server`) queda vivo bloqueante con
  `signal::ctrl_c` automático (systemd-friendly). Paridad
  bit-a-bit `fitz run` ↔ `fitz build`. **Ningún otro lenguaje
  combina cron + background workers + spawn tipado en el core sin
  broker externo**. Ver
  [cap 30 de la guía](docs/guide.md#30-jobs-sin-celery) y el
  ejemplo
  [`examples/guide/30-cron-background.fitz`](examples/guide/30-cron-background.fitz)
  (URL shortener con HTTP + cron stats + spawn de tracking).

### CLI

```bash
# Ejecutar un programa (intérprete + checker strict)
fitz run programa.fitz

# Validar tipos sin ejecutar (exit 1 si hay errores)
fitz check programa.fitz

# Ejecutar saltando el chequeo estático (warnings, no aborta)
fitz run --no-typecheck programa.fitz

# Compilar a binario nativo (Fase 5b)
fitz build programa.fitz
./programa

# Emitir el schema OpenAPI 3.1 a stdout (Fase 7 — útil para CI)
fitz openapi programa.fitz > schema.json
```

### Compilando con interop Python (Fase 8.1+)

Para usar `from python import math` desde `fitz run`, el binario
`fitz` tiene que estar compilado con la feature opt-in `python`:

```bash
# Build local
cargo build --features python

# O install global
cargo install --path . --features python
```

Necesita Python 3.10+ instalado en la máquina. En Linux/Debian:
`apt install python3-dev`. En macOS: `brew install python@3.10`
(o superior). En Windows con el instalador de python.org, cero
config extra. En Windows con instalaciones raras (Microsoft Store,
nuget wrapper), setear `PYO3_PYTHON` al `.exe` real + prepender al
PATH el dir con `python3.dll` — ver CLAUDE.md para detalle.

El binario `fitz` default (sin la feature) sigue siendo standalone
sin link a libpython. Programs Fitz que no usan `from python import` no pagan nada.

## Estabilidad

Fitz está construido sobre Rust, que tiene un compromiso de
estabilidad fuerte desde 2015: código que compila en una versión
estable sigue compilando en versiones futuras, y los cambios que
podrían romper se aíslan en _editions_ opt-in.

Encima de eso, en este repo:

- `rust-toolchain.toml` pinea la versión exacta de Rust con la que
  Fitz se construye. Cloná el repo y `rustup` baja esa versión sola
  — no importa qué Rust tengas instalado globalmente.
- `rust-version` en `Cargo.toml` documenta la versión mínima
  soportada. Cargo da un error claro si alguien intenta con una más
  vieja.
- `Cargo.lock` fija las versiones exactas de todas las dependencias
  transitivas, así que builds reproducibles entre máquinas y en el
  tiempo.

En la práctica: un cambio en Rust o en una dependencia no rompe Fitz
hasta que vos decidas subir las versiones de manera explícita.

## Empezar

¿Querés aprender Fitz hoy? Leé la **[guía del lenguaje](docs/guide.md)**.
Es una guía viva en español que solo cubre lo que ya funciona, con
ejemplos ejecutables en [`examples/guide/`](examples/guide/).

Para la especificación completa de sintaxis (incluye features futuras
todavía no implementadas), ver [docs/syntax-spec.md](docs/syntax-spec.md).

## Nombre

**Fitz** por el Fitz Roy — la montaña más icónica de la Patagonia, en El Chaltén, Argentina.
Un nombre que no se olvida.

## Autor

Desarrollado en El Chaltén, Santa Cruz, Argentina 🇦🇷
Por un developer independiente que quería un lenguaje que no tuviera que disculparse por nada.

TheGreekMan (Palopoli Martín)

## Licencia

MIT
