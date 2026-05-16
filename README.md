<p align="center">
  <img src="assets/logo.png" alt="Fitz logo" width="160" />
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
| Interop Python         | ✅     | ❌         | ❌ | 🚧 § |

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

§ **Interop Python via PyO3** — planificado para Fase 8,
todavía no implementado.

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

**Fase 9.x.1 + 9.x.2 + 9.x.3 + 9.x.4 (LSP MVP completo) CERRADAS
— 2026-05-15/16** — las cuatro sub-fases del LSP MVP. Habilitan la
experiencia "escribir Fitz en VSCode con errores subrayados al
tipear" + "pasá el mouse y ve qué tipo tiene" + "F12 sobre un
nombre te lleva a su declaración" + "autocomplete contextual con
tipos". Tres componentes coordinados:
**bin nuevo `fitz-lsp`** (opt-in con `--features lsp`,
`cargo build --release --features lsp`) que implementa el protocolo
LSP estándar (JSON-RPC sobre stdio, tower-lsp 0.20); **módulo
`fitz::lsp`** en la lib que expone el pipeline LSP-style
(`parse_with_recovery + check_program`) y el helper `FitzError → Diagnostic`; **extensión VSCode** en
[`editors/vscode/`](editors/vscode/) con grammar TextMate y cliente
LSP que spawnea `fitz-lsp` (configurable via setting `fitz.lspPath`).
Próxima sub-fase: **9.x.5 distribución VSCode Marketplace** —
publicar la extensión con binarios pre-compilados por plataforma
bundleados en el `.vsix`, al estilo rust-analyzer. Ver
[cap 22 de la guía](docs/guide.md#22-soporte-para-editores) para
instalación + settings.

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

**Fase 9 (Ecosistema) — F15 CERRADO (2026-05-15)**: error recovery
del parser end-to-end (nodos `Expr::Error`/`Stmt::Error` in-band,
API `parse_with_recovery` con sync points stmt-level + keywords,
cota 100 errores, checker silencioso sobre Error nodes). 15 unit
tests nuevos. CLI strict sin cambio. Próximo norte: **F16 (IR
tipado persistido por nodo)** — segundo pre-req habilitante del
LSP. Después: sub-fases 9.x.1 → 9.x.5 (LSP MVP →
diagnostics/hover/go-to-def/autocomplete/distribución), formatter,
linter, package manager. **Sub-paso separado pendiente sin presión**:
bundling CPython embebido (`fitz build --bundle-python`) con
dos opciones evaluadas (python-build-standalone — mantenida
activamente por Astral; PyOxidizer — ralentizada 2024-2025).
Ver el [roadmap](docs/roadmap.md) para detalle. **Deudas
comprometidas que siguen**: coerción Python list/dict → Fitz
`List<T>`/`Map<K,V>`/`Instance` en `fitz build` (helpers ya
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
- **Soporte para editores** (Fase 9.x.1 + 9.x.2 + 9.x.3 + 9.x.4):
  bin `fitz-lsp` (LSP server sobre tower-lsp) + extensión VSCode
  con highlighting, diagnostics en vivo, hover, go-to-definition
  y autocomplete contextual. Errores del lexer/parser/checker
  subrayados al tipear; mouse sobre una expresión muestra su tipo;
  F12 te lleva a su declaración; tras `.` aparecen los métodos
  del tipo, en otras posiciones aparecen los símbolos en scope.
  Ver
  [cap 22 de la guía](docs/guide.md#22-soporte-para-editores).

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
