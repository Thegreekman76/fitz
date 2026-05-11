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
**Estado: COMPLETADA**

El corazón del lenguaje. Al final de esta fase, Fitz puede ejecutar
programas básicos.

El criterio de éxito se cumple: `cargo run -- run examples/phase2.fitz`
ejecuta el programa de referencia end-to-end (270 tests pasando, incluida
la deuda accionable de 2.3/2.4 cerrada).

Tras cerrar la fase se publicó **docs/guide.md v0.1**: guía pedagógica
en español, 13 capítulos, 11 ejemplos ejecutables en `examples/guide/`.
La guía solo documenta lo que el intérprete ejecuta hoy; crece con
cada feature que se cierre. **Regla operativa**: cualquier cambio al
proyecto exige verificar la guía y sus ejemplos antes de declarar el
trabajo cerrado.

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
- ~~**Operadores lógicos `and` / `or`**~~ ✓ cerrado tras Fase 2 — tokens
  emitidos por el lexer, precedencia en parser (`or` < `and` < `==`),
  short-circuit ya estaba implementado en el evaluador.
- ~~**`while` / `loop`**~~ ✓ cerrado tras Fase 2 — AST tiene las variantes,
  parser y evaluador funcionan, `break`/`continue` se capturan dentro del
  loop.
- **`for`** — queda como deuda. Necesita rangos (`0..10`) o listas como
  fuente de iteración; espera Fase 3. El parser emite error explícito.
- **Method calls (`expr.method(args)`)** — `Expr::Call` solo admite
  `name: String`. Cuando mute a `callee: Box<Expr>` se desbloquea. Por
  ahora, el parser tira error explícito.
- **Asignación a campos (`user.name = ...`)** — `Stmt::Assign` solo admite
  identificador como destino. Retomar cuando definamos mutabilidad.
- ~~**Posición de errores en subexpresiones de interpolación**~~ ✓ mejorado
  tras Fase 2 — ahora apunta al `{` específico y traslada errores del
  sub-parser. Limitación residual: un char menos de precisión por cada
  escape (`\n`, `\t`) anterior al error, porque no tenemos el source
  original.
- ~~**`return` sin expresión**~~ ✓ cerrado tras Fase 2 — `return` solo
  equivale a `return null` implícito.
- **Patrones de match** — soporta `Int`, `Float`, `Str`, `Bool`, `Null`
  (con negativos), `Ident`, `_`, y `Ok(x)`/`Err(e)` (estos últimos parsean
  pero el evaluador emite error hasta Fase 3). Faltan rangos (`0..12`),
  tuples y listas.
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

#### 2.4 Evaluador ✓
**Completado** — `src/evaluator.rs`, `src/value.rs`, `src/env.rs` con 113
tests pasando (92 evaluador + 12 value + 9 env). Recorre el AST y ejecuta
el programa.

**Alcance de 2.4 (lo que SÍ se implementa):**
- Valores en runtime: `Int`, `Float`, `Str`, `Bool`, `Null`, `Function`
  (con closures), `Builtin`, `Type` (inerte hasta Fase 3).
- Operaciones: aritmética con promoción Int↔Float, comparación numérica
  y de strings, igualdad con coerción, `and`/`or` con short-circuit,
  unario `-`. División por cero → error explícito.
- Strings: concatenación con `+`, interpolación de expresiones con `{...}`.
- Control de flujo: `if`/`else`/`else if` como expresión y como sentencia.
- Funciones: `fn`/`=>`, closures con captura léxica, recursión, validación
  de aridad, `return` propagado vía `EvalSignal::Return`.
- `match` con patrones `Ident` (bind) y `Wildcard`.
- `type` registrado en el env como marcador inerte.
- Builtins: `print` (sigue la semántica de Python — args separados por
  espacio, newline final).
- Manejo unificado de errores y control de flujo vía `EvalSignal`
  (`Error` / `Return` / `Break` / `Continue`). Signals "huérfanos" (return
  fuera de función, break/continue fuera de loop) se reportan al usuario.

**Fuera de alcance — deuda explícita, retomar después:**
- ~~**Operadores `and`/`or`**~~ ✓ cerrado tras Fase 2 — lexer emite tokens,
  parser inserta en la cadena de precedencia, evaluador con short-circuit.
- ~~**`break` / `continue`** sin loops~~ ✓ cerrado tras Fase 2 —
  `while`/`loop` los capturan correctamente vía `run_loop_body`.
- **Patrones `Ok(x)` / `Err(e)`** — el evaluador emite error explícito
  citando "requiere el tipo Result (Fase 3)". Bloqueado hasta tener
  `Value::Result` o similar.
- **Field access (`obj.campo`)** — error explícito "requiere tipos custom
  instanciados (Fase 3)" porque sin struct literals no hay struct values
  en runtime.
- **Instanciación de tipos (`User { id: 1, name: "x" }`)** — el AST no
  los modela. `Value::Type` está listo para recibir instancias cuando se
  agregue.
- **HTTP endpoints (`@get`, etc.)** — Fase 4. El evaluador devuelve error
  explícito si se evalúan.
- **Async** — `is_async` en `FnDef` se ignora silenciosamente. Fase 4.
- **Anotaciones de tipo** — `let x: Int = ...` parsea, pero el evaluador
  ignora la anotación. El tipado gradual sin checks runtime es la
  intención; el type checker estático llega en Fase 5.
- **Scope de bloques (if/match/función)** — los bloques de `if` no crean
  scope nuevo (estilo Python). Variables definidas adentro persisten
  afuera. Si esto trae sorpresas, revisamos.
- **Overflow numérico** — `Int + Int` puede overflowear (paniquea en
  debug, wrappea en release). Sin `checked_*` por ahora.

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
**Estado: EN CURSO**

Agregar las features que hacen a Fitz expresivo. La fase está dividida
en cinco pasos; cada uno cierra una pieza independiente y suma su
capítulo a la guía.

### Pasos

#### 3.1 Listas, mapas, rangos ✓
**Completado** — el lenguaje ya tiene colecciones básicas y `for`.

- AST nuevo: `Expr::List`, `Expr::Map`, `Expr::Range`, `Expr::Index`,
  `Stmt::For`, `Pattern::Range`.
- Parser: literales `[...]` y `{...}`, rangos `start..end` con
  precedencia entre comparación y suma, indexing postfix `xs[i]`,
  `for var in iter { ... }`, patrón de rango `0..10` en match.
- Evaluator: `Value::List`, `Value::Map`, `Value::Range`; iteración
  para `for` sobre listas y rangos; matching de rango contra Int;
  errores explícitos para índices fuera de rango, claves no
  encontradas, tipos no indexables.
- Builtin: `len` para List/Map/Str/Range.

Tests del proyecto al cerrar 3.1: 366 (270 al cerrar Fase 2 + 96
nuevos repartidos entre ast, parser, value y evaluator).

Guía: capítulo 9 "Listas, mapas y rangos" sumado, capítulo de Match
extendido con patrones de rango, capítulo de Loops limpiado de la
deuda de `for`. Ejemplo nuevo: `examples/guide/09-listas-mapas.fitz`.

**Deuda explícita — retomar después:**
- **Mutación de listas** (`push`/`pop`/asignación a `xs[i]`) — espera
  3.4 (method calls). Por ahora las listas son inmutables desde el
  código fuente.
- **`for` sobre mapas** — necesita el tipo `Pair`/`entry`. El
  evaluador emite error explícito por ahora.
- **Índices negativos** (`xs[-1]` estilo Python) — sin soporte; el
  evaluador corta con "índice negativo".
- **`Range` indexable** (`(0..10)[3]`) — no soportado, sin uso claro
  hasta no tener method calls.
- **`Str` indexable** — pendiente decisión sobre la unidad (char vs
  byte vs grafema).
- **Rango inclusivo `..=`** — sin soporte, se suma si aparece la
  necesidad.

#### 3.2 Tipos custom instanciables ✓
**Completado** — los tipos declarados con `type` ahora se pueden
instanciar y consultar.

- AST nuevo: `Expr::StructLit { type_name, fields }`.
- Parser: `Nombre { campo: expr, ... }` como expresión, en cualquier
  posición salvo las condiciones directas de `if`/`while`/`for`/`match`
  (donde el `{` arranca un bloque). En esas posiciones, el flag
  `no_struct_literal` corta con un error explícito sugiriendo
  paréntesis. Adentro de `(...)`, `[...]`, args de llamada e
  indexing, los struct literals están permitidos sin envolver.
- Evaluator: `Value::Instance { type_name, fields }`. Al instanciar
  se valida campo extra → error, falta de campo sin default ni
  nullable → error; se aplican defaults (evaluados en el env de
  instanciación) y nullables (`Null` por omisión); los campos
  quedan ordenados según la declaración del `type`. Field access
  (`obj.campo`) implementado sobre `Value::Instance`. Igualdad
  estructural (mismo tipo, mismos campos en orden, coerción
  Int↔Float adentro).
- Cierra deuda de 2.3 (struct literals no parseaban) y de 2.4 (field
  access e instanciación en runtime).
- Capítulo 12 de la guía sale del estado "preview" y pasa a documentar
  el feature real; se sumó al ejemplo
  `examples/guide/12-type.fitz` la parte de instanciación y acceso a
  campos.

Tests del proyecto al cerrar 3.2: 405 (366 al cerrar 3.1 + 39 nuevos
repartidos entre ast, parser, value y evaluator).

**Deuda explícita — retomar después:**
- **Mutación de campos** (`user.name = "x"`) — espera 3.4
  (asignación a destinos no-identificador). Hoy `Stmt::Assign.name`
  es `String`; cuando mute a un destino más rico se desbloquea.
- **Métodos sobre instancias** (`user.greet()`) — espera 3.4
  (mutación de `Expr::Call` a `callee: Box<Expr>`).
- **Chequeo de tipos en runtime** — descartado por diseño (tipado
  gradual). Las anotaciones se guardan pero no se validan en
  runtime; el chequeo estático llega con el compilador en Fase 5.
- **Tipos compuestos en anotaciones de campo** (`emails: List<Str>`)
  — sigue siendo deuda de 2.3 (`Field.type_` es `String` simple).

#### 3.3 Result + Ok/Err + `?` ✓
**Completado** — el lenguaje maneja errores estilo Rust, sin
excepciones.

- AST nuevo: `Expr::Ok(Box<Expr>)`, `Expr::Err(Box<Expr>)`,
  `Expr::Try(Box<Expr>)` (operador `?` postfix).
- Parser: `Ok` y `Err` se detectan como keywords contextuales
  cuando aparecen como receptor de llamada (aridad 1 obligatoria);
  `?` se parsea en la cadena de postfix junto a `.`, `(...)`,
  `[...]`, encadenable con field access (`get(id)?.name`).
- Value: variante propia `Value::Result(ResultVariant)`, con
  `ResultVariant::Ok(Box<Value>)` y `ResultVariant::Err(Box<Value>)`.
  Display: `Ok(v)` / `Err(e)`, strings con comillas adentro (mismo
  criterio que List/Map/Instance). Igualdad estructural con la
  coerción Int↔Float recursiva.
- Evaluator: `Ok`/`Err` envuelven el inner evaluado. `?` desempaqueta
  cuando es `Ok`, y emite `EvalSignal::Return(Value::Result(Err))`
  cuando es `Err`, reusando la maquinaria existente de `return`.
  Sobre un valor que no es `Result`, `?` corta con error de tipo.
- Patrones `Ok(x)`/`Err(e)` en `match` ahora matchean contra
  `Value::Result` y bindean el inner — cierra deuda explícita de
  2.4.

Tests del proyecto al cerrar 3.3: 441 (405 al cerrar 3.2 + 36
nuevos repartidos entre ast, parser, value y evaluator —
incluyendo dos tests end-to-end con `find_user` y `divide` desde
fuente).

Guía: capítulo 13 nuevo "Result y manejo de errores" entre el cap
de Tipos y el de Errores del intérprete. Renumeración: cap 13
(Errores y mensajes) → 14; cap 14 (Qué sigue) → 15. Ejemplo nuevo
`examples/guide/12-result.fitz`; el antiguo `12-errores.fitz`
renombró a `13-errores.fitz`.

**Deuda explícita — retomar después:**
- **Mensaje específico para `?` fuera de función** — hoy reutiliza
  el signal de `return`, así que el usuario ve `` `return` solo
  puede usarse adentro de una función ``. Querido: signal propio
  (`EvalSignal::TryOutsideFunction`) con texto dedicado.
- **`Ok(_)` / `Err(_)` con wildcard real** — hoy `_` adentro del
  patrón funciona como nombre, no como wildcard, así que ensucia
  el scope con una variable llamada `_`. Solución: variantes
  `Pattern::OkWildcard` / `Pattern::ErrWildcard`.
- **Anotación `Result<T>` en parámetros y retorno** — sigue siendo
  deuda de 2.3 (`Param.type_` y `FnDef.return_type` son `String`
  simple, no admiten genéricos).
- **Chequeo de tipo del retorno cuando se usa `?`** — descartado por
  diseño hasta el type checker estático (Fase 5). Tipado gradual.

#### 3.4 Funciones anónimas + higher-order + method calls ✓
**Completado** — el paso más grande de Fase 3. Mutación del AST para
método calls, fn anónimas como expresión, dispatch por tipo del
receptor, primera tanda de built-ins, y representación compartida
(`Rc<RefCell<>>`) para listas, mapas y campos de instancia.

- AST mutado:
  - `Expr::Call` → `{ callee: Box<Expr>, args }`. Cierra deuda de 2.3.
  - Nueva `Expr::FnExpr { params, body }` — `fn(x) => x*2` y
    `fn(x) { return x*2 }` como expresión (sin nombre).
  - `Stmt::Assign` → `{ target: AssignTarget, type_, value }` con
    `AssignTarget::Ident(String)` y
    `AssignTarget::Field { object, field }`. Cierra deuda de 2.3 y 3.2.
- Parser:
  - `Token::Fn` seguido de `(` en posición de expresión → `FnExpr`.
    `fn name(...)` sigue siendo `Stmt::FnDef`.
  - `postfix` sin restricción sobre el callee: `xs.map(f)`,
    `(fn(x)=>x+1)(2)`, `find(id)?.name`, todo encadenable.
  - `parse_expr_or_assign_stmt`: parsea el LHS como expresión y
    decide después si era `Ident = ...`, `Ident : Tipo = ...`,
    `expr.campo = ...` o expr-stmt.
- Value:
  - `Value::List(Shared<Vec<Value>>)`,
    `Value::Map(Shared<Vec<(Value, Value)>>)`,
    `Value::Instance { fields: Shared<Vec<(String, Value)>>, ... }`,
    donde `Shared<T> = Rc<RefCell<T>>`. Constructores
    `Value::new_list`, `new_map`, `new_instance`.
  - Alias por referencia (estilo Python/JS): pasar una lista a una
    función o guardarla en un campo no clona. `.push(...)` y
    `user.name = "x"` se ven a través de todos los aliases.
  - Display, igualdad estructural y coerción `Int↔Float` mantienen
    su comportamiento observable.
- Evaluator:
  - `Expr::Call`: si el callee es `Expr::Field`, hace **method
    dispatch** por `(tipo del receptor, nombre del método)`. Si no,
    evalúa el callee como cualquier expresión e invoca el `Value`
    resultante (`Function` o `Builtin`).
  - `Expr::FnExpr`: crea `Value::Function` con closure sobre el env
    actual. Sin nombre y sin binding en el env — pura expresión.
  - `Stmt::Assign` con `AssignTarget::Field`: evalúa el objeto,
    valida `Value::Instance`, muta el campo. Errores explícitos si
    no es instancia o el campo no existe.
- Built-ins (primera tanda):
  - `List`: `push(v)` muta, `pop()` muta, `map(fn)`, `filter(fn)`,
    `find(fn) -> Result`, `len()`.
  - `Map`: `get(k) -> Result`, `has(k) -> Bool`, `keys() -> List`,
    `values() -> List`, `len()`.
  - `Str`: `len() -> Int`, `upper() -> Str`, `lower() -> Str`.
- Tests del proyecto al cerrar 3.4: 472 (441 al cerrar 3.3 + 31
  nuevos: 3 en ast, 1 ajuste en parser, y 27 en evaluator entre
  fn anónimas, mutación de campos, método dispatch, built-ins y
  el E2E del criterio de éxito de Fase 3).

Guía: capítulo 13 nuevo "Métodos y mutación" entre Tipos y Result.
Renumeración 13→14, 14→15, 15→16. Cap 9 (Listas/mapas) y cap 12
(Tipos) limpiados de deuda. Cap 11 (Funciones) suma sección
"Funciones anónimas inline". Ejemplo nuevo
`examples/guide/13-metodos.fitz`; `12-result.fitz` renombró a
`14-result.fitz` y `13-errores.fitz` a `15-errores.fitz`.

**Deuda que se cierra:**
- 2.3 "method calls (`expr.method()`)" — el parser ya construye
  `Expr::Call { callee: Expr::Field {...}, args }`.
- 2.3 "asignación a campos (`user.name = ...`)" — `Stmt::Assign.target`
  admite `AssignTarget::Field`.
- 3.1 "mutación de listas (`push`/`pop`)" — métodos vivos, con alias
  compartido. `xs[i] = v` queda como deuda explícita (abajo).
- 3.2 "mutación de campos" — vivo, visible vía alias.
- 3.2 "métodos sobre instancias" (la infraestructura): el dispatch
  está; métodos custom declarados por el usuario sobre `type`
  siguen siendo deuda (queda como Fase 5+).

**Deuda explícita — retomar después:**
- **Asignación a índice (`xs[0] = v`)** — `AssignTarget` admite
  `Ident` y `Field` pero no `Index`. Habilitarlo es agregar la
  variante al AST, una rama en el parser y otra en el evaluator
  (con el mismo patrón de `Field`). Fácil; se suma cuando importe.
- **Métodos custom sobre `type`** — `type User { ... fn greet() => ... }`
  no se parsea. El dispatch del evaluador ya prevé sumar otra
  fuente de lookup sin retoques.
- **`return` adentro de un brazo de `match` como expresión** — hoy
  el cuerpo de cada brazo es expresión, no statement, así que
  `Ok(u) => return Ok(u)` rompe en el parser. Salvable con
  `match`-statement (variante con bloque por brazo) o con
  expression-with-return. Decisión pendiente.
- **Encadenamiento multi-línea** — `xs.map(...)\n.filter(...)` corta
  en el newline porque el parser termina la sentencia. Lo arreglamos
  cuando moleste (newline-soft-after-postfix-token).
- **`Pattern::OkWildcard` / `ErrWildcard`** — sigue deuda de 3.3.
- **Mensaje propio para `?` huérfano** — sigue deuda de 3.3.
- **Anotaciones compuestas (`List<T>`, `Result<T>`, `Map<K,V>`)** —
  sigue deuda de 2.3.

#### 3.5 Módulos / `import`
**Pendiente** — infraestructural.

- File loading, name resolution, namespaces.

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
