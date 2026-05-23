# Especificación de Sintaxis — Fitz

> Estado: BORRADOR v0.7 (actualización 2026-05-17, post Fase 9.z entera).
> La mayoría del diseño original ya está implementado; lo pendiente
> queda señalado abajo.
>
> Este documento describe el **diseño completo** del lenguaje. Para
> ver solo lo que el intérprete ejecuta hoy, con ejemplos que corren,
> leé [docs/guide.md](guide.md).
>
> ## Matriz rápida de estado
>
> **Implementado y estable**:
> - Variables, primitivos, strings con interpolación, operadores (cap
>   3-6 de la guía).
> - Control de flujo: `if`/`while`/`for`/`loop`/`match` (caps 7-10).
> - Funciones, closures, higher-order (cap 11).
> - Tipos custom (`type`), structs, field access, defaults, nullables
>   (cap 12-13).
> - Listas, mapas, rangos, métodos built-in (`push`, `map`, `filter`,
>   `find`, `get`, etc.) — cap 9, 13.
> - `Result<T>` + `Ok`/`Err` + `?` + `match` exhaustivo (cap 14).
> - Módulos: `import foo`, `from foo import bar as baz` — cap 16.
> - Type checker estático: `fitz check` valida tipos en todo el
>   programa (Fase 5a). `fitz run` corre en modo strict por default.
> - HTTP nativo (cap 17): `@get`/`@post`/`@put`/`@delete`, path
>   params tipados, body JSON, query params, status codes custom
>   (`return <int> { ... }`), `@server(port, host, docs=Bool,
>   api_version="X")`, `@header(name="X", into="alias")`,
>   `@middleware(fn | cors(...))`.
> - OpenAPI 3.1 autogenerado + UI Scalar en `/docs` (cap 18).
>   Subcomando `fitz openapi archivo.fitz`. Status codes custom
>   reflejados en `responses` del schema.
> - Async nativo (cap 19): `async fn`, `.await`, `Future<T>` como
>   tipo built-in, builtin `sleep(ms)`.
> - **Paralelismo HTTP real (post-F17)**: el server (tanto `fitz run`
>   como el binario producido por `fitz build`) corre tokio
>   `rt-multi-thread` con N workers según cores. Containers de Value
>   y EnvRef migrados a `Arc<Mutex<>>` (Send + Sync). Bridge HTTP
>   `mpsc/oneshot` eliminado. 5 requests concurrentes a un handler
>   `sleep(1000).await` responden en ~1.2s (no ~5s como pre-F17).
> - Codegen a binario nativo via `fitz build` (cap 20).
> - Middleware + CORS con preflight automático y echo del Origin
>   recibido (`cors({"allow_origin": ["a.com", "b.com"]})`).
> - Interop con Python (`from python import sqlalchemy`) — Fase 8
>   entera (caps 21.1 → 21.12).
> - LSP (autocomplete + hover + go-to-def + diagnostics) + extensión
>   VSCode — Fase 9.x entera (cap 22). Distribución multi-plataforma.
> - Package manager: `fitz new`/`init`/`add`/`remove`/`update`,
>   manifest `fitz.toml`, deps path/git con lockfile — Fase 9.y.1 →
>   9.y.4. Registry (9.y.5) diferido.
> - Formatter `fitz fmt` (cero config, preserva comments) — Fase 9.z.1
>   (cap 23).
> - Test runner `fitz test` con `@test` + 4 assertion builtins
>   (`assert`, `assert_eq`, `assert_ne`, `assert_throws`) — Fase 9.z.2
>   (cap 24).
> - `fitz dev` (hot reload con file watcher + kill/respawn) — Fase
>   9.z.3 (cap 25).
> - `fitz repl` (REPL interactivo con env compartido, multi-line,
>   comandos `:type`/`:env`/`:load`, history persistente) — Fase
>   9.z.4 (cap 26).
> - `fitz lint` (4 lints: `unused_variable`, `unused_import`,
>   `useless_match`, `string_concat`; supresión por
>   `// @allow(<lint>)`) — Fase 9.z.5 (cap 27). **Cierra Fase 9.z
>   entera**.
>
> **Diseñado pero no implementado**:
> - `@bench` para benchmarks (post-MVP de 9.z.2).
> - Test fixtures (`@before_all`, `@before_each`, etc.) — post-MVP.
> - Auto-fix `fitz lint --fix` (post-MVP de 9.z.5).
> - Lints adicionales (`redundant_clone` cuando aterrice análisis
>   de movimientos).
> - `fitz lint` (linter de patrones más allá de tipos) — Fase 9.z.5.
> - Registry público (`fitz publish` + `fitz add foo@1.2.3`) — Fase
>   9.y.5, diferido.
> - Bundle Scalar offline embebido (hoy CDN) — deuda menor (Q.5,
>   postergada por trade-off de tamaño).
> - Doc-strings sobre handlers retenidos por el parser — refactor
>   invasivo lexer/parser/AST; pendiente.
> - CLI builder nativo (`@command`/`@arg`/`@flag`) — Fase 13 del
>   roadmap.
> - Frontend en `.fitz` (SFC + SSR) — Fase 11+ del roadmap.
>
> Cuando esta especificación y la guía discrepan, **gana la guía**
> (que solo documenta lo implementado).

---

## Comentarios

```fitz
// comentario de una línea

/* comentario
   multilínea */
```

---

## Variables

```fitz
// sin tipo — inferido. `let` es opcional, ambas formas conviven.
name = "Fitz"
let count = 42
active = true

// con tipo explícito
name: Str = "Fitz"
let count: Int = 42
score: Float = 3.14

// nullable — el ? indica que puede ser null
email: Str? = null
age: Int? = 25
```

> `let x = 1` y `x = 1` son equivalentes — el parser acepta ambas
> sin distinción semántica. Mutabilidad por defecto.

---

## Tipos primitivos

| Tipo | Descripción | Ejemplo |
|------|-------------|---------|
| `Int` | entero 64-bit | `42` |
| `Float` | punto flotante 64-bit | `3.14` |
| `Str` | string UTF-8 | `"hola"` |
| `Bool` | booleano | `true`, `false` |
| `Null` | ausencia de valor | `null` |

---

## Strings

```fitz
name = "Fitz"

// interpolación nativa con {}
greeting = "Hola, {name}!"
result = "La respuesta es {40 + 2}"

// multilínea
text = """
    Hola
    mundo
"""
```

---

## Tipos compuestos

```fitz
// listas
numbers: List<Int> = [1, 2, 3]
names = ["Fitz", "Rust", "Python"]    // inferido

// mapas
config: Map<Str, Any> = {
    "host": "localhost",
    "port": 3000
}

// tuplas
point = (10, 20)
coords: (Int, Int) = (x, y)
```

---

## Structs / Tipos custom

```fitz
type User {
    id: Int
    name: Str
    email: Str?
    active: Bool = true    // valor por defecto
}

// instanciar
user = User {
    id: 1,
    name: "Fitz",
    email: "fitz@example.com"
}

// acceder
print(user.name)
```

---

## Funciones

```fitz
// función básica
fn greet(name: Str) -> Str {
    return "Hola, {name}"
}

// función con tipos inferidos
fn add(a, b) {
    return a + b
}

// función flecha (una expresión)
fn double(n: Int) -> Int => n * 2

// función async
async fn fetch_user(id: Int) -> User {
    let user = db.find(id).await
    return user
}

// función sin retorno
fn log(msg: Str) {
    print(msg)
}
```

---

## Control de flujo

```fitz
// if / else
if age >= 18 {
    print("mayor")
} else if age >= 13 {
    print("adolescente")
} else {
    print("niño")
}

// if como expresión
status = if active { "activo" } else { "inactivo" }

// for
for item in items {
    print(item)
}

for i in 0..10 {
    print(i)
}

// while
while running {
    tick()
}

// loop infinito con break
loop {
    let input = read_line()
    if input == "quit" { break }
    process(input)
}
```

---

## Match

```fitz
// match básico
match status {
    "active"   => print("activo")
    "inactive" => print("inactivo")
    _          => print("desconocido")
}

// match con Result (manejo de errores)
match db.find(id).await {
    Ok(user)  => return user
    Err(e)    => return 404 { message: e }
}

// match con binding
match user.age {
    0..12  => print("niño")
    13..17 => print("adolescente")
    18..   => print("adulto")
}
```

---

## Manejo de errores

```fitz
// Result es el tipo de retorno para operaciones que pueden fallar
fn divide(a: Float, b: Float) -> Result<Float> {
    if b == 0.0 {
        return Err("División por cero")
    }
    return Ok(a / b)
}

// ? propaga el error automáticamente (como en Rust)
async fn get_user_name(id: Int) -> Result<Str> {
    let user = db.find(id).await?
    return Ok(user.name)
}

// match para manejar el error
match divide(10.0, 0.0) {
    Ok(result) => print("Resultado: {result}")
    Err(e)     => print("Error: {e}")
}
```

---

## Async — `async fn` y `await`

Fitz soporta concurrencia cooperativa con `async`/`await` al
estilo Rust. Una función marcada `async fn` devuelve un
`Future<T>` cuando se la llama; `await` extrae el `T`.

```fitz
async fn fetch_data(url: Str) -> Result<Str> {
    let body = http_get(url).await?
    return Ok(body)
}

async fn main() {
    let data = fetch_data("https://example.com").await
    print(data)
}
```

### Reglas

- `await` es **postfix**: se escribe `expr.await`, no `await expr`.
  Encaja naturalmente en method chains: `db.find(id).await?`.
- `await` sólo es legal **adentro de `async fn`**. Usarlo en una fn
  sync es un error de tipos.
- Llamar a una `async fn` sin `.await` devuelve un `Future<T>` —
  útil para guardar el future en una variable o pasarlo como
  argumento.
- Sync y async **conviven libremente**. Una `async fn` puede llamar
  a una sync fn (sin `.await`); una sync fn puede recibir un
  `Future<T>` pero no puede await-earlo.

### `Future<T>` como tipo

```fitz
let pending: Future<Int> = compute_async()  // sin await
let value: Int = pending.await              // con await
```

`Future<T>` es un generic built-in con la misma forma que
`List<T>`, `Map<K, V>`, `Result<T>` o `Nullable<T>` — válido en
anotaciones, parámetros, returns, y campos de `type`.

### HTTP async

Cualquier handler HTTP puede ser `async fn`. El runtime tokio
existente ejecuta los handlers async sin trabajo extra del
usuario. Sync sigue siendo válido para handlers triviales:

```fitz
@get("/users/{id}")
async fn get_user(id: Int) -> Result<User> {
    let user = db.find(id).await?
    return Ok(user)
}

@get("/health")
fn health() -> Str => "ok"   // sync — sin await
```

> Implementado en Fase 6. Hasta entonces, `async fn` se parsea
> pero el evaluator lo trata como sync (los handlers HTTP corren
> con bridge sync/async via mpsc). El operador `.await` se
> introduce en 6.1 y comienza a funcionar en 6.3.

---

## HTTP — Core del lenguaje

```fitz
// GET
@get("/")
async fn index() -> Str {
    return "Hola desde Fitz 🏔️"
}

// GET con parámetro de ruta
@get("/users/{id}")
async fn get_user(id: Int) -> User {
    return db.find(id).await?
}

// POST con body tipado
@post("/users")
async fn create_user(body: UserInput) -> User {
    return db.save(body).await?
}

// PUT
@put("/users/{id}")
async fn update_user(id: Int, body: UserInput) -> User {
    return db.update(id, body).await?
}

// DELETE
@delete("/users/{id}")
async fn delete_user(id: Int) -> Str {
    db.delete(id).await?
    return "eliminado"
}

// respuestas con status code explícito
@get("/protected")
async fn protected() -> Response {
    return 401 { message: "No autorizado" }
}
```

---

## HTTP avanzado — auth, websockets, jobs (futuro, Fase 9.w)

Decoradores adicionales que extienden `@get`/`@post`/... al resto
del stack web típico. **No implementado todavía** — sintaxis
tentativa, sujeta a revisión al arrancar Fase 9.w.

```fitz
// auth con decoradores apilables sobre handlers
@authenticated
@get("/me")
async fn me(user: User) -> User {
    return user
}

@admin
@delete("/users/{id}")
async fn delete_user(id: Int) -> Str { ... }

// el provider del auth lo define el usuario (una vez por proyecto)
@auth_provider
fn check_token(headers: Map<Str, Str>) -> Result<User> {
    let token = headers.get("authorization")?
    // ... validación, lookup en DB
    return Ok(user)
}

// websockets tipados — decorator paralelo a @get
type ChatMsg { user: Str, text: Str }

@ws("/chat")
async fn chat_handler(conn: WsConn<ChatMsg>) {
    loop {
        match conn.recv().await {
            Ok(msg) => conn.broadcast(msg).await,
            Err(_) => break,
        }
    }
}

// frames binarios raw — `WsConn<Bytes>` cambia el wire de
// text JSON a `Message::Binary` opaco. Útil para protocolos
// custom (protobuf, MessagePack), streaming audio/video, etc.
@ws("/raw")
async fn raw(conn: WsConn<Bytes>) {
    loop {
        match conn.recv().await {
            Ok(buf) => conn.send(buf).await,
            Err(_) => break,
        }
    }
}

// cron jobs
@cron("0 0 * * *")  // cada medianoche
async fn cleanup_sessions() { ... }

// background tasks fire-and-forget
@background
async fn send_email(to: Str, body: Str) { ... }

@post("/users")
async fn create(input: UserInput) -> User {
    let u = save(input)
    spawn send_email(u.email, "welcome")
    return u
}
```

---

## Módulos e imports

```fitz
// importar módulo del proyecto
import utils
import utils.format

// importar específico
from utils import format_date, slugify

// interop Python
from python import numpy as np
from python import pandas as pd

// importar paquete de fitz registry (futuro)
import fitz/http
import fitz/db
```

---

## Entry point

```fitz
// Si hay rutas HTTP definidas, el servidor arranca automáticamente
// en puerto 3000 por defecto.

// Configuración opcional:
@server(port: 8080, host: "0.0.0.0")

// Para programas CLI, el entry point es main:
fn main() {
    print("Hola mundo")
}
```

---

## Testing (Fase 9.z.2 — IMPLEMENTADO)

Test runner built-in con decorator `@test`. Cerrado entero el
2026-05-17. Para detalle de uso ver
[cap 24 de la guía](guide.md#24-fitz-test--testing-built-in).

```fitz
@test fn suma_funciona() {
    assert_eq(2 + 2, 4)
}

@test fn nullable_funciona() {
    let u = User { id: 1, name: "Ada" }
    assert(u.email == null)
}

@test async fn pausa_y_compara() {
    let r = sleep(0).await
    assert_eq(r, null)
}

// benchmarks (futuro post-MVP de @test, no implementado)
@bench fn fib_es_rapido() {
    fib(20)  // medido por iteración
}
```

Builtins de aserción: `assert(cond, msg?)`, `assert_eq(a, b)`,
`assert_ne(a, b)`, `assert_throws(fn)`.

Discovery: `fitz test` descubre todos los `@test` del proyecto
(`tests/*.fitz` top-level + `[lib].entry` para tests inline solo-lib).

---

## CLI builder (futuro, Fase 13)

Construcción de CLIs con decoradores + autogeneración de `--help`.
**No implementado todavía** — sintaxis tentativa, Fase 13 del
roadmap.

```fitz
@command("greet")
@arg("name", help="A quién saludar")
@flag("loud", short="l", help="MAYÚSCULAS")
fn greet(name: Str, loud: Bool = false) {
    let msg = if (loud) { "HOLA, {name}!" } else { "Hola, {name}" }
    print(msg)
}

@command("server", help="Inicia el server HTTP")
@arg("port", help="Puerto", default=3000)
fn run_server(port: Int) {
    // arranca el server
}
```

Sin imports — typer/click/clap built-in.

---

## Ejemplo completo

```fitz
// api.fitz

type User {
    id: Int
    name: Str
    email: Str?
}

type UserInput {
    name: Str
    email: Str
}

// base de datos en memoria (para el ejemplo)
let users: List<User> = []

@get("/users")
async fn list_users() -> List<User> {
    return users
}

@get("/users/{id}")
async fn get_user(id: Int) -> User {
    let user = users.find(fn(u) => u.id == id)
    match user {
        Ok(u)  => return u
        Err(_) => return 404 { message: "Usuario no encontrado" }
    }
}

@post("/users")
async fn create_user(body: UserInput) -> User {
    let user = User {
        id: users.len() + 1,
        name: body.name,
        email: body.email
    }
    users.push(user)
    return 201 user
}
```
