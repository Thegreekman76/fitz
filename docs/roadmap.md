# Roadmap — Fitz

---

## Fase 1 — Aprender Rust 🦀
**Estado: EN PROGRESO**

Antes de escribir el compilador, dominar las herramientas.

### Objetivos
- [ ] The Book capítulos 1-10 (rustlang-es.org)
- [ ] Rustlings — ejercicios básicos completos
- [ ] Entender ownership, borrowing y lifetimes
- [ ] Entender enums y pattern matching
- [ ] Primer proyecto Rust propio (pequeño)

### Recursos
- https://book.rustlang-es.org
- https://rustlings.cool
- https://doc.rust-lang.org/rust-by-example

### Criterio de completitud
Poder escribir un lexer básico en Rust sin consultar el libro en cada línea.

---

## Fase 2 — Intérprete base 🔬
**Estado: PENDIENTE**

El corazón del lenguaje. Al final de esta fase, Fitz puede ejecutar
programas básicos.

### Módulos a implementar

#### 2.1 Lexer
Convierte texto fuente en tokens.

```
"let x = 42 + 1"
→ [Let, Ident("x"), Eq, Int(42), Plus, Int(1)]
```

Tokens necesarios:
- Literales: Int, Float, Str, Bool, Null
- Operadores: +, -, *, /, ==, !=, <, >, <=, >=, =>, ?
- Delimitadores: (, ), {, }, [, ], ,, :, .
- Keywords: fn, async, return, if, else, for, while, match, let, type, import, from, true, false, null
- Decoradores: @get, @post, @put, @delete, @server
- Identificadores y comentarios

#### 2.2 AST (Abstract Syntax Tree)
Define las estructuras de datos que representan el programa.

```rust
enum Expr {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
    Ident(String),
    BinOp { op: Op, left: Box<Expr>, right: Box<Expr> },
    Call { name: String, args: Vec<Expr> },
    // ...
}

enum Stmt {
    Let { name: String, value: Expr },
    Return(Expr),
    If { cond: Expr, then: Block, else_: Option<Block> },
    // ...
}
```

#### 2.3 Parser
Convierte tokens en AST.

```
[Let, Ident("x"), Eq, Int(42), Plus, Int(1)]
→ Let { name: "x", value: BinOp { op: Add, left: Int(42), right: Int(1) } }
```

#### 2.4 Evaluador
Recorre el AST y ejecuta el programa.

### Criterio de completitud
Este programa funciona:
```fitz
name = "Fitz"
x = 10 + 5
print("Hola {name}, x es {x}")

fn double(n) => n * 2
print(double(x))
```

---

## Fase 3 — El lenguaje crece 🌱
**Estado: PENDIENTE**

Agregar las features que hacen a Fitz expresivo.

### Features
- [ ] Tipos custom (`type User { ... }`)
- [ ] Listas y mapas con operaciones básicas
- [ ] Match expressions
- [ ] Result / manejo de errores
- [ ] Funciones de orden superior
- [ ] String interpolation
- [ ] Módulos e imports básicos
- [ ] Tipado gradual — anotaciones opcionales

### Criterio de completitud
```fitz
type User {
    id: Int
    name: Str
}

fn find_user(users: List<User>, id: Int) -> Result<User> {
    let user = users.find(fn(u) => u.id == id)
    match user {
        Ok(u)  => return Ok(u)
        Err(_) => return Err("no encontrado")
    }
}
```

---

## Fase 4 — HTTP nativo 🌐
**Estado: PENDIENTE**

La feature que diferencia a Fitz. HTTP como ciudadano de primera clase.

### Implementación
- Integrar **Axum** o **Hyper** por debajo como runtime HTTP
- El evaluador detecta decoradores `@get`, `@post`, etc.
- Genera los handlers automáticamente
- Serialización/deserialización JSON automática por tipo de retorno
- Servidor arranca automáticamente si hay rutas definidas

### Criterio de completitud
```fitz
type User {
    id: Int
    name: Str
}

@get("/users/{id}")
async fn get_user(id: Int) -> User {
    return User { id: id, name: "Test" }
}
```
```bash
fitz run api.fitz
# GET http://localhost:3000/users/1
# → {"id": 1, "name": "Test"}
```

---

## Fase 5 — Compilador ⚡
**Estado: FUTURO**

Binario nativo. El salto de intérprete a compilador.

### Opciones
- **LLVM via inkwell** — máxima performance, alta complejidad
- **Cranelift** — más simple que LLVM, usado por Wasmtime
- **Compilar a C** — transpilación, menos purista pero efectivo

### Features
- [ ] Type checker completo
- [ ] Inferencia de tipos
- [ ] Optimizaciones básicas
- [ ] Binario nativo standalone
- [ ] Cross-compilation

---

## Fase 6 — Ecosistema 🌍
**Estado: VISIÓN FUTURA**

- [ ] Package manager (`fitz add`)
- [ ] Fitz registry (repositorio de paquetes)
- [ ] LSP (Language Server Protocol) — autocompletado en VSCode
- [ ] Formatter (`fitz fmt`)
- [ ] Linter (`fitz check`)
- [ ] Interop Python via PyO3
- [ ] Compilación a WebAssembly
- [ ] Documentación oficial en español e inglés
- [ ] Website del lenguaje

---

## Hitos clave

| Hito | Descripción |
|------|-------------|
| v0.1 | `print("hola")` funciona |
| v0.2 | Variables, funciones, control de flujo |
| v0.3 | Tipos custom, match, manejo de errores |
| v0.4 | HTTP nativo funcional |
| v0.5 | Primera API real escrita en Fitz |
| v1.0 | Compilador, binario nativo, package manager |
