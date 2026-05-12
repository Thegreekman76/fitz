# Fitz 🏔️

> Un lenguaje de programación moderno, compilado y orientado a servicios web.
> Nacido en la Patagonia. Construido con Rust.

```fitz
// Ejemplo aspiracional (sintaxis del syntax-spec). Lo de
// async/.await y status codes custom es deuda residual post-5.
@get("/users/{id}")
async fn get_user(id: Int) -> User {
    let user = db.find(id).await
    return user
}
```

Para ver un ejemplo **que corre hoy end-to-end con `fitz run`**,
mirá [`examples/server.fitz`](examples/server.fitz) — un CRUD
completo con `Result + ?`, body JSON y `@server(...)`. Para un
ejemplo **compilado a binario nativo con `fitz build`**, mirá
[`examples/guide/18-build.fitz`](examples/guide/18-build.fitz) —
server HTTP sin state compartido, compilable end-to-end.

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
| Async nativo         | ⚠️     | ✅         | ✅  | 🚧 ‡ |
| Interop Python       | ✅     | ❌         | ❌  | 🚧 § |

\* **Tipado gradual con chequeo estático** — Fase 5a completada.
`fitz check` y `fitz run` validan anotaciones en compile time;
sin anotación, se infiere o se trata como `Any`.

† **Compilado nativo** — Fase 5b completada. Backend:
transpile-a-Rust + Cargo. `fitz build` compila primitivos,
tipos custom, listas/mapas, `Result`/`?`/`match`, módulos y
HTTP (sin state compartido entre handlers) a binario standalone.
Ver [cap 18 de la guía](docs/guide.md#18-fitz-build--compilar-a-binario-nativo)
para el detalle del subset soportado y de la deuda residual.

‡ **Async nativo** — la sintaxis `async fn` se parsea, pero el
runtime sigue siendo síncrono. Los handlers HTTP corren en un
thread del intérprete con bridge a tokio (en `fitz run`) o como
fns sync wrapped por axum (en `fitz build`). El `await` real
llega post-5.

§ **Interop Python via PyO3** — planificado para Fase 6,
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

Hoy mismo, casi todo lo de arriba funciona — falta el `async`
real (la keyword se parsea pero el runtime es sync), los status
codes custom (`return 404 { ... }`), y el `await` del DB call:

```bash
fitz run examples/server.fitz
# Servidor en http://127.0.0.1:3000 (CRUD completo)
```

Un server HTTP **compilado a binario nativo** (sin state
compartido entre handlers, deuda residual de 5b.6):

```bash
fitz build examples/guide/18-build.fitz
./examples/guide/18-build      # Linux/macOS
# o:
.\examples\guide\18-build.exe  # Windows
```

## Estado del proyecto

🏔️ **Fase 5 completada — Fitz tiene type checker estático y
compilador a binario nativo.** El lenguaje pasa de intérprete
puro a un compilador completo que produce ejecutables standalone
(CLI o HTTP) via transpile-a-Rust + Cargo.

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

**949 tests pasando** (901 unit + 48 E2E que compilan binarios
con `fitz build` y validan output).

Próximo norte: **Fase 6 — Interop Python** (propuesta, no
comprometida) y **Fase 7 — Ecosistema** (package manager, LSP,
formatter, linter). Ver el [roadmap](docs/roadmap.md) para
detalle y la deuda residual de Fase 5b que queda como sub-paso
opcional (state compartido HTTP, higher-order completo, async
real, etc.).

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
  [cap 18 de la guía](docs/guide.md#18-fitz-build--compilar-a-binario-nativo)
  para el subset cubierto y las limitaciones residuales.

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
