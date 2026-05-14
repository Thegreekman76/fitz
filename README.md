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

🏔️ **Fase 7 completada — Fitz tiene docs HTTP autogeneradas.**
OpenAPI 3.1 + UI Scalar gratis en `/docs` y `/openapi.json` desde
los decoradores existentes. `@header(name="X")` para headers como
params. Opt-out con `@server(docs=false)`. Subcomando `fitz
openapi archivo.fitz` para CI. Paridad bit-a-bit entre `fitz run`
y `fitz build`. El lenguaje cumple la promesa de "HTTP nativo" a
tres niveles: ergonómico (cap 17), de ejecución (cap 19) y de
developer experience (cap 18).

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

**1189+ tests pasando** (1118+ unit + 71 E2E que compilan
binarios con `fitz build`, levantan el server y validan responses
crudas via TCP).

Próximo norte: **Fase 8 — Interop Python**, **Fase 9 — Ecosistema**.
Ver el [roadmap](docs/roadmap.md) para detalle. **Deudas
comprometidas que siguen**: F17 (Send completo) para paralelismo
real entre handlers; descripciones via doc-strings sobre handlers
(para enriquecer OpenAPI); modelo wrap de middleware (post-process)
si aparece presión real.

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
