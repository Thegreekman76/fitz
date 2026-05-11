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
