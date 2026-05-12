# Fitz 🏔️

> Un lenguaje de programación moderno, compilado y orientado a servicios web.
> Nacido en la Patagonia. Construido con Rust.

```fitz
// Ejemplo aspiracional (sintaxis del syntax-spec). Lo de
// async/.await y status codes custom llega en Fases 4.x/5.x.
@get("/users/{id}")
async fn get_user(id: Int) -> User {
    let user = db.find(id).await
    return user
}
```

Para ver un ejemplo **que corre hoy end-to-end**, mirá
[`examples/server.fitz`](examples/server.fitz) — un CRUD completo
con `Result + ?`, body JSON y `@server(...)`, ejecutable con
`fitz run`.

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
| Compilado nativo     | ❌     | ❌         | ✅  | 🚧 † |
| HTTP en el core      | ❌     | ❌         | ❌  | ✅   |
| Async nativo         | ⚠️     | ✅         | ✅  | 🚧 ‡ |
| Interop Python       | ✅     | ❌         | ❌  | 🚧 § |

\* **Tipado gradual con chequeo estático** — Fase 5a completada.
`fitz check` y `fitz run` validan anotaciones en compile time;
sin anotación, se infiere o se trata como `Any`.

† **Compilado nativo** — en curso, **Fase 5b.1** cerrado.
Backend elegido: transpile-a-Rust. `fitz build hello.fitz`
compila un subset primitivo (Int/Float/Str/Bool, funciones,
control de flujo, `print`) a binario standalone via `rustc`.
Tipos compuestos, Result, módulos y HTTP entran en 5b.2-5b.6.

‡ **Async nativo** — la sintaxis `async fn` se parsea, pero el
runtime sigue siendo síncrono. Los handlers HTTP corren en un
thread del intérprete con bridge a tokio. El `await` real
llega en Fase 4.x/5.x.

§ **Interop Python via PyO3** — planificado, todavía no
implementado.

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

```bash
fitz build && ./main          # ← objetivo final; hoy compila el subset primitivo (5b.1)
```

Hoy mismo, lo equivalente (sin `async`, sin status codes custom,
con el intérprete) sí corre:

```bash
fitz run examples/server.fitz
# Servidor en http://localhost:3000
```

Y un programa CLI primitivo ya se compila a binario nativo:

```bash
fitz build hello.fitz
./hello
```

## Estado del proyecto

🏔️ **Fase 5a completada + Fase 5b.2 cerrado — el lenguaje compila
tipos custom + control de flujo + métodos Str a binario nativo.**

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
  anotaciones, llamadas (aridad + tipos), returns vs return_type,
  operador `?`, exhaustividad de `match` sobre `Result`, métodos
  built-in paramétricos (`List<T>.map`, etc.), índices (`xs[i]`),
  FnExpr.ret inferido del body. `fitz run` aborta en modo strict
  por default; `--no-typecheck` lo salta.
- **Fase 5b.1 — Codegen subset primitivo**: `fitz build` compila
  programas con primitivos, BinOp/UnaryOp/StrInterp, asignación +
  reasignación, `if`/`while`/`loop`/`for-range` y funciones
  top-level a binario standalone via transpile-a-Rust + `rustc`.
- **Fase 5b.2 — Tipos custom compilados**: `type Foo { ... }` se
  traduce a `Rc<RefCell<FooData>>` (preserva el aliasing y la
  mutación compartida del intérprete). Struct literal con defaults
  inline-eados, field access/assign, igualdad estructural,
  `if`-as-expression, métodos `Str.len/upper/lower`, `StrInterp`
  con cualquier tipo. Listas/mapas, `Result`/`?` y módulos entran
  en sub-pasos 5b.3-5b.5; HTTP en 5b.6.

**868 tests pasando** (846 unit + 22 E2E que compilan binarios).
Próximo: **Fase 5b.3** — listas, mapas, indexing y method calls
sobre containers en el compilador.

Ver [roadmap](docs/roadmap.md) para el estado detallado y la
deuda explícita.

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
- **Compilación a binario nativo** (Fases 5b.1 y 5b.2): `fitz build`
  compila programas con primitivos, control de flujo (incluyendo
  `if`-as-expression), funciones, tipos custom (con defaults,
  nullables e igualdad estructural), métodos built-in sobre Str e
  interpolación con cualquier tipo. Listas/mapas, Result, módulos
  y HTTP entran en 5b.3-5b.6.

### CLI

```bash
# Ejecutar un programa (intérprete + checker strict)
fitz run programa.fitz

# Validar tipos sin ejecutar (exit 1 si hay errores)
fitz check programa.fitz

# Ejecutar saltando el chequeo estático (warnings, no aborta)
fitz run --no-typecheck programa.fitz

# Compilar a binario nativo (Fase 5b.1+5b.2)
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
