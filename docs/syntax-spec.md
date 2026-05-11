# Especificación de Sintaxis — Fitz

> Estado: BORRADOR v0.1 — sujeto a cambios durante la implementación.
>
> **Importante**: este documento describe el **diseño completo** del
> lenguaje, incluidas features que todavía no están implementadas
> (HTTP, async, listas, tipos custom instanciables, Result, etc.).
> Tomalo como dirección, no como contrato.
>
> Para ver solo lo que el intérprete ejecuta hoy, con ejemplos que
> corren, leé [docs/guide.md](guide.md).

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
// sin tipo — inferido
name = "Fitz"
count = 42
active = true

// con tipo explícito
name: Str = "Fitz"
count: Int = 42
score: Float = 3.14

// nullable — el ? indica que puede ser null
email: Str? = null
age: Int? = 25
```

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
