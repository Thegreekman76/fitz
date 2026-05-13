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
[`examples/guide/19-build.fitz`](examples/guide/19-build.fitz) —
server HTTP sin state compartido, compilable end-to-end. Para
async, [`examples/guide/18-async.fitz`](examples/guide/18-async.fitz).

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
| Interop Python       | ✅     | ❌         | ❌  | 🚧 § |

\* **Tipado gradual con chequeo estático** — Fase 5a completada.
`fitz check` y `fitz run` validan anotaciones en compile time;
sin anotación, se infiere o se trata como `Any`.

† **Compilado nativo** — Fase 5b completada. Backend:
transpile-a-Rust + Cargo. `fitz build` compila primitivos,
tipos custom, listas/mapas, `Result`/`?`/`match`, módulos,
HTTP y async a binario standalone. Ver
[cap 19 de la guía](docs/guide.md#19-fitz-build--compilar-a-binario-nativo)
para el detalle del subset soportado y de la deuda residual.

‡ **Async nativo** — Fase 6 completada. `async fn` y `.await`
postfix reales en el lenguaje. `Future<T>` como tipo built-in,
builtin `sleep`, evaluator async sobre tokio current_thread,
handlers HTTP async, codegen `async fn` Rust. Ver
[cap 18 de la guía](docs/guide.md#18-async-y-concurrencia).
Deuda visible: el server HTTP sigue single-threaded
(`current_thread` runtime) — el paralelismo real entre
handlers requiere F17 (Send completo, comprometida en
`docs/deudas-post-5b.md`).

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
`.await` (cap 18 de la guía) y status codes custom
(`return 404 { ... }`, cap 17). Lo único que falta es el driver
de DB:

```bash
fitz run examples/server.fitz
# Servidor en http://127.0.0.1:3000 (CRUD completo)
```

Un server HTTP **compilado a binario nativo**:

```bash
fitz build examples/guide/19-build.fitz
./examples/guide/19-build      # Linux/macOS
# o:
.\examples\guide\19-build.exe  # Windows
```

Y un programa CLI con `async fn` + `.await`:

```bash
fitz build examples/guide/18-async.fitz
./examples/guide/18-async      # Linux/macOS
```

## Estado del proyecto

🏔️ **Fase 6 completada — Fitz tiene async nativo.** `async fn`,
`.await` postfix, `Future<T>` como tipo built-in, builtin
`sleep`, evaluator async sobre tokio, handlers HTTP async,
codegen `async fn` Rust. El lenguaje cumple la promesa de
"HTTP nativo" tanto a nivel ergonómico (cap 17) como a nivel de
ejecución (cap 18).

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

**1085+ tests pasando** (1018 unit + 67 E2E que compilan binarios
con `fitz build` y validan output).

Próximo norte: **Fase 7 — DX HTTP** (OpenAPI + Scalar),
**Fase 8 — Interop Python**, **Fase 9 — Ecosistema**. Ver el
[roadmap](docs/roadmap.md) para detalle. **Deuda comprometida
post-Fase 6**: F17 (Send completo) habilita paralelismo real
entre handlers HTTP y elimina el bridge mpsc interno del server.

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
  [cap 19 de la guía](docs/guide.md#19-fitz-build--compilar-a-binario-nativo)
  para el subset cubierto y las limitaciones residuales.
- **Async nativo** (Fase 6): `async fn`, `.await` postfix,
  `Future<T>`, builtin `sleep`. Compatible con CLI y handlers
  HTTP. Ver [cap 18 de la guía](docs/guide.md#18-async-y-concurrencia).

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
