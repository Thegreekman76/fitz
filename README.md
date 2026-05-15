# Fitz 🏔️

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

| Feature              | Python | TypeScript | Go  | Fitz |
| -------------------- | ------ | ---------- | --- | ---- |
| Sintaxis limpia      | ✅     | ⚠️         | ❌  | ✅   |
| Tipado gradual       | ❌     | ✅         | ❌  | ✅ * |
| Compilado nativo     | ❌     | ❌         | ✅  | ✅ † |
| HTTP en el core      | ❌     | ❌         | ❌  | ✅   |
| Async nativo         | ⚠️     | ✅         | ✅  | ✅ ‡ |
| Docs HTTP automáticas | ⚠️    | ❌         | ❌  | ✅ ◊ |
| Interop Python       | ✅     | ❌         | ❌  | 🚧 § |

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
`@server(docs=false)`. Schema bit-a-bit idéntico entre `fitz
run`, `fitz openapi archivo.fitz` y `fitz build`. Ver
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

🏔️ **Fase 8.2 cerrada — Fitz pasa estructuras compuestas entre
los dos runtimes.** `List<T>` ↔ `list`, `Map<K, V>` ↔ `dict`, e
`Instance` → `dict` (por field name) cruzan la frontera Fitz ↔
Python con copia eager bidireccional. Cumple el criterio canónico
del roadmap: una función Python que recibe `List<User>` y devuelve
`Map<Str, Int>` (vía `collections.Counter`) funciona sin glue
extra y sin perder data. Los errores con tipos no marshalleables
llevan breadcrumb informativo (`arg0[2].email`). La interop completa
(excepciones → Result, anotaciones de tipo Python, SQLAlchemy/NumPy
ergonómico) llega en los sub-pasos siguientes (8.3 a 8.8). Ver el
[roadmap](docs/roadmap.md) para el plan completo.

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
  codegen `async fn` Rust + `tokio::time::sleep` para `fitz
  build`. Cumple la promesa de "HTTP nativo" a nivel de ejecución.
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
  `return null` o sin return → continúa la chain; `return <status>
  { ... }` → short-circuit). Built-in `Request` (method/path/headers)
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
- **Fase 8.1 — Embedding básico de CPython**: `from python import
  X` desde el intérprete (`fitz run --features python`). PyO3 0.28
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

**1245 tests pasando con `--features python`** (1245 unit + 80 E2E
con `fitz build` + 3 openapi_e2e). **1175 + 80 + 3** sin feature.
Clippy `-D warnings` limpio en ambos modos.

Próximo norte: **Fase 8.3 — Excepciones Python → `Result<T>`**
(wrap automático de toda llamada Python; el mensaje
`"<ClassName>: <message>"` que 8.1.2 ya emite queda estable —
solo cambia el envoltorio. Preserva la decisión de diseño "sin
excepciones" del lenguaje). Después: 8.4 (anotaciones del lado
Fitz para `let user: User = py_call(...)?`), 8.5 (`fitz py-types`
auto-mapeo SQLAlchemy), 8.6 (async + GIL), 8.7 (CPython bundled —
candidato para cerrar F19), 8.8 (guía + ejemplo CRUD). Ver el
[roadmap](docs/roadmap.md) para detalle. **Deudas comprometidas
que siguen**: F19 (codegen interop Python en `fitz build`),
descripciones via doc-strings sobre handlers (OpenAPI enrichment),
modelo wrap de middleware (post-process) si aparece presión real.

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
sin link a libpython. Programs Fitz que no usan `from python
import` no pagan nada.

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
