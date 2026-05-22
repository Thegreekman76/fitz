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

### ~~RP + MP + roadmap/architecture refresh — paridad HTTP + docs al día~~ ✓ CERRADO 2026-05-20 (mini-tanda RP + MP + Docs refresh)

Mega-bundle final pre-9.w. Cierra las dos asimétrias HTTP restantes
+ refresh masivo de roadmap y architecture docs.

**Parte 1 — RP: Result<T> + post mws codegen**:

- ~~**`-> Result<T>` con post middlewares ahora compila en
  `fitz build`**~~ ✓ El path Result construye un `__resp:
  __FitzResponse` intermedio via match Ok/Err, corre los post mws
  en reverse order, y convierte a axum Response al final. Cubre
  los mismos casos que el path Result sin post-mws:
  Ok → 200 + body, Err con `status` field → status validado +
  body, Err sin status field → 500 + `{"error": e}`, status fuera
  de rango → 500 + msg claro.
- **End-to-end paridad** ✓ Smoke manual: `fn maybe(n: Int) ->
  Result<Str> { if n>0 return Ok("pos") else return Err("neg") }`
  con post-mw `wrap` que devuelve `{"wrapped":"yes","method":req.method}`
  → ambos Ok y Err cases retornan la response wrappeada
  bit-a-bit en `fitz run` y `fitz build`.

**Parte 2 — MP: urlencoded bodies**:

- ~~**`application/x-www-form-urlencoded` body parsing**~~ ✓ Helper
  nuevo `parse_urlencoded_body` en `http.rs` parsea
  `key1=val1&key2=val2` a un `Value::Map<Str, Str>`. URL-decoding
  aplicado a keys y valores (`+` → espacio, `%XX` → byte hex).
  Duplicados: last-wins (paralelo a `serde_urlencoded`). Body
  vacío → Map vacío.
- ~~**Content-Type 415 actualizado**~~ ✓ Acepta JSON, urlencoded,
  o body sin Content-Type. Otros formatos (multipart, text/plain,
  etc.) → 415 con mensaje claro citando ambos formatos soportados
  y multipart como sub-paso futuro.
- **Codegen**: NO soporta urlencoded todavía. `axum::Json` rechaza
  con 415 default antes de llegar al handler. Deuda residual menor
  (la mayoría de los handlers JSON funcionan; urlencoded en codegen
  requiere swap del extractor a Bytes + manual parse).
- **Multipart con files**: sigue diferido. Requiere infrastructure
  adicional (manejo de file uploads, boundaries, parts). Sub-paso
  futuro dedicado (~4-6h) si entra demanda real.

**Parte 3 — Roadmap refresh masivo**:

- `docs/roadmap.md` sumó sección final "Mini-tandas post-Fase 8 —
  polish del lenguaje base (2026-05-17 → 2026-05-20)" con tabla
  cronológica de las ~25 mini-tandas cerradas en la serie:
  R.1/R.2/R.3, S/I/T/L, Md/It/Ex/Up/Mb-series, C/Fm/Err+/Re+,
  Bits/Cmp/Xor/Núm/Lit/F8/F9, Mln/F14/F15/F16, Rt/Lt, Math+Mb9,
  Fp+Sp/Fp.2/Fp.3/Sp.2, Vp/Vm/St/CM/Cd, HC.1/HC.2, LSPx/LSPy,
  Hpx.1/Hpx.2, Mw.next/5b.1/P2, P1, RP, MP. Stats al cierre:
  1983 unit sin feature, 2073 con `--features lsp`, 233+ compile_e2e.

**Parte 4 — architecture.md refresh**:

- Sumadas referencias a `MiddlewareKind::{Pre, Post}`,
  `parse_urlencoded_body`, `infer_return_type_from_body`,
  `infer_param_type_from_call_sites`, `has_unannotated_fn_params`,
  `fill_inferred_param_types`, Mw.next codegen helpers
  (`mw_user_fns_post`, `middleware_post_fn_names`,
  `emit_handler_dispatch_and_response` con post-chain). El doc
  ahora refleja las APIs públicas/privadas relevantes para
  contribuyentes del compilador.

**Implementación cross-cutting**:

- **`http.rs`**: helpers `parse_urlencoded_body` y `url_decode`.
  Content-Type matching actualizado en `handle_task`. 4 unit tests
  nuevos en `mp_*`.
- **`codegen.rs`**: branch `sig.returns_result && has_post_mws`
  construye `__resp: __FitzResponse` via match con todos los casos
  de Err (status field con validación, sin status field, fuera
  de rango), corre post-mws en reverse, convierte a axum Response.
- **`docs/roadmap.md`**: +~80 LoC en sección nueva con tabla
  cronológica.
- **`docs/architecture.md`**: +~40 LoC en módulo HTTP + codegen
  con las nuevas APIs.

**VSCode extension**: SIN cambios. Cambios server-side.

**Tests**: **4 unit nuevos** (`mp_urlencoded_basico_parsea_a_map`,
`mp_urlencoded_con_url_encoding`, `mp_urlencoded_body_vacio_es_map_vacio`,
`mp_multipart_sigue_rechazado_con_415`). Smoke manual del codegen
RP validó Ok+Err cases bit-a-bit `fitz run` ↔ `fitz build`.

**Total al cierre: 1983 sin feature, 2073 con --features lsp.**
Clippy `-D warnings` limpio en lib + bin `fitz-lsp`.

**Deuda residual (NO bloquea 9.w):**

- **Multipart/form-data con files**: sub-paso futuro dedicado
  (~4-6h). Requiere parser de multipart + decisión sobre file
  storage.
- ~~**urlencoded en codegen**: hoy solo `fitz run`~~ ✓ CERRADO
  2026-05-20 en mini-tanda UC + HA (ver entrada propia más abajo).
- **Wrap-style `next` callable middleware**: sigue diferido (~6-8h).

### ~~F13.C+D+E + OAPI Expr + File.content Bytes — cierre del bloque post-Fase-8~~ ✓ CERRADO 2026-05-20 (mini-tanda Final-Bundle)

**Bundle final ambicioso** que cierra **5 deudas restantes** del
bloque post-Fase-8 en una sola sesión. La única deuda que queda
visible es **Mw-Wrap codegen** — diferida con design + scope claro
porque requiere ~2-3h de trabajo invasivo sobre tipos async/Send/Sync
recursivos en Rust.

**Parte 1 — F13.E: List/Map anidados con mix interno**:

- ~~**`__FitzValue::List(Vec<__FitzValue>)`** y `Map(Vec<(FV, FV)>)`~~
  ✓ Variantes recursivas en el enum. Display recursivo + PartialEq
  byte a byte. Habilita `[1, [2, 3], "hola"]` y
  `[{"a": 1}, 42, [true, false]]` compilando bit-a-bit con
  `fitz run`.
- ~~**`wrap_as_fitz_value(_, Type::List | Type::Map)`**~~ ✓
  Recursión sobre el inner type. Bind del `Arc` antes del `.lock()`
  para extender el temporal (paralelo al patrón de show_expr).
- ~~**`Type::Any` como item de heterogéneo**~~ ✓ Passthrough sin
  re-wrap (el item ya es FitzValue de una lista anidada que ya
  disparó FitzValue).

**Parte 2 — F13.D: Method dispatch dinámico sobre Any/FitzValue**:

- ~~**`.as_int()` / `.as_float()` / `.as_str()` / `.as_bool()` /
  `.as_bytes()` / `.type_name()`**~~ ✓ Methods universales sobre
  cualquier `Value` (intérprete) y sobre `Type::Any` (codegen, que
  dispatcha por variant de FitzValue). Devuelven `Result<T>` con
  `Ok(v)` si match, `Err(Str)` si no. `type_name()` devuelve `Str`
  directo. Paridad bit-a-bit run↔build (incluso los mensajes de
  Err).
- ~~**Helper `__fv_type_name(v)` en el preludio**~~ ✓ Devuelve el
  nombre del tipo de un FitzValue como `&'static str`. Usado por
  los métodos `as_*` para mensajes de error consistentes.
- **Checker extendido**: `infer_method_call` sobre `Type::Any` ahora
  reconoce los 6 métodos universales con sus signatures explícitas;
  otros métodos siguen gradual (None).

**Parte 3 — F13.C: HTTP body heterogéneo**:

- ~~**`impl __FromFitzJson for __FitzValue`** en el preludio HTTP~~ ✓
  Dispatch por shape JSON: Null/Bool/Number(Int o Float)/String/
  Array (List)/Object (Map). Habilita `body: List<Any>` y
  `body: Map<Str, Any>` deserializando desde JSON entrante.
- ~~**`impl __ToFitzJson for __FitzValue`**~~ ✓ Simétrico para
  serializar. Bytes como base64 string; Nominal como string.
- ~~**`Type::Any` aceptado en `resolve_named` del checker**~~ ✓ El
  usuario ahora puede anotar `List<Any>` / `Map<Str, Any>` en
  parámetros de handlers (antes era "tipo desconocido Any").
- ~~**Pre-walker `program_uses_fitz_value` extendido**~~ ✓ Detecta
  `Any` adentro de anotaciones de tipo en `FnDef.params` y
  `FnDef.return_type` (no solo en literales de listas).
- **Smoke E2E**: handler `@post("/echo") fn echo(body: List<Any>)`
  con `body[0].as_int()` valida int en posición 0; curl con
  `[42, "hola", true]` → "first int: 42"; con `["nope", ...]` →
  "first not int". Paridad bit-a-bit con intérprete.

**Parte 4 — OAPI-Expr: status codes con expresiones simples**:

- ~~**`resolve_status_value` extendido**~~ ✓ Acepta `Expr::Int`,
  `Expr::Ident` (lookup en tabla), `UnaryOp::Neg`, y `BinOp` con
  Add/Sub/Mul. Const-eval recursivo. División/módulo evitados
  por simplicidad (división por 0). Overflow detectado via
  `checked_*` ops → None.
- ~~**`collect_top_level_int_consts` extendido**~~ ✓ Reusa
  `resolve_status_value` con walk en orden. Una const puede
  referenciar consts previas: `let BASE = 400; let NOT_FOUND =
  BASE + 4` resuelve NOT_FOUND a 404.
- **Test viejo actualizado**: `oapi_collect_top_level_int_consts_recolecta_lets_int`
  ahora espera que `SUM = 1+2` resuelva a 3 (antes era None).
- Habilita patrones del estilo `let TENANT_ERROR = BASE + 50` para
  status codes legibles.

**Parte 5 — File.content como Bytes**:

- ~~**`File.content` cambia de `Type::Str` a `Type::Bytes`**~~ ✓
  Habilita uploads binarios (imágenes, PDFs, zips) end-to-end.
  Para texto UTF-8, el usuario llama `f.content.to_str() ->
  Result<Str>`. Text fields (sin `filename=`) siguen exigiendo
  UTF-8 — para bytes binarios usar `filename=` para que se
  clasifique como file.
- ~~**`parse_multipart_body` refactor a raw bytes**~~ ✓ Antes
  hacía `from_utf8` sobre todo el body (rechazaba binarios).
  Ahora trabaja byte-por-byte: helpers `split_bytes_by`,
  `find_bytes`, `strip_prefix_bytes`, `strip_suffix_bytes`.
  Headers se parsean como ASCII (per RFC 7578). Content de file
  fields se guarda como `Value::Bytes(Vec<u8>)` sin requerir
  UTF-8.
- ~~**`FileData` en codegen**~~ ✓ `content: Vec<u8>`. Display
  delega a `__fitz_fmt_bytes`. `__ToFitzJson` serializa
  `content` como base64 string. `__FromFitzJson` acepta tanto
  base64 string como array de Int (round-trip legacy).
- ~~**Helpers `b64_encode_for_file` / `b64_decode_for_file`**~~
  ✓ Inline en codegen, sin deps externas.
- Tests viejos actualizados:
  `mp2_parse_multipart_file_field_construye_instance_file` verifica
  `Value::Bytes`; `mp2_parse_multipart_binary_no_utf8_es_error` →
  `mp2_parse_multipart_binary_file_field_funciona` (binary FILE
  field ahora funciona); test nuevo
  `mp2_parse_multipart_text_field_sin_filename_sigue_exigiendo_utf8`
  documenta la regla para text fields.

**Mw-Wrap codegen — DEFERIDO con design explícito**:

La única deuda residual visible que queda. Por qué se difiere:
- Requiere emit de cierre Rust con tipos `Arc<dyn Fn() -> Pin<Box<dyn Future<Output = __FitzResponse> + Send>> + Send + Sync>` para el `next` callable.
- Necesita captura recursiva: cada wrap nivel-N construye un
  callable que llama al nivel N+1, hasta llegar al handler + post
  chain.
- Implementación realista: emit recursivo (helper `async fn
  __wrap_chain_<route>(...)`) o cadena inline con clones de
  `Arc<dyn Fn>`.
- Tiempo estimado real: ~2-3h dedicados con tests.
- Sin presión real hoy: `fitz run` ya cubre wrap-style end-to-end;
  el codegen rechaza con msg claro citando `fitz run` como
  workaround. Mini-tanda dedicada futura cuando aparezca demanda.

**Implementación cross-cutting** (~700 LoC):

- **`src/codegen.rs`**: `__FitzValue` con 9 variantes (recursivas
  para List/Map). `__fv_type_name` helper. `wrap_as_fitz_value`
  cubre List/Map/Bytes/Nominal. `__FromFitzJson`/`__ToFitzJson`
  para FitzValue (gating: solo con HTTP). Type::Any como receiver
  de los 6 methods universales. `FileData.content: Vec<u8>` +
  base64 inline.
- **`src/http.rs`**: `parse_multipart_body` refactor a raw bytes.
  4 helpers de slice manipulation.
- **`src/evaluator.rs`**: 6 methods `fv_*` universales sobre Value.
  Dispatch en `dispatch_method` con tupla `(_, "as_int")` que
  matchea cualquier Value.
- **`src/types.rs`**: `File.content` → `Type::Bytes`. `Type::Any`
  como receiver en `infer_method_call` con 6 ramas para los
  methods universales. `"Any"` reconocido en `resolve_named`.
- **`src/openapi.rs`**: `resolve_status_value` con const-eval
  recursivo (BinOp + UnaryOp). `collect_top_level_int_consts`
  usa la nueva fn con walk en orden.

**Tests**: **~15 unit + 3 compile_e2e nuevos** (acumulados con los
de F13.A+B): cobertura del nuevo enum, methods, HTTP body,
const-eval, multipart binario. Test viejo de "no_utf8_es_error"
reemplazado por "binary_file_field_funciona". Test viejo de
"map_literal_valores_heterogeneos_es_error" reemplazado por
"emite_fitz_value". Test viejo de `SUM = 1+2 → None` ahora espera
Some(3).

**Cap 20 de la guía** actualizado: el bullet de heterogéneos
ahora dice "solo falta Functions y Tuples". Bloque "sí anda y
antes era deuda" es masivo, suma F13.C/D/E + OAPI-Expr +
File.content Bytes. La nueva deuda visible es Mw-Wrap codegen.

**Total al cierre: 2045 unit sin feature, ~2135 con --features
lsp, 250+ compile_e2e.** Clippy `-D warnings` limpio en lib + bin
`fitz-lsp`.

**Estado final del bloque post-Fase-8**: 95%+ del lenguaje
compila a binario nativo con paridad bit-a-bit. Solo wrap-style
middleware queda en `fitz run`-only por la complejidad técnica
del codegen (no por design — el diseño está claro).

**Próximo norte estratégico**: **Fase 9.w** (Stack web first-class).
Mw-Wrap codegen quedará como mini-tanda dedicada cuando entre
demanda real.

### ~~F13.A + F13.B + base64 JSON: Bytes/Nominales/Map heterogéneo + Bytes JSON estándar~~ ✓ CERRADO 2026-05-20 (mini-tanda F13.A + F13.B + base64)

Bundle de expansión del SPIKE de F13 + quick win. Cierra el caso
**90% del lenguaje** para heterogéneos en `fitz build` y reemplaza
una representación JSON no-estándar de Bytes por base64.

**Parte 1 — F13.A.1: Bytes en heterogéneos**:

- ~~**`__FitzValue::Bytes(Vec<u8>)`** nueva variante~~ ✓ Display
  delega a `__fitz_fmt_bytes` (helper ya existente desde
  mini-tanda Bytes). PartialEq byte a byte.
- ~~**`wrap_as_fitz_value(_, Type::Bytes)`**~~ ✓ Emite
  `__FitzValue::Bytes(code)` (el `code` ya es `Vec<u8>` por
  `rust_type_for(Bytes)`).
- Habilita `[1, b"raw", "hola"]` compilando bit-a-bit con
  `fitz run`.

**Parte 2 — F13.A.2: Map heterogéneo**:

- ~~**`rust_type_for(Type::Map(Any, _))`** o `Map(_, Any)`~~ ✓
  Mapea a `Arc<Mutex<Vec<(__FitzValue, __FitzValue)>>>`. Mapas
  homogéneos siguen sin overhead.
- ~~**`gen_map_lit` con sticky bit**~~ ✓ Paralelo a `gen_list_lit`:
  una vez que lub de keys o values falla, lockea `Type::Any`.
  Cuando AL MENOS UNO es Any, wrapea AMBOS lados como FitzValue
  (uniforma el tipo Rust).
- ~~**Pre-walker extendido**~~ ✓ `map_is_heterogeneous(pairs)`
  detecta keys o values con tipos AST distintos.
- Habilita `{"name": "fitz", "count": 7, "on": true}` y
  `{1: "uno", "dos": "dos"}` compilando.

**Parte 3 — F13.B: Nominales en heterogéneos**:

- ~~**`__FitzValue::Nominal(String)`** nueva variante~~ ✓ Captura
  el Display del instance como String. Display de la variant
  imprime el String directamente.
- ~~**`wrap_as_fitz_value(_, Type::Nominal(_))`**~~ ✓ Emite
  `__FitzValue::Nominal(format!("{}", &*({}).lock().unwrap()))`,
  que invoca el `Display for FooData` ya emitido por el codegen.
- **Trade-off documentado**: pierde field access tipado en
  heterogéneos pero evita dependencia en `serde_json` (que solo se
  emite con HTTP). Alternativa más completa (Nominal con
  `serde_json::Value`) queda como deuda menor.
- Habilita `[User { id: 1, name: "ana" }, 42]` compilando.

**Parte 4 — Quick win: Bytes JSON via base64**:

- ~~**`value_to_json(Value::Bytes)` emite base64 string**~~ ✓ Antes:
  array de Int (cada byte un i64) — no-estándar, infla ~4x. Ahora:
  base64 string (RFC 4648, estándar de facto para bytes en JSON).
- ~~**Encoder inline `b64_encode_standard(bytes)`**~~ ✓ Sin dep
  externa (`base64` crate). ~25 LoC. RFC 4648 con padding `=`.

**Implementación cross-cutting**:

- **`src/codegen.rs`**: `__FitzValue` enum extendido con Bytes +
  Nominal. `wrap_as_fitz_value` cubre los tipos nuevos. `gen_map_lit`
  con sticky bit + wrap dual. `rust_type_for(Map(Any, _))` emite
  el tipo tagged. Pre-walker `map_is_heterogeneous` para
  auto-detect. `wrap_as_fitz_value_with_env` simplificada a wrapper
  que ignora env (la simplificación de Nominal con Display no
  necesita resolver el nombre desde TypeEnv).
- **`src/http.rs`**: `b64_encode_standard` helper inline.
  `value_to_json(Value::Bytes)` cambia de array a base64 string.
- **`docs/design-fitzvalue.md`**: actualizado con el progreso
  acumulado. Follow-up reducido a F13.C (HTTP body con
  heterogéneos), F13.D (method dispatch), F13.E (anidados con
  mix interno).

**Tests**: **8 unit + 2 compile_e2e nuevos**:

- 4 codegen unit: `f13_a_bytes_en_lista_heterogenea_se_envuelve`,
  `f13_a_map_heterogeneo_emite_vec_fv_fv`,
  `f13_a_mapa_homogeneo_no_emite_fitz_value`,
  `f13_b_nominal_en_lista_heterogenea_captura_display`.
- 4 http unit: `b64_encode_empty`, `b64_encode_basico` (RFC 4648
  test vectors), `b64_encode_binarios`,
  `value_to_json_bytes_emite_base64`.
- 2 compile_e2e:
  `f13_a_bytes_y_nominal_en_lista_heterogenea_paridad_run_vs_build`,
  `f13_a_map_heterogeneo_paridad_run_vs_build`.
- Test viejo `map_literal_valores_heterogeneos_es_error`
  reemplazado por `map_literal_valores_heterogeneos_emite_fitz_value`.
- Test viejo `f13_spike_lista_con_tipo_no_soportado_aborta_con_msg_claro`
  reemplazado por `f13_lista_con_tipo_complejo_aborta_con_msg_claro`
  (Bytes ya es soportado; ahora probamos con List anidada que
  sigue como follow-up).

**Cap 20 de la guía** actualizado: el bullet de "Heterogéneos con
tipos avanzados" ahora cubre primitivos + Bytes + Nominales. Lo que
todavía no compila (anidados con mix interno, Functions/Tuples en
heterogéneos) queda explícito. Bloque "sí anda y antes era deuda"
suma F13.A+F13.B + base64.

**Total al cierre: 2044 unit sin feature, ~2134 con --features lsp,
252+ compile_e2e (2 nuevos − 0 obsoletos = +2).** Clippy
`-D warnings` limpio.

**Próximo norte estratégico**: **Fase 9.w** (Stack web first-class:
`@authenticated`/`@admin`, `@ws("/chat")`, `@cron`, `@background`).
F13.C/D/E quedan como follow-ups chicos refinables si entra demanda
real para el subset cubierto por ellos.

### ~~F13 SPIKE: design doc + prototipo mínimo de `FitzValue` (listas heterogéneas con primitivos)~~ ✓ CERRADO 2026-05-20 (mini-tanda F13 SPIKE)

**ÚLTIMA deuda residual del bloque post-Fase-8**. Spike de diseño +
prototipo mínimo que cierra el caso visible más común y deja el
camino claro para el cierre completo. Después de esto, el bloque
post-Fase-8 queda CERRADO ENTERO — todos los items residuales
documentados están cerrados o tienen design doc + scope claro.

**Decisión de alcance**: en lugar de cerrar F13 entero (~15-20h con
riesgo alto de decisiones de diseño a mitad de camino), separamos
en SPIKE (~2h) + follow-up (~10-15h). El SPIKE valida el approach
con un caso real mínimo (listas heterogéneas de primitivos) y
documenta el diseño en `docs/design-fitzvalue.md`.

**Parte 1 — Design doc** (`docs/design-fitzvalue.md`):

- Shape del enum `__FitzValue`: variantes Int/Float/Str/Bool/Null
  (primitivos del SPIKE) + List/Map/Bytes/Nominal (follow-up).
- Activación auto-detectada por el checker — `Type::List(Type::Any)`
  triggerea emisión; listas homogéneas siguen emitiendo el tipo
  concreto sin overhead.
- Tradeoffs documentados: nominales via JSON intermedio (pierde
  field access tipado en heterogéneos), method dispatch sobre
  FitzValue postergado, Hash para Map keys solo primitivos.
- Roadmap del follow-up (F13.A-E, ~10-15h total): Map heterogéneo,
  Nominales, HTTP body con heterogéneos, method dispatch básico,
  docs + ejemplos + smoke.

**Parte 2 — Prototipo mínimo**:

- ~~**`CodegenCtx.uses_fitz_value: bool`**~~ ✓ Nuevo flag, gating
  paralelo a `uses_fmt_helpers`/`uses_python`. Solo se emite el
  enum cuando el flag es true.
- ~~**Pre-walker `program_uses_fitz_value(program)`**~~ ✓ Heurística
  sintáctica conservadora: walka el AST detectando `Expr::List`
  con items literales de tipos distintos (Int + Str + Bool, etc.).
  Cubre el caso canónico `[1, "dos", true]`. Listas con elementos
  calculados pueden no triggerear el preludio — limitación
  aceptada (refinable post-SPIKE con un pase del checker).
- ~~**`gen_list_lit` con sticky bit**~~ ✓ La regla existente del
  `lub` colapsa `Any + T = T`, que arruina el SPIKE. Nueva
  estructura: una vez que `lub` falla entre dos items, lockeamos
  `common_ty = Type::Any` SIN llamar lub para el resto. Cuando
  `common_ty == Any`, emite `Arc<Mutex<Vec<__FitzValue>>>` con
  cada item wrappeado vía `wrap_as_fitz_value`.
- ~~**`wrap_as_fitz_value(code, ty)`**~~ ✓ Helper que envuelve un
  literal en su variante FitzValue. Tipos no cubiertos por el
  SPIKE (Bytes, List, Map, Nominal, Function) → error claro con
  prefijo "F13 SPIKE:" + workaround `fitz run`.
- ~~**Preludio HTTP suma `enum __FitzValue`**~~ ✓ Cuando
  `uses_fitz_value = true`, emite el enum + `impl Display`
  (formato paralelo a `Value` del intérprete: strings con
  comillas adentro de listas, Float con `.0` via `__fitz_fmt_float`)
  + `impl PartialEq` (coerción Int↔Float, igualdad estructural
  por variant).
- ~~**`rust_type_for(Type::List(Type::Any))`**~~ ✓ Mapea a
  `Arc<Mutex<Vec<__FitzValue>>>` (antes: error). Listas con T
  concreto siguen como `Arc<Mutex<Vec<T>>>` (sin overhead).
- ~~**`show_expr(Type::Any)`**~~ ✓ Cuando un Type::Any llega al
  formatter (caso típico: item adentro de lista heterogénea), usa
  `format!("{}", ...)` que invoca el Display impl del `__FitzValue`.

**Tests**: **3 unit + 2 compile_e2e nuevos**:

- 3 codegen unit:
  `list_literal_heterogeneo_emite_fitz_value` (verifica wrap por
  variant), `f13_spike_preludio_emite_fitz_value_enum` (verifica
  emisión del enum + impls), `f13_spike_lista_homogenea_no_emite_fitz_value`
  (sanity: cero overhead para homogéneo).
- 2 compile_e2e:
  `f13_spike_lista_heterogenea_compila_y_paridad_bit_a_bit`
  (paridad bit-a-bit `fitz run` ↔ `fitz build` sobre listas
  mixtas con todos los primitivos), `f13_spike_lista_con_tipo_no_soportado_aborta_con_msg_claro`
  (Bytes en heterogéneo → error claro citando follow-up).
- Test viejo `list_literal_heterogeneo_es_error_homogeneo_requerido`
  removido y reemplazado por `list_literal_heterogeneo_emite_fitz_value`
  (cambio intencional del behavior).

**Cap 20 de la guía**: actualizado. El bullet "Listas/mapas
heterogéneos" se splittea en dos: (a) mapas heterogéneos siguen
como follow-up; (b) listas heterogéneas con primitivos AHORA
COMPILAN (SPIKE). Bloque "sí anda y antes era deuda" suma F13
SPIKE con link al design doc.

**Total al cierre: 2036 unit sin feature, 2126 con --features lsp,
249+ compile_e2e (2 nuevos − 0 obsoletos = +2), 75 ejemplos en
smoke (sin cambio).** Clippy `-D warnings` limpio en lib + bin
`fitz-lsp`.

**Cierre formal del bloque post-Fase-8**: con F13 SPIKE cerrada,
TODAS las deudas residuales del bloque están cerradas o tienen
design doc + scope claro:

- R-series, S, I, T, L, Md, It, Ex, Up, Mb-series, C, Fm, Err+,
  Re+, Bits, Cmp, Xor, Núm, Lit, F8, F9, Mln, F14, F15, F16, Rt,
  Lt, Math+Mb9, Fp, Sp, Vp, Vm, St, CM, Cd, HC.1, HC.2, LSPx,
  LSPy, Hpx.1, Hpx.2, Mw.next, 5b.1, P2, P1, RP, MP, UC, HA, DZ,
  CT, OAPI, MP2, MP-Build, Bytes, Mw-Wrap, F13 SPIKE — **todas
  cerradas**.
- F13 completo: design doc + roadmap del follow-up
  (`docs/design-fitzvalue.md`) — refinable cuando entre demanda
  real para el subset cubierto por el follow-up (Bytes/List/Map/
  Nominal en heterogéneos, HTTP body con heterogéneos, etc.).
- Próximo norte: **Fase 9.w** (Stack web first-class:
  `@authenticated`/`@admin`, `@ws("/chat")`, `@cron`,
  `@background`).

### ~~Mw-Wrap: wrap-style middleware con `next` callable (intérprete only)~~ ✓ CERRADO 2026-05-20 (mini-tanda Mw-Wrap)

Cierra la deuda residual del modelo wrap-style middleware. El
intérprete (`fitz run`) ahora soporta `fn mw(req: Request, next:
Fn() -> Response) -> Response`. El codegen rechaza con un msg claro
citando `fitz run` como workaround — codegen real para Wrap mws es
deuda residual menor (refinable si entra demanda). Queda **F13 como
última residual** del bloque post-Fase-8.

**Implementación en 5 capas**:

- ~~**`Value::NativeFn(NativeAsyncFn)`**~~ ✓ Nueva variante del value
  system. `NativeAsyncFn` es wrapper struct sobre
  `Arc<dyn Fn(Vec<Value>) -> FitzFuture + Send + Sync>` (no type
  alias porque necesitamos impl Debug manual). Send + Sync para
  fluir por tokio multi-thread (post-F17.4). type_name = "Function",
  Display = `<native function>`, PartialEq cae al catch-all
  (siempre false — funciones no se comparan por valor).
- ~~**`MiddlewareKind::Wrap`**~~ ✓ Tercera variante (paralela a Pre,
  Post). El chain runner la distingue.
- ~~**Classifier `classify_2_arg_middleware`**~~ ✓ Inspecciona el
  tipo del segundo param: `TypeExpr::Function { ... }` o
  `TypeExpr::Generic { name: "Fn", ... }` (con Nullable opcional)
  → Wrap. Cualquier otro tipo (incluido `Response` o sin anotación)
  → Post. Preserva la semántica histórica de Post mws sin
  anotación.
- ~~**`invoke_value` dispatch**~~ ✓ Nueva rama en
  `evaluator::invoke_value` que invoca el callback de NativeFn con
  los args provistos (típicamente 0 args para `next: Fn() ->
  Response`). El callback devuelve `FitzFuture` que se await-ea
  asíncronamente.
- ~~**`run_wrap_chain` recursivo**~~ ✓ Nuevo en `http.rs`. Caso base:
  sin wraps → invocar handler + post chain. Caso recursivo: pop
  primer wrap, construir `Value::NativeFn(next)` capturando el
  resto de la chain por clone, invocar el wrap con `(req, next)`.
  La closure de `next` re-clonea las capturas en cada invocación
  (puede llamarse 0+ veces). El return del wrap se convierte a
  HandlerOutcome.

**Integración en `handle_task`**:

- Detecta si hay wrap-style mws en la ruta. Si sí, invoca
  `run_wrap_chain` que reemplaza el flujo clásico (handler +
  post). Si no, sigue el flujo histórico (handler directo, post
  mws después).
- Pre mws siguen corriendo ANTES de todo (gate-only sin cambios).
- Wraps + Post coexisten: wraps envuelven al handler, posts siguen
  corriendo después como sub-paso del caso base de
  `run_wrap_chain`.

**Codegen — rejected con msg claro**:

`collect_route_middlewares` del codegen detecta wrap-style mws
(`fn_sigs[mw].params[1]` es `Type::Function`) y aborta con:

```
@middleware(`<mw>`) sobre fn `<handler>`: wrap-style middleware
(segundo param `Fn() -> Response`) corre solo en `fitz run` por
ahora. Codegen es deuda residual menor — refinable si entra
demanda. Para `fitz build`, usá post-process (segundo param tipo
`Response`) o pre-process (1 arg).
```

**Tests**: **4 unit + 1 e2e nuevos**:

- 4 unit en `http::tests::mw_wrap_classifier_*` (param Fn → Wrap,
  param Response → Post, sin anotación → Post, Fn nullable → Wrap).
- 1 compile_e2e `mw_wrap_codegen_rechaza_con_msg_que_cita_fitz_run`
  que valida el reject del codegen.
- Smoke manual end-to-end con curl: 3 casos (handler con timing
  mw, gate condicional sin auth = 401, gate con auth = 200).

**Ejemplo nuevo**: `examples/guide/17d-middleware-wrap.fitz`. NO se
suma al smoke `GUIDE_EXAMPLES_COMPILE` porque el codegen rechaza
(`fitz run`-only por ahora — paralelo a otros ejemplos
intencionalmente no-compilables documentados en cap 18).

**Cap 17 (HTTP)**: nueva sub-sección "Variantes" en Middleware
con tabla Pre/Post/Wrap + ejemplo de Wrap + caveat de
`fitz build`.

**Total al cierre: 2034 unit sin feature, 2124 con --features lsp,
247+ compile_e2e (1 nuevo), 75 ejemplos en smoke (sin cambio — el
nuevo es run-only).** Clippy `-D warnings` limpio.

**Deuda residual restante (NO bloquea 9.w)** — ÚLTIMA:

- **F13 — listas/mapas heterogéneos en `fitz build`**: ~15-20h.
  Decisión grande: `FitzValue` tagged runtime en codegen output.
  Probable spike de diseño primero. Última residual del bloque
  post-Fase-8 antes de empujar 9.w (Stack web first-class).

### ~~Bytes: primitivo nuevo del lenguaje + paridad run↔build + VSCode~~ ✓ CERRADO 2026-05-20 (mini-tanda Bytes)

Cierra la deuda residual `Value::Bytes` que quedó abierta desde MP2.
Sexto primitivo del lenguaje paralelo a Str/Int/Float/Bool/Null.
Paridad bit-a-bit `fitz run` ↔ `fitz build` desde el primer día.
Las 2 residuales restantes (Mw-Wrap, F13) quedan como mini-tandas
dedicadas futuras.

**Capas tocadas (7 capas + VSCode/LSP)**:

- ~~**Value system** (`src/value.rs`)~~ ✓ Nueva variante
  `Value::Bytes(Vec<u8>)`. Display formato `b"..."` paralelo a Rust:
  ASCII printable directo, escapes comunes (`\n`/`\r`/`\t`/`\\`/`\"`),
  resto como `\xHH`. PartialEq byte a byte. type_name = "Bytes".
- ~~**Type system** (`src/types.rs`)~~ ✓ Nueva variante `Type::Bytes`
  (primitivo, aridad 0). `resolve_type_expr` reconoce `"Bytes"` como
  nombre del primitivo. Method type inference en
  `infer_bytes_method`: `len() -> Int`, `is_empty() -> Bool`,
  `to_str() -> Result<Str>`. `display`/`display_type`/`type_name`
  actualizados.
- ~~**Lexer** (`src/lexer.rs`)~~ ✓ Nuevo `Token::Bytes(Vec<u8>)`.
  `read_bytes_literal()` activado cuando peek es `b` + peek_next es
  `"`. Soporta escapes `\n`/`\r`/`\t`/`\0`/`\\`/`\"`/`\xHH`.
  Chars Unicode en source se codifican como bytes UTF-8 (Rust
  rechazaría eso en `b"..."`; Fitz es más permisivo).
- ~~**AST + Parser** (`src/ast.rs`, `src/parser.rs`)~~ ✓ Nueva variante
  `Expr::Bytes(Vec<u8>, Span)`. Parser primary añade
  `Token::Bytes(bs) => Expr::Bytes(bs, span)`. Span impl actualizado.
- ~~**Evaluator** (`src/evaluator.rs`)~~ ✓ Dispatch
  `Expr::Bytes → Value::Bytes`. Métodos `bytes_len`/`bytes_is_empty`/
  `bytes_to_str` en `dispatch_method`. Builtin global `bytes(s)` con
  type `fn(Str) -> Bytes`. `len(...)` global suma rama Bytes.
- ~~**Codegen** (`src/codegen.rs`)~~ ✓ `rust_type_for(Bytes) → Vec<u8>`.
  Literal `Expr::Bytes` → `vec![<byte>u8, ...]` (caso vacío:
  `Vec::<u8>::new()` para evitar E0282). Métodos sobre `Type::Bytes`:
  `.len()` → `(_.len() as i64)`, `.is_empty()` → `_.is_empty()`,
  `.to_str()` → `String::from_utf8(_)` envuelto en
  `Result<String, String>`. Helper `__fitz_fmt_bytes` en el preludio
  (paralelo a `__fitz_fmt_float`) para Display en `print`/interp.
  `show_expr(Bytes)` delega al helper. `field_eq_expr` incluye
  Bytes en la rama de `==` directo. Builtins globales `len(Bytes)`
  y `bytes(s: Str) -> Bytes` reconocidos en el codegen.
- ~~**LSP** (`src/lsp.rs`)~~ ✓ `Bytes` agregado a built-in types
  para autocomplete. `bytes` builtin agregado al array de builtins
  con signature `fn(s: Str) -> Bytes`. Method completion para
  `Type::Bytes` agrega `len`/`is_empty`/`to_str`.
- ~~**VSCode grammar** (`editors/vscode/syntaxes/fitz.tmLanguage.json`)~~ ✓
  Nuevo pattern `bytes-literal` matchea `b"..."` con escapes hex.
  Pattern incluido ANTES de `strings` para no colisionar. `Bytes`
  agregado a `support.type.builtin`. `bytes` agregado a `builtins`
  function names. `.vsix` rebuilt (`fitz-language-win32-x64-0.9.2.vsix`,
  1.62 MB).
- ~~**fmt.rs y lint.rs**~~ ✓ Walkers actualizados para incluir
  `Expr::Bytes(...)` en las ramas catch-all de primitivos. `fmt_expr`
  emite `b"..."` con escapes paralelos al Display.

**Helper `__fitz_fmt_bytes` del codegen**: paralelo bit-a-bit al
`fmt::Display for Value::Bytes` del intérprete. ASCII printable
directo, escapes comunes, resto como `\xHH`. Garantiza paridad.

**Limitaciones explícitas (deuda residual menor)**:

- **`Bytes` como Map key**: no soportado (los Map de Fitz aceptan
  Value como key pero el codegen requiere `__MapKey` trait que solo
  está implementado para primitivos hash-amigables — String/i64/
  f64/bool). Si entra demanda, agregar `__MapKey for Vec<u8>` con
  representación hex.
- **`Bytes` heterogéneo en `b"..."`**: no aplica — el literal es
  homogéneo por definición.
- **Métodos extras** (slicing, hex_encode, base64, indexing por
  byte): no incluidos en MVP. Refinables si entra demanda real.
- **JSON serialization**: `value_to_json(Value::Bytes)` emite array
  de Int (cada byte como Int). Alternativa común (base64 string)
  queda como deuda menor.

**Integración con File de MP2**: el `File.content` sigue siendo
`Str` (UTF-8 only). Cambiar a `Bytes` permitiría files binarios en
multipart — queda como follow-up dedicado cuando aparezca demanda
real. Hoy `Value::Bytes` está disponible como tipo independiente y
el usuario puede construir/manipular bytes sin tocar el path HTTP.

**Tests**: **18 unit + 1 compile_e2e nuevos**:

- 5 unit en `value::tests::bytes_*` (display ASCII, display hex,
  display escapes, igualdad byte a byte, distinto de Str).
- 7 unit en `lexer::tests::bytes_literal_*` (ASCII básico, hex, escapes
  comunes, vacío, ident `b` sin comilla, Unicode UTF-8, escape inválido).
- 6 unit en `evaluator::tests::bytes_*` (literal evalúa, len y is_empty,
  to_str Ok, to_str Err no-UTF8, builtin `bytes(s)`, `len(b"...")` global).
- 1 compile_e2e `bytes_paridad_bit_a_bit_run_vs_build` (ejecuta ambos
  paths y compara stdout línea a línea).

**Ejemplo nuevo**: `examples/guide/05e-bytes.fitz` con literal, escapes,
métodos, constructor, comparación bit-a-bit, integración con List.
Sumado al smoke `GUIDE_EXAMPLES_COMPILE`.

**Cap 3 (Variables y tipos primitivos)**: tabla actualizada a 6
primitivos (Bytes nuevo). Nota apunta al cap 5 sub-sección para
detalle.

**Cap 5 (Strings)**: nueva sub-sección "Bytes — datos binarios" al
final con literal, escapes soportados, métodos, constructor, diff
con Str, criterios de uso, link al ejemplo runnable.

**Total al cierre: 2030 unit sin feature, 2120 con --features lsp,
246+ compile_e2e (1 nuevo), 75 ejemplos en smoke (1 nuevo).**
Clippy `-D warnings` limpio en lib + bin `fitz-lsp`.

**Deuda residual restante (NO bloquea 9.w)**:

- **Mw-Wrap (wrap-style middleware con `next` callable)**: ~6-8h.
  Requiere `Value::NativeFn` async + integración en
  `http.rs::handle_task` + codegen paralelo. Mini-tanda dedicada
  futura.
- **F13 — listas/mapas heterogéneos en `fitz build`**: ~15-20h.
  Decisión grande — `FitzValue` tagged runtime en codegen output.
  Probable spike de diseño primero. Mini-tanda dedicada futura.

### ~~MP-Build: multipart en `fitz build` (paridad bit-a-bit run↔build)~~ ✓ CERRADO 2026-05-20 (mini-tanda MP-Build)

Cierra la deuda residual del codegen que quedó abierta en MP2. El
binario nativo (`fitz build`) ahora acepta `multipart/form-data`
con el mismo comportamiento que el intérprete (`fitz run`). Mw-Wrap,
Bytes y F13 quedan como mini-tandas dedicadas futuras (cada una
tiene decisiones de diseño que arrastran).

**Implementación**:

- ~~**Helper `__parse_multipart(bytes, boundary)` en el preludio**~~ ✓
  Paralelo a `parse_multipart_body` del intérprete (`http.rs`).
  Mismo algoritmo: split por `--<boundary>`, skip preámbulo y
  epílogo, parse de headers (`Content-Disposition`/`Content-Type`),
  distinción text/file por presencia de `filename`. Para files, el
  resultado es un `serde_json::Value::Object` con shape de
  `FileData` (`name`, `content_type`, `content`), que
  `__FromFitzJson for FileData` consume. Para text fields, un
  `Value::String` plano. Body no-UTF8 → `Err`.
- ~~**Helper `__extract_multipart_boundary(ct)` en el preludio**~~ ✓
  Paralelo a `extract_multipart_boundary` del intérprete. Skip del
  primer token, parse de `boundary=<value>` o `boundary="<value>"`,
  case-sensitive del valor. Devuelve `None` si no se encuentra.
- ~~**Dispatch en `emit_param_coercions`**~~ ✓ Tercera rama del
  matching de Content-Type, paralelo a JSON y urlencoded. Sin
  boundary → 400 claro. Body inválido (no-UTF8, malformado) → 400.
  Body válido → `serde_json::Value` que alimenta el
  `__from_fitz_json` ya existente.
- ~~**415 message actualizado**~~ ✓ Ahora cita los **3 CTs
  soportados** (JSON, urlencoded, multipart) en lugar de los 2 +
  workaround a `fitz run` (mensaje viejo de MP2 cuando codegen no
  soportaba multipart). Paridad bit-a-bit con el intérprete.

**Tests**: **2 unit + 3 compile_e2e nuevos**:

- 2 codegen unit (`mp_build_codegen_emite_helpers_multipart`,
  `mp_build_codegen_dispatch_incluye_multipart_branch`).
- 3 compile_e2e (`mp_build_multipart_text_field_compila_y_parsea`,
  `mp_build_multipart_file_field_compila_y_parsea`,
  `mp_build_multipart_sin_boundary_es_400`).
- Tests ajustados:
  `uc_http_body_415_msg_matchea_interprete` actualizado al msg
  nuevo (incluye los 3 CTs); `ha_http_content_type_text_plain_es_415_con_msg_claro`
  actualizado para verificar que el msg menciona los 3 CTs;
  el test viejo `mp2_codegen_multipart_devuelve_415_con_msg_que_cita_fitz_run`
  se removió (ya no aplica — multipart deja de ser 415 en codegen).

**Cap 17 de la guía actualizado**: la sub-sección de multipart
cambia "Limitación de `fitz build`" por "Paridad `fitz run` ↔
`fitz build`". El ejemplo `17c-multipart.fitz` actualiza el comment
de cierre. Sigue en el smoke `GUIDE_EXAMPLES_COMPILE`.

**VSCode extension**: SIN cambios. Cambios server-side (codegen).

**Total al cierre: 2012 unit sin feature, 2102 con --features lsp,
245+ compile_e2e (3 nuevos − 1 obsoleto = +2 netos).** Clippy `-D
warnings` limpio en lib + bin `fitz-lsp`.

**Deuda residual (NO bloquea 9.w)** — restantes después de MP-Build:

- **Mw-Wrap (wrap-style middleware con `next` callable)**: ~6-8h.
  Decisión de diseño grande — requiere `Value::NativeFn` nuevo
  (async + Send + Sync) en el value system, integración en
  `http.rs::handle_task`, codegen paralelo emitiendo `Box<dyn Fn() -> BoxFuture>`
  para el `next`. Mini-tanda dedicada futura.
- **`Value::Bytes` para files binarios**: ~5-8h. Primitivo nuevo
  del lenguaje (`b"..."` literal syntax, métodos `.len()`/`.to_str()`,
  codegen mapping a `Vec<u8>`). Mini-tanda dedicada futura.
- **Listas/mapas heterogéneos en `fitz build` (F13)**: ~15-20h.
  Decisión grande — requiere `FitzValue` tagged runtime en el
  output del codegen + impacto en cada helper `__to_fitz_json`/
  `__from_fitz_json`. Mini-tanda dedicada futura (probable spike de
  diseño primero).

### ~~MP2 + Docs refresh + VSCode rebuild: multipart en intérprete + guide cap 17/18 + nuevo ejemplo + extensión actualizada~~ ✓ CERRADO 2026-05-20 (mini-tanda MP2 + DocsRefresh + VSCode)

Bundle de polish post-OAPI. Cierra **MP2** (multipart con files en
el intérprete con representación File) + refresh masivo de docs +
nuevo ejemplo + rebuild de la extensión VSCode.

**Parte 1 — MP2: multipart/form-data en el intérprete**:

- ~~**Tipo built-in `File`**~~ ✓ Tercer nominal del runtime HTTP
  (paralelo a `Request` y `Response`), pre-registrado por
  `register_http_builtin_types` en `types.rs`. Fields: `name:
  Str?` (filename del Content-Disposition, `null` si no es file),
  `content_type: Str?` (MIME de la part), `content: Str`. El usuario
  lo referencia sin declararlo ni importarlo.
- ~~**Parser de multipart en `http.rs`**~~ ✓ `parse_multipart_body(
  bytes, boundary)` parsea cada part: extrae headers
  (`Content-Disposition`, `Content-Type`), valida `name=`, distingue
  text field (sin `filename`) de file (con `filename`), construye
  `Value::Str` o `Value::Instance` de File. Last-wins sobre
  duplicados de `name`. Helper `extract_multipart_boundary(ct)`
  parsea el header Content-Type para sacar el `boundary=<token>`
  (con o sin comillas, case-sensitive del valor).
- ~~**Dispatcher de Content-Type extendido**~~ ✓ `handle_task` ahora
  acepta `multipart/form-data` como tercer CT soportado (junto con
  JSON y urlencoded). Sin `boundary=` → 400 claro. Body no-UTF8 →
  400 (limitación intencional del MVP — `Value::Bytes` queda como
  sub-paso futuro). 415 actualizado para mencionar los tres CTs.
- ~~**Asimetría en `fitz build`**~~ ✓ El codegen sigue rechazando
  multipart con 415, pero el msg ahora cita `fitz run` como
  workaround. `FileData` se emite en el preludio HTTP del codegen
  para que `Map<Str, File>` tipos compilen (aunque el path runtime
  no se pueble vía multipart). Multipart en codegen es deuda
  residual.

**Parte 2 — Fix de regresión del parser OAPI**:

- ~~**Lookahead robusto en `parse_return`**~~ ✓ La versión inicial
  de OAPI usaba `expression_no_struct_lit()` que rompía
  `return P { x: 1 }` (struct literal legítimo). Refactor: el
  lookahead específico (3 tokens: head Int/Ident + LBrace + Str|
  RBrace skipping newlines) detecta el patrón ReturnStatus antes
  de invocar `expression()`. Preserva struct lits multi-línea
  (`return HttpClientReq {\n method: ...,\n ... }`).
- Smoke `GUIDE_EXAMPLES_COMPILE` ahora green con todos los ejemplos
  incluido `11d-named-args.fitz` que tenía multi-línea struct lit.

**Parte 3 — Cap 17 (HTTP) refresh masivo**:

- ~~**Sub-sección urlencoded**~~ ✓ Nueva entre "Body JSON" y
  "Respuestas". Cubre el caso canónico (`Map<Str, Str>`),
  URL-decoding (`+`/`%XX`), curl ejemplo, paridad bit-a-bit
  `fitz run` ↔ `fitz build`.
- ~~**Sub-sección multipart**~~ ✓ Cubre las dos variantes (text vs
  file), tipo built-in `File`, limitación UTF-8, asimetría
  intérprete vs codegen, link al ejemplo runnable nuevo.
- ~~**Status codes con consts (mini-tanda OAPI)**~~ ✓ Extendido el
  bloque "Reglas" para mencionar que el status acepta Int literal
  O Ident a const top-level. Ejemplo `let NOT_FOUND = 404; return
  NOT_FOUND { ... }`. Limitación documentada (solo top-level Int
  literal — vars locales/expresiones complejas siguen invisibles
  al schema).

**Parte 4 — Cap 18 (Docs automáticas) refresh**:

- ~~**Nueva sección "Cerrado en mini-tanda OAPI"**~~ ✓ Documenta el
  patrón `Err({status: NOT_FOUND, ...})` y `return NOT_FOUND
  { ... }` con consts. Ejemplo `ApiErr` + `Result<T, ApiErr>`.
  Aclara qué entra al schema (Int literal + Ident a const) vs qué
  no (vars locales, expresiones).

**Parte 5 — Ejemplo nuevo `17c-multipart.fitz`**:

- ~~**Ejemplo runnable cubriendo los dos casos canónicos**~~ ✓ Form
  text-only (`Map<Str, Str>`) + form con file field
  (`Map<Str, File>`). Comentado en detalle (decisiones de diseño
  + limitaciones). Sumado al smoke `GUIDE_EXAMPLES_COMPILE`
  (compila a binario en `fitz build` — el rechazo de multipart es
  solo runtime). Validado bit-a-bit `fitz run` con multipart real.

**Parte 6 — VSCode extension rebuild**:

- ~~**Grammar TextMate**~~ ✓ Agregado `File` a la lista de
  built-in types junto con `Request`, `Response`, etc.
  (`editors/vscode/syntaxes/fitz.tmLanguage.json`).
- ~~**LSP autocomplete**~~ ✓ Agregado `File` al array de built-in
  types en `lsp.rs::completion_items`.
- ~~**`.vsix` rebuilt**~~ ✓ Plataforma actual (win32-x64) regenerada
  via `npm run build:vsix`. Compila el `fitz-lsp.exe` con el
  cambio del autocomplete + empaqueta grammar + extension client.
  Genera `fitz-language-win32-x64-0.9.2.vsix` (1.62 MB, 211 files).

**Implementación cross-cutting**:

- **`types.rs::register_http_builtin_types`**: tercer nominal `File`
  pre-registrado con sus 3 fields. `nominal_count()` ahora es 3.
- **`http.rs`**: helpers `parse_multipart_body`,
  `extract_multipart_boundary`, `parse_cd_params`. Dispatcher
  `handle_task` extiende el branch de Content-Type para multipart.
- **`parser.rs::parse_return`**: lookahead robusto con skip de
  newlines (max 16 tokens) para distinguir map literal (ReturnStatus)
  de struct literal (return normal).
- **`codegen.rs`**: `FileData` emitido en el HTTP runtime preludio
  (Display + ToFitzJson + FromFitzJson). El 415 del codegen
  actualiza su msg para citar `fitz run` como workaround para
  multipart.
- **`lsp.rs`**: `File` en la lista de built-in types para
  autocomplete.

**Tests**: **10 unit MP2 nuevos** (8 en `http::tests::mp2_*` +
1 e2e en `compile_e2e::mp2_codegen_multipart_devuelve_415_*` +
1 ajuste de `programa_vacio_no_da_errores` para nominal_count=3).
Tests viejos actualizados:
- `mp_multipart_sigue_rechazado_con_415` → `mp2_multipart_sin_boundary_es_400`
  (multipart ahora se acepta; sin boundary → 400 distinto a 415).
- `hpx1_content_type_multipart_rechaza_con_415` → `mp2_content_type_charset_diff_no_oficial_rechaza`
  (multipart ya no rechaza; probamos con `application/octet-stream`).
- `uc_http_body_415_msg_matchea_interprete` actualizado al nuevo msg
  (incluye `fitz run` como workaround).
- `ha_http_content_type_no_soportado_es_415_con_msg_alineado` →
  `ha_http_content_type_text_plain_es_415_con_msg_claro` (multipart
  ya no es el caso de 415 — text/plain sí).

**Total al cierre: 2010 unit sin feature, 2100 con --features lsp,
243+ compile_e2e (1 nuevo + 1 renombrado), 74 ejemplos en el
smoke (1 nuevo).** Clippy `-D warnings` limpio en lib + bin
`fitz-lsp`.

**Deuda residual (NO bloquea 9.w)**:

- **Multipart en `fitz build`**: el codegen rechaza con 415 + msg
  que cita `fitz run`. ~4-6h para implementar el helper
  `__parse_multipart` en el preludio + dispatch en
  `emit_param_coercions`. Sub-paso futuro dedicado.
- **`Value::Bytes` para files binarios**: el MVP solo admite files
  UTF-8. PDFs, imágenes, zips fallan con 400. Requiere decisión
  grande del lenguaje (`Value::Bytes` o `List<Int>`). Sub-paso
  futuro grande.
- **Wrap-style `next` callable middleware**: sigue diferido (~6-8h).
- **Listas/mapas heterogéneos en `fitz build`**: requiere
  `FitzValue` tagged runtime (F13).

### ~~DZ + CT + OAPI: paridad chica run↔build + OpenAPI con consts~~ ✓ CERRADO 2026-05-20 (mini-tanda DZ + CT + OAPI)

Mini-tanda de polish chico que cierra 3 asimétrias entre `fitz run` y
`fitz build`. **MP2 (multipart con files) y TM (métodos custom sobre
type en build) quedaron fuera del bundle**: el primero por scope (requiere
`Value::Bytes` o `type File` built-in, ~8-12h dedicados); el segundo
fue verificado y resultó **ya cerrado** desde R.3 — los métodos custom
sobre `type` ya compilan a binario nativo, mi test inicial falló por
usar la sintaxis equivocada (`fn greet(self)` cuando Fitz usa fields-
como-locales).

**Parte 1 — DZ: división por cero literal**:

- ~~**`10 / 0`, `10 % 0`, `10.0 / 0.0` literal compilan en `fitz build`
  y panican en runtime con `división por cero`**~~ ✓ Pre-DZ: rustc
  rechazaba con `unconditional_panic` en const-eval, programa no
  compilaba. Post-DZ: `gen_binop` para `BinOp::Div` emite `{ let
  __a: <ty> = lhs; let __b: <ty> = rhs; if __b == <0|0.0> { panic!(
  "división por cero"); } (__a / __b) }`. La rama `BinOp::Mod` ya
  tenía este patrón desde R.1.2. El check se aplica también a Float
  (sin él, `f64 / 0.0` produce `inf`/`NaN` silencioso, divergencia
  con el intérprete que retorna error explícito).
- **Refactor**: la rama compartida `Sub | Mul | Div` se splittea —
  Div tiene su path dedicado con el wrap del check de cero.

**Parte 2 — CT: comparar tipos distintos**:

- ~~**`1 == "1"`, `true == 0`, `"x" == null` compilan y devuelven
  `false`/`true` literal**~~ ✓ Pre-CT: el codegen emitía `Rust` con
  `1i64 == String::from("1")` que rustc rechazaba con E0308. Post-CT:
  detector `ct_incompatible_eq(lt, rt)` para combinaciones primitivas
  incompatibles (Int↔Str, Bool↔Int, Str↔Null sin Nullable, etc.).
  Emite `{ let _ = lhs; let _ = rhs; false }` (o `true` para `!=`),
  preservando side effects de ambas expresiones. Alineado bit-a-bit
  con el intérprete que devuelve `false` por `Value::PartialEq`
  distinguiendo variants.
- **Scope**: cubre exhaustivamente pares de primitivos
  Int/Float/Str/Bool/Null. Int↔Float sigue coercionando vía
  `numeric_coerce`. Tipos no primitivos (Nominal, List, Map, etc.)
  caen al fallback original.

**Parte 3 — OAPI: status codes dinámicos en schema OpenAPI**:

- ~~**Pre-scan top-level Int consts**~~ ✓ `collect_top_level_int_consts(
  program) -> HashMap<String, i64>` walkea el program AST extrayendo
  bindings `let X = <Int literal>` y `let Y = -<Int literal>` (con
  UnaryOp::Neg). Otros RHS (expresiones complejas, vars, llamadas)
  se omiten — quedan invisibles para el schema.
- ~~**`Err({ status: <Ident>, ... })` resuelve a Int desde la tabla**~~
  ✓ `resolve_status_value(expr, consts)` acepta `Expr::Int(n)` directo
  o `Expr::Ident(name)` con lookup en `consts`. El walker de OpenAPI
  consulta esta función en lugar del match Int-only de HC.2. Patrón
  canónico: `let NOT_FOUND = 404; ... return Err(ApiErr { status:
  NOT_FOUND, ... })` ahora aparece en el schema como `responses.404`.
- ~~**`Stmt::ReturnStatus` con Ident**~~ ✓ El parser ahora acepta
  `return NOT_FOUND { "error": "x" }` (antes solo Int literal).
  Disambiguación clave: la `expression()` greedea `Ident { ... }` como
  struct literal — para evitar la colisión, en `parse_return`
  agregamos lookahead específico ANTES de invocar `expression()`:
  peek 3 tokens (`<Int|Ident>` `{` `<Str|RBrace|Newline>`) → ReturnStatus;
  caso contrario → path normal con `expression()`. Esto preserva
  `return P { x: 1 }` (struct lit, primera clave Ident) sin cambios.
- ~~**Runtime + codegen ya soportaban Ident**~~ ✓ `eval_expr`
  evalúa el `status` como expr arbitrario y chequea que sea Int; el
  codegen `gen_return_status` también pasa el status por `gen_expr`
  general. Ningún cambio adicional necesario en evaluator/codegen.
- **APIs públicas**: `collect_status_codes(body)` se mantiene como
  wrapper back-compat (delega con tabla vacía). Nueva
  `collect_status_codes_with_consts(body, consts)`. `routes_from_registry`
  ahora toma `&Program` como param adicional (firma cambió). 2 call
  sites actualizados (`http.rs::serve`, `main.rs::Commands::Openapi`).
- **Limitación**: solo resuelve Idents que apunten a `let X = <Int literal>`
  top-level. Vars locales del handler, params, expresiones derivadas
  (`let X = compute_code()`) siguen invisibles. Caen al 500 default
  histórico.

**MP2 — DEFERIDA con scope claro** (NO bloquea 9.w):

- `multipart/form-data` con file uploads requiere:
  1. Nuevo `Value::Bytes` o convención `type File { name: Str?, content: ?, content_type: Str? }` built-in.
  2. Parser de multipart en `http.rs` (boundary extraction, parts splitting,
     per-part headers, Content-Disposition con `name` y `filename`,
     decoded content per part).
  3. Codegen paralelo en `gen_param_coercions` (similar al de urlencoded).
  4. 415 alineado para casos malformados.
  5. Decisión sobre file representation y memoria (¿streaming?
     ¿in-memory?).
- Total estimado: ~8-12h. Mini-tanda dedicada cuando entre demanda real.
- Hoy: multipart sigue rechazado con 415 + msg claro (cerrado en
  UC + HA).

**Implementación cross-cutting**:

- **`codegen.rs::gen_binop`**: rama `Sub | Mul | Div` se splitea;
  rama `Div` nueva con check de cero. Nueva fn `ct_incompatible_eq`
  + rama nueva en `BinOpKind::Eq | NotEq` que emite literal `false`/
  `true` cuando los tipos son incompatibles. 13 unit tests nuevos
  (`dz_*` x3, `ct_*` x6, `oapi_*` x4).
- **`openapi.rs`**: nuevas APIs `collect_status_codes_with_consts`,
  `collect_top_level_int_consts`, `resolve_status_value`. Walker
  acepta `&HashMap<String, i64>` para resolver Idents.
- **`parser.rs::parse_return`**: lookahead específico (3 tokens) para
  detectar el patrón ReturnStatus con Ident como status. Sin tocar
  `expression()`.
- **`http.rs::serve` + `main.rs::Commands::Openapi`**: pass `&program`
  a `routes_from_registry`.

**Tests**: **13 unit + 5 compile_e2e nuevos**:

- 3 codegen DZ (Int check, Float check, literal compila aunque panique).
- 6 codegen CT (Int↔Str eq/neq, Bool↔Int, Str↔Null, Str==Str sigue ==,
  Int↔Float sigue coerce).
- 4 openapi OAPI (recolecta consts top-level, return Ident en schema,
  Err StructLit con Ident en schema, Ident no resuelve se omite).
- 5 compile_e2e:
  `dz_division_int_por_cero_compila_y_panica_con_msg_alineado`,
  `dz_division_float_por_cero_compila_y_panica_con_msg_alineado`,
  `ct_comparar_int_vs_str_compila_y_devuelve_false`,
  `ct_paridad_bit_a_bit_run_vs_build_comparaciones_incompatibles`,
  `oapi_return_ident_a_const_top_level_compila_y_emite_schema`.

**VSCode extension**: SIN cambios. Cambios server-side (codegen +
parser + openapi). Grammar TextMate y client TS sin updates.

**Total al cierre: 2001 unit sin feature, 2091 con --features lsp,
241+ compile_e2e (5 nuevos).** Clippy `-D warnings` limpio en lib +
bin `fitz-lsp`.

**Deuda residual (NO bloquea 9.w)**:

- **Multipart/form-data con files**: ~8-12h. Sub-paso futuro dedicado.
- **Status codes con expresiones complejas** (no literales ni Idents
  a consts): `let X = compute()` con `compute()` no resoluble
  estáticamente. Caen al 500 default histórico — refinable solo con
  un mini-evaluator estático sobre expresiones puras (complejidad
  no justificada por el caso).
- **Wrap-style `next` callable middleware**: sigue diferido (~6-8h).
- **Listas/mapas heterogéneos en `fitz build`**: requiere `FitzValue`
  tagged runtime (decisión grande, F13).

### ~~UC + HA: urlencoded en codegen + Hpx.1 msg alignment — paridad HTTP completa~~ ✓ CERRADO 2026-05-20 (mini-tanda UC + HA)

Cierra las DOS asimétrias HTTP restantes entre `fitz run` y
`fitz build` que arrastraban las mini-tandas anteriores. Después
de esta, el stack HTTP tiene **paridad bit-a-bit completa** entre
intérprete y binario nativo para todo el modelo de bodies.

**Parte 1 — UC: urlencoded bodies en codegen**:

- ~~**Extractor del body cambia de `axum::Json` a `axum::body::Bytes`**~~
  ✓ En `emit_axum_extractors` del codegen, cuando hay `body_param`
  el extractor pasa de `axum::Json(<bn>_raw): axum::Json<serde_json::Value>`
  a `<bn>_body_bytes: axum::body::Bytes`. Esto permite que el
  wrapper inspeccione el Content-Type antes de parsear (axum::Json
  imponía JSON parse antes de llegar al wrapper).
- ~~**Forzamos extracción del `HeaderMap` cuando hay body**~~ ✓
  La condición para extraer `__hmap: axum::http::HeaderMap` ahora
  incluye `sig.body_param.is_some()` — necesitamos leer el
  Content-Type. Antes solo se extraía si había `@header(...)`,
  middlewares o CORS.
- ~~**Dispatch por Content-Type en `emit_param_coercions`**~~ ✓
  Tres ramas paralelo al intérprete (`http::handle_task`): (a) JSON
  o vacío → `serde_json::from_slice(&<bn>_body_bytes)`; (b)
  urlencoded → `__parse_urlencoded(&<bn>_body_bytes)` que devuelve
  un `serde_json::Value::Object` con String values; (c) cualquier
  otro CT → 415 con el msg verbose del intérprete. Las tres ramas
  producen un `serde_json::Value` intermedio que alimenta el
  `__from_fitz_json` ya existente, así que el resto del wrapper
  queda intacto.
- ~~**Helpers `__parse_urlencoded` y `__url_decode` en el preludio
  HTTP**~~ ✓ Paralelos a `parse_urlencoded_body` y `url_decode` de
  `http.rs`. URL-decoding completo (`+` → espacio, `%XX` → byte
  hex con errores claros sobre %XX incompleto / no-hex).
- ~~**Impls nuevos `__FromFitzJson` para `Vec<T>` y `Vec<(K, V)>`**~~
  ✓ `Vec<T>` deserializa desde JSON Array (List body, deuda menor
  destrabada de paso). `Vec<(K, V)>` deserializa desde JSON Object
  parseando la clave K via trait nuevo `__MapKeyFromStr` (espejo
  de `__MapKey`). Habilita el case canónico de urlencoded:
  `Map<Str, Str>` como body.

**Parte 2 — HA: Hpx.1 msg alignment**:

- ~~**Mensaje del 415 alineado bit-a-bit con el intérprete**~~ ✓ El
  codegen emite el mismo formato del intérprete: `"Content-Type
  no soportado: '<ct>'. El handler espera JSON (\`application/json\`)
  o urlencoded (\`application/x-www-form-urlencoded\`). Multipart
  y otros formatos quedan como sub-paso futuro."`. Si falta el
  header, se cita `(sin header)`. La asimetría textual entre
  intérprete (msg verbose) y codegen (msg axum default) que
  quedó como deuda menor de Hpx.1 está cerrada.

**Implementación cross-cutting**:

- **`codegen.rs::emit_axum_extractors`**: extractor del body pasa
  de `axum::Json` a `axum::body::Bytes`; HeaderMap forzado cuando
  hay body. Naming convention: `<bn>_body_bytes` para los bytes,
  `<bn>_ct_primary` para el Content-Type primario.
- **`codegen.rs::emit_param_coercions`**: nuevo bloque que computa
  ct_primary, dispatchea entre JSON/urlencoded/415, y produce un
  `<bn>_raw: serde_json::Value` que alimenta el `__from_fitz_json`
  ya existente.
- **`HTTP_RUNTIME_PRELUDE`**: helpers `__parse_urlencoded` y
  `__url_decode` agregados después de `__apply_cors_and_respond`.
  Impls `__FromFitzJson for Vec<T>` y `__FromFitzJson for Vec<(K, V)>`
  + trait `__MapKeyFromStr` con 4 impls (String/i64/f64/bool).

**VSCode extension**: SIN cambios. Cambios server-side
(codegen). Grammar TextMate y client TS sin updates.

**Tests**: **5 unit + 3 compile_e2e nuevos**:

- 5 unit en `codegen::tests::uc_*`:
  `uc_http_body_extrae_bytes_no_json`,
  `uc_http_body_dispatch_por_content_type`,
  `uc_http_body_415_msg_matchea_interprete`,
  `uc_http_preludio_emite_helpers_urlencoded`,
  `uc_http_body_fuerza_hmap_extraction`.
- Test viejo `http_body_post_con_tipo_emite_from_fitz_json`
  actualizado al nuevo extractor (`body_body_bytes`).
- 3 compile_e2e end-to-end (build + spawn + raw TCP):
  `uc_http_post_urlencoded_parsea_a_map_str_str` (form data
  básico), `uc_http_post_urlencoded_con_url_encoding` (`+` y
  `%20` URL-decoded), `ha_http_content_type_no_soportado_es_415_con_msg_alineado`
  (multipart/form-data rechazado con msg alineado).
- Helper nuevo `build_spawn_request_with_ct` en `tests/compile_e2e.rs`
  que permite especificar Content-Type explícito (el viejo
  hardcodeaba `application/json`).

**Smoke manual**: validación bit-a-bit `fitz run` ↔ `fitz build`
con un handler `@post("/echo") fn echo(body: Map<Str, Str>) ->
Map<Str, Str> => body` corriendo curl con `-H "Content-Type:
application/x-www-form-urlencoded" -d "name=Fitz&age=25"`.

**Deuda residual (NO bloquea 9.w):**

- **Multipart/form-data con files**: sigue diferido (~4-6h).
  Requiere parser de multipart + decisión sobre file storage.
- **Wrap-style `next` callable middleware**: sigue diferido
  (~6-8h).

### ~~P1: Mw.next codegen + cleanup roadmap-ready~~ ✓ CERRADO 2026-05-20 (mini-tanda P1 + Cleanup final)

**Mega-bundle FINAL pre-9.w**. Cierra el último punto asimétrico
entre `fitz run` y `fitz build`: post-process middlewares ahora
compilan a binario nativo. Mw.next está COMPLETO end-to-end (no
solo el intérprete como en la mini-tanda anterior).

**Parte 1 — P1: Mw.next codegen**:

- ~~**`HandlerSig` separa Pre y Post middlewares**~~ ✓ Campo nuevo
  `mw_user_fns_post: Vec<String>` paralelo a `mw_user_fns` (Pre).
  `collect_route_middlewares` clasifica por aridad del mw fn (1 arg
  = Pre gate-only, 2 args = Post wrap-style).
- ~~**`CodegenCtx.middleware_post_fn_names`**~~ ✓ Tracking de fns
  marcadas como Post. Post-scan después de `pre_register_fns`
  reclasifica las que tienen 2 params.
- ~~**`gen_top_fn` emite Post mws con return `__FitzResponse`**~~ ✓
  En lugar de `Option<__FitzResponse>` (que Pre mws usan). Sin
  trailing `None` (no aplica). `in_middleware_fn=false` +
  `response_mode=true` para que `Stmt::ReturnStatus` se emita como
  `return __FitzResponse { ... }` directo.
- ~~**Segundo param `res: Response` se emite como `__FitzResponse`**~~ ✓
  Special case en gen_top_fn cuando `is_middleware_post && i == 1`.
  Esto permite que el call site del wrapper pase `__FitzResponse`
  directo sin conversión.
- ~~**`emit_handler_dispatch_and_response` emite la post-mw chain
  AFTER handler dispatch**~~ ✓ Cubre dos paths: `returns_response`
  (ya construía __FitzResponse) y plain-T (construye `__FitzResponse
  { status: 200, body: __to_fitz_json(__result) }`). Post mws corren
  en reverse order modificando `__resp` en cada call. Después se
  convierte a axum Response y se aplica CORS.
- **Limitación: `Result<T>` + post mws**: emite error de codegen
  claro citando "sub-paso futuro". El path Result construye `__built`
  via match inline; refactorizar para construir __FitzResponse
  intermedio es invasivo, dejado para si entra demanda real.

**Validación**: smoke manual con `fn wrap(req, res) { return 200 {
"wrapped": "yes", "method": req.method } } @middleware(wrap) @get(...)`
+ curl → response wrappeada bit-a-bit como en `fitz run`.

**Parte 2 — Cleanup final**:

- README "Estado del proyecto" actualizado: sumada sección
  "Mini-tandas post-Fase 8" (cerrado en cleanup anterior P2);
  Estado de Mw.next reflejado.
- Cap 17 de la guía: bullet de wrap-style con `next` callable
  actualizado — Mw.next post-process ahora marca "end-to-end run + build"
  con limitación documentada para Result.
- deudas_lenguaje.md: cierre del bullet de "Middleware con `next`
  callable" en dos pasos (intérprete + codegen). P1 cierra el codegen.

**Implementación cross-cutting**:

- **`http.rs`**: SIN cambios nuevos en esta tanda (Mw.next
  intérprete cerrado en mini-tanda anterior).
- **`codegen.rs`**: ~80 LoC nuevas. `HandlerSig` suma `mw_user_fns_post`.
  `CodegenCtx` suma `middleware_post_fn_names`. `gen_top_fn` switch
  para emit del return type + param signature de Post mws.
  `emit_handler_dispatch_and_response` añade post-chain emission
  en returns_response + plain-T paths; error claro para Result+post.
  `collect_route_middlewares` clasifica por aridad.

**VSCode extension**: SIN cambios. Cambios server-side.

**Tests**: smoke manual bit-a-bit `fitz run` ↔ `fitz build` validó
el caso canónico. **1979 unit sin feature, 2069 con --features lsp**
sin regresiones. Clippy `-D warnings` limpio en lib + bin
`fitz-lsp`.

**Deuda residual (NO bloquea 9.w)**:

- **`Result<T>` + post mws en `fitz build`**: error claro citando
  sub-paso futuro. ~2-3h refactor del path Result para construir
  __FitzResponse intermedio.
- **Wrap-style `next` callable**: el modelo completo donde el mw
  controla la invocación del handler sigue diferido (~6-8h).
- **Multipart/urlencoded bodies**: 415 hoy. Sub-paso futuro
  cuando aparezca presión real.

### ~~Paridad run↔build: 5b.1/Hpx.2 chained fix + cleanup masivo~~ ✓ CERRADO 2026-05-20 (mini-tanda P2 + Cleanup)

Mini-tanda final pre-9.w. Cierra el chained dependency entre 5b.1
(param type inference) y Hpx.2 (return type inference): cuando AMBOS
están sin anotar Y el return depende del param, antes fallaba por
order de ejecución (Hpx.2 corría con TypeInfo donde param era Any).

**Parte 1 — P2: 5b.1/Hpx.2 chained fix**:

- ~~**Re-check pass tras inferencia de params**~~ ✓ Implementación
  en `main.rs::build_file`: tras el primer `check_program`, si hay
  fns con params sin anotar, llamar a `codegen::fill_inferred_param_types`
  (muta el AST en-place fillingo `Param.type_` con tipos inferidos)
  y re-correr `check_program` para producir un TypeInfo refinado.
  Costo extra: 1 check pass solo cuando hay fns sin anotar. Para
  programas anotados: gratis.
- ~~**Helpers públicos en codegen**~~ ✓ `has_unannotated_fn_params(program)`
  y `fill_inferred_param_types(program, type_info)`. El segundo
  walkea el program, infiere tipos de params via call sites
  (reusa `infer_param_type_from_call_sites` de 5b.1), y convierte
  el `Type` resuelto a `TypeExpr` sintáctico via
  `type_to_type_expr`. Tipos cubiertos: primitivos, Nullable,
  List/Map, Result. Nominal queda como deuda menor (requiere
  acceso al TypeEnv).
- **`fn double(n) { return n * 2 }` ahora compila** ✓ con
  `double(21)` en algún punto del programa. La primera pasada del
  checker tipa `n` como Any → `n * 2` como Any. La inferencia 5b.1
  detecta `n: Int` desde el call site, mutamos `Param.type_ =
  Some(TypeExpr::named("Int"))`, re-corremos el checker que ahora
  tipa `n * 2` como Int, y Hpx.2 toma Int como ret type.

**Parte 2 — Cleanup masivo**:

- README "Estado del proyecto": sumada sección "Mini-tandas
  post-Fase 8 (polish del lenguaje base)" con bullet de la serie
  completa de mini-tandas (R.x, S/Mb-series, Math+Mb9, It/Cmp+/Up/Ex,
  Bits/Núm/Lit/F8/F9/Fmt-build, Cd/F11-F19, Fp+Sp/Fp.2/Fp.3/Sp.2,
  HC/LSPx/LSPy, Hpx.1/Hpx.2, Mw.next/5b.1/P2). Detalle en este
  archivo.
- **P1 (Mw.next codegen)**: DEFERIDO. Estimado original ~3-4h,
  refinado a ~4-5h tras evaluar el shape de la mw fn signature
  emitida por el codegen (Pre vs Post devuelven shapes distintos).
  Documentado como sub-paso futuro dedicado.

**Implementación**:

- **`main.rs::build_file`**: `let mut program = ...` (mutable),
  primera pasada del checker, si hay fns sin anotar correr
  `fill_inferred_param_types` y re-check. Si el re-check genera
  errores, surfacearlos.
- **`codegen.rs`**: 3 helpers públicos nuevos. El walker reusa
  el patrón de `infer_param_type_from_call_sites` (mini-tanda
  5b.1) sin duplicar lógica.

**VSCode extension**: SIN cambios. Cambio interno del pipeline de
build.

**Tests**: 0 unit nuevos (cubierto por compile_e2e existentes — el
test `fp_5b1_param_int_se_infiere_con_return_anotado` ya valida el
patrón con return anotado; el nuevo caso "ambos inferidos" se
valida via smoke manual `d:/tmp/p2-quick.fitz`). Smoke validó que
`fn double(n) { return n * 2 }` + `double(21)` compila bit-a-bit.

**Deuda residual (NO bloquea 9.w)**:

- **P1 (Mw.next codegen)**: post-mws en `fitz build` requieren
  mw fn signature distinta (return `__FitzResponse` en lugar de
  `Option<__FitzResponse>`). ~4-5h dedicado.
- **type_to_type_expr para Nominal**: hoy devuelve None (deja el
  param sin inferir). Requiere acceso al TypeEnv durante el
  `fill_inferred_param_types`. Refinable si entra demanda.
- **Wrap-style `next` callable**: el modelo completo donde el mw
  controla la invocación del handler sigue diferido (~6-8h).

### ~~Mw.next (post-process middleware) + Codegen 5b.1 (param type inference) + cleanup~~ ✓ CERRADO 2026-05-20 (mini-tanda Mw.next + 5b.1 + Cleanup)

Mega-bundle pre-9.w. Cierra 2 deudas medianas + cleanup chico. Algunas
partes quedan acotadas (codegen de Mw.next, combinación 5b.1+Hpx.2)
documentadas como sub-paso futuro.

**Parte 1 — Mw.next: middleware post-process**:

- ~~**`fn mw(req, res) -> Response` para post-process**~~ ✓ Decisión
  de diseño: en lugar de wrap-style con `next` callable (más invasivo,
  requiere construir un callable Fitz desde Rust en runtime), se
  implementó **post-process model** que cubre 80% del caso real
  (timing, headers, logging post-handler). Diferencia: el middleware
  no controla SI invocar el handler — siempre se invoca y el
  middleware ve la response final.
- ~~**`MiddlewareKind::{Pre, Post}` detectado por aridad**~~ ✓ 1 arg =
  Pre (gate-only clásico); 2 args = Post (recibe `(Request, Response)`,
  devuelve `Response`). Validado en `collect_middlewares` del
  evaluator.
- ~~**`run_post_middlewares` corre AFTER handler**~~ ✓ Reverse order de
  registración (último registrado es el más interno, ve la response
  primero). Cada Post recibe `(Request, Response)`, devuelve un
  nuevo Response. Extra headers (CORS, etc.) se preservan entre
  Post calls.
- **Codegen Mw.next**: NO soportado todavía. `fitz build` rechaza
  con mensaje claro si encuentra un Post middleware citando que el
  intérprete (`fitz run`) sí lo soporta. Codegen es un refactor del
  emit del wrapper (~3-4h adicional) — sub-paso futuro dedicado.

**Parte 2 — Codegen 5b.1: param type inference**:

- ~~**`fn greet(name) { ... }` infiere `name` desde call sites**~~ ✓
  Estrategia "first call site": cuando un param no tiene anotación,
  el codegen scanea el programa por `fn_name(args)` calls y consulta
  el tipo del arg en posición `param_idx` via `TypeInfo`. Si el tipo
  es concreto (no Any), usa eso como tipo del param.
- ~~**Walker recursivo cubre If/Match/While/Loop/For/BinOp/List**~~ ✓
  No se pierden call sites anidados en condiciones, branches, ni
  args de otros calls.
- **Limitación: combinación con Hpx.2**: si una fn sin anotar
  params Y sin anotar return type tiene un body cuyo return type
  depende del param (`fn double(n) { return n * 2 }`), Hpx.2 falla
  porque vio el body con param como Any. **Workaround**: anotar
  explícitamente el return type (`fn double(n) -> Int { return n * 2 }`),
  o anotar el param. Documentado en cap 20 de la guía.

**Parte 3 — Cleanup**:

- D6 de `deudas-post-5b.md` (deudas duplicadas en cap 13 vs cap 18):
  marcada CERRADA — las dos deudas originales (asignación a índice +
  state HTTP) ya cerraron via R.1.3 y F11.

**Implementación cross-cutting**:

- **`http.rs`**: `MiddlewareKind` enum nuevo (Pre/Post). `MiddlewareSpec`
  suma `kind: MiddlewareKind`. `run_middleware_chain` filtra Pre.
  Nueva fn `run_post_middlewares` corre Post en reverse order después
  del handler. `handle_task` invoca `run_post_middlewares` post-handler.
- **`evaluator.rs::collect_middlewares`**: detecta aridad (1 vs 2) y
  setea `MiddlewareSpec.kind`. Aridad 0 o ≥3 → error claro.
- **`codegen.rs`**: nuevo helper `infer_param_type_from_call_sites`
  que scanea el program por calls a una fn y consulta TypeInfo en el
  arg correspondiente. `resolve_param_type` lo usa como fallback antes
  de rechazar. `collect_route_middlewares` detecta post-mws (2 args) y
  rechaza con error claro citando "sub-paso futuro" para `fitz build`.
- **`pre_register_fns` enumera params** para pasar `param_idx` a
  `resolve_param_type`.

**VSCode extension**: SIN cambios. Server-side improvements.

**Tests**: **4 unit nuevos** (2 Mw.next en `http::tests::mwnext_*` + 2 5b.1
en compile_e2e). Total al cierre: **1979 sin feature, 2069 con
--features lsp**. Clippy `-D warnings` limpio en lib + bin
`fitz-lsp`.

**Deuda residual (NO bloquea 9.w)**:

- **Mw.next codegen**: post-mws no compilan todavía en `fitz build`.
  Sub-paso futuro dedicado.
- **Mw.next wrap-style con `next` callable**: el modelo "fn(req, next)"
  donde el middleware controla la invocación del handler queda
  comprometido como sub-paso futuro si entra demanda real. ~6-8h
  porque requiere construir un callable Fitz desde Rust en runtime.
- **5b.1 + Hpx.2 combinados**: cuando AMBOS param y return type están
  sin anotar y el return depende del param tipo, falla por order de
  ejecución (Hpx.2 corre con TypeInfo donde param es Any). Workaround:
  anotar al menos el return type.

### ~~HTTP polish: Content-Type 415 + Return type inference + cleanup~~ ✓ CERRADO 2026-05-20 (mini-tanda Hpx.1 + Hpx.2 + Cleanup)

Mini-tanda de polish HTTP pre-9.w. Cierra 2 deudas residuales que
arrastraba el stack HTTP + cleanup chico de docs stale. **Mw.next
(middleware wrap-style) quedó DEFERIDO** — el cambio es invasivo
(requiere chain composition recursiva con `next` callable Fitz,
~6-8h) y merece mini-tanda dedicada.

**Parte 1 — Hpx.1: Validación estricta de Content-Type**:

- ~~**415 con msg claro para body no-JSON**~~ ✓ El intérprete
  (`http.rs::handle_task`) chequea el header `content-type` ANTES
  de parsear el body. Si está presente y no es `application/json`
  (case-insensitive, ignora `; charset=...`), responde 415 con
  body `{"error": "Content-Type no soportado: '...'..."}`. Si el
  header está ausente, acepta JSON crudo (paridad con clientes
  tipo curl sin `-H`).
- **Paridad con codegen** ✓ El `axum::Json` extractor ya emite
  415 automáticamente para Content-Type no JSON. Los msgs difieren
  (axum: "Expected request with `Content-Type: application/json`";
  Fitz: msg verbose con valor inválido). Ambos 415 — deuda menor
  de alineación textual.

**Parte 2 — Hpx.2: Return type inference para `fitz build`**:

- ~~**`fn create(u: User) { return User { ... } }` sin `-> User`
  compila**~~ ✓ El codegen ahora toma `TypeInfo` del checker como
  parámetro y, cuando una fn no tiene `return_type` anotado, walkea
  el body buscando `Stmt::Return(e)`. Para cada uno, consulta el
  tipo de `e` en `TypeInfo` (poblado por F16) y unifica con `lub`.
  Si todos los returns dan un tipo concreto, ese es el ret type
  inferido. Si no hay returns, fallback a `Type::Null` (comportamiento
  histórico).
- **Cubre handlers HTTP + fns regulares + módulos** ✓ El cambio
  vive en `pre_register_fns` del codegen, que aplica a todas las
  fns top-level del main. Para módulos, `generate_module_rs_with_bindings`
  computa TypeInfo fresco con un `check_program` rápido del módulo.
- **Walker recursivo** ✓ `collect_return_types` baja en
  While/Loop/For body + If/Match/Loop/FnExpr expr stmts para
  capturar returns anidados. FnExpr inline tiene su propio scope
  (el ret de un closure inline NO contamina el de la fn outer).

**Parte 3 — Mw.next: DEFERIDO**

El modelo wrap-style (`fn mw(req, next) -> Response` con `next`
callable que invoca el resto de la chain) requiere:
- Detect aridad del middleware: 1 arg = gate-only (current); 2 args = wrap.
- Construir un `next` callable Fitz en runtime (necesita un nuevo
  Value variant tipo `NativeFunction` o un closure dinámico).
- Refactor de `run_middleware_chain` a forma recursiva.
- Codegen paralelo: emitir wrap-style mw como Rust fn con closure
  argument.

Total estimado ~6-8h. Quedará como mini-tanda dedicada después de
arrancar 9.w (que igualmente trae cambios en el stack HTTP).

**Parte 4 — Cleanup chico**:

- Nota stale en `deudas_lenguaje.md` sobre `g`/`G` general format
  marcada como CERRADA (cerró en Mb8+Bits-extras+Fmt-g hace varias
  mini-tandas).
- D2 en `deudas-post-5b.md` (cap stale citando métodos Str que
  cap 13 no desarrollaba) marcada CERRADA: cap 13 ya cubre los 26+
  métodos de Str via mini-tandas S/Mb/Math+Mb9.
- Cap 17 ("HTTP nativo" — "Qué todavía no anda"): "Validación de
  Content-Type" y "Inferencia de return type" salen de la lista
  (cerradas en Hpx.1/Hpx.2). El bullet de middleware `next`
  ahora explícito como sub-paso futuro.
- Cap 20 ("Qué todavía no anda con `fitz build`"): "Funciones
  sin anotar params" matizado a solo params (return type ya
  infiere desde Hpx.2). Bloque "sí anda y antes era deuda" suma
  Hpx.2.

**Implementación cross-cutting**:

- **`http.rs`**: en `handle_task`, validación de `content-type`
  header ANTES de `parse_body`. 415 con `serde_json::json!`.
- **`codegen.rs`**: `CodegenCtx` suma `type_info: &TypeInfo`,
  `CodegenCtx::new` toma el param adicional, `new_for_module`
  computa TypeInfo fresco para el módulo. `generate_project`
  expone `type_info` en la signature pública. `pre_register_fns`
  consulta el TypeInfo via helper nuevo `infer_return_type_from_body`
  (+ `collect_return_types` + `collect_returns_in_expr`).
  `lub` reusado (ya existía).
- **`main.rs`**: `build_file` plumb el TypeInfo del checker al
  codegen (antes lo descartaba como `_types`).

**VSCode extension**: SIN cambios. Cambios server-side
(intérprete + codegen). Grammar TextMate y client TS sin updates.

**Tests**: **8 unit + 3 compile_e2e nuevos**:
- 5 en `http::tests::hpx1_*` (json pasa, text/plain rechaza,
  multipart rechaza, ausente acepta, charset acepta).
- 3 compile_e2e bit-a-bit en `hpx2_*` (return inference para Str,
  Int, if/else con lub).

Total al cierre: **1980 sin feature, 2070 con --features lsp**.
Clippy `-D warnings` limpio en lib + bin `fitz-lsp`.

**Deuda residual (NO bloquea 9.w)**:

- **Mw.next (wrap-style middleware)**: comprometido como mini-tanda
  dedicada futura. ~6-8h.
- **Streaming HTTP / SSE**: pertenece a 9.w.
- **WebSockets `@ws(...)`**: pertenece a 9.w.
- **Multipart/urlencoded bodies**: 415 hoy. Sub-paso futuro si
  aparece presión real (~3-4h).
- **Hpx.1 msg alignment**: axum default vs Fitz msg. Cosmético —
  ambos 415. Refinable cambiando el extractor del codegen.

### ~~LSP polish completo (Range exacto + scope-aware autocomplete)~~ ✓ CERRADO 2026-05-20 (mini-tanda LSPy)

Mini-tanda de polish del LSP MVP antes de Fase 9.w. Cierra dos
deudas residuales que venían arrastrándose desde el cierre del
plan LSP (Fase 9.x):

- ~~**`end_span` / Range exacto en Hover/Definition/Diagnostics**~~ ✓
  Resuelto sin refactor del AST. Helpers `ident_range_at_position`
  e `ident_range_from_def` en `fitz::lsp` leen el source text +
  un Span (start) para computar el end del ident extrayendo el run
  de chars `[a-zA-Z0-9_]` adyacente. Para defs apuntando a un
  keyword (`let`/`fn`/`type`/`static`/`async`), el helper skipea
  el keyword + whitespace y devuelve el rango del ident real.
- ~~**Scope-aware autocomplete**~~ ✓ `collect_local_bindings_at`
  recorre el AST desde top-level hacia abajo, descendiendo en
  fns/loops/for stmts cuyo span_line ≤ cursor_line. Para cada
  scope visible suma: params del fn (FnDef), var del for
  (Pattern::Ident o Pattern::Tuple), y lets previos del bloque
  (`let x = ...` en línea ≤ cursor_line). No incluye lets en
  líneas posteriores al cursor (forward references no permitidas).

**Implementación cross-cutting**:

- **AST**: SIN cambios. El refactor de `end_span` en cada nodo
  (deuda S1 original) queda diferido — los LSP helpers computan
  el end leyendo el source text + nombre del ident. Trade-off
  documentado: el AST se mantiene chico, el cómputo en LSP es
  O(line_length) que para idents típicos (≤20 chars) es trivial.
- **`fitz::lsp`** (`src/lsp.rs`): nuevas APIs públicas
  `make_hover_with_range`, `make_definition_location_with_source`,
  `fitz_errors_to_diagnostics_with_source`. Las versiones legacy
  (sin source) se mantienen como wrappers para compatibilidad.
  Helpers privados `ident_range_at_position` (cursor → ident)
  e `ident_range_from_def` (def_span → ident del declarador).
- **`fitz-lsp` bin** (`src/bin/fitz-lsp.rs`): `check_and_publish`
  usa `_with_source` para diagnostics. `Backend::hover` usa
  `make_hover_with_range`. `Backend::goto_definition` usa
  `make_definition_location_with_source` para defs locales y
  re-lee el archivo target desde FS para defs cross-module.
- **scope_level_completions**: ahora toma `cursor_line` y llama
  a `collect_local_bindings_at` ANTES del walk top-level legacy.
  Las completions resultantes vienen en orden: locales del scope
  más interno → params → top-level (let/fn/type/import) →
  builtins → tipos → keywords.

**Decisiones técnicas**:

- **Sin refactor de AST `end_span`**: el LSP MVP no necesita
  end_span estricto; el ident bajo el cursor es suficiente para
  90% de los hover/def/diagnostics use cases. Refactor del AST
  postergado hasta que aparezca un caso (call chain `a.b().c()`)
  donde la heurística de ident-under-cursor no alcance.
- **Scope-aware con filter `start ≤ cursor_line`**: pragmático.
  El AST no lleva end_span en stmts; usamos la convención "el
  cursor está dentro del scope si el stmt arrancó antes y no
  pasamos a un sibling posterior". Funciona para 95% del código
  con indentación típica.
- **Cross-module re-lee FS en goto_definition**: el backend hace
  `std::fs::read_to_string(target_path)` para tener el source y
  poder computar el Range exacto del ident en el archivo target.
  Costo: 1 file read por goto-def. Trade-off aceptado — el LSP
  no cachea el contenido de archivos no abiertos.

**VSCode extension**: SIN cambios. Las mejoras viven 100% en el
runtime del LSP server. Para que los usuarios reciban el polish,
basta con rebuildear el binario `fitz-lsp` (`cargo build --release
--features lsp --bin fitz-lsp`) o el `.vsix` bundled (`cd editors/
vscode && npm run build:vsix`).

**Tests**: 10 unit nuevos en `lsp::tests::lspy_*` (4 sobre
helpers de Range + 1 hover + 1 diagnostic + 4 scope-aware
completion). Total al cierre: 1972 sin feature, **2062 con
--features lsp**. Clippy `-D warnings` limpio en lib + bin
`fitz-lsp`.

**Deuda residual (NO bloquea Fase 9.w)**:

- **Range exacto para Expr no-Ident**: hover sobre `xs.map(...)`
  highlightea solo `map`, no el call entero. Refactor del AST
  con `end_span` lo destrabaría — sub-paso futuro si entra
  presión real.
- **Scope-aware con shadow / re-binding**: hoy si `let x = ...`
  aparece dos veces en el mismo block, ambas aparecen en
  completion. El AST no distingue. Refinable con dedup.
- **Match arm bindings en completion**: `match v { Ok(inner) =>
  ... }` no incluye `inner` en el scope-aware completion adentro
  del arm body. Refactor menor pendiente.

### ~~HTTP polish + LSP cross-module go-to-def + cleanup docs~~ ✓ CERRADO 2026-05-20 (mini-tanda HC + LSPx + Cleanup)

Mega-bundle pre-9.w: tres áreas distintas pero todas chicas/medianas.
Cierra 3 deudas residuales que venían arrastrándose y limpia docs
stale antes de arrancar Fase 9.w (stack web first-class).

**Parte 1 — HC.1: Err status fuera de 100..1000**:

- ~~**Fallback silencioso a 500 reemplazado por error claro**~~ ✓
  Cuando `return Err(X { status: 50, ... })` o `... status: 1500`
  (fuera del rango HTTP válido 100-999), el runtime ya no cae a
  500 con `{"error": e}` opaco. Emite 500 con body explícito:
  `{"error": "status code inválido en Err: 50 (debe estar en 100..1000)"}`.
- **Paridad bit-a-bit** ✓ El handler wrapper del codegen
  (`fitz build`) emite el mismo mensaje cuando detecta status
  out-of-range, no caída silenciosa.

**Parte 2 — HC.2: Status codes custom en OpenAPI schema**:

- ~~**`return Err(X { status: 404, ... })` aparece en `responses`**~~ ✓
  El walker `collect_status_codes_expr` de `openapi.rs` ya cubría
  `Stmt::ReturnStatus` (Q.4). Sumamos cobertura para
  `Expr::Err(StructLit { status: <Int literal>, ... })`. Si el
  status es literal Int en rango [100..599], se registra en el
  schema bajo el code correspondiente. Si es una variable
  (`Err(X { status: code, ... })`), no se resuelve estáticamente
  — sigue solo el 500 default.
- **Body polimórfico** ✓ El schema del response custom queda como
  `{}` (any) — el usuario lo refina con docs externas si necesita.
  Mismo enfoque que Q.4 hizo para ReturnStatus.

**Parte 3 — LSPx: Cross-module go-to-definition**:

- ~~**F12 sobre `User` en `from foo import User` salta a foo.fitz**~~ ✓
  Cierra la deuda visible que el LSP MVP (Fase 9.x.3) dejaba: hasta
  ahora go-to-def apuntaba al span del `Stmt::FromImport` local.
  Ahora resuelve al `type User { ... }` real adentro del módulo
  importado. Funciona también para fns importadas y consts (let X
  top-level).
- **Implementación acotada al LSP** ✓ Pure-function nueva
  `resolve_cross_module_definition(program, doc_uri, target_span,
  target_name) -> Option<(Url, Span)>` en `fitz::lsp`. El backend
  del `fitz-lsp.rs` extrae el ident bajo el cursor con un helper
  `ident_under_cursor`, busca el Stmt::Import/FromImport
  correspondiente, resuelve el path target relativo al doc, parsea
  el archivo con `parse_with_recovery` y busca la decl top-level
  por nombre (`Stmt::FnDef`/`TypeDef`/`Assign(Ident)`).
- **Robustez** ✓ Si el path no resuelve (módulo movido, name no
  exportado), fallback al Location local — sin crash, sin loop.
  Solo `file://` URIs son soportados (deuda menor para URIs
  remotos / virtual fs).

**Parte 4 — Cleanup de docs stale**:

- Cap 20 ("Qué todavía no anda con `fitz build`"): removidos
  bullets de **F14** (let X = expr no literal en módulo —
  cerrado) e **F15** (imports transitivos — cerrado). Bloque
  "sí anda y antes era deuda" agregado al final mencionando ambos.
- Cap 17 ("HTTP nativo" — "Qué todavía no anda"): bullet sobre
  middleware `next` post-process explícitamente declarado como
  futuro. Bloque "sí anda" suma HC.1 (status codes inválidos
  con msg claro) y HC.2 (status codes en schema OpenAPI).
- Cap 22 ("Soporte para editores"): actualizada la descripción
  de go-to-definition para mencionar cross-module: "F12 sobre
  `User` en `from foo import User` salta al `type User` real
  adentro de foo.fitz".

**Tests**: **8 unit nuevos** (4 HC.1 en `http::tests` + 2 HC.2
en `openapi::tests` + 2 LSPx en `lsp::tests`). Total al cierre:
1972 unit sin feature, 2052 con `--features lsp`.

**Deuda residual (NO bloquea Fase 9.w)**:

- **Status codes en Err con variable** (no literal): hoy solo
  literales aparecen en el schema. Si el user hace
  `return Err(X { status: code, ... })` con `code: Int` var, el
  schema solo lista 500. Refinable si entra demanda — requiere
  análisis de flujo (qué valores puede tomar `code`).
- **Cross-module go-to-def sobre URIs virtuales** (`untitled:`,
  `inmemory:`, etc.): hoy solo `file://`. La mayoría de los
  setups del LSP usan archivos reales; URIs virtuales son edge
  case.
- **Cross-module hover** sigue mostrando solo el tipo del nodo
  local. Para mostrar la doc del símbolo importado del módulo
  remoto, hace falta capturar comments del módulo target —
  refactor más grande (~3-4h).

### ~~Varargs + named args + return-en-match — cierre del cap 11 y cap 13~~ ✓ CERRADO 2026-05-19 (mini-tanda Fp.2 + Fp.3 + Sp.2)

Bundle final de funciones polish + match expressivo antes de Fase
9.w. Cierra las 3 deudas explícitamente prometidas en la mini-tanda
anterior (Fp): varargs + named args + return-en-match-arm. Con esto,
**cap 11 y cap 13 quedan SIN deudas visibles** — el lenguaje base
está completo para arrancar 9.w (Stack web first-class).

**Parte 1 — Fp.2: Varargs `fn sum(...xs: T)`**:

- ~~**Sintaxis `...name: T`**~~ ✓ Prefijo de tres puntos antes del
  último param. El parser detecta `Token::DotDot` + `Token::Dot`
  (Fitz no tiene `Token::Ellipsis` aún; tres puntos consecutivos
  se lexean naturalmente como `..` + `.`). Reglas: solo el ÚLTIMO
  param puede ser varargs, no mutex con otros varargs ni params
  posteriores, no compatible con `default`.
- ~~**Binding como `List<T>` adentro del body**~~ ✓ El checker
  binda el param variádico con tipo `List<T>` (T = anotación del
  param, o `Any` sin anotación). Adentro del body, el usuario lo
  ve como `List<T>` regular — usa `xs.len()`, `for x in xs`, etc.
  Mismo binding semántico en evaluator y codegen.
- ~~**Aridad mínima sin contar el varargs**~~ ✓ Una fn
  `fn(a: Str, ...xs: Int)` acepta entre 1 y ∞ args. El call site
  con `< 1` args es error claro. El runtime y codegen emiten el
  mismo mensaje.
- ~~**Codegen: emit como `Arc<Mutex<Vec<T>>>` (`List<T>` Rust)**~~ ✓
  El call site empaca los args extras en `Arc::new(Mutex::new(vec![
  a, b, c]))`. La fn DEFINIDA en Rust recibe el param como
  `Arc<Mutex<Vec<T>>>`. Cero overhead respecto del path List<T>
  regular del lenguaje.

**Parte 2 — Fp.3: Named args `f(name: value)`**:

- ~~**Sintaxis `name: value` en el call site**~~ ✓ Variant nuevo
  `Expr::NamedArg { name, value }` que solo aparece adentro de
  `Call.args`. El parser lo emite cuando ve `Ident` + `Colon` en
  posición de arg (lookahead). Reusa el patrón de kwargs de
  `Decorator` que ya existía desde Fase 7.
- ~~**Reorder al despachar**~~ ✓ El evaluator (`invoke_value_named`,
  `dispatch_method_named`), el checker (relaja per-arg type check
  cuando hay nombres) y el codegen (`gen_call_with_sig` recursa
  con args re-ordenados) hacen la resolución `name → posición` y
  rellenan defaults para los slots no cubiertos. Helper compartido
  `resolve_named_args` en evaluator extrae el patrón.
- ~~**Reglas del MVP**~~ ✓ Positionals primero, named después. Sin
  duplicados (mismo nombre dos veces → error claro). Sin nombres
  desconocidos. NO compatible con varargs (decisión MVP — el
  varargs absorbería los args nombrados ambiguamente). NO
  compatible con higher-order callees (callbacks sin info de
  nombres). NO compatible con builtins (sin nombres expuestos en
  la signature de `Value::Builtin`).
- ~~**Codegen: reorder en-place y emit posicional**~~ ✓ El call
  site con named args se transforma a positional adentro del
  codegen (slots por nombre + defaults). La firma Rust de la fn
  emit posicional clásica. Cero overhead runtime.

**Parte 3 — Sp.2: `return`/`break`/`continue` en match arm**:

- ~~**`MatchArm.body: Vec<Stmt>` (en vez de `Expr`)**~~ ✓ Cambio
  estructural del AST paralelo a `Expr::If.then`. El parser
  acepta 3 formas de body:
  - `pat => expr` → `vec![Stmt::Expr(expr)]` (forma clásica).
  - `pat => { stmts }` → bloque parseado con `parse_block()`.
  - `pat => return X` / `break` / `continue` → `vec![Stmt::...]`
    directo (parseado con `parse_stmt()`, sin paréntesis).
- ~~**Evaluator y codegen evalúan stmts en orden**~~ ✓ El "valor"
  del arm es el valor del último `Stmt::Expr` (o `Null` si es
  control flow). Stmt::Return/Break/Continue propagan al fn/loop
  contenedor como cualquier otro statement. El codegen emite el
  block Rust con la última expr como tail; si la última es
  Return/Break/Continue, omite el trailing `;` para que el block
  tipe como `!` y coerce al tipo expected del match.
- ~~**Checker preserve el tipo del arm**~~ ✓ El último Stmt::Expr
  determina el tipo; control-flow stmts tipan como `Any` (never-
  coerce) — la unificación de arms ignora los arms que terminan
  en return/break/continue.

**Implementación cross-cutting**:

- **AST** (`src/ast.rs`): `Param` suma `varargs: bool`. `Expr` suma
  `NamedArg { name, value, span }`. `MatchArm.body` cambia de
  `Expr` a `Vec<Stmt>`.
- **Parser** (`src/parser.rs`): `parse_params` detecta `...` antes
  del name. `parse_call_args` detecta `Ident + Colon` con lookahead.
  `parse_match_expr` parsea body como `Vec<Stmt>` (3 formas).
- **Checker** (`src/types.rs`): `VarBinding.has_varargs` flag.
  `callee_has_varargs` helper. Reorder relax cuando hay named args.
  MatchArm: tipo derivado del último Stmt::Expr; control flow → Any.
- **Evaluator** (`src/evaluator.rs`): `invoke_value` extendido con
  varargs collect. `invoke_value_named`/`dispatch_method_named`
  para named args. `resolve_named_args` helper compartido.
  MatchArm eval iterates stmts.
- **Codegen** (`src/codegen.rs`): `FnSig` suma `has_varargs` y
  `param_names`. `gen_call_with_sig` re-orderea con named args.
  El emit del body de fn wrap-ea varargs en `List<T>` Rust.
  MatchArm emit block con control flow sin trailing `;`.
- **LSP** (`src/lsp.rs`): scope-level fn detail muestra varargs
  con prefijo `...`.
- **Walkers cross-cutting** (`fmt`, `lint`, `codegen`): updates
  para `Expr::NamedArg` (passthrough/transparent) y `MatchArm.body`
  (Vec<Stmt> iter).

**27 unit tests nuevos** (9 parser + 8 evaluator + 1 LSP + 9
otros distribuidos) + **6 compile_e2e nuevos** bit-a-bit. Ejemplos
runnables `examples/guide/11c-varargs.fitz`,
`examples/guide/11d-named-args.fitz`,
`examples/guide/13v-return-en-match.fitz` sumados al smoke
`GUIDE_EXAMPLES_COMPILE`.

Cap 11 de la guía: sub-secciones nuevas "Varargs (mini-tanda Fp.2)"
y "Argumentos nombrados (mini-tanda Fp.3)". Cap 13 sumó
"`return`/`break`/`continue` en match arm". Sección "Lo que todavía
no anda" del cap 11 ahora dice "nada significativo"; "Lo que todavía
no anda" del cap 13 quitó el bullet del return-en-match.

**Deuda residual (NO bloquea Fase 9.w)**:

- **Named args + varargs combo**: decisión MVP es mutex. Si entra
  demanda real (caso típico: middlewares HTTP con configuración),
  refinable.
- **Named args en builtins/methods built-in**: hoy solo soportado
  sobre fns top-level e instance/static methods custom. Sumar a
  `Value::Builtin` requiere capturar nombres de params al
  registrar — refactor mediano (~3-4h).
- **Match arm con stmts complejos sin trailing `;` ambiguity**:
  el parser acepta `pat => return X` y `pat => { stmts }`. El caso
  de `pat => { stmts }` donde el último stmt NO es Expr/Return ni
  Break/Continue se trata como Null tail — bit-a-bit en run y build.

### ~~Default params + cleanup docs~~ ✓ CERRADO 2026-05-19 (mini-tanda Fp + Sp)

Bundle de funciones + docs cleanup. Cierra la primera de las tres
deudas del cap 11 de la guía ("Parámetros con default"); varargs y
named args quedan como próxima mini-tanda (Fp.2/Fp.3).

**Parte 1 — Fp.1: Default params**:

- ~~**`fn greet(name: Str = "amigo")`**~~ ✓ Sintaxis Python-style:
  `name: Tipo = expr`. El default puede ser cualquier expresión
  válida (literales, idents, BinOp, etc.); se evalúa CADA vez que
  se llama sin ese arg (no se cachea — semántica idéntica a Python).
- ~~**Regla Python: defaults trailing obligatorios**~~ ✓ Una vez que
  un param tiene default, todos los siguientes también. El parser
  rechaza `fn f(a = 1, b)` con error claro citando el param ofensor.
- ~~**Soporte cross-feature**~~ ✓ Funciona en fns top-level, métodos
  custom sobre `type` (R.3) y métodos estáticos (St). `parse_params`
  es la single source of truth — todos los sitios que lo invocan
  heredan la feature automáticamente.

Implementación en 5 capas:
- **AST** (`src/ast.rs`): `Param` suma `default: Option<Expr>`.
  Espejo de cómo `Field` ya lleva `default: Option<Expr>` para
  struct fields (3.2).
- **Parser** (`src/parser.rs::parse_params`): tras el tipo opcional,
  detecta `=` y parsea la default expr. Si un param SIN default
  aparece después de uno CON default → `ErrorKind::UnexpectedToken`
  con mensaje claro.
- **Checker** (`src/types.rs`): `VarBinding` suma
  `defaults_count: usize`. `preregister_fn_signatures` lo popula
  contando params con `default.is_some()` (los defaults son
  trailing por construcción del parser). Nueva fn
  `required_arity_for_callee(ctx, callee, total)` lee la binding
  cuando `callee` es Ident resoluble y devuelve
  `total - defaults_count`. Fallback estricto a `total` para
  callbacks/fns como var (no se conserva info de defaults en
  `Type::Function`). El check de aridad pasa de `args.len() !=
  params.len()` a `args.len() < required || args.len() > params.len()`
  con mensaje informativo "espera entre R y T arg(s)".
- **Evaluator** (`src/evaluator.rs`): `invoke_value`,
  `invoke_custom_method` e `invoke_static_method` validan la nueva
  aridad y, al iterar params, completan los faltantes evaluando el
  `default` expr en el env del closure (paralelo a fields default
  de struct lit). La default expr se evalúa **en cada call** —
  útil para defaults con state como `[]` (no comparten ref).
- **Codegen** (`src/codegen.rs`): `FnSig` suma `defaults: Vec<Option<Expr>>`
  (uno por param, `None`/`Some`). `gen_call_with_sig`,
  `gen_custom_method_call` y `gen_static_method_call` rellenan
  args faltantes emitiendo `gen_expr(default_expr)` con coerción
  al tipo del param. La fn DEFINIDA en Rust mantiene firma fija
  (Rust no soporta default args nativos) — los defaults se inline
  en cada call site. **Higher-order**: vars con tipo `Type::Function`
  no llevan info de defaults (la signature paramétrica no
  conserva las exprs); estricta aridad para callbacks.
- **LSP** (`src/lsp.rs`): scope-level FnDef ahora muestra
  signature completa en `detail` con format `fn(p: T = default) -> R`.
  Helper nuevo `render_default_expr` para mostrar literales primitivos
  + idents + UnaryOp + listas/maps vacíos como string compacto;
  exprs complejas (BinOp, FnExpr, struct lit) → `...` placeholder.

**Tests**: 22 unit (5 parser + 6 checker + 5 evaluator + 1 LSP + 5 ya
incluidos en otros sitios) + 3 compile_e2e bit-a-bit `fitz run` ↔
`fitz build`. Ejemplo runnable
`examples/guide/11b-default-params.fitz` sumado al smoke
`GUIDE_EXAMPLES_COMPILE`.

Cap 11 de la guía: sub-sección nueva "Parámetros con default
(mini-tanda Fp)" con sintaxis, reglas, scopes soportados; "Lo que
todavía no anda" actualizado (default sale, varargs y named args
permanecen).

**Parte 2 — Sp.1: Cleanup docs stale**:

- Cap 1 ("Qué todavía no anda"): mencionaba tuplas y métodos custom
  como deuda — ambos cerrados hace varias mini-tandas. Reemplazado
  por mención de las deudas grandes que sí quedan abiertas
  (WebSockets, traits, herencia, operator overloading).
- Cap 12 ("Lo que todavía no anda"): mencionaba "Métodos custom
  sobre `type`" como deuda — cerrado en R.3. Reemplazado por nota
  positiva con link al cap 13.
- Cap 13 ("Lo que todavía no anda"): refactor — la lista de "métodos
  que faltan" era engañosa porque la API ya cubre el 99% de los
  casos. Texto reescrito como "abrí un issue si te falta alguno";
  el bullet de `return` adentro de match arm queda explícito como
  deuda pendiente.
- `deudas_lenguaje.md` línea 1331: marcado "Métodos sobre Int"
  como CERRADO (cerró en Math+Mb9+Int/Float methods anterior).

**Deuda residual derivada (NO bloquea)**:

- **Varargs `fn sum(...xs: Int)`**: comprometido como próxima
  mini-tanda Fp.2. Requiere `Param.varargs: bool` (mutex con
  default), `parse_params` detectar `...` antes del name, checker
  binda como `List<T>` en el body, evaluator recolecta extras en
  Vec, codegen emite con `&[T]`/`Vec<T>` Rust. ~4-6h.
- **Named args en call site `greet(name: "Fitz")`**: comprometido
  como Fp.3. Más invasivo — cambia `Call.args: Vec<Expr>` a
  `Vec<CallArg { name: Option<String>, value: Expr }>`. ~4-6h.
- **`return` adentro de match arm**: la deuda anterior de cap 13.
  Requiere agregar `Expr::Block(Vec<Stmt>)` o refactor de
  `MatchArm.body` para aceptar Vec<Stmt>. ~3-4h.

### ~~Math builtins + Mb9 + dispatch sobre Int/Float~~ ✓ CERRADO 2026-05-19 (mini-tanda Math + Mb9 + Int/Float methods)

Bundle numérico + polish final: 9 builtins globales de Math + 7
métodos chicos (Mb9) + dispatch sobre primitivos Int/Float (cierra
deuda explícita de Mb8 que comentaba "Fitz no tiene dispatch sobre
primitivos hoy — esa decisión queda como deuda futura").

**Parte 1 — Math (9 builtins globales numéricos)**:

- ~~**`abs(n)`**~~ ✓ Polimórfico: `Int → Int` o `Float → Float`.
  `Int::MIN` usa `wrapping_abs` (paralelo a Rust); evita panic en
  edge case. Heterogéneos en min/max no aplica (un solo arg).
- ~~**`min(a, b) / max(a, b)`**~~ ✓ Polimórficos. Mismos tipos
  obligatorio (`Int+Int` o `Float+Float`); mix → error claro.
- ~~**`pow(base, exp) -> Float`**~~ ✓ Siempre Float. Coerce ambos
  args a f64 + `.powf()`. Acepta cualquier combinación numérica.
- ~~**`sqrt(x) -> Float`**~~ ✓ Siempre Float. Coerce a f64.
- ~~**`ceil(x) / floor(x) / round(x) -> Int`**~~ ✓ Siempre Int.
  Pasa Int de largo; Float aplica el método y casta a i64.
- ~~**`clamp(x, lo, hi)`**~~ ✓ Polimórfico (Int o Float, mismos
  3 args). Emite `.clamp()` de Rust nativo.

Decisión polimorfismo: Math builtins tipados como `Type::Any` en
scope[0] del checker (no extendimos el sistema de tipos para
polimorfismo paramétrico real). El **codegen** hace el dispatch
concreto por nombre + tipos de args (`Int+Int → wrapping_abs`,
`Float → .abs()`, etc.). Trade-off: el checker no captura mix de
tipos como error (corre OK en intérprete polimórfico, falla
estático en codegen con mensaje claro). Esto matchea el patrón
ya establecido por `len` (poly-Any en checker, dispatch en codegen).

**Parte 2 — Mb9 (7 métodos chicos)**:

- ~~**`Str.swap_case() -> Str`**~~ ✓ Invierte mayúsculas y
  minúsculas char por char (estilo Python `str.swapcase`).
- ~~**`Str.title() -> Str`**~~ ✓ Capitaliza la primera letra de
  cada palabra. Split por whitespace, capitaliza con `.next()` +
  collect (paralelo a `str.title` Python).
- ~~**`Str.is_alpha() -> Bool`**~~ ✓ Todos chars son letras
  (ASCII alphabetic). Vacío → false (vacuous truth invertida,
  paralelo a Python).
- ~~**`Str.is_digit() -> Bool`**~~ ✓ Todos chars son `[0-9]`
  ASCII estricto. Vacío → false.
- ~~**`Str.is_numeric() -> Bool`**~~ ✓ El string completo parsea
  como número (Int o Float, con signo opcional). Más permisivo
  que `is_digit`: acepta `-42`, `3.14`. Decidimos parse-based
  porque Unicode-digit (`c.is_numeric()` de Rust) era casi
  redundante con `is_digit`. Vacío → false.
- ~~**`List.split_at(i) -> (List<T>, List<T>)`**~~ ✓ Parte la
  lista en char idx. Devuelve **Tuple** de dos lists nuevas
  (preserva semantics funcionales). Clamp safe en ambos
  extremos: `idx ≤ 0 → ([], xs)`, `idx ≥ len → (xs, [])`.
- ~~**`Map.has_value(v) -> Bool`**~~ ✓ Chequea si V está en
  algún value del map. Igualdad estructural via `PartialEq`.

**Parte 3 — Dispatch sobre primitivos Int/Float**:

Cierra deuda explícita de Mb8 (la nota decía "decisión futura si
aparece demanda"). Apareció.

- ~~**`n.abs()` para Int**~~ ✓ Equivalente a `abs(n)` pero como
  método. Útil para method chaining: `delta.abs() > threshold`.
- ~~**`n.to_str()` para Int**~~ ✓ Convierte a Str. Equivalente
  a `"{n}"` interp pero sin alocar template; nicer en chains.
- ~~**`n.to_str_base(b)` para Int**~~ ✓ Soporta bases 2/8/10/16
  (las que `format!("{:b/o/x}", n)` cubre nativamente). Bases
  inválidas → error de runtime claro. Salida sin prefijo (`ff`
  no `0xff`) para uniformidad cross-base.
- ~~**`x.abs() / x.to_str()` para Float**~~ ✓ Análogos a Int.
  `to_str` usa `__fitz_fmt_float` (bit-a-bit con `print`).
- ~~**`x.is_nan() / x.is_finite()` para Float**~~ ✓ Predicates
  útiles para validación numérica (output de divisiones,
  cálculos que podrían overflow, JSON con `NaN`/`Infinity`).

Implementación cross-cutting: el evaluator extiende
`dispatch_method` con ramas `(Value::Int(_), "method")` /
`(Value::Float(_), "method")`. El checker agrega
`infer_int_method` / `infer_float_method` y dispatcha en
`infer_method_call`. El codegen agrega ramas en `gen_method_call`
con coerción `Int → wrapping_abs / .to_string() / format!("{:b}")`.

**Bonus que entró en la misma mini-tanda**: arregla bug del
`show_expr` que no tenía rama para `Type::Tuple` y caía al fallback
Debug — split_at producía `(Mutex { data: [1, 2], poisoned: false,
.. }, ...)` en `fitz build`. Agregada rama dedicada que itera los
componentes del Tuple con `show_expr_inline` (strings entre comillas
adentro de tuples paralelo a listas/maps). Paridad bit-a-bit
recuperada.

Implementación: ~600 LoC entre evaluator (12 fns nuevas + 11 ramas
de dispatch), types (9 Math en scope[0] + helpers `infer_int_method`
/`infer_float_method` + 7 ramas Mb9), codegen (9 builtins Math en
`gen_call` + 7 ramas Mb9 + 7 ramas Int/Float en `gen_method_call`
+ rama `Type::Tuple` en `show_expr`), LSP (9 Math entries +
métodos para Int/Float + 7 Mb9).

**31 unit tests** (13 evaluator + 12 checker + 6 LSP) + **7
compile_e2e** bit-a-bit `fitz run` ↔ `fitz build`. Ejemplo runnable
`examples/guide/13u-math-mb9-y-int-float.fitz` sumado al smoke
`GUIDE_EXAMPLES_COMPILE`.

Cap 13 de la guía: filas nuevas en tabla `Str` (5 Mb9), `List<T>`
(split_at), `Map<K,V>` (has_value); tabla nueva para métodos sobre
Int/Float. Sub-sección dedicada con ejemplos para los 9 Math
builtins. VSCode extension: grammar TextMate sin cambios; LSP
autocomplete refleja todo via rebuild.

### ~~Métodos sub-lista + functional updates + bits + g/G~~ ✓ CERRADO 2026-05-19 (mini-tanda Mb8 + Bits-extras + Fmt-g)

Bundle final de polish del lenguaje base: 8 métodos chicos (Mb8) +
5 builtins globales sobre Int (Bits-extras) + completa el último
format spec faltante (`g`/`G`, cierra el resto de la deuda Fm).

**Parte 1 — Mb8 (8 métodos chicos)**:

- ~~**`List.starts_with(prefix) / ends_with(suffix) -> Bool`**~~ ✓
  Igualdad estructural. Prefix/suffix vacío → true. El chequeo del
  checker valida que el arg sea `List<T>` compatible.
- ~~**`List.insert_at(i, v) -> List<T>`**~~ ✓ Functional update.
  Clamp safe: `idx >= len` → al final; `idx < 0` → error claro.
- ~~**`List.remove_at(i) -> List<T>`**~~ ✓ Functional update.
  Exige idx en rango (no clamp — devuelve error claro fuera de
  rango, distinto a `insert_at` por convención: insert es relajado,
  remove es estricto).
- ~~**`List.zip_to_map(values) -> Map<K, V>`**~~ ✓ Convierte
  `List<K>` + `List<V>` en `Map<K, V>` (truncado al más corto).
  Equivalente a Python `dict(zip(ks, vs))`. Más natural que un
  método estático `Map.from_lists`.
- ~~**`Str.left(n) / right(n) -> Str`**~~ ✓ Primeros/últimos n
  chars (char-based, no byte). Clamp safe en ambos extremos.
- ~~**`Str.center(width, ch) -> Str`**~~ ✓ Padding bilateral.
  `ch` debe ser 1 char (validado runtime). Si el padding es impar,
  el extra va a la derecha (paralelo a Python `str.center`).

**Parte 2 — Bits-extras (5 builtins globales sobre Int)**:

- ~~**`popcount(n: Int) -> Int`**~~ ✓ Cantidad de bits en 1 (64-bit).
- ~~**`leading_zeros(n: Int) -> Int`**~~ ✓ Ceros líderes en 64 bits.
- ~~**`trailing_zeros(n: Int) -> Int`**~~ ✓ Ceros al final.
- ~~**`rotate_left(n: Int, bits: Int) -> Int`**~~ ✓ Rotación cíclica.
  `bits` se toma módulo 64 (paralelo a Rust `i64::rotate_left`).
- ~~**`rotate_right(n: Int, bits: Int) -> Int`**~~ ✓

Builtins en lugar de métodos sobre Int (Fitz no tiene dispatch
sobre primitivos hoy — esa decisión queda como deuda futura si
aparece demanda). Registrados en `register_builtins` (evaluator) y
en el scope global del checker (con firma `fn(Int) -> Int` o
`fn(Int, Int) -> Int`). Codegen los emite como métodos Rust
nativos: `count_ones()`/`leading_zeros()`/`rotate_left()`/etc.

**Parte 3 — Fmt-g (g/G general format en `fitz build`)**:

- ~~**`g` / `G` general format**~~ ✓ Cierra la última deuda residual
  de Fm/Fmt-build. Helper nuevo `__fitz_fmt_general(x, precision,
  upper)` en el preludio (gated por `uses_fmt_helpers`). Bit-a-bit
  con `src/format.rs::general_format` del intérprete:
  - precision = 0 → 1 (paralelo a Python).
  - exp = floor(log10(abs(x))) — categoriza la magnitud.
  - use_exp = exp < -4 || exp >= precision.
  - exp branch: precision - 1 después del punto, NO strip.
  - fixed branch: precision - 1 - exp dígitos después, CON strip
    de ceros trailing.
  - upper → uppercase ('e' → 'E').

  `spec_needs_helper` y `format_spec_to_rust` actualizados para
  incluir `GeneralLower`/`GeneralUpper`.

Implementación: ~700 LoC entre evaluator (8 fns Mb8 + 5 fns
builtins bits), types (8 ramas Mb8 + 5 bindings de scope global),
codegen (8 ramas Mb8 + 5 builtins en gen_call + helper
__fitz_fmt_general + helper_wrapper para g/G), LSP (8 entries
Mb8).

**23 unit tests** (12 evaluator + 9 checker + 2 LSP) + **9
compile_e2e** bit-a-bit `fitz run` ↔ `fitz build` (5 Mb8 + 2 bits
+ 2 fmt-g). Ejemplo runnable `examples/guide/13t-mb8-bits-y-fmt-g.fitz`
sumado al smoke `GUIDE_EXAMPLES_COMPILE`.

Cap 13 de la guía: 3 filas nuevas en tabla `Str` (left/right/center),
5 en tabla `List<T>` (starts_with/ends_with/insert_at/remove_at/
zip_to_map). Sub-sección dedicada con ejemplos para los 8 métodos
Mb8 + 5 builtins bits + format g/G. VSCode extension: grammar
TextMate sin cambios; LSP autocomplete refleja todo via rebuild.

**Deuda residual menor** (NO bloquea):

- ~~**Middleware con `next` callable (post-process)**~~ ✓
  PARCIALMENTE CERRADO 2026-05-20 (mini-tanda Mw.next). Implementado
  **post-process model**: middlewares de 2 args `(Request, Response)`
  corren después del handler, en reverse order. Cubre headers,
  logging, modificación de body. Soportado en `fitz run`; `fitz
  build` rechaza con mensaje claro (sub-paso futuro). El modelo
  **wrap-style con `next` callable** (donde el middleware controla
  la invocación del handler) queda como sub-paso separado — requiere
  construir un Fitz callable desde Rust en runtime.
- **Stubs `.pyi` parseados** (interop Python): requiere parser
  separado para archivos `.pyi` + integración con el bridge Python.
  Refactor grande. La interop Python actual funciona como `PyAny`
  opaco por default — los stubs darían tipado fino. Sub-paso futuro
  cuando aparezca demanda real.
- ~~**Métodos sobre Int**~~ ✓ CERRADO 2026-05-19 (mini-tanda
  Math+Mb9+Int/Float). Hoy ya hay dispatch sobre `Value::Int` y
  `Value::Float` en `dispatch_method`: `n.abs()`, `n.to_str()`,
  `n.to_str_base(b)`, `x.abs()`, `x.to_str()`, `x.is_nan()`,
  `x.is_finite()`. Los bit ops (`popcount`/`leading_zeros`/...)
  siguen como builtins globales — si entra demanda real para
  `n.popcount()`, ahora la infraestructura está lista.

### ~~Métodos extras + format specs faltantes en build~~ ✓ CERRADO 2026-05-19 (mini-tanda Mb7 + Fmt-build)

Bundle combinado de polish: 7 métodos chicos sobre colecciones y Str
(Mb7) + completar los format specs que faltaban en `fitz build`
(Fmt-build, cierra deuda residual de la mini-tanda Fm).

**Parte 1 — Mb7 (7 métodos chicos)**:

- ~~**`List.take(n)` / `List.drop(n)`**~~ ✓ Primeros/restantes
  elementos. Paralelo a `Iterator::take`/`skip` Rust. Clamp safe:
  `n <= 0` → vacía/full según método; `n >= len` → full/vacía.
- ~~**`List.init()` / `List.tail()`**~~ ✓ Todos menos último/
  primero (estilo Haskell). Vacía sobre vacía (sin error).
- ~~**`List.intersperse(sep)`**~~ ✓ Inserta `sep` entre cada par
  consecutivo. Paralelo a Haskell `intersperse`.
- ~~**`List.cycle(n)`**~~ ✓ Repite la lista `n` veces. `n <= 0`
  → vacía (política friendly).
- ~~**`Str.repeat_with(n, sep)`**~~ ✓ Variante de `repeat(n)` que
  intercala `sep`. Paralelo a Python `sep.join([s] * n)`. `n < 0`
  → error claro.
- ~~**`Map.with(k, v)`**~~ ✓ Functional update — Map nuevo con
  `k → v`, receiver intacto. Encadenable.

**Parte 2 — Fmt-build (format specs que faltaban)**:

- ~~**`,`/`_` grouping**~~ ✓ Separadores de miles para Int. El
  codegen emite `__fitz_fmt_grouping(value, ',' | '_')` (helper
  emitido en el preludio). El spec width/align Rust se aplica
  encima del String resultante.
- ~~**`%` percent**~~ ✓ Multiplica el value (Float) por 100 y
  agrega sufijo `%`. La precision del spec se pasa al helper
  `__fitz_fmt_percent(x, precision)`.
- ~~**`c` char codepoint**~~ ✓ `Int → caracter Unicode`. El helper
  `__fitz_fmt_char(n)` usa `char::from_u32(n as u32)` con fallback
  a `\u{HEX}` para codepoints inválidos.

Los 3 helpers se emiten solo cuando el programa los usa (gating
via `program_uses_fmt_helpers` que detecta `FormatSpec` con
grouping/Char/Percent en una pre-pasada). Programas que no usan
estos specs no pagan el costo del preludio extra.

**Refactor incidental**:
- `CodegenCtx` suma `uses_fmt_helpers: bool` (paralelo a
  `uses_async` y `uses_python`).
- `format_spec_to_rust` ahora devuelve `helper_wrapper:
  Option<String>` cuando el spec requiere uno de los helpers
  custom. El armado del spec Rust ignora sign/alternate/precision/
  kind cuando hay wrapper (el helper ya los aplicó); width/align
  sí siguen aplicándose sobre el String resultante.

Implementación: ~600 LoC entre evaluator (7 fns nuevas), types
(7 ramas), codegen (7 ramas + refactor de `format_spec_to_rust`
con helper_wrapper + nuevo `program_uses_fmt_helpers` walker +
3 helpers en el preludio), LSP (7 entries: 6 List + 1 Str + 1 Map).

**24 unit tests** evaluator/checker/LSP (11 evaluator Mb7 + 10
checker Mb7 + 3 LSP Mb7) + **11 compile_e2e** bit-a-bit `fitz run`
↔ `fitz build` (6 Mb7 + 5 Fmt-build: grouping coma, grouping
underscore, percent, char, grouping negativo). Ejemplo runnable
`examples/guide/13s-mb7-y-fmt-build.fitz` sumado al smoke
`GUIDE_EXAMPLES_COMPILE`.

Cap 13 de la guía: 6 filas nuevas en tabla `List<T>` (take, drop,
init, tail, intersperse, cycle), 1 en tabla `Str` (repeat_with),
1 en tabla `Map<K, V>` (with). Sub-sección dedicada "take + drop +
init + tail + intersperse + cycle + repeat_with + with + format
specs en build (mini-tanda Mb7 + Fmt-build)" con ejemplos inline.
VSCode extension: grammar TextMate sin cambios (los métodos son
identifiers; format specs son interpolation interna del Str que
no afecta tokens); LSP autocomplete refleja todo automáticamente
vía rebuild del `fitz-lsp` binary.

**Deuda residual menor** (NO bloquea):
- ~~**`g`/`G` general format**~~ ✓ CERRADO 2026-05-19 (mini-tanda
  Mb8 + Bits-extras + Fmt-g). Helper `__fitz_fmt_general(x,
  precision, upper)` bit-a-bit con `src/format.rs::general_format`
  del intérprete.

### ~~Métodos analíticos extra + async closures en build + HTTP refinements~~ ✓ CERRADO 2026-05-19 (mini-tanda Mb6 + Async-cl build + HTTP refinements)

Bundle combinado de polish: 3 métodos analíticos (Mb6) + cierre del
caveat de Async-cl build (async closures inline en `fitz build`) + 2
refinamientos del stack HTTP. Cierra el plan original de "HTTP
refinements + async closures + más métodos" en una sola mini-tanda.

**Parte 1 — Mb6 (3 métodos chicos)**:

- ~~**`List.scan(init, fn(acc, x) -> Acc) -> List<Acc>`**~~ ✓ Fold
  con outputs intermedios. Devuelve una lista con cada estado del
  acumulador. Paralelo a `Iterator::scan` Rust (sin Option). Casos
  canónicos: sumas parciales, máximos acumulados, running averages.
- ~~**`List.windows(n) -> List<List<T>>`**~~ ✓ Sliding windows.
  Paralelo a `slice::windows` Rust. `len < n` → lista vacía;
  `n <= 0` → error claro de runtime.
- ~~**`Map.merge_with(other, fn(V, V) -> V) -> Map<K, V>`**~~ ✓
  Generaliza `merge`. El callback decide qué value queda en
  conflicts (caso típico: `(a, b) => a + b` para sumar valores).

**Parte 2 — Async-cl build (cierra caveat de Async-cl)**:

- ~~**Async closures inline en `fitz build`**~~ ✓ El codegen ahora
  emite `move |args| -> Pin<Box<dyn Future<Output=T> + Send>> {
  Box::pin(async move { ... }) }`. Paridad bit-a-bit `fitz run` ↔
  `fitz build`. Refactor menor: `gen_fn_expr_as_value` toma un
  parámetro `is_async`; cuando es true, envuelve el body en
  `Box::pin(async move { ... })` y ajusta el return type al `Pin<...>`
  boxed. `Type::Future` se cambió a emitir con `+ Send` siempre
  (multi-threaded safe, paralelo al post-F17 modelo).

**Parte 3 — HTTP refinements**:

- ~~**Status codes específicos por kind de `Err`**~~ ✓ Convención:
  si el `Err` lleva una `Instance` con field `status: Int`, el
  runtime HTTP usa ese status code (clamp a `100..1000`, fuera de
  rango fallback a 500). El body es la Instance serializada
  íntegra (no envuelta en `{"error": ...}`). Sin field `status`,
  fallback al 500 histórico con `{"error": e}`.
  
  Implementación: `HandlerSig` suma `err_has_status_field: bool`
  detectado estáticamente vía nuevo helper `err_type_has_status_field`
  (busca `Nominal` con field `status: Int`); el wrapper emite
  código que lee `__e.lock().unwrap().status` y arma la response.
  Runtime análogo en `value_to_outcome` para el path `fitz run`.

- ~~**CORS request-aware con echo del Origin sin filtro**~~ ✓
  Nuevo variant `AllowOrigin::Echo` (paralelo a `Literal` y `Set`
  pre-existentes). Construido cuando el config Map tiene
  `allow_origin: "echo"` (Str literal especial). Hace echo del
  Origin recibido sin filtro (acepta cualquier Origin). Útil para
  dev local. Si la request NO manda Origin, NO se emite el header
  (paralelo a `Set` sin match — CORS estricto).

**Refactor incidental**:
- `Type::Future` → `Pin<Box<dyn Future<Output=T> + Send>>` (con
  `+ Send`). Antes no llevaba `+ Send`; ahora sí, requerido por
  el bound `+ Send + Sync` del `Arc<dyn Fn>` que envuelve async
  closures. Los `async move` Rust producen futures Send siempre
  que las capturas sean Send (que post-F17 todas lo son).
- `BuildAllowOrigin` (codegen) suma variant `Echo` paralelo a
  `AllowOrigin::Echo` del runtime.

Implementación: ~450 LoC entre evaluator (3 fns Mb6 + helper +
match arm `AllowOrigin::Echo` + parsing del Str "echo" del config
CORS), types (3 ramas Mb6 + ya estaban los tipos para Async-cl),
codegen (3 ramas Mb6 + refactor de `gen_fn_expr_as_value` para
async + lookup del field `status` en err handler + parsing "echo"
+ emit del Echo case en `__cors_resolve_*`), http.rs (variant
`Echo` + resolve + status code lookup en `value_to_outcome`).

**26 unit tests** (10 evaluator + 9 checker + 7 nuevos para HTTP
[3 status + 2 cors echo + 2 LSP Mb6]) + **8 compile_e2e** bit-a-bit:
3 Mb6 (scan + windows + merge_with) + 1 async closure inline en
build + 2 HTTP-Err (404 con body Instance, OK path) + 2 HTTP-Cors
echo (con/sin Origin). Ejemplo runnable
`examples/guide/13r-mb6-y-async-build.fitz` sumado al smoke
`GUIDE_EXAMPLES_COMPILE` con casos típicos.

Cap 13 de la guía: 2 filas nuevas en tabla `List<T>` (scan,
windows), 1 en tabla `Map<K, V>` (merge_with). Sub-sección
dedicada "scan + windows + merge_with + async closures en build
+ HTTP refinements" con ejemplos inline para Mb6, async closures,
HTTP-Err y CORS echo. VSCode extension: grammar TextMate sin
cambios (los métodos son identifiers genéricos; `async` ya es
keyword); LSP autocomplete refleja todo via rebuild del `fitz-lsp`.

**Deuda residual menor** (NO bloquea):
- ~~**Middleware con `next` callable (post-process)**~~ ✓ PARCIALMENTE
  CERRADO 2026-05-20 (mini-tanda Mw.next). Post-process model
  (2-arg middleware que recibe `(Request, Response)`) funciona en
  `fitz run`; `fitz build` queda como sub-paso futuro. El modelo
  wrap-style con `next` callable sigue diferido — requiere refactor
  invasivo (~6-8h).
- ~~**`Err` con status fuera de 100..1000**~~ ✓ CERRADO 2026-05-20
  (mini-tanda HC.1). Ya no cae silenciosamente a 500 con `{"error":
  e}`. Emite 500 + body con mensaje explícito citando el status
  inválido. Paridad bit-a-bit `fitz run` ↔ `fitz build`.
- ~~**Status codes en el OpenAPI schema**~~ ✓ CERRADO 2026-05-20
  (mini-tanda HC.2). `return Err(X { status: 404, ... })` con
  literal Int aparece en el schema bajo el code correspondiente.
  Vars dinámicos (`status: code`) siguen solo en el 500 default
  — deuda residual menor.

### ~~Métodos analíticos + async closures inline~~ ✓ CERRADO 2026-05-19 (mini-tanda Mb5 + Async-cl)

Bundle combinado siguiente a Mb4 + Cmp+: cuatro métodos analíticos
sobre colecciones + dos sobre Str + un feature nuevo (async closures
inline). Async-cl cierra una deuda explícita mencionada desde el
prompt inicial de la sesión.

**Parte 1 — Mb5 (6 métodos chicos)**:

- ~~**`List.group_by(fn(T) -> K) -> Map<K, List<T>>`**~~ ✓ Agrupa
  por key derivada del callback. Preserva orden de aparición. K se
  infiere del ret type del callback. Útil para clasificar listas
  de instancias por algún field o categoría calculada.
- ~~**`List.zip_with(ys, fn(T, U) -> V) -> List<V>`**~~ ✓ Combina
  zip + map en un paso. Trunca al más corto (paralelo a Python `zip`).
  V se infiere del ret type del callback. Implementación nota:
  reusa `gen_binary_callback_inline_with_ret` con `expected_ret_ty`
  explícito; dry-run del FnExpr con `infer_callback_ret_silently_binary_named`
  (helper nuevo que usa los nombres reales de los params, no
  placeholders).
- ~~**`List.max_by(fn(T) -> Int) -> Result<T>` y `min_by(...)`**~~
  ✓ Útil para tipos no numéricos (Instance, Str, etc.) donde
  `max`/`min` directos no aplican. El callback extrae un Int
  ranking; devuelve el item con max/min ranking. Vacía → Err.
- ~~**`Str.lines() -> List<Str>`**~~ ✓ Separa por `\n`. Paralelo
  a `str::lines` Rust: si el string termina con `\n`, NO agrega
  línea vacía al final.
- ~~**`Str.is_empty() -> Bool`**~~ ✓ Atajo de `len() == 0` con
  intención clara. `is_empty()` sobre `""` es `true`.

**Parte 2 — Async-cl (async closures inline)**:

- ~~**`async fn(...) => ...` y `async fn(...) { ... }` como
  expresión**~~ ✓ AST: `Expr::FnExpr` suma `is_async: bool`.
  Parser detecta el prefijo `async fn(...)` en posición de
  expresión (lookahead `peek_at(2)`). Evaluator: el `Value::Function`
  resultante hereda `is_async`, así que al invocar devuelve
  `Value::Future` perezoso (paralelo a fns async top-level). Checker:
  `await_stack` pushea `is_async` del FnExpr — habilita `.await`
  adentro del cuerpo del async closure; sync FnExpr lo sigue
  rechazando. El tipo final del FnExpr async es
  `Function { ret: Future<T> }`.

  **Limitación de codegen**: `fitz build` rechaza async closures
  inline con mensaje claro citando el workaround (declarar la fn
  async top-level y referenciarla por nombre). El boxing real con
  `Pin<Box<dyn Future + Send>>` requiere infraestructura adicional
  que queda como deuda residual menor. `fitz run` los soporta
  end-to-end.

  Habilita patrones funcionales async:

  ```fitz
  async fn pipeline(input, transform) -> Int {
      return transform(input).await
  }
  // Async closure como arg.
  pipeline(21, async fn(n: Int) -> Int { return n * 2 }).await
  ```

**Refactor incidental**:
- Nuevo helper `infer_callback_ret_silently_binary_named` en
  codegen que dry-run un FnExpr binario usando los nombres reales
  de los params (en lugar de placeholders `__p0_dry`/`__p1_dry`).
  Necesario para que el body del callback resuelva los idents
  declarados localmente. Reusado por `zip_with` para inferir V
  estático antes de emitir el closure binario.

Implementación: ~550 LoC entre evaluator (5 fns nuevas List + 2
Str + propagación de is_async en FnExpr), types (5 ramas List + 2
Str + propagación de is_async + await_stack), codegen (5 ramas
List + 2 Str + guard de async FnExpr + helper nuevo), LSP (4 + 2
entries), parser (detección de `async fn(...)` en primary +
parse_fn_expr).

**21 unit tests** evaluator/checker/LSP (10 evaluator + 9 checker
+ 2 LSP) + **7 compile_e2e** bit-a-bit `fitz run` ↔ `fitz build`
(group_by + zip_with + max_by + min_by vacía + lines + is_empty
+ async closure aborta con mensaje claro). Ejemplo runnable
`examples/guide/13q-mb5-y-async-closures.fitz` sumado al smoke
`GUIDE_EXAMPLES_COMPILE` con casos típicos.

Cap 13 de la guía: 2 filas nuevas en tabla `Str` (lines,
is_empty), 4 en tabla `List<T>` (group_by, zip_with, max_by,
min_by). Sub-sección dedicada "group_by + zip_with + max_by/min_by
+ lines + async closures (mini-tanda Mb5 + Async-cl)" con
ejemplos inline + caveat documentado del codegen para async
closures. VSCode extension: grammar TextMate sin cambios (`async`
ya es keyword; los métodos son identifiers genéricos); LSP
autocomplete refleja todo via rebuild del `fitz-lsp` binary.

**Deuda residual menor** (NO bloquea):
- Async closures inline en `fitz build` — requiere boxing con
  `Pin<Box<dyn Future + Send>>` y un type erasure adicional. La
  fn async top-level es el workaround documentado.
- `List.group_by` ordering — preservamos orden de 1ra aparición
  de cada key. Si el caso de uso necesita orden alfabético/
  numérico de keys, usar `m.keys_sorted()` (Mb2) sobre el output.
- `min_by`/`max_by` con callback `fn(T) -> Float` — hoy exigimos
  Int. Float ranking es deuda menor (necesita `partial_cmp` en
  el codegen).

### ~~Métodos chicos + comprehensions extendidas~~ ✓ CERRADO 2026-05-19 (mini-tanda Mb4 + Cmp+)

Bundle combinado que cierra dos sets de deudas relacionadas en una
mini-tanda: cuatro métodos chicos siguientes a Mb3 (Mb4) más las
deudas residuales explícitas de la mini-tanda C (Cmp+ — múltiples
`for` clauses + Map comprehensions, ambas marcadas como "diferido"
desde el cierre de C).

**Parte 1 — Mb4 (4 métodos chicos)**:

- ~~**`List.unique() -> List<T>`**~~ ✓ Dedup preservando orden de
  1ra aparición. Igualdad estructural via `PartialEq` de Value.
  O(n²) por búsqueda lineal — para listas chicas (<1000) es
  aceptable. Paralelo a Python `list(dict.fromkeys(xs))`.
- ~~**`List.partition(pred) -> (List<T>, List<T>)`**~~ ✓ Divide
  en truthy/falsy preservando orden. Callback `fn(T) -> Bool`.
- ~~**`Map.invert() -> Map<V, K>`**~~ ✓ Intercambia keys/values.
  Last-write-wins en values duplicados (paralelo a `to_map`).
- ~~**`Str.split_at(idx) -> (Str, Str)`**~~ ✓ Divide en char idx
  (no bytes — uniforme con el resto de los Str de Fitz). `idx >=
  len` → segundo elemento vacío; `idx < 0` → error de runtime
  claro.

**Parte 2 — Cmp+ (comprehensions extendidas)**:

- ~~**Múltiples `for` clauses en list comprehensions**~~ ✓ Cartesian
  product paralelo a Python: `[expr for x in xs for y in ys]`. El
  segundo iter puede depender del binding del primero. El AST suma
  `Expr::ListComp.extra_clauses: Vec<(Pattern, Expr)>` (vacío en
  el caso single-for, retro-compatible). Parser sumó loop que
  acepta `for <pat> in <iter>` repetido antes del `if` opcional.
  Checker reusa el mismo scope acumulativo para todos los clauses.
  Evaluator/codegen emiten loops anidados naturales.

- ~~**Map comprehensions `{key: value for ...}`**~~ ✓ AST nuevo
  `Expr::MapComp { key, value, var, iter, extra_clauses, filter,
  span }`. Parser detecta `for` después del primer `key: value` del
  literal map; los map literals normales `{k1: v1, k2: v2}` siguen
  funcionando idénticos. Soporta los mismos features que list
  comprehensions: múltiples for clauses + filter opcional. En keys
  duplicadas, last-write-wins (paralelo a Python dict
  comprehension). Codegen emite loops anidados Rust con
  `Vec<(K, V)>` interno y find-or-push (preserva insertion order
  del primer push de cada key).

**Refactor incidental**:
- Helper `check_comp_clause_in_checker` reusado por List y Map
  comprehensions (validación del iter + bind del pattern).
- Helper `gen_comp_clause_header` en `CodegenCtx` para emitir el
  header `for <binding> in <iter>` Rust nativo desde una `(Pattern,
  Expr)` clause. Reusa la lógica de `Pattern::Ident`/`Wildcard`/
  `Tuple` → binding Rust de Md/Up.
- Parser: helper compartido `parse_comprehension_clauses` consume
  la cola `for X in Y [for ...]* [if cond]?]` para list y map.
- Walkers (state refs en codegen, capture collection, lint
  walk_expr, lint collect_uses_in_expr, fmt end_line_of_expr,
  fmt fmt_expr) actualizados para iterar `extra_clauses` y para
  manejar la nueva variant `Expr::MapComp`.

Implementación: ~700 LoC entre AST (suma extra_clauses + variant
MapComp), parser (~80 LoC), evaluator (helpers recursivos
`run_list_comp` y `run_map_comp`, ~150 LoC), types (refactor
ListComp + nuevo MapComp + helper check_comp_clause, ~90 LoC),
codegen (refactor gen_list_comp + nuevo gen_map_comp + helper
gen_comp_clause_header, ~250 LoC), fmt (~50 LoC), lint (~50
LoC), LSP (4 entries para Mb4).

**22 unit tests** evaluator (mb4_: unique str/orden, partition,
invert basic/dups, split_at basic/edge/neg; cmp_: multi-for
cartesian/filter, map comp basic/filter/dups, scope) + **9 unit
tests** checker (mb4: unique/partition/invert/split_at + errores;
cmp: multi-for tipa correctamente + map comp basic/filter type)
+ **4 unit tests** LSP (autocomplete incluye los 4 métodos
nuevos) + **8 compile_e2e** bit-a-bit `fitz run` ↔ `fitz build`
(mb4 los 4 métodos + cmp multi-for + cmp map comp + filter).
Ejemplo runnable `examples/guide/13p-mb4-y-comprehensions-extendidas.fitz`
sumado al smoke `GUIDE_EXAMPLES_COMPILE` con casos típicos.

Cap 13 de la guía: 1 fila nueva en tabla `Str` (split_at), 2 en
tabla `List<T>` (unique, partition), 1 en tabla `Map<K, V>`
(invert). Sub-sección dedicada "Dedup + partition + invert +
split_at + multi-for / Map comprehensions (mini-tanda Mb4 +
Cmp+)" con ejemplos inline. Cap 9 sub-sección "List
comprehensions" actualizada con la cobertura post-Cmp+ (multi-for
+ filter + map comprehensions). VSCode extension: grammar
TextMate sin cambios (`for`/`if` ya estaban como keywords; los
nuevos métodos son identifiers genéricos); LSP autocomplete
refleja todo automáticamente via rebuild del `fitz-lsp` binary.

**Deuda residual menor** (NO bloquea):
- Set comprehensions (`{x for x in xs}` sin `:`) — Fitz no tiene
  un tipo Set built-in todavía, sin sentido habilitarlo aislado.
- Filters intercalados entre `for` clauses
  (`[x for x in xs if x > 0 for y in ys if y < 10]`) — Python lo
  permite; aceptamos UN filter al final (decisión de diseño MVP
  para mantener el parser simple).
- Async comprehensions (`async for x in async_iter`) — sin
  motivación real hoy.

### ~~Codegen polish: higher-order callbacks por nombre + F12 caveat fix~~ ✓ CERRADO 2026-05-19 (mini-tanda Cd)

Bundle de polish del codegen que cierra las dos limitaciones más
visibles heredadas del compilador. Ambas afectaban el day-to-day
y obligaban a workarounds en cada call site.

- ~~**Higher-order callbacks por nombre**~~ ✓ `gen_callback_inline`
  y `gen_binary_callback_inline_with_ret` ahora aceptan también
  `Expr::Ident(name)` cuando refiere a una fn top-level (registrada
  en `fn_sigs`) o a una var local con tipo `Function`. Nuevo helper
  `resolve_named_callback(name)` consulta ambas fuentes. Validaciones
  estructurales (aridad de la fn, compatibilidad de param y ret
  type) ya las hace el checker estático con `check_unary_callback` /
  `check_binary_callback`; el codegen las replica como back-stop
  defensive. El código emitido para `xs.map(double)` es `xs.iter()
  ...map(double)` Rust directo — las fn items de Rust implementan
  `Fn`, así que esto compila sin boxing. Cubre `map`/`filter`/
  `find`/`any`/`all`/`count`/`find_index`/`flat_map`/`reduce`/
  `sort_by` (List) y `filter`/`map_values`/`update` (Map) — todos
  los métodos con callback.

- ~~**F12 caveat fix: `let X = <const-eval>` accesible desde fns
  top-level**~~ ✓ El codegen ahora detecta `let X = <const-eval>`
  top-level del archivo principal que son referenciados por al
  menos una fn top-level y los "hoistea" a `const X: T = ...;`
  (Int/Float/Bool) o `static X: &str = "...";` (Str literal) en el
  output Rust. El binding hoisteado es accesible desde cualquier
  fn del programa generado. **Reglas**:
  - Solo RHS const-eval (literal o BinOp/UnaryOp puros sobre
    operands const-eval) o Str literal directo se hoistan.
  - Solo aplica si el name NO se reasigna en `main_stmts` (el
    helper `collect_f12_hoists` filtra por count).
  - El name DEBE ser referenciado por alguna fn top-level —
    si no, queda como local de `main()` y el path tradicional
    funciona.
  - Modo CLI únicamente. En modo HTTP, `detect_shared_state`
    + `thread_local` ya cubre el patrón con semántica distinta
    (state mutable compartido).
  - El lookup en `gen_expr Ident` chequea `hoisted_main_lets`
    ANTES de fallar con "variable desconocida"; resuelve al
    ident Rust directo (`String::from(NAME)` para Str, `NAME`
    para primitivos). Mismo patrón que `own_consts` en módulos.

  **Reusa la lógica** de `gen_module_top_let` (que ya hacía esto
  para módulos): mismo subset de tipos soportados, misma emisión
  Rust (const vs static). El campo nuevo del `CodegenCtx` es
  `hoisted_main_lets: HashMap<String, Type>`.

Implementación: ~250 LoC entre codegen (`collect_f12_hoists` helper
free, `gen_main_hoisted_let` y `resolve_named_callback` en
`CodegenCtx`, ramas nuevas en `gen_callback_inline` y
`gen_binary_callback_inline_with_ret`, skip en `gen_main`, llamada
desde `emit_main_rs_body` antes de top_fns) + dep `is_compatible`
expuesto en el use de `crate::types`.

**11 unit tests** del codegen (`cd_ho_*`: pasar fn nombrada a
map/filter/reduce, errores claros del checker para fn inexistente/
aridad/ret incompatible; `cd_f12_*`: hoist Int/Float/Str/const-eval
BinOp, no-hoist cuando no se referencia o cuando se reasigna) +
**8 compile_e2e** bit-a-bit `fitz run` ↔ `fitz build` (map+filter+
reduce con fn nombrada, const Int/Str/Float/const-eval, combinado).
Ejemplo runnable `examples/guide/13o-higher-order-y-consts-globales.fitz`
sumado al smoke `GUIDE_EXAMPLES_COMPILE` con caso típico.

Cap 13 de la guía: sub-sección dedicada "Higher-order por nombre +
constantes globales (mini-tanda Cd)" con ejemplos inline + reglas
del hoist + caveats documentados (RHS runtime, reasignación,
modo HTTP). VSCode extension: grammar TextMate sin cambios; LSP
ya sugería las fns top-level via scope-level completions.

**Deuda residual menor** (NO bloquea):
- `let X = <expr-runtime>` (call a fn, struct lit, list lit) sigue
  sin hoistar — el binding no es const Rust válido. Para soporte
  completo haría falta `LazyLock<X>` o un init en main que se
  exporta. Sub-paso futuro si aparece presión.
- Reasignación de globales hoisteados requiere `static mut` (unsafe)
  o `Atomic*`/`Mutex<T>`. Decisión de diseño abierta — por ahora
  el workaround es pasar como param.
- Closures inline que **capturan** vars locales del scope contenedor
  son otro camino del codegen (F12 original); este Cd no las toca.
  Las closures siguen funcionando como antes.

### ~~Métodos funcionales: fold + product + chars + entries + to_map~~ ✓ CERRADO 2026-05-18 (mini-tanda Mb3)

Bundle siguiente al de Mb2, completa la API canónica "functional
collections" con métodos pedidos del prompt anterior (`List.reduce`,
`List.product`, `Str.chars`, `Map.entries`, `List.to_map`):

- ~~**`List.reduce(init, fn(acc, x) -> Acc) -> Acc`**~~ ✓ Fold
  canónico. El callback es binario y `Acc` puede ser de cualquier
  tipo (no necesariamente igual al de los elementos). Evaluator
  async invoca el callback con `invoke_value`; codegen reusa
  `gen_binary_callback_inline_with_ret` (refactor del helper: el
  caller ahora puede pasar `expected_ret_ty` explícito, antes era
  fijo por nombre del método). Vacía → devuelve `init`.
- ~~**`List.product()`**~~ ✓ Análogo a `sum`. Solo sobre Int/Float
  homogéneo. Vacío → `Int(1)` sentinel (paralelo a Python
  `math.prod([])`). Reusa el helper `require_numeric_list` de Mb2.
- ~~**`Str.chars() -> List<Str>`**~~ ✓ Cada char del string como
  `Str` de 1 caracter. Unicode-aware (cuenta chars, no bytes).
  Habilita pipelines como `s.chars().count(fn(c) => ...)`.
- ~~**`Map.entries() -> List<(K, V)>`**~~ ✓ Paralelo a Python
  `dict.items()` / JS `Object.entries`. Preserva insertion order.
- ~~**`List<(K, V)>.to_map() -> Map<K, V>`**~~ ✓ Inversa de
  `entries()`. Last-write-wins en duplicados (paralelo a Python
  `dict(items)`). El checker requiere `T == Tuple<K, V>` con
  aridad 2; cualquier otro tipo → error.

**Refactor incidental**: `gen_binary_callback_inline` ahora delega
a `gen_binary_callback_inline_with_ret` que recibe el ret type
explícito. Cierra el TODO marcado en el código ("Sub-paso futuro
si llegamos a 4+ callers: pasar `expected_ret_ty` como param
explícito").

Implementación: ~250 LoC entre evaluator (5 fns nuevas:
`list_reduce`/`list_product`/`list_to_map`/`map_entries`/`str_chars`),
types (5 ramas nuevas en infer_list/map/str/method), codegen
(5 ramas nuevas + refactor del helper), LSP (5 entries nuevos).

**12 unit tests** del evaluator + **9 unit tests** del checker +
**3 unit tests** del LSP + **7 compile_e2e** bit-a-bit `fitz run`
↔ `fitz build` (incluye round-trip `entries().to_map()` que valida
preservación de orden + último valor en duplicados). Ejemplo
runnable `examples/guide/13n-reduce-product-chars-entries-to-map.fitz`
sumado al smoke `GUIDE_EXAMPLES_COMPILE`.

Cap 13 de la guía: 3 filas nuevas en tabla `List<T>` (product/
reduce/to_map), 1 en tabla `Str` (chars), 1 en tabla `Map<K, V>`
(entries). Sub-sección dedicada "Fold + product + chars + entries
+ to_map (mini-tanda Mb3)" con ejemplos inline + caso canónico
de pipeline (`scores.entries().reduce(...)`). VSCode extension:
grammar TextMate sin cambios (métodos comparten patrón general de
identifiers); LSP autocomplete refleja todo automáticamente vía
rebuild del `fitz-lsp` binary.

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
- ~~**Range exacto en respuestas Hover/Definition**~~ ✓ CERRADO
  2026-05-20 (mini-tanda LSPy). Resuelto sin refactor del AST:
  los helpers del LSP leen el source text + el span (start) para
  computar el end del ident extrayendo el run alphanumérico
  adyacente. Aplicable también a Diagnostics. Range exacto para
  Expr compuestos (call chains `a.b().c()`) sigue como deuda
  menor — el ident-bajo-cursor cubre 90% de los casos.
- ~~**Scope-aware autocomplete**~~ ✓ CERRADO 2026-05-20
  (mini-tanda LSPy.4). `collect_local_bindings_at` recorre el
  AST con cursor_line awareness, sumando params de fn, vars de
  for, y lets previos del bloque al scope visible. No requirió
  refactor del checker — el walker AST es suficiente.
- ~~**Cross-module go-to-definition**~~ ✓ CERRADO 2026-05-20
  (mini-tanda LSPx). F12 sobre `User` en `from foo import User`
  ahora salta al `type User { ... }` real adentro de foo.fitz,
  no al stmt de import local. Solo `file://` URIs soportados —
  URIs virtuales (`untitled:`, `inmemory:`) son edge case.
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

## ~~R.bug-deadlock — Deadlock en string interp con re-locks del mismo Mutex~~ ✓ CERRADO 2026-05-21

> **CERRADO el mismo día del descubrimiento (2026-05-21)** — el fix
> aterrizó en `gen_str_interp` (codegen). El test de regresión vive
> en `tests/compile_e2e.rs::r_bug_deadlock_str_interp_re_lock_mismo_arc_no_cuelga`
> y valida tanto el exit code (no timeout) como el output esperado.
>
> El cli-tool boilerplate compiló y corrió bit-a-bit como esperado
> con el código original limpio (sin workaround inline). Smoke
> `GUIDE_EXAMPLES_COMPILE` con los 78 ejemplos guide sigue verde
> (~123s), incluyendo `30-cron-background.fitz` que tiene el
> patrón típico de print con dos calls dentro.

**Prioridad: ALTA** — produce hang silencioso del binario sin
panic ni error visible. Difícil de diagnosticar porque el `fitz
check` y el `cargo build` pasan limpio.

### Descripción

`fitz build` emite código que puede causar **deadlock** cuando
una interpolación de string contiene **dos accesos al mismo
`Arc<Mutex<T>>`** vía `.lock()` adentro del mismo `format!(...)`.

**Repro mínimo**:

```fitz
type Sale { product: Str, amount: Float, region: Str }

let SALES: List<Sale> = [
    Sale { product: "a", amount: 1.0, region: "AR" },
    Sale { product: "b", amount: 2.0, region: "AR" },
]

fn total(sales: List<Sale>) -> Float {
    return sales.reduce(0.0, fn(acc: Float, s: Sale) => acc + s.amount)
}

fn by_region(sales: List<Sale>, region: Str) -> List<Sale> {
    return sales.filter(fn(s: Sale) => s.region == region)
}

let subset: List<Sale> = by_region(SALES, "AR")
// HANG: subset.len() lockea subset.0, total(subset) re-lockea adentro.
print("{subset.len()} - {total(subset)}")
```

El `fitz run` ejecuta esto OK (intérprete usa `parking_lot::Mutex`
que recursiva o detecta y panic). El `fitz build` emite código
con `std::sync::Mutex` que es **NO re-entrant** — el segundo
`.lock()` desde el mismo thread hace **deadlock** (espera para
siempre).

### Código generado problemático

Para `print("{subset.len()} - {total(subset)}")` el codegen emite:

```rust
println!("{}", format!("{} - {}",
    ((subset.clone()).lock().unwrap().len() as i64),   // ARG 1: lockea subset.0
                                                        //        MutexGuard vive hasta
                                                        //        el final de la statement
    total(subset.clone())                               // ARG 2: total() lockea subset.0
                                                        //        de nuevo → DEADLOCK
));
```

Rust mantiene los temporales (MutexGuard de ARG 1) vivos hasta
el final de la **statement** que los contiene. Mientras evalua
ARG 2, ese guard sigue vivo. La llamada a `total()` adentro
intenta `.lock()` sobre el mismo Arc y queda bloqueada esperando
que ARG 1 libere — pero ARG 1 no libera hasta que la statement
entera termine.

### Por qué solo afecta codegen, no intérprete

| Runtime | Mutex type | Re-entrancy |
|---|---|---|
| Intérprete (`fitz run`) | `parking_lot::Mutex` | NO re-entrant, pero `try_lock` retorna `WouldBlock` (no deadlock); F17 lo evita con "lock scope mínimo + clone-out" en el evaluator |
| Codegen (`fitz build`) | `std::sync::Mutex` | NO re-entrant, segundo `.lock()` **deadlock** silencioso |

El intérprete está diseñado para soltar el lock entre operaciones
(política de F17). El codegen NO hace eso explícitamente — confía
en el scope de Rust para dropear los guards, pero el scope de
temporales en `format!(...)` extiende los guards más allá de lo
necesario.

### Workaround del usuario

Romper la interpolación con bindings intermedios:

```fitz
// En lugar de:
print("{subset.len()} - {total(subset)}")

// Hacer:
let count: Int = subset.len()
let amount: Float = total(subset)
print("{count} - {amount}")
```

Cada `let` cierra una statement → el MutexGuard se dropea ANTES
del siguiente lock. Sin deadlock.

### Fix proposed en el codegen

Dos opciones:

**Opción A — Bindings intermedios automáticos en `gen_str_interp`**.
Cuando un fragment requiere `.lock()` (field access sobre Nominal,
`.len()` sobre List/Map, etc.) Y el `format!` tiene >1 fragment,
emitir cada fragment como `let __arg_N = <expr>;` ANTES del
`format!`. Eso fuerza scope-end de cada MutexGuard entre args.

```rust
let __arg1 = ((subset.clone()).lock().unwrap().len() as i64);  // guard dies aquí
let __arg2 = total(subset.clone());                             // sin conflict
println!("{}", format!("{} - {}", __arg1, __arg2));
```

**Opción B — Migrar el codegen a `parking_lot::Mutex`**. Pesa
una dep nueva en los binarios producidos pero acaba con el
problema de raíz. Mantiene la dep ya usada en el intérprete.
Trade-off: tamaño del binario (~50 KB) por simplicidad del fix.

Lean: **Opción A** porque es local al codegen sin nueva dep en
los binarios.

### Tests de regresión cuando se cierre

- Repro mínimo (de arriba) debe correr sin colgar.
- `examples/guide/13-metodos.fitz` con `print("{xs.len()} - {xs.first()}")`-style.
- Test E2E nuevo en `compile_e2e.rs`: timeout 5s, fail si el
  binario tarda más.
- Smoke `GUIDE_EXAMPLES_COMPILE` debe seguir verde.

### Descubrimiento

Encontrado al validar el primer boilerplate Dockerizado
(`boilerplates/cli-tool/`). El programa generaba un report de
ventas con `print("  {region}: {subset.len()} ventas, ${total(subset):.2f}")`
adentro de un `for region in distinct_regions(SALES)`. El binario
imprimía "Por región:" y se cortaba sin error. Diagnosticado
aislando con tests minimales (`/tmp/test_iter3.fitz` ... `/tmp/test_iter7.fitz`).

El boilerplate cli-tool tiene workaround inline mientras el fix
no está aplicado.

---

## R.missing-recursive-instance-coercion — Coerción `Map → Instance` no es recursiva sobre `List<T>`/`Map<K, V>` (descubierto 2026-05-22)

**Prioridad: MEDIA** — no rompe runtime, pero fuerza al usuario a
escribir boilerplate (loop manual) cada vez que recibe una colección
de dicts desde Python (`json.loads` de un array JSON, queries
SQLAlchemy que devuelven `List[dict]`, etc.).

### Descripción

La coerción runtime introducida en Fase 8.4.3 (`coerce_to_annotation`)
convierte un `Value::Map` en `Value::Instance` cuando el binding
tiene anotación nominal:

```fitz
let raw = db.get_user(42)?            // raw: Map<Str, Any>
let u: User = json.loads(raw_json)?   // ✓ coerce Map → Instance User
```

Pero la coerción **NO es recursiva**: si el value es `List<Map>` y
la anotación es `List<User>`, los items adentro NO se coercen
automáticamente.

```fitz
let raw = db.list_users()?            // raw: Str (JSON array)
let users: List<User> = json.loads(raw)?   // ✗ users es List<Map>,
                                            //   NO List<User>
```

El binding pasa el type check (porque `List<Map>` es compatible con
`List<User>` vía `Any`), pero los items SIGUEN siendo Maps en
runtime. Si después hacés `users.find(fn(u) => u.name == "x")`,
el callback recibe un Map y `u.name` falla con "Map no tiene field
name" o similar.

Lo mismo aplica a `Map<K, V>` cuando los values son Maps que
deberían coercerse a Instance.

### Workaround actual (loop manual)

Iterar la colección y coercer item por item con un binding
intermedio que sí dispare 8.4.3:

```fitz
let raw = db.list_users()?
let maps: List<Any> = json.loads(raw)?
let users: List<User> = []
for m in maps {
    let u: User = m       // ← acá dispara la coerción Map → User
    users.push(u)
}
```

Aplicado en el 6to boilerplate (`api-fullstack-postgres/src/main.fitz`)
en `list_tasks`. Sin este loop, el frontend recibía una colección
de Maps en lugar de Tasks tipados, y `tasks.filter`/`tasks.map`
fallaban porque el JSON output era double-encoded (devolver
`Result<Str>` con JSON crudo lo wrappea otra vez al serializar
la response HTTP).

### Fix propuesto

Extender `coerce_to_annotation` para que, cuando la anotación
destino es `List<T>` con `T = Named(N)` y el value es
`Value::List(items)`, recurse sobre cada `item` con anotación `T`.
Idem para `Map<K, V>` cuando el value es `Value::Map`.

Esquema en el evaluator:

```rust
fn coerce_to_annotation(annot: &TypeExpr, value: Value, ...) -> Value {
    match annot {
        // Caso ya implementado (8.4.3):
        TypeExpr::Named(t) if is_nominal(t) => coerce_map_to_instance(value, t),

        // NUEVO: recursión sobre List<T>.
        TypeExpr::Generic("List", [inner]) if is_nominal_or_nullable_nominal(inner) => {
            if let Value::List(items) = value {
                let coerced: Vec<_> = items.into_iter()
                    .map(|item| coerce_to_annotation(inner, item, ...))
                    .collect();
                Value::List(coerced)
            } else {
                value
            }
        }

        // NUEVO: recursión sobre Map<K, V> con V nominal.
        TypeExpr::Generic("Map", [_, value_ty]) if is_nominal(value_ty) => { ... }

        _ => value,
    }
}
```

### Impacto sobre 8.7 (codegen)

Cualquier fix del runtime necesita su contraparte en `fitz build`
(deuda **R.bug-8.7-coercion-list-codegen**, ya documentada).
El helper `__fitz_py_to_list_*` emitido por el preludio Python
codegen existe pero todavía no wirea con `coerce(PyAny → List<T>)`.
Fix coordinado: cerrar primero esta deuda en el intérprete (más
chico), después cerrar 8.7 en codegen referenciando la lógica del
runtime.

### Por qué no es bug crítico

- El type check estático NO atrapa el problema (es compatible con
  `List<Any>`), por eso "no rompe la build". Pero el runtime falla
  más tarde con error confuso (`Map has no field 'name'` en lugar
  de un mensaje claro de "no se pudo coercer").
- El workaround es bien claro (loop manual) y compacto (~5 LoC).
- Aparece solo en proyectos con interop Python que devuelven
  colecciones. Para handlers HTTP típicos que reciben body
  deserializado (no Python interop) no aplica.

### Tracking

Sin sub-fase asignada hoy. Cuando aparezca presión real (más
proyectos con SQLAlchemy/Pandas que devuelvan colecciones), se
asigna a una sub-fase. Por ahora documentado como
**R.missing-recursive-instance-coercion** y se aplica el
workaround.

---

## ~~R.bug-options-preflight-shared-path — OPTIONS preflight duplicado cuando varios handlers comparten path~~ ✓ CERRADO 2026-05-22

> **CERRADO el mismo día del descubrimiento (2026-05-22)** — el fix
> aterrizó en `src/http.rs::build_router_with_asyncapi` (intérprete)
> y `src/codegen.rs` (codegen, paridad). Los 4 tests de regresión
> viven en `src/http.rs::tests::bug_options_preflight_duplicado_*` y
> el E2E del codegen en
> `tests/compile_e2e.rs::r_bug_options_preflight_duplicado_en_fitz_build_paridad_con_fitz_run`.
>
> El 6to boilerplate (`api-fullstack-postgres`) destrabó la
> validación end-to-end y motivó el descubrimiento.

**Prioridad: ALTA** — produce panic visible en boot con mensaje
oscuro ("Overlapping method route. Handler for `OPTIONS /tasks`
already exists") que confunde al usuario porque `fitz check` y la
build pasan limpio.

### Descripción

Cuando dos o más handlers HTTP comparten el mismo path con CORS
declarado en cada uno (caso típico CRUD: `/tasks` con `@get` +
`@post`, o `/tasks/{id}` con `@get` + `@put` + `@delete`), cada
handler intentaba registrar su propio OPTIONS preflight para el
path. axum hace panic al construir el `Router` con:

```
Overlapping method route. Handler for `OPTIONS /tasks` already exists
```

El bug se manifestaba tanto en `fitz run` (al construir el Router
en `build_router_with_asyncapi`) como en `fitz build` (el binario
emitido también paniqueaba en boot).

### Repro mínimo

```fitz
@server(3000) fn main() => 0

@middleware(cors({"allow_origin": "http://localhost:8080", "allow_methods": ["GET", "OPTIONS"]}))
@get("/tasks")
fn list_tasks() -> Str => "[]"

@middleware(cors({"allow_origin": "http://localhost:8080", "allow_methods": ["POST", "OPTIONS"]}))
@post("/tasks")
fn create_task() -> Str => "created"
```

Pre-fix: el server panic al arrancar con el mensaje arriba (ambos
en `fitz run` y `fitz build`). Post-fix: arranca limpio, el
preflight unificado advierte `GET, POST, OPTIONS` en
`Access-Control-Allow-Methods`.

### Causa raíz

El comment original en `build_router` decía explícitamente:

> Por ahora cada (path, method) registra su MethodRouter directo —
> si hay dos métodos sobre el mismo path con CORS, el preflight
> termina sumado a la segunda ruta. Aceptable para MW.2; revisitable.

Pero el comportamiento real era: el segundo `router.route(path, ...)`
con `.options(preflight)` colisionaba con el OPTIONS ya registrado
por el primer handler → panic en el merge.

### Fix aplicado

**Intérprete (`src/http.rs::build_router_with_asyncapi`)**:

1. Pre-cómputo de `merged_cors_per_path: HashMap<String, CorsConfig>`:
   recorre las metas, mergea las `CorsConfig` de handlers que
   comparten path (unión de `allow_methods` preservando orden, unión
   de `allow_headers` case-insensitive, max de `max_age`, primer
   `allow_origin` gana).
2. `preflight_attached: HashSet<String>` tracking — primer handler
   con CORS de cada path emite el `attach_preflight` con el config
   MERGED; subsequent skipean.
3. Helper nuevo `fn merge_cors_into(existing: &mut CorsConfig, other: &CorsConfig)`.

**Codegen (`src/codegen.rs`)**:

1. Nuevos campos en `CodegenCtx`: `cors_merged_per_path` +
   `cors_preflight_owner` (primer handler de cada path con CORS).
2. Pre-scan `precompute_cors_merge(http_fns)` corrido ANTES del loop
   de wrappers en `generate_project`.
3. `emit_cors_helpers(sig)` solo emite `__cors_resolve_<NAME>` +
   `__preflight_<NAME>` para el OWNER del path.
4. Nuevo método `cors_resolve_fn_for(sig)` que devuelve el nombre
   del resolver del owner — los wrappers de los non-owners
   referencian el resolver compartido.
5. Route loop en `gen_http_main` solo añade `.options(__preflight_<OWNER>)`
   al `.route(...)` del owner; non-owners hacen `.route(...)` sin
   `.options(...)`. axum mergea verbos por path naturalmente.
6. Helper free `merge_build_cors_into` paralelo a `merge_cors_into`
   del runtime.

### Tests de regresión

- **`src/http.rs::tests::bug_options_preflight_duplicado_no_panicea_en_build_router`** — smoke: build_router no panic con 2 handlers compartiendo `/tasks`.
- **`bug_options_preflight_duplicado_merged_methods_en_preflight_response`** — `Access-Control-Allow-Methods` incluye GET + POST + OPTIONS al consultar OPTIONS /tasks.
- **`bug_options_preflight_duplicado_tres_handlers_con_path_id`** — caso del 6to boilerplate con `/tasks/{id}` + GET/PUT/DELETE.
- **`bug_options_preflight_duplicado_merge_de_headers_case_insensitive`** — dedup case-insensitive de allow_headers (`Content-Type` vs `content-type`).
- **`tests/compile_e2e.rs::r_bug_options_preflight_duplicado_en_fitz_build_paridad_con_fitz_run`** — E2E que buildea el binario, lo spawnea, hace OPTIONS /tasks vía raw TCP y valida que el preflight responde 204 + methods merged.

### Lo que el lenguaje aprende del bug

Política de merge documentada como contrato del lenguaje: handlers
que comparten path con CORS deberían declarar el mismo `allow_origin`
(si discrepan, el primero gana sin warning); los `allow_methods` y
`allow_headers` se unen. Si en el futuro queremos un warning de
"discrepancia de allow_origin entre handlers del mismo path", se
agrega en el checker estático.

---

## R.bug-8.7-coercion-list-codegen — Coerción `list`/`dict` Python → `List<T>`/`Map<K,V>`/`Instance` Fitz en codegen (deuda residual de Fase 8.7)

**Prioridad: ALTA** (post-boilerplates) — limita el patrón
"endpoint que devuelve `Result<List<T>>` con T nominal" desde
interop Python en `fitz build`. Hoy el workaround es devolver
JSON crudo (`Result<Str>`) que el cliente HTTP doble-parsea —
funcional pero feo.

### Estado actual

Cierre formal de Fase 8.7 (CHANGELOG v0.8.8) marca como deuda
residual:

> Coerción Python `list/dict` → Fitz `List<T>`/`Map<K,V>`/`Instance`
> en `fitz build` (helpers `__fitz_py_to_list_*` ya emitidos, falta
> wiring en `coerce`); `.await` con binding intermedio split;
> trait `__FitzFromPy` simétrico.

Los helpers `__fitz_py_to_list_int`, `__fitz_py_to_list_string`,
etc. **ya están emitidos** en el preludio HTTP del codegen
(`src/codegen.rs` líneas ~3960-3982). Falta el wiring en `coerce`
y la versión para `List<NominalT>` (que requiere también coerción
recursiva `dict` → `Instance` por field).

En el **intérprete (`fitz run`)** la coerción 8.4.3 (`Map` →
`Instance` con anotación nominal) ya funciona para items
individuales. Lo que falta:
- Iterar un `list` Python y coercear cada item a `Instance` Fitz.
- Empaquetar el resultado como `List<T>` Fitz nativa (no opaca).

### Repro

```fitz
type User { id: Int, name: Str, email: Str }
from python import db   // db.list_users() devuelve list[dict]

// Esto FALLA en codegen (deuda 8.7) y en intérprete (no hay
// coerción recursiva todavía):
@get("/users")
fn list_users() -> Result<List<User>> {
    let users: List<User> = db.list_users()?   // ← deuda
    return Ok(users)
}

// Workaround actual (api-postgres-python):
@get("/users")
fn list_users() -> Result<Str> {
    let raw = db.list_users()?
    return Ok(json.dumps(raw)?)   // string-doble-escapeado al cliente
}
```

Output del workaround:
```bash
$ curl localhost:3000/users
"[{\"id\":1,\"name\":\"Ada\",...}]"     # quotes + escapes
```

Lo esperado (cuando 8.7 cierre):
```bash
$ curl localhost:3000/users
[{"id":1,"name":"Ada",...}]              # JSON object limpio
```

### Fix proposed

**Sub-paso 8.7.bis — coerción list/dict completa**:

1. **Wiring de `coerce(PyAny → List<T>)` en codegen**: cuando
   el destino es `List<T>` con T primitivo, usar los helpers
   `__fitz_py_to_list_<T>` que ya existen. ~30 LoC en
   `src/codegen.rs::coerce`.

2. **Coerción recursiva `dict` → `Instance` en codegen** (para
   T nominal): emit un helper que itera los fields del `type
   <T>` y extrae cada uno del dict Python via `.get_item(key)`.
   Reusa `__FitzFromPy` (deuda hermana) si se implementa
   simétrico a `__FitzToPy`.

3. **Coerción `dict` → `Map<K,V>`** (cuando V es primitivo y K
   es Str): paralelo a (1) pero para Map.

4. **Soporte en intérprete**: `eval_coerce_to_annotation`
   (post-8.4.3) extiende para casos `List<T>` y `Map<K,V>` con
   item Python opaco. Iterar + coercer item por item.

5. **Tests**: round-trip Python `list[dict]` → Fitz `List<User>`
   tipada → JSON limpio al cliente HTTP. E2E del boilerplate
   `api-postgres-python` con el handler `list_users` retornando
   `Result<List<User>>` sin workaround.

### Tests de regresión cuando se cierre

- Repro arriba debe compilar y correr en `fitz run` Y `fitz build`.
- Boilerplate `api-postgres-python` refactorizado a usar
  `Result<List<User>>` directo en `list_users` sin workaround.
- `examples/python-interop-8.7.fitz` actualizado con el caso
  list/dict.
- Smoke `GUIDE_EXAMPLES_COMPILE` verde.

### Items vinculados

- `boilerplates/api-postgres-python/src/main.fitz` — `list_users`
  devuelve `Result<Str>` por workaround. Cuando 8.7.bis cierre,
  cambiar a `Result<List<User>>`.
- `boilerplates/api-fullstack-postgres` (6to boilerplate) — el
  endpoint `GET /tasks` tiene el mismo problema; probable que
  use mismo workaround o que se intente la iteración manual
  con json.loads + map + anotación nominal en callback (que
  parece funcionar en intérprete según 8.4.3, no validado E2E).
- `.await` con binding intermedio split (deuda hermana 8.7).
- Trait `__FitzFromPy` simétrico (deuda hermana 8.7).

---

## R.bug-loader-relative-only — Loader Fitz resuelve `from sub.foo` solo relativo al importer, no al root (descubierto 2026-05-22)

**Prioridad: MEDIA** — limita la organización de proyectos
multi-archivo cuando un módulo en subcarpeta necesita importar
tipos definidos en otra subcarpeta hermana.

### Descripción

El loader de módulos de Fitz (`resolve_module_path` en
`src/evaluator.rs`) resuelve `from sub.foo import X` relativo al
**archivo importer**, NO al root del proyecto. Ejemplo:

```
src/
├── main.fitz           # from types.user import User → src/types/user.fitz ✓
├── types/
│   └── user.fitz
└── data/
    └── users.fitz      # from types.user import User → src/data/types/user.fitz ✗
                         # (busca relativo al importer)
```

Si `data/users.fitz` quiere importar `User` definido en
`types/user.fitz`, el loader busca en `src/data/types/user.fitz`
(que no existe) en lugar de `src/types/user.fitz`. Fail:

```
Error — no se encontró el módulo `types.user` (buscado en
`/app/src/data/types/user.fitz`)
```

### Semántica actual

```rust
// src/evaluator.rs::resolve_module_path
fn resolve_module_path(segments: &[String]) -> EvalResult<PathBuf> {
    let base = LOADER.with(|cell| {
        cell.borrow().as_ref().map(|l| l.base_dir.clone())
    });
    // base_dir = dir del archivo importer
    let mut path = base.unwrap_or_else(...);
    for (i, seg) in segments.iter().enumerate() {
        if i + 1 == n { path.push(format!("{}.fitz", seg)); }
        else { path.push(seg); }
    }
    Ok(path)
}
```

`base_dir` se setea al directorio del archivo que invocó el
import (relative). No hay concepto de "project root" que sirva
como base alternativa para imports absolutos.

### Workaround actual (a nivel boilerplate)

`boilerplates/api-postgres-python/`: el módulo `src/data/users.fitz`
NO importa tipos. Devuelve `Result<Str>` (JSON crudo) y el
`src/main.fitz` hace la coerción a `User` con `let u: User =
json.loads(s)?` (porque main vive en `src/` y SÍ puede importar
`from types.user import User` correctamente).

Trade-off: la responsabilidad de "saber qué shape devuelve la DB"
queda en main en lugar de en el data layer. Aceptable como
patrón mientras el loader no soporte imports absolutos.

### Fix proposed en el loader

Opciones evaluadas:

**A — Soporte imports absolutos relativos al project root**:
detectar `fitz.toml` walk-up desde el archivo importer (Cargo
style) y usar el dir del manifest como root. `from types.user`
resuelve a `<project_root>/src/types/user.fitz` siempre,
independiente del importer. Requiere bandera explícita en el
import (¿prefix?) o detección heurística.

**B — Imports relativos Python-style**: `from ..types.user import
User` (dos puntos = subir un nivel). Requiere cambio del parser
(tokenize `..` adentro de `from ... import`) y semántica del
loader.

**C — Convención + path en `fitz.toml`**: el manifest declara
`[paths]` con un alias `src = "src"` y el user usa `from src.types.user
import User`. Más explícito pero verboso.

Lean: **A** combinado con la convención actual. El loader detecta
si la primera segment matchea un directorio adentro del manifest
root → usa root. Si no, fallback a path relativo del importer.
Mantiene backward compat con código existente que usa imports
relativos al importer (cap 16 de la guía).

### Tests de regresión cuando se cierre

- `src/data/users.fitz` con `from types.user import User` debe
  resolver a `src/types/user.fitz` (no `src/data/types/user.fitz`).
- Programas con imports relativos al importer existentes (cap 16
  de la guía + `examples/guide/16-modulos.fitz`) siguen verdes.
- `examples/guide/16c-modulos-transitivos.fitz` con sub-carpetas
  sigue verde.
- Boilerplate `api-postgres-python` refactorizado a usar el
  patrón absoluto sin workaround.

### Items vinculados

- `boilerplates/api-postgres-python/src/data/users.fitz` usa
  workaround (devolver Str crudo, coerce en main). Cuando este
  fix cierre, refactorizar para que `data/users.fitz` devuelva
  `Result<User>` tipado y la coerción quede en data layer.

---

## R.bug-pyo3-abi3-autoinit — `abi3-py310` + `auto-initialize` incompatibles en Cargo.toml (descubierto 2026-05-22)

**Prioridad: MEDIA** — produce binarios `--features python`
acoplados a versión específica de libpython del builder, en lugar
del binario portable que `abi3-py310` promete. Bloquea el flow
"buildear en cualquier máquina, correr en cualquier Python 3.10+"
que el feature documenta.

### Descripción

El `Cargo.toml` del proyecto Fitz declara:

```toml
pyo3 = { version = "0.28", features = ["abi3-py310", "auto-initialize"], optional = true }
```

Según [docs de pyo3](https://pyo3.rs/), `auto-initialize` y `abi3`
son **mutuamente excluyentes**:

- `abi3-py310`: linkea contra `libpython3.so` (stable ABI),
  binario corre en cualquier Python 3.10+.
- `auto-initialize`: pyo3 spawnea el intérprete embedded al boot,
  requiere link contra una versión específica de libpython.

Cuando ambos features están activos, **`auto-initialize` gana** y
el binario producido linkea contra `libpython3.<X>.so.1.0`
específica de la versión detectada en el builder. El compromiso de
`abi3-py310` (binario portable) se pierde silenciosamente.

### Síntoma observado

Al buildear `boilerplates/api-postgres-python/` con la imagen
`rust:slim` (que trae Python 3.13 en Debian Trixie) y runtime
`python:3.12-slim`, el container falla al boot con:

```
fitz: error while loading shared libraries: libpython3.13.so.1.0:
cannot open shared object file: No such file or directory
```

Lo esperado con `abi3-py310` puro: el binario linkea contra
`libpython3.so` (stable ABI) y corre en Python 3.10/3.11/3.12/
3.13/3.14 indistintamente.

### Workaround actual (a nivel boilerplate)

`boilerplates/api-postgres-python/Dockerfile` usa la **misma
imagen Python** (`python:3.12-slim`) en builder y runtime. Builder
agrega Rust con `rustup`; runtime descarta Rust. Match garantizado
de libpython entre stages.

Trade-off: el builder ya no es la imagen oficial `rust:slim`
optimizada para builds Rust — el `apt-get install build-essential
+ rustup` lleva ~30s extras al build inicial. Aceptable.

### Fix proposed en el Cargo.toml de Fitz

Opciones evaluadas:

**A — Quitar `auto-initialize`**: el usuario llama a
`Python::initialize()` explícito antes del primer call PyO3. Más
boilerplate del lado Fitz pero recupera `abi3` real. Patrón usado
por el bin `fitz-lsp` (que no usa PyO3) y otros proyectos PyO3.

**B — Separar en dos features**: `python` (sin `auto-initialize`,
abi3 puro) + `python-embedded` (con `auto-initialize`, no abi3).
El user elige según deploy target. Más flexible pero duplica la
feature matrix.

**C — Mantener `auto-initialize` y aceptar deuda**: documentar
que `--features python` produce binarios acoplados a la versión
del builder. Match builder/runtime es responsabilidad del user
(como hace el boilerplate hoy).

Lean: **A**. El boot manual es 1 línea de Fitz al inicio del
programa (o transparente — el evaluator puede invocar
`Python::initialize` lazy al primer `from python import` sin
exponer al usuario). Recupera la promesa "un solo binario corre
contra cualquier Python 3.10+" que el feature publicita.

### Tests de regresión cuando se cierre

- Binario compilado en builder con Python 3.13 + corrido en
  runtime con Python 3.12 → arranca sin error.
- Mismo binario corrido en 3.10, 3.11, 3.14 → funciona.
- `examples/python-interop-8.1.fitz` + `examples/python-interop-8.6.fitz`
  + `examples/guide/21-python-crud/app.fitz` siguen verdes.

### Items vinculados

- `boilerplates/api-postgres-python/Dockerfile` usa workaround
  (match builder/runtime Python). Cuando este fix cierre,
  simplificar a `FROM rust:slim AS builder` (más rápido) +
  cualquier `python:3.X-slim` como runtime.
- Documentar en `docs/guide.md` cap 21 ("Interop Python") la
  decisión final sobre auto-initialize vs manual init.

---

## R.bug-result-status — Handler con return type `Result<T>` + `return <status> { ... }` mezclados (descubierto 2026-05-21)

**Prioridad: MEDIA** — produce serialización incorrecta del
response body (wrap `Ok`/`Err` en JSON), no es deadlock ni crash.
Detectado al validar `boilerplates/api-simple/`.

### Descripción

Cuando un handler HTTP declara return type `Result<T>` Y tiene un
`return <status> { ... }` (ReturnStatus) adentro del body, el
codegen override el return type Rust a `__FitzResponse` (para
acomodar el status code custom). Pero **NO desempaca el `Ok(v)`**
en los `return Ok(v)` explícitos — emite el Result wrapper
serializado completo.

**Repro mínimo**:

```fitz
type Item { id: Int, name: Str }
let ITEMS: List<Item> = [Item { id: 1, name: "a" }, Item { id: 2, name: "b" }]

@server(3000)
fn main() => 0

@get("/items/{id}")
fn get_item(id: Int) -> Result<Item> {
    let found: Item = match ITEMS.find(fn(it: Item) => it.id == id) {
        Ok(it) => it,
        Err(_) => return 404 { "error": "no encontrado" },
    }
    return Ok(found)   // ← serializa como {"Ok":{"id":1,...}} en lugar de {"id":1,...}
}
```

Output observado en `fitz build`:
- `GET /items/1` → `{"Ok":{"id":1,"name":"a"}}` (mal, debería ser
  `{"id":1,"name":"a"}`).
- `GET /items/99` → `{"error":"no encontrado"}` status 404 (OK,
  ese path va por el `return 404 { ... }`).

El `fitz run` (intérprete) sí desempaca el Result correctamente
— solo el codegen tiene el bug.

### Código generado problemático

```rust
fn get_item(id: i64) -> __FitzResponse {
    // ... match ITEMS.find(...) ...
    return __FitzResponse {
        status: 200,
        body: <Result<Item, String> as __ToFitzJson>::__to_fitz_json(
            &(Ok(found.clone()))
        )  // ← serializa Result<T> con wrapper
    };
}
```

Lo correcto sería:
```rust
return __FitzResponse {
    status: 200,
    body: found.__to_fitz_json()  // ← desempaca: serializa solo el T interno
};
```

### Workaround del usuario

**Opción A** (cleaner): cambiar el return type del handler a `T`
directo (no `Result<T>`) cuando se usa `return <status> { ... }`:

```fitz
@get("/items/{id}")
fn get_item(id: Int) -> Item {
    match ITEMS.find(fn(it: Item) => it.id == id) {
        Ok(it) => return it,
        Err(_) => return 404 { "error": "no encontrado" },
    }
}
```

**Opción B**: NO usar `return <status> { ... }`, solo `Result<T>`
con `Err(...)` (status 500 implícito) o `Err({ status: 404, ... })`
con field `status` (status code dinámico via mini-tanda HTTP-Err).

El boilerplate `api-simple` usa Opción A.

### Fix proposed en el codegen

En `emit_handler_dispatch_and_response` (o donde se decide el
return type override por ReturnStatus): cuando el handler tiene
return type Fitz `Result<T>` Y `return <status> { ... }` adentro,
el codegen debe:

1. Cambiar el return type Rust a `__FitzResponse` (ya hace eso).
2. En `gen_return` para `Stmt::Return(Ok(expr))`: emitir
   `return __FitzResponse { status: 200, body: <expr>.__to_fitz_json() }`
   (desempaca el Ok).
3. En `gen_return` para `Stmt::Return(Err(expr))`: emitir
   `return __FitzResponse { status: 500, body: json!({"error": expr}) }`
   (desempaca el Err con la convención HTTP-Err).
4. Cualquier otro `return v` que no sea `Ok(v)`/`Err(e)`: error
   claro pidiendo que el user use el patrón explícito.

### Tests de regresión cuando se cierre

- Repro mínimo arriba: `GET /items/1` debe devolver
  `{"id":1,"name":"a"}` status 200 (no `{"Ok":{...}}`).
- Test E2E nuevo en `compile_e2e.rs` con un handler `-> Result<T>`
  + `return <status>` mezclados.
- Smoke `GUIDE_EXAMPLES_COMPILE` debe seguir verde.

### Descubrimiento

Encontrado al validar `boilerplates/api-simple/` (2do boilerplate
Dockerizado). El handler `get_item(id: Int) -> Result<Item>` con
`return 404 { "error": "..." }` para el caso no-encontrado
devolvía `{"Ok":{"id":2,...}}` para `GET /items/2` (existente)
en lugar del Item directo. Workaround inline aplicado en
api-simple.

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
