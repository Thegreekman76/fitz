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
**Estado: EN CURSO**

Plan aprobado: dos mitades cerrables.
- **5a — Type checker estático** (sobre el intérprete actual)
- **5b — Codegen a binario nativo** — backend a decidir. Recomendación
  inicial: Cranelift (pure-Rust, sin LLVM en Windows). Alternativa
  seria: transpile-a-Rust por reuso de tokio/axum/serde_json.
  Decisión real al arrancar 5b, después de fijar el IR tipado.

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

#### 5.3 — Type checker de expresiones y funciones
**En curso** — se divide en cinco sub-pasos cerrables.

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

##### 5.3.4 — Métodos built-in con templates paramétricos
**Pendiente** — `xs.map(f)`, `m.get(k)`, `s.upper()`, etc. Cada
método con su signature template (`List<T>.map(fn(T) -> U) -> List<U>`).

##### 5.3.5 — FnExpr ret inferido + cierre de 5.3
**Pendiente** — refinar `Expr::FnExpr` para sintetizar `ret`
desde el body. Cierre formal de 5.3.

### Features de la fase entera
- [x] TypeExpr en AST y parser (5.1)
- [x] Resolución de tipos y checker base (5.2)
- [x] Checker de expresiones — synthesis básico (5.3.1)
- [x] Llamadas y return contra return_type (5.3.2)
- [x] Result, `?`, match exhaustivo (5.3.3)
- [ ] Checker completo (5.3.4 → 5.3.5 + 5.4)
- [ ] Inferencia de tipos
- [ ] Optimizaciones básicas (5b)
- [ ] Binario nativo standalone (5b)
- [ ] Cross-compilation (5b)

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
