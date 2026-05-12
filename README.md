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

🏔️ **Fase 5a completada + Fase 5b.1 cerrado — el lenguaje compila
a binario nativo (subset primitivo).**

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
- **Fase 5b.1 — Codegen a binario nativo (subset primitivo)**:
  `fitz build` compila un subset Fitz (Int/Float/Str/Bool/Null,
  BinOp/UnaryOp/StrInterp, asignación + reasignación,
  `if`/`while`/`loop`/`for-range`, funciones top-level, `print`) a
  binario standalone via transpile-a-Rust + `rustc`. Tipos
  compuestos, Result, módulos y HTTP entran en sub-pasos
  5b.2 a 5b.6.

**833 tests pasando** (825 unit + 8 E2E que compilan binarios).
Próximo: **Fase 5b.2** — tipos custom + field access en el
compilador.

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
- **Compilación a binario nativo (subset primitivo)** (Fase 5b.1):
  `fitz build` compila programas CLI con primitivos, control de
  flujo y funciones a binario standalone. Tipos compuestos,
  Result, módulos y HTTP entran en 5b.2-5b.6.

### CLI

```bash
# Ejecutar un programa (intérprete + checker strict)
fitz run programa.fitz

# Validar tipos sin ejecutar (exit 1 si hay errores)
fitz check programa.fitz

# Ejecutar saltando el chequeo estático (warnings, no aborta)
fitz run --no-typecheck programa.fitz

# Compilar a binario nativo (subset primitivo — Fase 5b.1)
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
