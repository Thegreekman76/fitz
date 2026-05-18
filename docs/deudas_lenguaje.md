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

- ~~Métodos con visibilidad (`pub fn`/`fn` privado) — todos public
  en MVP~~ ✓ CERRADO 2026-05-18 (mini-tanda Vm). La misma convención
  de Vp aplicada a métodos: `_method` es privado, accesible solo
  desde adentro de métodos del propio `type` (instance + static).
  Implementación: checker reusa `is_private_field` + `current_type`
  ya introducidos en Vp; agrega validación en `infer_method_call`
  para `Type::Nominal(id)` antes de la aridad y el chequeo de
  tipos de args. LSP autocomplete filtra `_method` en `instance.`
  paralelo al filter de fields. Caveat documentado: los métodos
  de instancia no pueden llamar otros métodos del mismo type sin
  un receiver explícito (R.3 opción A — no hay `self`). El patrón
  canónico es `static fn` que recibe la instancia como param.
  4 unit tests (afuera = error, adentro = ok via static, otro
  tipo = error, método público no afectado) + 1 LSP unit + 1
  compile_e2e (método público sigue compilando).
- ~~Visibility en campos (`_field` privado)~~ ✓ CERRADO 2026-05-18
  (mini-tanda Vp). Convención estilo Python pero validada por el
  checker estático: los campos cuyo nombre arranca con `_` son
  privados — solo accesibles desde adentro de los métodos del
  propio `type`. Implementación en 3 capas:
  - **Checker**: `CheckCtx.current_type: Option<TypeId>` se
    setea/limpia en `check_custom_methods` alrededor de cada
    method body. Helper `is_private_field(name)` = nombre
    arranca con `_`. Tres call sites validan: `Expr::Field`
    (acceso desde fuera), `Expr::StructLit` (setear `_field`),
    `AssignTarget::Field` (asignar via `obj._field = v`). Todos
    chequean `current_type == Some(receiver_type)` y emiten
    error claro citando que es privado + sugerencia (usar
    constructor estático).
  - **LSP**: `after_dot_completions` para `Type::Nominal`
    filtra fields que arrancan con `_` — no aparecen en
    autocomplete sobre `instance.`. Adentro de un método del
    propio type siguen apareciendo (como locales del scope).
  - **Sin cambios al codegen**: Rust acepta cualquier
    identifier, incluido `_field`. El checker se encarga del
    enforcement; el codegen es transparente.
  - **Drive-by fix de St**: alineé el checker con la semántica
    de St — los métodos estáticos NO reciben fields como
    locales (paralelo al evaluator y codegen). Antes de Vp el
    checker pre-declaraba fields para todos los métodos
    incluido `static`, dejando un agujero entre check y
    runtime.
  Tests: 7 unit nuevos en types (acceso desde afuera = error,
  acceso desde adentro = ok, acceso desde otro tipo = error,
  struct lit afuera = error, struct lit adentro = ok via
  constructor estático, asignar afuera = error, campos públicos
  no afectados) + 1 LSP unit (filter en autocomplete).
  Ejemplo `examples/guide/13i-campos-privados.fitz` con `type
  Account { name, _balance }` + `static fn new`/`fn deposit`/
  `fn balance` + caveats comentados. Cap 13 sub-sección nueva
  "Campos privados (mini-tanda Vp)" con tabla de reglas +
  combinación natural con St (constructor estático).
  **Decisión de diseño**: encapsulamiento opt-in, sin keyword
  nueva — solo convención de nombres (`_`) validada
  estáticamente. Más liviano que añadir `pub`/`private` y
  consistente con la estética Python.
- ~~Static methods (`type::method`) — no implementado~~ ✓ CERRADO
  2026-05-18 (mini-tanda St). `static fn` adentro del `type` body
  declara un método sin receiver, invocado como `Type.method(args)`.
  Útil para constructores y factories (paralelo a Rust `User::new`
  y Python `@classmethod`). Implementación en 7 capas:
  - **Lexer**: `Token::Static` + keyword `"static"`.
  - **AST**: `MethodDef.is_static: bool`.
  - **Parser**: `parse_method_def` detecta `static` ANTES de
    `async`/`fn` y setea el flag. `parse_typedef` reconoce `Static`
    como otro inicio de método válido junto a `Async`/`Fn`.
  - **Checker**: `NominalMethod` suma `is_static: bool`; el
    resolver lo propaga desde el AST.
  - **Evaluator**: `dispatch_method` agrega rama para
    `Value::Type` que busca un método estático y lo invoca via
    `invoke_static_method` (paralelo a `invoke_custom_method` pero
    SIN pre-declarar fields del tipo como locales — no hay
    receiver). Errores claros si se invoca `instance.static_fn()`
    o `Type.instance_fn()` con sugerencia de la forma correcta.
  - **Codegen**: `emit_custom_method` emite static como
    `pub fn <name>(params)` (associated fn Rust, sin `&self` ni
    pre-bindings de fields). Nuevo helper `gen_static_method_call`
    para el call site: `Counter.of(5)` → `CounterData::of(5i64)`.
    `gen_method_call` intercepta `Expr::Ident(TypeName).method()`
    al inicio para detectar el patrón antes que `gen_expr(object)`
    falle ("variable desconocida").
  - **LSP**: `after_dot_completions` para `Type::Nominal` filtra
    static methods (no aparecen en `instance.`).
  - **Grammar**: `static` sumado al pattern de declaration keywords.
  4 unit tests evaluator (constructor + factory, sin acceso a
  fields como locales, instance-call de static = error,
  static-call de instance = error) + 2 compile_e2e bit-a-bit
  (constructores, coexistencia static + instance). Ejemplo
  `examples/guide/13g-static-methods.fitz` sumado al smoke
  `GUIDE_EXAMPLES_COMPILE`. Cap 13 sub-sección nueva "Métodos
  estáticos (mini-tanda St)" con explicación + caveats.
- Operator overloading (`fn +(self, other)`) — no implementado.
- ~~Métodos sobre tipos importados desde otro módulo~~ ✓ CERRADO
  2026-05-18 (mini-tanda CM). `fitz run` ya andaba (el evaluator
  busca por type_name canónico via env del módulo). `fitz build`
  fallaba con "el tipo `X` no tiene un método llamado `foo`"
  porque `type_methods` solo se poblaba con types definidos en el
  main. Fix: `LoadedModule` + `LoadedModuleSigs` suman
  `type_methods: HashMap<String, Vec<MethodDef>>`; el
  `load_module_inner` los recolecta del AST del módulo (`for stmt
  in &module_program { if let Stmt::TypeDef { name, methods, ..
  } = stmt { ... } }`); `install_loader_bindings` los copia a la
  `CodegenCtx`; la enrichment loop de imports
  (`from foo import User`) los reasocia al nombre LOCAL del
  importer (permite alias via `as`). 1 compile_e2e bit-a-bit
  (`cm_metodos_custom_sobre_tipos_importados_compilan`) con
  método de instancia + método estático cross-module.

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

- ~~**Tuples** `(1, "a", true)` + `Pattern::Tuple`~~ ✓ CERRADO
  2026-05-17 (mini-tanda T post-I). Incluye `Type::Tuple`,
  acceso por índice `.0`/`.1` (lexer maneja `t.0.0` chaining
  via flag `prev_was_dot`), destructuring `let (a, b) = expr`,
  tuple patterns en match (con nesting). Limitaciones residuales
  del MVP (originalmente):
  - ~~en `fitz build` los tuple patterns no admiten literales
    Str/Range/Or como sub-pattern~~ ✓ CERRADO 2026-05-18
    (mini-tanda Rt). Counter `pattern_slot_counter` en
    `CodegenCtx` sintetiza nombres únicos `__s_<n>`/`__n_<n>`/
    `__or_v_<n>` por slot. Pattern::Tuple en `gen_pattern`
    ahora combina los inner_guards de todos los sub-patterns
    con `&&`. `pattern_to_or_cond` toma el `bind_name` como
    parámetro (antes era `__or_v` hardcoded) para que coincida
    con el counter. 3 unit tests + 3 compile_e2e nuevos. Ejemplo
    `examples/guide/10b-match-tuple-subpatterns.fitz` sumado al
    smoke `GUIDE_EXAMPLES_COMPILE`. Cap 10 de la guía suma
    sub-sección "Tuple patterns con sub-patterns ricos
    (mini-tanda Rt)".
  - ~~`let (...)` solo admite Ident/Wildcard/Tuple (no literales
    ni Ok/Err)~~ ✓ CERRADO 2026-05-18 (mini-tanda Lt). El parser y
    evaluator ya soportaban sub-patterns ricos pre-Lt (heredados de
    match). Solo faltaba el codegen. Implementación acotada al
    **codegen**: nuevo predicado `pattern_is_pure_irrefutable` +
    helper `collect_pattern_bindings`. `gen_destructure` ahora
    bifurca: pure path emite `let pat = value` directo (sin
    cambios pre-Lt); rich path envuelve en `match` con catch-all
    `_ => panic!("destructuring no matcheó el valor")`. La
    estrategia reusa `gen_pattern` (que ya tiene el counter
    `pattern_slot_counter` de Rt para nombres únicos
    `__s_<N>`/`__n_<N>`/`__or_v_<N>` por slot). El scrutinee se
    bindea a `__destr_scrut` con anotación de tipo explícita
    (`let __destr_scrut: <rust_ty> = ...`) para resolver
    ambigüedades de inferencia tipo `Ok(99)` sin contexto del E.
    Casos cubiertos: literales (Int/Float/Str/Bool/Null), rangos,
    `Or`-patterns, `Ok(name)`/`Err(name)` bindings, `Ok(_)`/`Err(_)`
    wildcards, mezcla y anidamiento. **6 unit tests nuevos** del
    codegen + **5 compile_e2e nuevos** bit-a-bit (literal Int,
    Str, Range, Ok-binding, panic-si-no-matchea). Ejemplo
    `examples/guide/09f-let-destructure-rico.fitz` sumado al
    smoke `GUIDE_EXAMPLES_COMPILE`. Cap 9 de la guía sumó
    sub-sección "`let` con sub-patterns ricos (mini-tanda Lt)"
    + bullet stale "MVP solo Ident/Wildcard/Tuple" removido.
    Decisión de diseño: panic en runtime cuando no matchea
    (paralelo a Rust `let pat = val else { panic!() }`). Si el
    shape es incierto, preferí `match`.
- ~~**For sobre Map con destructuring** `for (k, v) in m`~~ ✓
  CERRADO 2026-05-18 (mini-tanda Md). `Stmt::For.var` cambió de
  `String` a `Pattern`. El parser usa `parse_pattern` general
  (reusa el del match) y el checker valida que sea Ident,
  Wildcard o Tuple — otros patterns rechazados con error claro.
  Evaluator: `Value::Map` se materializa como `Vec<Value::Tuple([k,
  v])>` (snapshot para evitar re-entrancia) y el helper
  `bind_for_pattern` descompone recursivamente. Checker: el
  elem_ty para Map es `Tuple(K, V)`, y `bind_for_pattern_in_checker`
  bindea k:K y v:V en el scope cuando el pattern es
  Pattern::Tuple. Codegen: emite `for (mut k, mut v) in m.lock()
  .unwrap().clone().into_iter() { ... }` nativo Rust con
  destructuring; `_` se emite sin `mut`. Wildcard `for _ in 0..N`
  también soportado (Rust nativo). 9 unit tests nuevos (3 parser
  + 4 checker + 2 evaluator de regresión) + 2 E2E bit-a-bit.
  Ejemplo `examples/guide/09e-for-map.fitz` sumado al smoke
  `GUIDE_EXAMPLES_COMPILE`. Cap 9 de la guía suma sub-sección
  "Iterar Maps con destructuring (mini-tanda Md)".

  **Deuda residual menor**: `for kv in m` (Pattern::Ident sobre
  Map, sin destructuring) funciona en `fitz run` (bindea como
  `Value::Tuple([k, v])` accesible con `kv.0`/`kv.1`), pero `fitz
  build` lo rechaza con error claro porque emitir un binding
  Rust tipo `(K, V)` que se use como Tuple Fitz requiere helpers
  que no tenemos. Workaround: usar tuple destructuring `for (k,
  v) in m`.
- **Trait-like polymorphism** — interfaces o traits con métodos
  abstractos. Decisión grande de diseño (Rust traits? Go
  interfaces? duck typing?). ~10-15h cuando aparezca el caso de
  uso real.
- **Herencia / composición de types** — no urgente, `type Order
  { user: User }` cubre.

### Operadores menores

- ~~**Operadores de bits** `& | ^ << >> ~`~~ ✓ CERRADO 2026-05-18
  (mini-tanda Bits). 5 binarios + 1 unario sobre `Int`.

  **AST**: `BinOpKind::BitAnd`/`BitOr`/`BitXor`/`Shl`/`Shr` + 
  `UnaryOpKind::BitNot`.

  **Lexer**: tokens nuevos `Amp`/`Caret`/`Shl`/`Shr`/`Tilde`.
  `Pipe` (R.2.1) se reutiliza como OR bit-a-bit; el parser distingue
  por contexto (expression nivel bitwise vs arm de match).

  **Parser**: 4 niveles de precedencia nuevos entre comparación y
  rango (paralelo a Python/C):
  `comparison < | < ^ < & < << >> < range_expr < ...`. Unario `~`
  con la misma precedencia que `-` / `not`.

  **Tema del lexer `>>`**: el lexer ahora produce `Token::Shr` para
  `>>`, lo que rompía `List<List<Int>>` (el parser de tipos
  esperaba dos `Token::Gt` separados). Fix: en `parse_type_expr`,
  cuando se espera cerrar un generic con `>` y aparece `Shr`, se
  splittea el `Shr` mutando el token actual a `Gt` y avanzando la
  columna 1 char (técnica estándar de C++/Java/Rust). Cero impacto
  en otros usos de `>>`.

  **Checker**: ambos operandos deben ser `Int` (o `Any` gradual).
  Float/Bool/Str → error claro citando el operador y el tipo.

  **Evaluator**: ops Rust nativos (`& | ^`, `wrapping_shl`/
  `wrapping_shr`, `!` para BitNot). Shifts con RHS fuera de
  `0..64` → error de runtime claro (paralelo a Rust panic con
  shift overflow, pero como error recuperable).

  **Codegen**: emite Rust nativo. Los shifts envuelven el RHS en
  un bloque con check de rango + cast `as u32` (Rust requiere u32
  como exponente del shift sobre i64). Paridad bit-a-bit con el
  evaluator validada.

  **Grammar TextMate**: `<<`/`>>` antes de comparison (para no
  romper `<=`/`>=`), `&`/`^`/`~` después del operator lógico
  (para no romper `&&`/`||`).

  Implementación: ~290 LoC total entre lexer + parser + checker +
  evaluator + codegen + grammar. **11 unit tests nuevos** (6
  evaluator + 4 checker + 1 más sobre el split de `>>`) + 2
  compile_e2e bit-a-bit. Ejemplo
  `examples/guide/04b-operadores-bit.fitz` sumado al smoke
  `GUIDE_EXAMPLES_COMPILE` con casos canónicos: máscaras,
  set/clear/toggle de bits, byte extraction, combinación con
  format specs `{n:#x}`/`{n:#b}`.

  ~~**Deuda residual menor**: operadores bit-a-bit compuestos
  (`&=`/`|=`/`^=`/`<<=`/`>>=`)~~ ✓ CERRADO 2026-05-18 (mini-tanda
  Cmp). Cinco tokens nuevos al lexer (`AmpEq`/`PipeEq`/`CaretEq`/
  `ShlEq`/`ShrEq`); el parser los suma al match `compound_op`
  paralelo a `PlusEq`/etc. Desugar a `x = x <op> rhs` en parse-time,
  sin cambios al checker/evaluator/codegen (reusan `Stmt::Assign`
  regular). 4 unit tests (lexer) + 4 evaluator + 1 E2E bit-a-bit.

- ~~**`xor` lógico**~~ ✓ CERRADO 2026-05-18 (mini-tanda Xor).
  Operador binario `a xor b` sobre Bool: equivale a `a != b` pero
  más declarativo. Mismo nivel de precedencia que `or` (left-assoc),
  más bajo que `and`. NO hace short-circuit (necesita ambos lados).
  Implementación en 5 capas: **lexer** suma `Token::Xor` + keyword
  `"xor"`; **AST** suma `BinOpKind::Xor`; **parser** `logic_or`
  refactor para aceptar tanto `Token::Or` como `Token::Xor` con
  loop genérico; **checker** rama `And | Or | Xor` exige Bool en
  ambos lados; **evaluator** route via `eval_logical` (que ya valida
  tipos, pero sin short-circuit para Xor) y devuelve `lb != rb`;
  **codegen** emite `({} != {})` Rust directo. `fmt.rs` suma `xor`
  al `binop_str`. Grammar TextMate suma `xor` al pattern de
  `keyword.operator.logical.fitz`. F14 `is_const_eval_expr` también
  acepta `Xor` en operands const-eval. Tests: 4 parser (basic,
  chain misma precedencia, mix con or, mix con and) + 3 evaluator
  (tabla de verdad, sin short-circuit, no-Bool type error) + 3
  compile_e2e bit-a-bit (tabla, chain, mix and+or+xor). Ejemplo
  `examples/guide/06-logica.fitz` extendido con sección xor;
  cap 6 actualizado (tabla + sub-sección de chain + nota sobre
  no-short-circuit).

### Lexer / tokenización

- ~~**Separadores en números** `1_000_000`~~ ✓ CERRADO 2026-05-18
  (mini-tanda Núm). Permitidos en Int, mantisa Float y exponente
  científico. Rechazos: doble `_`, `_` al inicio o al final del
  número. Implementación en helper `read_digit_run` del lexer:
  recorre `digit (_ digit)*` y valida que después de `_` haya un
  dígito (no `__` ni `_<no-digit>`).
- ~~**Notación científica** `3.14e-2`~~ ✓ CERRADO 2026-05-18
  (mini-tanda Núm). `e` o `E` con signo `+`/`-` opcional. Al
  menos un dígito post-signo (`1e`, `1e+`, `1e-` son errores).
  Resultado siempre `Float` (incluso `1e10` sin punto decimal).
  Separadores también admitidos en el exponente (`1e1_0`,
  `1_000e1_0`).

  Implementación full-stack acotada al **lexer**: el parser/
  checker/evaluator/codegen no necesitan cambios porque el
  Token::Int/Float sintetizado lleva el mismo valor numérico que
  un literal "clásico" (el `_` se descarta antes del parse a
  `i64`/`f64`, y `f64::parse` ya acepta `e`/`E` nativamente).

  Grammar TextMate actualizado para colorear separadores y
  exponente. 9 unit tests nuevos del lexer (int+separador,
  float+separador, error doble underscore, error terminal,
  científica básica, signed exp, separator en exp, error exp
  vacío, regresión `t.0.0`). Ejemplo
  `examples/guide/03b-numeros-legibles.fitz` sumado al smoke
  `GUIDE_EXAMPLES_COMPILE`. Cap 3 de la guía suma sub-sección
  "Números legibles".

- ~~**Literales hex/binario/octal** `0xFF`, `0b1010`, `0o755`~~ ✓
  CERRADO 2026-05-18 (mini-tanda Lit). Tres prefijos en
  minúscula (`0x`/`0b`/`0o`). Dígitos hex case-insensitive
  (`0xff` == `0xFF`). Separadores `_` heredados de Núm también
  funcionan (`0xDEAD_BEEF`, `0b1010_1010`, `0o7_5_5`). Overflow
  sobre `i64` → error claro del lexer.

  Implementación acotada al **lexer**: helper nuevo
  `read_radix_number(radix, name, line, col)` que consume el
  prefijo, lee dígitos válidos para la base con separadores
  intercalados, y parsea con `i64::from_str_radix`. El branch
  se inserta al inicio de `read_number` con un lookahead
  (`peek == '0'` + `peek_next == 'x'/'b'/'o'`). Cero cambios
  al parser/checker/evaluator/codegen — el `Token::Int`
  sintetizado lleva el mismo valor que un literal decimal
  equivalente.

  Grammar TextMate actualizado con 3 patterns nuevos (hex/bin/
  oct antes del Int decimal por especificidad). 8 unit tests
  nuevos del lexer (hex case-insensitive, bin+oct básicos,
  separadores, error sin dígitos tras prefijo, error dígito
  inválido, overflow, error underscore terminal/doble,
  regresión decimal `0`/`007`/`0.5`). Ejemplo
  `examples/guide/03c-bases-numericas.fitz` sumado al smoke
  `GUIDE_EXAMPLES_COMPILE`. Cap 3 de la guía suma sub-sección
  "Literales en otras bases (mini-tanda Lit)".

  ~~**Deuda residual menor**: prefijos en mayúscula (`0X`/`0B`/
  `0O`)~~ ✓ CERRADO 2026-05-18 (mini-tanda Cmp). El match en
  `read_number` ahora acepta `'x'|'X'`/`'b'|'B'`/`'o'|'O'`.
  Grammar TextMate también actualizado con `[xX]`/`[bB]`/`[oO]`
  en los patterns. 1 unit test (lexer).

- ~~**Identificadores no-ASCII** (`π`, `función`) — F8~~ ✓ CERRADO
  2026-05-18 (mini-tanda F8). Verificación + documentación + tests:
  el lexer ya usaba `is_alphabetic()`/`is_alphanumeric()` (que son
  Unicode-aware en Rust), así que en la práctica los identificadores
  con Unicode ya andaban — solo faltaba documentar el contrato y
  lockear el comportamiento con tests. Rust acepta Unicode
  identifiers desde edition 2021, así que `fitz build` los pasa
  transparente al código generado. Coverage:
  - Letras griegas (`π`, `σ`).
  - Acentos / ñ (`función`, `niño`, `café`).
  - CJK (`名前`, `用户`, `이름`).
  - Cirílico (`имя`).
  - Mezcla Unicode + ASCII + `_` (`user_名`, `café_2`).
  - Emojis EXCLUIDOS — Unicode "Symbol", no "Letter". Lex aborta.
  - Dígitos no-ASCII (`٢`) al inicio también rechazados.
  7 unit tests del lexer (`f8_*`) + 2 compile_e2e bit-a-bit. Ejemplo
  `examples/guide/03d-identifiers-unicode.fitz` sumado al smoke
  `GUIDE_EXAMPLES_COMPILE`. Cap 3 sub-sección nueva "Identificadores
  con Unicode (mini-tanda F8)" con tabla de reglas + convenciones
  recomendadas (ASCII para API pública, Unicode para uso interno).
  **Caveat heredado de F12**: el codegen no permite que un fn body
  referencie vars top-level (ni ASCII ni Unicode), así que `π`
  declarada top-level no es accesible desde `fn área_círculo(...)`
  — paso `π` como param. Limitación NO específica de Unicode.
- ~~**Multi-línea en `from import (...)` con paréntesis**~~ ✓
  CERRADO 2026-05-18 (mini-tanda Mln). Habilita la forma estilo
  Python: `from foo import (a, b, c,)` con items en líneas
  separadas y trailing comma opcional. Implementación acotada al
  **parser**: `parse_from_import` detecta `(` después de
  `import`, entra a modo multi-línea con helper
  `skip_newlines_inside_parens` que consume newlines entre
  items, parsea names + aliases con la misma lógica que
  single-line, y expecta `)` al final. Sin cambios al
  lexer/AST/checker/evaluator/codegen (el AST resultante es
  idéntico al de la forma single-line). Aliases (`as`)
  funcionan igual; mezclables. Grammar TextMate ya manejaba
  todos los tokens (`from`/`import` keywords + parens + commas
  + newlines), sin cambios. 5 unit tests parser (single-line
  con parens, multi-línea canónico, aliases mixtos, sin
  trailing comma, sin cerrar es error) + 2 compile_e2e
  bit-a-bit. Ejemplo
  `examples/guide/16d-import-multilinea.fitz` + módulo aux
  `import_multilinea_utils.fitz` sumado al smoke
  `GUIDE_EXAMPLES_COMPILE`. Cap 16 sub-sección nueva "Forma
  multi-línea con paréntesis (mini-tanda Mln)" + bullet stale
  "Multi-línea no soportado" removido de "Qué no se puede
  hacer todavía".
- ~~**Escapes extendidos** `\u{...}`, `\x..`, `\0`, `\b` — F9~~ ✓
  CERRADO 2026-05-18 (mini-tanda F9). Cuatro escapes adicionales en
  strings normales y triple-quote: `\0` (NUL), `\b` (backspace),
  `\xXX` (byte ASCII 0x00-0x7F), `\u{X...}` (Unicode escalar
  1-6 dígitos hex, hasta U+10FFFF). Implementación acotada al
  **lexer** (cero cambios al parser/checker/evaluator/codegen —
  el `Token::Str` ya viene con los chars resueltos). Helpers
  privados `read_unicode_escape` y `read_hex_byte_escape` con
  validaciones: codepoint > 10FFFF rechazado, surrogates D800-DFFF
  rechazados, `\u{}` vacío rechazado, `\u{...}` con >6 dígitos
  rechazado, `\xXX` con value >0x7F rechazado (sugerencia: usar
  `\u{...}`), `\xX` con <2 dígitos rechazado. Los nuevos escapes
  funcionan en strings simples y en `"""..."""` (lógica duplicada
  intencionalmente por simetría — ambos paths quedan auditables).
  Grammar TextMate actualizado con 2 patterns nuevos
  (`constant.character.escape.unicode.fitz` y
  `constant.character.escape.hex.fitz`) colocados ANTES del
  `\\.` genérico para que tengan precedencia. **10 unit tests
  nuevos** del lexer (null+backspace, unicode BMP+suplementario+
  lowercase+1-dígito, errores: vacío/sin-cerrar/surrogate/
  too-long, hex ASCII, hex fuera-de-rango rechazado, hex pocos
  dígitos, triple-quote con escapes extendidos) + 1 compile_e2e
  bit-a-bit. Ejemplo
  `examples/guide/05d-escapes-extendidos.fitz` sumado al smoke
  `GUIDE_EXAMPLES_COMPILE`. Cap 5 de la guía sumó tabla
  completa de escapes con los 4 nuevos + sub-sección de reglas
  + referencia al ejemplo. **Decisión de diseño**: `\xXX` se
  restringe a ASCII (paralelo a Rust); para chars no-ASCII hay
  que usar `\u{...}`. Esto evita ambigüedad con Latin-1 que
  Python sí acepta.

### ~~Strings — métodos extras~~ ✓ CERRADO 2026-05-17 (mini-tanda S.1 + S.2)

- ~~`Str.contains(s)`~~ ✓ (S.1)
- ~~`Str.split(sep)`~~ ✓ (S.2). Retorna `List<Str>` materializado
  (no iterator). Empty separator → chars individuales (igual que
  Python por default).
- ~~`Str.trim()`~~ ✓ (S.2). ~~`.trim_start()` / `.trim_end()`
  quedan como deuda menor~~ ✓ CERRADO 2026-05-18 (mini-tanda Mb).
  Ambas variantes parciales agregadas en 4 capas: `str_trim_start`/
  `str_trim_end` en evaluator (delegan a `String::trim_start`/
  `trim_end` de Rust); branch nuevo `"trim_start" | "trim_end"`
  en checker (signature `fn() -> Str`, arity 0); 2 ramas
  `(Type::Str, "trim_start")` y `(Type::Str, "trim_end")` en
  codegen (emite `({}).trim_start().to_string()`/`.trim_end()`).
  LSP autocomplete suma las 2 entradas con detail `fn() -> Str`.
  Grammar TextMate sin cambios (los métodos comparten el pattern
  general de identifiers).
- ~~`Str.starts_with(s)` / `.ends_with(s)`~~ ✓ (S.1).
- ~~`Str.replace(old, new)`~~ ✓ (S.2). Reemplaza TODAS las
  ocurrencias.
- ~~`Str.repeat(n)`~~ ✓ (S.2). `n < 0` es error; `n == 0` →
  string vacío.

Implementación en 4 capas (evaluator + checker + codegen + fmt
intact). Tests exhaustivos: ~15 unit del evaluator + ~10 del
checker + smoke E2E bit-a-bit `fitz run` ↔ `fitz build` sobre
`examples/guide/13c-metodos-extras.fitz` (sumado al smoke
GUIDE_EXAMPLES_COMPILE).

### ~~Listas — métodos extras~~ ✓ CERRADO 2026-05-17 (mini-tanda S.3)

- ~~`xs.sort()`~~ ✓ (S.3). IN-PLACE, soporta List<T> para T en
  {Int, Float, Str, Bool}. Float usa `partial_cmp` con fallback
  `Equal` (NaN-tolerant). Heterogéneos → error de runtime claro;
  el codegen rechaza tipos no soportados estático.
- ~~`xs.reverse()`~~ ✓ (S.3). IN-PLACE, cualquier T.
- ~~`xs.contains(v)`~~ ✓ (S.3). Igualdad estructural via
  `PartialEq` (la custom emitida para nominales/listas/maps).
- ~~`xs.sort_by(fn)` — diferido. Necesita callback comparator~~
  ✓ CERRADO 2026-05-18 (mini-tanda Mb). Callback estilo Rust/JS
  `cmp(a, b) -> Int` (negativo si a<b, cero si igual, positivo si
  a>b). 4 capas: `list_sort_by` async en evaluator con selection
  sort O(n²) (callback es async, no podemos pasarlo a `Vec::sort_by`
  que es sync; aceptable hasta que aparezca presión real); rama
  `"sort_by"` en checker validando que el callback sea
  `fn(T, T) -> Int`; rama codegen que emite `sort_by` Rust nativo
  con closure binaria que mapea Int → Ordering. Nuevo helper
  `gen_binary_callback_inline` para callbacks con 2 params
  (paralelo a `gen_callback_inline`). LSP autocomplete suma
  `("sort_by", "fn(T, T) -> Int) -> Null")`. Bindeo del receptor
  a un local antes del lock para evitar E0716 con temporaries.
- ~~`xs.zip(ys)`~~ ✓ CERRADO 2026-05-18 (mini-tanda It). Ver
  entrada dedicada abajo.
- ~~`xs.flatten()` para `List<List<T>>` — diferido~~ ✓ CERRADO
  2026-05-18 (mini-tanda Mb). Aplana un nivel: `List<List<U>>`
  → `List<U>`. 4 capas: `list_flatten` en evaluator (snapshot +
  loop con type-check; error de runtime claro si un elemento no
  es List); rama `"flatten"` en checker valida `T == List<U>` y
  devuelve `List<U>`, `Any` recipient pasa gradual; codegen emite
  `Arc::new(Mutex::new(...iter().cloned().flat_map(|sub|
  sub.lock().unwrap().clone())...))`; LSP autocomplete suma la
  entrada con detail `fn() -> List<U>  // requiere List<List<U>>`.
- ~~`xs.any(pred)` / `xs.all(pred)` / `xs.count(pred)` /
  `xs.find_index(pred)`~~ ✓ CERRADO 2026-05-18 (mini-tanda Lx).
  Cuatro predicados funcionales sobre `List<T>`, completan la
  API funcional con patrones canónicos de programación funcional.
  Todos toman `fn(T) -> Bool`. Devuelven: `any`/`all` → `Bool`
  (short-circuit en primer true/false), `count` → `Int`,
  `find_index` → `Result<Int>` (Ok del índice 0-based o
  Err("no encontrado")). Lista vacía: `any` → false, `all` →
  true (vacuous truth, paralelo a Python/Rust), `count` → 0,
  `find_index` → Err. 4 capas: evaluator (4 fns nuevas
  `list_any`/`list_all`/`list_count`/`list_find_index`),
  checker (signatures en `infer_list_method` reutilizan
  `check_unary_callback` con ret Bool), codegen (`any`/`all`
  usan `.iter().cloned().any(<cb>)` / `.all(<cb>)` directo
  porque Rust acepta `FnMut(T) -> bool`; `count` y `find_index`
  van por manual loop porque `Iterator::filter`/`position`
  toman `FnMut(&T)` y no encajan con nuestro callback que
  espera `T` por valor), LSP autocomplete suma 4 entradas.
  5 unit tests evaluator + 1 LSP unit + 3 compile_e2e bit-a-bit.
  Ejemplo `examples/guide/13h-predicados-list.fitz` sumado al
  smoke `GUIDE_EXAMPLES_COMPILE` con caso típico (filtrar
  reportes graves, validación de edades). Cap 13 tabla de
  métodos `List<T>` extendida con las 4 nuevas filas.

### ~~Métodos chicos + Range step (List.min/max/sum + Str.pad_start/pad_end + Map.keys_sorted + Range.step_by)~~ ✓ CERRADO 2026-05-18 (mini-tanda Mb2 + Rg)

Bundle de polish ergonómico chico, todos en 4 capas (evaluator +
checker + codegen + LSP autocomplete). Cierra deudas residuales
del prompt de cierre de sesión anterior ("Más métodos chicos:
`List.min`/`max`/`sum` (homogéneos), `Str.pad_start`/`pad_end`,
`Map.keys_sorted`" + "Range step `(0..10 step 2)`").

- ~~**`List.min()` / `List.max()`**~~ ✓ Devuelven `Result<T>` —
  `Err("lista vacía")` cuando no hay elementos. Solo válidos sobre
  `List<Int>` o `List<Float>` homogéneos; otros tipos → error del
  checker (estático) o del evaluator (gradual). Float usa
  `partial_cmp` con NaN handling (Equal como fallback, paralelo a
  `list_sort`).
- ~~**`List.sum()`**~~ ✓ Devuelve `T` (`Int` o `Float`). Lista vacía
  → `Int(0)` sentinel (sin info de tipo declarado en runtime).
  Mismo chequeo de homogeneidad que min/max. Codegen emite
  `.iter().copied().sum::<T>()` directo (Rust nativo).
- ~~**`Str.pad_start(width, ch)` / `Str.pad_end(width, ch)`**~~ ✓
  Padding paralelo a Python `str.rjust`/`ljust`. `ch` debe ser
  exactamente 1 char (validado en runtime; runtime error con
  mensaje claro si tiene 0 o ≥2). Si `len(s) >= width`, devuelve
  `s` sin cambios.
- ~~**`Map.keys_sorted()`**~~ ✓ Devuelve `List<K>` con keys ordenadas.
  K en {Int, Float, Str, Bool} (validado runtime). Map vacío →
  lista vacía. Útil para iterar en orden canónico cuando insertion
  order no es lo deseado. El codegen bindea el receptor a `__map`
  antes del lock (paralelo a `first`/`last`) para evitar E0716.
- ~~**`Range.step_by(n)`**~~ ✓ Materializa el rango con step.
  `n: Int > 0` (validado runtime — 0 o negativo → error claro).
  Devuelve `List<Int>`. El codegen detecta el patrón
  `Expr::Range.step_by(n)` ANTES del bloque general de Range
  (que materializa todo el rango) y emite directo
  `(start..end).step_by(n).collect()` Rust nativo — evita
  materializar el rango entero primero.

Implementación: ~250 LoC entre evaluator (helpers + 7 fns nuevas
`list_min`/`list_max`/`list_sum`/`require_numeric_list`/
`str_pad_args`/`str_pad_start`/`str_pad_end`/`map_keys_sorted`/
`range_step_by`), types (`infer_list_method` suma 3 ramas,
`infer_str_method` suma 1, `infer_map_method` suma 1,
`infer_range_method` suma 1), codegen (4 ramas nuevas + caso
especial para Range.step_by), LSP (4 entries nuevos en
`after_dot_completions`).

**14 unit tests** del evaluator + **12 unit tests** del checker
+ **8 compile_e2e** bit-a-bit `fitz run` ↔ `fitz build`. Ejemplo
runnable `examples/guide/13m-min-max-sum-pad-keys-step.fitz`
sumado al smoke `GUIDE_EXAMPLES_COMPILE` (con casos canónicos
para los 4 métodos + edge cases vacíos + chain con sum).

Cap 13 de la guía: 3 filas nuevas en tabla `List<T>` (min/max/
sum), 2 en tabla `Str` (pad_start/pad_end), 1 en tabla `Map<K,V>`
(keys_sorted), sub-sección dedicada "Reducciones + padding + keys
ordenadas + Range con step (mini-tanda Mb2 + Rg)" con ejemplos
inline. Cap 9 sumó sub-sección "Step con `step_by(n)`" en la
parte de rangos. VSCode extension: grammar TextMate sin cambios
(identifiers genéricos); LSP autocomplete refleja todo
automáticamente via rebuild del `fitz-lsp` binary.

### ~~Updates + Polish ergonómico (Map.update + comp tuple destruct + LSP param names)~~ ✓ CERRADO 2026-05-18 (mini-tanda Up)

Bundle de 3 deudas residuales chicas, todas relacionadas a "ergonomía"
del lenguaje y tooling:

- ~~**`Map.update(k, fn(V) -> V)`**~~ ✓ Update inmutable atómico de
  un value asociado a una key. Si `k` no está, no-op (no inserta).
  Cubre el patrón canónico `m.update("ada", fn(v) => v + 10)` sin
  tener que `get(k)?` + reconstruir el map. Paralelo a Rust
  `HashMap::entry().and_modify()`. Implementación en 4 capas con
  callback async + signatures fijas (`fn(V) -> V`, mismo V para
  preservar el tipo del Map).

- ~~**Comprehension con tuple destructuring**~~ ✓ Ver entrada arriba
  en la sección de Comprehensions — `Expr::ListComp.var` migró de
  `String` a `Pattern`. Reusa toda la infraestructura de Md
  (`bind_for_pattern`, `bind_for_pattern_in_checker`, codegen
  `pattern_to_simple_binding` + tuple destructuring).

- ~~**LSP autocomplete con param names**~~ ✓ Ver entrada arriba en
  la deuda residual del LSP. `NominalMethod` ahora incluye
  `param_names: Vec<String>` paralelo a `params`. Mejor UX en
  autocomplete y hover sobre métodos custom.

Tests: 3 unit nuevos evaluator (Up.1 update key existente/inexistente,
Up.2 comprehension tuple destructure) + 2 LSP unit (update en
autocomplete, param names en signatures) + 1 parser unit nuevo
(`up_comprehension_acepta_tuple_destructuring`) + 2 compile_e2e
bit-a-bit. Ejemplo `examples/guide/13l-update-comp-tuple-paramnames.fitz`
sumado al smoke `GUIDE_EXAMPLES_COMPILE` con pipelines chained
(scores.update().merge()), comprehension con tuple destructure
construyendo instancias, y Point con distance_to demostrando la
firma del autocomplete. Cap 13 tabla Map sumó row `update` + cap 9
"Cobertura del MVP" de comprehensions actualizada (de "deuda" a
"sí anda con Up").

VSCode extension: grammar TextMate sin cambios (los métodos comparten
identifiers + la sintaxis de comprehension tuple destructure usa
tokens existentes `(`, `,`, `)`). LSP autocomplete refleja `update`
automáticamente + el param names update es de upgrade transparente
(re-build del fitz-lsp binary toma el cambio).

### ~~Extras de API 2: List.flat_map/first/last + Map.merge~~ ✓ CERRADO 2026-05-18 (mini-tanda Ex2)

Bundle siguiente al de Ex, cierra deudas chicas adicionales:

- ~~**`xs.flat_map(fn(T) -> List<U>) -> List<U>`**~~ ✓ Combina
  map + flatten en un paso. Cierra la deuda diferida de S.3
  ("flat_map combinación de map + flatten"). Implementación en
  evaluator (snapshot + loop con type-check del ret del callback)
  + checker (inferencia de U del ret del callback) + codegen
  (snapshot + for-loop + flatten via `.extend(__sub.lock()...)`).
- ~~**`xs.first()` / `xs.last()` → `Result<T>`**~~ ✓ Accessors
  seguros que devuelven `Err("lista vacía")` en vez de panic.
  Codegen reusa el bindeo del receptor a un local antes del lock
  para evitar E0716 con temporaries (mismo patrón que sort_by).
- ~~**`m.merge(other)` → `Map<K, V>`**~~ ✓ Combina dos Maps con
  política last-write-wins (paralelo a Python `{**m, **other}` /
  JS spread / Rust `extend`). Preserva orden: keys del receiver
  primero, keys nuevas de `other` al final. Implementación en
  evaluator (clone del receiver + loop buscando keys existentes
  para sobreescribir) + checker (valida `Map<K2, V2>` compatible
  con `Map<K, V>`) + codegen (mismo patrón).

Implementación en 4 capas cada uno (eval + checker + codegen +
LSP). 5 unit tests evaluator + 2 LSP unit + 3 compile_e2e
bit-a-bit. Ejemplo `examples/guide/13k-flat-map-first-last-merge.fitz`
sumado al smoke `GUIDE_EXAMPLES_COMPILE` con casos típicos
(`Order` con flat_map sobre items, config con merge). Cap 13
tablas de List + Map extendidas con 4 filas nuevas.

VSCode extension: grammar TextMate sin cambios (los nombres son
identifiers genéricos). LSP autocomplete actualizado con las 4
firmas — `flat_map`/`first`/`last` en List, `merge` en Map.

### ~~Extras de API: Str search + Map transforms~~ ✓ CERRADO 2026-05-18 (mini-tanda Ex)

Mini-tanda bundle que cierra 3 deudas chicas relacionadas:

- ~~**`Str.find(sub)` / `Str.index_of(sub)` / `Str.last_index_of(sub)`**~~
  ✓ Devuelven `Result<Int>` con el char index (no byte index) de
  la 1ra ocurrencia (o última, para `last_index_of`); `Err("no
  encontrado")` si no matchea. `index_of` es alias de `find`
  (estilo JS/TS — ambos nombres son comunes en distintas
  comunidades). El codegen convierte byte index de Rust a char
  index con `s[..byte_idx].chars().count()` para que el output
  matchee el evaluator bit-a-bit (importante para strings con
  Unicode no-ASCII tipo "café latte").

- ~~**`Map<K,V>.filter(fn(K, V) -> Bool)` / `Map<K,V>.map_values(fn(V) -> U)`**~~
  ✓ Transformaciones funcionales sin mutar el receiver. `filter`
  keeps pares donde el callback es true, devuelve `Map<K, V>`.
  `map_values` aplica `fn(V) -> U` a cada value, mantiene las
  keys, devuelve `Map<K, U>`. Codegen reusa
  `gen_binary_callback_inline` (refactorizado para aceptar ret
  type Bool o Int según el método caller) para `filter`; usa
  `gen_callback_inline` (1-arg) para `map_values`. Habilita
  pipelines tipo `scores.filter(...).map_values(...)`.

Implementación: 4 capas cada uno (evaluator + checker + codegen
+ LSP). Helper nuevo `check_binary_callback` en el checker para
validar callbacks de 2 params + ret esperado (paralelo a
`check_unary_callback` heredado de S.3). 5 unit tests evaluator
+ 2 LSP unit + 2 compile_e2e bit-a-bit. Ejemplo
`examples/guide/13j-extras-str-map.fitz` sumado al smoke
`GUIDE_EXAMPLES_COMPILE` con pipelines típicos. Cap 13 tabla
`Str` extendida con 3 filas + tabla `Map` con 2 filas
+ referencia al ejemplo.

**F5 + F1 docs cleanup** (mismo bundle):
- ~~F5 verificación~~ ✓ `FnDef.is_async: bool` cableado end-to-end
  desde Fase 6 + F17. Sin deuda residual.
- ~~F1 matriz Type::Any~~ ✓ Lista auditada de casos donde aparece
  `Type::Any` en el checker (ver entrada F1 abajo).

### ~~Iteradores estilo Python — `enumerate`/`zip`/`chain`~~ ✓ CERRADO 2026-05-18 (mini-tanda It)

Tres métodos canónicos sobre `List<T>` que componen listas sin
loops manuales. Encajan natural con Md (tuple destructuring del
for) — el caso canónico es `for (i, x) in xs.enumerate()`.

- ~~`xs.enumerate()`~~ → `List<(Int, T)>` con pares (índice,
  elemento). Checker: signature directa. Evaluator: snapshot
  con `iter().enumerate().map(...)`. Codegen: emite Rust nativo
  `.iter().cloned().enumerate().map(|(__i, __v)| (__i as i64, __v))`
  con `Vec<(i64, T)>` final.
- ~~`xs.zip(ys)`~~ → `List<(T, U)>`, paramétrica en U. Trunca al
  más corto (paralelo a Python). El checker permite U arbitrario.
  Codegen: `.iter().cloned().zip(...).collect::<Vec<(T, U)>>`.
- ~~`xs.chain(ys)`~~ → `List<T>` concatenada. `ys` debe ser
  `List<T>` (mismo tipo). Codegen: `.iter().cloned().chain(...)`.

**Cambio colateral al codegen del for** para soportar el caso
canónico: cuando el iter es `List<Tuple(...)>` y el var del `for`
es `Pattern::Tuple` del mismo aridad, emite destructuring nativo
Rust `for (a, b) in xs.lock()...`. Paralelo a cómo Map ya lo
hacía. Esto destraba `for (i, x) in xs.enumerate() { ... }` en
`fitz build` con paridad bit-a-bit.

Implementación: ~210 LoC en `src/types.rs` (3 signatures
nuevas) + `src/evaluator.rs` (3 fns) + `src/codegen.rs` (3
ramas + refactor del for sobre List<Tuple>) + `src/lsp.rs`
(autocomplete suma 3 entries). **8 unit tests nuevos** (3
evaluator + 5 checker) + 1 LSP test + 2 E2E compile bit-a-bit
(`fitz run` ↔ `fitz build`). Ejemplo
`examples/guide/13d-iteradores.fitz` sumado al smoke
`GUIDE_EXAMPLES_COMPILE`. Cap 13 de la guía suma sub-sección
"Iteradores: enumerate / zip / chain (mini-tanda It)".

**Deuda residual menor**:
- ~~Iteradores sobre `Range` (`(0..10).enumerate()`)~~ ✓ CERRADO
  2026-05-18 (mini-tanda Ir). Habilita `enumerate`/`zip`/`chain`/
  `len` sobre `Type::Range` además de `List<T>`. 4 capas:
  evaluator dispatcha 4 ramas nuevas `(Value::Range, "...")` que
  materializan via helper `range_to_list(start, end)` y delegan a
  los métodos de List; checker añade `infer_range_method` con
  signatures fijas (enumerate → `List<(Int, Int)>`, zip → con U
  paramétrico, chain → `List<Int>`, len → `Int`); codegen
  intercepta `Expr::Range` como receptor de method call y
  materializa inline a `Arc<Mutex<Vec<i64>>>` con
  `(start..end).collect::<Vec<i64>>()` (inclusivo suma 1 al end
  paralelo al parser de R.1.4), luego delega al dispatch de
  `List<Int>` natural; LSP autocomplete suma `Type::Range` con
  los 4 métodos. 4 unit tests evaluator + 1 LSP unit + 4
  compile_e2e bit-a-bit. Ejemplo
  `examples/guide/13f-range-iteradores.fitz` sumado al smoke
  `GUIDE_EXAMPLES_COMPILE`. Cap 13 sub-sección nueva "Iteradores
  sobre Range". **Caveats documentados**: `Range.chain(Range)`
  directo no funciona (chain espera `List<Int>` — workaround:
  materializar el segundo con list comprehension), y `Range` no
  expone `map`/`filter`/`find`/`sort` (usar List materializada).
- ~~`xs.flat_map(fn)` (combinación de map + flatten)~~ ✓ CERRADO
  2026-05-18 (mini-tanda Ex2). Ver entrada dedicada abajo.

### ~~Loops~~ ✓ CERRADO 2026-05-17 (mini-tanda L post-T)

- ~~**`loop` como expresión con valor** `let x = loop { break v }`~~
  ✓ (L.1). Nuevo `Expr::Loop { body, label, span }` paralelo a
  `Stmt::Loop`. `EvalSignal::Break(Value, Option<String>)`. El
  tipo del Expr::Loop es el lub de todos los `break <v>`
  adentro; sin break con valor → Null. Codegen emite Rust nativo
  `loop { break <v> }`.
- ~~**Labels** en break/continue `break 'outer`~~ ✓ (L.2).
  Lexer suma `Token::Label(String)` para `'name`. AST suma
  `label: Option<String>` a Loop/While/For/Expr::Loop +
  `Stmt::Break(value, label, span)`/`Stmt::Continue(label,
  span)`. Evaluator usa `label_matches()` para decidir si
  capturar o propagar signal. Codegen emite Rust nativo:
  `'name: loop { ... break 'name <v>; }`.

Implementación en 6 capas (ast, lexer, parser, evaluator,
checker, codegen). Cap 8 de la guía actualizado con
sub-secciones "Loop como expresión" y "Labels". Ejemplo nuevo
`examples/guide/08b-loops-avanzados.fitz` sumado al smoke.

### ~~Index / slicing~~ ✓ CERRADO 2026-05-17 (mini-tanda I post-S)

- ~~**Índices negativos** `xs[-1]`~~ ✓ (I.1). Wrap `len + i`.
  Funciona en lectura (`xs[-1]`), asignación (`xs[-1] = v`), y
  para strings (`s[-1]` devuelve `Str` de un char). Out-of-range
  → error de runtime.
- ~~**Slicing** `xs[a..b]`, `xs[..b]`, `xs[a..]`, `xs[..]`,
  `xs[a..=b]`~~ ✓ (I.2). Sintaxis parsea via flag
  `in_slice_context` que silencia `range_expr` adentro de `[`.
  Nueva variante `Expr::Slice { object, start: Option<Box<Expr>>,
  end: Option<Box<Expr>>, inclusive: bool, span }`. Política
  Python-style: clamp silencioso para extremos fuera de rango,
  `start > end` tras clamp → vacío. Devuelve copia (no view),
  funciona sobre List<T> y Str.

Implementación en 5 capas (ast, parser, checker, evaluator,
codegen, fmt). **16 unit tests nuevos** + smoke E2E bit-a-bit
`fitz run` ↔ `fitz build` sobre
`examples/guide/09b-indexing-slicing.fitz` (sumado al smoke
`GUIDE_EXAMPLES_COMPILE`). Cap 9 de la guía suma sub-sección
"Indexing y slicing (mini-tanda I)".

**Diferido como deuda menor**: slicing con paso (`xs[::2]`).
Sin demanda concreta.

### ~~Comprehensions~~ ✓ CERRADO 2026-05-18 (mini-tanda C)

- ~~`[x * 2 for x in xs]`~~ ✓ — list comprehensions con AST node
  dedicado `Expr::ListComp { expr, var, iter, filter, span }`.
  Decisión: AST propio (no desazúcar a `.map()` en parse) para
  que el fmt preserve la sintaxis y los errores del checker
  apunten al `for` real. Consistente con cómo T sumó
  `Expr::Tuple` propio.
- ~~Filter inline `[x for x in xs if x > 0]`~~ ✓ —
  `if cond` opcional al final. Tipa como `Bool` en el checker;
  short-circuit en runtime con `continue`.
- ~~`iter` puede ser `List<T>` o `Range`~~ ✓ — paralelo a la
  cobertura de `for ... in` del evaluator.
- **Scope local del var** (decisión Python-style) — a diferencia
  del `for ... in` clásico de Fitz que deja la var visible
  afuera, las comprehensions abren un env hijo dedicado y el
  var no escapa. El checker hace `push_scope`/`pop_scope`
  paralelo.

Implementación en 6 capas (ast, parser, evaluator, checker,
codegen, fmt, lint, lsp walker pendiente). **14 unit tests
nuevos** (4 parser + 5 evaluator + 5 checker) + 2 E2E
compile bit-a-bit `fitz run` ↔ `fitz build` sobre
`examples/guide/09d-comprehensions.fitz` (sumado al smoke
`GUIDE_EXAMPLES_COMPILE`). Cap 9 de la guía suma sub-sección
"List comprehensions (mini-tanda C)".

**Diferido como deuda residual menor**:
- ~~Destructuring del var `[a + b for (a, b) in pairs]`~~ ✓
  CERRADO 2026-05-18 (mini-tanda Up). `Expr::ListComp.var` cambió
  de `String` a `Pattern` (paralelo a `Stmt::For.var` de Md).
  Parser reusa `parse_pattern`, checker `bind_for_pattern_in_checker`,
  evaluator `bind_for_pattern`, codegen emite destructuring nativo
  Rust `for (mut a, mut b) in ...`. Cero refactor adicional —
  toda la infraestructura ya existía de Md. fmt.rs también
  actualizado para emitir Pattern via `fmt_pattern`.
- Múltiples `for` clauses `[x*y for x in xs for y in ys]` —
  cartesian product. Python lo soporta; sin demanda concreta.
- Set/Map comprehensions — Map comprehension `{k: v for ...}`
  podría ser útil si entra demanda; el grammar lo destrabaría
  con un patrón paralelo.

### ~~Format specifiers~~ ✓ CERRADO 2026-05-18 (mini-tanda Fm)

- ~~`{ratio:.2f}` en interpolación~~ ✓ — full Python-compatible
  subset implementado en 7 capas (ast + parser + evaluator +
  checker + codegen + fmt + módulo runtime nuevo `src/format.rs`).
- **AST**: `StrPart::Expr` cambió shape de `Expr(Expr)` a
  `Expr(Expr, Option<FormatSpec>)`. Nueva struct `FormatSpec` con
  enums `FormatAlign`/`FormatSign`/`FormatKind`. Helpers `to_char()`
  y `FormatSpec::to_source()` para reconstruir la sintaxis.
- **Parser**: `build_string_expr` separa `{expr:spec}` por el
  primer `:` a depth 0 (no adentro de paréntesis/brackets/braces
  balanceados). Helper `parse_format_spec` con gramática Python
  `[[fill]align][sign][#][0][width][grouping][.precision][type]`.
- **Evaluator**: módulo nuevo `src/format.rs` con
  `format_value_with_spec(value, spec) -> Result<String, String>`.
  Aplica width/align/fill/sign/alternate/grouping/precision/kind
  con la semántica Python. Cubre todos los kinds: `b`/`c`/`d`/`e`/
  `E`/`f`/`F`/`g`/`G`/`o`/`s`/`x`/`X`/`%`.
- **Checker**: `validate_format_spec_for_type` valida que el tipo
  del expr sea compatible con el `kind`. `{x:.2f}` con `x: Str` da
  error antes de runtime. `{x:d}` con `x: Float` también. Sin
  `kind`, cualquier tipo pasa.
- **Codegen** (`fitz build`): `format_spec_to_rust` traduce el
  spec Fitz a un format string Rust nativo (`:.2`, `:#x`, `:05d`,
  `:*>5`, etc.). El binario nativo produce output **bit-a-bit
  idéntico** al evaluator para el subset que Rust soporta directo.
  Specs sin equivalente directo en Rust (`,`/`_` grouping, `g`/`G`,
  `c`, `%`) → error de codegen claro citando `fitz run` como
  workaround.
- **Fmt**: `FormatSpec::to_source()` reconstruye la sintaxis
  source del spec para que `fitz fmt` la preserve en el output.

Implementación: ~530 LoC en `src/format.rs` + cambios en 7 archivos
del compiler. **24 unit tests nuevos** (12 format runtime + 7
parser + 5 checker) + 4 E2E compile bit-a-bit `fitz run` ↔ `fitz
build` sobre Float precision, Int zero-pad, hex alternate, alignment.
Cap 5 de la guía suma sub-sección "Format specifiers (mini-tanda
Fm)" con tabla de gramática completa y matriz de compatibilidad
por type. Ejemplos:
- `examples/guide/05b-format-specs.fitz` (subset compilable,
  sumado al smoke `GUIDE_EXAMPLES_COMPILE`).
- `examples/guide/05c-format-specs-advanced.fitz` (full Python,
  solo `fitz run`).

**Deuda residual** (NO bloquea mini-tandas siguientes):
- Subset reducido en `fitz build`: `,`/`_` grouping, `g`/`G`,
  `c`, `%` requieren `fitz run`. Refactor para soportarlos en
  binario nativo requeriría emitir el código del runtime
  `format_value_with_spec` adentro del binario o helpers manuales
  per-spec. Trade-off aceptado.
- `n` (locale-aware) — Fitz no tiene locale; sin sentido.
- Spec dinámico `{x:.{n}f}` (precision determinada en runtime)
  no soportado — Python lo tiene, pero requiere doble parse.

### ~~Result avanzado~~ ✓ CERRADO 2026-05-18 (mini-tanda Err+)

- ~~**`?` fuera de fn con mensaje propio**~~ ✓ (cap 14). El `?`
  internamente reutilizaba el mecanismo de `return`, así que al
  escapar a top-level daba el genérico "return fuera de función".
  Fix: `signal_to_error` detecta `Return(Value::Result(Err(...)))`
  y devuelve un FitzError específico mostrando el contenido del
  Err con `Display`. Mensaje nuevo: `operación `?` falló con Err:
  <value>`. Funciona end-to-end en `fitz run` con cualquier tipo
  del Err (Str/Int/Instance/Tuple).

- ~~**`Err` con valores no-Str en `fitz run`**~~ ✓ (cap 14). El
  evaluator ya aceptaba `Err(any_value)` por design — Err+ valida
  que funcione end-to-end con tipos custom: `Err(Int)` preserva
  el number, `Err(MiError { ... })` preserva la Instance con su
  Display canónico, etc. Al desempacar con `match Err(e)`, el
  binding `e` mantiene el tipo exacto.

- **Codegen** sigue con **`Result<T, String>` pinned** — el `Err`
  se coerce a String. Mejoras del codegen en Err+:
  - `Err(Int)`/`Err(Float)`/`Err(Bool)`/`Err(Null)` → `format!("{{}}",
    code)` (ya funcionaba).
  - `Err(Instance)` → `format!("{{}}", *(code).lock().unwrap())`
    deref del `Arc<Mutex<TData>>` antes del format!, porque
    `Mutex<T>` no implementa Display aunque `TData` adentro sí.
    Paridad bit-a-bit con el Display del intérprete.
  - `Err(List<T>)`/`Err(Map<K, V>)` → error claro de codegen
    citando `fitz run` como workaround (el wrap es más profundo
    y requiere helpers que no tenemos hoy).

Implementación: ~85 LoC entre evaluator (`signal_to_error`) +
codegen (`gen_err`). **6 unit tests nuevos** (4 evaluator + 2
sobre mensajes específicos) + 2 compile_e2e bit-a-bit
(`Err(Int)` y `Err(Instance)` con Display). Ejemplo
`examples/guide/14b-errores-tipados.fitz` sumado al smoke
`GUIDE_EXAMPLES_COMPILE`. Cap 14 de la guía suma dos sub-secciones:
"Err con tipos custom (mini-tanda Err+)" y "`?` fuera de fn —
mensaje propio".

**Deuda residual** (NO bloquea próximas tandas):
- ~~**`Result<T, E>` con E tipado en codegen**~~ ✓ CERRADO
  2026-05-18 (mini-tanda Re+). Ver entrada dedicada a continuación.
- ~~**`Err(List<T>)`/`Err(Map<K, V>)` en codegen**~~ ✓ CERRADO
  2026-05-18 (mini-tanda El). Post-Re+ el codegen ya emite
  `Err(<code>)` directo con el E tipado; solo faltaba quitar el
  guard de `gen_err` que rechazaba List/Map explícitamente. La
  match arm de tipos aceptados sumó `Type::List(_) | Type::Map(_,
  _)` (paralelo a los primitivos + nominal). El value se preserva
  como `Arc<Mutex<Vec<U>>>` (List) o `Arc<Mutex<Vec<(K, V)>>>`
  (Map); el binding `Err(e)` tipa con el E real, y métodos
  `.len()`, `.get(k)`, indexing, etc. funcionan sobre el value.
  Print del `Err(<list>)`/`Err(<map>)` ya pasaba por `show_expr`
  recursivo (que maneja Result/List/Map nativamente), bit-a-bit
  con el evaluator. 2 unit nuevos en codegen + 4 compile_e2e
  (List preserva value, List print directo, propagación con `?`,
  Map preserva value). Ejemplo
  `examples/guide/14d-err-compuestos.fitz` sumado al smoke
  `GUIDE_EXAMPLES_COMPILE`. Cap 14 sub-sección nueva "Err con
  tipos compuestos: List y Map (mini-tanda El)".

### ~~`Result<T, E>` con E tipado~~ ✓ CERRADO 2026-05-18 (mini-tanda Re+)

Refactor del shape `Type::Result(Box<Type>)` → `Type::Result {
ok: Box<Type>, err: Box<Type> }`. Cierra la deuda residual más
visible de Err+: el binding `Err(e)` ahora tipa con el E real,
así que **acceder a fields del Err funciona end-to-end** (`fitz
run` Y `fitz build`).

**Sintaxis source**:
- `Result<T>` (1 arg) — default `err = Str`, compat con código
  existente.
- `Result<T, E>` (2 args) — E explícito.

**Inferencia del checker**:
- `Ok(x)` → `Result { ok: type(x), err: Any }`.
- `Err(e)` → `Result { ok: Any, err: type(e) }`.
- LUB de Results recursivo en ambos lados.
- `is_compatible` para Results requiere compat en ok Y err.

**Pattern `Err(e)`**: el binding `e` ahora extrae el err del
scrutinee `Result { ok, err }`. Hereda el E concreto (Int,
Instance, Tuple) o el default Str cuando es un Result legacy.

**Codegen**:
- `rust_type_for(Type::Result { ok, err })` → `Result<T_rust,
  E_rust>` con E real (default Str cuando E es Str default,
  `_` cuando es Any para inferencia rustc).
- `gen_err(value)`: ya NO coerce a String. Emite `Err(<code>)`
  directo con el tipo Rust real. Tipo Fitz sintetizado:
  `Result { ok: Any, err: type(value) }`.
- `pattern_to_arm` para `Pattern::ErrBinding`: extrae el `err`
  del `scrut_ty` y bindea con ese tipo en lugar de Str hardcoded.

**Display de `Type::Result`** omite el E cuando es Str (default)
o Any para no contaminar mensajes con `Result<T, Str>` redundante.

Implementación: ~80 LoC entre types (`Type::Result` enum +
inferencia + is_compatible + lub + display + pattern bind) +
codegen (rust_type_for + gen_err + pattern_to_arm). Refactor
mecánico de ~25+ sitios donde se construía/destructuraba
`Type::Result(t)` — todos cubiertos con sed regex automáticos.
**8 unit tests nuevos** (5 checker — anotación explícita,
binding inferido, legacy compat, aridad inválida, display
condicional) + 3 compile_e2e bit-a-bit (caso canónico `Err(e)
=> e.status`, `Err(Int)` con binding Int, legacy `Result<T>`).
Ejemplo `examples/guide/14c-result-tipado.fitz` sumado al smoke
`GUIDE_EXAMPLES_COMPILE`. Cap 14 de la guía suma sub-sección
"`Result<T, E>` con E tipado (mini-tanda Re+)".

**Deuda residual menor** (NO bloquea):
- ~~`Err(List<T>)`/`Err(Map<K, V>)`~~ ✓ CERRADO 2026-05-18
  (mini-tanda El). Ver entrada de El en la sub-sección anterior.

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

- ~~**F1**: documentar matriz cobertura de `Type::Any`~~ ✓ CERRADO
  2026-05-18 (mini-tanda Ex). `Type::Any` aparece en estos casos
  del checker:
  - Variables sin anotación cuyo RHS tipa Any (típicamente
    `let x = foo.something_imported()` donde el módulo importado
    no declara la fn).
  - Args/return de fns sin anotación (deuda 5b.1 — inferencia
    completa pendiente).
  - List/Map literales vacíos sin contexto: `let xs = []` →
    `List<Any>` hasta que un binding tipado lo restrinja.
  - Callbacks de `map`/`filter`/`find` cuando el FnExpr inline no
    declara `return_type` (el wrapper infiere via dry-run pero
    cae a Any si el body es ambiguo).
  - Identificadores no resueltos en runtime (escape gradual; el
    evaluator emite error real).
  - Lados de `BinOp`/`Index`/`Field` cuando el receptor es Any
    — propaga a Any sin chequeo (gradual).
  - Imports sin anotación: `from foo import bar` donde `bar` es
    una fn del módulo cuyo retorno no está declarado.
  - **Distinto de** `Type::PyAny`: ese marca valores que vienen
    del bridge Python (8.4); tiene reglas propias (call envuelve
    en Result<Any>, field access devuelve PyAny opaco).
  - **Distinto de** `Type::Nullable(T)`: `T?` es "T o Null", NO
    es Any. El checker mantiene la unión real.
  Esta lista está auditada — cualquier caso nuevo donde aparezca
  `Type::Any` desde 2026-05-18 debe sumarse acá. Política: Any se
  usa como escape gradual deliberado, no como fallback silencioso
  por bug del checker.
- ~~**F5**: `is_async` en `FnDef`~~ ✓ CERRADO 2026-05-18 (mini-tanda
  Ex, verificación). El field `FnDef.is_async: bool` existe en
  AST desde Fase 6, está cableado al evaluator (despacho de
  `Value::Function { is_async }` en `eval_call` + `Value::Future`
  perezoso), al codegen (`gen_top_fn` emite `pub async fn` y
  ajusta el ret type a `Pin<Box<dyn Future>>`), y al checker
  (`await_stack` en `CheckCtx`, validado por `Expr::Await`). F17
  cerró el ciclo eliminando el bridge HTTP sync/async (`Rc<RefCell>`
  → `Arc<Mutex>`, futures `+ Send`). Nada residual; cerrado por
  cobertura cruzada de Fase 6 + F17.
  Verificar y marcar cerrado en deudas si aplica.
- **F13**: heterogéneos `[1, "dos", true]` en `fitz build` — `FitzValue`
  tagged runtime. Decisión grande.
- ~~**F14**: `let X = <expr>` no-literal a nivel top-level del
  módulo~~ ✓ CERRADO 2026-05-18 (mini-tanda F14). El codegen ahora
  acepta RHS arbitrarias y elige el shape Rust según `is_const_eval_expr`:
  reducible a const (literales + BinOp/UnaryOp aritmético/lógico/bit
  sobre operands const-eval recursivos) → `pub const X: T = <rhs>`;
  caso contrario → accessor function `pub fn X() -> T { <rhs> }` con
  el call site emitiendo `mod::X()` en lugar de `mod::X`. Decisión:
  no propagamos const-ness entre `let`s del módulo — una RHS que
  referencia otra const del mismo módulo cae al camino accessor por
  simplicidad (invisible para el usuario). Cobertura: `LoadedModule`
  + `LoadedModuleSigs` + `CodegenCtx` sumaron `accessor_consts:
  HashSet<String>`; `gen_module_top_let` reescrito con dos caminos;
  `resolve_namespace_field` y `gen_expr Ident` chequean
  `accessor_consts` para decidir `X` vs `X()`. Tests: 3 unit nuevos
  (`modulo_top_level_acepta_expr_const_eval_como_pub_const`,
  `modulo_top_level_acepta_expr_no_const_como_pub_fn`,
  `modulo_top_level_str_concat_se_emite_como_pub_fn`) reemplazando
  el viejo `modulo_top_level_no_acepta_expr_compleja`. 3 E2E nuevos
  (`f14_modulo_let_const_eval_compila_y_devuelve_valor_inlineado`,
  `f14_modulo_let_runtime_str_concat_compila`,
  `f14_modulo_let_runtime_struct_lit_via_fn_call`). Ejemplo runnable
  `examples/guide/16b-modulos-let-expr.fitz` +
  `module_let_expr_utils.fitz` sumado al smoke
  `GUIDE_EXAMPLES_COMPILE`. Cap 16 de la guía sumó sub-sección
  "Constantes del módulo con RHS calculada"; bullet stale en "Qué
  no se puede hacer todavía" removido. Validado bit-a-bit `fitz
  run` ↔ `fitz build`.
- ~~**F15**: imports transitivos en codegen — un módulo cargado puede
  tener su propio `import`~~ ✓ CERRADO 2026-05-18 (mini-tanda F15).
  El `ModuleLoader` del codegen ahora hace load recursivo + detección
  de ciclos paralelo al evaluator. Cambios principales:
  `ModuleLoader` suma `loading_stack: Vec<PathBuf>` que pushea
  canonical path antes de procesar cada módulo (cycle detect con
  mismo mensaje del evaluator: `"ciclo de imports detectado: a -> b
  -> a"`); `load_module` divide en `load_module` (cycle guard +
  push/pop) y `load_module_inner` (parse+check+recursive load +
  codegen + push to modules); `LoadedModule` suma `local_bindings:
  HashMap<String, ResolvedBinding>` que captura los bindings
  transitivos del módulo (Namespace/Named); nueva fn
  `generate_module_rs_with_bindings` reemplaza `generate_module_rs`,
  instala firmas + bindings en el `CodegenCtx` ANTES del pre-registro
  (para que pre_register_top_lets pueda resolver tipos cross-module).
  `CodegenCtx::mod_path_prefix()` devuelve `"crate::"` en Module mode
  y `""` en Main mode; usado en `resolve_namespace_field`,
  `resolve_namespace_call`, y el call a `__default_<T>_<F>()` para
  defaults de tipos importados. Nuevo método
  `CodegenCtx::emit_module_use_decls` paralelo a
  `ModuleLoader::emit_use_decls` pero con `use crate::<other>::...`.
  `Stmt::Import`/`Stmt::FromImport` ahora se ignoran adentro del
  loop de partición en `generate_module_rs_with_bindings` (ya
  procesados por el loader). Imports Python dentro de módulos
  transitivos NO se soportan en F15 (error explícito sugerendo
  workaround) — deuda residual menor. Tests: 1 unit nuevo
  (`f15_module_loader_acepta_imports_transitivos_en_modulo`),
  3 E2E nuevos (`f15_import_transitivo_namespace_y_named_mixto`,
  `f15_ciclo_de_imports_transitivos_aborta_con_error_claro`,
  `f15_import_transitivo_con_type_compartido`). El viejo E2E
  `modulo_con_import_propio_es_error_transitivo` se reapuntó a
  `modulo_con_import_propio_compila_via_import_transitivo` (test
  positivo). Ejemplo runnable `examples/guide/16c-modulos-transitivos.fitz`
  + tres archivos auxiliares (`transitivos_app/_models/_format.fitz`)
  sumado al smoke `GUIDE_EXAMPLES_COMPILE`. Cap 16 de la guía sumó
  sub-sección "Imports transitivos" + bullet stale removido de
  "Qué no se puede hacer todavía". Validado bit-a-bit `fitz run` ↔
  `fitz build`.

### ~~Tooling — VSCode catch-up~~ ✓ CERRADO 2026-05-17 (mini-tanda V)

El usuario marcó como política que la extensión VSCode siempre
debe estar sincronizada con cada feature nuevo del lenguaje. La
mini-tanda V salda el gap acumulado tras las mini-tandas R, S,
I, T, L sobre el grammar TextMate y el LSP autocomplete.

- **V.1 — Grammar TextMate** (`editors/vscode/syntaxes/fitz.tmLanguage.json`):
  - Keyword `not` sumado a `keyword.operator.logical.fitz`
    (R.1.1).
  - Strings multilínea `"""..."""` con interpolación recursiva
    como pattern dedicado `strings-triple` colocado ANTES de
    `strings` para que matchee primero (R.1.5).
  - Labels de loops (`'name` y `'name:`) como
    `entity.name.label.fitz` (L.2). Sin esto el apóstrofe
    quedaba como token desconocido y rompía el highlighting
    del resto de la línea.
  - Operadores compuestos `+=`/`-=`/`*=`/`/=`/`%=` (R.2.3) y
    rangos inclusivos `..=` (R.1.4) como patterns dedicados,
    colocados antes que `=` y `..` para que el regex no se
    quede con la primera parte.
  - Or-pattern `|` en match arms (R.2.1) como
    `keyword.operator.alternative.fitz`, posicionado después
    del `||` lógico para que dos pipes consecutivos sigan
    matcheando como un solo operator.
  - JSON validado con `ConvertFrom-Json` (smoke estructural).

- **V.1.bis — Assertion builtins en el grammar** (gap
  detectado al releer post-V.4): los 4 builtins de testing
  (`assert`, `assert_eq`, `assert_ne`, `assert_throws`)
  introducidos en la mini-fase 9.z.2 NO estaban marcados
  como `support.function.builtin.fitz` en el grammar — solo
  estaban en el LSP autocomplete (`scope_level_completions`).
  Sumados a la regex de `builtins` para consistencia visual
  con `print`/`len`/`sleep`/`cors`.

- **V.2 — LSP autocomplete** (`src/lsp.rs::after_dot_completions`):
  - **Str sumó 7 métodos** (mini-tanda S.1+S.2): `contains`,
    `starts_with`, `ends_with`, `split`, `trim`, `replace`,
    `repeat`. Quedó en 10 totales con los 3 originales
    (`upper`/`lower`/`len`).
  - **List sumó 3 métodos** (mini-tanda S.3): `sort`,
    `reverse`, `contains`. Quedó en 9 totales.
  - **Tuple field access** (mini-tanda T.1): case nuevo
    `Type::Tuple(items)` que devuelve labels numéricos
    `0`/`1`/`2`... como `CompletionItemKind::FIELD` con
    `detail` = tipo del elemento. Estilo rust-analyzer
    (label sin punto — VSCode ya consumió el `.`).
  - 3 unit tests nuevos en `lsp::tests`:
    `after_dot_sobre_str_incluye_metodos_de_mini_tanda_s`,
    `after_dot_sobre_list_incluye_sort_reverse_y_contains`,
    `after_dot_sobre_tuple_lista_indices_numericos_con_tipo`.

- **V.3 — Build del `.vsix` + smoke manual**: `npm run
  build:vsix` produce `fitz-language-win32-x64-0.9.2.vsix`
  (~1.53 MB) con el binario `fitz-lsp.exe` (rebuild en
  release, 3.47 MB) y la grammar nuevos bundleados. Smoke
  manual del autor confirma el highlighting + autocomplete
  sobre un archivo `.fitz` con los features nuevos.

- **V.4 — Cierre formal**: esta entrada + refresh del cap 22
  de la guía (conteos de métodos actualizados: Str 3→10,
  List 6→9; mención de tuple field access en autocomplete;
  mención de labels, multilínea, ops compuestos y rangos
  inclusivos en la lista del syntax highlighting).

- **V.5 — Métodos custom sobre `type` en autocomplete** (R.3
  en LSP): el case `Type::Nominal` de `after_dot_completions`
  ahora lista fields **+ métodos custom** del type. Aprovecha
  `NominalInfo.methods: Vec<NominalMethod>` que el checker
  R.3 ya populaba en una tercera vuelta sobre el TypeEnv
  (`types.rs::set_methods`). El item se emite como
  `CompletionItemKind::METHOD` con `detail` = firma
  `fn(T1, T2) -> Ret` (o `async fn(...) -> Ret` cuando
  `is_async`). Limitación heredada: `NominalMethod` guarda
  solo tipos de params (no nombres), así que la firma muestra
  `fn(Int) -> Float` y no `fn(x: Int) -> Float` —
  trade-off consistente con cómo Map/List exponen signatures.
  1 unit test nuevo: `after_dot_sobre_nominal_incluye_metodos_custom_r3`
  (cubre fn sin args, fn con args, async fn — los 3 casos
  ejemplares de R.3). Suite del LSP queda en **40 unit**
  (era 36 al cerrar V.4 = 36 + 3 V.2 + 1 V.5) + 5 E2E.

- **V.6 — Re-build + cierre formal definitivo**: rebuild del
  `.vsix` con los cambios de V.1.bis + V.5 bundleados;
  refresh del cap 22 mencionando que los métodos custom
  (R.3) ahora aparecen en autocomplete.

**Decisiones tomadas al arrancar**: (a) sin bump de versión
de la extensión — el usuario lo hace cuando publique al
Marketplace; (b) tuple labels numéricos crudos sin "campo X"
extra en el detail (consistencia con rust-analyzer); (c)
firmas de métodos custom muestran solo tipos de params (no
nombres) — trade-off consistente con cómo se exponen las
signatures de Map/List/Str.

**Deuda residual visible** (NO bloquea próximas mini-tandas;
encaje en mini-tanda futura tipo "Sp" sin compromiso):
- **Range exacto en respuestas Hover/Definition** sigue
  dependiendo de `end_span` en el AST (deuda S1 heredada
  del LSP MVP). ~6-10h cuando aparezca demanda.
- **Scope-aware autocomplete** (vars locales y params
  visibles según posición del cursor) sigue como deuda
  del LSP. Refactor del checker para persistir scope-table
  por posición. ~4-6h.
- **Cross-module go-to-definition** sigue apuntando al
  `Stmt::Import` local en vez del símbolo remoto. Deuda
  documentada desde 9.x.3.
- **Highlighting de `0..=10`**: el `=` del `..=` no se
  distingue visualmente del `=` de asignación. Aceptable
  como trade-off del grammar.
- ~~**Firmas de params con nombres** en autocomplete de
  métodos custom (`fn(x: Int)` vs `fn(Int)`)~~ ✓ CERRADO
  2026-05-18 (mini-tanda Up). `NominalMethod` ahora incluye
  `param_names: Vec<String>` paralelo a `params`, populado en
  `resolve_program`. LSP `after_dot_completions` combina ambos
  vectores para producir `fn(x: Int, y: Int) -> R` en el detail
  del item.

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
