# M2.C3 — Operadores y control de flujo (`if` / `else`)

**Pre-requisitos**: [M2.C2 — Variables](c2-variables.md).
Sabés declarar y reasignar, anotaciones, y la diferencia entre
scope de bloques vs. funciones.

**Objetivo**: dominar **todos los operadores de Fitz**
(aritmética, comparación, lógica, bitwise, asignación
compuesta) con sus reglas de precedencia y asociatividad, y el
control de flujo con `if`/`else if`/`else` — incluyendo **`if`
como expresión con valor**.

**Por qué importa**: hasta acá tus programas eran lineales —
secuencias de prints. Con `if` arrancan a **decidir**, y con
operadores arrancan a **calcular**. Conocer la precedencia te
evita bugs sutiles (`a + b == c` vs `a + (b == c)`).

---

## Mapa del cap

```mermaid
flowchart LR
    A[Operadores] --> B[Aritméticos<br/>+, -, *, /, %]
    A --> C[Comparación<br/>==, !=, <, >, <=, >=]
    A --> D[Lógicos<br/>and, or, not]
    A --> E[Bit-a-bit<br/>&, |, ^, ~, <<, >>]
    A --> F[Asignación compuesta<br/>+=, -=, *=, ...]
    G[Control de flujo] --> H[if / else if / else]
    G --> I[if como expresión<br/>con valor]
```

---

## Paso 1 — Operadores aritméticos

```fitz
print(2 + 3)       // 5    suma
print(10 - 4)      // 6    resta
print(6 * 7)       // 42   multiplicación
print(10 / 3)      // 3    división entera (Int / Int = Int)
print(10.0 / 3)    // 3.3333333333333335   división float
print(10 % 3)      // 1    módulo (resto)
print(-7)          // -7   unario negativo
```

### Tabla de operadores binarios numéricos

| Op | Nombre | Aplica a | Tipo del resultado |
|---|---|---|---|
| `+` | Suma | `Int`, `Float`, `Str` (concat) | Igual al input (con promoción) |
| `-` | Resta | `Int`, `Float` | Igual al input (con promoción) |
| `*` | Multiplicación | `Int`, `Float` | Igual al input (con promoción) |
| `/` | División | `Int`, `Float` | Int si ambos son Int (división entera); Float si alguno es Float |
| `%` | Módulo (resto) | `Int`, `Float` | Igual al input |

### Unarios

| Op | Nombre | Aplica a | Ejemplo |
|---|---|---|---|
| `-` | Negativo | `Int`, `Float` | `-5`, `-3.14` |

> **No hay `+` unario** explícito (sería no-op). Tampoco existe
> `++` (incremento) ni `--` (decremento) — usá `n += 1` / `n -= 1`.

### División entera vs flotante (importante)

**Cuando los DOS operandos son `Int`, `/` divide entero**:

```fitz
let a = 10 / 3        // 3 (Int, truncado)
let b = 10.0 / 3      // 3.3333... (Float)
let c = 10 / 3.0      // 3.3333... (Float)
```

Esta es **decisión deliberada de Fitz** — evita cambios
silenciosos del tipo del resultado.

| Operandos | Resultado | Tipo |
|---|---|---|
| `Int / Int` | División entera (truncada hacia 0) | `Int` |
| `Float / Int` | División flotante | `Float` |
| `Int / Float` | División flotante | `Float` |
| `Float / Float` | División flotante | `Float` |

### Promoción `Int` ↔ `Float`

En **operaciones mixtas**, el `Int` se promociona a `Float`
automáticamente:

```fitz
print(5 + 2.5)     // 7.5    (Int 5 → Float 5.0, luego 5.0 + 2.5)
print(2 * 1.5)     // 3.0
print(10 - 0.5)    // 9.5
```

### Lo que NO existe en el MVP

| Operador | Estado |
|---|---|
| `**` (potencia) | ❌ NO existe |
| `++`, `--` (incremento) | ❌ NO existe |
| `+` unario | ❌ NO existe (sería no-op) |

Para potencia, usá:
- `n * n * n` para potencias chicas.
- `<<` si es base 2 (`2 << 8` = 512 = 2⁹).
- Loop manual con `for in 1..=N`.
- Interop Python con `math.pow(...)`.

---

## Paso 2 — Comparación e igualdad

| Op | Significado | Tipo del resultado |
|---|---|---|
| `==` | Igual a | `Bool` |
| `!=` | Distinto a | `Bool` |
| `<` | Menor que | `Bool` |
| `>` | Mayor que | `Bool` |
| `<=` | Menor o igual | `Bool` |
| `>=` | Mayor o igual | `Bool` |

### Demo

```fitz
print(2 < 3)        // true
print(2 <= 2)       // true
print(2 > 3)        // false
print(5 == 5)       // true
print(5 != 5)       // false
print(5 == 5.0)     // true   ← Int ↔ Float comparan numéricamente
print(5 == "5")     // false  ← tipos distintos siempre son distintos
```

### Comparación de strings

Las strings comparan **lexicográficamente** (orden Unicode):

```fitz
print("a" < "b")        // true
print("ab" < "ac")      // true
print("abc" < "abd")    // true
print("z" < "aa")       // false ('z' > 'a' por codepoint)
print("Hola" < "hola")  // true  (mayúsculas vienen antes)
```

### Comparación con tipos distintos

| `a` | `b` | `a == b` | `a < b` |
|---|---|---|---|
| Int | Int | numérica | numérica |
| Float | Float | numérica (cuidado IEEE) | numérica |
| Int | Float | promoción + numérica | promoción + numérica |
| Str | Str | char-by-char | lexicográfica |
| Bool | Bool | OK (true≠false) | error |
| Int | Str | siempre `false` | error |
| Otros mixtos | — | `false` o error | error |

> **`!` NO es operador unario** en Fitz. Solo existe como parte
> de `!=`. Para negación lógica, usá la keyword `not` (Paso 3).

### Igualdad de tipos compuestos

Listas, mapas, y tipos custom (`type`) **comparan
estructuralmente**:

```fitz
print([1, 2, 3] == [1, 2, 3])    // true
print([1, 2] == [1, 2, 3])       // false (longitudes distintas)
print({"a": 1} == {"a": 1})      // true
```

Para `type` custom, dos instancias son iguales si **todos los
fields son iguales**:

```fitz
type Point { x: Int, y: Int }
let p1 = Point { x: 1, y: 2 }
let p2 = Point { x: 1, y: 2 }
print(p1 == p2)    // true
```

---

## Paso 3 — Operadores lógicos: `and`, `or`, `not`

**Estilo Python** (no C/Java):

| Op | Nombre | Aplica a | Ejemplo |
|---|---|---|---|
| `and` | AND lógico | `Bool` | `true and false` → `false` |
| `or` | OR lógico | `Bool` | `true or false` → `true` |
| `not` | Negación | `Bool` | `not true` → `false` |

```fitz
let logueado = true
let admin = false

print(logueado and admin)     // false
print(logueado or admin)      // true
print(not admin)              // true
```

### Equivalencias en otros lenguajes

| Fitz | JavaScript/Java/Rust | Python | Por qué |
|---|---|---|---|
| `and` | `&&` | `and` | Keywords más legibles |
| `or` | `\|\|` | `or` | Idem |
| `not` | `!` | `not` | Idem; `!` reservado para `!=` |

> **`&&` y `||` NO funcionan** en Fitz. Si vienen de
> JS/Rust/Java y los usás por costumbre, vas a tener errores de
> parser claros.

### Short-circuit (corto-circuito)

`and` y `or` **evalúan por la izquierda y cortan si pueden
decidir**:

```fitz
fn danger() -> Bool {
    print("evaluado!")
    return true
}

let safe = false
let result = safe and danger()   // ← danger() NUNCA se llama
                                 // (short-circuit en false and ...)
```

Salida: nada de `evaluado!`.

Para `or`:

```fitz
let happy = true
let result = happy or danger()   // ← danger() NUNCA se llama
                                 // (short-circuit en true or ...)
```

### Patrones canónicos con short-circuit

```fitz
// Evita NullPointerException-style
let safe_value = (usuario != null) and usuario.admin    // ⚠️ no funciona — usuario.admin solo si es Nullable
```

> **Nota**: en Fitz no hay refinement automático en `and` — el
> compilador no sabe que `usuario != null` adentro de `and`
> implica que `usuario.admin` es safe. Workaround: `if (usuario
> != null) { ... usuario.admin ... }` y refinement explícito.

---

## Paso 4 — Operadores bit-a-bit (sobre `Int`)

Operan a nivel de bits — útiles para flags, máscaras, IDs
comprimidos.

| Op | Nombre | Ejemplo | Resultado |
|---|---|---|---|
| `&` | AND bit a bit | `0xFF & 0x0F` | `15` (0b00001111) |
| `\|` | OR bit a bit | `0b1010 \| 0b0101` | `15` (0b01111) |
| `^` | XOR bit a bit | `0xFF ^ 0x0F` | `240` (0b11110000) |
| `<<` | Shift izquierda | `1 << 3` | `8` |
| `>>` | Shift derecha | `16 >> 2` | `4` |
| `~` | NOT bit a bit (complemento) | `~0` | `-1` (todos los bits prendidos en signed) |

```fitz
print(0xFF & 0x0F)        // 15
print(0b1010 | 0b0101)    // 15
print(0xFF ^ 0x0F)        // 240
print(1 << 3)             // 8
print(16 >> 2)            // 4
print(~0)                 // -1
```

### Cuándo usarlos

| Caso | Operador |
|---|---|
| Combinar flags (`READ \| WRITE`) | `\|` |
| Chequear flag (`flags & READ != 0`) | `&` |
| Toggle flag | `^` |
| Multiplicar por potencia de 2 | `<<` |
| Dividir por potencia de 2 (Int) | `>>` |

---

## Paso 5 — Asignación compuesta

Atajos para reasignar (la var debe existir y ser compatible):

| Op | Equivalente a |
|---|---|
| `+=` | `x = x + valor` |
| `-=` | `x = x - valor` |
| `*=` | `x = x * valor` |
| `/=` | `x = x / valor` |
| `%=` | `x = x % valor` |
| `&=` | `x = x & valor` (bitwise AND) |
| `\|=` | `x = x \| valor` (bitwise OR) |
| `^=` | `x = x ^ valor` (bitwise XOR) |
| `<<=` | `x = x << valor` (shift) |
| `>>=` | `x = x >> valor` (shift) |

Demo:

```fitz
let n = 10
n += 5     // n ahora 15
n -= 3     // n ahora 12
n *= 2     // n ahora 24
n /= 4     // n ahora 6
n %= 4     // n ahora 2

let flags = 0
flags |= 0x01    // set bit 0
flags |= 0x04    // set bit 2
print(flags)     // 5
flags ^= 0x01    // toggle bit 0
print(flags)     // 4
```

### Con strings (solo `+=`)

`+=` también funciona para strings (concatena):

```fitz
let saludo = "hola"
saludo += ", Patagonia"
print(saludo)     // hola, Patagonia
```

---

## Paso 6 — Tabla completa de precedencia

De **más alta** (binds más fuerte) a **más baja**:

| Nivel | Operadores | Asociatividad | Notas |
|---|---|---|---|
| 1 | `()` (agrupación), `[]` (indexing), `.` (field) | Izquierda | Highest |
| 2 | `-` unario, `not`, `~` | Derecha | Prefijos |
| 3 | `*`, `/`, `%` | Izquierda | Multiplicativos |
| 4 | `+`, `-` | Izquierda | Aditivos |
| 5 | `<<`, `>>` | Izquierda | Shifts |
| 6 | `&` | Izquierda | Bitwise AND |
| 7 | `^` | Izquierda | Bitwise XOR |
| 8 | `\|` | Izquierda | Bitwise OR |
| 9 | `<`, `>`, `<=`, `>=` | Izquierda | Comparación |
| 10 | `==`, `!=` | Izquierda | Igualdad |
| 11 | `and` | Izquierda | Lógico AND |
| 12 | `or` | Izquierda | Lógico OR |
| 13 | `..`, `..=` | — | Range (no se encadena) |

### Ejemplos de precedencia

```fitz
print(2 + 3 * 4)             // 14   (* > +, así que 3*4 primero)
print((2 + 3) * 4)           // 20   (paréntesis fuerza)
print(2 < 3 and 3 < 4)       // true (comparación > and)
print(true or false and false)   // true (and > or, así que false and false primero)
print(not true and false)    // false (not > and)
print(1 + 2 == 3)            // true (+ > ==)
print(5 + 3 - 2)             // 6    (izq-asoc: (5+3)-2)
print(10 - 3 - 2)            // 5    (izq-asoc: (10-3)-2, no 10-(3-2))
print(10 / 2 * 3)            // 15   (izq-asoc: (10/2)*3, no 10/(2*3))
```

> **En la duda, agregá paréntesis**. Es mejor verboso y
> correcto que sutilmente erróneo.

---

## Paso 7 — `if` / `else if` / `else`

Sintaxis básica:

```fitz
if (<cond>) {
    <body si true>
} else if (<otra cond>) {
    <body>
} else {
    <body si nada matcheó>
}
```

```fitz
let n = 7

if (n > 0) {
    print("positivo")
} else if (n < 0) {
    print("negativo")
} else {
    print("cero")
}
```

```
positivo
```

### Reglas sintácticas

| Regla | Por qué |
|---|---|
| Paréntesis OBLIGATORIOS en la condición | Evita ambigüedades con expresiones complejas |
| Bloque `{ }` OBLIGATORIO (no hay `if cond stmt`) | Consistencia, evita misreads |
| `else if` con dos palabras (no `elif`) | Estilo JavaScript/Java/Rust |
| `else` final opcional | Puede no estar |

### Lo que NO existe

- **`elif`** (estilo Python). Usá `else if`.
- **`if cond statement`** sin bloque. Bloque obligatorio.
- **Ternario `cond ? a : b`** explícito. Usá `if cond { a } else { b }` (Paso 8).

📚 **Detalle**: [cap 7 — if / else](../../guide.md#7-if--else)
de la guía.

---

## Paso 8 — `if` como expresión con valor

Esta es **la feature más importante de `if` en Fitz**. Un `if`
con ambas ramas que devuelven un valor **es una expresión
asignable**:

```fitz
let n = 5
let signo = if (n > 0) {
    "positivo"
} else if (n < 0) {
    "negativo"
} else {
    "cero"
}

print(signo)     // positivo
```

### Reglas

| Regla | Detalle |
|---|---|
| Las ramas terminan en **expresión** (sin `;`, sin `return`) | El valor de la expresión sale del bloque |
| Todas las ramas deben tener un valor | Sino, el binding es `Null` |
| Tipos de las ramas deben ser compatibles | Promoción Int→Float OK; ambos `T?` OK |
| `else` requerido para que el resultado sea no-nullable | Sin `else`, el resultado es `T?` |

### Ejemplos

```fitz
// Compatible: Int + Int → Int
let maximo = if (a > b) { a } else { b }       // Int

// Compatible: Int + Float → Float (promoción)
let avg = if (modo == "preciso") { (a + b) / 2.0 } else { a + b / 2 }

// Sin else → Nullable
let opcional = if (cond) { "value" }           // Str?
```

### Cuándo cada estilo

| Caso | Estilo | Por qué |
|---|---|---|
| Side effect (print, asignar, mutar) | `if` con `{}` y stmts adentro | No necesita valor |
| Calcular un valor | `let x = if (cond) { a } else { b }` | Más declarativo |

### Gotcha: `print(...)` adentro de la rama

`print()` devuelve `Null`. Si una rama termina en `print(...)`,
el binding queda `Null`:

```fitz
let x = if (cond) {
    print("ok")          // ← devuelve Null
} else {
    42
}
print(x)    // null o 42 según la cond
```

Para que `x` sea siempre `Int`, separá los stmts:

```fitz
if (cond) {
    print("ok")
}
let x = if (cond) { 0 } else { 42 }
```

---

## Paso 9 — Reglas combinadas: `and`/`or` con `if`

Las condiciones del `if` pueden ser cualquier expresión `Bool`:

```fitz
let n = 7
let admin = true

if (n > 0 and admin) {
    print("positivo y admin")
}

if (n > 100 or n < 0) {
    print("fuera de rango normal")
}
```

Combinaciones con negación:

```fitz
if (not (n == 0)) {
    print("distinto de cero")
}
```

> **Más legible**: `if (n != 0)` que `if (not (n == 0))`.
> Usá `!=`/`==` cuando aplica directo.

### Patrón "no es null y cumple X"

```fitz
let edad: Int? = obtener_edad()

if (edad != null and edad > 18) {       // ⚠️ no refina edad
    // ...
}
```

Mejor con `if` anidado para que el checker refine:

```fitz
if (edad != null) {
    if (edad > 18) {
        // ...
    }
}
```

---

## Paso 10 — Aplicarlo a `mi-saludos`

Hagamos algo que **decida**. Editá `src/main.fitz`:

```fitz
let nombre = "Patagonia"
let altitud_m = 350
let umbral_montana = 1000
let umbral_llanura = 200

// if como expresión: clasifica en una sola declaración
let categoria = if (altitud_m > umbral_montana) {
    "altura"
} else if (altitud_m > umbral_llanura) {
    "media"
} else {
    "llanura"
}

print("{nombre} ({altitud_m} m) clasifica como: {categoria}")

// Combinando operadores: lógica de campamento
if (altitud_m > umbral_llanura and altitud_m <= umbral_montana) {
    print("  → buen lugar para acampar")
}

// Calcular precio según condiciones
let temporada = "alta"
let descuento = 0.15
let precio_base = 100.0
let precio_final = if (temporada == "alta") {
    precio_base
} else if (temporada == "media") {
    precio_base * 0.9
} else {
    precio_base * (1.0 - descuento)
}
print("precio para temporada {temporada}: ${precio_final}")

// Bit flags para permisos
let READ = 0x01
let WRITE = 0x02
let EXECUTE = 0x04
let user_perms = READ | WRITE
print("user tiene READ? {(user_perms & READ) != 0}")
print("user tiene EXECUTE? {(user_perms & EXECUTE) != 0}")
```

```bash
fitz run
```

```
Patagonia (350 m) clasifica como: media
  → buen lugar para acampar
precio para temporada alta: $100
user tiene READ? true
user tiene EXECUTE? false
```

---

## Validación

- [ ] `print(10 / 3)` imprime `3` (división entera).
- [ ] `print(10.0 / 3)` imprime `3.33...` (división float).
- [ ] `not true` imprime `false`. `!true` da error de parser.
- [ ] `&&` y `||` dan error de parser; `and`/`or` funcionan.
- [ ] `if (n > 0) { ... } else { ... }` compila; sin paréntesis
      da error.
- [ ] `let signo = if (n > 0) { "pos" } else { "neg" }` bindea
      `signo` a un Str.
- [ ] El operador compuesto `n += 1` reasigna `n` a `n + 1`.
- [ ] `2 + 3 * 4` da `14` (precedencia `*` > `+`).
- [ ] `1 << 3` da `8`.

---

## Troubleshooting

### `error: '!' solo es válido como parte de '!='`

Usaste `!cond`. Cambialo a `not cond`.

### `error: Se esperaba una expresión, se encontró 'Amp'`

Usaste `cond1 && cond2`. Cambialo a `cond1 and cond2`. Idem
`||` → `or`.

### El `if` como expresión da `Null` en vez del valor que esperaba

Probablemente la última línea de las ramas es un `print(...)`
(que es `Null`) en vez de la expresión que querías. Quitá el
`print` para que el valor "salga" del bloque.

### División de Ints me da `0` cuando esperaba decimal

`Int / Int = Int`. Convertí al menos uno a `Float`: `1.0 / 3`,
no `1 / 3`.

### `2 ** 8` me da error

`**` no existe en el MVP. Workarounds:
- `2 * 2 * 2 * 2 * 2 * 2 * 2 * 2` (chango pero claro).
- `1 << 8` si es potencia de 2 (= 256).
- Loop: `let r = 1; for _ in 0..8 { r *= 2 }`.

### Mi expresión bool no short-circuita como esperaba

`and` y `or` short-circuitan, pero **no hacen refinement**:
después de `(x != null) and x.field`, el checker NO sabe que
`x` no es null para el `x.field`. Usá `if` anidado.

### `1 + 2 == 3` parsea raro

`+` tiene precedencia más alta que `==`, así que `1 + 2 == 3`
es `(1+2) == 3` → `true`. Es lo que esperás.

Pero `a < b == true` sería `(a<b) == true` → `Bool` redundante.
Mejor: `a < b` directo.

---

## Lo que viene en C4

Sabés decidir entre dos caminos. En el próximo cap aprendés a
**repetir**: `while` (mientras una condición sea cierta), `loop`
(infinito con `break`), `for in` (sobre un rango o una lista).
Después de C4 podés escribir un FizzBuzz, una pirámide, o sumar
los primeros N números.
