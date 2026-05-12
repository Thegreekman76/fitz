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
**Estado: COMPLETADA**

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

#### 3.5 Módulos / `import` ✓
**Completado** — el último paso de Fase 3. Carga de archivos, dos
formas de importar, namespaces aislados, cache por path canonicalizado
y detección de ciclos.

- AST nuevo: `Stmt::Import { path: Vec<String> }` y
  `Stmt::FromImport { path: Vec<String>, names: Vec<String> }`.
- Parser:
  - `import foo` / `import sub.foo.bar` — paths puntudos
    acumulando segmentos. El parser garantiza al menos un
    segmento.
  - `from foo import a, b, c` — lista de nombres (al menos
    uno; acepta trailing comma). Path puede ser punteado
    (`from sub.foo import bar`).
- Value: `Value::Module { name, env: EnvRef }`. Display
  `<module name>`. Igualdad por identidad del `Rc<RefCell>>`
  del env (dos imports del mismo archivo dan módulos iguales
  porque el cache devuelve el mismo Rc).
- Evaluator:
  - Loader como thread_local (`Option<Loader>`). Estado:
    `base_dir` (rotativo al cargar módulos anidados, vuelve
    al salir), `loading: Vec<PathBuf>` (stack para ciclos),
    `cache: HashMap<PathBuf, Value>` (por path canonicalizado).
  - Resolución relativa al `base_dir` actual: `["sub","foo"]`
    → `<base>/sub/foo.fitz`. `canonicalize` valida existencia
    y normaliza para cache + cycle.
  - Eager: al ver `Stmt::Import`/`FromImport`, carga, lexea,
    parsea y evalúa el archivo entero en un env aislado antes
    de seguir. El env del módulo registra builtins propios.
  - `import` bindea bajo el último segmento del path
    (`sub.foo` → `foo`). `from import` bindea cada nombre
    directo y NO expone el módulo.
  - Field access sobre `Value::Module` resuelve en el env
    del módulo (`utils.foo` → `utils.env.get("foo")`).
  - Method dispatch sobre `Value::Module` busca el método en
    el env del módulo y lo invoca con `invoke_value`
    (reuso del path de llamada normal).
- `eval_with_base(program, base_dir)` como entrada explícita;
  `eval(program)` queda como wrapper que usa el cwd. `main.rs`
  pasa el directorio del archivo `.fitz` que se está ejecutando.
- Cierra deuda original de 2.1: los tokens `Import` y `From`
  del lexer dejan de ser huérfanos.
- Tests del proyecto al cerrar 3.5: 503 (472 al cerrar 3.4 +
  31 nuevos: 3 en ast, 10 en parser, 3 en value, 15 en
  evaluator — incluyendo E2E con fixtures en tempdir).

Guía: capítulo 16 nuevo "Módulos" entre Errores (15) y "Qué
sigue" (que pasa a 17). Ejemplo nuevo
`examples/guide/16-modulos.fitz` + auxiliar
`examples/guide/guide_utils.fitz` (sin numeración para que el
binding generado por `import` sea un identificador válido).

**Deuda explícita — retomar después:**
- **Qualified struct literals** (`foo.User { ... }`) — el parser
  de struct literal espera `Ident { ... }`. Para usar el literal,
  hoy hay que `from foo import User`. Se puede extender el parser
  para aceptar paths como type name si la asimetría molesta.
- **`as` / aliasing** (`import foo as f`, `from foo import bar as b`)
  — sin soporte. Cuando importe se suma; es palabra contextual,
  no necesita token nuevo.
- **`pub` / privacidad** — hoy todo top-level del módulo es
  público. Si más adelante queremos marcar internos, se suma
  `pub` o convención `_underscore` validada.
- **stdlib** (`from fitz import http`) — sin soporte. El prefijo
  `fitz/` no es especial todavía; se reserva para Fase 4+.
- **Multi-línea en `from ... import (...)` con paréntesis** — sin
  soporte. La lista de nombres tiene que ir en una línea.
- **Imports anidados en bloques/funciones** — hoy nada lo
  prohíbe sintácticamente, pero el caso no está pensado: el
  binding queda en el env donde se ejecuta el import. Si
  conviene restringir a top-level, se suma flag en el parser.

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
**Estado: COMPLETADA**

La feature que diferencia a Fitz. HTTP como ciudadano de primera clase.
Cinco pasos cerrables; cada uno suma su tanda de tests y, al final, su
capítulo a la guía.

### Decisiones de diseño (tomadas antes de arrancar)

- **Runtime HTTP**: Axum + tokio (multi-thread). Bypasseamos los
  extractors tipados (nuestros handlers son `Value::Function`, no `fn`
  de Rust) — `Router::route` + closure que recibe `Request<Body>` y
  devuelve `Response`.
- **Decoradores en AST**: `decorators: Vec<Decorator>` adentro de
  `FnDef`. Genérico, no atado a HTTP. El evaluator despacha por nombre
  (`@get`/`@post`/`@put`/`@delete` → registran ruta; `@server` →
  configura; cualquier otro → error explícito). Por ahora args son
  solo positionals (named args queda como deuda — destrabaría
  `@server(port: 8080)`).
- **Arranque del servidor**: automático al final de eval si hay rutas
  registradas. `@server(...)` configura host/port, no dispara nada.
  Coincide con el syntax-spec.
- **Bridge sync/async**: Fitz sigue siendo intérprete síncrono;
  `is_async` se sigue ignorando. Cada request lee el body async
  (axum/tokio) y corre el handler Fitz vía `spawn_blocking` para no
  bloquear el reactor. Para evitar `Rc`-cross-thread, vamos por un
  thread dedicado al intérprete + canal de tareas. Async real adentro
  del lenguaje es deuda explícita (Fase 4.x o 5).
- **Serialización JSON**: automática desde `Value`. Primitivos
  obvios; `List` → array; `Map` → object (claves Str obligatorias);
  `Instance` → object con campos en orden; `Result::Ok(v)` → 200
  con `v`; `Result::Err(e)` → 500 con `{"error": e}`; tipos no
  serializables → error explícito.
- **Deserialización del body**: el handler declara `body: TipoCustom`;
  el runtime valida el JSON contra el `type` (mismas reglas que
  `StructLit`) y construye `Value::Instance`. Errores → 400.
- **Path params**: convertidos según el tipo del parámetro
  (`id: Int` → parsear como int; fallo → 400). Sin anotación
  default a `Str`. Query params quedan como deuda.

### Pasos

#### 4.1 — Decoradores genéricos sobre FnDef ✓
**Completado** — refactor preparatorio. El AST y el parser hablan
decoradores apilables; el cableado real con el runtime HTTP llega
en 4.2.

- AST: nueva `Decorator { name: String, args: Vec<Expr> }`;
  `Stmt::FnDef` gana `decorators: Vec<Decorator>`. Eliminados
  `Stmt::HttpEndpoint` y `HttpMethod` (cumplido el TODO de Fase 4
  que tenía el AST desde Fase 2).
- Parser: `parse_decorated_fndef` apila uno o más
  `@nombre(args...)` antes de `[async] fn ...`. Args usan
  `parse_call_args` — son expresiones cualquiera. Cada decorator
  exige paréntesis (incluso vacíos: `@server()`), mantiene la
  sintaxis predecible.
- Evaluator: `Stmt::FnDef` con `decorators` no vacío corta con
  error explícito mencionando los nombres concretos
  (`@get`/`@server`/etc.) y "requieren Fase 4.2 — runtime HTTP".
  Es un puente intencional: AST y parser ya soportan la sintaxis,
  la semántica espera el runtime.

Tests al cerrar 4.1: 511 (503 al cerrar 3.5 + 8 nuevos: 3 en ast,
5 netos en parser, 2 en evaluator).

**Observación de diseño cerrada**: el path en `@get("/users/{id}")`
llega al evaluator como `Expr::StrInterp` (porque `{id}` es sintaxis
de interpolación de Fitz). En 4.2 los `StrPart::Expr(Ident(...))`
del path se reconocerán como path params sin necesidad de un mini
parser dedicado dentro del decorator. Es una buena noticia, no un
bug — reusa la maquinaria existente.

#### 4.2 — Runtime HTTP mínimo (GET + Result handling) ✓
**Completado** — server axum + tokio + bridge sync/async funcionando.
GET con path params tipados, serialización JSON automática, Result
auto-handling. `cargo run -- run server.fitz` levanta el server.

- Nuevo módulo `src/http.rs` con:
  - `HttpRegistry` + `RouteSpec` + `RouteMeta` + `HttpMethod`.
  - `with_active_registry(...)` para que el evaluator vea un registry
    durante eval vía thread_local.
  - `parse_path_template`: traduce `Expr::Str` o `Expr::StrInterp` del
    decorator a path axum (`/users/{id}`) + lista de param names.
  - `value_to_json` + `value_to_outcome`: serialización total con
    Result auto-handling (`Ok(v)`→200, `Err(e)`→500 con `{"error":e}`),
    tipos opacos (Function/Module/Type/Range) → 500 explícito.
  - `coerce_path_param`: convierte string crudo a Int/Float/Str/Bool
    según el tipo declarado del parámetro del handler. Falla → 400.
  - `build_router` + `serve(registry, addr)`: arranca axum en un
    std::thread spawneado; el thread main entra al
    `run_interpreter_loop`. Bridge vía
    `mpsc::UnboundedSender<InterpTask>` + `oneshot::Sender<HandlerOutcome>`
    por request. Graceful shutdown con Ctrl-C.
- Evaluator: cuando ve `Stmt::FnDef` con decorator `@get`/`@post`/
  `@put`/`@delete` y hay registry activo, valida (1 arg path, path
  starts with `/`, cada `{x}` tiene su parámetro en el handler) y
  registra una `RouteSpec`. Sin registry → error explícito con
  sugerencia "ejecutá con `fitz run`". Decoradores no implementados
  (`@server`, `@patch`, etc.) → error con el nombre.
- `main.rs`: envuelve `eval_with_base` en `with_active_registry`. Si
  después de eval el registry tiene rutas, llama a `http::serve` en
  127.0.0.1:3000 (`@server(...)` configurable llega en 4.4).
- Axum 0.8 (no 0.7): la sintaxis `{id}` del syntax-spec mapea directa
  al matcher de axum. Bumpeo deliberado.

Tests al cerrar 4.2: 558 (511 al cerrar 4.1 + 47 nuevos repartidos
en `src/http.rs` — 28 unit tests del módulo + 7 E2E con `Router::oneshot`
sobre `LocalSet` — y 8 nuevos en `evaluator::tests` para el flujo de
registro). Validado manualmente: server real con `curl` responde 200,
400, 500 según corresponde.

**Decisión de threading documentada en código**: el intérprete vive
en el thread main (donde corrieron los `Rc<RefCell<>>` del eval), no
en un thread spawneado — `Value` no es `Send`. Tokio corre en un
std::thread propio. Lo que cruza el canal son strings y números.

**Limitaciones de 4.2 (deuda explícita, no nueva):**
- Solo GET/POST/PUT/DELETE sin body — el body llega en 4.3.
- Sin query params, sin headers, sin status codes custom.
- `@server(...)` parsea pero no hace nada (4.4).
- `async fn` se acepta sintácticamente pero no aporta nada en
  runtime (deuda vieja: async real adentro del lenguaje).

#### 4.3 — Body + deserialización JSON ✓
**Completado** — handlers POST/PUT/DELETE (y cualquier método)
pueden declarar un body. El runtime parsea el JSON, valida contra
el `type` declarado y construye una `Value::Instance` antes de
invocar al handler. Body sin anotación de tipo llega como
`Value` libre (Map/List/primitivos).

- AST/Parser: sin cambios (decorators ya genéricos desde 4.1, body
  es simplemente un parámetro del handler).
- `http::RouteSpec.body_param: Option<BodyParam>` y
  `RouteMeta.expects_body: bool`. `BodyParam` lleva nombre, el
  `Value::Type` declarado (clonado del env durante registro) y el
  nombre del tipo para mensajes.
- `http::json_to_value`: deserialización total sin schema. Números
  enteros → Int, con parte fraccional → Float, objects → Map con
  claves Str, arrays → List.
- `http::json_to_instance`: con schema — valida contra los campos
  del `type` (faltantes con default OK, faltantes nullables → Null,
  extras → 400 mencionando el nombre, body no objeto → 400). Los
  defaults soportan literales constantes; defaults complejos
  (expresiones que usan otros bindings) son deuda explícita.
- `http::InterpTask` gana `body: Vec<u8>`. `handle_task` parsea el
  body antes de armar args; body roto o inválido → 400 con mensaje;
  body vacío con handler que lo espera → 400 ("body requerido").
- `build_method_router` ahora tiene 4 ramas (path × body) porque
  los extractors de axum aparecen como args del handler. El helper
  `wrap(method, h)` evita repetir el match por verbo en cada rama.
- Convención de registro: cada parámetro del handler es path param
  (su nombre está en `path_params`) o body. Máximo un body por
  handler — más de uno → error explícito al registrar.
- `serde_json` con feature `preserve_order` para que el JSON de
  respuesta respete el orden declarado del `type`.

Tests al cerrar 4.3: 581 (558 al cerrar 4.2 + 23 nuevos: 15 en el
módulo http — 3 de `json_to_value`, 6 de `json_to_instance`,
6 de `handle_task` con body — 4 E2E nuevos sobre `Router::oneshot`,
y 4 nuevos del evaluator validando registro de body).

Validado manualmente con `curl`:
  - POST `/users` body válido → 200 con orden de campos preservado.
  - Campo nullable faltante → `email: null`.
  - Campo extra → 400 con nombre.
  - Body JSON roto → 400 con error de parseo.
  - PUT con path param + body mezclando ambos.
  - POST con body sin anotación → echo como Value libre.

**Limitaciones de 4.3 (deuda explícita):**
- Defaults complejos (no literales) en campos del body — fallan
  silencioso al validar; mensaje sugerente "pasalo explícito".
- Sin validación de Content-Type: cualquier body se intenta como
  JSON. Multipart/form-data, urlencoded → cuando hagan falta.
- Sin validación de tipos compuestos en campos del body (`emails: List<Str>`)
  — sigue siendo deuda vieja del type system (Fase 5).
- Query params siguen sin soporte.

#### 4.4 — `@server(...)` configuración ✓
**Completado** — el programa puede declarar puerto y host del
server con `@server(port, host)` sobre cualquier `fn` (típicamente
`fn main()` como placeholder; la fn queda definida en el env pero
no se ejecuta automáticamente).

- `http::ServerConfig { host: String, port: u16 }` con
  `default_addr()` (127.0.0.1:3000) y `to_socket_addr()` (parsea
  el host como IP literal, sin DNS).
- `HttpRegistry.server_config: Option<ServerConfig>` y
  `resolved_config()` que devuelve el explícito o el default.
- `http::set_server_config` impone unicidad: dos `@server` →
  `Err` con el config previo.
- Evaluator `register_server_config`:
  - 0/1/2 args positionals; >2 → error.
  - Port: Int en `[1, 65535]`, otro tipo/literal → error.
  - Host: Str literal que parsea como IP (IPv4/IPv6), otro → error.
  - Sin registry activo → error explícito.
- `main.rs` usa `registry.resolved_config().to_socket_addr()` antes
  de llamar a `http::serve`.

Razón de diseño documentada en código: `@server` se aplica como
decorator sobre una fn (la fn queda definida pero no se ejecuta).
Mantiene uniformidad con `@get/@post/etc` y evita un caso especial
en el parser. La forma con named args (`@server(port: 8080)`) sigue
siendo deuda — espera a que el lenguaje tenga named args en `Call`.

Tests al cerrar 4.4: 595 (581 al cerrar 4.3 + 14 nuevos: 5 en
`http::tests` para `ServerConfig`/`set_server_config`/
`resolved_config`, 9 en `evaluator::tests` cubriendo registro
válido, defaults parciales, errores de tipo, rango, IP inválida,
doble decorator).

Validado a mano: `@server(8181, "127.0.0.1")` levanta en 8181 y
3000 no responde.

#### 4.5 — Guía + ejemplos + cierre de fase ✓
**Completado** — cierre formal de Fase 4 con documentación viva.

- Capítulo 17 nuevo "HTTP nativo" entre Módulos (16) y Qué sigue
  (renumerado a 18). Cubre: primer endpoint, los cuatro verbos,
  path params tipados, body con `type` o libre, serialización JSON
  automática, `@server(port, host)`, integración con `Result + ?`,
  modelo de threading (intérprete sync + tokio en thread aparte),
  qué todavía no anda (`async` real, status codes custom, query
  params, headers, middleware, named args).
- `examples/guide/17-http.fitz` ejecutable: mini API con `/`,
  `/users`, `/users/{id}`, `POST /users`. Memoria del server =
  env del programa.
- `examples/server.fitz` reescrito como criterio de éxito de Fase 4:
  CRUD completo (GET/POST/PUT/DELETE) con `Result + ?`. Validado
  a mano contra `curl` end-to-end.
- Guía pre-cap-17: actualizado preámbulo (fecha, número de tests,
  HTTP movido de "no funciona aún" a "feature core"), índice
  reorganizado en 8 partes, "Qué cubre" y "Qué no anda"
  reactualizados, cierres de cap obsoletos limpiados, deuda del
  cap 5 (métodos sobre strings) corregida (los básicos sí existen
  desde 3.4).
- Cierre cap 18 "Qué sigue" reescrito a post-Fase-4: lo aprendido
  incluye HTTP, el "más adelante" apunta a Fase 5 (compilador) y
  Fase 6 (ecosistema).

**Decisión de implementación documentada en la guía**: `return`
adentro de un brazo de match no parsea (deuda viva del 3.4) — el
ejemplo del cap 17 usa `return match { ... }` con el valor directo
en cada brazo. Sigue siendo deuda explícita; cuando se cierre,
ambas formas van a funcionar.

Tests al cerrar 4.5: 595 (sin cambios respecto de 4.4 — el cierre
de fase es documentación + ejemplos, sin código nuevo).

### Deuda explícita ya identificada para Fase 4

- **Named args en decorators** (`@server(port: 8080, host: "0.0.0.0")`)
  — hoy sólo positionals (`@server(8080, "0.0.0.0")`).
- **Async real en el lenguaje** — `await`, futures, async fn que de
  verdad sea async. Probablemente Fase 4.x o 5.
- **Response builder rico** — `return 401 { ... }` del syntax-spec,
  status code custom, headers, content-type. Sigue siendo deuda.
- **Query params** — `?page=1&size=10`. Sin soporte en 4.x.
- **Middleware** — auth, logging, CORS via decoradores apilables.
- **Hot reload** del server al cambiar el `.fitz`.

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
**Estado: 5a COMPLETADA / 5b EN CURSO (5b.2 cerrado)**

Plan aprobado: dos mitades cerrables.
- **5a — Type checker estático** (sobre el intérprete actual) ✓
  Cerrado al cerrar el paso 5.4: el checker recorre lexer → parser →
  resolución de anotaciones → expresiones (synthesis, llamadas,
  return, Result/`?`/match exhaustivo, métodos built-in,
  FnExpr.ret inferido, Index). `fitz run` corre el checker por
  default y aborta si hay errores; `--no-typecheck` lo salta.
- **5b — Codegen a binario nativo** — backend elegido:
  **transpile-a-Rust** sobre Cranelift/LLVM. Razones decisivas:
  reuso de toda la infra del compilador, async real cuando llegue
  (sin escribir runtime propio), cross-compile a todos los targets
  de rustc, y la posibilidad de mapear handlers `@get`/`@post` a
  `async fn` axum sin trabajo extra cuando llegue 5b.6. Trade-off:
  compile times = rustc. Se divide en siete sub-pasos cerrables
  (5b.1 → 5b.7) con criterio de "hello world compilado" hoy y
  CRUD HTTP compilado al cerrar 5b.7.

### Pasos

#### 5.1 — TypeExpr en AST y parser ✓
**Completado** — refactor preparatorio del checker. El AST y el
parser ahora modelan tipos compuestos; el evaluator y el runtime
HTTP los consumen sin cambiar comportamiento observable.

- AST nuevo: `TypeExpr` con tres variantes:
  - `Named(String)` — `Int`, `Str`, `User`.
  - `Generic { name, args }` — `List<Int>`, `Map<Str, User>`,
    `Result<List<User>>`, anidable.
  - `Nullable(Box<TypeExpr>)` — sufijo `?` (`User?`, `List<Int>?`).
  - Helpers: `display_name()` (reproduce la forma del fuente),
    `head_name()` (cabeza ignorando genéricos y nullables, para que
    el runtime HTTP resuelva tipos custom), `is_nullable()`.
- AST refactor: `Param.type_`, `Stmt::Assign.type_` y
  `Stmt::FnDef.return_type` pasan de `Option<String>` a
  `Option<TypeExpr>`. `Field.type_` pasa de `String` a `TypeExpr`,
  y el flag `Field.nullable: bool` se elimina — la nullabilidad
  vive adentro como `TypeExpr::Nullable(...)`.
- Parser: nueva regla `parse_type_expr` (gramática
  `atom '?'?`, `atom = Ident generic_args?`). Reemplaza los tres
  call sites (`parse_optional_type_annotation`,
  `parse_optional_return_type`, field type adentro de
  `parse_typedef`). El lexer ya emitía `>` como `Token::Gt` único
  (no hay `>>` como un solo token), así que `Result<List<Int>>`
  se cierra consumiendo dos `Gt` separados sin trabajo extra.
- Evaluator: migrado para usar `head_name()` al resolver el
  `Value::Type` declarado del body param de un handler HTTP, y al
  empaquetar tipos de path params (que siguen siendo primitivos
  para `coerce_path_param`). Sin cambios de semántica en runtime
  — las anotaciones se siguen ignorando, exactamente como antes.
- http.rs: misma migración + `field.nullable` reemplazado por
  `field.type_.is_nullable()`.

Cierra deuda 2.3 "tipos compuestos en anotaciones
(`List<T>`, `Map<K,V>`, `Str?`)" a nivel sintáctico.

Tests al cerrar 5.1: 614 (595 al cerrar 4.5 + 19 nuevos: 6 en
ast.rs cubriendo `display_name`/`head_name`/`is_nullable` y
formas anidadas; 13 en parser.rs cubriendo `let x: T = ...`,
params/return, fields, nullable de generic vs adentro del
generic, errores `List<>` / generic sin cerrar / `:` sin tipo,
y round-trip de display sobre `Map<Str, Result<List<User>?>>`).

**Deuda explícita — retomar después:**
- **Función como tipo** (`fn(Int) -> Int` como `TypeExpr`) — no
  se modela todavía. Se suma cuando el checker (5.3) lo necesite
  para callbacks tipados.
- **Validación semántica** (nombre no resoluble, aridad incorrecta
  del genérico, coerción Int↔Float consistente) — es 5.2.
- **`T??` repetido** — el parser solo consume un `?`. Un segundo
  `?` queda sin consumir y rompe en la siguiente etapa con un
  mensaje desafortunado. Definir si lo permitimos (semánticamente
  `Nullable(Nullable(T)) == Nullable(T)`) o si erroramos explícito.
- **Guía**: sin capítulo nuevo. Tipos compuestos en anotaciones
  se aceptan en sintaxis pero el evaluator los sigue ignorando
  igual que antes; el capítulo entra cuando 5.2 los chequea.

#### 5.2 — Resolución de tipos y type checker base ✓
**Completado** — primer chequeo estático real. Nuevo módulo
`src/types.rs` con representación interna `Type`, tabla `TypeEnv`,
resolución de `TypeExpr → Type` con validación de aridad y
existencia, y pasada de chequeo sobre las anotaciones del programa.

- `Type` con primitivos como singletons (`Int`, `Float`, `Str`,
  `Bool`, `Null`, `Range`), genéricos built-in con aridad fija
  (`List<T>`, `Map<K, V>`, `Result<T>`), `Nominal(TypeId)` para
  tipos declarados, `Nullable(Box<Type>)` para `T?`. Identidad
  nominal por `TypeId` — dos `type User` en módulos distintos
  serían tipos distintos.
- `TypeEnv` con declare_nominal/set_fields/lookup, soporta forward
  refs cross-tipo (`type A { b: B }; type B { a: A }`).
- `resolve_type_expr`: traduce `TypeExpr` a `Type` validando
  primitivo + 0 args, genéricos con aridad exacta, nominal sin
  args. Errores claros: "tipo desconocido `Foo`", "el tipo `List`
  espera 1 argumento(s) de tipo, recibió 2".
- `resolve_program` en tres vueltas: nombres → fields → resto de
  anotaciones (params, return type, lets — incluso adentro de
  bodies de funciones). Acumula todos los errores en lugar de
  cortar al primero.
- `check_field_default` valida defaults literales contra el tipo
  declarado del campo (`Int = "x"` → error; `Float = 1` → OK por
  coerción; `Str? = null` → OK). Defaults no-literales se
  postergan a 5.3.
- CLI: `fitz check <file>` corre lexer + parser + resolución,
  reporta errores con contexto, exit code 1 si hay alguno.
  `fitz run` lo corre **en modo warning**: imprime los errores
  pero no aborta — los programas existentes siguen ejecutándose
  igual durante 5.x. El default flipea a "strict aborta" al
  cerrar 5a (después de 5.4).
- `FitzError` Display ahora omite el prefijo `en línea 0:0` cuando
  no hay posición. Beneficia al checker (sin posiciones todavía)
  y a varios errores del evaluator que ya estaban así.

Cierra parcialmente deuda 2.4 "anotaciones de tipo se ignoran"
— las anotaciones ahora se resuelven y validan; el chequeo de
valores contra los tipos resueltos entra en 5.3.

Tests al cerrar 5.2: 651 (614 al cerrar 5.1 + 37 nuevos en
`types.rs`: resolución por primitivo, genérico con aridad
correcta e incorrecta, nullable de primitivo y de generic,
nominal declarado y desconocido, generic con arg inválido que
propaga, programa vacío, type con primitivos, type con
generic+nullable, type que referencia otro type, forward refs
mutuas, type con field de tipo inexistente, type redeclarado,
defaults literales compatibles/incompatibles, default null sobre
nullable/no-nullable, default Int sobre Float, default
no-literal aceptado, fndef con anotaciones válidas, fndef con
param/return/generic inválido, assign con tipo inválido, lets
adentro de body validados, múltiples errores acumulados, AST
construido a mano).

Validado a mano: los 17 ejemplos de la guía y `examples/server.fitz`
pasan `fitz check` sin errores. Un archivo de prueba con 4 errores
distintos (tipo desconocido en campo, default incompatible, otro
tipo desconocido, aridad incorrecta de generic) se reporta
completo y con contexto en `fitz check` (exit 1) y como warnings
en `fitz run` (exit 0, programa ejecuta).

**Deuda explícita — retomar después:**
- **Posiciones de error** — `TypeExpr` no carga línea/columna, así
  que los errores del checker salen sin posición. Mismo issue que
  varios `FitzError` del evaluator. Pelarlo es un refactor amplio
  del parser; cuando se cierre, el Display vuelve a mostrar
  línea/columna en todos los casos.
- **Chequeo de expresiones contra el tipo declarado** — `let x:
  Int = "hola"` hoy no falla (el valor es un literal Str, el tipo
  declarado es Int, pero el checker no compara). Es el corazón
  de 5.3.
- **Sugerencias "¿quisiste decir...?"** — sin similaridad de
  nombres todavía. Nice-to-have post-5.4.
- **Imports cross-módulo en el checker** — el evaluator carga
  módulos lazy/eager, pero `resolve_program` chequea cada archivo
  por separado. Cuando 5.3 valide expresiones, los tipos
  importados van a tener que aparecer en el `TypeEnv` del archivo
  que los usa. Por ahora, los handlers que usan tipos importados
  (caso típico: `examples/server.fitz` sin imports) siguen
  funcionando porque cada archivo se resuelve solo y sus `type`s
  locales están en su env.

#### 5.3 — Type checker de expresiones y funciones ✓
**Completado** — los cinco sub-pasos cerrados. Cubre la
sintaxis completa del lenguaje observable hoy. El cierre formal
con la lista de pendientes naturales (que no bloquean 5.4) está
al final de 5.3.5.

##### 5.3.1 — Synthesis básico ✓
**Completado** — primera pasada del checker que mira EXPRESIONES.
Sintetiza tipos para literales, idents, BinOp aritmético/comparación/
lógico, UnaryOp Neg, StrInterp, `if`, list/map literales, struct
lit, field access sobre Nominal, Range. Asignaciones con anotación
validan compatibilidad. Scopes locales para FnDef/FnExpr/while/for/
loop. Match bindea las variables de los patrones (Ident, OkBinding,
ErrBinding). Imports registran nombres como vars (`import foo`,
`from foo import X`) y los nombres de `FromImport` se registran
también como nominales en el TypeEnv para que `User { ... }` no
falle.

- Nuevas variantes de `Type`: `Function { params, ret }` (modelado
  pero ret todavía es Any en 5.3.1) y `Any` (escape gradual para
  expresiones que el checker aún no modela y para anotaciones que
  faltan).
- `CheckCtx`: stack de scopes para variables, errores acumulados,
  builtins (`print`, `len`) pre-registrados como Any.
- `infer_expr`: synth bottom-up. Cubre los Expr listados arriba;
  Call/FnExpr/Index/Match/Ok/Err/Try devuelven Any o info parcial
  hasta que las sub-fases siguientes los refinen.
- `check_stmt` + `check_block`: walker de Stmt con scopes. Stmt::
  Assign con anotación compara RHS contra el tipo declarado;
  para FnDef abre scope y bindea params con tipo declarado o Any.
- `is_compatible`: Any compatible con todo, Null compatible con
  `T?`, `T` compatible con `T?`, Int compatible con Float
  (coerción), resto = igualdad estructural.
- Pre-registro de firmas: las `FnDef` top-level se registran como
  `Type::Function` en el scope global antes de walkear los bodies,
  habilitando referencias hacia adelante y recursión mutua.
- Entry point público nuevo: `check_program` (corre
  `resolve_program` + pasada de expresiones). `resolve_program`
  queda como API privada del módulo para tests granulares.
  `fitz check` y `fitz run` ahora llaman a `check_program`.

Tests al cerrar 5.3.1: 700 (651 al cerrar 5.2 + 49 nuevos
cubriendo ident desconocido/conocido/nominal-como-value,
builtins, BinOps aritmético/comparación/lógico con tipos OK y
errores, UnaryOp Neg, Range, lists vacías/homogéneas/anotadas
mal, maps vacíos, StructLit con tipo conocido/desconocido/campo
mal tipado/campo extra, field access OK e incompatible, Assigns
con varias formas de compatibilidad, if/while con cond mala,
for sobre Range/List/no-iterable, FnDef y FnExpr bindeando
params, match con Ident/OkBinding/ErrBinding bindings, imports
y from-imports, acumulación de múltiples errores).

Validado a mano: los 17 ejemplos de la guía y
`examples/server.fitz` pasan `fitz check` limpios; `fitz run`
produce el mismo output que antes; un archivo de prueba con 7
errores variados (assign con tipo mal, BinOp con Str/Int, var
desconocida, if con cond Int, for sobre Int, StructLit con
campo mal tipado, StructLit con tipo desconocido) se reportan
con mensajes específicos y contexto.

**Limitaciones conocidas de 5.3.1 (todas pendientes en sub-pasos siguientes):**
- Llamadas no validan aridad ni tipos de args — Call devuelve
  el ret del Function si lo conoce, Any si no. Es 5.3.2.
- `Stmt::Return` no se compara contra return_type — es 5.3.2.
- `?` sobre no-Result no falla, y no exige que la fn contenedora
  devuelva Result. Es 5.3.3.
- Match sobre Result no exige ambas ramas (exhaustividad). Es 5.3.3.
- Métodos built-in (`xs.map`, `m.get`, etc.) no se chequean: el
  Field dentro de un Call devuelve Any. Es 5.3.4.
- `FnExpr.ret` queda en Any. 5.3.5 sintetizará a partir del body.

##### 5.3.2 — Llamadas, return contra return_type ✓
**Completado** — el checker valida llamadas (aridad + tipos de
args) y `Stmt::Return` contra el return type declarado. Reusa el
pre-registro de firmas top-level de 5.3.1.

- `is_compatible` ahora recursa adentro de generics built-in:
  `List<a>↔List<b>`, `Map<ka,va>↔Map<kb,vb>`, `Result<a>↔Result<b>`,
  `Nullable<a>↔Nullable<b>`, y `Function` (estructural: misma
  aridad + cada param compatible + ret compatible). Caso clave
  que destraba: `Err("...")` sintetiza `Result<Any>` y ahora pasa
  contra una declaración `-> Result<User>` sin escape adicional.
- `CheckCtx.return_stack: Vec<Type>` — stack para soportar
  funciones anidadas. `Stmt::FnDef` pushea el return type
  resuelto (o `Any` si la anotación faltó / no resolvió);
  `Expr::FnExpr` pushea `Any` porque el AST no carga return type
  declarado para FnExpr (la inferencia desde el body llega en
  5.3.5).
- `Stmt::Return` infiere el tipo de la expresión y, si hay algo
  en `return_stack`, compara con `is_compatible`. Mensaje:
  ``` `return` devuelve `X` pero la función declara `Y` ```.
  Return huérfano (fuera de función) no chequea — el evaluator
  lo emite en runtime, sin solapamiento.
- `Expr::Call` valida aridad y compatibilidad de cada arg contra
  `Function.params`, y devuelve `*ret` como tipo sintetizado.
  Reglas:
  - Callee `Any` → no chequea (escape gradual). Esto cubre
    variables traídas por `from import` cuyo tipo real
    desconocemos hasta que carguemos módulos cross-archivo.
  - Callee `Function { params, ret }` → aridad estricta + tipos
    estrictos; en errores incluye índice 1-based del argumento.
  - Callee de tipo concreto distinto (`Int`, `Str`, etc.) →
    error explícito "`X` no es una función".
- Helper `describe_callee` produce etiquetas amigables para los
  mensajes de error: `Expr::Ident("foo")` → "la función `foo`",
  `Expr::Field { field: "map", .. }` → "el método `map`", resto
  → "esta llamada".
- Builtins: `len` deja de ser `Any` y pasa a
  `Function { params: [Any], ret: Int }`. Captura `len(1, 2)` /
  `len()` como errores de aridad y permite asignar el resultado a
  un `Int` sin warning. `print` queda como `Any` (variádico, sin
  representación dedicada todavía). **Convención que se
  establece**: builtins de aridad fija reciben firma real;
  variádicos siguen siendo `Any` hasta tener `Type::Variadic` o
  un mecanismo equivalente.
- E2E: `examples/server.fitz` (CRUD con `-> Result<User>` y
  `return Err("...")` / `return Ok(...)`) pasa `fitz check`
  limpio gracias a la recursividad de `is_compatible`. Los 17
  ejemplos de la guía pasan limpios excepto `15-errores.fitz`,
  que es intencional — el ejemplo demuestra un error de aridad
  (`fn add(a, b) => a + b; print(add(5))`) y ahora el checker lo
  capta estáticamente antes del error de runtime. Cap 15 de la
  guía actualizado con una nota corta.

Tests al cerrar 5.3.2: 727 (700 al cerrar 5.3.1 + 27 nuevos en
`types.rs` — calls con aridad correcta/menos/más/tipos, coerción
Int→Float, Null→nullable, recursión + forward ref, callee no-fn,
FnExpr inline; builtins `len` y `print`; Stmt::Return con tipo
compatible/incompatible, sin anotación, arrow implícito,
`Ok`/`Err` contra `Result<User>`, return huérfano; recursividad
de `is_compatible` en List/Map/Result/Function).

**Deuda explícita — retomar después:**
- **Métodos built-in sin chequear** — el callee `Expr::Field`
  evalúa a `Any` (hasta 5.3.4), por lo que `xs.map(1, 2, 3)` o
  `s.upper(extra)` pasan sin warning. Es 5.3.4.
- **`Expr::Try` (`?`) sin chequeo** — sigue devolviendo `Any` y
  no exige que el operando sea `Result` ni que la fn contenedora
  retorne `Result`. Es 5.3.3.
- **Match sobre `Result` no exige exhaustividad** — un brazo
  solo `Ok(x)` sin `Err(_)` (o viceversa) pasa. Es 5.3.3.
- **`FnExpr.ret` queda en `Any`** — el cuerpo no informa el ret
  sintetizado. Es 5.3.5.
- **Llamadas a vars `Any` no chequean aridad** — cuando un
  nombre importado (`from foo import bar`) viene como `Any`, las
  llamadas a `bar(...)` no validan nada. Es consistente con el
  modelo gradual; cuando 5.3.x cargue módulos cross-archivo, las
  firmas reales destrabarían el chequeo.

##### 5.3.3 — Result, `?`, match exhaustivo ✓
**Completado** — el checker valida el operador `?` y exige
exhaustividad de `match` cuando el scrutinee tipa como `Result<T>`.

- `Expr::Try` (`?`) valida el operando:
  - `Type::Any` → devuelve `Any` sin chequear (gradual escape;
    cubre el caso típico de método built-in cuyo callee `Field`
    todavía devuelve `Any` hasta 5.3.4).
  - `Type::Result(inner)` → desempaca a `*inner`. Si el operando
    es Result concreto y estamos adentro de una función con
    `return_type` concreto, exige que ese return type también
    sea `Result<...>` (o `Any`) — el `?` propaga el `Err(_)` vía
    `return`, así que la fn contenedora tiene que poder
    recibirlo. Reusa `return_stack`. Mensaje: "el operador `?`
    solo puede usarse adentro de una función que retorne
    `Result<...>`; esta retorna `X`". Top-level y `Expr::FnExpr`
    no disparan la regla (return_stack vacío o `Any`).
  - Otro tipo concreto → error: "el operador `?` requiere un
    `Result`, recibió `X`".
- `Expr::Match` exige exhaustividad **solo cuando el scrutinee
  tipa como `Result<T>` puro** (no nullable). Decisión de
  diseño: `Result<T>?` (un valor que puede ser ok/err/null) es
  semánticamente raro; lo dejamos sin exigir hasta que aparezca
  como necesidad real. Match sobre Int/Str/Bool/Any/etc. tampoco
  exige exhaustividad — no tenemos semántica de variantes para
  ellos todavía.
- Helper nuevo `check_result_match_exhaustiveness`: recorre los
  arms y setea `has_ok`, `has_err`, `has_catchall`. Catch-all =
  `Pattern::Wildcard` o `Pattern::Ident(_)`. Si hay catch-all o
  ambos `Ok` y `Err` → exhaustivo. Si no, error mencionando qué
  variante falta (`Ok`, `Err`, o ambas). `Ok(_)` / `Err(_)`
  cuentan como `Ok` / `Err` (la deuda de `Pattern::OkWildcard` /
  `ErrWildcard` de 3.3 queda fuera de scope; el `_` adentro se
  comporta hoy como un nombre que ensucia el scope pero a nivel
  exhaustividad pesa lo mismo). Patrones literales/de rango
  sobre Result son técnicamente "imposibles"; no los rechazamos
  acá (sería un check separado).
- E2E: los 17 ejemplos + `examples/server.fitz` pasan
  `fitz check` sin warnings nuevos. Razón: las fns que usan `?`
  no declaran `-> Result<X>` (return_stack queda `Any`, la regla
  no dispara) y los `?` operan típicamente sobre métodos
  built-in que devuelven `Any` hasta 5.3.4. Los matches sobre
  Result existentes (`14-result.fitz`, `server.fitz`) son todos
  `Ok + Err`, exhaustivos.

Tests al cerrar 5.3.3: 742 (727 al cerrar 5.3.2 + 15 nuevos en
`types.rs`: `?` sobre Result + fn Result OK, sobre Any (no
chequea), sobre no-Result (error), adentro de fn no-Result
(error), adentro de fn sin return_type (no chequea), top-level
(no chequea regla de fn), encadenado con field access
(`r?.id`); match sobre Result con Ok+Err exhaustivo, solo Ok
(error falta Err), solo Err (error falta Ok), wildcard solo,
Ok+wildcard, ident catch-all, match sobre Int / Any
(no exige exhaustividad)).

**Deuda explícita — retomar después:**
- **`Pattern::OkWildcard` / `ErrWildcard`** — sigue deuda vieja
  de 3.3. Hoy `Ok(_)` parsea como `OkBinding("_")` y ensucia el
  scope con una variable llamada `_`. El checker lo trata como
  `has_ok = true` sin diferenciar.
- **Patrones literales sobre Result** son "imposibles" pero no
  se emiten warnings. Es un check separado (dead-code de match)
  que podría llegar como nice-to-have.
- **Métodos built-in que deberían retornar `Result<T>`** —
  `xs.find(...)`, `m.get(...)`. Hoy son `Any` (el chequeo de `?`
  los deja pasar gradual). Cuando 5.3.4 les dé firma real, el
  encadenado `xs.find(...)?` va a chequear con precisión real.
- **`?` adentro de `Result<X>?`** — no exigido, decisión de
  diseño. Revisitable si aparece el caso.

##### 5.3.4 — Métodos built-in con templates paramétricos ✓
**Completado** — `Expr::Call` con `callee: Expr::Field` ahora
despacha por `(tipo del receptor, nombre del método)` a una tabla
built-in en lugar de caer en el camino general (que no podía
modelar signatures paramétricas).

- Nuevo helper `infer_method_call(ctx, receiver_ty, method,
  args_ty) -> Option<Type>` con sub-dispatchers
  `infer_list_method`, `infer_map_method`, `infer_str_method`.
  Cada uno hace match sobre el nombre del método, valida aridad
  + tipos vs la signature concreta del receptor, y devuelve el
  ret instanciado.
- Tabla de signatures cubierta (14 métodos):
  - `List<T>`: `push(T) -> Null`, `pop() -> T`, `len() -> Int`,
    `map(fn(T) -> U) -> List<U>`,
    `filter(fn(T) -> Bool) -> List<T>`,
    `find(fn(T) -> Bool) -> Result<T>`.
  - `Map<K, V>`: `get(K) -> Result<V>`, `has(K) -> Bool`,
    `keys() -> List<K>`, `values() -> List<V>`, `len() -> Int`.
  - `Str`: `len() -> Int`, `upper() -> Str`, `lower() -> Str`.
- Helpers compartidos:
  - `check_method_arity(name, args_ty, expected) -> bool` —
    aridad fija, devuelve `false` cuando no coincide (caller
    puede saltarse validaciones extra).
  - `check_unary_callback(cb, elem_ty, method, expected_ret)
    -> Type` — exige `Function` con aridad 1, valida que el
    param sea compatible con T, y opcionalmente que el ret sea
    compatible con un tipo esperado (caso `filter`/`find`
    exigen `Bool`). Callback `Any` pasa sin chequear (gradual).
- Política sobre método desconocido:
  - Receptor built-in concreto (`List`/`Map`/`Str`) → error con
    el nombre del receptor y del método. Captura typos
    (`xs.lenght()`).
  - Receptor `Nominal(id)` → `None`; la llamada cae a gradual
    (Any) sin chequear. Los métodos custom sobre `type` siguen
    siendo deuda de 3.2; no rompemos código que los use.
  - Receptor `Any` → `None`, gradual.
  - Otro tipo concreto (Int, Bool, Range, Result, etc.) →
    error "no tiene el método X". El evaluator también lo
    rechazaba en runtime; ahora se atrapa estáticamente.
- Impacto sobre 5.3.3: `xs.find(...)` y `m.get(k)` antes eran
  `Any` y ahora son `Result<T>` / `Result<V>` concretos. Eso
  hace que:
  - `users.find(...)?` opere sobre `Result<User>` concreto en
    vez de gradual. La regla "fn contenedora retorna Result"
    sigue chequeando — si la fn no declara return_type
    (`return_stack.last() == Any`), no dispara. Los ejemplos
    existentes (server.fitz `update_user`) no declaran return
    type así que siguen pasando.
  - `match users.find(...) { Ok(u) ... Err(_) ... }` ahora pasa
    por el chequeo de exhaustividad de 5.3.3 (antes el
    scrutinee Any no exigía exhaustividad). Los matches
    existentes son todos `Ok + Err`, completos.
- E2E: los 17 ejemplos + `examples/server.fitz` pasan
  `fitz check` sin regresiones — la mayor parte del código
  built-in en los ejemplos era invisible al checker hasta acá,
  y ahora se valida sin warnings nuevos.

Tests al cerrar 5.3.4: 767 (742 al cerrar 5.3.3 + 25 nuevos en
`types.rs`: List push/pop/len/map/filter/find con tipos
compatibles, incompatibles, aridad incorrecta, callback sin
anotaciones (gradual), callback param incompatible; Map
get/has/keys/values/len con tipos compatibles e incompatibles;
Str upper/lower/len; método desconocido sobre cada built-in
(typos), método sobre Int (error), método sobre Nominal sin
chequeo (gradual), encadenado `xs.map(...).filter(...)` en una
sola línea).

**Deuda explícita — retomar después:**
- **FnExpr.ret inferido del body** — los callbacks inline
  (`fn(x) => x * 2`) hoy tienen `ret = Any`. Eso significa que
  `.filter(fn(x: Int) -> Int { ... })` con ret no-Bool no se
  detecta si el callback es FnExpr inline (sí se detecta si
  viene como Function declarada con ret concreto). Lo cubre
  5.3.5.
- **`Expr::Index`** (`xs[i]`, `m[k]`) sigue devolviendo `Any`.
  Es un paso análogo a métodos built-in pero independiente;
  candidato a 5.3.5 o sub-paso separado.
- **Encadenamiento multi-línea** (`xs.map(...)\n.filter(...)`)
  sigue siendo deuda explícita del parser (3.4). El test usa
  forma de una sola línea.
- **Métodos custom sobre `type`** — sigue deuda de 3.2.

##### 5.3.5 — FnExpr.ret inferido + Expr::Index + cierre de 5.3 ✓
**Completado** — último paso del checker de expresiones. Cierra
la sub-fase 5.3 entera.

- `CheckCtx` gana `inferred_returns: Vec<Vec<Type>>` paralelo a
  `return_stack`. Cada frame recolecta los tipos sintetizados
  de los `Stmt::Return` del body de su función. `Expr::FnExpr`
  lo consume al salir para sintetizar `ret` vía `unify_returns`
  + `lub`. `Stmt::FnDef` también pushea un frame por
  consistencia pero descarta el contenido (ya tiene
  `return_type` declarado; la unificación queda disponible para
  un eventual check futuro "declarado vs inferido"). `Stmt::Return`
  pushea su tipo al frame de la fn contenedora.
- `lub(a, b)`: "least upper bound" pragmático para unificar
  tipos de ramas distintas de un `return`. Reglas:
  - `a == b` → `a`.
  - Cualquiera Any → el otro (Any cede al concreto).
  - Int + Float → Float (coerción).
  - Null + T → `T?` (caso típico de "una rama devuelve null").
  - T + T? → `T?`.
  - Generics built-in (`List`/`Map`/`Result`/`Nullable`)
    recursivos.
  - Mix arbitrario → Any.
  No es un lattice formal — prioriza preservar información
  útil (`lub(Result<User>, Result<Any>) = Result<User>`).
- `unify_returns(types)`: fold con `lub`. Lista vacía → `Null`
  (matchea la semántica del evaluator: una fn que termina sin
  `return` explícito devuelve `Value::Null`).
- Caso clave destrabado: `xs.filter(fn(x: Int) => x * 2)`
  ahora detecta que el ret inferido del callback (`Int`) no es
  el `Bool` que filter exige. El test correspondiente que
  abandonamos en 5.3.4 volvió.
- **`Expr::Index`** (`xs[i]`, `m[k]`): deja de devolver `Any`
  silenciosamente.
  - `List<T>[Int]` → `T`. Índice no-Int → error.
  - `Map<K, V>[K]` → `V`. Índice incompatible con K → error.
  - `Str[?]` → error "no soporta indexing todavía" (deuda 3.1
    sobre unidad char/byte/grafema).
  - `Any` o `Nominal` → `Any` (gradual; los indexers custom
    sobre `type` no existen todavía).
  - Otro tipo concreto → error "no soporta indexing".

Tests al cerrar 5.3.5: 784 (767 al cerrar 5.3.4 + 17 nuevos en
`types.rs`: FnExpr ret inferido para arrow/block/sin-return,
lub con Int+Float, Null+T, Result+Result; Index sobre
List/Map/Str/Int/Any con tipos compatibles e incompatibles;
helpers `lub` y `unify_returns` directos).

Verificación end-to-end: los 17 ejemplos + `examples/server.fitz`
pasan `fitz check` limpios. El cambio más sensible (FnExpr.ret
real) no rompe nada porque todos los callbacks de filter/find
en los ejemplos retornan `Bool` (`u.id == id`, `n == 2 or n == 4`,
etc.); los de map retornan tipos arbitrarios sin chequeo de ret
forzado.

**Cierre formal de 5.3 — Type checker de expresiones y
funciones:**

El checker estático cubre hoy el lenguaje observable:
literales, ident, operadores aritméticos/lógicos/comparación
con coerción Int↔Float, StrInterp, control de flujo
(if/while/for/loop), list/map/struct literals, field access,
match (con exhaustividad sobre Result), Range, Ok/Err, `?`,
struct lit, llamadas a fn/builtin con aridad y tipos,
`Stmt::Return` contra `return_type`, métodos built-in
paramétricos para List/Map/Str (14 métodos), FnExpr con `ret`
inferido del body, y `Expr::Index` sobre receptores conocidos.
La regla gradual (`Any` cede a cualquier tipo) preserva el
modelo del lenguaje sin obligar a anotar.

**Deuda explícita — pendientes naturales que NO bloquean 5.4:**
- **Métodos custom sobre `type`** — deuda vieja de 3.2. El
  dispatch del checker está preparado para sumar otra fuente
  de lookup sin retoques.
- **`Pattern::OkWildcard` / `ErrWildcard`** — deuda de 3.3.
  Hoy `Ok(_)` parsea como `OkBinding("_")` y ensucia el scope.
- **`Result<X>?` como scrutinee de match** — no exigido,
  decisión de diseño. Revisitable si aparece como caso real.
- **Patrones imposibles sobre Result** (literal `Int` en match
  sobre `Result<T>`) — son dead code; el chequeo es independiente.
- **Encadenamiento multi-línea en method chains** (deuda 3.4
  del parser) — `xs.map(...)\n.filter(...)` corta en el
  newline; el checker funciona pero la sintaxis no llega.
- **Posiciones de error** — `TypeExpr` y muchos `FitzError` del
  checker todavía salen sin línea/columna. Refactor amplio,
  pendiente.
- **FnExpr con tipo de retorno declarado en sintaxis**
  (`fn(x: Int) -> Int { ... }` como expresión) — el AST y el
  parser no lo modelan. Hoy el ret se infiere siempre. Cuando
  aparezca la sintaxis, el checker compara declarado vs
  inferido sin trabajo extra.

#### 5.4 — Modo strict en `fitz run` + cierre de 5a ✓
**Completado** — flip del default de `fitz run`: ahora aborta
cuando el checker estático encuentra errores, en lugar de
emitirlos como warnings y seguir. Cierra formalmente la
sub-fase 5a (Type checker estático sobre el intérprete).

- `src/main.rs`:
  - Nueva flag `--no-typecheck` en la variante `Run` del enum
    `Commands` (`#[arg(long)]`). Sin la flag, modo strict; con
    la flag, los errores se reportan como warnings y el
    programa se ejecuta igual.
  - Strict (default): mensaje `✗ <archivo> — N error(es) de
    tipo:` + lista de errores + sugerencia `Usá \`fitz check\`
    para revisar, o \`fitz run --no-typecheck <archivo>\` para
    correr igual.` Exit code 1. El evaluator nunca arranca.
  - Gradual (`--no-typecheck`): mensaje `⚠ N warning(s) del
    checker de tipos (modo \`--no-typecheck\`):` + lista. Sigue
    al evaluator.
- `examples/guide/15-errores.fitz` reescrito: cambió de
  aridad-incorrecta (que el checker ahora detectaba desde
  5.3.2) a división por cero (error de runtime puro que el
  sistema de tipos no analiza por diseño). Sigue cumpliendo el
  rol pedagógico del capítulo — mostrar cómo se ve un error de
  runtime — sin entrar en conflicto con el modo strict del
  checker.
- `docs/guide.md` cap 15 reorganizado: documenta cuatro etapas
  (lexer → parser → **checker** → evaluador) en lugar de tres;
  suma sección "Modo strict y `--no-typecheck`"; tabla nueva
  de errores típicos del checker; ejemplo final de runtime
  puro. Cap 3 actualizado (anotaciones de tipo se chequean en
  compile time). Cap 18 actualizado: Fase 5a cerrada, 5b
  (codegen) como próximo bloque. Header bumpeado.

Sin tests automatizados nuevos en 5.4 — el feature es CLI y la
infraestructura para tests CLI no existe todavía. Verificado a
mano:
- Programa con error de tipo → `fitz run` aborta con exit 1.
- Mismo programa con `--no-typecheck` → ejecuta con warning.
- `examples/guide/15-errores.fitz` pasa `fitz check` (división
  por cero no es tipo) y `fitz run` emite `Error — división
  por cero` desde el evaluator.
- Los 14 ejemplos no-HTTP de la guía corren limpios con
  `fitz run`.

Total al cerrar 5.4: 784 tests (sin cambios respecto de 5.3.5).

**Cierre formal de Fase 5a — Type checker estático:**

5a queda completada. El checker cubre la sintaxis completa
observable del lenguaje hoy: anotaciones en variables,
parámetros, return types y campos de `type`; expresiones de
todos los nodos del AST; llamadas con aridad y tipos contra la
signature declarada; `Stmt::Return` contra return_type
declarado; operador `?` con la regla de fn contenedora;
exhaustividad de `match` sobre `Result<T>`; métodos built-in
paramétricos sobre `List`/`Map`/`Str` (14 métodos); `Expr::Index`
sobre `List`/`Map`; inferencia básica (synthesis + `lub` para
FnExpr.ret).

Lo que queda abierto para futuras fases (no bloquea 5b):
- Métodos custom sobre `type` (deuda 3.2; dispatch del checker
  preparado).
- ~~`Pattern::OkWildcard` / `ErrWildcard` (deuda 3.3).~~ ✓
  Cerrada en el paso de deuda residual post-5a: el parser
  reconoce `Ok(_)` y `Err(_)` como wildcards dedicados, el
  evaluator matchea sin bindear y el checker los cuenta para
  exhaustividad.
- Patrones imposibles sobre Result (dead-code check separado).
- Encadenamiento multi-línea en method chains (deuda 3.4 del
  parser).
- ~~Reasignación sin anotación contra tipo previo (`m: Int = 1;
  m = "x"` no chequea — el binding se relaja al tipo nuevo).~~
  ✓ Cerrada en el paso de deuda residual post-5a: `VarBinding`
  ahora guarda un flag `annotated`; reasignaciones sin
  anotación contra una var anotada se chequean contra el tipo
  declarado.
- Posiciones de error precisas en TypeExpr y errores del
  checker. **Pospuesta para post-5b**: el refactor amplio del
  AST (posiciones en `Expr`/`Stmt`/`TypeExpr` con propagación
  desde el parser) cubre los errores de anotación pero no los
  de expresiones; mejor combinarlo con la infra del IR tipado
  que va a sumar 5b. Hoy los errores del checker reportan
  posición `0:0` con mensajes descriptivos como puente.
- FnExpr con return type declarado en sintaxis (AST/parser).
- Sugerencias "did you mean..." en typos.

5b arranca con un IR tipado encima de lo que produce este
checker.

#### 5b.1 — Codegen a binario nativo (subset primitivo) ✓
**Completado** — primer paso de Fase 5b. Transpile AST de Fitz →
código Rust → binario via `rustc`. Cubre programas CLI con
primitivos, sin tipos compuestos, sin HTTP.

**Backend elegido — transpile-a-Rust** sobre Cranelift/LLVM:
- Reusamos el compilador de Rust completo. Optimizaciones (LTO,
  inlining) gratis.
- Cross-compile a todos los targets de rustc sin trabajo extra.
- `async fn` Fitz puede mapear a `async fn` Rust cuando llegue
  5b.6 (HTTP / async real).
- Type-safety: si el codegen tiene un bug, rustc lo va a cazar.
- Trade-off explícito: compile times = los de rustc (~2s para
  programas pequeños). Para servicios web —el público objetivo
  de Fitz— se hace una vez por deploy, no es interactive.

**Nuevo módulo `src/codegen.rs`**:
- `pub fn generate_rust(program: &Program, env: &TypeEnv) ->
  Result<String, FitzError>` — entry point.
- `CodegenCtx` con stack de scopes (`HashMap<String, Type>`) y
  tabla `fn_sigs` de firmas pre-registradas. Las firmas top-level
  se computan **antes** de generar cuerpos, así las llamadas
  resuelven el return type sin importar el orden de las fns.
- Visitor sobre AST tipado. No introducimos IR intermedio en
  5b.1: para un subset chico, un visitor a un buffer `String`
  alcanza. Cuando 5b.2+ traiga tipos compuestos posiblemente
  sumemos uno.
- Helpers: `coerce(code, from, to)` para Int→Float, `numeric_coerce`
  para BinOp con tipos mixtos, `rust_type_for(t)` para mapear
  primitivos, `rust_str_literal(s)` para literales escapados,
  `type_name(t)` para mensajes.

**Mapping AST de Fitz → Rust**:

| Fitz | Rust |
|------|------|
| `Int` | `i64` |
| `Float` | `f64` |
| `Str` | `String` |
| `Bool` | `bool` |
| `Null` | `()` |
| `let x: Int = 42` | `let mut x: i64 = 42i64;` |
| `let x = 1` (inferido) | `let mut x: i64 = 1i64;` (usa tipo del checker) |
| `"hola {x}"` | `format!("hola {}", x)` |
| `s1 + s2` (Str) | `format!("{}{}", s1, s2)` |
| `1 + 2.0` | `((1i64 as f64) + 2f64)` |
| `print(a, b)` | `println!("{} {}", a, b)` |
| `for i in 0..3` | `for mut i in (0i64 as i64)..(3i64 as i64)` |
| `fn f(n: Int) -> Int { ... }` | `fn f(mut n: i64) -> i64 { ... }` |

Convenciones:
- Variables siempre `let mut` para simplificar reasignación.
  Reasignación detectada mirando si la var ya existe en algún
  scope visible (no solo el top); si sí, emite `x = ...` en
  vez de `let mut x = ...`.
- Strings se concatenan siempre con `format!` para evitar los
  juegos de ownership de `String + &str`. Ineficiente pero
  correcto.
- Strings pasados como args usan `.clone()` (ineficiente pero
  evita refactor de ownership).
- Coerción `Int → Float` se inserta como `(x as f64)` en cada
  punto donde se necesita (BinOp mixto, asignación a Float
  anotado, paso de Int a param Float).
- `print()` sin args → `println!()` (newline).
- `print(a, b, c)` → format string con `{}` separados por
  espacio, replicando la semántica del intérprete.

**Subset soportado en 5b.1**:
- Literales Int / Float / Str / Bool / Null.
- BinOp (aritméticos con coerción, comparación numérica y de
  Str via `.as_str()`, lógicos `and`/`or`).
- UnaryOp `Neg`.
- StrInterp.
- Asignación con o sin anotación, reasignación.
- `if`/`else` como sentencia.
- `while`, `loop`, `break`, `continue`.
- `for var in start..end` (rangos exclusivos).
- Funciones top-level con params/return tipados.
- `print()` builtin.
- `return` explícito.

**Fuera de scope (refinamos en pasos siguientes con errores
explícitos)**:
- Tipos custom, struct lit, field access → 5b.2.
- Listas, mapas, indexing → 5b.3.
- `Result`/`?`/`match` → 5b.4.
- Módulos → 5b.5.
- HTTP / `@server` / handlers → 5b.6.
- Funciones anónimas (FnExpr) → 5b.2 o 5b.3.
- Decoradores → error "5b.6".

**Subcomando `fitz build`** (antes era stub que imprimía "🚧"):
- Flow: lex → parse → checker en **modo strict siempre** (no hay
  `--no-typecheck` en build; build exige programa correcto) →
  `codegen::generate_rust` → escribe `target/fitz-build/<nombre>/main.rs`
  (visible para debug) → invoca `rustc --edition 2021 -O <main.rs>
  -o <bin>` → copia el binario adyacente al .fitz fuente.
- Naming: `hello.fitz` → `hello.exe` (Windows) o `hello`
  (Linux/macOS).
- Sin Cargo todavía — 5b.1 no tiene dependencias externas. Cuando
  llegue 5b.4+ con serde (para Result) o 5b.6 con axum/tokio,
  pasamos a generar `Cargo.toml + src/main.rs` y llamar
  `cargo build --release`.

**Tests**:
- **28 tests unitarios** en `codegen.rs`: cubren cada feature
  del subset (literales, BinOp con coerciones, StrInterp, print,
  fns top-level con block y arrow, llamadas, if/while/for/loop,
  reasignación, UnaryOp, lógicos, comparación Str con
  `.as_str()`) + cada feature fuera de scope con error
  específico mencionando el sub-paso futuro.
- **8 tests E2E** en `tests/compile_e2e.rs` (integration tests):
  invocan `fitz build` sobre programas reales, ejecutan el
  binario, comparan stdout y exit code. Cubren: criterio de éxito
  hello-world, `if`/`else`, `while`+reasignación, `for`-range,
  coerción Int→Float, recursión, build aborta con error de tipo
  strict, build aborta sobre feature no soportada (lista). Usan
  un `Mutex` global para serializar las invocaciones de rustc
  (múltiples rustc paralelos sobre el mismo target dir producían
  cross-talk de outputs en Windows).

**Criterio de éxito**:
```fitz
let name = "Fitz"
let x = 10 + 5
print("Hola, {name}, x es {x}")

fn double(n: Int) -> Int => n * 2
print(double(x))
```
```bash
fitz build hello.fitz
./hello
# Hola, Fitz, x es 15
# 30
```
Validado a mano y vía test E2E.

Tests al cerrar 5b.1: **833** (797 al cerrar deuda residual + 28
unit codegen + 8 E2E = 833). Los 17 ejemplos + `server.fitz`
pasan `fitz check` limpios sin cambios.

**Deuda explícita — retomar en pasos siguientes**:
- **`Type::Any` en variables sin anotación inferible**: el
  codegen exige conocer el tipo. Si el checker no pudo
  sintetizar (caso raro en 5b.1), error. En 5b.2+ podemos
  refinar a `Box<dyn Any>` o pedir anotación al usuario.
- **Vars declaradas adentro de bloques quedan confinadas en
  Rust**: en Fitz `while { x = 5 }` deja `x` definida afuera;
  en el binario generado, no. Discrepancia conocida; cierra
  con pre-declaración de vars en el outer scope si se pide.
- **Compile time del binario**: ~2s para programas chicos por
  `rustc` cold. Aceptable para 5b.1; si molesta en 5b.6+ con
  axum, pasamos a `cargo build` que cachea.
- **Strings pasados con `.clone()` siempre**: ineficiente.
  Optimización post-5b cuando estabilicemos el modelo de
  ownership en codegen.

##### 5b.2 — Tipos custom + field access + struct literal ✓
**Completado** — segundo paso de Fase 5b. `type Foo { ... }` se
transpila a `struct FooData { ... }` con type alias
`type Foo = Rc<RefCell<FooData>>;` para preservar la semántica
de referencia compartida del intérprete. Trade-off conocido:
field access caro (`u.borrow().name.clone()`), optimizable
post-5b.

Cubierto:
- Struct literal con defaults inline-eados y wrapping
  `Some(...)`/`None` automático para campos nullables.
- Field access (`u.name`) con `.clone()` selectivo según
  `needs_clone` (Str/Nominal/Nullable clonan, primitivos Copy no).
- Field assignment (`u.name = "x"`).
- Igualdad estructural entre instancias (`==`/`!=`) via
  `#[derive(PartialEq)]` — `Rc<RefCell<T>>` compara por
  contenido, recursando en campos nominales anidados igual que
  el intérprete.
- Tipos custom como campo de otro tipo custom
  (`type Order { user: User? }`).
- Pasaje a/desde funciones: el `Rc` se clona en cada uso de
  `Ident` Nominal — el clone es del puntero refcontado, así que
  preserva aliasing.
- `Display for FooData` reproduce el formato del intérprete
  (strings con comillas, Float con `.0`, Nullable como `null`,
  nominales recursivos).

Bonus que entraron en el mismo bloque (cierran deuda chica de
5b.1):
- **`if` como expresión con valor**: cuando ambas ramas terminan
  en `Stmt::Expr` no-`print`, `gen_if_expr` emite el `if` como
  expresión Rust con tail sin `;`. LUB simple: `Int+Float→Float`,
  `T+Null→T?`. Statement-mode preservado para `if cond { print(x) }`.
  Cierra deuda de 5b.1 sobre `let x = if cond { a } else { b }`.
- **Métodos built-in sobre Str**: `s.len()`→`chars().count()`,
  `s.upper()`→`to_uppercase()`, `s.lower()`→`to_lowercase()`.
  Despacho por callee `Expr::Field { object, field }`.
- **StrInterp con Null/Float/Nominal/Nullable**: alineado al
  formato del intérprete. `"x es {null}"` ahora produce `"x es
  null"` (antes rustc no compilaba por `()` sin Display).

Deuda explícita marcada con error de codegen claro:
- Métodos custom sobre `type`: depende de cerrar la deuda de 3.2
  en parser/AST (hoy `Stmt::TypeDef` solo guarda `fields`, sin
  bloque de métodos).
- Tipos importados: hasta 5b.5.
- Listas/mapas (literales, indexing, métodos): hasta 5b.3.
- `Result`/`?`/`match`: hasta 5b.4.

Tests: 21 unit nuevos en `src/codegen.rs` + 14 E2E nuevos en
`tests/compile_e2e.rs`. Total acumulado: 868 (846 unit + 22 E2E).
Validado a mano contra `fitz run`: `examples/types.fitz`,
`examples/guide/05-strings.fitz`, `examples/guide/07-if.fitz`
y `examples/guide/12-type.fitz` producen output idéntico.

##### 5b.3 — Listas, mapas, indexing, method calls ✓
**Completado** — tercer paso de Fase 5b. `List<T>` →
`Rc<RefCell<Vec<T>>>` y `Map<K, V>` → `Rc<RefCell<Vec<(K, V)>>>`:
mismo modelo de referencia compartida que 5b.2 (Nominal). Orden de
inserción preservado por Vec. Aliasing por referencia: `xs.push(x)`,
`xs[i].name = "x"` vía cualquier alias se ve en la colección original.
Trade-off conocido (paralelo a 5b.2): `xs[i]` → `xs.borrow()[i as
usize].clone()`. Optimizable post-5b.

Cubierto:
- **Literales**: `[e1, e2, ...]` con coerción de cada item al tipo
  común (LUB pragmático: Int↔Float→Float, T↔Null→T?, T?↔T→T?,
  recursivo en generics built-in). `{k: v, ...}` análogo a `Vec<(K, V)>`.
  Vacíos sintetizan `List<Any>`/`Map<Any, Any>` y el contexto
  (anotación destino) los resuelve.
- **Heterogéneos irrecuperables** (`[1, "dos"]`, `{"a": 1, "b": "x"}`)
  → error de codegen con mensaje claro. El subset compilado no
  soporta tagged unions runtime.
- **Indexing** `xs[i]` (List) y `m[k]` (Map). List: borrow + clone
  del item (clone del Rc para Nominal/List/Map → preserva
  aliasing). Map: búsqueda lineal con `panic!` si la clave falta,
  mensaje idéntico al del intérprete. Para evitar E0716 con `Rc`
  temporales, el bloque liga primero el Rc a una var local antes
  del `.borrow()`.
- **`for v in xs`** sobre `List<T>`: snapshot via `borrow().clone()
  .into_iter()` para evitar re-entrancia si el body muta la lista.
  Map como iterable directo NO se soporta (alineado con el
  intérprete).
- **Métodos**: `push`, `pop`, `len`, `map`, `filter` sobre List;
  `has`, `keys`, `values`, `len` sobre Map. `pop` paniquea sobre
  lista vacía con el mensaje del intérprete. `keys`/`values`
  devuelven `List<K>`/`List<V>` envuelto en Rc, permitiendo
  method chaining.
- **`find` (List) y `get` (Map)** devuelven `Result<T>` y se
  difirieron a 5b.4 con error de codegen específico mencionando
  "5b.4". El cap 13 entero (que usa find + match) queda bloqueado
  hasta 5b.4; la versión reducida sin find/match compila bit-a-bit.
- **Builtin global `len(x)`**: despacha por tipo del argumento
  (Str → `chars().count()`, List/Map → `borrow().len()`). Una fn
  `len` definida por el usuario gana — `fn_sigs` se chequea antes
  del builtin.
- **FnExpr inline como callback** de `.map(...)` y `.filter(...)`:
  emite Rust closure tipado `|p: T| -> U { body }`. Inferencia
  mini-LUB del ret type sobre el primer `Stmt::Return` del body
  (o último `Stmt::Expr` no-print). **Higher-order completo**
  (FnExpr como var, param o retorno) → error explícito con
  referencia a "sub-paso posterior". El cap 11 (closures,
  `make_adder`, `apply`) queda como deuda visible.
- **`print`/interpolación de List/Map**: formato bit-a-bit
  idéntico al intérprete (`[1, 2, 3]`, `{"a": 1, "b": 2}`,
  strings entre comillas adentro vía `show_expr_inline`). El
  bloque inline liga el Rc a `let __list`/`let __map` antes del
  `.borrow()` (vida del temporal); itera con `.iter().cloned()`
  para que `__it` venga por valor.
- **`lub_for_if` renombrado a `lub`** y extendido recursivamente
  para generics built-in (List/Map/Result/Nullable). Reusado
  desde if-as-expression (5b.2) y desde unificación de items de
  literales (5b.3).

Tests: 28 unit nuevos en `src/codegen.rs` (literales, indexing,
métodos, FnExpr inline, builtin global, print bit-a-bit, errores
explícitos) + 7 E2E nuevos en `tests/compile_e2e.rs` (push+len+
for, indexing+pop, mapa has/keys/values/len, lista de instancias +
alias del cap 13, chain `.filter().map()`, promoción Int→Float,
lista heterogénea aborta). Total acumulado: **902 tests** (873
unit + 29 E2E). El test viejo `listas_no_soportadas` se reemplazó
por `listas_heterogeneas_son_error`; el E2E
`build_aborta_si_codegen_no_soporta_feature` ahora apunta a
`Ok(...)` (Result, 5b.4).

Validado a mano contra `fitz run`: `examples/guide/09-listas-mapas.fitz`
reducido al subset compilable (sin Range como valor, sin mezclas
heterogéneas, con anotaciones) y `examples/guide/13-metodos.fitz`
reducido (sin find/match/get) producen output idéntico bit-a-bit.

**Deuda explícita — retomar en pasos siguientes**:
- **Heterogéneos**: introducir un `FitzValue` runtime tagged si
  el caso aparece como bloqueante en la práctica. Por ahora se
  resuelve con `fitz run`.
- **find/get → Result**: se desbloquean en 5b.4.
- **Higher-order completo**: closures que escapan, FnExpr como
  var/param/retorno. Probable sub-paso 5b.4.5 o post-5b.6 con
  `Box<dyn Fn(...)>` + captura por clone explícita.
- **Range como valor / `print(range)`**: deuda residual de 5b.1
  (sigue siendo error de codegen). Cierra cuando lo necesite un
  ejemplo concreto.

##### 5b.4 — Result, `?`, match ✓
**Cerrado.** Cuarto paso de Fase 5b. Desbloquea el cap 13 entero
(find + match + get) y la mecánica completa de `Result` en el
compilador.

**Decisión clave**: `Result<T>` Fitz → `Result<T, String>` Rust
nativo — el Err side está **pinned a `String`**. Trade-off
aceptado: `Err(42)` o cualquier inner no-Str se coerce con
`format!("{}", x)` a String. Justificación:
- Todos los ejemplos de la guía y `examples/server.fitz`
  construyen `Err(...)` con strings literales.
- El intérprete mismo emite `Value::Str` desde `find`/`get`/
  divisiones por cero.
- Encaje natural con el `?` Rust (E = String en ambos lados):
  propagación sin glue.
- Encaje natural con HTTP 5b.6: el handler serializa Err como
  `{"error": <inner>}`, y un String se mapea directo.

**Alternativa rechazada — tagged `FitzValue` runtime**: añade un
módulo de tipos boxed + Display + Eq custom, fuera del scope de
5b. Reabrible post-5b si aparece presión real.

**Implementación**:
- `rust_type_for(Result<T>)` → `Result<T, String>`. T = Any
  (Err suelto sin contexto) → `Result<_, String>`, rustc infiere
  desde la anotación destino.
- `gen_expr` para `Ok(e)`: emite `Ok(<coerced e>)`. Tipo
  sintetizado `Result<T>` donde T es el tipo del inner.
- `gen_expr` para `Err(e)`: emite `Err(e.to_string())` si el
  inner es Str, o `Err(format!("{}", e))` si no. Tipo Fitz
  sintetizado: `Result<Any>` — el contexto destino refina.
- `gen_expr` para `Try(e)` (operador `?`): emite `(<expr>)?`
  Rust nativo. Nuevo `CodegenCtx.ret_stack: Vec<Type>` con
  push/pop en `gen_top_fn` y `gen_callback_inline` para
  validar que el contenedor retorna `Result<...>` (o `Any`,
  para el escape gradual). Top-level `?` y `?` en fn con ret
  concreto distinto de Result → error de codegen explícito.
- `gen_match`: emite el match siempre como **expresión Rust**
  (`(match s { ... })`); en stmt position, el `;` de `Stmt::Expr`
  lo cierra. Patrones soportados:
  - Literales Int/Float/Bool/Null directos como pattern Rust.
  - Str via guard `ref __s if __s.as_str() == "..."` (Rust no
    acepta `"x"` contra `String` directo).
  - Ident (binding), Wildcard.
  - Ok(x)/Err(e)/Ok(_)/Err(_) — Rust nativos.
  - Range `a..b` via guard `__n if (a..b).contains(&__n)`.

  **Exhaustividad**: si los arms no cubren todo (sin Ident/
  Wildcard ni cobertura completa Ok+Err sobre scrutinee Result),
  agregamos arm `_ => panic!("el `match` no matcheó ningún
  brazo")` — mismo mensaje del intérprete.

  **Bodies con `print(...)`**: como `print` no es expresión en
  Fitz, los emitimos como bloque stmt-wrapped `{ println!(...); }`.
  Detalle pequeño, importante para que arms de match con `print`
  compilen.
- **find/get**: los errores "5b.4" desaparecen. `.find(callback)`
  sobre `List<T>` emite loop que devuelve `Ok(item)` al primer
  match, `Err("no encontrado".to_string())` si nada matchea
  (mensaje idéntico al intérprete). `.get(k)` sobre `Map<K,V>`
  devuelve `Ok(v)` o `Err(format!("clave no encontrada: {}", k))`.

  Detalle clave detectado en validación bit-a-bit del cap 13:
  la clave en el mensaje se formatea con `show_expr` (modo
  Display de Value, sin comillas para Str), no con
  `show_expr_inline` (que sí mete comillas — solo aplica
  adentro de listas/mapas).
- **`print` de Result**: `show_expr` agrega caso `Type::Result(_)`
  con sub-match inline que emite `Ok(<inline T>)` / `Err("<msg>")`
  con comillas dobles alrededor del mensaje. Bit-a-bit como el
  intérprete (que usa `write_inline_value` con `Value::Str` →
  comillas).
- **`needs_clone(Result<_>)`** → `true` (Result no es Copy).
- **`lub`**: agregamos caso `Result(a) ↔ Result(b)` recursivo y
  caso `Any ↔ T → T` (Err sin contexto unifica con `Ok(<T>)`).

**Tests**: 15 unit nuevos en `src/codegen.rs` (15 nuevos − 3
viejos reemplazados que esperaban error "5b.4"; los reapunté a
testear el nuevo comportamiento) + 6 E2E en `tests/compile_e2e.rs`
(cap 14 con anotaciones, `?` propagation, find+match, get+match
con clave faltante, print Result, match-range). Reapunté el E2E
"feature no soportada en codegen" desde `Ok(...)` a `import`
(5b.5).

**Validación bit-a-bit**:
- `examples/guide/13-metodos.fitz` entero (find + match + get)
  compila contra `fitz run` — salida idéntica.
- `examples/guide/14-result.fitz` se actualizó con anotaciones
  de tipo en las fns (`divide(a: Int, b: Int) -> Result<Int>`,
  etc.) para que también compile end-to-end. Las anotaciones
  son didácticas — refuerzan el contrato `Result<T>` que las
  fns ya respetaban implícitamente. Salida bit-a-bit idéntica
  al run anterior.

**Deuda residual que sigue post-5b.4**:
- **Higher-order completo**: closures que escapan, FnExpr como
  var/param/retorno → post-5b.6 con `Box<dyn Fn(...)>` + clone
  explícito en captura.
- **`?` adentro de FnExpr inline**: el codegen no maneja el caso
  (el callback hoy no tiene un return type "Result" propio).
  Ningún ejemplo lo usa; queda como deuda visible.
- **Inferencia de tipos de params de fns sin anotar**: deuda
  vieja de 5b.1. Bloquea compilación de fns como
  `fn divide(a, b) { ... }`. Workaround: anotar.
- **Posiciones de error precisas en codegen**: deuda
  pospuesta de 5a.

##### 5b.5 — Módulos / `import` ✓
**Cerrado.** Quinto paso de Fase 5b. Cierra la brecha más visible
entre `fitz run` y `fitz build`: los programas con `import foo` /
`from foo import X` ya compilan a binario nativo. Habilita el cap
16 de la guía end-to-end con `fitz build`.

**Decisión clave de pipeline**: pasamos de `rustc` directo a
**siempre generar un Cargo project**. Trade-off aceptado: la
primera compilación cuesta ~1-2s más. Justificación:
- Los imports cross-archivo necesitan múltiples `.rs` con `mod`,
  que es la abstracción nativa de Cargo.
- Cuando llegue 5b.6 con axum/tokio, las deps se suman al
  `Cargo.toml` generado sin reescribir pipeline.
- Cargo cachea incremental — segunda compilación rápida.

**Estructura del project generado**:
```
target/fitz-build/<stem>/
├── Cargo.toml         # [package] / [bin] / sin deps por ahora
└── src/
    ├── main.rs        # mod foo; use foo::{...}; + fn main()
    ├── foo.rs         # pub fn / pub struct / pub type / pub const
    └── ...
```
El binario final se copia adyacente al `.fitz` original. Sanitización
del nombre del crate: `02-hola.fitz` → crate `fitz_02-hola`, binario
adyacente `02-hola.exe` (el stem original se preserva).

**Implementación del codegen**:
- Nuevo `ModuleLoader` con cache por path canonicalizado y
  stack de loading para detección de ciclos. Para cada import,
  lee + lexea + parsea + chequea el módulo y lo genera como
  Rust en modo `Module` (todo `pub`, sin `fn main()`).
- Nuevo `GenMode { Main, Module }` en el `CodegenCtx`: `Module`
  marca todas las defs top-level como `pub` y soporta
  `let X = <literal>` → `pub const`/`pub static`.
- **Bindings cross-module**:
  - `import foo` → `mod foo;` + binding namespace. `foo.greet(x)`
    se traduce a `foo::greet(x)` Rust.
  - `from foo import User` → `mod foo;` + `use foo::{User,
    UserData};`. Permite usar `User { ... }` en el importer
    porque el codegen importa el data struct también.
  - `from foo import greet` (fn) → `use foo::greet;` con la firma
    del módulo para resolver la llamada.
  - `from foo import PREFIX` (const Str) → `use foo::PREFIX;`,
    consumido como `String::from(PREFIX)`.
- **Enriquecimiento de TypeEnv del importer**: el checker
  registra los tipos importados sin fields (no carga el módulo).
  El codegen copia los fields del módulo cargado al
  `fields_by_id` del importer, manteniendo el `TypeId` del
  importer — así `User { id: 1 }` y `u.id` resuelven correcto.
- **Top-level del módulo**:
  - `type X { ... }` → `pub struct XData` + `pub type X = ...`
    + `impl Display`.
  - `fn f(p: T) -> U` → `pub fn` (anotaciones requeridas —
    deuda 5b.1).
  - `let X = <literal>` → `pub const X: T` (primitivos) o
    `pub static X: &str` (Str). El módulo pre-registra las
    consts antes de emitir bodies, así una fn del módulo puede
    referenciar la const en su cuerpo.
  - RHS no literal o stmts no `type`/`fn`/`let` → error de
    codegen citando "5b.5" como deuda.

**Limitaciones aceptadas en 5b.5**:
- **Imports transitivos no soportados**: un módulo cargado por
  el main no puede tener su propio `import`. Loader aborta con
  mensaje claro citando 5b.5 como deuda residual. Workaround:
  aplanar imports al main.
- **`fitz build` con cap 14 / cap 16**: ambos requieren
  anotaciones de tipo en fns. El cap 16 (`guide_utils.fitz`) se
  actualizó: `fn greet(name: Str) -> Str => ...`. El intérprete
  sigue infiriendo.

**Tests**: 6 unit nuevos en `src/codegen.rs` (modo Module emite
`pub` en struct/alias/fn, `let` top-level → static/const, RHS no
literal aborta, fn body referencia const local) + 5 E2E nuevos
en `tests/compile_e2e.rs` (`from import` type+fn, `from import`
const Str, `import` namespace con fn, módulo inexistente aborta,
módulo con `import` propio aborta por transitividad). Reapunté
el E2E "feature no soportada en codegen" desde `import` a `@get`
(5b.6).

**Validación bit-a-bit**: `examples/guide/16-modulos.fitz` +
`guide_utils.fitz` (con `greet` anotado) compilan con `fitz
build` y producen output idéntico a `fitz run`.

**Deuda explícita que sigue post-5b.5**:
- **Imports transitivos**: un módulo importado puede tener
  imports propios. Quitar la restricción requiere recursar el
  loader sin perder el binding cross-archivo. Sub-paso futuro
  si aparece presión.
- **`import foo as f`** (aliases) y **`from foo import X as Y`**:
  no soportado.
- **`foo.User { ... }`** (struct literal con path): el parser
  no acepta `Path { ... }`. Workaround: `from foo import User`.
- **`let X = <expr>`** no literal a nivel mod: deuda 5b.5.
- **Inferencia de tipos de params** en fns sin anotar: deuda
  vieja de 5b.1, sigue.

##### 5b.6 — HTTP / `@server` / handlers
**Pendiente** — los decoradores `@get`/`@post`/etc. se traducen
a registración en una `Router` axum dentro del `main`. `async
fn` real cuando llegue. Bridge sync/async puede simplificarse
porque tokio + axum corren todo async nativamente. Probablemente
suma `[dependencies] axum = "0.8"` + `tokio` + `serde_json` al
Cargo.toml generado.

##### 5b.7 — Guía + ejemplos + cierre de Fase 5b
**Pendiente** — capítulo nuevo de la guía sobre `fitz build`,
ejemplos compilados, criterio de éxito final ("CRUD HTTP
compilado a binario standalone"), cierre formal de Fase 5.

### Features de la fase entera
- [x] TypeExpr en AST y parser (5.1)
- [x] Resolución de tipos y checker base (5.2)
- [x] Checker de expresiones — synthesis básico (5.3.1)
- [x] Llamadas y return contra return_type (5.3.2)
- [x] Result, `?`, match exhaustivo (5.3.3)
- [x] Métodos built-in con templates paramétricos (5.3.4)
- [x] FnExpr.ret inferido + Expr::Index + cierre formal de 5.3 (5.3.5)
- [x] Modo strict y cierre de 5a (5.4)
- [x] Inferencia de tipos básica (synthesis de expresiones,
  unión de returns en FnExpr — la inferencia bidireccional más
  rica queda como deuda)
- [x] Backend de codegen decidido — transpile-a-Rust (5b)
- [x] Codegen subset primitivo + `fitz build` (5b.1)
- [x] Tipos custom + field access (5b.2)
- [x] Listas, mapas, indexing, métodos built-in (5b.3)
- [x] Result, `?`, match (5b.4)
- [x] Módulos / `import` (5b.5)
- [ ] HTTP / `@server` / handlers (5b.6)
- [ ] Optimizaciones básicas (post-5b — strings sin `.clone()`,
  pre-declaración de vars que cruzan bloques)
- [x] Binario nativo standalone (5b.1 — subset primitivo;
  features faltantes en 5b.2-5b.6)
- [x] Cross-compilation (gratis via rustc targets)

---

## Fase 6 — Interop Python 🐍
**Estado: PROPUESTA — no comprometida**

Una vez cerrada la Fase 5b, Fitz va a tener HTTP nativo, type
checker estricto y binarios standalone — pero no va a poder
hablarle a una base de datos, ni a NumPy/pandas, ni a librerías
de criptografía o de scraping. Construir desde cero todo el
ecosistema de un lenguaje de producción tomaría años, y
mientras tanto Fitz quedaría como lenguaje de juguete para
APIs in-memory.

La Fase 6 abre una puerta lateral: importar librerías de Python
desde código Fitz para heredar el ecosistema, mientras Fitz
construye su propio stack a su ritmo.

El caso de uso motivador es concreto: un proyecto Fitz con
handlers `@get`/`@post` que adentro usan SQLAlchemy contra
Postgres, mapeando los modelos del ORM a `type` de Fitz para
que el checker siga validando los handlers end-to-end.

### Posicionamiento estratégico

Esta es una decisión más política que técnica, y la propuesta
la pone arriba de todo a propósito: **Fitz NO se vuelve "Python
con sintaxis distinta"**.

- El código Fitz sigue compilando a binario nativo via 5b.
- El checker sigue mandando sobre el código Fitz.
- HTTP sigue siendo decoradores del lenguaje.
- Result + match + `?` siguen siendo el modelo de errores.

Python entra como **backend de librerías**, no como identidad
del lenguaje. La regla operativa: si los usuarios terminan
escribiendo handlers en `def` con sintaxis Python adentro de
archivos `.fitz`, perdimos. Si escriben `fn` Fitz que llaman a
`session.query(...)` y arman la respuesta con `Result<User>`,
ganamos.

Esta regla se traduce en concretos para la propuesta:
- La guía nueva (6.8) ilustra el patrón "Python para librerías
  pesadas, Fitz para todo lo demás" explícitamente.
- Los ejemplos canónicos mantienen handlers `fn` Fitz puros que
  consumen Python solo donde no hay alternativa nativa.
- La documentación enumera qué partes del stack van a migrar a
  Fitz nativo en fases futuras (DB driver, ORM) — Python es el
  puente, no el destino.

### Caso de uso canónico

Lo concreto que esta fase tiene que habilitar:

```fitz
from python import sqlalchemy as sa
from python.sqlalchemy.orm import Session

type User { id: Int, email: Str, name: Str }

let engine = sa.create_engine("postgresql://localhost/app")

@get("/users/{id}")
fn get_user(id: Int) -> Result<User> {
    let session = Session(engine)
    let row = session.query(UserModel).filter_by(id: id).first()?
    return Ok(User { id: row.id, email: row.email, name: row.name })
}

@post("/users")
fn create_user(body: UserInput) -> Result<User> {
    let session = Session(engine)
    let model = UserModel(email: body.email, name: body.name)
    session.add(model)
    session.commit()?
    return Ok(User { id: model.id, email: model.email, name: model.name })
}
```

La sintaxis exacta es propuesta — cada sub-paso puede afinarla.

### Pasos

#### 6.1 — Embedding básico de CPython
**Pendiente** — embeber CPython en el runtime de Fitz via PyO3.
Punto de partida de toda la fase. Sólo cubre el caso más
chiquito: importar un módulo Python y llamar funciones top-level
con argumentos y returns primitivos (Int, Float, Str, Bool).

- Nueva sintaxis: `from python import <módulo>`. El parser
  reconoce `python` como prefijo reservado para imports y
  produce un nodo nuevo `Stmt::PyImport { path, alias }` (o
  reutiliza `FromImport` con un flag, decisión a tomar al
  implementar).
- `from python import sqlalchemy as sa` — aliasing en imports.
  Fitz hoy no lo soporta; cerrar esta sub-deuda como parte de
  6.1 destraba nombres largos de módulos Python que serían
  insufribles sin alias.
- Nuevo valor en runtime: `Value::PyObject(Py<PyAny>)` que
  envuelve un objeto Python con su referencia counted.
- El evaluator detecta `Expr::Call` sobre un `Value::PyObject`
  (función) y cruza al runtime Python via `Python::with_gil`.
  Args se convierten a `PyObject` (sólo primitivos en 6.1),
  return se convierte de vuelta a `Value` Fitz.
- Build modes: `cargo build --features python` activa el
  embedding. Sin la feature, los imports de Python disparan
  error explícito mencionando que el binario no se compiló con
  soporte de interop. Esto preserva la promesa "binario nativo
  standalone" para proyectos que no la necesitan.

**Criterio de éxito**:
```fitz
from python import math
print(math.sqrt(16.0))  // 4
print(math.pi)          // 3.141592653589793
```

**Decisiones técnicas a resolver**:
- Versión mínima de CPython: probablemente 3.10 con ABI3 para
  amortizar versiones futuras.
- Si Fitz embebe CPython solo en el binario generado por
  `fitz build` o también en el binario del compilador (`fitz
  run`). Probablemente ambos, con la feature `python` activada
  por default y switch para desactivarla.
- Cómo el lexer distingue `python` como prefijo de import vs
  un nombre normal. Una opción: keyword contextual (sólo
  especial después de `from`). Otra: reservar `python` como
  identifier prohibido para módulos del usuario.

#### 6.2 — Marshaling de tipos compuestos
**Pendiente** — extender las conversiones bidireccionales a
List, Map, Instance y Null. Sin esto, los casos reales no son
viables.

- `List<T> ↔ list`: copia eager elemento por elemento. T
  concreto requerido del lado Fitz (`List<Any>` cae al modelo
  gradual).
- `Map<K, V> ↔ dict`: copia eager. K debe ser hashable en
  Python (Str/Int/Bool/Float). K = tipo nominal → error con
  mensaje claro.
- `Instance ↔ dict`: traducción nominal vía nombres de campo.
  De Fitz a Python: `User { id: 1, name: "x" }` se convierte
  a `{"id": 1, "name": "x"}`. De Python a Fitz: dict con
  campos compatibles con el tipo declarado del receptor. Campos
  faltantes (nullable o con default) se completan; faltantes
  no-nullables → error.
- `Null ↔ None`.
- Tipos no marshalleables (Range, Function, otros PyObject
  anidados) → error explícito al cruzar la frontera.

**Criterio de éxito**: una función Python que recibe
`List<User>` y devuelve `Map<Str, Int>` (un `count_by_email`,
por ejemplo) tipa limpio en Fitz y se ejecuta sin perder data.

**Decisiones técnicas a resolver**:
- **Identidad vs referencia**: ¿`Rc<RefCell<List<T>>>` de Fitz
  comparte estado con la `list` Python o se copia? El intérprete
  Fitz tiene aliasing real entre instancias. Recomendación
  para 6.2: **copia eager bidireccional**. Trade-off conocido
  (caro para listas grandes), pero evita pesadillas de lifetime
  entre dos GCs (refcount Rc + GC Python) y race conditions
  GIL/tokio. Optimizaciones zero-copy con buffers protocolo
  quedan para Fase 7+.
- **Costo de marshaling**: una llamada Python con N args
  primitivos + ret compuesto cuesta O(tamaño del ret) por la
  copia. Cuantificable; las queries SQLAlchemy típicas
  devuelven decenas de filas con pocos campos, entra cómodo en
  el presupuesto de latencia de un endpoint HTTP.

#### 6.3 — Excepciones Python → Result<T>
**Pendiente** — convención automática: **toda** llamada a una
función Python desde Fitz se envuelve en `Result<T>`. Si Python
lanza `ValueError("x")`, Fitz lo recibe como `Err("ValueError:
x")`. Esto preserva la decisión de diseño "sin excepciones"
intacta y evita que excepciones Python escapen al runtime de
Fitz como panics opacos.

- Implementación: `Python::with_gil` envuelve la llamada en un
  `match` sobre `PyResult<T>`. `Err(PyErr)` se serializa al
  string del lado Fitz como `<ClassName>: <message>`.
- El checker (6.4) refleja esta convención: una llamada Python
  cuyo tipo de retorno no esté anotado tipa como `Result<Any>`,
  no como `Any`. El usuario es forzado a manejar el error
  (`match`, `?`) — el modelo de errores de Fitz se preserva.
- KeyboardInterrupt, SystemExit y otras excepciones "de
  control" propagan como Err también — no hay forma de matar
  el runtime Fitz desde una excepción Python.

**Criterio de éxito**:
```fitz
from python import json

fn parse(input: Str) -> Result<Map<Str, Any>> {
    return json.loads(input)  // implícito: Result<Map<Str, Any>>
}

match parse("{ malformado") {
    Ok(m)  => print("ok: {m}"),
    Err(e) => print("error: {e}")  // "error: JSONDecodeError: Expecting value..."
}
```

**Decisiones técnicas a resolver**:
- Formato del string de error: `<ClassName>: <message>` para
  6.3, legible humano. Si más adelante hace falta acceso
  estructurado (type, message, traceback), agregar un valor
  `Value::PyException` opaco que el usuario puede inspeccionar
  con métodos dedicados. Para 6.3 el string alcanza.
- ¿Algún caso donde NO queremos envolver en Result?
  Probablemente no — la consistencia es más valiosa que el
  optimismo. Funciones Python que "nunca fallan" igual pueden
  fallar (out-of-memory, etc.); envolver siempre es seguro.

#### 6.4 — Tipos del lado del checker (anotaciones y opacidad)
**Pendiente** — el checker estático no puede ver dentro de
Python. Sin estrategia explícita, todas las llamadas Python
serían `Result<Any>` y el valor del checker se diluiría en
proyectos que dependen mucho de interop.

**Estrategia escalonada**:

1. **Default**: una llamada `python_module.func(args)` sin
   anotación tipa como `Result<Any>`. El Result viene del 6.3
   (automático), el Any preserva el modelo gradual de Fitz.
2. **Anotación explícita del lado Fitz**: el usuario anota el
   binding sitio con un tipo Fitz, y el checker valida que la
   conversión es posible (las reglas del marshaling de 6.2).
   ```fitz
   let row: User = session.query(UserModel).filter_by(id: id).first()?
   ```
   Acá el `?` desempaca el `Result<Any>` a `Any`, y la anotación
   `: User` coerciona Any→User. El runtime valida que el dict
   Python tiene los campos requeridos.
3. **Stubs `.pyi` parseados** (pospuesto a Fase 7+): leer
   stubs PEP 561 y traducirlos automáticamente. Esto sería el
   equivalente a `@types/...` de TypeScript. Mucho trabajo;
   queda como upgrade futuro.

**Criterio de éxito**: el caso de uso canónico (CRUD con
SQLAlchemy) chequea con tipos Fitz reales (`User`, no `Any`)
usando solo anotaciones explícitas en los puntos donde Python
cruza a Fitz. Sin necesidad de stubs.

**Decisiones técnicas a resolver**:
- Sintaxis para tipos opacos de Python. Opciones consideradas:
  - `Any` (gradual, sin info) — más simple, recomendado para
    arrancar.
  - `PyObject` (tipo dedicado, opaco) — distingue "objeto
    Python" de "valor desconocido Fitz", útil para el checker.
  - `PyObject<"sqlalchemy.Engine">` (string fantasma) —
    permite distinguir engines de sessions sin saber su
    estructura.
  - Recomendación: empezar con `Any` opaco + anotaciones
    explícitas en bindings. Promover a `PyObject<...>` si la
    fricción aparece en proyectos reales.
- Métodos sobre objetos Python (`engine.connect()`): hoy `Expr::Call`
  con `callee: Expr::Field` sobre `Any` cae a gradual sin
  chequear. La cadena ya funciona sin cambios al checker — los
  imports de Python se benefician del comportamiento gradual
  existente. No hace falta nuevo dispatch.

#### 6.5 — Auto-mapeo de modelos SQLAlchemy a `type` de Fitz
**Pendiente** — caso especial pero crítico para la ergonomía
del caso canónico. Una clase `class User(Base): id =
Column(Integer); ...` debería poder usarse desde Fitz como
`type User { id: Int, ... }` sin escribir el `type` a mano
dos veces.

**Estrategia**: herramienta separada `fitz py-types
<archivo.py> [--out <archivo.fitz>]`. La herramienta:
1. Importa el archivo Python en un subprocess.
2. Introspecciona `Base.metadata.tables` o subclasses de
   `DeclarativeBase`.
3. Por cada modelo, emite un `type` Fitz con los campos
   traducidos:
   - `Column(Integer)` → `Int`
   - `Column(String)` → `Str`
   - `Column(Float)` → `Float`
   - `Column(Boolean)` → `Bool`
   - `Column(DateTime)` → `Str` (ISO 8601) por ahora; tipo
     `DateTime` nativo es deuda Fase 7+
   - `nullable=True` → `T?`
   - `default=...` → valor por defecto del campo
4. El archivo generado es commiteable e inspeccionable. No
   hay magia en build time.

**Criterio de éxito**:
```bash
fitz py-types models.py --out models.fitz
# models.fitz ahora tiene:
#   type User { id: Int, email: Str, name: Str }
#   type Order { id: Int, user_id: Int, total: Float }
```
Y `from models import User, Order` adentro del proyecto Fitz
funciona como cualquier otro import.

**Decisiones técnicas a resolver**:
- Herramienta separada vs integración: la separada es más
  simple y los archivos generados se versionan. Integración
  (`from python.sqlalchemy import models as fitz_types`) es
  más mágica pero introduce side effects en compile time.
  Recomendación: arrancar con herramienta separada; promover
  a integración si la fricción de "olvidé regenerar los
  tipos" molesta en uso real.
- Cobertura de otros ORMs: Django, Tortoise, Pony, peewee. Empezar
  con SQLAlchemy (caso canónico). La arquitectura (introspección
  + generación de `type`) se puede reusar; sub-comandos
  específicos (`fitz py-types-django`) si hace falta.
- Sincronía con cambios del schema: si el schema Python cambia,
  ¿`fitz check` avisa que `models.fitz` está desactualizado?
  Probablemente no en 6.5 — flujo manual de regeneración.
  Verificación automática podría llegar como linter regla en
  Fase 7+.

#### 6.6 — Async + GIL — bridge tokio ↔ asyncio
**Pendiente** — los handlers HTTP de Fitz corren sobre tokio
(axum, desde Fase 4.2). SQLAlchemy 2.x tiene API async basada
en asyncio. Llamar una corutina asyncio desde un task tokio no
es trivial: requiere ejecutar el event loop de asyncio en un
thread dedicado y schedular las corutinas en él. El paquete
`pyo3-asyncio` resuelve esto.

- Sintaxis: handlers `async fn` (cuando 5b.6 cierre con async
  real) pueden hacer `await py_coro()` sobre una corutina
  Python. Por debajo, `pyo3-asyncio::tokio::into_future`
  convierte la corutina en un `Future<Output = PyResult<T>>`
  awaiteable desde tokio.
- Bridge invisible al usuario: desde Fitz se ve igual que
  await sobre un future nativo.

**Riesgo central — el GIL**: serializa todo el código Python.
Aunque tengamos 100 conexiones HTTP concurrentes en axum,
cuando todas pasen por `await session.execute(...)` van a
serializarse en el GIL del lado Python. Para una API
mayormente DB-bound (el caso típico de CRUD), esto es
aceptable — la DB es el cuello de botella, no la CPU. Para
una API CPU-intensiva con NumPy/pandas, podría ser fatal y
hace falta soltar el GIL en las llamadas que se sabe que
esperan I/O.

**Criterio de éxito**:
```fitz
@get("/users/{id}")
async fn get_user(id: Int) -> Result<User> {
    let session = AsyncSession(engine)
    let row = await session.execute(
        "SELECT * FROM users WHERE id = $1",
        [id]
    )?
    return Ok(User { id: row.id, email: row.email, name: row.name })
}
```
50 requests concurrentes a este endpoint completan sin
deadlock, con latencia razonable (P99 dominado por la DB,
no por contención GIL).

**Decisiones técnicas a resolver**:
- Política de GIL por default: ¿soltarlo en cada llamada
  (más concurrencia, overhead) o mantenerlo (menos overhead,
  menos concurrencia)? PyO3 ofrece ambos vía
  `Python::allow_threads`. Recomendación: soltar GIL
  automáticamente cuando la llamada es a una corutina (caso
  típico de I/O), mantener para llamadas síncronas (caso
  típico de cómputo corto). Anotación opt-in
  (`@release_gil`?) para casos donde el default no encaje.
- Compatibilidad con 5b.6: el `async fn` que Fitz introduzca
  como sintaxis tiene que componer con interop Python sin
  sorpresas. El bridge `pyo3-asyncio` ya conoce tokio; el
  trabajo es generar el código de adapter correcto desde
  codegen.

#### 6.7 — Distribución del binario con CPython embebido
**Pendiente** — un binario Fitz que use interop Python necesita
CPython disponible en runtime. La promesa "binario nativo
standalone" de la Fase 5b se mantiene como **modo de build
disponible** (proyectos sin interop), pero proyectos con
interop tienen tres opciones de distribución:

1. **CPython preinstalado en el sistema** (default 6.1-6.6,
   simple, peor experiencia): el binario asume `python3.X` en
   el `PATH`. Falla en máquinas que no lo tienen.
2. **CPython embebido en el binario** (PyOxidizer o
   equivalente, mejor experiencia, más tamaño): el binario
   lleva su propio intérprete + standard library. ~10-30MB
   extra de tamaño. Sin dependencias externas en runtime.
3. **Docker base con Python** (compromise, cloud-native): el
   binario espera Python en runtime, el contenedor lo provee.
   Útil cuando ya estás deployando en contenedores.

**Recomendación**: empezar con (1) durante desarrollo y
testing (más rápido de iterar); agregar `fitz build
--bundle-python` para emitir (2) cuando 6.7 cierre. Documentar
(3) como pattern para deployments containerizados.

**Criterio de éxito**: `fitz build api.fitz --bundle-python`
produce un binario standalone que corre en un sistema fresh
install (Ubuntu / Alpine / Windows sin Python instalado) y
sirve el CRUD canónico de 6.5 contra una Postgres remota.

**Decisiones técnicas a resolver**:
- Herramienta de empaquetado: PyOxidizer es la opción
  mainstream Rust-friendly, pero el proyecto se ralentizó en
  2024-2025. Verificar estado al arrancar 6.7. Alternativas:
  scripts custom que copian el `python3X.so` + standard lib +
  archivo zip; o cosign de un release de python-build-standalone.
- Librerías Python con extensiones C (numpy, pandas, psycopg2):
  empaquetarlas standalone es notoriamente difícil. La
  documentación de la fase debe ser honesta sobre qué funciona
  out-of-the-box vs qué requiere venv preconfigurado.
- Tamaño aceptable: ~30MB extra es razonable para un binario
  de API. Si llegamos a 200MB porque NumPy+pandas+SQLAlchemy
  cargan todo el universo, replantear.

#### 6.8 — Guía + ejemplos + cierre de Fase 6
**Pendiente** — capítulo nuevo en `docs/guide.md` titulado
"Interop Python", probablemente entre el cap 17 (HTTP nativo)
y el 18 ("Qué sigue"). Ejemplo ejecutable
`examples/guide/19-python.fitz` (numeración tentativa) con un
CRUD completo usando SQLAlchemy contra Postgres. Setup
auxiliar `examples/guide/19-python.setup/` con `docker-compose.yml`
para levantar Postgres local y `models.py` con los modelos
SQLAlchemy. Actualización del cap "Qué sigue" para reflejar
que la interop ya está cubierta.

**Criterio de éxito**: un usuario que sabe Python básico + ha
leído los caps 1-18 de la guía puede:
1. Leer el cap nuevo (estimado 20-30 min de lectura).
2. Correr `docker compose up` en el setup.
3. Correr `fitz py-types models.py --out models.fitz`.
4. Correr `fitz run 19-python.fitz`.
5. Hacer `curl localhost:3000/users` y ver respuesta tipada.

Sin pasos intermedios fuera de la guía.

**Cierre formal de Fase 6**: todas las features de la lista de
abajo marcadas, los 18 ejemplos existentes pasan `fitz check`
sin regresiones, el ejemplo nuevo `19-python.fitz` corre
end-to-end contra una Postgres real, y `examples/server.fitz`
opcionalmente se reescribe para usar SQLAlchemy en lugar de
state in-memory (decisión a tomar al cerrar — el server.fitz
actual puede quedar como referencia de modo standalone).

### Decisiones cross-cutting

Estas decisiones no caen en un sub-paso específico; son
consistencias que la fase entera tiene que mantener.

1. **Sintaxis de import desde Python**. Opciones consideradas:
   - **(A)** `from python import sqlalchemy` (namespace virtual
     `python` reservado).
   - **(B)** `import py:sqlalchemy` (prefijo de scheme estilo
     URI).
   - **(C)** `@python use sqlalchemy` (decorator a nivel
     módulo).
   - Recomendación: **(A)**. Reusa la sintaxis `from X import Y`
     que el usuario ya conoce, no introduce caracteres
     especiales nuevos, el AST puede reusar `Stmt::FromImport`
     con un discriminante. Es la forma más cercana a
     "se siente igual que un import normal pero el target es
     Python".

2. **Aliasing en imports (`as`)**. Fitz hoy no lo soporta para
   imports normales; agregarlo es necesario para Python (los
   módulos `sqlalchemy.orm.declarative_base`, etc., son
   imposibles de usar sin alias). Decisión: cerrar la sub-deuda
   en 6.1 — agregar `as` para imports normales también, no
   solo Python. Beneficio bonus para el sistema de módulos
   Fitz existente.

3. **Dependencia Python como condicional**. Un proyecto Fitz
   que no usa `from python import` no debería pagar nada
   (tamaño del binario, tiempo de compilación, dependencia
   runtime). Implementación: el codegen detecta presencia de
   imports Python en el AST; sólo entonces emite el código de
   embedding y la dependencia a PyO3 en `Cargo.toml`. Build
   sin interop = binario igual al de la Fase 5b. **Esta
   decisión preserva la promesa "binario nativo standalone"
   como modo por default.**

4. **Identidad en marshaling**: cubierto en 6.2. Default:
   copia eager bidireccional, sin aliasing entre los dos
   runtimes. Optimizaciones (zero-copy) son trabajo de Fase
   7+.

5. **Herencia desde clases Python**: explícitamente **NO
   soportada** en Fase 6. Un `type` de Fitz no puede heredar
   de una clase Python. Composición sí — un `type` puede
   tener un campo `engine: Any` que envuelve un objeto Python.
   La razón: herencia cruza modelos de objetos (Python tiene
   MRO, Fitz no tiene clases) y abre una caja de pandora que
   no aporta al caso de uso central.

6. **Versionado de Python soportado**: ABI3 para amortizar
   versiones. Mínimo Python 3.10 (versión más vieja con
   soporte upstream a fecha de cierre de la fase; revisar al
   arrancar 6.1).

7. **Métodos sobre objetos Python (`obj.method()`)**: el
   dispatch sobre Any/PyObject cae al modelo gradual del
   checker existente (5.3.4). Sin trabajo nuevo. Anotaciones
   explícitas siguen disponibles para casos donde se quiere
   precisión.

### Trade-offs reconocidos

La fase tiene costos reales que vale la pena enumerar honestamente:

- **Tensión con "binario nativo standalone"** (cap "Lo que
  Fitz NO es" de vision.md). Resolución: dos modos de build
  — puro nativo (sin interop, igual que Fase 5b) y nativo +
  CPython embebido (con `--bundle-python`). El usuario elige
  al deployar. La promesa original se preserva como modo
  default; la interop es opt-in al nivel del proyecto.
- **El GIL limita la concurrencia** justo donde Fitz quiere
  brillar (handlers HTTP concurrentes). Mitigado en 6.6 con
  soltado oportunístico del GIL en llamadas async, pero es
  un techo real para APIs CPU-intensivas en Python.
- **Costo de marshaling** Rust↔Python en cada cruce. Un
  handler que hace 5 queries SQLAlchemy genera 10+ cruces.
  Cuantificable; los benchmarks deberían formar parte del
  criterio de cierre de 6.6.
- **Riesgo de identidad del lenguaje**: si los usuarios usan
  Python para TODO (HTTP, queries, JSON, lógica de negocio),
  Fitz queda como cáscara sintáctica. Mitigación: la guía y
  los ejemplos muestran el patrón explícitamente —
  "Python para librerías pesadas, Fitz para lo demás" — y
  los handlers de la guía siguen siendo `fn` Fitz puros con
  Result, no envoltorios delgados.
- **Tooling complejo**: la fase introduce dos artefactos
  nuevos (CPython embebido, herramienta `fitz py-types`),
  dos dependencias externas pesadas (PyO3,
  posiblemente PyOxidizer), y una superficie nueva de bugs
  que cruzan dos runtimes (refcount Rc + GC Python).
  Estimar tiempo de implementación realista: probablemente
  meses, no semanas.

### Precedentes consultados

Lenguajes y proyectos de los que esta fase aprende:

- **PyO3** — la biblioteca base. Madura, mantenida, con tres
  patrones de uso (extension modules, embedding, shared
  layout). Fitz usa embedding como base, eventualmente puede
  sumar extension modules para "Fitz como librería de
  Python" en una fase futura.
- **pyo3-asyncio** — bridge tokio ↔ asyncio. Crítico para
  6.6.
- **PyOxidizer** — para 6.7, empaquetado de CPython en
  binarios standalone. Revisar estado del proyecto.
- **Mojo** (Modular) — superset sintáctico de Python con
  interop directa. Lección: ellos eligieron compatibilidad
  sintáctica total. Fitz NO tiene esa restricción (y la
  rechaza explícitamente — ver "Posicionamiento estratégico").
- **Nim + nimpy** — precedente más directo. Lenguaje
  compilado a nativo que importa Python. Resolvió bien la
  sintaxis (`pyImport`) y el marshaling automático. Vale la
  pena leer el código de nimpy antes de 6.1.
- **Julia + PyCall** — lenguaje JIT con interop Python.
  Resolvió el GIL con threading separado. Aplicable
  parcialmente (Fitz no es JIT, pero el modelo de GIL
  traduce).
- **Crystal + Python bindings** — lenguaje compilado con
  macros para envolver código externo. Menos relevante para
  el caso DB, más para extender el lenguaje con C.

### Riesgos

- **Magnitud de la fase**: probablemente la más larga del
  proyecto hasta acá. Estimación gruesa: 3-6 meses de
  trabajo enfocado, muy por encima de cualquier fase de la
  5b. Antes de arrancar 6.1, decidir explícitamente si la
  fase entra completa o se parte en (6 — embedding y
  marshaling, suficiente para casos simples) + (7 —
  SQLAlchemy/async/bundling, llevándolo al caso de uso
  canónico). Decisión a tomar al cerrar 5b.
- **Dependencia externa pesada**: la fase atan Fitz a PyO3,
  al equipo PyO3, y por extensión al ABI de CPython. Si
  CPython cambia su API de embedding (cosas como GIL-free
  Python en 3.13+), Fitz se ve afectado. Mitigación: ABI3
  amortiza versiones, pero no protege contra cambios
  estructurales.
- **Fragmentación de proyectos**: parte de los proyectos
  Fitz usan interop, parte no. Convivencia, distribución,
  CI, todo se complica. La decisión cross-cutting #3
  (dependencia condicional) mitiga pero no elimina.
- **Riesgo de canibalización**: si interop Python es
  demasiado bueno, ¿por qué alguien escribiría una librería
  en Fitz puro? Mitigación: la performance del código Fitz
  nativo tiene que seguir siendo claramente mejor que llamar
  a Python (binario sin GIL, sin marshaling), y la
  ergonomía de Fitz nativo (HTTP, tipos, Result) tiene que
  estar varios pasos arriba de lo equivalente en Python.

### Alternativa explícita: ORM y stack DB nativos (Fase 8+)

A futuro, Fitz debería tener su propio stack de DB nativo:
- Driver Postgres en Fitz puro (bindings directos a `libpq`
  o port de `tokio-postgres` al codegen de Fitz).
- ORM nativo declarativo sobre `type` (estilo Diesel o sqlx).
- Migraciones, pool de conexiones, async nativo end-to-end.

Eso es un proyecto en sí mismo, probablemente Fase 8+. La
interop Python de la Fase 6 es **el puente** hasta llegar
ahí, no el destino final. Vale la pena decirlo explícitamente
en la documentación que la fase produce: "interop existe
para que Fitz sea usable hoy; el stack nativo llega cuando
lleguemos".

### Features de la fase entera
- [ ] Embedding básico de CPython + sintaxis `from python
  import` (6.1)
- [ ] Aliasing en imports (`as`) — sub-deuda de 6.1 que
  beneficia a imports normales también
- [ ] Marshaling List/Map/Instance/Null ↔ list/dict/None
  (6.2)
- [ ] Excepciones Python → Result<T> automático (6.3)
- [ ] Anotaciones explícitas del lado Fitz + opacidad PyObject
  (6.4)
- [ ] Stubs `.pyi` — pospuesto a Fase 7+
- [ ] Auto-mapeo SQLAlchemy → `type` via `fitz py-types` (6.5)
- [ ] Bridge tokio ↔ asyncio + política de GIL (6.6)
- [ ] Distribución con `--bundle-python` (6.7)
- [ ] Guía + ejemplo CRUD + cierre formal (6.8)

---

## Fase 7 — Ecosistema 🌍
**Estado: VISIÓN FUTURA**

- [ ] Package manager (`fitz add`)
- [ ] Fitz registry (repositorio de paquetes)
- [ ] LSP (Language Server Protocol) — autocompletado en VSCode
- [ ] Formatter (`fitz fmt`)
- [ ] Linter (`fitz check` ya cubre tipos; queda lint de estilo
  y patrones)
- [ ] Stubs `.pyi` para interop Python (pospuesto desde Fase 6)
- [ ] Driver Postgres nativo (paso previo al ORM Fitz, ver Fase 8+)
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
