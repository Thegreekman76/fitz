# Syntax Specification — Fitz

> Status: DRAFT v0.7 (updated 2026-05-17, post-Phase 9.z full).
> Most of the original design is already implemented; the open
> items are flagged below.
>
> This document describes the **complete design** of the language.
> For only what the interpreter executes today, with runnable
> examples, see [docs/guide.md](guide.md).
>
> ## Quick status matrix
>
> **Implemented and stable**:
> - Variables, primitives, strings with interpolation, operators
>   (guide chapters 3-6).
> - Control flow: `if`/`while`/`for`/`loop`/`match` (chapters 7-10).
> - Functions, closures, higher-order (chapter 11).
> - Custom types (`type`), structs, field access, defaults,
>   nullables (chapters 12-13).
> - Lists, maps, ranges, built-in methods (`push`, `map`, `filter`,
>   `find`, `get`, etc.) — chapters 9, 13.
> - `Result<T>` + `Ok`/`Err` + `?` + exhaustive `match` (chapter 14).
> - Modules: `import foo`, `from foo import bar as baz` — chapter 16.
> - Static type checker: `fitz check` validates types across the
>   whole program (Phase 5a). `fitz run` runs in strict mode by
>   default.
> - Native HTTP (chapter 17): `@get`/`@post`/`@put`/`@delete`,
>   typed path params, JSON body, query params, custom status codes
>   (`return <int> { ... }`), `@server(port, host, docs=Bool,
>   api_version="X")`, `@header(name="X", into="alias")`,
>   `@middleware(fn | cors(...))`.
> - OpenAPI 3.1 auto-generated + Scalar UI at `/docs` (chapter 18).
>   Subcommand `fitz openapi file.fitz`. Custom status codes are
>   reflected in the schema's `responses`.
> - Native async (chapter 19): `async fn`, `.await`, `Future<T>`
>   as a built-in type, builtin `sleep(ms)`.
> - **Real HTTP parallelism (post-F17)**: the server (both
>   `fitz run` and the binary produced by `fitz build`) runs tokio
>   `rt-multi-thread` with N workers based on cores. Value and
>   EnvRef containers migrated to `Arc<Mutex<>>` (Send + Sync). The
>   HTTP `mpsc/oneshot` bridge has been removed. 5 concurrent
>   requests to a `sleep(1000).await` handler respond in ~1.2s
>   (not ~5s like pre-F17).
> - Codegen to a native binary via `fitz build` (chapter 20).
> - Middleware + CORS with automatic preflight and echo of the
>   received Origin (`cors({"allow_origin": ["a.com", "b.com"]})`).
> - Python interop (`from python import sqlalchemy`) — full
>   Phase 8 (chapters 21.1 → 21.12).
> - LSP (autocomplete + hover + go-to-def + diagnostics) + VSCode
>   extension — full Phase 9.x (chapter 22). Multi-platform
>   distribution.
> - Package manager: `fitz new`/`init`/`add`/`remove`/`update`,
>   `fitz.toml` manifest, path/git deps with lockfile — Phases
>   9.y.1 → 9.y.4. Registry (9.y.5) deferred.
> - Formatter `fitz fmt` (zero config, preserves comments) —
>   Phase 9.z.1 (chapter 23).
> - Test runner `fitz test` with `@test` + 4 assertion builtins
>   (`assert`, `assert_eq`, `assert_ne`, `assert_throws`) — Phase
>   9.z.2 (chapter 24).
> - `fitz dev` (hot reload with file watcher + kill/respawn) —
>   Phase 9.z.3 (chapter 25).
> - `fitz repl` (interactive REPL with shared env, multi-line,
>   `:type`/`:env`/`:load` commands, persistent history) — Phase
>   9.z.4 (chapter 26).
> - `fitz lint` (4 lints: `unused_variable`, `unused_import`,
>   `useless_match`, `string_concat`; suppression via
>   `// @allow(<lint>)`) — Phase 9.z.5 (chapter 27). **Closes
>   Phase 9.z in full**.
>
> **Designed but not implemented**:
> - `@bench` for benchmarks (post-MVP of 9.z.2).
> - Test fixtures (`@before_all`, `@before_each`, etc.) — post-MVP.
> - Auto-fix `fitz lint --fix` (post-MVP of 9.z.5).
> - Additional lints (`redundant_clone` once movement analysis
>   lands).
> - `fitz lint` (linter of patterns beyond types) — Phase 9.z.5.
> - Public registry (`fitz publish` + `fitz add foo@1.2.3`) —
>   Phase 9.y.5, deferred.
> - Embedded offline Scalar bundle (CDN today) — minor debt (Q.5,
>   postponed by size trade-off).
> - Doc-strings on handlers retained by the parser — invasive
>   lexer/parser/AST refactor; pending.
> - Native CLI builder (`@command`/`@arg`/`@flag`) — Phase 13 of
>   the roadmap.
> - Frontend in `.fitz` (SFC + SSR) — Phase 11+ of the roadmap.
>
> When this specification and the guide disagree, **the guide
> wins** (it only documents what is implemented).

---

## Comments

```fitz
// single-line comment

/* multi-line
   comment */
```

---

## Variables

```fitz
// no type — inferred. `let` is optional, both forms coexist.
name = "Fitz"
let count = 42
active = true

// with explicit type
name: Str = "Fitz"
let count: Int = 42
score: Float = 3.14

// nullable — the ? marks the value as possibly null
email: Str? = null
age: Int? = 25
```

> `let x = 1` and `x = 1` are equivalent — the parser accepts
> both with no semantic difference. Mutable by default.

---

## Primitive types

| Type | Description | Example |
|------|-------------|---------|
| `Int` | 64-bit integer | `42` |
| `Float` | 64-bit floating point | `3.14` |
| `Str` | UTF-8 string | `"hello"` |
| `Bool` | boolean | `true`, `false` |
| `Null` | absence of a value | `null` |

---

## Strings

```fitz
name = "Fitz"

// native interpolation with {}
greeting = "Hello, {name}!"
result = "The answer is {40 + 2}"

// multi-line
text = """
    Hello
    world
"""
```

---

## Composite types

```fitz
// lists
numbers: List<Int> = [1, 2, 3]
names = ["Fitz", "Rust", "Python"]    // inferred

// maps
config: Map<Str, Any> = {
    "host": "localhost",
    "port": 3000
}

// tuples
point = (10, 20)
coords: (Int, Int) = (x, y)
```

---

## Structs / Custom types

```fitz
type User {
    id: Int
    name: Str
    email: Str?
    active: Bool = true    // default value
}

// instantiate
user = User {
    id: 1,
    name: "Fitz",
    email: "fitz@example.com"
}

// access
print(user.name)
```

---

## Functions

```fitz
// basic function
fn greet(name: Str) -> Str {
    return "Hello, {name}"
}

// function with inferred types
fn add(a, b) {
    return a + b
}

// arrow function (single expression)
fn double(n: Int) -> Int => n * 2

// async function
async fn fetch_user(id: Int) -> User {
    let user = db.find(id).await
    return user
}

// function with no return
fn log(msg: Str) {
    print(msg)
}
```

---

## Control flow

```fitz
// if / else
if age >= 18 {
    print("adult")
} else if age >= 13 {
    print("teen")
} else {
    print("child")
}

// if as expression
status = if active { "active" } else { "inactive" }

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

// infinite loop with break
loop {
    let input = read_line()
    if input == "quit" { break }
    process(input)
}
```

---

## Match

```fitz
// basic match
match status {
    "active"   => print("active")
    "inactive" => print("inactive")
    _          => print("unknown")
}

// match with Result (error handling)
match db.find(id).await {
    Ok(user)  => return user
    Err(e)    => return 404 { message: e }
}

// match with binding
match user.age {
    0..12  => print("child")
    13..17 => print("teen")
    18..   => print("adult")
}
```

---

## Error handling

```fitz
// Result is the return type for operations that may fail
fn divide(a: Float, b: Float) -> Result<Float> {
    if b == 0.0 {
        return Err("Division by zero")
    }
    return Ok(a / b)
}

// ? propagates the error automatically (like in Rust)
async fn get_user_name(id: Int) -> Result<Str> {
    let user = db.find(id).await?
    return Ok(user.name)
}

// match to handle the error
match divide(10.0, 0.0) {
    Ok(result) => print("Result: {result}")
    Err(e)     => print("Error: {e}")
}
```

---

## Async — `async fn` and `await`

Fitz supports cooperative concurrency with `async`/`await`
Rust-style. A function marked `async fn` returns a `Future<T>`
when called; `await` extracts the `T`.

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

### Rules

- `await` is **postfix**: you write `expr.await`, not `await expr`.
  Fits naturally in method chains: `db.find(id).await?`.
- `await` is only legal **inside an `async fn`**. Using it in a
  sync fn is a type error.
- Calling an `async fn` without `.await` returns a `Future<T>` —
  useful for storing the future in a variable or passing it as
  an argument.
- Sync and async **coexist freely**. An `async fn` may call a
  sync fn (without `.await`); a sync fn may receive a `Future<T>`
  but cannot await it.

### `Future<T>` as a type

```fitz
let pending: Future<Int> = compute_async()  // no await
let value: Int = pending.await              // with await
```

`Future<T>` is a built-in generic with the same shape as
`List<T>`, `Map<K, V>`, `Result<T>` or `Nullable<T>` — valid in
annotations, parameters, returns, and `type` fields.

### Async HTTP

Any HTTP handler may be `async fn`. The existing tokio runtime
executes async handlers with no extra work from the user. Sync
remains valid for trivial handlers:

```fitz
@get("/users/{id}")
async fn get_user(id: Int) -> Result<User> {
    let user = db.find(id).await?
    return Ok(user)
}

@get("/health")
fn health() -> Str => "ok"   // sync — no await
```

> Implemented in Phase 6. Until then, `async fn` parses but the
> evaluator treats it as sync (HTTP handlers run with a
> sync/async mpsc bridge). The `.await` operator is introduced in
> 6.1 and starts working in 6.3.

---

## HTTP — language core

```fitz
// GET
@get("/")
async fn index() -> Str {
    return "Hello from Fitz 🏔️"
}

// GET with path parameter
@get("/users/{id}")
async fn get_user(id: Int) -> User {
    return db.find(id).await?
}

// POST with typed body
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
    return "deleted"
}

// responses with explicit status code
@get("/protected")
async fn protected() -> Response {
    return 401 { message: "Unauthorized" }
}
```

---

## Advanced HTTP — auth, websockets, jobs (future, Phase 9.w)

Additional decorators that extend `@get`/`@post`/... to the rest
of the typical web stack. **Not implemented yet** — tentative
syntax, subject to revision when Phase 9.w begins.

```fitz
// auth with stackable decorators on handlers
@authenticated
@get("/me")
async fn me(user: User) -> User {
    return user
}

@admin
@delete("/users/{id}")
async fn delete_user(id: Int) -> Str { ... }

// the auth provider is defined by the user (once per project)
@auth_provider
fn check_token(headers: Map<Str, Str>) -> Result<User> {
    let token = headers.get("authorization")?
    // ... validation, DB lookup
    return Ok(user)
}

// typed websockets — decorator parallel to @get
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

// raw binary frames — `WsConn<Bytes>` changes the wire from
// text JSON to opaque `Message::Binary`. Useful for custom
// protocols (protobuf, MessagePack), streaming audio/video, etc.
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
@cron("0 0 * * *")  // every midnight
async fn cleanup_sessions() { ... }

// fire-and-forget background tasks
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

## Modules and imports

```fitz
// import a project module
import utils
import utils.format

// specific imports
from utils import format_date, slugify

// Python interop
from python import numpy as np
from python import pandas as pd

// import a package from the fitz registry (future)
import fitz/http
import fitz/db
```

---

## Entry point

```fitz
// If there are HTTP routes defined, the server starts
// automatically on port 3000 by default.

// Optional configuration:
@server(port: 8080, host: "0.0.0.0")

// For CLI programs, the entry point is main:
fn main() {
    print("Hello world")
}
```

---

## Testing (Phase 9.z.2 — IMPLEMENTED)

Built-in test runner with the `@test` decorator. Closed in full
on 2026-05-17. For usage detail see
[chapter 24 of the guide](guide.md#24-fitz-test--testing-built-in).

```fitz
@test fn sum_works() {
    assert_eq(2 + 2, 4)
}

@test fn nullable_works() {
    let u = User { id: 1, name: "Ada" }
    assert(u.email == null)
}

@test async fn pause_and_compare() {
    let r = sleep(0).await
    assert_eq(r, null)
}

// benchmarks (future post-MVP of @test, not implemented)
@bench fn fib_is_fast() {
    fib(20)  // measured per iteration
}
```

Assertion builtins: `assert(cond, msg?)`, `assert_eq(a, b)`,
`assert_ne(a, b)`, `assert_throws(fn)`.

Discovery: `fitz test` discovers every `@test` in the project
(top-level `tests/*.fitz` + `[lib].entry` for inline lib-only
tests).

---

## CLI builder (future, Phase 13)

Building CLIs with decorators + auto-generation of `--help`.
**Not implemented yet** — tentative syntax, Phase 13 of the
roadmap.

```fitz
@command("greet")
@arg("name", help="Who to greet")
@flag("loud", short="l", help="UPPERCASE")
fn greet(name: Str, loud: Bool = false) {
    let msg = if (loud) { "HELLO, {name}!" } else { "Hello, {name}" }
    print(msg)
}

@command("server", help="Starts the HTTP server")
@arg("port", help="Port", default=3000)
fn run_server(port: Int) {
    // start the server
}
```

No imports — built-in typer/click/clap.

---

## Full example

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

// in-memory database (for the example)
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
        Err(_) => return 404 { message: "User not found" }
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
