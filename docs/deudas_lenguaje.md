# Deudas del lenguaje base — plan de cierre (mini-fase R)

> Documento creado el 2026-05-17 al cerrar Fase 9.z entera + refresh
> masivo de docs. Se mantiene vivo: cada deuda cerrada se marca con
> ~~strikethrough~~ + el sub-paso que la cerró + fecha. Cuando todo
> esté en ~~strikethrough~~, este doc archiva la mini-fase R y se
> queda como referencia histórica.

## Por qué este doc existe

Durante la auditoría post-9.z (2026-05-17) descubrimos que la guía
acumuló secciones "Lo que todavía no anda" cap por cap apuntando a
features del lenguaje base que nunca cerramos. Algunas son
pedagógicamente molestas (no tener `not` lógico, no poder hacer
`xs[0] = v`), otras son grandes (polimorfismo). Antes de saltar a
Fase 9.w (stack web first-class), priorizamos **robustecer el
lenguaje base** para que todo lo que la guía promete tenga
implementación detrás.

La mini-fase **R** (Robustez del lenguaje) tiene 3 tandas
progresivas con commit por tanda. Total estimado: 3-4 días.

## Resumen ejecutivo

| Tanda | Foco | Esfuerzo | Items |
|---|---|---|---|
| **R.1** | Quick wins de sintaxis | ~1 día | 5 |
| **R.2** | Match más expresivo + ops compuestos | ~1 día | 4 |
| **R.3** | Métodos custom sobre `type` | ~1-2 días | 1 (grande) |

Después de R, lo que queda en la sección "Deudas diferidas"
abajo son deudas conocidas que NO entran al MVP del lenguaje
base pero quedan documentadas para sub-pasos futuros.

## Política de testing por cada item

Esta mini-fase toca el **lenguaje base**. Cada item cerrado exige
**tests exhaustivos en 4 niveles**:

1. **Unit tests del parser** (`src/parser.rs::tests::*`):
   happy path + 2-3 casos de error sintáctico (token faltante,
   estructura inválida).
2. **Unit tests del checker** (`src/types.rs::tests::*`): tipos
   correctos + casos de type mismatch + interacción con `match`
   exhaustividad / `Result` / fns con anotación.
3. **Unit tests del evaluator** (`src/evaluator.rs::tests::*`):
   semántica runtime + casos de error claros (out-of-bounds,
   tipo incorrecto en runtime gradual).
4. **Cli_e2e o compile_e2e**: smoke end-to-end con `fitz run` +
   `fitz build` (si aplica al codegen). Validar paridad bit-a-bit
   intérprete/binario cuando el feature toca codegen.

**Mínimo**: 5-10 tests por item, idealmente 10-15 para items que
toquen múltiples capas.

**Smoke manual** además: probar a mano con archivos `.fitz` reales
antes del commit, especialmente para los ítems de interacción
(asignación a índice con type complejo, match con guards
anidados).

## Política de docs + ejemplos por cada item

**No se cierra un item si la guía y los ejemplos no reflejan el
cambio.** Cada item cierra con:

1. **Cap relevante de `docs/guide.md`** actualizado:
   - Sacar el ítem de la sub-sección "Lo que todavía no anda".
   - Sumar documentación + sintaxis + ejemplo inline donde
     corresponda (típicamente en el cap que ya cubre la feature
     vecina).
2. **Ejemplo runnable** en `examples/guide/`:
   - Si el item es chico, **actualizar el ejemplo del cap existente**
     (ej. `04-operadores.fitz` para `%`, `06-logica.fitz` para
     `not`, `09-listas-mapas.fitz` para asignación a índice).
   - Si el item es grande (R.3 métodos custom), **crear ejemplo
     nuevo** (ej. `13b-metodos-custom.fitz`) sumado al smoke
     `GUIDE_EXAMPLES_COMPILE`.
3. **`docs/syntax-spec.md`** actualizado: mover ítem de
   "Diseñado pero no implementado" a la matriz de implementado.
4. **`docs/architecture.md`** actualizado si el item toca AST /
   pipeline / codegen.
5. **`docs/deudas_lenguaje.md`** (este archivo): marcar item con
   ~~strikethrough~~ + fecha + sub-paso que lo cerró.

**Smoke obligatorio antes del commit**: correr el ejemplo
actualizado/nuevo con `fitz run`, validar output, y si el feature
toca codegen también `fitz build` + ejecutar el binario + comparar
output bit-a-bit.

---

## R.1 — Quick wins de sintaxis (~1 día)

### ~~R.1.1 — Operador `not`~~ ✓ CERRADO 2026-05-17

**Hoy**: `not true` no parsea. El lexer trata `not` como identifier
común. Workaround `== false` o invertir comparaciones.

**Esperado**:
```fitz
if not active { print("inactivo") }
let inactive = not user.is_admin
```

**Implementación**:
- Lexer: agregar `Token::Not` (keyword `not`).
- Parser: prefix operator con precedencia entre comparación y
  unary `-`. Sintetiza `Expr::UnaryOp { op: UnaryOpKind::Not, ... }`.
- AST: agregar variante `UnaryOpKind::Not`.
- Checker: tipa solo `Bool`; cualquier otro → type error.
- Evaluator: invierte el `Value::Bool`.
- Codegen: emite `!` Rust.

**Tests**: ~5 unit + 1 cap-style.

### ~~R.1.2 — Operador `%`~~ ✓ CERRADO 2026-05-17 (módulo, cap 4)

**Hoy**: `n % 2` no parsea. Útil en casi cualquier programa.

**Esperado**:
```fitz
if n % 2 == 0 { print("par") }
let resto = total % batch_size
```

**Implementación**:
- Lexer: `Token::Percent`.
- Parser: misma precedencia que `*` y `/`.
- AST: `BinOpKind::Mod`.
- Checker: solo `Int` por simplicidad MVP (no `Float % Float` por
  ambigüedad semántica entre `fmod` y `rem_euclid`).
- Evaluator: `i64::rem_euclid` (mismo signo del divisor — más
  predecible que `%` Rust que usa truncate-toward-zero).
- Codegen: emite `.rem_euclid(...)` o `%` (decisión).

**Tests**: ~4 unit.

### ~~R.1.3 — Asignación a índice~~ ✓ CERRADO 2026-05-17 (caps 9, 13)

**Hoy**: `xs[0] = nuevo` y `m["k"] = v` no parsean. Workaround
con `push`/`pop` o reconstrucción.

**Esperado**:
```fitz
let xs = [1, 2, 3]
xs[0] = 99
print(xs)  // [99, 2, 3]

let m = {"a": 1}
m["b"] = 2
m["a"] = 10
print(m)  // {"a": 10, "b": 2}
```

**Implementación**:
- AST: agregar `AssignTarget::Index { object: Box<Expr>,
  index: Box<Expr> }` paralelo a `AssignTarget::Field`.
- Parser: detectar el patrón `<expr>[<idx>] =` (lookahead post-`]`
  por `=` que NO sea `==`). Igual que para `obj.field =`.
- Checker: receiver debe ser `List<T>` o `Map<K,V>`. RHS compatible
  con `T` o `V`. Out-of-bounds en List es error runtime, no de tipo.
- Evaluator:
  - `List`: bounds check, asigna. Out-of-bounds → `FitzError` claro.
  - `Map`: inserta o sobreescribe.
- Codegen: `xs[0] = v` → `xs.lock().unwrap()[0_usize] = v;` con
  bounds check antes (panic-free). `m["k"] = v` → linear search +
  push si no existe, replace si existe (para mantener insertion
  order).

**Tests**: ~8 unit + ~3 cli_e2e + 1 compile_e2e.

### ~~R.1.4 — Rangos inclusivos `0..=10`~~ ✓ CERRADO 2026-05-17 (cap 9)

**Hoy**: solo `0..10` (exclusivo). Cargo style espera `..=` también.

**Esperado**:
```fitz
for i in 0..=10 { print(i) }  // imprime 0 a 10 inclusive
match score {
    0..=59 => "fail",
    60..=100 => "pass",
    _ => "invalid",
}
```

**Implementación**:
- Lexer: `Token::DotDotEq` (tokenizar `..=`).
- Parser: produce `Expr::Range { start, end, inclusive: true }`.
- AST: agregar `inclusive: bool` a `Expr::Range` y `Pattern::Range`.
- Evaluator: en `for` itera incluyendo `end`. En `match`, el guard
  `(start..=end).contains(&n)` se emite igual.
- Codegen: emite `..=` Rust nativo.

**Decisión**: tomar opción menos invasiva — `Expr::Range` gana un
field `inclusive`. Tests existentes siguen funcionando porque
default `false`.

**Tests**: ~5 unit.

### ~~R.1.5 — Strings multilínea `"""..."""`~~ ✓ CERRADO 2026-05-17 (cap 5)

**Hoy**: strings son single-line. `\n` literal funciona pero es feo
para SQL/HTML/mensajes largos.

**Esperado**:
```fitz
let sql = """
    SELECT *
    FROM users
    WHERE active = true
"""

let html = """
    <h1>Hola, {name}!</h1>
    <p>Bienvenido.</p>
"""
```

**Implementación**:
- Lexer: detectar triple-quote `"""`. Captura todo hasta `"""`
  siguiente. Interpolación `{expr}` sigue funcionando.
- Indentación: por simplicidad MVP, el string se preserva tal cual
  (sin auto-strip de leading whitespace común). El usuario decide
  si quiere indent o no.
- Escapes: `\\`, `\"`, `\n`, `\t` siguen funcionando.

**Tests**: ~4 unit.

### R.1 — Estado

- [x] **R.1.1 — `not`** ✓ (2026-05-17). Implementación en 7 capas
  (lexer, AST, parser, checker, evaluator, codegen, fmt) +
  **16 unit tests nuevos** (5 parser, 6 checker, 5 evaluator) +
  smoke E2E bit-a-bit `fitz run`/`fitz build`. Cap 6 de la guía
  + `examples/guide/06-logica.fitz` actualizados. Sin truthy/falsy
  — exige Bool estricto.
- [x] **R.1.2 — `%`** ✓ (2026-05-17). Implementación en 7 capas
  (lexer Token::Percent, BinOpKind::Mod, parser con precedencia
  de Mul/Div, checker Int-only, evaluator `i64::rem_euclid` +
  check de %0, codegen emite `rem_euclid` con check explícito,
  fmt). **13 unit tests nuevos** (3 parser, 5 checker, 5
  evaluator) + smoke E2E bit-a-bit. Cap 4 de la guía +
  `examples/guide/04-operadores.fitz` actualizados. Semántica
  euclidean (mismo signo del divisor, como Python). `Float % T`
  rechazado en MVP (decisión de scope).
- [x] **R.1.3 — Asignación a índice** ✓ (2026-05-17). `xs[i] = v`
  y `m[k] = v` end-to-end. AST suma `AssignTarget::Index`; parser
  destructura `Expr::Index` cuando viene seguido de `=`; checker
  valida `List<T>` con idx Int + RHS=T, `Map<K,V>` con idx=K +
  RHS=V; evaluator dispatch sobre List (bounds check con error
  claro "fuera de rango") o Map (linear search + insert preservando
  insertion order); codegen emite bloque acotado con patrón
  **"compute first, lock last"** para evitar deadlock cuando el
  RHS o el index acceden al mismo Mutex (descubierto y arreglado
  durante el smoke). **15 unit tests nuevos** (3 parser, 6 checker,
  6 evaluator) + smoke E2E bit-a-bit `fitz run`/`fitz build`. Caps
  1, 9, 13 + `examples/guide/09-listas-mapas.fitz` actualizados.
- [x] **R.1.4 — Rangos inclusivos** ✓ (2026-05-17). `0..=10` y
  `match { 0..=100 => ... }`. Lexer suma `Token::DotDotEq`; AST
  suma `inclusive: bool` a `Expr::Range` y `Pattern::Range`;
  parser detecta `..=` paralelo a `..` en `range_expr` y
  `try_int_or_range`; evaluator convierte inclusive→exclusive
  con `end + 1` (sin tocar `Value::Range`); codegen emite `..=`
  Rust nativo en for loops + pattern guards; fmt emite `..=`
  cuando inclusive. **11 unit tests nuevos** (3 parser, 4
  evaluator) + smoke E2E bit-a-bit. Cap 9 + ejemplo
  `examples/guide/09-listas-mapas.fitz` actualizados.
- [x] **R.1.5 — Strings multilínea `"""..."""`** ✓ (2026-05-17).
  Lexer detecta triple-quote (`"""`) y delega a
  `read_triple_string`: captura todo el contenido hasta el cierre
  `"""`, preserva newlines literales, soporta los mismos escapes
  que strings normales (`\n`, `\t`, `\\`, `\"`, `\{`, `\}`),
  comillas simples aisladas adentro se preservan (solo `"""`
  cierra). Interpolación `{expr}` sigue funcionando vía
  `build_string_expr` igual que strings de comilla simple (con
  la deuda residual ya documentada: el buscador ingenuo de `}`
  no entiende strings anidados — workaround: escapar las llaves
  externas con `\{ \}` cuando hay JSON-like adentro).
  **5 unit tests nuevos** en `src/lexer.rs::tests` (smoke
  multilínea sin escapes, comillas simples adentro, escapes
  estándar, sin cerrar → error, integración con tokenize) +
  smoke E2E `fitz run examples/guide/05-strings.fitz` (output
  esperado bit-a-bit). Cap 5 + ejemplo actualizados con SQL
  multilínea + JSON con interpolación y llaves escapadas.

> **R.1 CERRADA ENTERA (2026-05-17)** — los 5 quick wins de
> sintaxis están implementados, testeados (60 unit tests
> nuevos + 4 smokes E2E manuales), documentados (5 caps de la
> guía + 4 ejemplos actualizados) y validados bit-a-bit
> `fitz run` vs `fitz build` cuando aplica. Próximo: R.2.

---

## R.2 — Match más expresivo + operadores compuestos (~1 día)

### ~~R.2.1 — Or-patterns `1 | 2 | 3 =>`~~ ✓ CERRADO 2026-05-17 (cap 10)

**Hoy**: hay que repetir el body para cada caso o usar guard manual.

**Esperado**:
```fitz
match dia {
    "lun" | "mar" | "mie" | "jue" | "vie" => "laboral",
    "sab" | "dom" => "fin de semana",
    _ => "?",
}
```

**Implementación**:
- AST: `Pattern::Or(Vec<Pattern>)`.
- Parser: detectar `|` entre patterns en match arm.
- Checker: cada sub-pattern debe ser compatible con el scrutinee.
  Cuidado: si los sub-patterns bindean nombres, todos deben bindear
  los mismos (mismo tipo) — MVP rechazar bindings en or-patterns
  (Rust hace esto con `|` también).
- Evaluator: probar cada sub-pattern hasta matcheo.
- Codegen: emite `pat1 | pat2 | pat3 => ...` Rust nativo.

**Tests**: ~5 unit.

### ~~R.2.2 — Guards en match `pat if cond =>`~~ ✓ CERRADO 2026-05-17 (cap 10)

**Hoy**: condiciones extra se manejan con `if` adentro del body o
descomponiendo el match. Menos expresivo.

**Esperado**:
```fitz
match user {
    User { age, name } if age >= 18 => "adulto: {name}",
    User { age, name } if age >= 13 => "adolescente: {name}",
    _ => "niño",
}
```

**Implementación**:
- AST: `MatchArm` gana `guard: Option<Expr>`.
- Parser: tras el pattern, si viene `if`, parsea expresión hasta
  `=>` como guard.
- Checker: el guard debe tipar `Bool`. El binding del pattern está
  visible en el guard.
- Evaluator: match si pattern matchea Y guard evalúa a `true`.
- Codegen: emite `pat if cond => ...` Rust nativo.
- Exhaustividad: arms con guard NO cuentan como exhaustivos (Rust
  hace lo mismo) — el checker exige `_` o equivalente al final.

**Tests**: ~5 unit.

### ~~R.2.3 — Operadores compuestos `+=`/`-=`/`*=`/`/=`~~ ✓ CERRADO 2026-05-17 (cap 4)

**Hoy**: `x = x + 1`, `total = total + amount`.

**Esperado**:
```fitz
let total = 0
for item in items {
    total += item.price
}
```

**Implementación**:
- Lexer: 4 tokens nuevos (`Token::PlusEq`, `MinusEq`, `StarEq`,
  `SlashEq`).
- Parser: detectar tras un `AssignTarget`. Desugar a `target = target
  <op> value` durante el parsing — más simple que sumar variante
  AST nueva, y el checker/eval/codegen trabajan con el AST normal.
- Validación: solo válidos con `AssignTarget::Ident` o `Field` o
  `Index` (todas las formas de asignación que ya soportamos).

**Tests**: ~4 unit.

### ~~R.2.4 — F3: checker rechaza `return`/`break`/`continue` huérfanos~~ ✓ CERRADO 2026-05-17

**Hoy**: el checker permite `return` en top-level; el evaluator
emite error en runtime. Mejor cazarlo estáticamente.

**Esperado**:
```fitz
return 42   // ✗ error de check: "return solo dentro de fn"
break       // ✗ error de check: "break solo dentro de loop/while/for"
```

**Implementación**:
- Checker: ya tiene `return_stack` (Fase 5.3.2). Sumar `loop_depth`
  paralelo para break/continue.
- En `check_stmt`, al ver `Stmt::Return`/`Break`/`Continue`, si el
  stack está vacío → error específico.

**Tests**: ~6 unit.

### R.2 — Estado

- [x] **R.2.1 — Or-patterns** ✓ (2026-05-17). `pat1 | pat2 | pat3
  =>` con sub-patterns sin bindings (vetados por el parser, igual
  que Rust). Implementación en 6 capas (lexer Token::Pipe, AST
  `Pattern::Or(Vec<Pattern>)`, parser `parse_or_pattern` con
  rechazo claro de Ident/Ok/Err bindings, checker
  `update_result_coverage` recursivo, evaluator `match_pattern`
  helper extraído + caso Or, codegen estrategia uniforme
  `ref __or_v if cond1 || cond2 || ...` con catch-all artificial
  forzado porque Rust no infiere exhaustividad de guards, fmt
  con separador `|`). **19 unit tests nuevos** (7 parser, 7
  evaluator, 5 checker) + smoke E2E bit-a-bit `fitz run`/`fitz
  build`. Cap 10 + ejemplo `examples/guide/10-match.fitz`
  actualizados.
- [x] **R.2.2 — Guards en match** ✓ (2026-05-17). `pat if cond =>`
  con cond visible para el binding del pattern. Arms con guard
  NO cuentan para exhaustividad de Result (paralelo a Rust) — el
  checker exige catch-all explícito. AST suma `MatchArm.guard:
  Option<Expr>`; parser parsea `if <expr>` entre pattern y `=>`;
  checker valida `Type::Bool`; evaluator chequea cond después
  del pattern (scope con binding) y avanza al siguiente arm si
  false; codegen refactoreado `gen_pattern` devuelve
  `(pattern_code, Option<inner_guard>)` y combina con
  outer_guard usando `&&`; fmt emite ` if cond` entre pattern y
  `=>`. **14 unit tests nuevos** (5 parser, 6 evaluator, 5
  checker) + smoke E2E bit-a-bit. Cap 10 + ejemplo
  `examples/guide/10-match.fitz` actualizados.
- [x] **R.2.3 — Operadores compuestos `+=`/`-=`/`*=`/`/=`** ✓
  (2026-05-17). Desugar en el parser: `x += rhs` → `x = x + rhs`.
  Lexer suma 4 tokens (`PlusEq`/`MinusEq`/`StarEq`/`SlashEq`)
  con manejo del overlap `->` (Arrow vs MinusEq). Parser
  detecta el compound op después de `parse_expr_or_assign_stmt`,
  arma `AssignTarget` apropiado (Ident/Field/Index) y sintetiza
  `BinOp(target, op, rhs)` como value. Como es desugar en el
  parser, el resto del pipeline (checker, evaluator, codegen)
  trabaja sin cambios. **13 unit tests nuevos** (7 parser, 6
  evaluator) + smoke E2E bit-a-bit. Cap 4 + ejemplo
  `examples/guide/04-operadores.fitz` actualizados.
- [x] **R.2.4 — F3: checker rechaza `return`/`break`/`continue`
  huérfanos** ✓ (2026-05-17). `CheckCtx` gana `loop_depth:
  usize` (incrementa en While/Loop/For body, decrementa al
  salir, resetea a 0 al entrar a FnDef/FnExpr y restaura al
  salir — break/continue NO escapan funciones). `Stmt::Return`
  emite error si `return_stack` vacío. `Stmt::Break`/`Continue`
  emite error si `loop_depth == 0`. **10 unit tests nuevos**
  del checker. Test viejo `return_huerfano_no_chequea`
  reapuntado a `return_huerfano_chequea` (contrato cambió).

> **R.2 CERRADA ENTERA (2026-05-17)** — los 4 ítems
> implementados, testeados (56 unit tests nuevos + 4 smokes E2E
> manuales bit-a-bit `fitz run`/`fitz build`), documentados
> (caps 4 y 10 de la guía + 2 ejemplos actualizados). Próximo:
> R.3 (métodos custom sobre `type`).

---

## R.3 — Métodos custom sobre `type` (~1-2 días)

**Mencionado como deuda en 3 caps (11, 12, 13)**. El "polimorfismo
natural" sin meterse en traits.

### Sintaxis (decisión tomada: opción A — fields como locales)

```fitz
type User {
    id: Int
    name: Str
    email: Str?
}

type User {
    // Método sin args: fields del type son locales del body.
    fn greet() -> Str {
        return "Hola, {name}!"   // ← `name` es el field del User
    }

    // Método con args: combinan con fields.
    fn match_domain(domain: Str) -> Bool {
        // `email` es el field, `domain` es el arg.
        if email == null {
            return false
        }
        return email.contains(domain)
    }

    // async fn también funciona.
    async fn fetch_profile() -> Result<Str> {
        let url = "https://api.test/users/{id}"   // ← `id` field
        // ... uso de sleep().await, http.get().await, etc.
    }
}

// Uso:
let u = User { id: 1, name: "Ada", email: "ada@test.com" }
print(u.greet())                         // "Hola, Ada!"
print(u.match_domain("test.com"))        // true
let profile = u.fetch_profile().await
```

### Razonamiento de opción A

- **Consistencia con cómo el lenguaje hoy expone fields**: dentro
  del body de un struct lit (defaults), los fields previos son
  visibles sin prefijo. Métodos custom siguen esa convención.
- **Menos boilerplate** que `self.name` o `this.name`. Python /
  Ruby / Crystal lo hacen así.
- **Trade-off conocido**: si el método declara un local con el
  mismo nombre que un field, shadow del local gana. Documentamos
  como caveat. Workaround: nombrar distinto el local.

### Implementación

- **AST**:
  - `Stmt::TypeDef` gana `methods: Vec<MethodDef>`.
  - `MethodDef` paralelo a `FnDef` pero sin `decorators` (los
    métodos no llevan decorators en MVP).
- **Parser**:
  - Dentro del `{` del `type`, distinguir `name: Type` (field) de
    `fn nombre(...) ...` (método). Lookahead trivial.
  - Métodos respetan la sintaxis de `fn` (con o sin flecha).
- **Checker**:
  - Resuelve el tipo en dos pasadas:
    1. Primera: registrar campos.
    2. Segunda: registrar firmas de métodos (resolver param/return
       types con todos los nominales ya conocidos).
    3. Tercera: chequear cada body de método con un scope que
       pre-declara los fields como locales (`name: Str`, `email:
       Str?`, etc.) además de los params.
  - Type-method dispatch: cuando ve `Expr::Call { callee:
    Expr::Field { object: <Instance>, field: "method_name" } }`,
    busca método en el `Type::Nominal` correspondiente.
- **Evaluator**:
  - `dispatch_method` extiende el branch existente: receiver
    `Value::Instance` busca primero en `methods` del tipo
    declarado; si no existe, fallback a "no method".
  - Body se evalúa con un env hijo que tiene cada field como var
    local + params + closure (env del tipo).
- **Codegen**:
  - Emite `impl FooData { pub fn greet(&self) -> String { ... } }`.
  - Adentro del body, las referencias a fields se traducen a
    `self.<field>.clone()` (o lo que ya hace para field access).
  - El call `u.greet()` se traduce a `u.lock().unwrap().greet()`.
  - Async methods → `pub async fn ...`.

### Tests

- ~10 unit (parser + checker + evaluator).
- ~3 compile_e2e (binario con métodos custom).
- Ejemplo runnable: actualizar `examples/guide/13-metodos.fitz` con
  una sección "Métodos custom" o crear cap 13b.

### Deuda derivada (NO blocker de R.3)

- Métodos con visibilidad (`pub fn`/`fn` privado) — todos public
  en MVP.
- Static methods (`type::method`) — no implementado.
- Operator overloading (`fn +(self, other)`) — no implementado.
- Métodos sobre tipos importados desde otro módulo — verificar
  que el dispatch siga al tipo cross-module.

### R.3 — Estado

- [x] **R.3 — Métodos custom sobre `type`** ✓ (2026-05-17, "opción
  A": fields como locales). Sintaxis: `type Foo { field: T,
  fn metodo(params) -> R { body } }` con fields y métodos
  mezclados libremente. Adentro del body, los fields del type son
  variables locales (sin prefijo `self.`). Si un param tiene el
  mismo nombre que un field, el param gana (shadowing
  documentado). Implementación en 7 capas:
  - **AST**: `Stmt::TypeDef` suma `methods: Vec<MethodDef>`;
    `MethodDef` paralelo a `FnDef` sin decorators (los métodos no
    aceptan `@get`/etc.).
  - **Parser**: `parse_typedef` distingue field (`name:`) de
    método (`[async] fn`) por lookahead trivial; `parse_method_def`
    reusa `parse_params` + `parse_optional_return_type` +
    sintaxis `=> expr` o bloque.
  - **Evaluator**: `dispatch_method` extendido para receiver
    `Value::Instance` — busca el tipo en el env por `type_name`,
    matchea el método por nombre, delega a `invoke_custom_method`.
    Body se ejecuta en scope hijo del env con fields pre-declarados
    como locales + params (lookup en env hecho ANTES del `.await`
    para no holdear el lock vía suspensión).
  - **Codegen**: `gen_type_def` emite `impl FooData { pub fn
    metodo(&self, ...) -> T { let mut <field>: T = self.<field>
    .clone(); ... <body> } }`. Fields homónimos a params se
    skipean del pre-binding para preservar shadowing.
    `gen_method_call` para `Type::Nominal` busca en
    `type_methods` y emite `{ let __recv = obj.clone(); let __g =
    __recv.lock().unwrap(); __g.<m>(<args>) }`. Async methods en
    `fitz build` quedan como deuda menor (error explícito).
  - **Checker**: `check_custom_methods` walkea cada body de
    método con `push_scope` + fields pre-declarados como locales
    + params + return_stack + loop_depth reset (consistente con
    `Stmt::FnDef`). Cazó errores de tipo, idents desconocidos,
    return mismatch.
  - **Fmt**: `fmt_typedef` emite fields, blank line, métodos
    formateados con `fmt_method_def`.
  - **Value**: `Value::Type` suma `methods: Vec<MethodDef>`;
    `load_module` propaga los methods al rebuild del Type post-
    pre-evaluación de defaults; `Stmt::TypeDef` los pasa al
    construir el `Value::Type`.

  **20 unit tests nuevos** (7 parser, 7 evaluator, 6 checker) +
  smoke E2E bit-a-bit `fitz run`/`fitz build` sobre el ejemplo
  nuevo `examples/guide/13b-metodos-custom.fitz` (sumado al
  smoke `GUIDE_EXAMPLES_COMPILE`). Caps 13 actualizado con
  sub-sección "Métodos custom sobre `type`" (sale de "Lo que
  todavía no anda" y entra como feature implementada).

  **Deuda residual visible**:
  - ~~`async fn` adentro de `type` en `fitz build`~~ ✓
    CERRADO 2026-05-17 (post-R.3). Codegen emite `pub async fn
    name(self, ...)` con `self` por valor (clone) para no
    holdear el `MutexGuard` a través del `.await`. El call
    site usa patrón "clone-out": lock corto + clone del Data
    + invoke fuera del lock. NominalInfo del TypeEnv suma
    `methods: Vec<NominalMethod>` para que el checker tipe
    `instance.async_method().await` como `T` (vía `Future<T>`).
    Bit-a-bit `fitz run` ↔ `fitz build` validado con sleep +
    fields + múltiples calls sobre el mismo instance.
  - Visibilidad (`pub fn` / `fn` privado).
  - Static methods (`Counter::create(...)`).
  - Operator overloading.

> **R.3 CERRADA (2026-05-17)** — primer caso de polimorfismo
> "natural" (sin traits) en el lenguaje. Próximo: cierre formal
> de mini-fase R entera.

---

## Cierre de mini-fase R

> **MINI-FASE R CERRADA ENTERA (2026-05-17)** — los 10 ítems
> originales (5 de R.1 + 4 de R.2 + 1 de R.3) implementados,
> testeados (~135 unit tests nuevos + smokes E2E bit-a-bit
> `fitz run`/`fitz build` sobre cada ejemplo afectado),
> documentados (5 caps de la guía + 1 ejemplo nuevo +
> 4 ejemplos actualizados).
>
> **Total acumulado al cierre de R**:
>   - 1516 unit + 76 cli_e2e + 79 compile_e2e + 3 openapi.
>   - Clippy `-D warnings` limpio.
>
> Próximo norte técnico (post-R): retomar la planificación de
> Fase 9.w (Stack web first-class: `@authenticated`/`@admin`,
> `@ws("/chat")`, `@cron`/`@background`).

---

## Deudas DIFERIDAS (fuera de R)

Documentadas para sub-fases futuras. No bloquean Fase 9.w ni
posteriores.

### Sintaxis grande (sub-fase G dedicada)

- **Tuples** `(1, "a", true)` + `Pattern::Tuple` + iteración Maps
  con Pair. Toca AST + parser + checker (incluyendo
  exhaustividad) + codegen. ~6-8h.
- **Trait-like polymorphism** — interfaces o traits con métodos
  abstractos. Decisión grande de diseño (Rust traits? Go
  interfaces? duck typing?). ~10-15h cuando aparezca el caso de
  uso real.
- **Herencia / composición de types** — no urgente, `type Order
  { user: User }` cubre.

### Operadores menores

- **Operadores de bits** `&|^<<>>` — útil para protocolos
  binarios; sin presión.
- **`xor` lógico** — `a != b` sobre `Bool` cubre. Sumar si aparece
  presión.

### Lexer / tokenización

- **Separadores en números** `1_000_000` — F7. Quality of life.
- **Notación científica** `3.14e-2` — F7. Útil para float math.
- **Identificadores no-ASCII** (`π`, `función`) — F8. Bajo
  impacto.
- **Escapes extendidos** `\u{...}`, `\x..`, `\0`, `\b` — F9.
  ASCII tabla cubre lo común.

### Strings — métodos extras (cap 13)

- `Str.contains(s)` — útil. ~30 min.
- `Str.split(sep)` — necesita decidir tipo de retorno (`List<Str>`).
  ~1h.
- `Str.trim()` / `.trim_start()` / `.trim_end()` — ~30 min.
- `Str.starts_with(s)` / `.ends_with(s)` — ~30 min.
- `Str.replace(old, new)` — ~30 min.
- `Str.repeat(n)` — ~30 min.

Todos chicos, pueden ir en una mini-tanda dedicada.

### Listas — métodos extras

- `xs.sort()` / `.sort_by(fn)` — necesita comparación generic.
- `xs.reverse()` — trivial.
- `xs.contains(v)` — trivial.
- `xs.flatten()` (para `List<List<T>>`).
- `xs.zip(ys)` — necesita tuples.

### Loops

- **`loop` como expresión con valor** (`let x = loop { break v }`).
  ~2h.
- **Labels** en break/continue (`break 'outer`). ~3h, parser más
  complejo.

### Index / slicing

- **Índices negativos** `xs[-1]` — ~1h, lookup tiene que distinguir
  signo.
- **Slicing** `xs[1..5]`, `xs[..3]`, `xs[2..]` — ~3h, sintaxis ya
  parsea como `Range`.

### Comprehensions

- `[x * 2 for x in xs]` — azúcar sobre `.map()`. ~3-4h. Decisión:
  ¿filter inline también? `[x for x in xs if x > 0]`?

### Format specifiers

- `{ratio:.2f}` en interpolación — ~2-3h. Cambio menor al parser
  de StrInterp.

### Result avanzado

- **`?` fuera de fn con mensaje propio** (cap 14) — ~30 min.
  Cambio cosmético del mensaje de error.
- **`Err` con valores no-Str y bindings tipados en codegen** (cap
  14) — refactor del `Result<T, String>` pinned. ~4h. Sub-paso si
  aparece presión.

### Bridge async Fitz ↔ Python asyncio (Fase 8.6)

- **~~Reescribir bridge con event loop persistente~~** ✓ CERRADO
  2026-05-17 (Fase 8.6-bis). Ver entrada al final de esta
  sección con la implementación y benchmarks.

- **Original (8.6.1)**: `py_coro_to_fitz_future` (en
  `src/py_interop.rs`) usaba `tokio::task::spawn_blocking` +
  `asyncio.new_event_loop()` + `run_until_complete(coro)` para
  cada call. Era funcional pero iba a doler bajo carga real:
  - Cada `<py_call>?.await` paga el costo de **crear y cerrar
    un event loop nuevo** (cientos de microsegundos, plus el
    GIL acquire/release).
  - El `spawn_blocking` consume un thread del blocking pool
    de tokio (default 512) — cargas con cientos de awaits
    concurrentes pueden saturarlo.
  - No hay reuso de conexiones de runtime asyncio (DB pools,
    HTTP clients) entre calls — cada call ve un loop fresco.

  **Alternativa**: un event loop asyncio dedicado (singleton
  estático) corriendo en un thread Python persistente, con
  `asyncio.run_coroutine_threadsafe(coro, loop)` para encolar
  la corutina desde Rust. El Future Fitz hace `.await` sobre
  un `tokio::sync::oneshot::Receiver` que el callback del
  asyncio future completa.

  **Complejidad**: ~6-8h. Requiere:
  - Inicializar el loop en lazy static (`OnceCell<Loop>` con
    `Python::attach` para crearlo).
  - Bootstrap thread Python que corre `loop.run_forever()`.
  - `run_coroutine_threadsafe` + bridge oneshot.
  - Cleanup graceful en shutdown del runtime Fitz.

  **Por qué no se cerró en 8.6**: la crate `pyo3-async-runtimes`
  ofrece `into_future` que hace esto, pero requiere control
  del tokio runtime (`init_with_runtime` o `#[tokio::main]`).
  Choca con el tokio que Fitz ya tiene corriendo (current_thread
  CLI / rt-multi-thread HTTP). Para 8.6 elegimos el "baseline
  blocking" como trade-off explícito; la versión persistente
  queda comprometida acá.

  **Tests de validación**: micro-benchmark de 100 awaits
  secuenciales (mide overhead per-call) + 100 awaits concurrentes
  (verifica que no satura blocking pool). Debería bajar de
  ~5ms/call a <0.5ms/call.

### Fase 8.6-bis — Bridge asyncio persistente CERRADO (2026-05-17)

**Implementación**: thread Python dedicado (`fitz-asyncio`) que
mantiene un único event loop `asyncio` vivo entre calls. Cada
`.await` desde Fitz construye una `AsyncioRequest { coro,
response }` (con `response: tokio::sync::oneshot::Sender`), la
envía por un `std::sync::mpsc::Sender` al thread del loop, y
hace `.await` sobre el `Receiver`. El thread del loop bucla:
`rx.recv()` afuera de `Python::attach` (NO holdea GIL durante
la espera — clave para no bloquear marshaling concurrente),
seguido de `Python::attach { loop.run_until_complete(coro) }`
por iteration.

**Por qué NO `run_coroutine_threadsafe`**: el approach
"`loop.run_forever()` en un thread + threadsafe schedule desde
otros threads" choca con la coordinación GIL en PyO3 0.28 sin
`pyo3-asyncio` (que requiere control del runtime tokio,
incompatible con `current_thread`/`rt-multi-thread` ya
establecidos en Fitz). Intentado y descartado en la primera
versión de 8.6-bis: el thread del loop necesita el GIL para
reaccionar a la tarea agendada, pero el thread que la programa
lo tiene durante el call mismo. Diseño documentado en
`src/py_interop.rs`.

**Mejoras conseguidas**:
- **Cero overhead por call de event loop**: el loop se crea
  UNA vez. Antes: `new_event_loop()` + `close()` por cada `.await`.
- **No consume blocking pool de tokio**: solo un thread Python
  dedicado. Cientos de awaits Fitz pendientes encolan en el
  mpsc, no saturan threads.
- **Reuso de estado asyncio**: DB pools, HTTP clients y otros
  primitives que cachean por loop sobreviven entre calls.

**Benchmarks (2026-05-17, máquina del autor)**:
- 100 awaits secuenciales con `asyncio.sleep(0)`: **~160ms
  total ⇒ ~1.6ms/call** (release build, debug ~157ms).
  Incluye el cost del marshaling + call Python + return.
- 50 awaits con `asyncio.sleep(0.01)`: ~1.0s total (500ms de
  sleep efectivo + 500ms de overhead distribuido).
- Antes (8.6.1): roadmap estimaba ~5ms/call. **Reducción ~3x**.

**Limitación del MVP**: los requests se serializan en el thread
del loop (uno por vez con `run_until_complete`). El GIL lo
imponía igual, así que no perdimos paralelismo real, pero la
verdadera concurrencia llegará si entra demanda (sub-loops via
`asyncio.gather`, multi-process, etc.).

**Diferido para tanda futura — más benchmarks**:
- Awaits **concurrentes** (`asyncio.gather` desde Python con
  varias corutinas en paralelo): medir si el throughput escala.
- Awaits **paralelos desde Fitz** (`tokio::join!` de dos
  `.await` Fitz): hoy se serializan en el thread del loop —
  documentar qué se gana en throughput vs latencia.
- **Bench de marshaling** (List<User> grande Fitz→Python y
  vuelta): identificar bottleneck (es el GIL? los `.clone`?).
- **Bench DB-bound real**: asyncpg + SELECT 1k rows con
  SQLAlchemy async. Caso típico que justifica este sub-paso.

Convención: cada vez que sumemos un benchmark nuevo, lo
acumulamos acá con fecha + hardware + comando exacto.

### Robustez interna del compilador (matriz F en `deudas-post-5b.md`)

- **F1**: documentar matriz cobertura de `Type::Any`. Solo docs.
- **F5**: `is_async` en `FnDef`. Casi cerrado por Fase 6 + F17.
  Verificar y marcar cerrado en deudas si aplica.
- **F13**: heterogéneos `[1, "dos", true]` en `fitz build` — `FitzValue`
  tagged runtime. Decisión grande.
- **F14**: `let X = <expr>` no-literal a nivel top-level del módulo.
  Pre-eval eager al cargar el módulo.
- **F15**: imports transitivos en codegen — un módulo cargado puede
  tener su propio `import`. Refactor del module loader del codegen.

---

## Cómo se actualiza este doc

- Cada vez que cerramos un item de R, **marcamos con ~~strikethrough~~
  + fecha + sub-paso**.
- Cuando R.1 entera cierra, sumamos blockquote `> **R.1 CERRADA
  (fecha)**`.
- Si descubrimos una deuda nueva del lenguaje base durante R,
  la sumamos a la lista de DIFERIDOS (o a R si encaja con el
  scope actual).
- Cuando R.1+R.2+R.3 estén entera cerradas, **archivamos el doc**:
  sumamos un blockquote final "MINI-FASE R CERRADA ENTERA" y
  movemos los DIFERIDOS a una sub-fase nueva (sub-fase G, sub-fase
  de strings, etc.) o los dejamos sin compromiso si no hay
  presión.
