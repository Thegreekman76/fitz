# Roadmap — Fitz

---

## Fase 1 — Aprender Rust 🦀
**Estado: COMPLETADA**

Antes de escribir el compilador, dominar las herramientas.

### Objetivos
- [x] The Book capítulos 1-10 (rustlang-es.org)
- [x] Rustlings — ejercicios básicos completos
- [x] Entender ownership, borrowing y lifetimes
- [x] Entender enums y pattern matching
- [x] Primer proyecto Rust propio (pequeño)

### Recursos
- https://book.rustlang-es.org
- https://rustlings.cool
- https://doc.rust-lang.org/rust-by-example

### Criterio de completitud
Poder escribir un lexer básico en Rust sin consultar el libro en cada línea.

---

## Fase 2 — Intérprete base 🔬
**Estado: EN PROGRESO**

El corazón del lenguaje. Al final de esta fase, Fitz puede ejecutar
programas básicos.

### Módulos a implementar

#### 2.1 Lexer ✓
**Completado** — `src/lexer.rs` con 16 tests pasando.
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

#### 2.2 AST (Abstract Syntax Tree) ✓
**Completado** — `src/ast.rs` con 3 tests pasando. Soporta el programa del criterio de éxito de Fase 2 (incluye `StrInterp` para interpolación).
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

#### 2.3 Parser ✓
**Completado** — `src/parser.rs` con 111 tests pasando. Convierte tokens
en AST mediante recursive descent. El criterio de éxito de Fase 2 parsea
end-to-end (lexer → parser → AST).

```
[Let, Ident("x"), Eq, Int(42), Plus, Int(1)]
→ Let { name: "x", value: BinOp { op: Add, left: Int(42), right: Int(1) } }
```

**Alcance de 2.3 (lo que SÍ se implementa):**
- Expresiones: literales, identificadores, paréntesis, operadores aritméticos
  (`+`, `-`, `*`, `/`), comparación (`<`, `>`, `<=`, `>=`), igualdad
  (`==`, `!=`), unario `-`.
- Postfix: field access (`user.name`), llamadas a función (`f(args)`) con
  nombre simple.
- `StrInterp`: parsing del contenido de `Token::Str` para detectar `{...}`.
- Sentencias: `let`/asignación, `return`, expr-statement, `fn` (forma de
  bloque y de flecha), `type`, `if`/`else` (como sentencia o expresión),
  `match`, `break`, `continue`.
- Decoradores HTTP: `@get`/`@post`/`@put`/`@delete` envolviendo una `FnDef`.

**Fuera de alcance — deuda explícita, retomar después:**
- **Operadores lógicos `and` / `or`** — el lexer aún no emite tokens
  correspondientes; `BinOpKind::And` y `Or` quedan definidos pero sin uso.
  Retomar antes del evaluador si los necesitamos, o en Fase 3.
- **`while`, `for`, `loop`** — el AST aún no tiene estas variantes como
  `Stmt`. Retomar en Fase 3.
- **Method calls (`expr.method(args)`)** — `Expr::Call` solo admite
  `name: String`. Cuando mute a `callee: Box<Expr>` se desbloquea. Por
  ahora, el parser tira error explícito.
- **Asignación a campos (`user.name = ...`)** — `Stmt::Assign` solo admite
  identificador como destino. Retomar cuando definamos mutabilidad.
- **Posición de errores en subexpresiones de interpolación** — se reporta
  la posición del string entero, no la posición interna del `{...}`
  ofensor. Aceptable para v0.1.
- **Patrones de match** — solo `Ident`, `_` (wildcard), `Ok(x)`, `Err(e)`.
  No hay patrones literales (`"active"`), rangos (`0..12`), tuples ni
  listas. El AST tampoco los modela todavía.
- **Struct literals (`User { id: 1, name: "x" }`)** — el AST no los
  modela. Por ahora la instanciación tiene que hacerse vía función
  constructora.
- **Listas y mapas literales (`[]`, `[1,2]`, `{"k": v}`)** — ni el lexer
  los reconoce especialmente ni el AST tiene `Expr::List`/`Expr::Map`.
- **Tipos compuestos en anotaciones (`List<T>`, `Map<K,V>`, `Str?`)** —
  `Stmt::Assign.type_` y `Param.type_` son `Option<String>`, solo
  nombres simples. (El `?` post-tipo SÍ se modela en campos de `type`
  vía `Field.nullable`, pero no en anotaciones de variables/parámetros.)
- **Error recovery** — el primer error mata el parseo. Sin paniqueo y
  resincronización todavía.

#### 2.4 Evaluador — PENDIENTE
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
