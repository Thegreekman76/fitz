# Guía de Fitz

> Estado: viva — cubre lo que el intérprete ejecuta hoy y lo que el
> compilador (`fitz build`) produce como binario nativo.
> Última actualización: 2026-05-16 (Fase 9.z.1 cerrada — `fitz fmt`
> production-ready con preservación de comments + blank lines.
> 1333 unit + 55 cli_e2e + 79 compile_e2e + 3 openapi).

Esta guía es para developers que vienen de Python, TypeScript, Vue o
similares y quieren aprender Fitz escribiendo programas reales. Está
pensada para leerse de arriba a abajo: cada capítulo asume lo del
anterior.

Lo que ves acá funciona hoy contra el binario del repo. Si un ejemplo
no corre, es un bug de la guía, del intérprete o del compilador —
abrí un issue.

---

## Índice

**Parte 1 — Empezando**
1. [Bienvenida](#1-bienvenida)
2. [Tu primer programa](#2-tu-primer-programa)

**Parte 2 — Datos y expresiones**
3. [Variables y tipos primitivos](#3-variables-y-tipos-primitivos)
4. [Operadores](#4-operadores)
5. [Strings](#5-strings)

**Parte 3 — Control de flujo y colecciones**
6. [Booleanos y lógica](#6-booleanos-y-lógica)
7. [if / else](#7-if--else)
8. [Loops](#8-loops)
9. [Listas, mapas y rangos](#9-listas-mapas-y-rangos)
10. [Match](#10-match)

**Parte 4 — Abstracción**
11. [Funciones](#11-funciones)
12. [Tipos con `type`](#12-tipos-con-type)
13. [Métodos y mutación](#13-métodos-y-mutación)

**Parte 5 — Errores**
14. [Result y manejo de errores](#14-result-y-manejo-de-errores)
15. [Errores y mensajes](#15-errores-y-mensajes)

**Parte 6 — Organización**
16. [Módulos](#16-módulos)
16b. [Package manager](#16b-package-manager)

**Parte 7 — HTTP nativo y concurrencia**
17. [HTTP nativo](#17-http-nativo)
18. [Docs automáticas](#18-docs-automáticas)
19. [Async y concurrencia](#19-async-y-concurrencia)

**Parte 8 — Compilar**
20. [`fitz build` — compilar a binario nativo](#20-fitz-build--compilar-a-binario-nativo)

**Parte 9 — Interop**
21. [Interop Python](#21-interop-python)

**Parte 10 — Tooling**
22. [Soporte para editores](#22-soporte-para-editores)
23. [`fitz fmt` — formateador automático](#23-fitz-fmt--formateador-automático)
24. [`fitz test` — testing built-in](#24-fitz-test--testing-built-in)
25. [`fitz dev` — hot reload](#25-fitz-dev--hot-reload)
26. [`fitz repl` — REPL interactivo](#26-fitz-repl--repl-interactivo)
27. [`fitz lint` — linter de patrones](#27-fitz-lint--linter-de-patrones)

**Parte 11 — Cerrando**
28. [Qué sigue](#28-qué-sigue)

---

## 1. Bienvenida

Fitz es un lenguaje nuevo, pensado para gente que ama la ergonomía de
Python y TypeScript pero se cansó de la lentitud del primero y del
bagaje histórico del segundo. Algunas ideas centrales:

- **Sintaxis liviana**, inspirada en Python y TypeScript. Punto y coma
  opcional, llaves para los bloques, indentación libre.
- **Tipado gradual con chequeo estático**: las anotaciones de tipo
  son opcionales, pero cuando están, `fitz check` y `fitz run` las
  validan en compile time (Fase 5a, cerrada). Sin anotación, el
  tipo se infiere o se trata como `Any` (escape gradual).
- **HTTP como ciudadano de primera clase** — `@get`, `@post`, etc. son
  parte del lenguaje, no de una librería. El servidor arranca solo
  si tu programa registra rutas. Lo vas a ver en el [capítulo 17](#17-http-nativo).
- **Sin excepciones**: los errores se manejan con `Result` y `match`,
  estilo Rust.
- **Objetivo final: binario nativo**. Hoy Fitz es un intérprete escrito
  en Rust, y eso es lo que cubre esta guía.

El nombre es por el [cerro Fitz Roy](https://es.wikipedia.org/wiki/Cerro_Fitz_Roy),
en El Chaltén, Patagonia.

### Qué cubre esta guía

Solo lo que el intérprete ejecuta hoy:

- Variables y tipos primitivos: `Int`, `Float`, `Str`, `Bool`, `Null`.
- Aritmética con promoción `Int` ↔ `Float`, comparaciones, igualdad.
- Strings con concatenación e interpolación (`"Hola, {name}"`).
- Booleanos con `and` / `or` y short-circuit.
- `if` / `else` / `else if`, tanto sentencia como expresión.
- `while`, `loop`, `break`, `continue`.
- Listas (`[1, 2, 3]`), mapas (`{"k": v}`), rangos (`0..10`).
- Indexing con `xs[i]` / `m["k"]`.
- `for x in xs` y `for i in 0..n`.
- `match` con patrones literales, binding por identificador, `_` y rangos `0..10`.
- Funciones (`fn` en bloque y `=>` en flecha), funciones anónimas
  (`fn(x) => x*2`), closures, recursión.
- Declaración de tipos con `type`, instanciación (`User { id: 1, name: "x" }`),
  acceso a campos (`user.name`), mutación de campos (`user.name = "Otro"`),
  defaults y nullables.
- Method calls sobre listas, mapas, strings e instancias:
  `xs.map(fn(n) => n*2)`, `xs.push(v)`, `m.get("k")`, `s.upper()`.
- Manejo de errores con `Result`, `Ok(x)`, `Err(e)`, `match` sobre las
  variantes y operador `?` para propagar.
- Organización en archivos con `import foo` y `from foo import bar`.
- **HTTP nativo**: decoradores `@get`, `@post`, `@put`, `@delete` para
  registrar rutas, path params tipados, body deserializado contra un
  `type`, serialización JSON automática, `@server(port, host)` para
  configurar.
- Builtins globales: `print`, `len`, `sleep`, `cors`.

### Qué todavía no anda

Estas cosas aparecen en la [especificación de sintaxis](syntax-spec.md)
pero el intérprete aún no las ejecuta. Si las tipeás vas a ver un
error explícito:

- Tuplas (`(1, "a", true)`).
- Métodos custom declarados por el usuario sobre `type`
  (`type User { ... fn greet() => ... }`).

Todo eso está mapeado en el [roadmap](roadmap.md). Cada vez que una de
estas piezas se cierra, esta guía suma el capítulo correspondiente.

### Cómo está organizada

La guía está dividida en partes que se leen en orden:

1. **Empezando** — qué es Fitz y cómo correr tu primer programa.
2. **Datos y expresiones** — los tipos básicos y cómo se combinan.
3. **Control de flujo y colecciones** — decidir, repetir, agrupar datos.
4. **Abstracción** — funciones, tipos custom, métodos, mutación.
5. **Errores** — `Result` y mensajes del intérprete.
6. **Organización** — partir el código en módulos + package
   manager (`fitz.toml`, deps, lockfile).
7. **HTTP nativo y concurrencia** — el diferencial de Fitz: decoradores
   y server automático, docs autogeneradas, async.
8. **Compilar** — `fitz build` a binario nativo standalone.
9. **Interop** — `from python import ...` para reusar el ecosistema
   Python.
10. **Tooling** — LSP + extensión VSCode, formateador `fitz fmt`,
    test runner `fitz test`, hot reload `fitz dev`, REPL `fitz repl`,
    linter `fitz lint`.
11. **Cerrando** — el mapa de lo que viene.

### Cómo usar los ejemplos

Cada ejemplo de la guía vive como archivo en `examples/guide/` y se
ejecuta así:

```bash
cargo run -- run examples/guide/02-hola.fitz
```

Si copiás y pegás a un archivo propio, también funciona — los ejemplos
son completos, no fragmentos sueltos.

### Convenciones

- Los nombres en código están en inglés (`name`, `count`, `greet`),
  los comentarios y la prosa en español.
- Cuando una feature tiene un hueco conocido lo digo explícito, no lo
  escondo. Mejor saber lo que falta que tropezarse después.
- Los snippets cortos van inline. Los programas completos viven en
  `examples/guide/`.

---

Listo, ya sabés contra qué te estás peleando. En el próximo capítulo
escribimos y corremos el primer programa.

---

## 2. Tu primer programa

Antes de escribir código, asegurate de poder ejecutarlo.

### Requisitos

Fitz hoy es un intérprete escrito en Rust. Para correrlo necesitás:

- **Rust toolchain** (cargo + rustc). Si todavía no la tenés,
  instalá con [rustup](https://rustup.rs). El repo pinea la versión
  exacta de Rust en `rust-toolchain.toml`, así que rustup la baja sola
  la primera vez que corras `cargo` adentro del proyecto — no hace
  falta que coincida con la versión global de tu sistema.
- **Git**, para clonar el repo.

No hace falta nada más. No hay package manager de Fitz todavía (eso es
Fase 9), así que el "intérprete" es directamente el ejecutable del
proyecto.

### Bajar Fitz

```bash
git clone https://github.com/Thegreekman76/fitz.git
cd fitz
```

### Correr un programa

La forma más simple es dejar que `cargo` compile y ejecute en un solo
paso:

```bash
cargo run -- run examples/hello.fitz
```

Ese `--` separa los argumentos de cargo de los argumentos de Fitz. Lo
que viene después (`run examples/hello.fitz`) se lo come el binario
de Fitz: `run` es el subcomando para ejecutar un archivo, y después va
la ruta al `.fitz`.

Si querés un binario suelto que no dependa de cargo cada vez, podés
compilarlo en modo release:

```bash
cargo build --release
./target/release/fitz run examples/hello.fitz   # Linux/macOS
.\target\release\fitz.exe run examples\hello.fitz  # Windows
```

### La salida actual

Cuando ejecutás un programa hoy, vas a ver bastante más que la salida
del propio programa. Fitz está en Fase 2, así que el binario imprime
los pasos intermedios (los tokens del lexer y el árbol de sintaxis)
antes de la ejecución real:

```
🏔️  Fitz v0.1.0
   Ejecutando: examples/hello.fitz
   (intérprete en construcción — Fase 2)

--- Tokens ---
   1:1    Ident("print")
   1:6    LParen
   ...

--- AST ---
  [0] Expr(...)
  ...

--- Ejecución ---
Hola desde Fitz 🏔️
Hola, Patagonia!
```

Lo que escribió tu programa aparece debajo de `--- Ejecución ---`.
Todo lo de arriba es ruido útil para debuggear el intérprete, no para
vos. Más adelante en el roadmap esa salida se va a esconder detrás de
un flag `--debug`; por ahora convivimos con ella.

### Tu primer archivo

Vamos a escribir un programa propio. Creá [examples/guide/02-hola.fitz](../examples/guide/02-hola.fitz)
con este contenido (o copialo del repo):

```fitz
// 01-hola.fitz — El primer programa de la guía.
// Muestra: print, asignación sin tipo, interpolación de strings.

print("Hola desde Fitz 🏔️")

name = "Patagonia"
print("Hola, {name}!")
```

Lo corrés igual que cualquier otro:

```bash
cargo run -- run examples/guide/02-hola.fitz
```

Y vas a ver, al final:

```
Hola desde Fitz 🏔️
Hola, Patagonia!
```

### Anatomía línea por línea

```fitz
// 01-hola.fitz — El primer programa de la guía.
```

Comentarios de una línea con `//`. Para bloques largos también podés
usar `/* ... */`. El lexer los ignora, no llegan al programa.

```fitz
print("Hola desde Fitz 🏔️")
```

`print` es un builtin: viene incluido en el intérprete, no tenés que
importarlo. Recibe uno o más argumentos, los imprime separados por
espacio y agrega un salto de línea al final (mismo comportamiento que
Python). Los strings van entre comillas dobles. Los emojis y caracteres
no-ASCII funcionan, porque Fitz trabaja en UTF-8.

```fitz
name = "Patagonia"
```

Asignación. No hace falta `let` ni declarar el tipo: la primera vez
que aparece un identificador asignado, queda creado en el scope actual.
El tipo (`Str` en este caso) se infiere del valor.

```fitz
print("Hola, {name}!")
```

Interpolación. Dentro de un string, cualquier cosa entre `{...}` se
evalúa y se inserta en el lugar. Acá metimos un identificador, pero
también podrías meter expresiones más complejas — vamos a verlo en el
capítulo de strings.

### Errores comunes en este punto

Si el comando no encuentra el archivo:

```
Error leyendo examples/guide/02-hola.fitz: ...
```

Revisá la ruta. Cargo corre con la raíz del proyecto como working
directory, así que las rutas son relativas a la carpeta `fitz/`.

Si el archivo está pero hay un error de sintaxis, el intérprete corta
con línea y columna del problema. Vamos a aprender a leer esos
mensajes en el capítulo 15.

---

Con esto ya podés escribir, correr y ver salida. En el próximo
capítulo entramos a los datos: qué tipos hay y cómo se anotan.

---

## 3. Variables y tipos primitivos

Una variable en Fitz es un nombre asociado a un valor. La forma más
corta es:

```fitz
name = "Fitz"
```

No hace falta declarar nada antes. La primera asignación crea la
variable; las siguientes la reasignan. El tipo se infiere del valor.

### Los cinco tipos primitivos

Fitz tiene cinco tipos básicos. Los compuestos (listas, mapas,
tipos custom instanciados) los ves en capítulos posteriores.

| Tipo   | Qué es                | Ejemplos             |
|--------|-----------------------|----------------------|
| `Int`  | Entero de 64 bits con signo | `42`, `-7`, `0` |
| `Float`| Punto flotante de 64 bits   | `3.14`, `-0.5`, `2.0` |
| `Str`  | String UTF-8                | `"hola"`, `"Patagonia 🏔️"` |
| `Bool` | Booleano                    | `true`, `false` |
| `Null` | Ausencia de valor           | `null` |

Algunas notas:

- Los `Int` y `Float` son tipos distintos: `1` y `1.0` no son lo mismo,
  aunque se mezclan en operaciones (eso lo vemos en el cap. 4).
- Los strings van con comillas dobles. Las simples no se usan.
- Los emojis y caracteres no-ASCII funcionan: el lexer es UTF-8.
- `null` es un valor de su propio tipo (`Null`), no es un caso especial
  de otro. Imprimir `null` muestra literalmente `null`.

### Números legibles (mini-tanda Núm)

Los literales numéricos aceptan **separadores `_`** entre dígitos y
**notación científica** `e`/`E`. Ambas formas son azúcar sintáctica
del lexer — el valor numérico final es el mismo que sin la notación.

```fitz
// Separadores en Int y Float — mejora legibilidad sin cambiar el valor.
let poblacion: Int = 8_000_000_000
let pi_long: Float = 3.141_592_653

// Notación científica — `e` o `E`, signo opcional.
let mil: Float = 1e3                // 1000.0
let micro: Float = 1.5e-6            // 0.0000015
let big: Float = 1.23E4              // 12300.0

// Combinados — separador adentro de mantisa y exponente.
let valor: Float = 2.997_924_58e8    // 299792458.0
```

Reglas:

- `_` solo entre dígitos. Inválido: `_1`, `1_`, `1__0` (doble underscore).
- `e`/`E` con exponente opcionalmente firmado (`+`/`-`). Exige al
  menos un dígito tras el signo: `1e`, `1e+` son errores del lexer.
- **`1e10` produce `Float`**, no `Int` (incluso sin punto decimal).
  Si querés un entero grande, usá `10_000_000_000` (`Int`).

Ver [examples/guide/03b-numeros-legibles.fitz](../examples/guide/03b-numeros-legibles.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

### Literales en otras bases (mini-tanda Lit)

Los `Int` aceptan tres prefijos para expresarse en distintas bases:

| Prefijo | Base       | Ejemplo        | Valor |
|---------|------------|----------------|-------|
| `0x`    | Hexadecimal | `0xFF`, `0xff` | 255   |
| `0b`    | Binario     | `0b1010`       | 10    |
| `0o`    | Octal       | `0o755`        | 493   |

```fitz
let max_byte: Int = 0xFF
let nibble_alto: Int = 0b1111_0000     // 240
let perms_rwxr_xr_x: Int = 0o755        // 493
let dead_beef: Int = 0xDEAD_BEEF        // 3735928559
```

Reglas:

- **Solo minúsculas en el prefijo** (`0x`, no `0X`). Los dígitos hex
  sí son case-insensitive (`0xff` == `0xFF`).
- **Separadores `_`** permitidos entre dígitos válidos para la base
  (`0xDEAD_BEEF`, `0b1010_1010`, `0o7_5_5`).
- **Overflow sobre `i64`** → error claro del lexer.
- **Sin notación científica adentro de hex/bin/oct**. La `e` en hex
  es un dígito válido (`0xCAFE`, `0xFE`), no exponente.

Combinados con format specs de la mini-tanda Fm, podés mostrar un
mismo número en distintas bases para debug:

```fitz
let n: Int = 0xCAFE
print("dec: {n}, hex: {n:#x}, bin: {n:#b}, oct: {n:#o}")
// dec: 51966, hex: 0xcafe, bin: 0b1100101011111110, oct: 0o145376
```

Ver [examples/guide/03c-bases-numericas.fitz](../examples/guide/03c-bases-numericas.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

### Asignación

La forma corta es solo `nombre = valor`:

```fitz
name = "Fitz"
count = 42
ratio = 3.14
active = true
nothing = null
```

Pero si querés, también podés usar la keyword `let`:

```fitz
let mountain = "Fitz Roy"
```

`let` es opcional. Hoy las dos formas hacen exactamente lo mismo y
ambas se compilan a la misma sentencia interna. La diferencia es solo
de estilo: muchos lenguajes (Rust, JS, Swift) usan `let` o `const` para
marcar **declaración nueva** vs. **reasignación**. En Fitz esa
distinción no existe todavía. Usá la forma que te resulte más clara —
en esta guía vamos a preferir la corta para programas chicos y `let`
cuando hay que dejar claro que es una variable nueva en un bloque
denso.

### Anotación de tipo (opcional, todavía no se valida)

Podés anotar el tipo con `: Tipo` después del nombre:

```fitz
age: Int = 30
city: Str = "El Chaltén"

let height: Float = 3405.0
```

**Desde la Fase 5, estas anotaciones se chequean en compile time**.
Si el valor no coincide con el tipo declarado, `fitz check` reporta
el problema y `fitz run` aborta en modo strict:

```fitz
x: Int = "no soy int"
// ✗ Error — `x` declarado como `Int` recibió un valor `Str`
```

El [tipado gradual](roadmap.md) sigue siendo el modelo: las
anotaciones son opcionales. Sin anotación, el tipo se infiere
(`let n = 42` → `n` es `Int`) o se trata como `Any` cuando no se
puede determinar. Si querés saltarte el chequeo en una corrida
puntual, agregale `--no-typecheck` al comando.

La sintaxis de anotaciones admite tipos compuestos: `List<Int>`,
`Map<Str, User>`, `Result<User>`, y nullable `Str?`.

### Reasignación

Asignar de nuevo al mismo nombre simplemente cambia el valor:

```fitz
count = 42
count = count + 1
print(count)   // 43
```

Sin anotación, el tipo del valor también puede cambiar entre
asignaciones (consecuencia del modelo gradual, no algo que
recomiende):

```fitz
n = 42
n = "ahora soy texto"   // pasa porque n no tiene tipo declarado
```

Con anotación, asignar un valor incompatible falla en `fitz check`:

```fitz
m: Int = "no soy int"   // ✗ error — `m` declarado como `Int` recibió un valor `Str`
```

### Ámbito (scope)

Una variable existe en el bloque donde se define y en los anidados,
hasta donde se cierra ese bloque. Por ahora **los bloques de `if`,
`match` y `while` no crean su propio scope**: una variable definida
adentro persiste afuera. Es un comportamiento estilo Python, no estilo
Rust. Las funciones sí crean su propio scope (cap. 11).

Esto puede sorprender — lo dejamos marcado y, si en algún momento trae
problemas reales, lo reconsideramos.

### Ejemplo completo

[examples/guide/03-variables.fitz](../examples/guide/03-variables.fitz):

```fitz
// 02-variables.fitz — Variables y tipos primitivos.

// Sin tipo: se infiere del valor.
name = "Fitz"
count = 42
ratio = 3.14
active = true
nothing = null

print(name)
print(count)
print(ratio)
print(active)
print(nothing)

// Con anotación (se acepta y todavía no se valida).
age: Int = 30
city: Str = "El Chaltén"

print(age)
print(city)

// `let` es opcional.
let mountain = "Fitz Roy"
print(mountain)

// Reasignar es volver a usar el mismo nombre.
count = count + 1
print(count)
```

Salida:

```
Fitz
42
3.14
true
null
30
El Chaltén
Fitz Roy
43
```

---

En el próximo capítulo combinamos estos valores con operadores:
aritmética con promoción automática entre `Int` y `Float`,
comparaciones, igualdad y unario negativo.

---

## 4. Operadores

Una vez que tenés valores, queremos combinarlos. Fitz cubre los
operadores que esperás de cualquier lenguaje, con algunas decisiones
puntuales que vale la pena marcar.

### Aritmética

| Operador | Significado     | Ejemplo       |
|----------|-----------------|---------------|
| `+`      | Suma            | `2 + 3` → `5` |
| `-`      | Resta           | `10 - 4` → `6` |
| `*`      | Multiplicación  | `6 * 7` → `42` |
| `/`      | División        | ver abajo     |

Los cuatro operan sobre `Int` y `Float`. Lo interesante aparece cuando
mezclás los dos tipos.

### Promoción `Int` ↔ `Float`

Si los dos operandos son `Int`, el resultado es `Int`. Si los dos son
`Float`, el resultado es `Float`. Si **mezclás** un `Int` y un `Float`,
el `Int` se promueve a `Float` y el resultado es `Float`:

```fitz
print(2 + 3)       // 5      (Int + Int = Int)
print(2.0 + 3.0)   // 5.0    (Float + Float = Float)
print(1 + 1.0)     // 2.0    (Int + Float = Float)
```

### División: entera vs flotante

Esto es lo que más sorprende a quien viene de Python. En Fitz, `/`
entre dos `Int` da `Int`:

```fitz
print(10 / 3)      // 3, no 3.333…
```

Para forzar división flotante, alcanza con que uno de los dos
operandos sea `Float`:

```fitz
print(10 / 3.0)    // 3.3333333333333335
print(10.0 / 3)    // 3.3333333333333335
print(10.0 / 3.0)  // 3.3333333333333335
```

El comportamiento es el de Rust (y C, y Go): la división se "porta
como" el tipo de sus operandos. Si querés el comportamiento de Python
3 (`/` siempre flotante, `//` entera), por ahora tenés que ser
explícito convirtiendo uno de los lados.

> Aviso de IEEE 754: vas a ver `3.3333333333333335` y no
> `3.3333333333333333`. No es un bug — los `Float` de 64 bits no
> pueden representar exactamente decimales periódicos. Es el mismo
> "error" que vas a ver en Python (`0.1 + 0.2`), JavaScript y Rust.

### División por cero

Dividir por cero, en `Int` o `Float`, corta la ejecución con un error
explícito:

```
Error en línea 0:0 — división por cero
```

No hay `NaN` ni `Infinity` silenciosos; el intérprete prefiere parar y
avisar. (La línea sale como `0:0` por una limitación actual en cómo
guardamos posiciones de subexpresiones — lo vamos a mejorar.)

### Unario negativo

`-` también funciona como prefijo para negar:

```fitz
print(-5)       // -5
print(-3.14)    // -3.14
x = 10
print(-x)       // -10
```

No hay un unario `+` redundante; tampoco hay incremento/decremento
(`++`, `--`). Si querés sumar 1 a una variable, escribís
`x = x + 1`.

### Comparación

| Operador | Significado |
|----------|-------------|
| `<`      | Menor que   |
| `<=`     | Menor o igual |
| `>`      | Mayor que   |
| `>=`     | Mayor o igual |

Devuelven `Bool`. Sirven para números (`Int` y `Float`, con la misma
promoción que la aritmética) y para strings, que se comparan
lexicográficamente carácter por carácter:

```fitz
print(1 < 2)        // true
print(3 >= 3)       // true
print("a" < "b")    // true
print("abc" < "abd")// true
```

### Igualdad

| Operador | Significado |
|----------|-------------|
| `==`     | Igual a     |
| `!=`     | Distinto de |

La comparación de igualdad tiene **una sola coerción**: entre `Int` y
`Float` numéricamente equivalentes. Todo el resto compara primero el
tipo:

```fitz
print(1 == 1)        // true
print(1 == 1.0)      // true  — coerción Int↔Float
print(1 == "1")      // false — tipos distintos, sin coerción
print(true == 1)     // false — Bool y Int son tipos distintos
print(null == null)  // true
print(null == 0)     // false
```

Es a propósito: nada de la maldad histórica de `==` en JavaScript.
Si dos valores tienen tipos distintos (salvo `Int`/`Float`), son
distintos.

### Precedencia

De más fuerte a más débil (lo que toca en este capítulo):

1. Unario `-`
2. `*`, `/`
3. `+`, `-`
4. `<`, `<=`, `>`, `>=`
5. `==`, `!=`

Y como siempre, los paréntesis ganan:

```fitz
print(2 + 3 * 4)     // 14
print((2 + 3) * 4)   // 20
print(-2 * 3)        // -6
```

Los lógicos `and` / `or` también participan de la precedencia (van
*debajo* de igualdad). Los vemos en el capítulo 6.

### Módulo `%`

`%` calcula el resto de la división entera (R.1.2, mini-fase R).
Solo válido entre `Int`:

```fitz
print(10 % 3)     // 1
print(12 % 4)     // 0
print(7 % 2)      // 1 — útil para detectar pares/impares

// En condición:
let n = 8
if (n % 2 == 0) {
    print("par")
}
```

**Semántica euclidean** (mismo signo del divisor, igual que
Python — distinto del `%` Rust que es truncate-toward-zero):

```fitz
print(-7 % 3)     // 2 (NO -1)
print(7 % -3)     // -2
```

`n % 0` es **error runtime** (división por cero), no infinity.
El mismo binario producido por `fitz build` tiene el mismo check
y emite "división por cero" en lugar de panic crudo de Rust.

### Asignación compuesta

`+=`, `-=`, `*=`, `/=` aplican la operación al destino sin tener
que escribirlo dos veces (R.2.3, mini-fase R). Se *desugar* en el
parser a `target = target <op> rhs`, así que valen sobre cualquier
destino de asignación: identificador, campo, índice:

```fitz
let total = 0
total += 5            // total = total + 5
total -= 2
total *= 3
total /= 2

let xs = [10, 20, 30]
xs[0] += 100          // xs[0] = xs[0] + 100

type Counter { count: Int = 0 }
let c = Counter {}
c.count += 1          // c.count = c.count + 1
```

Patrón típico: acumular adentro de un loop:

```fitz
let suma = 0
for i in 1..=5 {
    suma += i
}
print(suma)           // 15
```

### Operadores bit-a-bit (mini-tanda Bits)

Seis operadores bit-a-bit sobre `Int`. Combinan natural con
literales hex/binario/octal (mini-tanda Lit) para máscaras de bits,
flags y manipulación de bytes.

| Operador | Aridad | Función                   |
|----------|--------|---------------------------|
| `&`      | Binario | AND bit-a-bit             |
| `\|`     | Binario | OR bit-a-bit              |
| `^`      | Binario | XOR bit-a-bit             |
| `<<`     | Binario | Shift left                |
| `>>`     | Binario | Shift right (aritmético)  |
| `~`      | Unario  | NOT bit-a-bit             |

```fitz
let raw: Int = 0xABCD
let lo: Int = raw & 0xFF                     // 0xCD = 205
let hi: Int = (raw >> 8) & 0xFF              // 0xAB = 171
let flags: Int = 0b0001 | 0b0010             // 0b0011 = 3
let toggled: Int = 0xFF ^ 0xAA               // 0x55 = 85
let doubled: Int = 1 << 4                    // 16
let inverted: Int = ~0                       // -1 (i64 con signo)
```

**Precedencia** (paralelo a Python/C): `|` < `^` < `&` < `<<`/`>>`.
Sin paréntesis, `a | b & c` se parsea como `a | (b & c)`. El
unario `~` tiene la misma precedencia que `-` (negación numérica).

**Reglas estrictas**:
- Ambos operandos deben ser `Int`. Float/Bool/Str → error del checker.
- Shifts con RHS fuera de `0..64` → error de runtime.

Encaja natural con format specs (mini-tanda Fm) para debug visual:

```fitz
let n: Int = 0xCAFE
print("hi: {(n >> 8) & 0xFF:#x}, lo: {n & 0xFF:#x}")
// hi: 0xca, lo: 0xfe
```

Ver [examples/guide/04b-operadores-bit.fitz](../examples/guide/04b-operadores-bit.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

### Lo que todavía no anda

- `%=` (módulo compuesto) — sub-paso menor si aparece presión.
- `%` sobre `Float` (la ambigüedad entre `fmod` y `rem_euclid`
  requiere decisión de diseño; sub-paso futuro si aparece presión).

> Lo que **sí anda** y antes era deuda: operador `%` (módulo
> sobre `Int` con semántica euclidean, R.1.2); operadores
> compuestos `+=`/`-=`/`*=`/`/=` (R.2.3 mini-fase R);
> **operadores de bits** `&`/`|`/`^`/`<<`/`>>`/`~` (mini-tanda
> Bits — ver sub-sección de arriba); **compuestos bit-a-bit**
> `&=`/`|=`/`^=`/`<<=`/`>>=` y **prefijos mayúscula** `0X`/`0B`/
> `0O` (mini-tanda Cmp — ver sub-sección de abajo).

### Asignación compuesta bit-a-bit (mini-tanda Cmp)

Simetría natural con `+=`/`-=`/etc. Cinco ops compuestos:

```fitz
let flags: Int = 0b0101
flags |= 0b0010     // setear bit:    0b0111
flags &= 0b1110     // clearear bit:  0b0110
flags ^= 0b0100     // toggle bit:    0b0010
flags <<= 2         // shift left 2:  0b1000
flags >>= 1         // shift right 1: 0b0100
```

Semántica: `x &= y` ≡ `x = x & y`. Solo sobre `Int`.

Además, los prefijos hex/bin/oct aceptan mayúscula (Python-compat):

```fitz
let h: Int = 0XFF        // == 0xFF
let b: Int = 0B1010      // == 0b1010
let o: Int = 0O755       // == 0o755
```

Ver [examples/guide/04c-asignacion-compuesta-bit.fitz](../examples/guide/04c-asignacion-compuesta-bit.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

### Ejemplo completo

[examples/guide/04-operadores.fitz](../examples/guide/04-operadores.fitz):

```fitz
print(2 + 3)
print(10 - 4)
print(6 * 7)
print(10 / 3)        // 3 — división entera
print(10.0 / 3.0)    // 3.3333333333333335
print(1 + 1.0)       // 2.0
print(10 / 3.0)      // 3.3333333333333335

print(-5)
x = 10
print(-x)

print(1 < 2)
print(3 >= 3)
print("a" < "b")

print(1 == 1)
print(1 == 1.0)
print(1 == "1")
print(null == null)
```

Salida:

```
5
6
42
3
3.3333333333333335
2.0
3.3333333333333335
-5
-10
true
true
true
true
true
false
true
```

---

En el próximo capítulo le entramos a los strings: concatenación,
interpolación de expresiones, y qué cosas se pueden meter adentro de
las llaves `{...}`.

---

## 5. Strings

Los strings ya aparecieron en cada capítulo anterior. Acá los miramos
con detenimiento: cómo se combinan, cómo se interpolan, y qué
caracteres especiales se pueden escapar.

### Forma básica

Un string va entre comillas dobles:

```fitz
name = "Fitz"
print(name)
```

Las comillas simples no se usan en Fitz — siempre comillas dobles. El
contenido es UTF-8, así que emojis y acentos funcionan tal cual:

```fitz
print("Hola desde la Patagonia 🏔️")
```

Por ahora un string vive en **una sola línea**. Si abrís comillas y
hacés un Enter sin cerrarlas, el lexer corta con:

```
Error en línea N:M — String sin cerrar — salto de línea antes de la comilla de cierre
  Sugerencia: Usá \n para incluir un salto de línea dentro del string
```

Los strings multilínea con `"""..."""` están en la
[especificación](syntax-spec.md) pero todavía no están implementados.

### Concatenación con `+`

Para unir dos strings, usá `+`:

```fitz
name = "Fitz"
greeting = "Hola, " + name
print(greeting)        // Hola, Fitz
```

`+` entre strings es **estricto**: solo `Str + Str`. Si intentás
sumar un string con un número, el intérprete corta:

```fitz
n = 42
print("n = " + n)
// Error en línea 0:0 — operación `+` no soportada entre `Str` y `Int`
```

Esta decisión es intencional: para juntar valores de tipos distintos,
la herramienta correcta es la **interpolación**, no `+`. Así evitamos
el lío histórico de JavaScript con `"1" + 1 == "11"`.

### Interpolación con `{...}`

Dentro de un string, cualquier cosa entre `{` y `}` se evalúa y se
convierte a texto. La forma más simple es interpolar un identificador:

```fitz
name = "Fitz"
print("Hola, {name}!")    // Hola, Fitz!
```

Pero adentro de las llaves no estás limitado a identificadores —
podés meter cualquier expresión:

```fitz
count = 42
print("doble: {count * 2}")             // doble: 84
print("dos más dos es {2 + 2}")         // dos más dos es 4
```

Todos los tipos primitivos saben cómo se serializan a texto cuando se
interpolan:

| Tipo   | Cómo se interpola         |
|--------|---------------------------|
| `Str`  | el contenido tal cual     |
| `Int`  | `42`                      |
| `Float`| `3.14`                    |
| `Bool` | `true` / `false`          |
| `Null` | `null`                    |

```fitz
count = 42
ratio = 3.14
active = true
nothing = null
print("count={count}, ratio={ratio}, active={active}, nothing={nothing}")
// count=42, ratio=3.14, active=true, nothing=null
```

Si la expresión adentro de `{...}` referencia algo que no existe,
el intérprete corta con un error explícito:

```fitz
print("hola, {missing}")
// Error en línea 0:0 — variable `missing` no definida
```

### Escapes

Adentro de un string, el backslash (`\`) introduce un escape:

| Escape | Significa |
|--------|-----------|
| `\n`   | Salto de línea |
| `\t`   | Tab |
| `\"`   | Comilla doble literal |
| `\\`   | Backslash literal |
| `\{`   | Llave de apertura literal (sin interpolar) |
| `\}`   | Llave de cierre literal |

```fitz
print("línea 1\nlínea 2")
// línea 1
// línea 2

print("nombre:\tFitz")
// nombre:	Fitz

print("dijo: \"hola\"")
// dijo: "hola"

print("barra: \\")
// barra: \

print("config: \{ port: 3000 \}")
// config: { port: 3000 }
```

El último caso es importante: si querés mostrar una llave literal en
un string (por ejemplo, JSON inline o un fragmento de código), tenés
que escaparla — si no, el intérprete intenta interpretar lo que hay
entre `{` y `}` como una expresión a interpolar.

### Strings multilínea con `"""..."""`

Para mensajes largos, SQL, HTML, JSON inline, etc., usá
triple-quote (R.1.5, mini-fase R):

```fitz
let sql = """
    SELECT u.id, u.name, u.email
    FROM users u
    WHERE u.active = true
    ORDER BY u.created_at DESC
"""
print(sql)

// La interpolación sigue funcionando adentro:
let name = "Fitz"
let bienvenida = """
    Hola, {name}!
    Esta es una bienvenida
    de varias líneas.
"""
```

Características:
- **Newlines literales** son válidos (no necesitás `\n`).
- **Comillas dobles aisladas** se preservan literalmente. Solo
  `"""` (tres seguidas) cierra el string. Útil para embeber
  JSON con comillas:
  ```fitz
  let json = """{"name": "Fitz", "age": 1}"""
  ```
- **Mismos escapes** que strings normales (`\n`, `\t`, `\\`,
  `\"`, `\{`, `\}`).
- **Interpolación** `{expr}` funciona igual.
- Indentación: el contenido se preserva tal cual; si querés
  recortar el indent común usá `.replace(...)` o construilo sin
  indent.

### Format specifiers (mini-tanda Fm)

Los `{...}` de interpolación aceptan un `:spec` opcional después del
expr, con la misma sintaxis que Python:

```fitz
let pi: Float = 3.14159
print("pi: {pi:.2f}")              // "pi: 3.14"

let n: Int = 42
print("[{n:05d}]")                  // "[00042]"
print("[{n:>5}]")                   // "[   42]"

let byte: Int = 255
print("hex: {byte:#x}")             // "hex: 0xff"

let big: Int = 1000000
print("con coma: {big:,}")          // "con coma: 1,000,000"

let ratio: Float = 0.42
print("pct: {ratio:.1%}")           // "pct: 42.0%"
```

Gramática del spec: `[[fill]align][sign][#][0][width][grouping][.precision][type]`.

| Componente   | Valores                | Función |
|--------------|-----------------------|---------|
| `fill`       | cualquier char         | Caracter de relleno. Solo válido si va con `align`. |
| `align`      | `<` `>` `^` `=`        | Alineación left / right / center / after-sign. |
| `sign`       | `+` `-` ` `            | Signo: siempre / solo negativos / espacio en positivos. |
| `#`          | (flag)                 | Alternate form (`0x` en hex, etc.). |
| `0`          | (flag)                 | Zero-pad (atajo para `fill='0'`). |
| `width`      | dígitos                | Ancho mínimo total. |
| `grouping`   | `,` o `_`              | Separador de miles. |
| `.precision` | `.N`                   | Decimales (Float) o longitud máx (Str). |
| `type`       | `b`/`c`/`d`/`e`/`E`/`f`/`F`/`g`/`G`/`o`/`s`/`x`/`X`/`%` | Forma de presentación. |

**Compatibilidad por type**:
- `f`/`F`/`e`/`E`/`g`/`G`/`%` — Float (Int promueve transparente).
- `d`/`b`/`o`/`x`/`X`/`c` — Int estricto.
- `s` — cualquier tipo (vía Display).
- Sin type — Display por default.

El checker valida la compatibilidad: `{x:.2f}` con `x: Str` da error
de tipo antes de runtime.

**Subset compilable con `fitz build`** (subset que mapea directo a
`format!` de Rust): precisión Float, width/zero-pad de Int, alineación,
fill custom, sign, alternate (`#`), hex/binario/octal. **Solo `fitz
run`** (Rust no tiene equivalente nativo): grouping (`,`/`_`),
percent (`%`), exponente (`e`/`E`), general (`g`/`G`), char (`c`).
El codegen emite error claro citando `fitz run` como workaround.

Ver [examples/guide/05b-format-specs.fitz](../examples/guide/05b-format-specs.fitz)
(subset compilable, validado bit-a-bit `fitz run` ↔ `fitz build`) y
[examples/guide/05c-format-specs-advanced.fitz](../examples/guide/05c-format-specs-advanced.fitz)
(full Python, solo `fitz run`).

### Lo que todavía no anda

- Comillas simples como alternativa a las dobles.

> Lo que **sí anda** y antes era deuda: **strings multilínea
> `"""..."""`** — cerrado en R.1.5 (mini-fase R). Incluyendo
> interpolación adentro y preservando newlines literales.
> Métodos `.split(sep)`, `.contains(s)`, `.starts_with(s)`,
> `.ends_with(s)`, `.trim()`, `.replace(old, new)`, `.repeat(n)`
> — cerrados en mini-tanda S (S.1 + S.2). Vivos en `fitz run` y
> `fitz build` (ver [cap 13](#13-métodos-y-mutación) +
> ejemplo [13c-metodos-extras.fitz](../examples/guide/13c-metodos-extras.fitz)).
> **Format specifiers** `{x:.2f}`, `{n:05d}`, etc. cerrados en
> mini-tanda Fm (ver sub-sección "Format specifiers" arriba).

### Ejemplo completo

[examples/guide/05-strings.fitz](../examples/guide/05-strings.fitz):

```fitz
name = "Fitz"
print(name)

greeting = "Hola, " + name
print(greeting)

print("Hola, {name}!")

count = 42
ratio = 3.14
active = true
nothing = null
print("count={count}, ratio={ratio}, active={active}, nothing={nothing}")

print("dos más dos es {2 + 2}")
print("doble: {count * 2}")

print("línea 1\nlínea 2")
print("nombre:\tFitz")
print("dijo: \"hola\"")
print("barra: \\")

print("config: \{ port: 3000 \}")
```

Salida:

```
Fitz
Hola, Fitz
Hola, Fitz!
count=42, ratio=3.14, active=true, nothing=null
dos más dos es 4
doble: 84
línea 1
línea 2
nombre:	Fitz
dijo: "hola"
barra: \
config: { port: 3000 }
```

---

Con esto ya podés representar y combinar valores. En el próximo
capítulo arrancamos con la lógica: `and`, `or` con short-circuit, y
cómo se conecta con la comparación que vimos en el cap. 4.

---

## 6. Booleanos y lógica

`Bool` tiene exactamente dos valores: `true` y `false`. Los operadores
de comparación e igualdad del cap. 4 ya devuelven `Bool`. Acá los
combinamos con los operadores lógicos `and` y `or`.

### `and` y `or`

Como palabras, no como símbolos. (Fitz no usa `&&` ni `||`.) Operan
sobre `Bool` y devuelven `Bool`:

| Expresión          | Resultado |
|--------------------|-----------|
| `true and true`    | `true`    |
| `true and false`   | `false`   |
| `false and true`   | `false`   |
| `false and false`  | `false`   |
| `true or true`     | `true`    |
| `true or false`    | `true`    |
| `false or true`    | `true`    |
| `false or false`   | `false`   |

El uso real casi siempre es combinando con comparaciones:

```fitz
age = 20
print(age >= 18 and age < 65)   // true
print(age < 13 or age >= 65)    // false
```

### Short-circuit

Igual que Python, JavaScript y Rust, los operadores lógicos en Fitz
hacen **short-circuit**:

- `a or b` — si `a` ya es `true`, `b` no se evalúa.
- `a and b` — si `a` ya es `false`, `b` no se evalúa.

Esto importa cuando el lado derecho tiene side effects, o cuando
hace algo costoso:

```fitz
fn ruido() {
    print("¡me llamaron!")
    return true
}

print(true or ruido())     // imprime solo "true"
print(false and ruido())   // imprime solo "false"
print(true and ruido())    // imprime "¡me llamaron!" y luego "true"
```

En la práctica, esto te deja escribir cosas como
`x != null and x > 0` sin preocuparte por evaluar `x > 0` cuando `x`
es `null` (cuando lleguen los tipos custom y la nullabilidad en
serio).

### Tipo estricto: no hay "truthy" / "falsy"

A diferencia de Python o JavaScript, en Fitz **`and` y `or` solo
trabajan con `Bool`**, y los condicionales también. Si pasás otro
tipo, el intérprete corta:

```fitz
print(1 and 2)
// Error en línea 0:0 — operando izquierdo de `and` debe ser Bool, no `Int`

if 1 { print("ups") }
// Error en línea 0:0 — la condición de `if` debe ser Bool, no `Int`
```

Lo mismo aplica a `while` y demás condicionales (los vemos en el cap.
8). Si querés un check sobre un número, hacelo explícito:

```fitz
n = 5
if n != 0 {
    print("no es cero")
}
```

Es más texto, pero también más claro: `n != 0` dice exactamente qué
estás chequeando, sin reglas mentales sobre qué cuenta como "falsy".

### Precedencia completa

Extendiendo la tabla del cap. 4, la lista total de precedencia (de
más fuerte a más débil) es:

1. Unario `-`
2. `*`, `/`
3. `+`, `-`
4. `<`, `<=`, `>`, `>=`
5. `==`, `!=`
6. `and`
7. `or`

Eso significa que `true or false and false` se evalúa como
`true or (false and false)` → `true`. Si querés invertir el orden,
usá paréntesis:

```fitz
print(true or false and false)     // true
print((true or false) and false)   // false
```

Como regla cómoda: si tenés que pensarlo dos veces, ponele
paréntesis. Cuesta dos caracteres y le ahorra trabajo a quien lea el
código.

### Negación con `not`

`not <expr>` invierte un `Bool`. Útil para hacer condiciones más
legibles cuando el opuesto es lo que querés expresar:

```fitz
let active = false

if (not active) {
    print("inactivo")
}

// not se aplica antes que las comparaciones, así que:
let x = 5
if (not x == 10) {       // ← (not x) == 10 — type error si x:Int
    print("no es 10")
}

// Para negar el resultado de una comparación, usá paréntesis:
if (not (x == 10)) {     // ← negación del Bool resultado
    print("no es 10")
}
```

**`not` exige `Bool` estricto** (no truthy/falsy). `not 0` o
`not ""` son **type error** — consistente con la decisión de
diseño "sin truthy/falsy" del lenguaje. El checker estático lo
caza antes de correr.

Funciona idéntico en `fitz run` y `fitz build`: el codegen emite
`!` Rust nativo.

### Lo que todavía no anda

- **XOR lógico** — no hay `xor`. Si lo necesitás puntualmente,
  `a != b` sobre dos `Bool` te da el mismo resultado.

> Lo que **sí anda** y antes era deuda: operador `not <expr>`
> implementado en mini-fase R (R.1.1). Sin truthy/falsy, exige
> `Bool` estricto.

### Ejemplo completo

[examples/guide/06-logica.fitz](../examples/guide/06-logica.fitz):

```fitz
print(true and true)
print(true and false)
print(false or true)
print(false or false)

age = 20
print(age >= 18 and age < 65)
print(age < 13 or age >= 65)

fn ruido() {
    print("¡me llamaron!")
    return true
}

print(true or ruido())
print(false and ruido())
print(true and ruido())

print(true or false and false)
print((true or false) and false)
```

Salida:

```
true
false
true
false
true
false
true
false
¡me llamaron!
true
true
false
```

---

Con `and`/`or` y comparación ya tenés todo para escribir condiciones
ricas. En el próximo capítulo arrancamos a usarlas: `if`, `else if`,
`else`, y `if` como expresión.

---

## 7. if / else

`if` en Fitz cumple dos roles a la vez: es una **sentencia** (decide
qué bloque ejecutar) y también una **expresión** (produce un valor).
Esto último es lo que más probablemente te va a sorprender si venís
de Python.

### Como sentencia

La forma básica:

```fitz
age = 20
if age >= 18 {
    print("mayor")
} else {
    print("menor")
}
```

Las llaves son obligatorias incluso para una sola sentencia. La
condición tiene que ser `Bool` — no hay truthy/falsy (cap. 6).

Para encadenar más casos, usá `else if`:

```fitz
score = 75
if score >= 90 {
    print("A")
} else if score >= 80 {
    print("B")
} else if score >= 70 {
    print("C")
} else {
    print("F")
}
```

`else` es opcional. Si solo querés actuar en el caso verdadero, lo
omitís:

```fitz
n = 5
if n > 0 {
    print("positivo")
}
```

### Como expresión

Acá viene lo interesante. En Fitz, `if` también es una expresión que
**evalúa a un valor**: el de la última expresión del bloque elegido.
Eso te deja escribir:

```fitz
active = true
status = if active { "on" } else { "off" }
print(status)        // on
```

Es la misma sintaxis que la sentencia — la diferencia es que ahora
está al lado derecho de un `=` (o en cualquier lugar donde se espera
un valor). Si venís de TypeScript, es el equivalente al operador
ternario `cond ? a : b`, pero más legible y sin tener que aprender
una segunda sintaxis. Si venís de Rust, es exactamente el mismo
patrón.

Funciona con `else if` también:

```fitz
n = 0
sign = if n > 0 { "positivo" } else if n < 0 { "negativo" } else { "cero" }
print(sign)          // cero
```

### Bloques con varias sentencias

El bloque puede tener varias sentencias. El valor del `if` es **la
última expresión** del bloque elegido:

```fitz
total = if true {
    let a = 10
    let b = 20
    a + b           // ← esta es la última expresión
} else {
    0
}
print(total)        // 30
```

Esto se parece a cómo funcionan los bloques en Rust. Las sentencias
intermedias hacen su trabajo (asignaciones, prints, llamadas), y la
última expresión es lo que "sale" del bloque.

### Sin `else` como expresión

Si usás `if` como expresión **sin** `else` y la rama no se cumple, el
valor es `null`:

```fitz
x = if false { 1 }
print(x)            // null
```

Funciona, pero suele ser confuso de leer. Recomendación: cuando uses
`if` como expresión, escribí siempre el `else`. Si solo te interesa
el efecto cuando la condición es verdadera, usalo como sentencia y
listo.

### Scope (recordatorio)

Como vimos en el cap. 3, los bloques de `if` **no crean scope nuevo**.
Una variable definida adentro sigue viva afuera:

```fitz
if true {
    inner = 42
}
print(inner)        // 42
```

Es comportamiento estilo Python, no estilo Rust. Si en el futuro esto
trae problemas reales lo reconsideramos, pero por ahora simplifica
mucho el lenguaje.

### Ejemplo completo

[examples/guide/07-if.fitz](../examples/guide/07-if.fitz):

```fitz
age = 20
if age >= 18 {
    print("mayor")
} else {
    print("menor")
}

score = 75
if score >= 90 {
    print("A")
} else if score >= 80 {
    print("B")
} else if score >= 70 {
    print("C")
} else {
    print("F")
}

active = true
status = if active { "on" } else { "off" }
print(status)

n = 0
sign = if n > 0 { "positivo" } else if n < 0 { "negativo" } else { "cero" }
print(sign)

total = if true {
    let a = 10
    let b = 20
    a + b
} else {
    0
}
print(total)
```

Salida:

```
mayor
C
on
cero
30
```

---

Con `if` ya podés decidir. En el próximo capítulo le sumamos repetir:
`while`, `loop`, `break` y `continue`.

---

## 8. Loops

Para repetir código en Fitz hay tres construcciones: `while`, `loop` y
`for ... in`. Este capítulo cubre las dos primeras; `for` necesita
listas y rangos, así que vive en el [capítulo 9](#9-listas-mapas-y-rangos)
junto con las colecciones sobre las que itera.

### `while`

Repite el bloque mientras la condición sea `true`. Igual que con `if`,
la condición tiene que ser `Bool` (sin truthy/falsy):

```fitz
i = 0
while i < 3 {
    print("i = {i}")
    i = i + 1
}
```

Salida:

```
i = 0
i = 1
i = 2
```

Si la condición arranca en `false`, el bloque no se ejecuta nunca. Y
si nunca cambia a `false` adentro, tenés un loop infinito — el
intérprete no te va a salvar de eso.

### `loop`

Loop infinito, sin condición. Pensado para los casos donde la salida
es por `break`, no por condición de entrada:

```fitz
n = 0
loop {
    if n >= 3 {
        break
    }
    print("n = {n}")
    n = n + 1
}
```

Es lo que en otros lenguajes escribirías como `while true { ... }`,
con un nombre propio. La diferencia es semántica: cuando escribís
`loop`, le estás avisando al lector "esto sale por `break`, no por
condición".

### `break` y `continue`

- `break` corta el loop y sigue después del bloque.
- `continue` salta al inicio de la próxima iteración.

```fitz
j = 0
while j < 5 {
    j = j + 1
    if j == 3 {
        continue
    }
    print("j = {j}")
}
```

Salida:

```
j = 1
j = 2
j = 4
j = 5
```

Notá que en este caso `j = j + 1` va **antes** del `continue`. Si lo
ponés después, te quedás colgado: la iteración salta y `j` nunca
incrementa.

Si usás `break` o `continue` fuera de un loop, el intérprete corta
con un error explícito:

```
Error en línea 0:0 — `break` solo puede usarse adentro de un loop
```

### Loops anidados

`break` y `continue` actúan sobre el loop **más interno** que los
contiene. No hay labels todavía (estilo Rust) para romper varios
niveles:

```fitz
fila = 0
while fila < 2 {
    col = 0
    while col < 3 {
        if col == 2 {
            break       // rompe solo el while interno
        }
        print("({fila},{col})")
        col = col + 1
    }
    fila = fila + 1
}
```

Salida:

```
(0,0)
(0,1)
(1,0)
(1,1)
```

Si necesitás cortar el loop externo desde adentro, lo más limpio hoy
es mover una bandera:

```fitz
done = false
fila = 0
while fila < 5 and done == false {
    col = 0
    while col < 5 {
        if col == 3 and fila == 2 {
            done = true
            break
        }
        col = col + 1
    }
    fila = fila + 1
}
```

(Cerrado: ahora podés usar **labels** para romper varios niveles —
ver la sub-sección "Labels en break/continue" abajo.)

### Loop como expresión con valor (mini-tanda L)

`loop { ... }` también funciona como expresión: el valor del primer
`break <v>` que dispara es el valor de la expresión. Útil para
retry patterns y polling.

```fitz
let counter = 0
let result = loop {
    counter = counter + 1
    if counter == 5 {
        break counter * 10
    }
}
print(result)             // 50

// break sin valor → Null
let nothing = loop { break }
```

`loop` sigue funcionando como statement (sin retorno) para
compatibilidad con código existente.

### Labels en break / continue (mini-tanda L.2)

Para escapar de un loop externo desde un loop anidado, declarás
un label `'name:` antes del loop y lo referenciás en break o
continue:

```fitz
'outer: for i in 0..5 {
    for j in 0..5 {
        if i * j == 6 {
            break 'outer      // sale de los DOS for
        }
    }
}

'main: while (running) {
    if exhausted {
        break 'main
    }
}

// Con loop como expresión + label + valor:
let result = 'top: loop {
    loop {
        if cond {
            break 'top 42     // sale de los dos loops, valor = 42
        }
    }
}
```

Sintaxis paralela a Rust. El label se valida en el lexer
(apóstrofe + identificador) y en el codegen se emite Rust
nativo (`'name: loop {}`, `break 'name expr`).

Ver [examples/guide/08b-loops-avanzados.fitz](../examples/guide/08b-loops-avanzados.fitz)
para el ejemplo completo.

### Lo que todavía no anda

- (nada importante de la lista original — los principales
  faltantes de loops se cerraron en mini-tanda L.)

### Ejemplo completo

[examples/guide/08-loops.fitz](../examples/guide/08-loops.fitz):

```fitz
i = 0
while i < 3 {
    print("i = {i}")
    i = i + 1
}

n = 0
loop {
    if n >= 3 {
        break
    }
    print("n = {n}")
    n = n + 1
}

j = 0
while j < 5 {
    j = j + 1
    if j == 3 {
        continue
    }
    print("j = {j}")
}

fila = 0
while fila < 2 {
    col = 0
    while col < 3 {
        if col == 2 {
            break
        }
        print("({fila},{col})")
        col = col + 1
    }
    fila = fila + 1
}
```

Salida:

```
i = 0
i = 1
i = 2
n = 0
n = 1
n = 2
j = 1
j = 2
j = 4
j = 5
(0,0)
(0,1)
(1,0)
(1,1)
```

---

En el próximo capítulo vamos a las **colecciones** — listas, mapas y
rangos — y al `for ... in` que itera sobre ellas. Después de eso, el
capítulo de `match` cierra la parte de control de flujo.

---

## 9. Listas, mapas y rangos

Hasta acá manejamos valores sueltos. En este capítulo entran las tres
estructuras que te dejan agrupar muchos valores y recorrerlos:
**listas**, **mapas** y **rangos**. Y con ellas llega `for ... in`,
la forma natural de iterar.

### Listas

Una lista es una secuencia ordenada de valores. Se escribe entre
corchetes, separados por coma:

```fitz
nums = [1, 2, 3, 4, 5]
print(nums)
// → [1, 2, 3, 4, 5]
```

Los elementos pueden ser de cualquier tipo, incluso mezclados:

```fitz
mezcla = [1, "dos", true, null, 3.14]
print(mezcla)
// → [1, "dos", true, null, 3.14]
```

La lista vacía es `[]`:

```fitz
vacia = []
print(len(vacia))  // → 0
```

### Acceso por índice

`xs[i]` devuelve el elemento en la posición `i`, base 0:

```fitz
nums = [10, 20, 30]
print(nums[0])  // → 10
print(nums[2])  // → 30
```

Si te pasás del tamaño, el intérprete corta:

```
Error en línea 0:0 — índice fuera de rango: 5 en lista de tamaño 3
```

Los índices negativos al estilo Python (`xs[-1]` para el último)
**no** están soportados todavía — dan error explícito. Si necesitás
el último elemento, hacé `xs[len(xs) - 1]`.

### Mapas

Un mapa asocia claves con valores. Se escribe entre llaves, separando
clave y valor con `:`:

```fitz
user = {"name": "Martín", "age": 43}
print(user)
// → {"name": "Martín", "age": 43}
```

Las claves típicamente son strings, pero podés usar cualquier valor
comparable como clave (`Int`, `Bool`, etc.). El orden de inserción se
preserva: si insertaste `"a"` antes que `"b"`, así se imprime.

Para leer un valor, usá la misma sintaxis de indexing:

```fitz
print(user["name"])  // → Martín
```

Si la clave no existe, el intérprete corta:

```
Error en línea 0:0 — clave no encontrada en mapa: ausente
```

El mapa vacío es `{}`:

```fitz
m = {}
print(len(m))  // → 0
```

### Rangos

Un rango representa una secuencia de enteros entre dos extremos. Se
escribe con dos puntos:

```fitz
r = 0..5
print(r)        // → 0..5
print(len(r))   // → 5
```

El **extremo derecho es exclusivo**: `0..5` representa `0, 1, 2, 3, 4`
(cinco valores). Es la misma convención que Rust o que `range(n)` de
Python. Si el rango va al revés (`10..0`), tiene longitud cero —
nunca itera.

**Rangos inclusivos** con `..=` (R.1.4, mini-fase R):

```fitz
r2 = 0..=5
print(len(r2))   // → 6 — incluye el 5

for i in 0..=10 {
    print(i)     // 0, 1, ..., 10 (11 iteraciones)
}
```

`..=` también funciona en patrones de `match`:

```fitz
fn nota(score: Int) -> Str {
    return match score {
        0..=59 => "F"
        60..=69 => "D"
        70..=79 => "C"
        80..=89 => "B"
        90..=100 => "A"   // matchea 90 hasta 100 inclusive
        _ => "fuera de rango"
    }
}
print(nota(100))   // → "A"
```

Los rangos son valores como cualquier otro: podés asignarlos, pasarlos
a funciones, y compararlos por igualdad. Pero su uso natural es
iterar, que viene ahora.

### `for ... in`

`for var in iterable { body }` recorre los elementos de la lista o
los enteros del rango, una iteración por valor. La variable `var` se
redefine en cada vuelta:

```fitz
for x in [10, 20, 30] {
    print(x)
}
// → 10
// → 20
// → 30

for i in 0..3 {
    print(i)
}
// → 0
// → 1
// → 2
```

`break` y `continue` funcionan igual que en `while`:

```fitz
total = 0
for i in 0..10 {
    if i == 5 {
        break
    }
    total = total + i
}
print(total)   // → 0 + 1 + 2 + 3 + 4 = 10
```

La variable de iteración persiste después del loop (misma política
que el resto de los bloques de Fitz — las variables no crean scope
nuevo):

```fitz
for i in 0..3 {}
print(i)   // → 2 (el último valor antes de que el rango se acabe)
```

Si querés iterar varias dimensiones, anidás:

```fitz
total = 0
for i in 0..3 {
    for j in 0..3 {
        total = total + 1
    }
}
print(total)   // → 9
```

### Iterar Maps con destructuring (mini-tanda Md)

`for` también itera sobre `Map<K, V>`, produciendo un par `(k, v)`
por cada iteración. El patrón canónico es destructurar en el binding:

```fitz
let inventario: Map<Str, Int> = {"manzanas": 5, "peras": 3}

for (fruta, cantidad) in inventario {
    print("{fruta}: {cantidad}")
}
```

El orden de iteración es el orden de inserción del Map (Fitz preserva
inserción order). El `for ... in` toma un snapshot del Map antes de
iterar, así que mutar el Map durante el loop no afecta la iteración.

Para casos donde querés ignorar un campo (o todo el elemento), usá `_`:

```fitz
let suma: Int = 0
for (_, v) in inventario {       // solo valores
    suma = suma + v
}

for _ in 0..5 {                   // solo contar
    print("tick")
}
```

El `_` no bindea nada — útil con `for _ in 0..N` para "repetir N veces".

Si necesitás el par como `Tuple` (sin destructurar), usá un Ident
solo (solo `fitz run`; el codegen exige tuple pattern):

```fitz
for kv in inventario {
    print("{kv.0} = {kv.1}")     // accedés por .0/.1
}
```

Ver [examples/guide/09e-for-map.fitz](../examples/guide/09e-for-map.fitz)
para el ejemplo completo, validado bit-a-bit `fitz run` ↔ `fitz build`.

### Patrón de rango en `match` (adelanto)

Los rangos también se pueden usar como **patrones** en `match`, para
clasificar un `Int` en bandas:

```fitz
fn clasificar(n) {
    return match n {
        0..10   => "chico"
        10..100 => "mediano"
        _       => "grande"
    }
}
```

Esto se ve en detalle en el [próximo capítulo](#10-match).

### Anidación

Listas, mapas y rangos se combinan libremente:

```fitz
matriz = [[1, 2, 3], [4, 5, 6], [7, 8, 9]]
print(matriz[1][2])   // → 6

usuarios = [
    {"name": "Ana", "age": 30},
    {"name": "Beto", "age": 25},
]
print(usuarios[0]["name"])   // → Ana
```

### Asignación a índice

Mutación posicional con `xs[i] = v` y `m["k"] = v` (R.1.3,
mini-fase R):

```fitz
let xs = [1, 2, 3, 4]
xs[0] = 99           // replace
xs[2] = 77
print(xs)            // [99, 2, 77, 4]

let m = {"a": 1, "b": 2}
m["a"] = 10          // replace existente
m["c"] = 3           // insert nuevo al final (preserva insertion order)
print(m)             // {"a": 10, "b": 2, "c": 3}
```

**Listas**: el índice tiene que ser `Int` en rango `[0, len)`.
Out-of-bounds o negativo → error runtime claro
(`índice 5 fuera de rango (lista de tamaño 2)`). Los binarios
producidos por `fitz build` emiten el mismo error (no panic
crudo de Rust).

**Maps**: la clave puede ser cualquier tipo hashable (Str, Int,
Bool, Float, Null). Si la clave existe, se sobreescribe sin
mover su posición; si no existe, se inserta al final. El
insertion order se preserva igual que en literales.

Combinado con índice computado:

```fitz
let nums = [1, 2, 3]
let i = 0
while (i < 3) {
    nums[i] = nums[i] * 10
    i = i + 1
}
print(nums)          // [10, 20, 30]
```

### Indexing y slicing (mini-tanda I)

Listas y strings soportan **índices negativos** y **slicing**
(post-mini-tanda I).

**Índices negativos**: `xs[-1]` es el último, `xs[-2]` el
penúltimo, etc. (igual que Python). Resolución: `effective =
len + i`. Out-of-range → error claro.

```fitz
let xs = [10, 20, 30, 40, 50]
print(xs[-1])             // 50
print(xs[-2])             // 40

let s = "fitz"
print(s[0])               // "f"
print(s[-1])              // "z"

xs[-1] = 99               // asignación con negativo también
```

`s[i]` sobre `Str` devuelve un `Str` de un char (Fitz no tiene
tipo Char). Cuenta CHARS, no bytes (consistente con
`s.len()`).

**Slicing**: `xs[a..b]`, `xs[..b]`, `xs[a..]`, `xs[..]`,
`xs[a..=b]`. Funciona para listas y strings; devuelve siempre
una **copia** (mutar el slice no afecta al original).

```fitz
let xs = [10, 20, 30, 40, 50]
print(xs[1..3])           // [20, 30]
print(xs[..2])            // [10, 20]
print(xs[3..])            // [40, 50]
print(xs[..])             // [10, 20, 30, 40, 50] (copia)
print(xs[1..=3])          // [20, 30, 40]
print(xs[-2..])           // [40, 50]

let s = "hola fitz"
print(s[..4])             // "hola"
print(s[-4..])            // "fitz"
```

**Clamp** silencioso para slices fuera de rango (estilo Python):
`xs[100..]` con `len=5` → `[]`. `xs[..100]` → copia entera. Si
`start > end` tras clamp → vacío.

Ver [examples/guide/09b-indexing-slicing.fitz](../examples/guide/09b-indexing-slicing.fitz).

### Lo que todavía no anda

- **Métodos sobre listas y mapas** — `xs.push(...)`, `xs.map(...)`,
  `m.get(...)`, etc. ya están vivos desde el paso 4 de Fase 3.
  Los ves en el [capítulo 13](#13-métodos-y-mutación).
- **`for` sobre mapas**: necesita el tipo `Pair`/`entry`. Si lo
  intentás, el intérprete corta:

  ```
  Error — `for` sobre Map aún no soportado — necesita el tipo Pair
  ```

- **Comprehensions** (`[x * 2 for x in xs]`).
- **Slicing con paso** (`xs[::2]`) — sin demanda concreta.

### Tuples (mini-tanda T)

Tipos compuestos heterogéneos de tamaño fijo, similares a Rust.
Útil para retornos múltiples y agrupar valores ad-hoc sin
declarar un `type`.

```fitz
// Literal — la coma distingue tuple de paréntesis de agrupación.
let pair: (Int, Str) = (42, "fitz")
print(pair.0)             // 42
print(pair.1)             // fitz

// Tupla vacía (unit) y de 1 elemento:
let unit: () = ()
let single = (42,)        // trailing comma obligatoria

// Retornos múltiples:
fn divmod(a: Int, b: Int) -> (Int, Int) {
    return (a / b, a % b)
}

// Destructuring con `let`:
let (q, r) = divmod(17, 5)
print(q)                  // 3
print(r)                  // 2

// Wildcards y nesting:
let (a, _, c) = (10, 20, 30)
let ((x, y), z) = ((1, 2), 3)

// Tuple pattern en match:
fn clasif(p: (Int, Int)) -> Str {
    return match p {
        (0, 0) => "origen"
        (0, _) => "eje Y"
        (_, 0) => "eje X"
        (a, b) => "({a}, {b})"
    }
}
```

**Limitaciones del MVP**:
- En `fitz build`, los tuple patterns en match no admiten
  literales `Str`/`Range`/Or como sub-pattern (`("ada", n)` no
  compila; el intérprete sí lo acepta). Workaround: usar bind +
  guard. `(name, n) if name == "ada"`.
- `let (a, b) = ...` solo admite `Ident`, `_` y tuple
  patterns anidados — no literales ni Ok/Err.
- Tuples como llave de Map: no soportado por ahora.

Ver [examples/guide/09c-tuples.fitz](../examples/guide/09c-tuples.fitz)
para el ejemplo completo.

### List comprehensions (mini-tanda C)

Sintaxis compacta para construir listas derivadas. Azúcar sobre
los patrones `.map()` y `.filter().map()` — útil cuando el dato
viene directo de otro iterable y querés transformarlo/filtrarlo.

```fitz
// Simple — el equivalente a `.map(fn(x) => x * 2)`.
let doublados: List<Int> = [x * 2 for x in [1, 2, 3]]
// → [2, 4, 6]

// Sobre Range — los rangos son iterables igual que listas.
let cuadrados: List<Int> = [n * n for n in 0..5]
// → [0, 1, 4, 9, 16]

// Con filter inline — `if cond` al final.
let pares: List<Int> = [n for n in 0..10 if n % 2 == 0]
// → [0, 2, 4, 6, 8]

// Expr compuesta — strings interpolados, llamadas, lo que sea.
let etiquetados: List<Str> = ["item-{i}" for i in 0..3]
// → ["item-0", "item-1", "item-2"]
```

**Scope local del var** (a diferencia del `for ... in`):

```fitz
let i: Int = 100
let _: List<Int> = [i for i in 0..3]   // el `i` de adentro es nuevo
print(i)                                // 100 — el original intacto
```

Las comprehensions abren un scope dedicado para el binding del
`for`. Esto es lo que hace Python y evita shadowear variables del
scope contenedor sin querer. El `for ... in` clásico de Fitz NO
tiene esta propiedad (su var queda visible afuera del loop) —
diferencia documentada pero intencional.

**Cobertura del MVP**:
- Una sola `for` clause (no `[x*y for x in xs for y in ys]`).
- El `var` es un solo identificador (no destructuring de
  tuples — `[a+b for (a, b) in pairs]` queda como deuda).
- `iter` puede ser `List<T>` o `Range` (igual que `for ... in`).
- Filter inline `if cond` opcional al final.

Ver [examples/guide/09d-comprehensions.fitz](../examples/guide/09d-comprehensions.fitz)
para el ejemplo completo y validado bit-a-bit `fitz run` ↔
`fitz build`.

> Lo que **sí anda** y antes era deuda (mini-tanda I post-S):
> **índices negativos** `xs[-1]` para listas y strings + **slicing**
> `xs[a..b]`, `xs[..b]`, `xs[a..]`, `xs[..]`, `xs[a..=b]` (con
> clamp silencioso). **Tuples** `(T1, T2)` con acceso `.0`/`.1`,
> destructuring `let (a, b) = ...`, y `Pattern::Tuple` en match
> (mini-tanda T). **Comprehensions** `[expr for var in iter]` con
> filter inline opcional (mini-tanda C). Ver las sub-secciones de
> arriba.

> Lo que **sí anda** y antes era deuda: **asignación a índice**
> (R.1.3) — ver sección "Asignación a índice" arriba. **Rangos
> inclusivos** (`0..=10`) cerrado en R.1.4 (mini-fase R). Ahora
> `for i in 0..=10` itera 0..10 inclusive, y `match n { 0..=100
> => ... }` matchea ambos extremos.

### Ejemplo completo

[examples/guide/09-listas-mapas.fitz](../examples/guide/09-listas-mapas.fitz):

```fitz
nums = [1, 2, 3, 4, 5]
print(nums)
print("primer: {nums[0]}")
print("último: {nums[4]}")
print("cantidad: {len(nums)}")

mezcla = [1, "dos", true, null, 3.14]
print(mezcla)

vacia = []
print("vacía: {vacia}, len: {len(vacia)}")

user = {"name": "Martín", "age": 43}
print(user)
print("nombre: {user[\"name\"]}")

items = {"primero": 1, "segundo": 2, "tercero": 3}
print(items)

r = 0..5
print(r)
print("cantidad: {len(r)}")

total = 0
for n in nums {
    total = total + n
}
print("suma de nums: {total}")

print("contando:")
for i in 0..5 {
    print("  {i}")
}

fn clasificar(n) {
    return match n {
        0..10  => "chico"
        10..100 => "mediano"
        100..1000 => "grande"
        _ => "fuera"
    }
}
print(clasificar(3))
print(clasificar(42))
print(clasificar(500))
print(clasificar(99999))
```

---

En el próximo capítulo cerramos control de flujo con `match`:
patrones literales, binding por identificador, `_`, y los **patrones
de rango** que recién vimos en acción.

---

## 10. Match

`match` compara un valor contra una serie de **patrones** y ejecuta el
primero que coincide. Es la herramienta natural cuando estás haciendo
un `else if` tras otro, todos comparando contra la misma variable.

### Forma general

```fitz
match valor {
    patron1 => expresion_o_bloque
    patron2 => otra_cosa
    _       => default
}
```

El `=>` (fat arrow) separa el patrón de lo que se ejecuta cuando
coincide. Cada brazo termina con un newline; no hace falta coma.

### Patrones literales

Los cinco tipos primitivos se pueden usar tal cual como patrón:

```fitz
status = "active"
match status {
    "active"   => print("activo")
    "inactive" => print("inactivo")
    _          => print("desconocido")
}
```

Funciona con `Int`, `Float`, `Str`, `Bool` y `Null`. Los enteros y
floats negativos también:

```fitz
n = -1
match n {
    0  => print("cero")
    -1 => print("menos uno")
    _  => print("otro")
}
```

Y con `null`:

```fitz
match value {
    null => print("vacío")
    _    => print("algo")
}
```

### Binding con identificador

Un patrón que es un identificador "captura" el valor y lo deja
disponible dentro del brazo. Es decir, **funciona como un default que
además le pone nombre al valor**:

```fitz
val = 42
match val {
    0 => print("cero")
    x => print("otro: {x}")    // x toma el valor 42
}
```

Importante: como el identificador siempre matchea, **tiene que ir al
final**. Si lo ponés primero, todos los brazos siguientes son código
muerto — y el intérprete no te avisa de eso todavía.

### Wildcard `_`

`_` es el "no me importa el valor". Equivale al binding pero sin
darle nombre. Convención: usá `_` cuando no necesitás el valor en el
brazo, y un identificador cuando sí.

```fitz
match status_code {
    200 => print("OK")
    404 => print("no encontrado")
    _   => print("otro código")
}
```

### Como expresión

Igual que `if`, `match` también es una expresión:

```fitz
day = 3
name = match day {
    1 => "lunes"
    2 => "martes"
    3 => "miércoles"
    _ => "otro día"
}
print(name)        // miércoles
```

El valor de cada brazo es el de la expresión a la derecha del `=>`.
Si querés un bloque, podés usar llaves (mismo patrón que en `if`):

```fitz
result = match n {
    0 => "cero"
    x => {
        let etiqueta = "número"
        "{etiqueta} {x}"
    }
}
```

### ¿Qué pasa si nada coincide?

Si ningún brazo coincide, el intérprete corta:

```
Error en línea 0:0 — el `match` no matcheó ningún brazo
```

Desde la Fase 5.3.3, **`fitz check` exige exhaustividad cuando el
valor matcheado tipa como `Result<T>`** — vas a ver un error si te
falta el caso `Ok` o el caso `Err` (a menos que tengas un `_` o un
binding final que actúe como catch-all). Para los demás tipos
(`Int`, `Str`, etc.), la exhaustividad todavía es tu
responsabilidad y la regla práctica sigue siendo: si el conjunto de
valores posibles no está acotado, terminá siempre con `_`.

### Cuándo usar match vs if / else if

Como guía:

- Si estás comparando **el mismo valor** contra distintas constantes,
  `match` es más legible.
- Si tus condiciones son **diferentes** entre sí (rangos, expresiones
  con varias variables, llamadas a función), seguí con `if` /
  `else if`.

Ejemplo donde `match` claramente gana:

```fitz
// Con if / else if:
if status == "active" {
    print("activo")
} else if status == "inactive" {
    print("inactivo")
} else if status == "pending" {
    print("pendiente")
} else {
    print("desconocido")
}

// Con match:
match status {
    "active"   => print("activo")
    "inactive" => print("inactivo")
    "pending"  => print("pendiente")
    _          => print("desconocido")
}
```

### Patrones de rango

Para los `Int`, podés usar un rango como patrón. Matchea si el valor
es `Int` y cae adentro del rango (con la cota derecha **exclusiva**,
igual que el operador `..`):

```fitz
fn clasificar(n) {
    return match n {
        0..10   => "chico"
        10..100 => "mediano"
        _       => "grande"
    }
}

print(clasificar(5))    // → chico
print(clasificar(10))   // → mediano  (10 no entra en 0..10)
print(clasificar(500))  // → grande
```

Los extremos negativos también son válidos:

```fitz
match temperatura {
    -50..0 => print("bajo cero")
    0..10  => print("frío")
    10..25 => print("templado")
    _      => print("calor")
}
```

Si el valor no es `Int` (por ejemplo, un `Float`), el patrón de
rango simplemente no matchea, y se evalúa el siguiente brazo.

### `Ok(x)` / `Err(e)` — un adelanto

Si el valor matcheado es un `Result`, los patrones `Ok(v)` y `Err(e)`
matchean cada variante y bindean el inner:

```fitz
match find_user(1) {
    Ok(u)  => print("hola, {u.name}")
    Err(e) => print("falló: {e}")
}
```

Lo cubrimos en detalle en [el próximo capítulo](#13-result-y-manejo-de-errores).

### Or-patterns `pat1 | pat2 | pat3`

Cuando varios patrones llevan al mismo body, escribilos separados
por `|` en un solo arm (R.2.1, mini-fase R). El arm matchea si
**cualquiera** de los sub-patrones matchea:

```fitz
match dia {
    "lun" | "mar" | "mie" | "jue" | "vie" => "laboral"
    "sab" | "dom" => "fin de semana"
    _ => "?"
}
```

Funciona con literales, rangos y wildcards de Result:

```fitz
match n {
    0 | 1 | 2 => "muy chico"
    3..=10 | 100..=1000 => "medio o muy grande"
    _ => "otro"
}

match r {
    Ok(_) | Err(_) => "cualquier resultado"
}
```

**Restricción**: los sub-patrones de un or-pattern **no pueden
bindear** (igual que Rust). `1 | x => ...` y `Ok(v) | Err(_) =>
...` son errores de parser; usá `_` o desdoblá el arm si necesitás
binding distinto por caso.

### Guards `pat if cond =>`

Un guard es una condición extra que se chequea **después** de que
el pattern matchee (R.2.2, mini-fase R). El arm matchea si el
pattern matchea Y el guard evalúa a `true`:

```fitz
match age {
    a if a < 0 => "edad inválida"
    a if a < 13 => "niño"
    a if a < 18 => "adolescente"
    a if a < 65 => "adulto"
    _ => "mayor"
}
```

El binding del pattern es visible adentro del guard:

```fitz
match r {
    Ok(v) if v > 0 => "positivo"
    Ok(v) if v == 0 => "cero"
    Ok(_) => "negativo"
    Err(_) => "error"
}
```

**Exhaustividad**: los arms con guard NO cuentan para la
exhaustividad de `Result` — el guard puede ser `false` y dejar el
match incompleto. Si todos tus arms tienen guard, el checker te
exige un catch-all (`_` o ident) al final:

```fitz
// Error de checker: match no exhaustivo, falta el caso Err
let s = match r { Ok(_) if true => "x" }
```

### Tuple patterns con sub-patterns ricos (mini-tanda Rt)

Adentro de un tuple pattern (`(a, b)`), los sub-patterns ahora
admiten Str literal, Range, y Or-pattern. Antes de Rt, los
sub-patterns con guard no andaban en `fitz build` (el workaround
era bind + guard manual):

```fitz
fn clasif(p: (Str, Int)) -> Str {
    return match p {
        ("ada", 1)        => "ada uno",       // Str + Int literal
        ("ada", n)        => "ada otro: {n}", // Str literal + bind
        (name, 1 | 2)     => "{name} chico",  // Bind + Or-pattern
        (name, 0..10)     => "{name} dig",    // Bind + Range
        (name, n) if n > 100 => "{name} grande: {n}",
        (name, n)         => "{name}: {n}"
    }
}
```

Funciona bit-a-bit en `fitz run` y `fitz build`. El codegen
sintetiza nombres únicos `__s_<n>`/`__n_<n>`/`__or_v_<n>` por
slot del tuple para que dos sub-patterns con guard no choquen.

Combinable con guards explícitos (`if cond`) en el mismo arm,
con paréntesis anidados (`((a, b), c)`), y con cualquier
combinación de Ident/Wildcard/literal/Range/Or.

Ver [examples/guide/10b-match-tuple-subpatterns.fitz](../examples/guide/10b-match-tuple-subpatterns.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

### Lo que todavía no anda

- **Listas como patrón** — `[head, ...rest]`, etc. Sin demanda
  concreta.
- **Exhaustividad para tipos no-Result** — desde Fase 5.3.3
  `fitz check` exige exhaustividad sobre `Result<T>`. Para Int,
  Str y otros tipos no acotados sigue siendo responsabilidad
  tuya cerrar con `_`.

> Lo que **sí anda** y antes era deuda: or-patterns (R.2.1),
> guards (R.2.2), tuple patterns (mini-tanda T), y **tuple
> patterns con sub-patterns Str/Range/Or en `fitz build`**
> (mini-tanda Rt — ver sub-sección de arriba).

### Ejemplo completo

[examples/guide/10-match.fitz](../examples/guide/10-match.fitz):

```fitz
status = "active"
match status {
    "active"   => print("activo")
    "inactive" => print("inactivo")
    _          => print("desconocido")
}

n = -1
match n {
    0  => print("cero")
    -1 => print("menos uno")
    _  => print("otro")
}

val = 42
match val {
    0 => print("cero")
    x => print("otro: {x}")
}

day = 3
name = match day {
    1 => "lunes"
    2 => "martes"
    3 => "miércoles"
    _ => "otro día"
}
print(name)

match 3.14 {
    3.14 => print("pi")
    _    => print("otro")
}

match true {
    true  => print("sí")
    false => print("no")
}

match null {
    null => print("vacío")
    _    => print("algo")
}
```

Salida:

```
activo
menos uno
otro: 42
miércoles
pi
sí
vacío
```

---

Con esto cerramos la parte de control de flujo. En el próximo
capítulo entramos a las funciones: `fn` bloque, `=>` flecha,
parámetros, recursión, y closures con captura léxica.

---

## 11. Funciones

Una función agrupa una serie de pasos bajo un nombre, recibe entradas
(parámetros) y devuelve un valor. En Fitz hay dos formas de
escribirlas, ambas con la misma palabra clave: `fn`.

### Forma de bloque

La forma "larga" con llaves y `return`:

```fitz
fn greet(name) {
    return "Hola, {name}!"
}
print(greet("Fitz"))     // Hola, Fitz!
```

El bloque puede tener varias sentencias; el valor se devuelve con
`return`.

### Forma flecha

Cuando el cuerpo es **una sola expresión**, podés usar `=>` y saltearte
las llaves y el `return`:

```fitz
fn double(n) => n * 2
print(double(21))        // 42
```

Es el mismo concepto que las arrow functions de JavaScript o las
lambdas de Python: azúcar sintáctica para funciones cortas. La regla
mental: si la función es `return expresion`, escribilo con `=>
expresion`.

### Parámetros y anotaciones

Los parámetros van entre paréntesis, separados por coma. Como con las
variables, podés anotarles el tipo, y también podés anotar el tipo de
retorno después de `->`:

```fitz
fn add(a: Int, b: Int) -> Int {
    return a + b
}
print(add(2, 3))         // 5
```

Las anotaciones se **parsean pero todavía no se validan**, igual que
para variables (cap. 3). Cuando llegue el type checker (Fase 5), van
a empezar a chequearse.

### `return` con y sin valor

`return` corta la ejecución de la función y devuelve un valor:

```fitz
fn abs(n) {
    if n < 0 {
        return -n
    }
    return n
}
```

Si llamás `return` sin expresión, devuelve `null`:

```fitz
fn shout(msg) {
    print(msg + "!")
    return                // equivale a `return null`
}
```

Y si la función termina sin pasar por ningún `return`, también devuelve
`null`:

```fitz
fn nothing() {
    let x = 1
}
print(nothing())          // null
```

Hoy no hay un tipo "void" separado — todo lo que no sea un valor
explícito es `null`.

### Aridad estricta

Si llamás una función con una cantidad incorrecta de argumentos, el
intérprete corta:

```
Error en línea 0:0 — `add` espera 2 argumento(s), recibió 1
```

No hay argumentos opcionales, ni con valor por defecto, ni varargs
todavía. Esa es deuda explícita; por ahora la firma manda.

### Recursión

Una función puede llamarse a sí misma. El clásico factorial:

```fitz
fn fact(n) {
    if n <= 1 {
        return 1
    }
    return n * fact(n - 1)
}
print(fact(5))           // 120
```

No hay tail call optimization todavía, así que recursiones muy
profundas pueden agotar la pila — pero para los usos típicos no es
problema.

### Closures: capturar el scope externo

Una función definida adentro de otra ve las variables de la externa.
Esto te permite "fabricar" funciones que llevan estado:

```fitz
fn make_adder(x) {
    fn add(y) => x + y   // `x` viene del scope de make_adder
    return add
}

add5 = make_adder(5)
print(add5(3))           // 8
```

`add5` es una función que "recuerda" que `x` valía 5. Eso es un
**closure**: la función más su entorno léxico.

### Captura por referencia

Importante saber esto, porque sorprende. El closure no congela el
valor de la variable en el momento de definirse — guarda una
**referencia al entorno**. Si la variable cambia después, el closure
ve el nuevo valor:

```fitz
counter_val = 10
fn show() => counter_val
counter_val = 20
print(show())            // 20, no 10
```

Es el mismo comportamiento que en Python y JavaScript. Si querés
"capturar por valor", la práctica común es pasar la variable como
parámetro de la función fabricante (como en `make_adder` arriba): así
el valor queda fijado en cada llamada.

### Funciones como valor

Una función es un valor más. La podés guardar en una variable, pasarla
como argumento, devolverla desde otra función:

```fitz
fn apply(f, x) => f(x)
fn square(n) => n * n

print(apply(square, 7))  // 49
```

### Funciones anónimas inline

Cuando una función es lo bastante chica como para no merecer un
nombre — típicamente una callback que pasás a otra función —
podés definirla "al vuelo" con la misma palabra clave `fn`, pero
sin nombre:

```fitz
let cuadrado = fn(n) => n * n
print(cuadrado(7))                       // 49
```

La forma de flecha es la típica, pero la forma de bloque también
existe:

```fitz
let abs = fn(n) {
    if (n < 0) {
        return -n
    }
    return n
}
print(abs(-5))                           // 5
```

La utilidad real aparece cuando pasás la anónima como argumento:

```fitz
fn apply(f, x) => f(x)
print(apply(fn(n) => n * 10, 7))          // 70
```

Y se vuelven ergonómicas con los métodos sobre listas y mapas (cap.
13): `xs.map(fn(n) => n * 2)`, `users.find(fn(u) => u.id == 1)`.

Una anónima es una función como cualquier otra, así que también
captura el scope donde se definió — lo mismo que con las nombradas.

```fitz
let factor = 3
let triplicar = fn(n) => n * factor
print(triplicar(5))                       // 15
```

### Lo que todavía no anda

- **Parámetros con default** (`fn greet(name = "amigo") { ... }`).
- **Varargs** (`fn sum(...xs)`).
- **Argumentos nombrados al llamar** (`greet(name: "Fitz")`).
- **Métodos custom sobre `type`** (`type User { ... fn greet() => "Hola" }`)
  — los built-in sobre listas, mapas, strings ya andan (cap. 13),
  pero declarar métodos propios sobre tus tipos sigue siendo deuda.

### Ejemplo completo

[examples/guide/11-funciones.fitz](../examples/guide/11-funciones.fitz):

```fitz
fn greet(name: Str) -> Str {
    return "Hola, {name}!"
}
print(greet("Fitz"))

fn double(n: Int) -> Int => n * 2
print(double(21))

fn add(a: Int, b: Int) -> Int {
    return a + b
}
print(add(2, 3))

fn nothing() {
    let x = 1
}
print(nothing())

fn fact(n: Int) -> Int {
    if (n <= 1) {
        return 1
    }
    return n * fact(n - 1)
}
print(fact(5))

// Closure: la fn interna captura `x` del scope externo.
fn make_adder(x: Int) -> Fn(Int) -> Int {
    return fn(y: Int) => x + y
}
let add5 = make_adder(5)
print(add5(3))

// Pasar funciones como argumento.
fn square(n: Int) -> Int => n * n
fn apply(f: Fn(Int) -> Int, x: Int) -> Int => f(x)
print(apply(square, 7))

// Funciones anónimas inline.
print(apply(fn(n: Int) => n * 10, 7))
let abs = fn(n: Int) -> Int {
    if (n < 0) {
        return -n
    }
    return n
}
print(abs(-5))
```

> **Sobre las anotaciones**: con `fitz run` son opcionales — el
> intérprete infiere desde el body. Con `fitz build` el subset
> compilable las exige en params y retorno (deuda 5b.1). El
> ejemplo lleva anotaciones para que compile a binario igual de
> bit-a-bit con `fitz build` que con `fitz run`. El tipo `Fn(Int)
> -> Int` describe una función que toma un `Int` y devuelve un
> `Int` — es el tipo que tienen `square`, `make_adder(5)`, etc.

Salida:

```
Hola, Fitz!
42
5
null
120
8
49
70
5
```

---

Con funciones ya tenés todo lo necesario para escribir programas
completos. En el próximo capítulo entramos a `type`: cómo declarar
tus propios tipos, instanciarlos y acceder a sus campos.

---

## 12. Tipos con `type`

Hasta acá tu código modeló datos sueltos: ints, strings, listas,
mapas. Para varios casos eso alcanza, pero apenas tenés "un usuario
con id, nombre y email" empezás a pasar tres cosas relacionadas como
si fueran independientes. Los **tipos custom** son la forma de
nombrar esa relación: declarás una vez la forma de un dato, y después
trabajás con instancias enteras.

### Declarar un tipo

Un `type` define una estructura nueva — un conjunto de campos con
nombre y tipo:

```fitz
type User {
    id: Int
    name: Str
    email: Str?
    active: Bool = true
}
```

Notas sobre la sintaxis:

- Los campos se separan con **newline** (o con coma, también). No hace
  falta `;`.
- Cada campo declara `nombre: Tipo`.
- Un `?` después del tipo lo marca como **nullable** (el campo puede
  ser `null`). Esto solo se permite hoy en campos de `type`, todavía
  no en anotaciones de variables sueltas.
- Después de `=` podés dar un **valor por defecto**.

Otro ejemplo, para algo tipo configuración:

```fitz
type Config {
    host: Str
    port: Int = 3000
    debug: Bool = false
}
```

Una declaración de `type` no crea ninguna instancia por sí sola — solo
registra la forma. Para crear datos concretos, usás un **struct
literal**.

### Instanciar un tipo

```fitz
let u = User {
    id: 1,
    name: "Fitz",
    email: "fitz@example.com",
}
```

Los campos van entre llaves, separados por coma o newline, con la
forma `nombre: valor`. El valor puede ser cualquier expresión:

```fitz
let p = Point { x: 1 + 2, y: f(3) }
```

El **orden** en el literal es libre — la instancia se ordena según la
declaración del `type`. Así dos instancias del mismo tipo se imprimen
igual, sin importar en qué orden las tipeaste:

```fitz
let a = User { id: 1, name: "Fitz" }
let b = User { name: "Fitz", id: 1 }
// a y b son iguales (==) y se imprimen idéntico.
```

### Acceder a campos

Con `.` sacás un campo de una instancia:

```fitz
print(u.name)     // Fitz
print(u.email)    // fitz@example.com
```

Funciona encadenado, si un campo es otra instancia:

```fitz
type Order {
    user: User
    total: Int
}

let o = Order { user: u, total: 100 }
print(o.user.name)    // Fitz
```

Si pedís un campo que no existe, el intérprete corta:

```fitz
print(u.color)
// Error en línea 0:0 — el tipo `User` no tiene un campo llamado `color`
```

### Defaults

Si un campo tiene un valor por defecto en la declaración, podés
omitirlo al instanciar y se aplica el default:

```fitz
let c = Config { host: "localhost" }
print(c.port)     // 3000
print(c.debug)    // false
```

Los defaults son **expresiones** y se evalúan **cuando instanciás**,
no cuando declarás el tipo. Eso permite cosas como derivar el default
de una variable del scope:

```fitz
let base = 4000
type Cfg { port: Int = base + 1 }

let c = Cfg {}
print(c.port)     // 4001
```

**Defaults de tipos importados.** Cuando un tipo se exporta a otro
archivo vía `from foo import T`, sus defaults pueden referenciar
consts u otros símbolos del módulo de origen sin que el importer los
tenga que re-importar. El loader los pre-evalúa en el env del módulo
de origen al cargarlo, así `T {}` desde el importer ya tiene los
valores resueltos.

```fitz
// foo.fitz
let MAX = 99
type User { id: Int = MAX }
```

```fitz
// main.fitz
from foo import User    // no hace falta `from foo import MAX`
let u = User {}
print(u.id)             // 99
```

### Campos nullables

Un campo declarado con `Tipo?` puede valer `null`. Si lo omitís al
instanciar, queda en `null` automáticamente:

```fitz
let anon = User { id: 2, name: "Anon" }
print(anon.email)    // null
```

También podés ponerlo explícito:

```fitz
let anon = User { id: 2, name: "Anon", email: null }
```

Si un campo **no** es nullable y **no** tiene default, omitirlo es
error:

```fitz
let u = User { id: 1 }
// Error en línea 0:0 — falta el campo `name` al instanciar `User`
//                     (no tiene default y no es nullable)
```

Y si pasás un campo que no está declarado en el tipo, también es
error:

```fitz
let u = User { id: 1, name: "x", color: "red" }
// Error en línea 0:0 — el tipo `User` no tiene un campo llamado `color`
```

### Instancias en condiciones — usá paréntesis

Esto es la única fricción de sintaxis a tener en cuenta. Mirá:

```fitz
if User { id: 1 } == other { print("igual") }
```

¿Dónde termina la condición y dónde empieza el bloque del `if`? El
parser no tiene cómo adivinarlo sin lookahead arbitrario, así que
**los struct literals no se permiten directamente** como condición
de `if`, `while`, `for` o `match`. Si los tipeás ahí, el intérprete
te corta con un mensaje claro:

```
Error en línea 1:11 — los struct literals no se permiten directamente
en condiciones de if/while/for/match — envolvélo en paréntesis:
`(User { id: 1 })`
```

La solución es exactamente lo que el mensaje dice: envolver el struct
literal en paréntesis.

```fitz
if (User { id: 1 }) == other { print("igual") }
```

Adentro de paréntesis, listas (`[User { id: 1 }]`), argumentos de
llamada (`print(User { id: 1 })`) e indexing (`m[Key { id: 1 }]`) los
struct literals están permitidos sin envolver — no hay ambigüedad
porque cada uno de esos contextos tiene un cierre propio.

Es el mismo trade-off que hacen Rust y Go.

### Comparar instancias

`==` compara instancias **estructuralmente**: mismo tipo y mismos
valores en los mismos campos. La coerción Int↔Float que vimos en el
cap. 4 sigue valiendo dentro de los campos.

```fitz
let a = User { id: 1, name: "Fitz" }
let b = User { id: 1, name: "Fitz" }
let c = User { id: 1, name: "Otro" }

print(a == b)    // true
print(a == c)    // false
```

Dos instancias de tipos distintos son siempre desiguales aunque
tengan la misma forma:

```fitz
type Admin { id: Int, name: Str }

let user  = User  { id: 1, name: "x" }
let admin = Admin { id: 1, name: "x" }
print(user == admin)    // false
```

### Imprimir instancias

`print(u)` muestra el formato canónico — nombre del tipo, llaves,
campos en orden de declaración:

```
User { id: 1, name: "Fitz", email: "fitz@example.com", active: true }
```

Los strings adentro van con comillas (mismo criterio que listas y
mapas), para distinguir `1` de `"1"`. La interpolación de un campo
suelto en un string sigue sin comillas, como cualquier `Str`:

```fitz
print("Hola, {u.name}!")    // Hola, Fitz!
```

### Lo que todavía no anda

- **Métodos custom sobre `type`** (`type User { ... fn greet() => ... }`)
  — hoy todo método propio se hace con funciones aparte que reciben
  la instancia como parámetro. Los métodos built-in (sobre `List`,
  `Map`, `Str`) ya están vivos: ver el [próximo capítulo](#13-métodos-y-mutación).
- **Herencia / composición de tipos** — un `type` no puede heredar
  campos de otro. Los structs son planos. Para compartir campos,
  por ahora repetirlos o anidarlos (`type Order { user: User, ... }`).
- **Trait-like polymorphism** — no hay interfaces / traits. Si
  necesitás polimorfismo, hoy es vía `match` sobre un enum tipo
  `type Shape { ... }` con un campo discriminador.

> Lo que **sí anda** y antes era deuda (cerrado fase tras fase):
> chequeo estático de anotaciones contra valores (Fase 5a — `let
> x: Int = "hola"` ahora falla en `fitz check`), genéricos
> compuestos en campos (`List<Str>`, `Map<Str, User>`, etc.,
> validados por el checker desde Fase 5.1), defaults que
> referencian otros símbolos del módulo de origen (PreF8.3).

### Ejemplo completo

[examples/guide/12-type.fitz](../examples/guide/12-type.fitz):

```fitz
type User {
    id: Int
    name: Str
    email: Str?
    active: Bool = true
}

type Config {
    host: Str
    port: Int = 3000
    debug: Bool = false
}

let u = User { id: 1, name: "Fitz", email: "fitz@example.com" }
print(u.name)
print(u.email)

let c = Config { host: "localhost" }
print(c.port)

let anon = User { id: 2, name: "Anon" }
print(anon.email)

print(u)
print(c)
```

Salida:

```
Fitz
fitz@example.com
3000
null
User { id: 1, name: "Fitz", email: "fitz@example.com", active: true }
Config { host: "localhost", port: 3000, debug: false }
```

---

En el próximo capítulo entramos en métodos: la sintaxis `receptor.metodo(args)`
sobre listas, mapas, strings e instancias, y cómo funciona la
mutación en Fitz.

---

## 13. Métodos y mutación

Hasta acá las listas, los mapas y las strings los manejabas con
operaciones globales: `len(xs)`, `for n in xs`, indexing con `[]`.
Funciona, pero a veces lo natural es escribirlo en orden "objeto
primero": `xs.len()`, `xs.map(...)`. Eso son **métodos**: funciones
que se llaman sobre un valor con la sintaxis `receptor.metodo(args)`.

### Por qué método y no función suelta

Misma operación, dos formas de escribirla:

```fitz
let xs = [1, 2, 3, 4]

// Función global.
print(len(xs))                     // 4

// Método sobre la lista.
print(xs.len())                    // 4
```

Ambas formas valen y, en este caso, hacen lo mismo. La forma de
método brilla en cadenas: `xs.map(...).filter(...)` se lee de
izquierda a derecha, paso a paso, sin paréntesis anidados.

En Fitz, los métodos se resuelven por **el tipo del receptor**: hay
una tabla interna que sabe qué métodos tiene `List`, qué métodos
tiene `Map`, qué métodos tiene `Str`, etc. Si llamás un método que
no existe para ese tipo, el intérprete te corta con un mensaje
claro:

```fitz
[1, 2].volar()
// Error — el tipo `List` no tiene un método llamado `volar`
```

Desde la Fase 5.3.4, **`fitz check` también valida los métodos
built-in estáticamente**: tipos de argumentos, aridad, tipo del
receptor del callback en `map`/`filter`/`find`, y typos sobre
métodos inexistentes (`xs.lenght()`) los detectás sin tener que
ejecutar el programa.

### Métodos de `List`

| Método             | Qué hace                                            |
|--------------------|-----------------------------------------------------|
| `push(v)`          | Agrega `v` al final. **Muta** la lista.             |
| `pop()`            | Saca y devuelve el último. **Muta** la lista.       |
| `map(fn)`          | Aplica `fn` a cada elemento y devuelve una lista nueva. |
| `filter(fn)`       | Devuelve una lista nueva con los elementos para los que `fn` da `true`. |
| `find(fn)`         | Devuelve `Ok(elemento)` para el primero que matchea, o `Err("no encontrado")`. |
| `len()`            | Cantidad de elementos.                              |

`fn` es cualquier función unaria. La forma más cómoda es la fn
anónima inline (cap. 11):

```fitz
let xs = [1, 2, 3, 4]

let doblados = xs.map(fn(n) => n * 2)
print(doblados)                    // [2, 4, 6, 8]

let pares = xs.filter(fn(n) => n == 2 or n == 4)
print(pares)                       // [2, 4]

let tres = xs.find(fn(n) => n == 3)
print(tres)                        // Ok(3)

let veinte = xs.find(fn(n) => n == 20)
print(veinte)                      // Err("no encontrado")
```

`push` y `pop` **mutan** la lista; los demás devuelven datos nuevos
y dejan el receptor intacto.

```fitz
let xs = [1, 2]
xs.push(3)
xs.push(4)
print(xs)                          // [1, 2, 3, 4]
let last = xs.pop()
print(last)                        // 4
print(xs)                          // [1, 2, 3]
```

### Métodos de `Map`

| Método      | Qué hace                                                |
|-------------|---------------------------------------------------------|
| `get(k)`    | Devuelve `Ok(valor)` si la clave existe, o `Err(...)`.  |
| `has(k)`    | `true` si la clave existe, `false` si no.               |
| `keys()`    | Lista con las claves, en orden de inserción.            |
| `values()`  | Lista con los valores, en orden de inserción.           |
| `len()`     | Cantidad de pares.                                      |

```fitz
let m = {"a": 1, "b": 2}

print(m.has("a"))                  // true
print(m.has("x"))                  // false
print(m.get("a"))                  // Ok(1)
print(m.get("x"))                  // Err("clave no encontrada: x")
print(m.keys())                    // ["a", "b"]
print(m.values())                  // [1, 2]
```

La diferencia entre `m["a"]` y `m.get("a")` está en cómo modelan la
falta: `m["a"]` corta con error si no hay clave, `m.get("a")` te
devuelve un `Result` y vos decidís qué hacer. Si querés evitar el
corte, usá `get` (y matcheá el `Result`, cap. 14).

### Métodos de `Str`

| Método      | Qué hace                                |
|-------------|-----------------------------------------|
| `len()`     | Cantidad de caracteres (no bytes).      |
| `upper()`   | Devuelve una copia en mayúsculas.       |
| `lower()`   | Devuelve una copia en minúsculas.       |

```fitz
print("hola".len())                // 4
print("hola".upper())              // HOLA
print("HOLA".lower())              // hola
```

Las strings son inmutables: `upper`/`lower` devuelven una nueva,
sin tocar la original.

### Encadenar métodos

Como cada método devuelve un valor, podés enganchar el próximo
sobre el resultado. Esto es donde el estilo "objeto primero" se
empieza a sentir natural:

```fitz
let pares_al_cuadrado = [1, 2, 3, 4, 5]
    .filter(fn(n) => n == 2 or n == 4)
    .map(fn(n) => n * n)
print(pares_al_cuadrado)           // [4, 16]
```

El parser tolera el salto de línea antes de cada `.`, así que un
chain largo se puede partir en varias líneas — la forma idiomática
cuando el callback de cada paso ocupa lugar:

```fitz
let activos = usuarios
    .filter(fn(u) => u.activo)
    .map(fn(u) => u.nombre)
```

Es exactamente equivalente a `usuarios.filter(...).map(...)` en
una sola línea — el AST resultante es idéntico.

### Mutación de campos

Hasta ahora `user.name` era solo lectura. En este capítulo se
desbloquea la escritura: `user.name = "Otro"` reemplaza el valor
del campo en la instancia.

```fitz
type User { id: Int, name: Str }

let u = User { id: 1, name: "Fitz" }
print(u)                            // User { id: 1, name: "Fitz" }

u.name = "Roy"
print(u)                            // User { id: 1, name: "Roy" }
```

El compilador (estático) eventualmente va a permitir marcar campos
como `let`/inmutables y forzar el chequeo. Por ahora cualquier
campo es escribible.

Si intentás asignar a un campo que no existe, error claro:

```fitz
u.nope = 99
// Error — el tipo `User` no tiene un campo llamado `nope`
```

### Alias y referencias compartidas

Acá entra una decisión de diseño que te va a parecer familiar si
venís de Python o JavaScript: las listas, mapas e instancias se
pasan **por referencia compartida**. Eso quiere decir que cuando
dos variables apuntan a la misma lista, mutar por una se ve por la
otra.

```fitz
let a = [1, 2]
let b = a                          // `b` mira la misma lista que `a`.
a.push(3)
print(b)                           // [1, 2, 3]   ← se ve la mutación.
```

Lo mismo pasa con instancias:

```fitz
let original = User { id: 1, name: "Fitz" }
let alias = original
alias.name = "Otro"
print(original.name)               // Otro
```

Esto es el mismo modelo que objetos en Python/JS: las primitivas
(Int, Float, Bool, Str, Null) se copian por valor; las
**colecciones e instancias** se aliasean. Si querés una copia
genuina, hoy hay que reconstruir a mano (`xs.map(fn(x) => x)` para
listas, por ejemplo). El día que necesitemos un `clone()` formal lo
sumamos.

### Funciones anónimas como callback

Los métodos `map`/`filter`/`find` reciben una función. Podés pasar
una fn con nombre, pero lo típico es definir la callback al vuelo
con `fn(x) => ...` (cap. 11):

```fitz
let usuarios = [
    User { id: 1, name: "Fitz" },
    User { id: 2, name: "Roy" },
]

// `find` en una lista de instancias.
let resultado = usuarios.find(fn(u) => u.id == 2)
print(resultado)                   // Ok(User { id: 2, name: "Roy" })
```

Como una anónima es un closure, también ve las variables del scope
donde fue definida:

```fitz
let umbral = 10
let grandes = [1, 5, 12, 20].filter(fn(n) => n > umbral)
print(grandes)                     // [12, 20]
```

### Métodos custom sobre `type`

Desde R.3 (mini-fase R) podés declarar métodos adentro del bloque
`type`. Sintaxis: `fn nombre(params) -> Tipo { body }`, separado
de los fields por newline o coma:

```fitz
type Counter {
    count: Int = 0
    step: Int = 1

    fn current() -> Int {
        return count
    }

    fn next_value() -> Int {
        return count + step
    }

    fn label(prefix: Str) -> Str {
        return "{prefix}: {count}"
    }
}

let c = Counter { count: 10, step: 5 }
print(c.current())          // 10
print(c.next_value())       // 15
print(c.label("c"))         // c: 10
```

**Decisión clave (opción A)**: los **fields del type son variables
locales** en el body del método (sin prefijo `self.`). Es la
convención de Python/Ruby/Crystal — menos boilerplate que `self.x`
de Rust, y consistente con cómo Fitz expone fields adentro de
struct literals (los defaults pueden referenciar fields previos).

**Caveat de shadowing**: si un parámetro tiene el mismo nombre
que un field, el parámetro gana. Workaround: nombrá distinto el
local. Igual que en Rust con bindings:

```fitz
type Renamer {
    name: Str

    fn pick(name: Str) -> Str {
        return name              // ← el PARÁM, no el field
    }
}
```

**Method chaining** funciona naturalmente cuando un método
devuelve otra instancia:

```fitz
type Point {
    x: Int
    y: Int

    fn doubled_p() -> Point {
        return Point { x: x * 2, y: y * 2 }
    }

    fn show() -> Str {
        return "({x}, {y})"
    }
}

let p = Point { x: 3, y: 4 }
print(p.doubled_p().show())     // (6, 8)
```

**`async fn` adentro de `type`** funciona en `fitz run` y
`fitz build` (cerrado post-R.3, 2026-05-17). El receiver se
pasa por valor (clone) al método async para que el Future no
holdee el lock del Mutex; los Arc<Mutex> internos (listas /
maps / instancias anidadas) siguen siendo refs compartidas
como esperás.

```fitz
type Task {
    id: Int

    async fn label(prefix: Str) -> Str {
        sleep(10).await
        return "{prefix}-{id}"
    }
}

let t = Task { id: 7 }
let l = t.label("step").await    // step-7
```

**Limitaciones del MVP** (R.3, mini-fase R):
- Todos los métodos son públicos (sin `pub fn` / `fn` privado).
- Sin static methods (`Counter::create(...)`).
- Sin operator overloading (`fn +(self, other)`).

Ver [examples/guide/13b-metodos-custom.fitz](../examples/guide/13b-metodos-custom.fitz)
para el ejemplo completo (incluye la sección async).

### Métodos chicos de Str y List (mini-tanda S)

Resumen de los métodos cerrados en la mini-tanda S (post-R):

**Sobre `Str`** (S.1 + S.2):

| Método           | Args         | Retorna     | Notas |
|------------------|--------------|-------------|-------|
| `.contains(s)`   | `Str`        | `Bool`      | empty string siempre matchea |
| `.starts_with(s)`| `Str`        | `Bool`      | case-sensitive |
| `.ends_with(s)`  | `Str`        | `Bool`      | case-sensitive |
| `.split(sep)`    | `Str`        | `List<Str>` | empty separator → chars individuales |
| `.trim()`        | —            | `Str`       | whitespace ambos lados |
| `.replace(o, n)` | `Str`, `Str` | `Str`       | TODAS las ocurrencias |
| `.repeat(n)`     | `Int`        | `Str`       | `n < 0` es error |

**Sobre `List<T>`** (S.3):

| Método          | Args | Retorna | Notas |
|-----------------|------|---------|-------|
| `.sort()`       | —    | `Null`  | IN-PLACE, T ∈ {Int, Float, Str, Bool} |
| `.reverse()`    | —    | `Null`  | IN-PLACE, cualquier T |
| `.contains(v)`  | `T`  | `Bool`  | igualdad estructural |

Ver [examples/guide/13c-metodos-extras.fitz](../examples/guide/13c-metodos-extras.fitz)
para el ejemplo completo.

### Iteradores: `enumerate` / `zip` / `chain` (mini-tanda It)

Tres métodos canónicos para componer listas sin loops manuales,
inspirados en Python/Rust. Todos devuelven una **lista nueva**
(no mutan el receptor).

| Método              | Args         | Retorna           | Notas |
|---------------------|--------------|-------------------|-------|
| `.enumerate()`      | —            | `List<(Int, T)>`  | Pares (índice, elemento). |
| `.zip(ys)`          | `List<U>`    | `List<(T, U)>`    | Empareja dos listas; trunca al más corto. |
| `.chain(ys)`        | `List<T>`    | `List<T>`         | Concatena dos listas del mismo tipo. |

El caso canónico de `enumerate` combina con el tuple destructuring
del `for` (mini-tanda Md):

```fitz
let nombres: List<Str> = ["ada", "bea", "cam"]
for (i, n) in nombres.enumerate() {
    print("{i}: {n}")
}
// 0: ada / 1: bea / 2: cam
```

`zip` permite recorrer dos listas en paralelo. Si los tamaños
difieren, trunca al menor (paralelo a Python):

```fitz
let valores: List<Int> = [10, 20, 30]
let pesos: List<Int> = [1, 2]
let pares: List<(Int, Int)> = valores.zip(pesos)
print(pares.len())               // 2 (no 3 — `pesos` tiene 2 items)
```

`chain` concatena (sin mutar):

```fitz
let primeras: List<Int> = [1, 2, 3]
let segundas: List<Int> = [4, 5]
let todo: List<Int> = primeras.chain(segundas)
print(todo.len())                // 5
```

Ver [examples/guide/13d-iteradores.fitz](../examples/guide/13d-iteradores.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

### Lo que todavía no anda

- **`return` adentro de un brazo de `match` como expresión** —
  como cada brazo es una expresión, no podés cortar la función
  desde adentro con `return`. Se puede pulir cuando moleste.
- **`xs.sort_by(fn)`** — sort con comparator custom. Si aparece
  demanda, sub-paso futuro.
- **`xs.flatten()`** — `List<List<T>>` → `List<T>`. Sin demanda
  concreta.
- **Más métodos**: `.find()` para strings, etc. Se irán sumando
  con la práctica.

> Lo que **sí anda** y antes era deuda: encadenamiento multi-línea
> (cerrado en PreF8.2), **asignación a índice** `xs[0] = v` y
> `m["k"] = v` (R.1.3 mini-fase R, ver
> [cap 9 sub-sección "Asignación a índice"](#9-listas-mapas-y-rangos)),
> **métodos custom sobre `type`** (R.3 mini-fase R, ver
> sub-sección de arriba), **métodos chicos de Str y List**
> (mini-tanda S — `.contains`/`.starts_with`/`.ends_with`/
> `.split`/`.trim`/`.replace`/`.repeat` sobre Str;
> `.sort`/`.reverse`/`.contains` sobre List), **iteradores
> `.enumerate()`/`.zip()`/`.chain()`** (mini-tanda It — ver
> sub-sección de arriba).
> Forma idiomática del chain multi-línea:
> ```fitz
> let nombres = users
>     .filter(fn(u) => u.active)
>     .map(fn(u) => u.name)
> ```

### Ejemplo completo

[examples/guide/13-metodos.fitz](../examples/guide/13-metodos.fitz):

```fitz
type User { id: Int, name: Str }

let usuarios = [
    User { id: 1, name: "Fitz" },
    User { id: 2, name: "Roy" },
]

usuarios.push(User { id: 3, name: "Cerro" })
print(usuarios.len())

let nombres = usuarios.map(fn(u) => u.name)
print(nombres)

let con_o = usuarios.filter(fn(u) => u.name.lower() == "roy")
print(con_o)

let buscado = usuarios.find(fn(u) => u.id == 1)
print(buscado)

let no_encontrado = usuarios.find(fn(u) => u.id == 99)
print(no_encontrado)

let primero = usuarios.find(fn(u) => u.id == 1)
match primero {
    Ok(u)  => print("hola, {u.name}!")
    Err(e) => print("no debería pasar: {e}")
}

let primer = usuarios[0]
primer.name = "Patagonia"
print(usuarios)

let m = {"a": 1, "b": 2, "c": 3}
print(m.has("a"))
print(m.get("z"))
print(m.keys())
print(m.values())
print(m.len())

print("Hola".upper())
print("MUNDO".lower())
print("hola".len())
```

Salida:

```
3
["Fitz", "Roy", "Cerro"]
[User { id: 2, name: "Roy" }]
Ok(User { id: 1, name: "Fitz" })
Err("no encontrado")
hola, Fitz!
[User { id: 1, name: "Patagonia" }, User { id: 2, name: "Roy" }, User { id: 3, name: "Cerro" }]
true
Err("clave no encontrada: z")
["a", "b", "c"]
[1, 2, 3]
3
HOLA
mundo
4
```

---

Con métodos y mutación ya tenés todo lo que hace falta para escribir
programas que cambian de estado y usan datos en colecciones de
manera ergonómica. En el próximo capítulo entra Fitz a manejar
errores **del programa** sin excepciones: el tipo `Result`, los
constructores `Ok` y `Err`, y el operador `?` para propagar.

---

## 14. Result y manejo de errores

Fitz no tiene excepciones. Cuando una operación puede fallar, su
resultado se modela explícitamente con el tipo built-in `Result`,
que tiene dos variantes: `Ok(valor)` para éxito y `Err(error)` para
falla. El caller siempre ve "esto puede fallar" en el tipo, y decide
qué hacer con la falla.

Es el mismo modelo que Rust. En Python y JavaScript, los errores
viajan "por el costado" vía excepciones; en Fitz, los errores son
valores comunes que viajan por el mismo camino que el resto.

### Construir un `Result`

`Ok(v)` y `Err(e)` son constructores: envolvés un valor cualquiera
en la variante correspondiente.

```fitz
fn divide(a, b) {
    if (b == 0) {
        return Err("división por cero")
    }
    return Ok(a / b)
}

print(divide(10, 2))   // Ok(5)
print(divide(10, 0))   // Err("división por cero")
```

Por convención, el inner de un `Err` suele ser un `Str` con el
mensaje, pero el lenguaje no lo obliga: podés meter ahí cualquier
valor (un código, una instancia de un tipo de error custom, lo que
quieras). Lo mismo aplica al inner de `Ok`.

### Consumir un `Result` con `match`

`match` sobre un `Result` usa los patrones `Ok(v)` y `Err(e)`. Cada
uno matchea su variante y bindea el inner al nombre que pongas.

```fitz
match divide(10, 2) {
    Ok(v)  => print("resultado: {v}")
    Err(e) => print("falló: {e}")
}
// resultado: 5
```

Reglas a tener en cuenta:

- `Ok(v)` solo matchea si el valor es `Value::Result` de variante
  exitosa. No matchea contra un Int o un Str pelados.
- Mismo criterio con `Err(e)`.
- El binding (`v`, `e`) vive solo dentro del cuerpo del brazo, como
  cualquier binding de `match` (cap. 10).
- Si querés ignorar el inner, hoy igual hay que poner un nombre
  (`Ok(_)` también funciona — `_` queda como una variable basura en
  el scope del arm). El soporte para `_` real adentro de Ok/Err es
  deuda menor; cuando moleste lo agregamos.

### El operador `?` — propagar el `Err`

Escribir `match` cada vez que llamás algo que puede fallar es
verboso. Cuando lo único que querés es "si falla, devolver el mismo
error", el operador `?` lo hace por vos:

```fitz
fn find_user(id) {
    if (id == 1) {
        return Ok(User { id: 1, name: "Fitz" })
    }
    return Err("usuario no encontrado")
}

fn describe_user(id) {
    let u = find_user(id)?     // si find_user devuelve Err, describe_user
                               // corta y devuelve ese mismo Err.
                               // Si devuelve Ok(u), `u` queda con el User.
    return Ok("#{u.id} es {u.name}")
}

print(describe_user(1))    // Ok("#1 es Fitz")
print(describe_user(42))   // Err("usuario no encontrado")
```

Mentalmente, `expr?` se lee así:

```fitz
match expr {
    Ok(v)  => v                  // desempaqueta y seguí
    Err(e) => return Err(e)      // propagá inmediatamente
}
```

Pero más corto y encadenable: `find_user(id)?.name` desempaqueta
primero y después accede al campo.

### Result en una variable o `print`

Como cualquier otro valor, un `Result` se puede asignar, comparar,
imprimir, pasar a otra función. El `print` muestra el formato
canónico:

```fitz
let r = Ok(42)
let e = Err("boom")
print(r)            // Ok(42)
print(e)            // Err("boom")
```

La igualdad es estructural: dos `Ok(1)` son iguales, dos `Err("x")`
también, y `Ok(1) == Err(1)` da `false`.

### `Result<T, E>` con E tipado (mini-tanda Re+)

Anotar el tipo del Err explícitamente con `Result<T, E>` habilita
errores estructurados accesibles end-to-end:

```fitz
type ApiError { status: Int, msg: Str }

fn fetch(url: Str) -> Result<Int, ApiError> {
    if url == "/health" {
        return Ok(200)
    }
    return Err(ApiError { status: 503, msg: "service unavailable" })
}

// El `e` del Err tipa ApiError, NO Str — fields accesibles.
match fetch("/users") {
    Ok(code) => print("status: {code}"),
    Err(e) => print("err {e.status}: {e.msg}")
}
// err 503: service unavailable
```

Funciona bit-a-bit en `fitz run` y `fitz build`.

Sintaxis:
- `Result<T>` (1 arg) — default `E = Str`, compat con código existente.
- `Result<T, E>` (2 args) — E concreto: `Result<Int, ApiError>`,
  `Result<User, Int>` (códigos de error), etc.

Si omitís la anotación del E en una fn que devuelve `Result<T>` y
hacés `Err(MiError {...})`, el checker bindea el `e` como `Str` por
default. Para que el binding `Err(e)` tipa como tu tipo custom,
anotá explícitamente el E.

Ver [examples/guide/14c-result-tipado.fitz](../examples/guide/14c-result-tipado.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

### Err con tipos custom (mini-tanda Err+)

El `Err` acepta cualquier value, no solo `Str`. En `fitz run`
preserva el tipo exacto al desempacar:

```fitz
type ApiError { status: Int, msg: Str }

fn fetch(url: Str) -> Result<Int> {
    if url == "/health" {
        return Ok(200)
    }
    return Err(ApiError { status: 503, msg: "service unavailable" })
}

match fetch("/users") {
    Ok(c) => print("status: {c}"),
    Err(e) => print("err: {e}")
}
// err: ApiError { status: 503, msg: "service unavailable" }
```

En `fitz build`, el `Err` se coerce a `String` via Display
(el codegen sigue con `Result<T, String>` pinned). El value se
imprime igual, pero **acceder a fields del Err** (`Err(e) => e.status`)
solo funciona en `fitz run`; en `fitz build` el `e` tipa `Str` y
da error de "field access sobre Str". Workaround portable: imprimir
el Err completo (vía Display) o usar `Err(Int)` con códigos
numéricos.

### `?` fuera de fn — mensaje propio

Cuando un `?` en top-level (o adentro de una fn sin `-> Result<T>`)
recibe un `Err`, el programa aborta con un mensaje específico
mostrando el contenido del Err:

```text
Error — operación `?` falló con Err: ApiError { status: 503, msg: "..." }
```

Antes de Err+ daba `` `return` solo puede usarse adentro de una
función `` — frío y engañoso (el `?` reusaba el mecanismo de
`return` internamente). Ahora el usuario ve **qué** falló.

Ver [examples/guide/14b-errores-tipados.fitz](../examples/guide/14b-errores-tipados.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

### Lo que todavía no anda

- **Chequeo estático de `?`** — desde Fase 5.3.3, `fitz check`
  exige que el operando de `?` sea `Result<T>` y que la función
  contenedora declare `-> Result<...>` (a menos que la función
  esté sin anotación de retorno, donde queda en modo gradual).
- **`Err` con bindings tipados en codegen** — el binding `e` del
  pattern `Err(e)` siempre tipa `Str` en el código compilado,
  porque el Err side sigue pinned a `Result<T, String>`. En el
  intérprete conserva el tipo original del inner. Refactorear
  `Type::Result` para llevar también el tipo del Err es deuda
  residual — toca 10+ sitios del checker.

> Lo que **sí anda** y antes era deuda (mini-tanda Err+):
> **`Err(<no-Str>)`** preserva el tipo en `fitz run` (Int, Instance,
> Tuple, etc. al desempacar con match); **`?` en top-level**
> aborta con mensaje específico mostrando el contenido del Err en
> lugar del genérico "return fuera de función".

### Ejemplo completo

[examples/guide/14-result.fitz](../examples/guide/14-result.fitz):

```fitz
type User { id: Int, name: Str }

fn divide(a: Int, b: Int) -> Result<Int> {
    if (b == 0) {
        return Err("división por cero")
    }
    return Ok(a / b)
}

match divide(10, 2) {
    Ok(v) => print("ok: {v}")
    Err(e) => print("err: {e}")
}

match divide(10, 0) {
    Ok(v) => print("ok: {v}")
    Err(e) => print("err: {e}")
}

fn find_user(id: Int) -> Result<User> {
    if (id == 1) {
        return Ok(User { id: 1, name: "Fitz" })
    }
    return Err("usuario no encontrado")
}

fn describe_user(id: Int) -> Result<Str> {
    let u = find_user(id)?
    return Ok("#{u.id} es {u.name}")
}

match describe_user(1) {
    Ok(desc) => print(desc)
    Err(e) => print("falló: {e}")
}

match describe_user(42) {
    Ok(desc) => print(desc)
    Err(e) => print("falló: {e}")
}
```

Salida:

```
ok: 5
err: división por cero
#1 es Fitz
falló: usuario no encontrado
```

---

En el próximo capítulo damos un paseo corto por los errores **del
intérprete**: cómo leer un mensaje, qué significa cada uno, y las
limitaciones de precisión que todavía tiene. Ojo a la distinción:
acá hablamos de errores que maneja tu programa Fitz; allá, de
errores que te cuenta el intérprete cuando tu programa está mal
escrito.

---

## 15. Errores y mensajes

Tarde o temprano vas a tipear algo mal y el intérprete te va a cortar.
Este capítulo es un mapa de los errores **del intérprete**: los que
aparecen cuando tu programa Fitz está mal escrito o intenta algo
inválido en runtime. No los confundas con los errores **del
programa** — los `Err(...)` que devuelve una función — que cubrimos
en el [cap. 14](#14-result-y-manejo-de-errores).

### Formato general

Un error de Fitz tiene esta forma:

```
Error en línea L:C — descripción del problema
  Sugerencia: (opcional, si hay una)
```

`L:C` es la línea y la columna donde se detectó el problema. El
intérprete corta la ejecución apenas encuentra el primer error, así
que vas a ver uno por corrida.

### De qué fase vino el error

Fitz procesa tu programa en cuatro etapas, y cada una puede tirar
errores con distinto sabor:

1. **Lexer** — separa el texto en tokens. Si una comilla no cierra o
   aparece un carácter raro, falla acá.
2. **Parser** — arma el árbol de sintaxis. Si la gramática no
   coincide (`if` sin `{`, `match` sin `=>`, expresión incompleta),
   falla acá.
3. **Checker estático** (Fase 5) — recorre el árbol validando las
   anotaciones de tipo y las expresiones. Si declaraste `x: Int` y
   le asignás `"hola"`, si llamás `add(5)` cuando `add` espera dos
   argumentos, o si el callback de `.filter(...)` no devuelve `Bool`,
   falla acá.
4. **Evaluador** — ejecuta el árbol. Si tu programa pasó las tres
   etapas anteriores pero hace algo inválido en runtime (dividir por
   cero, indexar fuera de rango, matchear sin brazo que coincida),
   falla acá.

Hoy el lexer y el parser dan **posiciones precisas**. El checker y
el evaluador, en cambio, suelen reportar `0:0` — es deuda explícita:
nos faltan ubicaciones para las subexpresiones. La descripción del
error sí es buena, así que usamos eso para orientarnos hasta que se
mejore.

### Modo strict y `--no-typecheck`

Desde la Fase 5.4, `fitz run` aborta cuando el checker estático
encuentra errores. Eso quiere decir que un programa con errores de
tipo **no llega a ejecutarse**:

```
✗ archivo.fitz — 1 error(es) de tipo:
  Error — `x` declarado como `Int` recibió un valor `Str`
   Usá `fitz check` para revisar, o `fitz run --no-typecheck archivo.fitz` para correr igual.
```

Si querés saltarte el chequeo (por ejemplo, para probar una rama
mientras todavía hay errores en otra parte del programa, o para
diagnosticar un bug del propio checker), agregá `--no-typecheck`:

```bash
fitz run --no-typecheck archivo.fitz
```

En ese modo los errores del checker se reportan como warnings y el
programa se ejecuta igual.

### Errores típicos del lexer

| Mensaje | Qué pasó | Cómo arreglar |
|---------|----------|---------------|
| `String sin cerrar — salto de línea antes de la comilla de cierre` | Abriste `"` y llegaste al final de la línea sin cerrar. | Cerrá las comillas o usá `\n` (ver cap. 5). |

Ejemplo:

```fitz
x = "sin cerrar
```

```
Error en línea 1:5 — String sin cerrar — salto de línea antes de la comilla de cierre
  Sugerencia: Usá \n para incluir un salto de línea dentro del string
```

### Errores típicos del parser

| Mensaje | Qué pasó |
|---------|----------|
| `Se esperaba una expresión, se encontró 'X'` | Faltó la expresión donde el parser la esperaba (después de `+`, después de `=`, dentro de paréntesis). |
| `se esperaba ')' para cerrar la llamada` | Una llamada quedó sin cerrar paréntesis. |
| `se esperaba '=>' después del patrón` | Brazo de `match` mal formado, típicamente por un patrón no soportado (cap. 10). |
| `los struct literals no se permiten directamente en condiciones de if/while/for/match` | Un `User { id: 1 }` adentro de la condición de un `if`/`while`/`for`/`match`. Envolvelo en paréntesis (cap. 12). |
| `se esperaba ',', salto de línea o '}' entre campos del struct literal` | Faltó el separador entre dos campos de una instancia (cap. 12). |
| `índice fuera de rango: N en lista de tamaño M` | `xs[i]` con `i` por fuera de la lista (cap. 9). |
| `clave no encontrada en mapa: k` | `m[k]` con clave que no existe (cap. 9). |
| `el tipo 'X' no soporta indexing con '[]'` | Intentaste `[i]` sobre algo que no es lista ni mapa (cap. 9). |

Ejemplo:

```fitz
x = 1 +
```

```
Error en línea 1:8 — Se esperaba una expresión, se encontró 'Newline'
```

### Errores típicos del checker estático

Estos los detectás con `fitz check` o aparecen al correr `fitz run`
(que aborta en modo strict desde Fase 5.4):

| Mensaje | Cuándo aparece |
|---------|----------------|
| `variable desconocida \`x\`` | Usaste un nombre que no fue declarado. |
| `` `x` declarado como `T` recibió un valor `U` `` | Mismatch entre la anotación y el valor. |
| `la función \`f\` espera N argumento(s), recibió M` | Aridad incorrecta en la llamada. |
| `` `return` devuelve `T` pero la función declara `U` `` | El cuerpo de la fn devuelve algo distinto a lo declarado. |
| `el operador \`?\` requiere un \`Result\`, recibió \`X\`` | Usaste `?` sobre algo que no es Result. |
| `match sobre \`Result\` no es exhaustivo: falta el caso \`X\`` | Match sin cubrir Ok o Err (y sin wildcard). |
| `el tipo \`X\` no tiene el método \`Y\`` | Typo de método (`xs.lenght()`) o método inexistente. |
| `el tipo \`X\` no soporta indexing con \`[]\`` | `obj[i]` sobre algo que no es lista/mapa. |

Ejemplo:

```fitz
let x: Int = "hola"
```

```
✗ archivo.fitz — 1 error(es) de tipo:
  Error — `x` declarado como `Int` recibió un valor `Str`
```

### Errores típicos del evaluador

Estos son los que más vas a ver mientras escribís lógica — pasaron
el checker porque el sistema de tipos no analiza valores:

| Mensaje | Cuándo aparece |
|---------|----------------|
| `variable 'x' no definida` | Usaste un identificador antes de asignarle nada. También aparece en interpolaciones (cap. 5). |
| `operación '+' no soportada entre 'Str' y 'Int'` | Concatenaste tipos distintos sin coerción (cap. 5). Lo mismo para `-`, `*`, `/`. |
| `división por cero` | Dividiste por `0` (Int) o `0.0` (Float). Cap. 4. |
| `la condición de 'if' debe ser Bool, no 'Int'` | Pasaste un valor no-Bool a la condición. Lo mismo aplica a `while`. Cap. 6. |
| `operando izquierdo de 'and' debe ser Bool, no 'X'` | Igual, en `and` / `or`. Cap. 6. |
| `'add' espera 2 argumento(s), recibió 1` | Aridad incorrecta al llamar (cap. 11). |
| `'n' no es invocable (es Int)` | Intentaste llamar como función algo que no lo es. |
| `'break' solo puede usarse adentro de un loop` | `break` / `continue` fuera de un loop. Cap. 8. |
| `'return' solo puede usarse adentro de una función` | `return` en el nivel global. Cap. 11. |
| `el 'match' no matcheó ningún brazo` | El `match` no tenía wildcard y ningún patrón coincidió. Cap. 10. |
| `no se puede iterar sobre un valor de tipo 'X'` | `for x in v` con `v` que no es List ni Range (cap. 9). |
| `el tipo 'X' no tiene un campo llamado 'Y'` | Acceso a un campo que no existe, o instanciación con un campo no declarado. Cap. 12. |
| `falta el campo 'Y' al instanciar 'X' (no tiene default y no es nullable)` | Omitiste un campo obligatorio en un struct literal. Cap. 12. |
| `tipo 'X' no definido` | Instanciaste un tipo que no fue declarado con `type` (o lo escribiste mal). Cap. 12. |
| `acceso a campo '.X' sobre un valor de tipo 'Y'` | Hiciste `obj.campo` sobre algo que no es una instancia (Int, Str, List, etc.). Cap. 12. |
| `'Ok' espera exactamente 1 argumento, recibió N` | Constructor `Ok` / `Err` con aridad incorrecta (cap. 14). |
| `` el operador `?` requiere un valor `Result`, recibió 'X' `` | Usaste `?` sobre algo que no es un `Result` (cap. 14). |

Ejemplo (el archivo de este capítulo):

```fitz
let x = 10
let y = 0
print(x / y)
```

```
Error en línea 0:0 — división por cero
```

### Cuando el mensaje no alcanza

Si el error no te da pistas claras, recordá que **el binario hoy
imprime los tokens y el AST antes de la ejecución**:

```
--- Tokens ---
   1:1    Ident("print")
   1:6    LParen
   ...

--- AST ---
  [0] Expr(
    Call { ... }
  )

--- Ejecución ---
Error en línea 0:0 — ...
```

Ese dump te dice exactamente cómo el lexer leyó tu código y cómo el
parser lo armó. Para errores de runtime con posición `0:0`, ese
contexto suele ser lo que más sirve para ubicar el problema mientras
no tengamos posiciones finas.

### Lo que viene

- **Posiciones de subexpresiones** en errores de runtime, para que
  desaparezca el `0:0`.
- **Error recovery** en el parser: hoy el primer error corta el
  parseo. Más adelante el parser va a poder seguir y reportar varios
  errores de una sola corrida.
- **Cuadro de contexto** debajo del mensaje, con la línea del código
  fuente subrayando el problema (estilo Rust / Elm).

### Ejemplo completo

[examples/guide/15-errores.fitz](../examples/guide/15-errores.fitz):

```fitz
let x = 10
let y = 0
print(x / y)
```

`fitz check` lo deja pasar: el sistema de tipos no analiza valores
(no sabe que `y` siempre vale `0` en este punto). El error aparece
al correr (`fitz run`):

```
--- Ejecución ---
Error en línea 0:0 — división por cero
```

Si querés ver cómo se ven los errores del checker, probá agregar
`let x: Int = "hola"` en un archivo y correrlo: `fitz run` aborta
en strict mode antes de ejecutar. Sumando `--no-typecheck` los
errores pasan a warnings y el programa sigue corriendo.

---

Con el mapa de errores del intérprete a mano, podemos pasar a un
tema distinto: cómo partir el código en archivos. Hasta ahora todos
los ejemplos vivieron en un solo `.fitz`. En el próximo capítulo
vemos cómo separar pedazos y traerlos con `import`.

---

## 16. Módulos

Hasta acá, todos los programas vivieron en un archivo. Cuando los
proyectos crecen, querés partir el código: una pieza por archivo,
con sus tipos, funciones y constantes, y traerlos donde haga falta.
Fitz tiene dos formas de hacerlo: `import foo` para usar el archivo
como namespace, y `from foo import a, b` para traer nombres directo
al scope.

### Tu primer módulo

Pongamos dos archivos lado a lado:

`utils.fitz`:

```fitz
let PREFIX = "saludos, "

fn greet(name: Str) -> Str {
    return "{PREFIX}{name}"
}
```

`main.fitz`:

```fitz
import utils

let g = utils.greet("Fitz")
print(g)
```

Salida:

```
saludos, Fitz
```

Lo que pasa:

1. Al ver `import utils`, el intérprete busca un archivo
   `utils.fitz` **relativo a `main.fitz`** y lo evalúa entero, en
   un scope aislado.
2. `utils` queda bindeado en el scope de `main.fitz` como un
   **módulo**: un valor que responde a `utils.<nombre>` con lo que
   el módulo tenga top-level.
3. `utils.greet("Fitz")` busca `greet` adentro del env del módulo,
   lo invoca, y devuelve el `Str` interpolado. La closure de
   `greet` ve `PREFIX` porque está en el mismo env.

### `from ... import` — traer nombres directos

Si no querés escribir `utils.` cada vez, podés pedir nombres
específicos:

`main.fitz`:

```fitz
from utils import greet, PREFIX

print(greet("Fitz"))     // saludos, Fitz
print(PREFIX)            // saludos,
```

Diferencias con `import utils`:

- `from import` **no expone** el módulo como tal — solo bindea los
  nombres pedidos.
- El módulo igual se carga entero (eager): si tiene side effects
  top-level, pasan al evaluar el `from`.
- Si el módulo no exporta uno de los nombres, error explícito al
  evaluar el `from`.

Trailing comma admitida: `from utils import greet, PREFIX,` (la
coma final se ignora). La forma multi-línea con paréntesis
(`from utils import (\ngreet,\nPREFIX\n)`) todavía no se soporta;
si la lista se hace larga, mantenela en una línea.

### Alias con `as`

Tanto `import` como `from import` aceptan `as <ident>` para
renombrar el binding local. Útil cuando:

- el nombre original es largo o choca con un símbolo del archivo
  actual (`from foo import PREFIX as REMOTE` mientras tenés tu
  propia `let PREFIX = ...`);
- querés usar un alias corto para un namespace
  (`import muy_largo_paquete as p`);
- el código se lee mejor con el alias (`from db import Connection
  as Conn`).

```fitz
import utils as u
from utils import greet as saludar, PREFIX as REMOTE

print(saludar("Fitz"))   // saludos, Fitz
print(REMOTE)            // saludos,
print(u.PREFIX)          // saludos,
```

Una entry de `from import` puede tener alias o no — se pueden
mezclar: `from foo import a as x, b, c as z`.

Para un tipo importado con alias, los struct literals se escriben
con el alias (`Person { id: 1 }`), pero el `Display` mantiene el
nombre original del tipo (`User { id: 1 }`) — el alias es local
al archivo importer, no parte de la identidad del tipo. Esto da
paridad bit-a-bit entre `fitz run` y `fitz build`.

### Paths con puntos — subdirectorios

Los segmentos separados por `.` mapean a subdirectorios:

`sub/foo.fitz`:

```fitz
fn one() => 1
```

`main.fitz`:

```fitz
import sub.foo

print(foo.one())     // 1
```

Reglas:

- `import sub.foo` resuelve a `<dir-del-archivo-importer>/sub/foo.fitz`.
- El **binding** es el **último segmento** (`foo`), no
  `sub.foo`. Para acceder al módulo: `foo.one()`. No hay un
  binding `sub` que tenga `foo` adentro.
- `from sub.foo import bar` también funciona: misma resolución
  de path, pero el binding es `bar` directo (sin pasar por
  `foo`).

### Tipos importados y struct literals

Los `type` declarados en un módulo son valores comunes: se
exportan como cualquier otro. Para usarlos con la sintaxis de
struct literal (`User { id: 1, name: "x" }`), hay que traer el
tipo al scope con `from ... import`:

`models.fitz`:

```fitz
type User {
    id: Int
    name: Str
}
```

`main.fitz`:

```fitz
from models import User

let u = User { id: 7, name: "Fitz" }
print(u)             // User { id: 7, name: "Fitz" }
print(u.name)        // Fitz
```

Por qué hace falta el `from import`: el parser de struct literal
espera un `Ident { ... }` simple. `import models` +
`models.User { ... }` no parsea hoy — el parser no sabe que
`models.User` es el "type name". El `from import` te trae `User`
directo y resuelve el problema sin extender el parser. (Asimetría
que se va a cerrar cuando moleste.)

### Aislamiento

Cada módulo tiene su propio env. Las variables, funciones y tipos
top-level del módulo viven ahí; **el scope del importer NO ve esas
definiciones** salvo lo que se traiga vía `import` o `from import`.

Eso significa que dos módulos pueden tener nombres iguales sin
chocar:

`a.fitz`:

```fitz
fn ping() => "desde a"
```

`b.fitz`:

```fitz
fn ping() => "desde b"
```

`main.fitz`:

```fitz
import a
import b

print(a.ping())     // desde a
print(b.ping())     // desde b
```

Y las **closures** de funciones exportadas siguen viendo el env
**del módulo donde se definieron**, no el del importer. Por eso
`greet` en el primer ejemplo encontró `PREFIX` aunque `PREFIX` no
está en el scope de `main.fitz`.

### Cache — el mismo archivo no se ejecuta dos veces

Si dos partes del proyecto importan el mismo archivo, se carga una
sola vez. La segunda vez devuelve el módulo ya cargado, sin
re-evaluar el body:

```fitz
import utils       // primera vez: utils.fitz se evalúa
import utils       // segunda vez: hit en cache, no se re-evalúa
```

Esto es importante si tu módulo tiene side effects top-level (un
`print` o un `let` con cómputo): pasan una sola vez. La identidad
del módulo es la misma — si guardás el resultado de los dos
imports en dos variables, son iguales (`u1 == u2`).

### Ciclos

Un módulo no puede importar (directa o transitivamente) un módulo
que todavía no terminó de cargarse. El intérprete detecta el ciclo
y corta con un error explícito:

`a.fitz`:

```fitz
import b
let from_a = 1
```

`b.fitz`:

```fitz
import a
let from_b = 2
```

`main.fitz`:

```fitz
import a
```

Salida:

```
Error en línea 0:0 — ciclo de imports detectado: ...\a.fitz -> ...\b.fitz -> ...\a.fitz
```

(Los paths van canonicalizados — vas a ver los absolutos.)

No intentamos resolver ciclos automáticamente: son raros en código
bien organizado y agregan complejidad sin payoff. Si te encontraste
con uno, reorganizá: típicamente significa que las dos piezas
querían ser una sola, o que hay un tercer módulo "core" que las dos
tendrían que importar.

### Qué se exporta

Hoy, **todo lo top-level del módulo es público**: variables,
funciones, tipos. No hay marcador `pub` ni convención de
underscore. Si por ahora querés indicar que algo es "interno",
ponele un nombre claro (`_helper`) por convención — pero no hay
chequeo del intérprete.

Cuando aparezca la necesidad real (módulos con superficie pública
pequeña y privada grande), se va a sumar `pub` explícito o
convención de underscore validada por el compilador estático.

### Constantes del módulo con RHS calculada

Desde la mini-tanda **F14**, los `let X = <expr>` top-level del
módulo soportan RHS arbitrarias (no solo literales). El compilador
elige automáticamente entre dos formas:

- **Const-eval** — RHS reducible a un valor Rust constante (literales
  más BinOp/UnaryOp aritmético/lógico/bit sobre operands const-eval):
  se emite como `pub const X: T = <rhs>` en el módulo Rust generado.
  Cero overhead, el valor se inlinea como cualquier constante Rust.

- **Runtime** — RHS no const-eval (call a una fn, struct lit, concat
  de strings, field access, etc.): se emite como accessor function
  `pub fn X() -> T { <rhs> }`. Cada referencia `mod.X` o `X` (tras
  `from mod import X`) se traduce a una llamada `X()` que re-evalúa
  la RHS. Útil para inicializar valores compuestos que no entrarían
  en una `const` Rust.

[examples/guide/16b-modulos-let-expr.fitz](../examples/guide/16b-modulos-let-expr.fitz):

```fitz
import module_let_expr_utils as utils

print(utils.SECONDS_PER_HOUR)  // 3600  — pub const inlineado
print(utils.MAX_USERS)         // 100   — accessor fn (depende de const)
print(utils.DEFAULT_USER)      // accessor fn que devuelve User
print(utils.GREETING)          // accessor fn (Str concat)
```

[examples/guide/module_let_expr_utils.fitz](../examples/guide/module_let_expr_utils.fitz):

```fitz
let SECONDS_PER_HOUR: Int = 60 * 60       // const-eval
let MAX_USERS: Int = SECONDS_PER_HOUR / 36 // accessor (referencia ident)

type User { id: Int = 0, name: Str = "anon" }
fn make_user() -> User => User {}
let DEFAULT_USER: User = make_user()       // accessor (call)
let GREETING: Str = "¡hola, " + "Fitz!"    // accessor (concat Str)
```

Detalle: una RHS const-eval que referencia otra const del mismo
módulo (como `MAX_USERS` arriba) cae al camino accessor por
simplicidad — el codegen no propaga const-ness entre `let`s del
módulo. En la práctica no importa: la diferencia entre `pub const`
y `pub fn X()` es invisible para el código que llama a `mod.X`.

### Qué no se puede hacer todavía

- **`foo.User { ... }`** — el struct literal con namespace no
  parsea. La forma actual es `from foo import User`.
- **`stdlib`** (`from fitz import http`) — el prefijo `fitz/` se
  reserva para Fase 4 cuando entre HTTP nativo. Hoy todo es código
  de usuario.
- **Multi-línea en `from import (...)` con paréntesis** — sin
  soporte. Una línea sola.
- **Compilar (`fitz build`)** — desde 5b.5 el compilador
  soporta módulos. Con dos restricciones que no afectan al
  intérprete:
  - Las funciones del módulo **deben anotar tipos** de parámetros
    y retorno (limitación de codegen 5b.1; la inferencia de
    tipos de params es deuda residual).
  - **Imports transitivos no se soportan**: un módulo cargado
    por el main no puede tener su propio `import`. Workaround
    hasta que se cierre: aplaná los imports al archivo principal.

### Ejemplo completo

[examples/guide/16-modulos.fitz](../examples/guide/16-modulos.fitz):

```fitz
import guide_utils
from guide_utils import User

let u = User { id: 7, name: "Fitz" }
print(guide_utils.greet(u.name))
print(u)
```

[examples/guide/guide_utils.fitz](../examples/guide/guide_utils.fitz):

```fitz
let PREFIX = "saludos, "

fn greet(name: Str) -> Str => "{PREFIX}{name}"

type User {
    id: Int
    name: Str
}
```

Por qué `guide_utils.fitz` y no `16-utils.fitz`: el binding que
produce `import` es el último segmento del path, y tiene que ser
un identificador válido. `16-utils` arranca con dígito y contiene
`-`, así que no se puede usar como nombre de variable en Fitz. El
auxiliar va con nombre limpio.

Salida (los paths del dump de tokens/AST se omiten):

```
saludos, Fitz
User { id: 7, name: "Fitz" }
```

---

Con módulos cubrimos cómo partir el código adentro de un mismo
proyecto. Lo que sigue es **cómo armar proyectos** — el
[package manager](#16b-package-manager) (`fitz.toml`, deps,
lockfile, `fitz new`/`add`/`remove`). Después de eso entramos a
HTTP nativo, que es donde Fitz se diferencia.

---

## 16b. Package manager

Hasta acá los ejemplos vivieron en archivos sueltos: `fitz run
mi_archivo.fitz`. Para proyectos reales (varios archivos,
dependencias compartidas, binarios distribuibles), Fitz tiene un
**package manager built-in** desde la Fase 9.y. Patrón Cargo:
manifest `fitz.toml`, lockfile `fitz.lock`, sub-comandos para
crear/agregar/quitar/actualizar deps.

### El manifest `fitz.toml`

Un proyecto Fitz arranca con un archivo `fitz.toml` en su raíz:

```toml
[package]
name = "miapp"          # ident usable en Fitz (sin hyphens)
version = "0.1.0"
edition = "2026"

[bin]
main = "src/main.fitz"  # entry point del binario

[dependencies]
# (vacío por ahora — agregamos después)
```

Las tres secciones obligatorias son `[package]` (metadata),
**una de** `[bin]` (para programas) o `[lib]` (para librerías
importables), y `[dependencies]` (puede ir vacío).

### `fitz new` / `fitz init` — scaffolding

```bash
# Crea un proyecto nuevo en una carpeta nueva
fitz new miapp

# Inicializa un proyecto en el cwd actual (sin crear carpeta)
fitz init
```

`fitz new <nombre>` arma:

```text
miapp/
├── fitz.toml          # con [bin] main = "src/main.fitz"
├── .gitignore
└── src/
    └── main.fitz      # template hello world
```

También corre `git init` (a menos que pases `--no-git`). El
nombre debe matchear `^[a-z][a-z0-9_-]{0,63}$` (crates.io style),
con el caveat de que **si querés que sea importable desde código
Fitz no podés usar hyphens** (el parser no admite `-` en
identificadores). Para una lib usable, `miapp` ✓, `mi-app` ✗.

Flag `--http` cambia el template a un server HTTP mínimo
(`@get("/")` que devuelve "Hola"). `--no-git` evita el
`git init`.

### Manifest mode: `fitz run` / `fitz build` / `fitz check` sin args

Cuando estás adentro de un proyecto, el CLI **lee el `fitz.toml`
automáticamente**:

```bash
cd miapp/
fitz run        # corre [bin].main (o [lib].entry si no hay bin)
fitz build      # compila a target/release/miapp{.exe}
fitz check      # chequea tipos del [bin].main
```

El CLI hace **walk-up Cargo-style**: busca `fitz.toml` desde el
cwd y va subiendo carpetas hasta encontrar uno. Útil si corrés el
comando desde `src/` o subcarpetas más profundas.

### Dependencias path

La forma más simple de compartir código entre proyectos locales
es **path deps**:

```toml
[dependencies]
greetings = { path = "../greetings" }
```

`greetings` apunta a un proyecto vecino que declara `[lib]
entry = "src/lib.fitz"`. Desde tu código Fitz:

```fitz
from greetings import hola, formal

print(hola("Fitz"))
```

El loader resuelve `from greetings import X` consultando primero
el dep_registry construido del `fitz.toml`, después fallback a
paths relativos. Caveat: el nombre de la dep debe ser usable como
identificador Fitz (sin hyphens) porque eso es lo que escribís
en el `import`.

### Dependencias git

Para deps remotas:

```toml
[dependencies]
fitz-foo = { git = "https://github.com/algun/foo.git", tag = "v0.2.0" }
otra = { git = "https://github.com/algun/otra.git", rev = "abc123" }
```

Acepta `tag` (release pinned) **o** `rev` (commit SHA exacto) —
mutuamente exclusivos. **No acepta `branch`** porque branches se
mueven (no reproducible). El primer `fitz run` clona la dep al
cache local (`~/.fitz/cache/git/<sanitized-url>@<ref>/`) y la
reusa en corridas siguientes. `fitz update <name>` invalida el
cache y re-clona (útil cuando el tag upstream se actualiza).

### Lockfile `fitz.lock`

Cada `fitz run`/`build`/`check` actualiza `fitz.lock` con la
versión resuelta exacta de cada dep:

```toml
version = 1

[[package]]
name = "greetings"
version = "0.1.0"

[[package]]
name = "fitz-foo"
version = "0.2.0"
source = "git+https://github.com/algun/foo.git#abc123def..."
```

Idempotente: si el manifest no cambió, no se reescribe. Para path
deps no hay `source` (son determinísticas por path). Para git
deps el `source` incluye el commit hash exacto, garantizando que
re-clones siempre traen el mismo árbol.

Convención: commiteás `fitz.lock` en binarios (`[bin]`), lo
dejás fuera en librerías (`[lib]`) — igual que Cargo.

### `fitz add` / `remove` / `update`

Para no editar el `fitz.toml` a mano:

```bash
# Agregar dep path (relativa al manifest)
fitz add greetings --path ../greetings

# Agregar dep git con tag (o --rev <sha>)
fitz add fitz-foo --git https://github.com/algun/foo.git --tag v0.2.0

# Quitar una dep
fitz remove greetings

# Re-resolver deps (invalida cache git, re-clona)
fitz update              # todas
fitz update fitz-foo     # solo una
```

`fitz add` preserva comentarios y formato del `fitz.toml`
original (vía `toml_edit`). Si la dep ya existe con el mismo
nombre, sobreescribe sin preguntar (cargo-style). Para revertir,
`fitz remove`.

### Lo que NO anda todavía

- **Registry público** (`fitz publish` / `fitz add foo@1.2.3`
  sin path ni git) — la decisión de hosting + infra queda
  diferida. Path + git deps cubren el 90% del caso real. Cuando
  aparezca demanda concreta, sub-fase dedicada.
- **Dev-dependencies** (`[dev-dependencies]`) — diferidas como
  sub-paso futuro. Hoy todo dep va en `[dependencies]` y se
  carga siempre.
- **Workspaces** (multi-proyecto con `Cargo.toml` virtual del
  root) — sub-paso futuro si aparece presión.
- **Branches en git deps** — solo `tag` o `rev` (reproducible).
- **Transitive deps** — el dep_registry hoy es flat: si tu lib
  declara una dep, el bin que importa tu lib NO la ve heredada.
  Workaround: declarar la dep también en el bin.

### Ejemplo ejecutable

[`examples/guide/16b-pkg-manager/`](../examples/guide/16b-pkg-manager/)
tiene un mini-proyecto con dos paquetes — `greetings` (lib) y
`greeter` (bin que importa via path dep). Ver el README ahí para
el flujo completo. Test cli_e2e
(`cap_16b_ejemplo_greeter_corre_y_genera_lockfile`) valida que
todo el flow funciona end-to-end.

---

## 17. HTTP nativo

Hasta acá Fitz fue un lenguaje "normal" — tipos, control de flujo,
funciones, módulos. El capítulo de HTTP es donde se nota el
diferencial: no hay librería que importar, no hay framework que
inicializar. Decorás una función con `@get` o `@post`, corrés el
archivo con `fitz run`, y hay un servidor HTTP escuchando.

### Tu primer endpoint

`hola_server.fitz`:

```fitz
@get("/")
fn index() => "hola desde Fitz"
```

Corrélo:

```bash
fitz run hola_server.fitz
```

Salida en consola:

```
🏔️  Fitz HTTP escuchando en http://127.0.0.1:3000
   GET /
```

En otra terminal:

```bash
curl http://127.0.0.1:3000/
```

Respuesta:

```
"hola desde Fitz"
```

Para bajarlo, **Ctrl-C** en la terminal del server. El intérprete
termina de procesar las requests en vuelo y cierra limpio.

Lo que pasó:

1. Al ver `@get("/")` arriba de `fn index`, el intérprete registra
   la ruta en una tabla interna. La fn queda definida como
   cualquier otra; el decorator solo asocia método + path con ella.
2. Al terminar de evaluar el archivo, el intérprete nota que hay
   rutas registradas y arranca el server en `127.0.0.1:3000`.
3. Cuando llega un `GET /`, el server llama a `index()` con los
   args que correspondan (en este caso ninguno), serializa el
   valor de retorno a JSON, y responde 200.

### Verbos: `@get`, `@post`, `@put`, `@delete`

Cada decorator HTTP toma un único argumento: la ruta. Mismas reglas
para todos:

```fitz
@get("/users")
fn list_users() => []

@post("/users")
fn create_user(body) => body

@put("/users/{id}")
fn update_user(id: Int, body) => body

@delete("/users/{id}")
fn delete_user(id: Int) => "ok"
```

Si un decorator no es de los cuatro de arriba (ni `@server`), el
intérprete corta con error explícito.

### Path params: `/users/{id}`

Las llaves dentro del path son params:

```fitz
@get("/users/{id}")
fn get_user(id: Int) {
    return User { id: id, name: "fitz" }
}
```

El nombre adentro de `{...}` tiene que coincidir con un parámetro
de la función. El tipo declarado del parámetro decide cómo se
convierte:

- `id: Int` → el path param se parsea como entero. Si la URL trae
  `/users/abc`, la respuesta es **400** con
  `{"error":"path param 'id': se esperaba Int, recibió 'abc'"}`.
- `id: Float`, `id: Bool`, `id: Str` — mismas reglas, según
  el tipo.
- Sin anotación: llega como `Str`.

Una ruta puede tener varios path params:

```fitz
@get("/orgs/{org}/users/{id}")
fn get_user(org: Str, id: Int) => "{org}/{id}"
```

Repetir un nombre (`/a/{x}/b/{x}`) es error al registrar.

### Body: JSON deserializado a un `type`

Para handlers que reciben datos (típicamente POST y PUT), declarás
un parámetro extra cuyo nombre **no** está en el path. Ese
parámetro es el **body**:

```fitz
type UserInput {
    name: Str
    email: Str?
}

@post("/users")
fn create_user(body: UserInput) {
    return User { id: 1, name: body.name, email: body.email }
}
```

Cuando llega un `POST /users` con
`{"name":"fitz","email":"fitz@example.com"}`:

1. El intérprete parsea el body como JSON.
2. Lo valida contra el `type UserInput`: cada campo declarado tiene
   que estar (con default o ser nullable, o presente).
3. Construye una `Value::Instance` y la pasa como `body` al
   handler.

Si el JSON no parsea, o falta un campo no nullable sin default, o
trae un campo extra, **400** con un mensaje claro:

```
{"error":"body para 'UserInput': falta el campo 'name'"}
{"error":"body para 'UserInput': campo no declarado: age"}
{"error":"body no es JSON válido: ..."}
```

Reglas de la convención:

- Cualquier parámetro del handler que **no** está en `path_params`
  cuenta como body.
- **Máximo uno por handler.** Más de uno → error al registrar.
- Si el body no tiene anotación de tipo (`fn h(body)`), llega como
  `Value` libre — `Map<Str,Value>` para un objeto, `List<Value>`
  para un array, etc. Útil para webhooks o APIs sin schema fijo.

Mezclando path params y body:

```fitz
@put("/users/{id}")
fn update(id: Int, body: UserInput) {
    return User { id: id, name: body.name, email: body.email }
}
```

`id` viene del path, `body` del cuerpo de la request.

### Respuestas: serialización JSON automática

Lo que devolvés del handler se serializa a JSON, sin que tengas
que tocar nada:

| `Value` que devolvés | JSON | Status |
|---|---|---|
| `Int`, `Float`, `Str`, `Bool`, `Null` | el primitivo | 200 |
| `List(...)` | array | 200 |
| `Map(...)` con claves `Str` | object | 200 |
| `Instance` de un `type` | object con campos en orden de declaración | 200 |
| `Ok(v)` | `v` serializado | 200 |
| `Err(e)` | `{"error": e}` | **500** |

Por eso podés escribir el handler como cualquier función Fitz:

```fitz
fn divide(a: Float, b: Float) -> Result<Float> {
    if b == 0.0 {
        return Err("división por cero")
    }
    return Ok(a / b)
}

@get("/half/{n}")
fn half(n: Float) {
    return divide(n, 2.0)
}
```

`GET /half/10` → 200 con `5.0`. `GET /half/0` (si lo armaras para
fallar) → 500 con `{"error":"división por cero"}`.

Tipos que **no** se pueden serializar (funciones, tipos opacos,
módulos, rangos) generan 500 con un mensaje explícito. No te va a
pasar por accidente — pasaría si devolvieras una función entera,
por ejemplo.

### `@server(port, host)` — configurar el server

Por default el server escucha en `127.0.0.1:3000`. Para cambiar
puerto o host, decorá una fn con `@server`:

```fitz
@server(8080, "0.0.0.0")
fn main() => 0

@get("/")
fn index() => "escuchando en todas las interfaces"
```

Reglas:

- Args positional: primero `port: Int`, después `host: Str`. Cualquiera
  se puede omitir (`@server(8080)` deja el host default).
- `port` tiene que estar en `[1, 65535]`. Fuera → error al registrar.
- `host` tiene que parsear como **IP literal** (IPv4 o IPv6). No
  hay resolución DNS — `"localhost"` no funciona, usar
  `"127.0.0.1"`.
- Solo un `@server` por programa. Dos → error con el config previo.

La fn que decora `@server` queda definida en el env como cualquier
otra: no se ejecuta automáticamente. La convención es ponerlo
sobre un placeholder como `fn main() => 0`, pero el nombre no es
mágico.

### Result + ? = handlers limpios

Por la regla de "Result se desempaqueta automático", podés usar
el operador `?` adentro del handler para propagar errores hacia
la respuesta 500:

```fitz
type User { id: Int, name: Str }

let users = [
    User { id: 1, name: "ana" },
    User { id: 2, name: "luis" },
]

fn find_user(id) {
    let found = users.find(fn(u) => u.id == id)
    return match found {
        Ok(u)  => Ok(u)
        Err(_) => Err("usuario {id} no encontrado")
    }
}

@get("/users/{id}")
fn get_user(id: Int) {
    let u = find_user(id)?
    return u
}
```

`GET /users/1` → 200 con `{"id":1,"name":"ana"}`.
`GET /users/99` → 500 con `{"error":"usuario 99 no encontrado"}`.

El `?` corta la fn devolviendo `Err(...)`, el runtime lo destila a
status 500 con `{"error": e}`. Sin try/catch, sin if-not-found,
sin hacer nada raro.

Detalle de sintaxis: hoy un brazo de `match` admite una expresión
como cuerpo, no un statement. Es decir, **`return adentro de un
brazo no parsea**: tenés que hacer `return match { ... }` y poner
el valor directo en cada brazo, como en el ejemplo. Está en la
lista de deuda del lenguaje; cuando se cierre, ambas formas van
a funcionar.

### Qué pasa adentro

Mientras corre, Fitz tiene **un solo runtime tokio multi-thread**
que comparte el intérprete y axum. Cada request entra como una
task tokio independiente, axum la dispatchea a un worker, y el
worker invoca el handler Fitz directamente. **Las requests
corren en paralelo entre workers** — un handler lento no bloquea
a los demás.

Esto es post-Fase F17 (2026-05-14). Antes Fitz tenía dos threads
con un bridge `mpsc/oneshot` entre el intérprete sync y tokio
async — esa indirección la metió la Fase 4 porque los `Value`
usaban `Rc<RefCell<>>` no-Send. F17 migró los containers a
`Arc<parking_lot::Mutex<>>`, lo que destrabó:

- **Send completo** en todo el evaluator (`Value`/`EnvRef`).
- **Eliminación del bridge** (~269 LoC menos en `http.rs`).
- **Paralelismo HTTP real**: 5 requests concurrentes a un
  handler `sleep(1000).await` responden en ~1.2s en lugar de
  ~5s (medido). Ver
  [examples/guide/19b-paralelismo.fitz](../examples/guide/19b-paralelismo.fitz).

### Qué todavía no anda

- **Validación de Content-Type** — cualquier body se intenta
  parsear como JSON. Multipart o urlencoded → cuando hagan falta.
- **Streaming de respuestas** — hoy las respuestas se serializan
  completas antes de mandarse. Server-sent events y descargas
  grandes están en el roadmap.
- **WebSockets** — `@ws("/chat")` está diseñado pero no
  implementado (Fase 9.w).
- **Inferencia de return type en handlers para `fitz build`** —
  las fns HTTP compiladas necesitan anotación de return type
  explícita. El intérprete sí infiere desde el body.

> Lo que **sí anda** y antes era deuda (cerrado fase tras fase):
> async/await reales en handlers (Fase 6), paralelismo HTTP real
> (F17), status codes custom `return 401 { ... }` (post-F7),
> query params `?page=1&size=10` (post-F7), headers de request
> con `@header(name="X")` (Fase 7.6), kwargs en decoradores
> `@server(docs=false)` (Fase 7.0), middleware `@middleware(fn)`
> + CORS con preflight automático (mini-fase MW), state HTTP
> compartido en `fitz build` (F11). El cap incluye sub-secciones
> propias para [Status codes custom](#status-codes-custom),
> [Query params](#query-params) y [Middleware y CORS](#middleware-y-cors)
> más abajo.

### Ejemplo completo

[examples/guide/17-http.fitz](../examples/guide/17-http.fitz):

```fitz
@server(3000)
fn main() => 0

type User {
    id: Int
    name: Str
    email: Str?
}

type UserInput {
    name: Str
    email: Str?
}

let users = [
    User { id: 1, name: "ana", email: "ana@x.com" },
    User { id: 2, name: "luis", email: null },
]

@get("/")
fn index() -> Str => "Fitz HTTP corriendo"

@get("/users")
fn list_users() -> List<User> => users

@get("/users/{id}")
fn get_user(id: Int) -> Result<User> {
    let found = users.find(fn(u) => u.id == id)
    return match found {
        Ok(u)  => Ok(u)
        Err(_) => Err("usuario {id} no encontrado")
    }
}

@post("/users")
fn create_user(body: UserInput) -> User {
    let new_id = users.len() + 1
    let u = User { id: new_id, name: body.name, email: body.email }
    users.push(u)
    return u
}
```

Las anotaciones de return (`-> Str`, `-> Result<User>`, etc.) son
necesarias si vas a compilar el programa con `fitz build`. El
intérprete las infiere igual y funciona sin ellas; el codegen es
más estricto (deuda 5b.1).

Levantalo con `fitz run` (sin compilar):

```bash
cargo run -- run examples/guide/17-http.fitz
```

O compilalo a binario nativo:

```bash
cargo run -- build examples/guide/17-http.fitz
./examples/guide/17-http
```

Y probalo:

```bash
curl http://127.0.0.1:3000/
# "Fitz HTTP corriendo"

curl http://127.0.0.1:3000/users
# [{"id":1,"name":"ana","email":"ana@x.com"},{"id":2,"name":"luis","email":null}]

curl http://127.0.0.1:3000/users/1
# {"id":1,"name":"ana","email":"ana@x.com"}

curl http://127.0.0.1:3000/users/99
# {"error":"usuario 99 no encontrado"}

curl -X POST http://127.0.0.1:3000/users \
     -H "Content-Type: application/json" \
     -d '{"name":"sofi"}'
# {"id":3,"name":"sofi","email":null}

curl http://127.0.0.1:3000/users
# (ahora sofi está adentro)
```

Las modificaciones de `users.push(...)` persisten entre requests
porque `users` es una `List` (compartida por referencia) y todos
los handlers cierran sobre el mismo env del módulo top-level.
**Sin estado externo, sin base de datos: la "memoria" del server
es el env del programa.** Para producción real querés persistir
en disco o en una DB; para prototipos y juguetes, alcanza.

### Status codes custom

Por default, el runtime mapea el retorno del handler así:

- Cualquier valor (`Str`, `Int`, `Instance`, ...) → status **200**.
- `Result<T>::Ok(v)` → **200** con `v` serializado.
- `Result<T>::Err(e)` → **500** con `{"error": "<e>"}`.

Para devolver otro status code, Fitz tiene sintaxis dedicada:
`return <status> <body>` adentro del handler. El status es un
literal Int (rango 100-599); el body es cualquier expresión
serializable a JSON (map literal, struct, valor primitivo).

```fitz
@get("/protected") fn protected() -> Str {
    return 401 {"message": "no autorizado"}
}

@get("/users/{id}") fn get_user(id: Int) -> Str {
    if (id == 1) {
        return "alice"          // 200 (default)
    }
    return 404 {"error": "no encontrado"}
}
```

```bash
curl -i http://127.0.0.1:3000/protected
# HTTP/1.1 401 Unauthorized
# {"message":"no autorizado"}

curl -i http://127.0.0.1:3000/users/1
# HTTP/1.1 200 OK
# "alice"

curl -i http://127.0.0.1:3000/users/2
# HTTP/1.1 404 Not Found
# {"error":"no encontrado"}
```

**Reglas**:

1. `return <int> { ... }` solo funciona adentro de un handler HTTP
   (`@get`/`@post`/`@put`/`@delete`). Afuera, el checker lo rechaza
   con error claro.
2. El body es obligatorio. Para "no content" (204), usá `{}`
   explícito: `return 204 {}`.
3. El status debe ser un literal Int. El parser solo dispara la
   sintaxis nueva cuando ve `Int { ... }`; `return 200 user`
   (sin braces) sigue siendo un `Return` normal del lenguaje.
4. El return type formal del handler se ignora en este path —
   un handler `-> Str` puede mezclar `return "ok"` con
   `return 404 { ... }` en la misma fn.

Las claves del map literal van entre comillas dobles porque la
sintaxis de map literal en Fitz exige que la key sea un valor
(`{"x": 1}`), no un identificador (`{x: 1}` lee la variable `x`).

### Query params

Para recibir parámetros de la query string (`?limit=10&offset=20`),
declarálos adentro del path del decorator con la misma sintaxis
de path params, pero después de un `?`:

```fitz
@get("/items?limit={limit}&offset={offset}")
fn list_items(limit: Int, offset: Int) -> List<Item> {
    // limit y offset llegan ya tipados como Int
    ...
}
```

Cada `{name}` adentro del query corresponde a un parámetro del
handler con el mismo nombre. La key del query y el nombre del
parámetro deben coincidir — `?l={limit}` es error.

**Obligatorios vs opcionales**:

- `limit: Int` → obligatorio. Si la query no incluye `limit=...`,
  la response es 400 con `{"error": "query param 'limit': falta
  — es obligatorio"}`.
- `limit: Int?` → opcional. Si falta, el handler ve `null`.

```fitz
@get("/items?name={name}&limit={limit}")
fn search(name: Str, limit: Int?) -> Str {
    // name es obligatorio; limit puede llegar null
    if (limit == null) {
        return "buscando '{name}' sin límite"
    }
    return "buscando '{name}' con límite {limit}"
}
```

```bash
curl "http://127.0.0.1:3000/items?name=fitz"
# "buscando 'fitz' sin límite"

curl "http://127.0.0.1:3000/items?name=fitz&limit=10"
# "buscando 'fitz' con límite 10"

curl "http://127.0.0.1:3000/items?limit=10"
# {"error":"query param 'name': falta — es obligatorio"}
```

**Tipos soportados** en query params: `Int`, `Float`, `Str`,
`Bool` (los primitivos), opcionalmente nullables (`Int?`, etc.).
`List<T>` y tipos custom no se soportan todavía — irían como
body, no como query param.

**Coerción**: los valores de query siempre llegan como `String`
desde HTTP. Fitz los parsea al tipo declarado. Si el parse falla
(`limit=abc` con `limit: Int`), 400 con el mensaje claro.

**Combinable con path params y body**: una ruta puede tener
los tres a la vez.

```fitz
type Patch { value: Int }

@put("/items/{id}?dry_run={dry_run}")
fn update_item(id: Int, dry_run: Bool, body: Patch) -> Str {
    // id ← path, dry_run ← query, body ← JSON del request
    ...
}
```

### Middleware y CORS

Hasta acá los handlers responden directamente. Para todo lo que pasa
**antes** del handler — logging, autenticación, rate limiting, CORS —
Fitz tiene **middleware**: funciones que se apilan sobre un handler
con `@middleware(fn)` y se ejecutan en orden top-down.

**Sintaxis**:

```fitz
fn logger(req: Request) {
    // no devuelve nada → la cadena continúa
}

fn auth(req: Request) {
    if (req.headers.has("authorization")) {
        return null
    }
    return 401 {"error": "falta header Authorization"}
}

@middleware(logger)
@middleware(auth)
@get("/admin")
fn admin() -> Str => "datos administrativos"
```

**Reglas**:

1. Los `@middleware(...)` deben apilarse **antes** del decorator de
   ruta (`@get`/`@post`/`@put`/`@delete`).
2. Cada middleware recibe un único arg `Request` (built-in con
   fields `method: Str`, `path: Str`, `headers: Map<Str, Str>`).
   Los headers llegan con las keys en lowercase.
3. **Modelo gate-only**: el middleware puede *cortar la cadena* con
   `return <status> { ... }`, o *dejarla seguir* devolviendo `null`
   (o sin return explícito al cierre del body). Cualquier otro valor
   de retorno es error.
4. El orden de ejecución es **top-down**: el `@middleware(...)` más
   arriba corre primero. El último corre justo antes del handler.

**CORS**:

Para servir APIs a un frontend (Vue, React, etc.) que vive en otro
dominio hace falta CORS. Fitz trae un built-in `cors(...)` que se
aplica como un middleware más:

```fitz
@middleware(cors())
@get("/api/items")
fn list_items() -> List<Item> => items
```

`cors()` sin args usa defaults permisivos: `allow_origin: "*"`,
métodos `GET/POST/PUT/DELETE/OPTIONS`, headers `content-type` y
`authorization`. Para overrides, pasale un Map con las keys que
quieras pisar:

```fitz
@middleware(cors({
    "allow_origin": "https://app.example.com",
    "allow_methods": ["GET", "POST"],
    "max_age": 3600
}))
@get("/api/items")
fn list_items() -> List<Item> => items
```

Keys soportadas:

- `allow_origin: Str` (default `"*"`).
- `allow_methods: List<Str>` (default métodos comunes).
- `allow_headers: List<Str>` (default `["content-type", "authorization"]`).
- `max_age: Int` (default ausente — el browser usa su cache default).

Cuando aplicás `cors(...)`:

- El runtime registra un handler **OPTIONS** automático para el
  mismo path. Una request preflight `OPTIONS /api/items` responde
  **204 No Content** con los headers `Access-Control-Allow-*` ya
  configurados, sin tocar tu handler.
- Las responses reales del handler (`GET /api/items`, `POST ...`)
  llevan los headers `Access-Control-Allow-Origin` etc. inyectados.
  Esto vale también para responses de error (500/400/etc.) — sin
  eso, el browser tapa el error real con un "CORS error" en
  consola.

**Restricciones**:

- Máximo **un** `cors(...)` por ruta. Apilar dos da error.
- `cors(...)` y user-fn middlewares conviven sin problema:
  ```fitz
  @middleware(logger)
  @middleware(cors())
  @get("/api")
  fn endpoint() => ...
  ```
- En `fitz build`, `cors(...)` se evalúa en build-time: el codegen
  emite un `static __FITZ_CORS_*` con los headers precomputados y
  un handler de preflight dedicado. Cero overhead por request.

**Ejemplo completo**: `examples/guide/17b-middleware.fitz`.

---

Con HTTP cerramos la Fase 4 y la mini-fase de Middleware + CORS
post-Fase 7. Tenés ahora todas las piezas para escribir APIs reales
en Fitz: rutas, JSON tipado, manejo de errores propagable,
configuración del server, status codes custom, query params,
middleware apilable y CORS configurable. El próximo capítulo cubre
la **paridad con FastAPI en developer experience**: documentación
de la API autogenerada (`/openapi.json` + UI Scalar en `/docs`).
Después, el cap 19 cubre la otra mitad de "HTTP nativo":
concurrencia con `async fn` y `.await`.

---

## 18. Docs automáticas

Fitz expone dos rutas más cuando hay handlers HTTP en el
programa, sin que tengas que hacer nada:

| Ruta | Qué sirve |
|------|-----------|
| `GET /openapi.json` | Schema OpenAPI 3.1 autogenerado del programa |
| `GET /docs` | UI [Scalar](https://scalar.com/) interactiva (carga el schema en el browser) |

Cualquier herramienta del ecosistema OpenAPI (Postman, Insomnia,
`openapi-generator` para SDKs en otros lenguajes, etc.) se enchufa
directo contra `/openapi.json`.

### Cómo funciona

El runtime HTTP recorre los decoradores que ya escribiste (`@get`,
`@post`, `@header`, los tipos custom anotados como body, las
anotaciones de return type) y arma el schema en memoria al
arrancar el server. El subcomando `fitz openapi archivo.fitz`
escupe el mismo schema a stdout, útil para CI o snapshot testing
del contrato sin tener que levantar el server.

```bash
fitz openapi mi_api.fitz > schema.json
```

### Mapping `TypeExpr` → JSON Schema

| Fitz | JSON Schema emitido |
|------|---------------------|
| `Int` | `{"type":"integer","format":"int64"}` |
| `Float` | `{"type":"number"}` |
| `Str` | `{"type":"string"}` |
| `Bool` | `{"type":"boolean"}` |
| `T?` | schema de `T` + `"nullable": true` |
| `List<T>` | `{"type":"array","items":<T>}` |
| `Map<Str, V>` | `{"type":"object","additionalProperties":<V>}` |
| `Result<T>` (en return) | `200` con schema de `T` + `500` con `{error: string}` |
| `User` (nominal) | `{"$ref":"#/components/schemas/User"}` |

Cada `type Foo { ... }` del programa entra a
`components.schemas.Foo` con sus campos como properties.
`required` incluye los campos sin default y no nullables; el
resto queda como opcional.

### Headers como params del handler

`@header(name="HTTP-Name")` apilado antes del decorator de ruta
declara que un param del handler viene de un header HTTP. El
nombre del param Fitz se deriva por convención:
**lowercase + `-` → `_`**.

```fitz
@header(name="Authorization")
@get("/private")
fn private(authorization: Str) -> Str => authorization

@header(name="X-Trace-Id")
@get("/traced")
fn traced(x_trace_id: Str?) -> Str => "ok"
```

Reglas:

- Tipos soportados: `Str` (obligatorio) y `Str?` (opcional). Si
  declarás otro tipo, el evaluador rechaza con error claro.
- Falta header obligatorio → respuesta `400` con
  `{"error":"header 'Foo': falta — es obligatorio"}`.
- Lookup case-insensitive en HTTP: `authorization` matchea
  contra `@header(name="Authorization")`.
- En el schema OpenAPI, los headers aparecen como `parameters`
  con `in: "header"` y `required` derivado del tipo.

### Opt-out: `@server(docs=false)`

Default: docs habilitados. Si querés apagarlos (servidor más
chico, schema no público, etc.):

```fitz
@server(3000, docs=false)
fn main() => 0
```

Con `docs=false`, ni `/openapi.json` ni `/docs` se registran
(ambas devuelven `404`). El opt-out funciona idéntico en `fitz
run` y `fitz build`.

### Paridad `fitz run` ↔ `fitz build`

El schema generado es **bit-a-bit idéntico** entre `fitz openapi
archivo.fitz`, `fitz run archivo.fitz` (sirviendo `/openapi.json`)
y `fitz build archivo.fitz` (el binario nativo embebe el schema
como `&'static str` al compilar). Una sola fuente de verdad para
el contrato.

### Si el usuario declara `/openapi.json` o `/docs` propio

`@get("/openapi.json") fn miyo() -> ...` se respeta — el
auto-register cede. Mismo comportamiento para `/docs`. Útil si
querés servir un schema custom o una UI distinta.

### Ejemplo ejecutable

[examples/guide/18-docs.fitz](../examples/guide/18-docs.fitz)
muestra un CRUD chiquito con path params, query params, body
tipado, return `Result<T>` (status `200`/`500`), header
obligatorio. Compila con `fitz build` end-to-end.

```bash
fitz run examples/guide/18-docs.fitz
# en otra terminal:
curl http://127.0.0.1:3000/openapi.json | head -20
open http://127.0.0.1:3000/docs        # macOS — abrí la UI en el browser
```

### Limitaciones conocidas

- **Descripciones vacías**: `info.description` y
  `paths.*.*.description` no se llenan todavía. El lexer hoy
  descarta comentarios; doc-strings sobre handlers son deuda
  post-F17 (refactor invasivo del lexer/parser/AST).
- **`@header` solo acepta `Str`/`Str?`**: si querés un header
  numérico, parsealo adentro del handler.
- **Bundle Scalar offline**: la UI `/docs` carga el bundle JS
  desde `cdn.jsdelivr.net`. El browser cachea tras el primer
  load, pero hace falta red la primera vez. Embeber offline
  cuesta ~3.7 MB extra al binario; quedó como deuda menor.

**Cerradas en la tanda Q (2026-05-14)**:

- **`info.version` override**: `@server(api_version="X.Y.Z")` lo
  refleja en el schema.
- **Aliases en `@header`**: `@header(name="X-Auth", into="token")`
  mapea explícito a un param Fitz.
- **Status codes custom en el schema**: un handler con
  `return 404 { ... }` produce ahora un entry `"404"` en
  `responses`, con description vía reason phrase HTTP.

---

## 19. Async y concurrencia

Hasta acá las funciones de Fitz son sincrónicas: corren, devuelven,
fin. `async fn` agrega una segunda forma: la función devuelve un
**valor pendiente** (un `Future<T>`) que se "ejecuta" cuando otra
parte del código lo **await**-ea. Sirve para operaciones que toman
tiempo sin trabajo de CPU — esperar una respuesta HTTP, leer de
disco, dormir N milisegundos — sin bloquear todo el intérprete.

Fitz cumple la promesa de async nativo: `async fn`, `Future<T>` y
el operador `.await` están en el core del lenguaje, no en una lib.
El runtime tokio (de Rust) maneja el scheduling abajo; el usuario
solo ve la sintaxis.

### `async fn` — declarar una función async

Una `async fn` se ve igual que una `fn` normal pero con el prefijo
`async`:

```fitz
async fn pausa(ms: Int) -> Int {
    let _ = sleep(ms).await
    return ms
}
```

Por fuera, la firma de `pausa` es `(Int) -> Future<Int>`: llamarla
NO ejecuta el cuerpo, devuelve un `Future` que representa la
ejecución pendiente. Por dentro, los `return n` siguen retornando
`Int` puro — el `async` es **transparente desde adentro** del cuerpo.

### `.await` — desempaqueta un `Future`

`.await` es **postfix** (después del valor) y encaja naturalmente
en method chains:

```fitz
let n = pausa(100).await        // espera 100ms, devuelve 100
let m = pausa(50).await + 1     // chain: 50 + 1 = 51
```

Sin `.await`, la llamada devuelve el `Future<Int>` "crudo" — útil
para guardarlo, pasarlo como argumento o componerlo. Con `.await`,
el future se ejecuta y obtenemos el `Int` interno.

### `Future<T>` como tipo

`Future<T>` es un genérico built-in, igual que `List<T>` o
`Result<T>`:

```fitz
let pending: Future<Int> = pausa(0)
let value: Int = pending.await
```

Aparece naturalmente en el return type de cualquier `async fn`
(desde afuera) y se puede usar en anotaciones de variables,
parámetros y campos de tipo.

### El builtin `sleep(ms)`

`sleep(ms: Int) -> Future<Null>` produce un future que pausa N
milisegundos cuando se await-ea. Es el primer "async primitive"
del lenguaje:

```fitz
async fn esperar_y_saludar(nombre: Str) -> Str {
    let _ = sleep(100).await
    return "hola, {nombre}"
}
```

### Dónde se permite `.await`

`.await` solo dispara la ejecución de un `Future` si está adentro
de un contexto async. Las reglas:

| Contexto                                          | `.await` permitido |
|---------------------------------------------------|--------------------|
| Adentro de una `async fn`                         | ✅ sí              |
| A nivel **top-level** del archivo                 | ✅ sí (el runtime tokio se arma al ejecutar) |
| Adentro de una `fn` sync                          | ❌ error de tipo   |
| Adentro de `fn(x) => ...` (FnExpr / closure)      | ❌ error de tipo   |

El último caso es porque Fitz **no soporta closures async**
todavía (`async fn(x) => ...` no existe en la gramática). Si
necesitás un callback async, declarate una `async fn` con nombre
y pasala como valor.

### Handlers HTTP async

Cualquier handler HTTP puede ser `async fn`. El runtime tokio
existente lo invoca con `.await` automático; el usuario no escribe
nada extra:

```fitz
@server(3000)
fn main() => 0

@get("/lento")
async fn lento() -> Str {
    let _ = sleep(500).await
    return "después de medio segundo"
}

@get("/rapido")
fn rapido() -> Str => "ya"
```

Los handlers sync y async **conviven libremente**. axum los acepta
ambos. La diferencia se nota cuando el handler async hace I/O real
(en el futuro: `fetch(url).await`, `db.query(...).await`): un
endpoint que está esperando una respuesta externa cede CPU para
que el intérprete avance otras tareas adentro del mismo handler.

### Paralelismo HTTP real

El runtime HTTP usa `tokio` en modo **multi-thread**: N workers
según los cores disponibles procesan handlers en simultáneo. Dos
requests al mismo handler — aún uno lento — corren en paralelo,
una por worker. Concurrencia real, no solo intercalada adentro
de un mismo handler.

Demostración con `examples/guide/19b-paralelismo.fitz`:

```fitz
@server(3000)
fn main() => 0

@get("/lento")
async fn lento() -> Str {
    let _ = sleep(1000).await
    return "ok"
}
```

5 requests concurrentes vs 5 en serie:

```sh
fitz run examples/guide/19b-paralelismo.fitz &

# Paralelo: 5 requests al mismo tiempo
time seq 5 | xargs -P 5 -I _ curl -s http://127.0.0.1:3000/lento
# real    0m1.2s   ← cada worker duerme 1s, todos en paralelo

# Serie: una atrás de otra
time for i in 1 2 3 4 5; do curl -s http://127.0.0.1:3000/lento; done
# real    0m5.3s   ← suma de los sleeps
```

Pre-F17 ambos eran ~5s (el server estaba en `current_thread`).
Post-F17 los contenedores de `Value` y `EnvRef` migraron a
`Arc<Mutex<>>` (Send + Sync) y axum invoca el evaluator directo
sobre sus workers — sin bridge, sin serialización.

Para programas CLI con `.await`, esto no cambia nada: hay una
sola tarea a la vez, runtime `current_thread` es suficiente.

### Ejemplos del capítulo

Dos ejemplos:

- **[examples/guide/19-async.fitz](../examples/guide/19-async.fitz)**
  — CLI con tres `async fn` que se componen y se await-ean desde
  top-level.
- **[examples/guide/19b-paralelismo.fitz](../examples/guide/19b-paralelismo.fitz)**
  — server HTTP con un handler lento (`sleep(1000)`) y uno rápido
  para medir paralelismo real con curl.

```sh
# CLI async
fitz run examples/guide/19-async.fitz
fitz build examples/guide/19-async.fitz
./examples/guide/19-async                # Linux/macOS
.\examples\guide\19-async.exe            # Windows

# Server con paralelismo
fitz run examples/guide/19b-paralelismo.fitz
fitz build examples/guide/19b-paralelismo.fitz
./examples/guide/19b-paralelismo &
time seq 5 | xargs -P 5 -I _ curl -s http://127.0.0.1:3000/lento
```

Salida esperada del cap 19 CLI:

```
hola, Fitz
total ms = 0
```

---

Async cumple la promesa de "HTTP nativo" a nivel de ejecución:
podés escribir un handler que pausa, cede CPU, sigue. Y con N
workers tokio, varios pueden estar pausando a la vez. El próximo
capítulo cubre el otro gran salto: **compilar el programa a un
binario nativo standalone** con `fitz build`.

---

## 20. `fitz build` — compilar a binario nativo

Hasta acá usamos siempre `fitz run`: el intérprete lee el archivo,
lo lexea, parsea, chequea y ejecuta en proceso. Es rápido para
iterar y conserva toda la riqueza del lenguaje (lista heterogénea,
inferencia completa sin anotaciones, mutación implícita).

`fitz build` toma el mismo `.fitz` y produce un **binario nativo
standalone** que corre sin Fitz instalado. Es el modo "deployar":
más lento de compilar (segundos en vez de milisegundos), pero el
output es un ejecutable que podés copiar a otro servidor.

### Cómo funciona

```
fitz build hello.fitz
```

Hace, en orden:

1. **Lexer + parser**: igual que `fitz run`.
2. **Type checker estático en modo strict** (sin `--no-typecheck`):
   los errores de tipo abortan el build acá.
3. **Codegen**: traduce el AST a un **Cargo project** completo
   adentro de `target/fitz-build/<nombre>/`. Estructura:
   ```
   target/fitz-build/hello/
   ├── Cargo.toml
   └── src/
       ├── main.rs
       └── (mod_files si hay imports)
   ```
4. **`cargo build --release`**: invoca Cargo, que llama a rustc.
   Si el programa tiene `@get`/`@post`/etc., el `Cargo.toml`
   incluye `axum`, `tokio`, `serde` y `serde_json` como
   dependencias. Sin HTTP, queda minimalista.
5. **Copia el binario** producido (`target/release/hello`)
   adyacente al `.fitz` original. En Windows es `hello.exe`; en
   Linux/macOS, `hello`.

Inspeccionar el Rust generado es libre: el `src/main.rs` queda
ahí mientras no lo borres. Si rustc se queja, ver el código
generado suele desambiguar.

### Mapping de tipos Fitz → Rust

Acá la traducción base, para que el código generado no te tome
por sorpresa si lo abrís:

| Fitz                 | Rust                                                          |
|----------------------|---------------------------------------------------------------|
| `Int`                | `i64`                                                         |
| `Float`              | `f64`                                                         |
| `Str`                | `String`                                                      |
| `Bool`               | `bool`                                                        |
| `Null`               | `()`                                                          |
| `T?`                 | `Option<T>`                                                   |
| `List<T>`            | `Rc<RefCell<Vec<T>>>` (referencia compartida)                 |
| `Map<K, V>`          | `Rc<RefCell<Vec<(K, V)>>>` (orden de inserción preservado)    |
| `Result<T>`          | `Result<T, String>` (Err pinned a String — ver cap 14)        |
| `type Foo { ... }`   | `struct FooData { ... }` + `type Foo = Rc<RefCell<FooData>>;` |

Las instancias de tipos custom van detrás de `Rc<RefCell<>>` para
preservar la semántica del intérprete: mutar `u.name = "x"` a
través de un alias se ve en cualquier otra var que apunte a la
misma instancia.

### Qué se soporta

| Feature                                            | Soporte |
|----------------------------------------------------|---------|
| Primitivos + operadores + interpolación            | ✅      |
| `if` / `else` / `while` / `loop` / `for ... in`    | ✅      |
| `match` con literales, ranges, Ok/Err, wildcards   | ✅      |
| Tipos custom: instanciación, fields, defaults      | ✅      |
| Listas y mapas (**homogéneos**), indexing, métodos | ✅      |
| `Result`, `?`, propagación de Err                  | ✅      |
| Módulos: `import foo` / `from foo import X`        | ✅      |
| Funciones anónimas, closures, `Fn(...) -> ...`     | ✅      |
| HTTP: `@get`/`@post`/`@put`/`@delete`, `@server`   | ✅      |
| Body JSON deserializado contra `type` custom       | ✅      |
| Serialización JSON automática de respuestas        | ✅      |

### Qué todavía no anda con `fitz build`

Cosas que sí corren con `fitz run` pero todavía no compilan:

- **Funciones sin anotar params** — `fn greet(name)` corre en el
  intérprete (el tipo se infiere desde el body). El compilador
  exige `fn greet(name: Str) -> Str`. Workaround: anotar.
- **Listas/mapas heterogéneos** — `[1, "dos", true]` corre en el
  intérprete (cada item conserva su tipo). El compilador exige
  homogéneo (`List<Int>`, `Map<Str, Int>`, etc.) porque Rust no
  tiene un tipo "Value" genérico tagged en runtime sin un
  refactor. Workaround: armar dos colecciones, o usar `fitz run`.
- **`let X = <expr>` no literal a nivel top de un módulo** — las
  constantes top-level de un módulo deben tener una RHS literal
  (`"texto"`, `42`, `3.14`, `true`, `null`). `let X = compute()`
  no compila.
- **Imports transitivos** — un módulo cargado por el main no
  puede tener su propio `import`. Workaround: aplaná los imports.
- **División por cero literal** — `print(10 / 0)` no compila
  (rustc rechaza la operación en compile-time). En el intérprete
  es un error de runtime explícito.
- **Comparar valores de tipos distintos** — `1 == "1"` corre en
  el intérprete (devuelve `false` porque los tipos no coinciden);
  el compilador rechaza la comparación.

Si te tropezás con algo de esta lista, el mensaje del codegen lo
cita explícitamente. La salida tiene la forma:

```
✗ codegen: Error — <descripción> ...
   (Fase 5b soporta un subset progresivo; los mensajes citan el sub-paso correspondiente.)
```

> Lo que **sí anda** y antes era deuda: state HTTP compartido
> (cualquier `let users = [...]` top-level que un handler
> referencia, cerrado en F11), **paralelismo HTTP real** entre
> requests con el binario compilado (post-F17, runtime tokio
> multi-thread default), interop Python en `fitz build`
> (Fase 8.7 cerrada — `from python import X` produce binarios
> nativos con PyO3 linkeado). Ver
> [cap 19 sub-sección "Paralelismo HTTP real"](#paralelismo-http-real).

### Ejemplo: programa CLI primitivo

```fitz
let name = "Fitz"
let x = 10 + 5
print("Hola, {name}, x es {x}")

fn double(n: Int) -> Int => n * 2
print(double(x))
```

```
$ fitz build hello.fitz
✓ binario: hello.exe

$ ./hello.exe
Hola, Fitz, x es 15
30
```

### Ejemplo: server HTTP compilado

[examples/guide/20-build.fitz](../examples/guide/20-build.fitz)
es un server HTTP simple que cubre endpoints estáticos, path params,
Result con Err → 500, y un POST con body deserializado a un `type`
custom:

```fitz
@server(3000)
fn main() => 0

@get("/")
fn index() -> Str => "Fitz HTTP compilado"

@get("/double/{n}")
fn double(n: Int) -> Result<Int> {
    if (n < 0) {
        return Err("n debe ser >= 0")
    }
    return Ok(n * 2)
}

type Echo {
    msg: Str
    times: Int = 1
    note: Str?
}

@post("/echo")
fn echo(body: Echo) -> Echo => body
```

```
$ fitz build examples/guide/20-build.fitz
✓ binario: examples/guide/20-build.exe

$ ./examples/guide/20-build.exe &
Fitz HTTP escuchando en http://127.0.0.1:3000

$ curl http://127.0.0.1:3000/
"Fitz HTTP compilado"

$ curl http://127.0.0.1:3000/double/21
42

$ curl http://127.0.0.1:3000/double/-1
{"error":"n debe ser >= 0"}

$ curl -X POST http://127.0.0.1:3000/echo \
       -H "Content-Type: application/json" \
       -d '{"msg":"hola"}'
{"msg":"hola","times":1,"note":null}
```

El binario es ~5 MB y arranca instantáneo. No necesita ni Fitz
ni Rust instalados en la máquina destino: es un ejecutable
nativo standalone.

### Cuándo usar `fitz run` y cuándo `fitz build`

- **Iterando**: `fitz run`. Cambios en el `.fitz` se reflejan
  inmediato; sin paso de compilación.
- **Explorando features experimentales**: `fitz run`. Listas
  heterogéneas, error de runtime puro como división por cero — todo
  lo que está en "qué todavía no anda" sigue funcionando en el
  intérprete sin restricciones.
- **Producción / deploy**: `fitz build`. Un binario que se
  copia a un servidor y arranca, sin runtime de Fitz alrededor.
- **Distribuir un script CLI**: `fitz build`. El binario chico
  es más fácil de compartir que pedir a alguien que instale
  Fitz primero.

### Cross-compilation

Como por debajo está rustc, **cross-compilar es gratis** vía
`rustup target add <triple>`. El subcomando `fitz build` todavía
no expone una flag `--target`, pero el flujo está abierto: en el
project generado podés correr `cargo build --release --target
x86_64-unknown-linux-musl` (o el target que precises) directo.
Si esto te interesa, abrí un issue.

---

Con `fitz build` cerramos Fase 5. El lenguaje tiene ahora todas
las piezas centrales: type checker estático, intérprete maduro,
y un compilador que genera binarios nativos para CLI y HTTP. El
último capítulo es el mapa hacia adelante.

---

## 21. Interop Python

Fitz puede llamar código Python desde sus programas. La motivación
es práctica: Python tiene el ecosistema más grande del mundo
(SQLAlchemy, numpy, pandas, httpx, FastAPI...). Pedirle al usuario
de Fitz que reescriba todo de cero es irreal. La interop con Python
es el puente para usar Fitz hoy, sin renunciar a las librerías que
ya tenés en tu stack.

### 21.1 Setup

La interop con Python es **opt-in**: el binario `fitz` default NO
linkea libpython. Para activarla, compilá `fitz` con la feature
`python`:

```bash
cargo build --features python
```

Eso le pide a PyO3 que linkee CPython 3.10+ al binario `fitz`.
Los programas Fitz que **no** usan `from python import` siguen
produciendo binarios libres como en Fase 5b (cero costo si no
necesitás la interop).

**Política de venvs**: el patrón estándar Python — activá tu venv
con `source venv/bin/activate` (o equivalente Windows) antes de
correr Fitz; CPython lee `VIRTUAL_ENV` automáticamente al bootear.
Sin venv, Python busca los paquetes en el `site-packages` global.

### 21.2 Sintaxis: `from python import`

Fitz reusa la sintaxis de imports que ya conocés del cap 16, con
un namespace virtual `python` reservado:

```fitz
from python import math
from python import json
from python import asyncio
```

También aceptamos formas alternativas:

```fitz
from python import math as m         // alias
import python.os.path                // import punteado, binding `path`
from python import os.path as p      // alias + path punteado
```

El último segmento del path (o el alias) es el binding visible en
el scope Fitz.

### 21.3 Constantes y atributos

`math.pi`, `os.name`, etc. — el field access funciona igual que
sobre tipos Fitz, pero del otro lado hay un objeto Python:

```fitz
from python import math

print("pi = {math.pi}")                    // pi = 3.141592653589793
print("e  = {math.e}")                     // e  = 2.718281828459045
print("nombre = {math.__name__}")          // nombre = math
```

El intérprete coerciona automáticamente los primitivos Python
(`int → Int`, `float → Float`, `str → Str`, `bool → Bool`,
`None → Null`). Cualquier otro tipo (función, clase, instancia,
submódulo) queda como **`PyObject` opaco** — podés pasarlo a otra
fn Python o hacer field access más adentro, pero Fitz no sabe qué
hay adentro.

### 21.4 Llamadas a funciones Python

Las llamadas usan la sintaxis de Fitz pero con un detalle clave: 
**toda llamada Python devuelve un `Result<T>` automáticamente**.

```fitz
from python import math

let raw = math.sqrt(16.0)
match raw {
    Ok(v)  => print("sqrt(16) = {v}"),     // sqrt(16) = 4.0
    Err(e) => print("error: {e}"),
}
```

Si la función Python lanza una excepción (`ValueError`,
`TypeError`, etc.), Fitz la captura y la convierte en
`Err(Str("<ClassName>: <message>"))`:

```fitz
let raw = math.sqrt(-1.0)
match raw {
    Ok(_)  => print("(no debería)"),
    Err(e) => print("caught: {e}"),       // caught: ValueError: math domain error
}
```

Esto preserva la decisión de diseño "sin excepciones" de Fitz: el
usuario es forzado a manejar la falla con `match` o `?`, igual
que con `find`/`get`/`json.loads` nativos. El programa Fitz **no
aborta** por excepciones Python.

### 21.5 Propagación con `?`

Adentro de una fn que retorna `Result<T>`, el operador `?` propaga
errores Python sin glue manual:

```fitz
from python import math

fn root_safe(x: Float) -> Result<Float> {
    let v: Float = math.sqrt(x)?
    return Ok(v)
}

match root_safe(25.0) {
    Ok(r)  => print("r = {r}"),           // r = 5.0
    Err(e) => print("err: {e}"),
}
```

Notá la **anotación destino** `let v: Float = ...`: como
`math.sqrt(x)?` desempaca a `PyAny`, la anotación dispara una
coerción automática de Python `float` a `Float` Fitz. Sin la
anotación, `v` queda como `PyAny` opaco.

### 21.6 Marshaling de tipos compuestos

Listas, mapas e instancias Fitz cruzan a Python como `list`, `dict`
y `dict` (por field name), respectivamente:

```fitz
from python import json

let xs: List<Int> = [1, 2, 3]
match json.dumps(xs) {
    Ok(s)  => print(s),                   // [1, 2, 3]
    Err(_) => print("err"),
}

type User { id: Int, name: Str }
let u = User { id: 1, name: "Ada" }
match json.dumps(u) {
    Ok(s)  => print(s),                   // {"id": 1, "name": "Ada"}
    Err(_) => print("err"),
}
```

En sentido contrario, Python `list` y `dict` cruzan a Fitz como
`List` y `Map` opacos (sin tipo concreto del lado Fitz). El próximo
capítulo cubre cómo recuperar el tipo Fitz con anotaciones.

### 21.7 Recuperando tipos Fitz desde Python

Cuando una fn Python te devuelve un `dict` que querés tratar como
una instancia Fitz, **anotá el binding destino** y el runtime hace
la coerción automática (paralelo a Fase 8.4):

```fitz
from python import json

type User {
    id: Int,
    name: Str,
    email: Str? = null,
}

fn parse_user(s: Str) -> Result<User> {
    let row: User = json.loads(s)?
    return Ok(row)
}

match parse_user("{\"id\": 1, \"name\": \"Ada\"}") {
    Ok(u)  => print("User: {u.name}"),    // User: Ada
    Err(e) => print("err: {e}"),
}
```

El runtime itera los fields declarados:

- Si el `dict` tiene el field → lo usa.
- Si no, aplica el default (si hay).
- Si no hay default pero el field es nullable → `null`.
- Si no hay default ni es nullable → error claro.

Campos extras del `dict` se ignoran silenciosamente (Python suele
devolver más data de la necesaria; SQLAlchemy es típico ahí).

### 21.8 `fitz py-types` — auto-mapeo de modelos SQLAlchemy

Si tu proyecto Python usa SQLAlchemy, podés generar los `type`
Fitz correspondientes con un comando:

```bash
fitz py-types models.py --out models.fitz
```

El comando introspecciona las clases con `__table__.columns` y emite:

```fitz
// Generado por fitz py-types desde models.py
type User {
    id: Int,
    name: Str,
    email: Str? = null,
    created_at: Str,
}
```

Mapeo: `Integer/BigInteger → Int`, `Float/Numeric → Float`,
`String/Text → Str`, `Boolean → Bool`, `DateTime/Date → Str`
(ISO 8601), `nullable=True → ?`, default literal inline, callable
ignorado. Tipos desconocidos quedan como `Any` con un comentario
para refinar a mano.

Después de generar, podés usar `from models import User, Order`
en tus archivos Fitz y los tipos están listos para combinar con
el patrón `let row: User = py_call(...)?` del cap 21.7.

### 21.9 Async — `await` sobre corutinas Python

Cuando una fn Python es `async def` (devuelve una corutina), el
`.await` Fitz la ejecuta vía el bridge tokio ↔ asyncio:

```fitz
from python import asyncio

async fn esperita() -> Result<Str> {
    let _ = asyncio.sleep(0.1)?.await
    return Ok("done")
}

match esperita().await {
    Ok(v)  => print("got = {v}"),         // got = done
    Err(e) => print("err: {e}"),
}
```

El patrón canónico Fitz es `<py_call>?.await`. El `?` desempaca
el `Result` del call (el wrap automático del cap 21.4); el `.await`
ejecuta la corutina. Excepciones asyncio aparecen como `Err`,
igual que con calls sync.

**Implementación**: el bridge usa `tokio::task::spawn_blocking` +
`asyncio.run_until_complete()` adentro del worker. Funcional para
APIs DB-bound (queries SQLAlchemy/asyncpg cortas). El GIL serializa
las corutinas — para hot paths CPU-bound, mejor reescribir en
Fitz nativo.

### 21.10 `fitz build` con interop Python

`fitz build` también compila programas con `from python import`.
El binario resultante linkea pyo3 con `abi3-py310 + auto-initialize`
y asume Python instalado en la máquina destino (igual que el
binario `fitz` mismo necesita CPython al boot).

```bash
cargo run --features python -- build mi_app.fitz
./mi_app  # requiere Python 3.10+ en el PATH
```

Bit-a-bit con `fitz run` para los patrones cubiertos. Lo que
**falta** vs `fitz run` (deuda residual de 8.7):

- Coerción Python `list` → Fitz `List<T>`, `dict` → `Map<K,V>`,
  `dict` → `Instance` (con anotaciones del 21.7). En `fitz build`
  estos casos siguen quedando como `PyObject` opaco.
- `.await` con binding intermedio split (`let fut = py_call()?;
  fut.await` con el future ligado a una var antes del await). El
  patrón inmediato `<py_call>?.await` sí anda.

Para el caso canónico de CRUD con SQLAlchemy + recuperación de
filas como instancias Fitz tipadas, usá `fitz run` por ahora.
`fitz build` cubre handlers HTTP CRUD si no necesitás esa
coerción (pasar dict opaco al cliente vía `serde_json` también
funciona).

**Bundling de CPython embebido** (`fitz build --bundle-python`)
queda como sub-paso futuro separado. Hoy el binario asume Python
instalado; bundling lo embebería para hacer el binario realmente
standalone.

### 21.11 Ejemplo CRUD ejecutable

`examples/guide/21-python-crud/` arma un CRUD completo:

- `models.py` — modelo SQLAlchemy (User) sobre SQLite.
- `db.py` — helpers DB (init, add, list, get) que devuelven
  dicts/lists nativos Python (sin instancias del modelo
  SQLAlchemy) para que el marshaling a Fitz sea directo.
- `models.fitz` — output de `fitz py-types models.py`.
- `app.fitz` — handlers HTTP `@get`/`@post` que insertan y
  listan usuarios via los helpers de `db.py`.

Setup (una vez):

```bash
pip install sqlalchemy
```

Correr (`PYTHONPATH` apunta a la carpeta del ejemplo para que
Python encuentre `db.py` y `models.py`):

```bash
# Linux/macOS:
PYTHONPATH=examples/guide/21-python-crud \
  cargo run --features python -- run examples/guide/21-python-crud/app.fitz

# Windows PowerShell:
$env:PYTHONPATH = "examples\guide\21-python-crud"
cargo run --features python -- run examples/guide/21-python-crud/app.fitz
```

El server arranca en `127.0.0.1:3000`. Probalo con curl:

```bash
curl -X POST http://localhost:3000/users \
  -H "Content-Type: application/json" \
  -d '{"name": "Ada", "email": "ada@example.com"}'

curl http://localhost:3000/users
```

El ejemplo demuestra el flujo completo: bindings Python → fns
helper Fitz que llaman SQLAlchemy → handlers HTTP que serializan
las filas a JSON.

### 21.12 Limitaciones — lo que NO anda

Para mantener la honestidad del lenguaje:

- **GIL de Python**: las corutinas Python compiten por el GIL
  adentro del bridge. Para hot paths concurrentes, reescribir en
  Fitz nativo (sin interop) es mejor.
- **Numpy/pandas con C extensions**: funcionan si están instalados
  en el venv. Bundlearlos standalone es notoriamente difícil
  (deuda del ecosistema Python, no de Fitz).
- **Herencia desde clases Python**: no soportada. Un `type` Fitz
  no puede heredar de una clase Python (los modelos de objetos
  son distintos — Python tiene MRO, Fitz no tiene clases). La
  composición sí: un `type` puede tener un campo opaco que
  envuelve un objeto Python.
- **`asyncio.gather` con futures Fitz**: el marshaling Future Fitz
  → corutina Python no está soportado. Workaround: definir el
  `gather` adentro de un helper Python que toma corutinas.

Si necesitás algo de la lista de arriba y no aparece como deuda
abierta en el roadmap, abrí un issue.

---

> **Cierre Parte 9.** Interop Python te permite usar el
> ecosistema más grande del mundo desde Fitz sin renunciar a
> tipos estáticos ni a HTTP nativo. Con eso, Fitz es usable hoy
> para proyectos que ya viven en Python — el stack nativo (ORM,
> driver Postgres) llega como fase posterior. El último capítulo
> es el mapa hacia adelante.

---

## 22. Soporte para editores

Hasta acá la guía cubrió el lenguaje y sus herramientas de línea
de comando. Pero la experiencia diaria de escribir código pasa por
el editor: errores subrayados al tipear, autocompletar, navegación.
A partir de la **Fase 9.x.1** (cerrada el 2026-05-15), Fitz tiene
su propio **Language Server Protocol** (LSP) y una **extensión
VSCode** que lo aprovecha.

### Qué da hoy

- **Syntax highlighting** sobre archivos `.fitz` (grammar TextMate
  embebida en la extensión). Colorea keywords (`let`, `fn`, `if`,
  `match`, `async`/`await`, `not`/`and`/`or`...), tipos built-in
  (`Int`, `Float`, `Str`, `Bool`, `List`, `Map`, `Result`,
  `Future`...), tipos nominales (`User`, `Order`...), strings con
  interpolación — incluyendo multilínea `"""..."""` (cap 5) —, el
  `{name}` resaltado distinto del resto, números, decoradores
  (`@get`, `@server`, `@middleware`...), comentarios (`//` y
  `/* */`), constantes (`true`/`false`/`null`/`Ok`/`Err`),
  built-ins (`print`/`len`/`sleep`/`cors`/`assert`/`assert_eq`/
  `assert_ne`/`assert_throws`), labels de loops
  (`'outer` — mini-tanda L), operadores compuestos (`+=`/`-=`/...)
  y rangos inclusivos (`..=`).
- **Diagnostics en vivo** — los errores del lexer, parser y type
  checker aparecen subrayados en rojo al tipear, con el mismo
  mensaje + sugerencia que `fitz check` muestra en la terminal.
  Pipeline: tokenize → `parse_with_recovery` (parser tolerante a
  buffer en construcción, Fase 9.0 — F15) → `check_program` (Fase
  5a + F16). Severity `ERROR`, source `"fitz"` (visible en el
  Problems panel de VSCode).
- **Hover con tipos** — pasás el mouse sobre una variable o
  expresión y aparece su tipo en un tooltip (renderizado como
  bloque de código Fitz, con syntax highlighting). Funciona sobre
  literales (`42` → `Int`), identificadores en uso (`nombre` →
  `Str`), expresiones compuestas (`xs.map(...)` → `List<U>`),
  tipos nominales (`u` → `User`). Heurística pragmática: el último
  Expr iniciado antes del cursor en la misma línea. Cubre el 90%
  del caso; refinable con span completo cuando aterrice esa
  deuda del AST.
- **Go-to-definition** — F12 (o Ctrl+Click) sobre el uso de una
  variable, función o tipo te lleva a la línea de su declaración.
  Funciona sobre variables locales (`let x = ...`), funciones
  top-level (`fn foo(...)`), tipos custom (`type User { ... }`),
  parámetros de fn, variables del `for ... in`, bindings de
  `match` (`Ok(x)`, `Err(e)`, `Ident pat`), e imports
  (apuntando al `from foo import X` local — cross-module def
  remota es deuda visible). Los built-ins (`print`/`len`/
  `sleep`/`cors`) no resuelven (no hay archivo donde saltar).
- **Autocomplete contextual** — al tipear, VSCode te muestra una
  lista de sugerencias según el contexto:
  - Tras `.` (caso *after-dot*): si el receiver es un tipo custom
    (`u: User`), aparecen sus fields **y sus métodos custom**
    (mini-fase R.3, firma `fn(T1, T2) -> Ret` o `async fn(...)
    -> Ret` en el detail). Si es `List<T>`, sus 9 métodos built-in
    (`push`/`pop`/`map`/`filter`/`find`/`len`/
    `sort`/`reverse`/`contains`). Si es `Map<K, V>`, sus 5
    (`get`/`has`/`keys`/`values`/`len`). Si es `Str`, sus 10
    (`upper`/`lower`/`len`/`contains`/`starts_with`/`ends_with`/
    `split`/`trim`/`replace`/`repeat`). Si es un tuple (`(Int,
    Str, Bool)`), aparecen los índices `0`/`1`/`2` como campos
    con el tipo de cada elemento. Para otros tipos (`Any`, `PyAny`,
    primitivos) la lista queda vacía.
  - En cualquier otra posición (caso *scope-level*): aparecen las
    variables y funciones top-level del archivo, los tipos custom
    declarados, los símbolos importados, los builtins (`print`/
    `len`/`sleep`/`cors`), los tipos built-in (`Int`/`Float`/`Str`/
    `Bool`/`List`/`Map`/...), y los keywords del lenguaje (`let`/
    `fn`/`if`/`match`/...).
  El autocomplete no es scope-aware todavía: vars locales y params
  no aparecen en la lista scope-level, pero el usuario puede
  tipearlas igual sin que el LSP las marque como error.

### Lo que viene

El MVP del LSP está completo — las 5 sub-fases visibles (9.x.1
→ 9.x.5) están cerradas. Lo que sigue son refinamientos opcionales
post-MVP (sin sub-fase asignada todavía):

- **Publicación real al VSCode Marketplace** — la extensión está
  lista (`.vsix` per-plataforma generable), pero la publicación
  requiere acciones del autor (cuenta de publisher, decisión sobre
  hacer el repo público). Ver "Publicar al Marketplace" al final
  de este capítulo.
- **CI multi-platform** — GitHub Actions workflow que genera los
  `.vsix` de las 6 plataformas automáticamente en cada release.
- **Features LSP refinadas**: rename, refactoring, semantic
  highlighting, inlay hints, hover con docstrings, etc. Cuando
  aparezca demanda real.

### Cómo lo instalo

Hay dos modalidades:

#### A) Bundled (recomendado) — el `.vsix` trae el binario adentro

Un solo comando genera un `.vsix` per-plataforma con `fitz-lsp`
bundleado en `server/`. No necesitás configurar nada después de
instalar.

```bash
cd editors/vscode
npm install
npm run build:vsix
```

Esto produce `editors/vscode/fitz-language-X.Y.Z-<platform>-<arch>.vsix`
(ej. `fitz-language-0.9.6-win32-x64.vsix`, ~1.5 MB) y corre:

1. `cargo build --release --features lsp` (compila `fitz-lsp`).
2. Copia el binario a `editors/vscode/server/`.
3. `tsc` compila la extensión.
4. `vsce package --target <platform>` empaqueta todo.

Después lo instalás en VSCode:

```bash
code --install-extension editors/vscode/fitz-language-*.vsix --force
```

Abrí cualquier `.fitz` y deberías ver highlighting + diagnostics +
hover + go-to-def + autocomplete funcionando. Cero settings extra.

Para empaquetar para **otra plataforma** (cross-compile, requiere
`rustup target add <triple>` previo):

```bash
node scripts/build-vsix.mjs --target linux-x64
node scripts/build-vsix.mjs --target darwin-arm64
# Plataformas soportadas: win32-x64, win32-arm64, linux-x64,
# linux-arm64, darwin-x64, darwin-arm64
```

#### B) Manual (alfa / desarrollo) — `fitz-lsp` en PATH o setting

Si querés iterar sobre el LSP sin re-empaquetar cada vez:

```bash
# Build local
cargo build --release --features lsp

# Instalar global (opcional, deja `fitz-lsp` en PATH)
cargo install --path . --features lsp
```

Después instalás una versión "delgada" del `.vsix` (sin binario
bundleado):

```bash
cd editors/vscode
npm install && npm run compile
npx @vscode/vsce package  # sin --target, no bundlea
code --install-extension fitz-language-*.vsix
```

Si `fitz-lsp` no está en `PATH`, agregá esto al `settings.json` de
VSCode (Ctrl+, → "Open Settings (JSON)"):

```json
{
  "fitz.lspPath": "C:\\Users\\me\\fitz\\target\\release\\fitz-lsp.exe"
}
```

La extensión sigue una **cascada de resolución**:

1. Si setteás `fitz.lspPath` a algo distinto del default — lo
   respeta (override manual).
2. Si no, busca `fitz-lsp` bundleado en `server/` adentro del
   `.vsix` (modo bundled).
3. Como último fallback, busca `fitz-lsp` en el `PATH` del sistema.

Si algo falla, abrí el output panel ("View → Output → Fitz Language
Server") para ver qué dice.

### Publicar al Marketplace (autor)

La publicación real al [VSCode Marketplace](https://marketplace.visualstudio.com/)
es **acción del autor**, no del repo. Requiere:

1. **Cuenta de publisher**: Microsoft account + Azure DevOps
   organization. [Docs](https://code.visualstudio.com/api/working-with-extensions/publishing-extension).
2. **Personal Access Token** (PAT) con scope "Marketplace
   (manage)".
3. **Repo público**: pre-requisito para que el `repository` field
   del `package.json` sea válido + para el Social Preview.
4. **`vsce publish`** por cada plataforma:

```bash
vsce publish --packagePath editors/vscode/fitz-language-X.Y.Z-win32-x64.vsix
vsce publish --packagePath editors/vscode/fitz-language-X.Y.Z-linux-x64.vsix
vsce publish --packagePath editors/vscode/fitz-language-X.Y.Z-darwin-arm64.vsix
# ... una por target
```

Marketplace muestra solo la versión apropiada al cliente que
descarga.

### Settings

| Setting | Default | Para qué |
|---|---|---|
| `fitz.lspPath` | `"fitz-lsp"` | Path al binario. Default asume `PATH`. |
| `fitz.trace.server` | `"off"` | Debug del protocolo LSP en el output panel. `"verbose"` muestra payloads JSON-RPC completos — útil si la extensión actúa raro. |

### Otros editores

El protocolo LSP es estándar — cualquier editor con cliente LSP
puede usar `fitz-lsp`. La configuración varía por editor:

- **Neovim**: con `nvim-lspconfig`, agregás un setup que apunte a
  `fitz-lsp` para el filetype `fitz`.
- **Helix**: en `languages.toml`, definís `[[language]]` con
  `name = "fitz"` + `[[language.language-server]]` con
  `command = "fitz-lsp"`.
- **Zed**: el extension API permite definir un language server
  personalizado.

La extensión VSCode es la única que mantenemos hoy en este repo;
las demás integraciones quedan abiertas a contribuciones.

### Estado del proyecto LSP

**El plan LSP entero está cerrado**: diagnostics (9.x.1), hover
(9.x.2), go-to-definition (9.x.3), autocomplete contextual (9.x.4)
y distribución multi-platform (9.x.5). Las cinco sub-fases visibles
del LSP están vivas. La publicación real al Marketplace queda como
acción del autor (ver sección anterior).

Si encontrás bugs en el LSP o sugerencias para la grammar
TextMate (palabras que no se colorean, falsos positivos), abrí
un issue en [github.com/Thegreekman76/fitz](https://github.com/Thegreekman76/fitz).

---

## 23. `fitz fmt` — formateador automático

`fitz fmt` aplica un estilo canónico a tu código Fitz. Cero config:
no hay archivo `.fitzfmt`, no hay opciones de la CLI para indent o
comillas. La filosofía es la de **gofmt** — la uniformidad
cross-codebase vale más que la preferencia individual. Si discrepás
con una regla, abrí un issue; las reglas pueden ajustarse, pero
NO se configuran por proyecto.

Llegó con la Fase 9.z.1 (cerrada el 2026-05-16) y es
**production-ready**: preserva los comentarios y blank lines del
usuario, no solo el código.

### Qué hace

- **Pretty-printer sobre el AST**. Re-emite el código siguiendo el
  estilo canónico (4 espacios de indent, comillas dobles,
  `and`/`or`/`not` como keywords, paréntesis obligatorios en `if`/
  `while`, type defs siempre multi-línea, un field por línea).
- **Preserva comments** (`//` y `/* */`) en cualquier posición:
  top-level, entre stmts, trailing. Los comments `//foo` se
  normalizan a `// foo` (espacio post-`//`). Trailing comments
  tienen 2 espacios de separación: `let x = 1  // explicación`.
- **Preserva blank lines** del usuario, con dos reglas: máximo 1
  blank line consecutiva (las múltiples se colapsan), y blank
  obligatoria entre `fn` o `type` top-level consecutivos.
- **Idempotente**: aplicarlo dos veces produce el mismo resultado.
  Si `fitz fmt --check` reporta diffs después de un `fitz fmt`, es
  un bug — reportalo.

### Cómo se usa

```bash
# Formatear archivos explícitos in-place
fitz fmt src/main.fitz src/utils.fitz

# Formatear todo el proyecto (requiere fitz.toml)
fitz fmt

# Modo check para CI / pre-commit (read-only, exit 1 si hay diffs)
fitz fmt --check
fitz fmt --check src/main.fitz
```

Sin args, hace walk recursivo del proyecto excluyendo `target/` y
directorios ocultos — necesita un `fitz.toml` en el cwd o un
ancestro (manifest mode). Con archivos explícitos no exige
manifest.

Cada archivo que cambia reporta `✓ formateado <path>`; los que ya
estaban canónicos quedan silenciosos. Si un archivo tiene errores
de sintaxis, el formatter aborta para ese archivo con el error del
parser y sigue con los demás.

### El estilo canónico (resumen)

| Aspecto | Regla |
|---|---|
| Indent | 4 espacios, no tabs |
| Strings | Comillas dobles siempre (`"hola"`, nunca `'hola'`) |
| Operadores lógicos | `and`/`or`/`not` keywords |
| `if`/`while` | Paréntesis obligatorios en la condición |
| `for`/`loop` | Sin paréntesis |
| Type defs | Siempre multi-línea, un field por línea |
| Trailing commas | Solo en multi-línea (match arms, type fields) |
| Blank lines | Máximo 1 consecutiva. Obligatoria entre fn/type top-level |
| Comments `//` | Normalizados a `// texto` con espacio post-`//` |

La referencia completa, con los casos particulares (FnExpr inline,
struct lits, method chains, etc.), vive en
[docs/fmt-style.md](fmt-style.md).

### Ejemplo: antes y después

Input desformateado (`src/main.fitz`):

```fitz
let users=[{"id":1,"name":"Ada"},{"id":2,"name":"Bob"}]
fn greet(u){return "Hola, {u.name}!"}//inline
for user in users{print(greet(user))}
```

Después de `fitz fmt src/main.fitz`:

```fitz
let users = [{"id": 1, "name": "Ada"}, {"id": 2, "name": "Bob"}]

fn greet(u) {
    return "Hola, {u.name}!" // inline
}

for user in users {
    print(greet(user))
}
```

Re-aplicar `fitz fmt` sobre el output no produce más cambios
(idempotencia).

### Ejemplo: preservación de comments y blank lines

Input con comments en varias posiciones:

```fitz
// Lista inicial de usuarios

let users = [
    {"id": 1, "name": "Ada"},  // pionera
    {"id": 2, "name": "Bob"},
]

// Helper de saludo
fn greet(u) {
    // formato compacto
    return "Hola, {u.name}!"
}
```

Tras `fitz fmt`, el output respeta los 3 comments (top-level,
trailing, y dentro del body) y la blank line entre el `let` y la
`fn`. Comments `//foo` sin espacio se normalizan a `// foo`.

### Modo `--check` en CI

Para evitar drift de estilo en una codebase compartida, usalo en
pre-commit hook o pipeline:

```bash
# .git/hooks/pre-commit
fitz fmt --check || {
    echo "✗ código no formateado. Corré: fitz fmt"
    exit 1
}
```

El exit code es 0 si todo está canónico, 1 si algún archivo
difiere. Sin escribir nada.

### Limitaciones conocidas

- **No auto-wrappea líneas largas**: 100 chars es soft limit, no
  enforced. El user decide cuándo partir una línea.
- **Multi-líneas user-formateadas se colapsan**: si formateaste
  una lista o un method chain en varias líneas para legibilidad y
  entran en una sola, el formatter las inlinea. Deuda futura.
- **Comments adentro de expresiones** (`f(x, // foo`,`y)`): no
  soportados. Si aparecen, pueden quedar mal posicionados al
  re-formatear.
- **Comments entre el último stmt de un bloque y el `}`**: pueden
  terminar fuera del bloque al re-formatear. Caso raro.
- **Format-on-save desde el LSP**: no conectado todavía.
  `fmt::format_source` es library-able, así que el wiring desde el
  LSP es trivial — pendiente cuando aparezca demanda.

---

## 24. `fitz test` — testing built-in

Fitz trae **test runner integrado**: marcás una fn con `@test`, la
corrés con `fitz test`, y obtenés output estilo cargo con
ok/FAILED + exit code. Sin librerías, sin glue, sin elegir entre
3 frameworks. Llegó con la **Fase 9.z.2** (cerrada el 2026-05-17).

### Cuatro aserciones built-in

Disponibles globalmente, igual que `print` y `len`:

| Builtin | Qué hace | Falla si... |
|---|---|---|
| `assert(cond, msg?)` | Pasa si `cond` es `true` | `cond` es `false` (mensaje opcional al final) |
| `assert_eq(a, b)` | Pasa si `a == b` | distintos (output con `left:`/`right:` estilo cargo) |
| `assert_ne(a, b)` | Pasa si `a != b` | iguales |
| `assert_throws(fn)` | Pasa si el callback **tira** un error | el callback retornó normal |

La igualdad de `assert_eq`/`assert_ne` es **estructural recursiva**:
funciona sobre primitivos, `List`, `Map`, `Instance`, `Result`. Coerciona
`Int` ↔ `Float` (igual que el `==` del lenguaje).

### El decorator `@test`

```fitz
@test fn suma_funciona() {
    assert_eq(2 + 2, 4)
}
```

Tres reglas del MVP:

1. **Sin args, sin kwargs**: `@test fn foo() { ... }` (sin paréntesis
   después de `@test`). `@test() fn foo()` también parsea por
   simetría con otros decorators.
2. **La fn no recibe parámetros**: `@test fn foo() { ... }`. Si pasás
   params (fixtures), el evaluator aborta con mensaje claro. Las
   fixtures (`@before_all`, `@before_each`, etc.) son sub-paso futuro
   si aparece presión.
3. **Async OK**: `@test async fn carga() { let r = sleep(0).await }`.
   El runner detecta `is_async` y `await`-ea el `Future` resultante
   antes de reportar el test.

Fuera del runner (`fitz run`, `fitz build`), las `@test fn` son
**no-op silencioso**: no se ejecutan, no aparecen en el output.
Paralelo a `#[cfg(test)]` de Rust — el código de tests vive
junto al de producción, pero solo corre cuando lo pedís.

### `fitz test` — el sub-comando

Dos modos de uso:

#### Single-file

```bash
fitz test --file mis_tests.fitz
```

Carga el archivo, descubre sus `@test fn`, las corre serie, reporta.

#### Manifest mode (proyecto)

```bash
fitz test
```

Sin args, busca `fitz.toml` en el cwd o ancestros y descubre tests
automáticamente. Dos casos:

- **Proyecto con `tests/*.fitz`**: carga cada archivo top-level de
  `tests/` (no recursivo, ordenado alfabéticamente). Estos
  archivos típicamente importan la lib con
  `from <package-name> import X` — el package se auto-registra en
  el resolver de deps (similar a `use my_crate::*` de Rust).
- **Proyecto solo con `[lib]` (sin `tests/`)**: carga el `[lib].entry`
  directamente para descubrir `@test` inline. Útil para
  librerías pequeñas con tests pegados al código que prueban.

Si en un proyecto con tests integration el lib tiene `@test`
inline, esos tests se descubren solo si **al menos un**
`tests/*.fitz` importa la lib. El loader cachea por path
canonical, así que se ejecutan una sola vez aunque varios tests
los importen.

### Filtrado por substring

```bash
fitz test suma           # corre solo tests cuyo nombre contiene "suma"
fitz test --file t.fitz filtro
```

Cargo style: substring case-sensitive sobre el nombre del test
(sin el prefijo del archivo). Los tests filtered out aparecen en
el output como `running N tests (M filtered out)`.

### Output

Estilo cargo:

```text
running 3 tests
test src/lib.fitz::doble_funciona ... ok
test tests/math.fitz::doble_de_5 ... ok
test tests/math.fitz::doble_de_cero ... FAILED

failures:

---- tests/math.fitz::doble_de_cero stdout ----
Error — assert_eq falló:
  left:  0
  right: 1

failures:
    tests/math.fitz::doble_de_cero

test result: FAILED. 2 passed; 1 failed; finished in 0.00s
```

Características:

- **Prefijo del archivo**: en manifest mode, cada test aparece como
  `<file>::<nombre>` para localizar fallos rápido. En single-file
  mode, solo `<nombre>`.
- **Colores ANSI**: verde para `ok`, rojo para `FAILED`. Se autodetecta
  si stdout es un TTY (`std::io::IsTerminal`); cuando redirigís a
  archivo o pipe, se omiten los códigos de color.
- **Exit code**: 0 si todos pasan, 1 si al menos uno falla. Útil
  para CI.

### Async tests

Las fns async funcionan transparente. El runner detecta `is_async`,
invoca, y `await`-ea el `Future` resultante antes de reportar el
test como ok/FAILED.

```fitz
@test async fn la_pausa_pasa() {
    let r = sleep(100).await
    assert_eq(r, null)
}
```

Restricción: `assert_throws` con callback async no funciona en el
MVP — el callback `async fn` produce un `Future` suelto que no es
equivalente a "tirar". Para casos async, usá `match` directo
sobre el `Result` de tu fn.

### Estructura típica de un proyecto con tests

```text
mi-proyecto/
├── fitz.toml
├── src/
│   └── lib.fitz          # código de producción + opcional `@test` inline
└── tests/
    ├── math.fitz         # integration tests, importan la lib
    └── strings.fitz
```

`fitz.toml`:

```toml
[package]
name = "miproyecto"      # ← sin hyphens, usable como ident en Fitz
version = "0.1.0"
edition = "2026"

[lib]
entry = "src/lib.fitz"
```

`tests/math.fitz`:

```fitz
from miproyecto import doble

@test fn doble_de_5() {
    assert_eq(doble(5), 10)
}
```

> Nota sobre el nombre del paquete: el resolver de deps registra
> el `[lib].entry` bajo `package.name` para que los tests integration
> puedan importarlo con `from <pkg> import X`. Como Fitz no admite
> hyphens en identificadores, **el nombre del paquete debe ser
> usable como identifier** (`miproyecto`, no `mi-proyecto`).
> Refinamiento futuro si aparece presión.

### Lo que NO anda todavía

- **Fixtures** (`@before_all`, `@before_each`, `@after_all`,
  `@after_each`): post-MVP si aparece presión real.
- **`@bench` para benchmarks**: post-MVP.
- **Mocks/spies built-in**: NO — problema de ecosistema, no del
  lenguaje.
- **`assert_throws` con callback async**: rechazado en runtime;
  workaround con `match`.
- **Coverage**: sub-paso futuro complejo (requiere instrumentación).
- **Tests en paralelo**: el runner corre serie. La paralelización
  llega si los tiempos de la suite duelen.
- **Reporte de span del fallo**: los errores de `assert*` reportan
  el mensaje pero no la línea exacta de la aserción fallida.
  Refinamiento útil; necesita propagar el span del call site al
  builtin.

### Ejemplo ejecutable

`examples/guide/24-tests.fitz` tiene un mini-set de tests sobre una
función `factorial` que podés correr así:

```bash
fitz test --file examples/guide/24-tests.fitz
```

Tres tests pasan, uno falla intencional para que veas el formato
de FAILED + summary final.

---

## 25. `fitz dev` — hot reload

`fitz dev` mantiene tu programa corriendo y lo **re-arranca
automáticamente** cuando guardás un cambio. Pensado para el loop
del developer: editás, guardás, ves el efecto, repetís. Llegó
con la **Fase 9.z.3** (cerrada el 2026-05-17).

### Qué hace

- **File watcher** sobre el directorio del proyecto (manifest mode)
  o el directorio del archivo (single-file mode). Usa el backend
  nativo del SO via la crate [`notify`](https://crates.io/crates/notify):
  FSEvents en macOS, inotify en Linux,
  ReadDirectoryChangesW en Windows.
- **Kill + respawn** del proceso al detectar un cambio. Estrategia
  simple y correcta — incremental rebuild es deuda futura.
- **Debounce 100ms** para colapsar saves múltiples del editor
  (VSCode emite write tmp + rename + chmod en un save).
- **Banner** entre runs: clear screen (ANSI) + run number +
  target. Sin TTY (output redirigido), separa con líneas.
- **Ctrl+C** atrapado: mata el child antes de salir para evitar
  procesos zombie.

### Cómo se usa

```bash
# Single-file mode: watch el parent del archivo
fitz dev --file mi_script.fitz

# Manifest mode: watch el dir del fitz.toml, corre el [bin].main
fitz dev
```

### Qué dispara restart

Sólo archivos relevantes al lenguaje:

- `*.fitz` (cualquier archivo Fitz, en cualquier subdirectorio).
- `fitz.toml` (cambios al manifest).

Se **excluyen** automáticamente:

- `target/` (binarios y build artifacts de `fitz build`).
- `.git/` (historia del repo).
- `node_modules/` (lock para devs con tooling JS al lado).
- `.fitz/`, `dist/`, `build/` (carpetas de output convencionales).
- Cualquier carpeta o archivo oculto (empezado en `.`).

### Output típico

```text
🔄 fitz dev — watching /path/to/mi_proyecto
   ejecutando: proyecto `miapp`
   (Ctrl+C para salir)

▶ fitz dev (run #1) — proyecto `miapp`

Hola mundo
42

✓ programa terminó OK (exit 0) — esperando cambios ...

↻ cambio detectado en src/main.fitz — reiniciando ...

▶ fitz dev (run #2) — proyecto `miapp`

Hola mundo modificado
84

✓ programa terminó OK (exit 0) — esperando cambios ...
```

Para programas HTTP, el comportamiento es análogo: el child arranca
el servidor; al detectar cambio, lo mata + respawnea, así el nuevo
código toma efecto sin re-ejecutar curl o refrescar el browser por
varios segundos.

### Lo que NO anda todavía

- **Incremental rebuild** (cambiar 1 archivo y re-cargar sólo eso):
  hoy es kill+respawn full. Mejora cuando aparezca el modelo de
  módulos pre-compilados.
- **Browser auto-refresh para HTTP** (inyectar WebSocket en
  respuestas): NO en MVP. Quien edite HTML/CSS junto al backend
  Fitz puede usar herramientas separadas (Live Server, etc.).
- **`[dev]` section en `fitz.toml`** para configurar paths
  watched, debounce time, etc.: usamos defaults razonables. Sumar
  config si aparece demanda concreta.
- **Print de errors del checker mientras escribís** sin disparar
  restart: el child es quien imprime los errores de tipo en
  arranque. La mitad del valor del `fitz dev` es que el typecheck
  ya estaba bloqueando los errores antes (modo strict del
  `fitz run`). Para feedback continuo sin restart, el LSP del cap
  22 ya hace diagnostics in-editor.
- **Reload solo sobre cambios "significativos"** (filtrar saves
  sin cambios reales del contenido): hoy cualquier `Modify` del
  filesystem dispara. Si tu editor toca timestamps sin contenido
  real, vas a ver restarts spurios. Refinable si duele.

### Cómo encaja con `fitz test`

Si querés ejecutar tus tests cada save, hoy hacés:

```bash
# Terminal 1: corré el programa principal
fitz dev

# Terminal 2: corré los tests con un wrapper simple
while true; do fitz test; sleep 2; done
```

Un sub-comando `fitz dev --test` que watchee y corra `fitz test`
en lugar de `fitz run` queda como sub-paso futuro si aparece
presión.

---

## 26. `fitz repl` — REPL interactivo

`fitz repl` abre un prompt donde podés ingresar expresiones y
statements línea por línea, viendo el resultado de cada uno
inmediatamente. Es el patrón "Read-Eval-Print Loop" — el mismo
que vive en Python/Node/Ruby. Llegó con la **Fase 9.z.4**
(cerrada el 2026-05-17).

Útil para:
- Aprender el lenguaje: probar expresiones sin armar un archivo.
- Debuggear: cargar tu lib con `:load` y ejercer fns una por una.
- Experimentar: ver tipos de expresiones con `:type`, listar
  bindings con `:env`.

### Cómo arrancarlo

```bash
fitz repl
```

Aparece el prompt:

```text
Fitz REPL
Tipos: `:help` para comandos disponibles. Ctrl+D para salir.

fitz>
```

Cada línea se evalúa contra un **env compartido**: lo que declarás
en una línea sigue disponible en las siguientes.

### Expresiones vs statements

El REPL imprime el valor de la expresión cuando es una expresión
top-level (Python style, sin `print` explícito):

```text
fitz> 1 + 2
= 3
fitz> "hola, " + "fitz"
= "hola, fitz"
fitz> [1, 2, 3].map(fn(n) => n * 2)
= [2, 4, 6]
```

Los statements (`let`, `fn`, `import`, etc.) son silenciosos:

```text
fitz> let x = 5
fitz> fn doble(n: Int) -> Int { return n * 2 }
fitz> doble(x)
= 10
```

### Multi-line continuation

Si tu input tiene `{`/`(`/`[` sin cerrar, el prompt cambia a `... `
y espera más:

```text
fitz> fn factorial(n: Int) -> Int {
...       if (n <= 1) {
...           return 1
...       }
...       return n * factorial(n - 1)
...   }
fitz> factorial(5)
= 120
```

La detección es por balanced brackets — el parser real puede aún
emitir un error sintáctico distinto, que se muestra y volvés al
prompt.

### Async funciona

`async fn` y `.await` funcionan transparente en el prompt — el
REPL corre adentro de un runtime tokio:

```text
fitz> async fn lento(n: Int) -> Int {
...       let _ = sleep(10).await
...       return n * 10
...   }
fitz> lento(7).await
= 70
```

### Comandos especiales

Toda línea que empieza con `:` es un comando del REPL, no código
Fitz:

| Comando | Qué hace |
|---|---|
| `:help`, `:h` | Lista de comandos. |
| `:quit`, `:q`, `:exit` | Sale. También Ctrl+D. |
| `:env` | Lista los bindings que definiste en esta sesión. |
| `:reset` | Limpia el scope: perdés todas las vars/fns. |
| `:type <expr>` | Muestra el tipo de una expresión. |
| `:load <archivo.fitz>` | Evalúa un archivo en el scope actual. |

Ejemplo de sesión usando casi todos:

```text
fitz> let users = [{"id": 1, "name": "Ada"}, {"id": 2, "name": "Bob"}]
fitz> users.len()
= 2
fitz> :type users
:: Map<Any, Any>... (deuda: ver "Lo que NO anda" más abajo)
fitz> :env
Definido en el scope:
  users = [{"id": 1, "name": "Ada"}, ...]  // List
fitz> :reset
✓ scope reseteado
fitz> :env
(scope vacío — no definiste nada todavía)
fitz> :quit
👋 hasta luego!
```

### History persistente

Las líneas que tipeás se guardan en `~/.fitz/history` (o
`%USERPROFILE%\.fitz\history` en Windows). En sesiones futuras:

- **Flecha ↑/↓**: navegar las líneas anteriores.
- **Ctrl+R**: buscar incrementalmente en la history.
- **Home/End/Ctrl+A/Ctrl+E**: mover el cursor.
- **Ctrl+C**: cancela el buffer actual (sale del multi-line si
  estaba abierto), volvés al prompt.
- **Ctrl+D**: sale del REPL.

(Todo esto lo aporta `rustyline`, el mismo crate que usa
`cargo-edit` y otros REPLs Rust.)

### Lo que NO anda todavía

- **`:type` scope-aware**: hoy `:type x` con `let x = 5` previo
  devuelve `Any`. El comando arma un programa sintético
  independiente y el checker no ve las vars previas del REPL.
  Refinable feedeando el env al checker — sub-paso futuro si
  aparece presión real.
- **`:load` con paths relativos**: usá paths absolutos o relativos
  al directorio donde arrancaste `fitz repl`. No hay autocompletion
  de paths.
- **Manifest mode**: `fitz repl` sin args es siempre single-file
  (sin manifest). Si querés cargar tu proyecto, usá `:load
  src/lib.fitz`.
- **Pretty-print por defecto**: las instancias se imprimen estilo
  Display (que es el de `print`). Para JSON formateado, llamá
  manualmente a tu helper.
- **Indentación automática en multi-line**: el prompt `... ` no
  ajusta indent. Lo tipeás vos.
- **Comandos `:save <archivo>` (volcar la sesión a un .fitz),
  `:undo` (deshacer última línea), `:debug` (modo verbose)**:
  sub-pasos futuros si entra demanda.

### Cuándo usar `fitz repl` vs `fitz run`

| Caso | Usá |
|---|---|
| Probar una expresión sin armar archivo | `fitz repl` |
| Aprender el lenguaje, experimentar | `fitz repl` |
| Debuggear una fn aislada con varios inputs | `fitz repl` + `:load` |
| Correr tu programa principal | `fitz run` |
| Loop de desarrollo con auto-restart al guardar | `fitz dev` |
| Correr los tests | `fitz test` |

---

## 27. `fitz lint` — linter de patrones

`fitz lint` detecta patrones que **sí compilan pero son code smells**:
vars no usadas, imports muertos, `match` con un solo arm catch-all,
concatenación de strings con `+` en lugar de interpolación. El
linter complementa al type checker:

- `fitz check` — errores de tipo (bloqueantes, exit 1).
- `fitz lint` — sugerencias de estilo/patrón (warnings, exit 0 por
  default). Promovés a error con `--deny <lint>` en CI.

Llegó con la **Fase 9.z.5** (cerrada el 2026-05-17). **Cierra
Fase 9.z entera**.

### Los 4 lints del MVP

| Lint | Qué detecta |
|---|---|
| `unused_variable` | `let x = ...` cuyo nombre no aparece en ningún uso (skip prefijo `_`). |
| `unused_import` | `import X` o `from X import Y` cuyo binding no se usa. |
| `useless_match` | `match expr { _ => body }` con un solo arm catch-all (= un `let` directo). |
| `string_concat` | `"a" + "b"` con ambos operandos literales (usá interpolación). |

### Cómo se usa

```bash
# Lintear archivos explícitos
fitz lint src/main.fitz src/utils.fitz

# Lintear todo el proyecto (requiere fitz.toml)
fitz lint

# Tratar un lint como error (CI)
fitz lint --deny unused_variable
fitz lint --deny unused_variable --deny string_concat   # múltiples
```

**Default**: warnings, exit 0 incluso con findings. Solo el flag
`--deny <name>` con un lint que aparezca convierte el exit a 1.

### Supresión: `// @allow(<lint>)`

Si un lint es intencional, prefijás la línea anterior con un
comment `@allow(<name>)`:

```fitz
// @allow(unused_variable)
let placeholder = compute_thing()  // intencional, no flagueado
```

Solo la línea inmediatamente anterior cuenta. El comment puede
tener texto adicional (`// @allow(unused_variable) — pending fix`).

### Output

Estilo cargo-clippy:

```text
warning: variable `temp` declarada pero no usada [unused_variable]
  --> src/main.fitz:7:5
  = nota: si es intencional, prefijá con `_` (ej. `_temp`) o suprimí con
         `// @allow(unused_variable)` en la línea anterior.

warning: concatenación de strings literales — usá interpolación [string_concat]
  --> src/main.fitz:4:24
  = nota: reemplazá `"a" + "b"` con `"ab"` (o usá interpolación
         `"{a}{b}"` si los lados son variables).

2 findings en 1 archivo(s)
```

Colores ANSI auto cuando stdout es TTY: amarillo para `warning`,
rojo para `error` (modo `--deny`).

### Lo que NO anda todavía

- **Auto-fix (`--fix`)**: el roadmap lo menciona como flag opcional.
  En el MVP, todos los lints emiten sugerencias textuales pero no
  modifican código. `string_concat` es el candidato natural a
  auto-fix (sub-paso futuro).
- **Lints adicionales**: `redundant_clone` necesita análisis de
  movimientos que el compilador todavía no hace. `panic_in_test_only`
  no aplica (Fitz no tiene un `panic!` distinguido — los asserts
  son builtins normales).
- **Catálogo extensible (plugins)**: por ahora los 4 lints viven
  hardcoded en `src/lint.rs`. Plugins externos no son scope del MVP.
- **Sub-stmt suppression**: el `// @allow(<name>)` solo afecta la
  línea siguiente. No hay `// @allow(name) { ... }` para suprimir
  un bloque entero.
- **Análisis cross-scope estricto**: `unused_variable` usa
  un set global de uses — no detecta shadowing (`let x = 5; let x
  = 10; x` no reporta el primer `x` como unused aunque
  técnicamente sí es). Refinamiento si aparece presión.

### Integración con CI

Patrón típico en pre-commit o pipeline:

```bash
fitz fmt --check          # exit 1 si hay diffs
fitz check                # exit 1 si hay errores de tipo
fitz lint --deny unused_variable --deny unused_import   # exit 1 si los hay
fitz test                 # exit 1 si algún test falla
```

Solo `fitz lint` permite ser laxo por default (warnings sin
romper). El resto siempre exige cero issues.

---

## 28. Qué sigue

Si llegaste hasta acá: gracias. Esta es una versión temprana de la
guía y vos sos parte muy temprana del proyecto.

### Lo que ya sabés

Con los capítulos 1 a 27 podés:

- Escribir y correr programas que combinan **variables, aritmética y
  strings** con interpolación.
- Controlar el flujo con **`if` / `else if` / `else`**, **`while`**,
  **`loop`** y **`for ... in`**, y elegir entre alternativas con
  **`match`** sobre literales y rangos.
- Agrupar datos en **listas**, **mapas** y **rangos**, accederlos por
  índice o clave, recorrerlos e iterarlos.
- Definir **funciones** con su forma de bloque y su forma flecha,
  hacer **recursión** y crear **closures** con captura léxica.
- Declarar **tipos custom** con `type`, **instanciarlos**
  (`User { id: 1, name: "x" }`), acceder a sus campos
  (`user.name`), **mutarlos** (`user.name = "Otro"`), con defaults
  y campos nullables.
- Llamar **métodos** sobre listas (`xs.push`, `xs.map`,
  `xs.filter`, `xs.find`), mapas (`m.get`, `m.has`, `m.keys`,
  `m.values`), y strings (`s.upper`, `s.lower`, `s.len`), usando
  **funciones anónimas inline** (`fn(n) => n * 2`) como callbacks.
- Manejar errores con **`Result`**, **`Ok`**, **`Err`** y el
  operador **`?`** para propagar, sin excepciones.
- Partir el código en **módulos** con `import foo` y
  `from foo import a, b`.
- Escribir **APIs HTTP** con `@get`/`@post`/`@put`/`@delete`,
  path params tipados, body deserializado contra `type`,
  serialización JSON automática, headers con
  `@header(name="X")`, y `@server(port, host)` para configurar.
- Obtener **OpenAPI 3.1 + UI Scalar gratis** en `/openapi.json`
  y `/docs`, generados desde los decoradores. Opt-out con
  `@server(docs=false)`. Schema idéntico bit-a-bit entre
  `fitz run` y `fitz build` (Fase 7).
- Leer un mensaje de error del intérprete y ubicar de qué fase vino.
- Validar tipos en compile time con **`fitz check`**, y dejar que
  `fitz run` aborte en modo strict cuando encuentra errores (Fase
  5a cerró el type checker estático).
- **Compilar a binario nativo** con `fitz build`: programa CLI
  o server HTTP que corre sin Fitz instalado en la máquina
  destino (Fase 5b — codegen via transpile-a-Rust + Cargo).
- **Llamar código Python** con `from python import X`:
  importar módulos (`math`, `json`, `sqlalchemy`...), invocar
  funciones con marshaling automático de tipos compuestos (List
  → list, Map → dict, Instance → dict), manejar errores con
  `Result<T>` (excepciones Python → `Err`), recuperar tipos Fitz
  desde Python con anotaciones (`let row: User = py_call(...)?`),
  auto-generar `type` Fitz desde SQLAlchemy con `fitz py-types`,
  y `await` corutinas Python via bridge tokio ↔ asyncio (Fase 8).
- **Tooling de editor**: extensión VSCode con highlighting +
  diagnostics + hover + go-to-definition + autocomplete contextual
  via LSP (Fase 9.x.1 → 9.x.5, MVP completo + distribución
  multi-platform). Errores subrayados al tipear, mouse sobre una
  expresión muestra su tipo, F12 te lleva a su declaración, tras
  `.` aparecen métodos del tipo. `.vsix` per-plataforma con el
  binario bundleado adentro vía `npm run build:vsix`. Sin ir a
  la terminal. Ver [cap 22](#22-soporte-para-editores) para cómo
  instalar.
- **Formato canónico** con `fitz fmt` (Fase 9.z.1): cero config,
  estilo gofmt, preserva comments y blank lines del usuario.
  Modo `--check` para CI / pre-commit hooks. Ver
  [cap 23](#23-fitz-fmt--formateador-automático).
- **Tests built-in** con `@test` + `fitz test` (Fase 9.z.2):
  decorator `@test` sobre fns sin args, 4 assertion builtins
  (`assert`, `assert_eq`, `assert_ne`, `assert_throws`), runner
  con output estilo cargo (ok/FAILED + summary + exit code),
  filtrado por substring, async tests, discovery automático en
  manifest mode (`tests/*.fitz` + `[lib]` integration). Cero
  librerías. Ver [cap 24](#24-fitz-test--testing-built-in).
- **Hot reload** con `fitz dev` (Fase 9.z.3): file watcher sobre
  el proyecto + kill/respawn del child al detectar cambio en
  `.fitz` o `fitz.toml`. Debounce 100ms, exclusión de
  `target/`/`.git/`/`node_modules/`, banner ANSI entre runs,
  Ctrl+C atrapa sin dejar zombies. Ver
  [cap 25](#25-fitz-dev--hot-reload).
- **REPL interactivo** con `fitz repl` (Fase 9.z.4): prompt
  `fitz> ` con env compartido entre líneas, multi-line
  automático (`... `), pretty-print Python-style del último
  valor, 5 comandos especiales (`:help`/`:env`/`:type`/`:reset`/
  `:load`/`:quit`), history persistente en `~/.fitz/history` con
  arrow up/down + Ctrl+R, async transparente (`sleep(100).await`
  funciona). Ver [cap 26](#26-fitz-repl--repl-interactivo).
- **Linter** con `fitz lint` (Fase 9.z.5): 4 lints —
  `unused_variable`, `unused_import`, `useless_match`,
  `string_concat`. Default warning + exit 0; `--deny <lint>`
  promueve a error + exit 1 para CI. Supresión con
  `// @allow(<lint>)` en la línea anterior. Output estilo
  cargo-clippy con colores ANSI auto. Ver
  [cap 27](#27-fitz-lint--linter-de-patrones).

Es decir: todo lo que el intérprete de Fitz hoy ejecuta end-to-end,
con un chequeo estático que atrapa errores antes de que se
ejecuten, un compilador que produce binarios standalone, y un
puente al ecosistema Python para usar SQLAlchemy/numpy/asyncpg
sin abandonar Fitz.

### Lo que viene — más allá del LSP MVP (9.x.1 → 9.x.5 cerradas)

Las fases cerradas (al cierre de Fase 8): type checker estático
(5a), codegen a binario nativo (5b), async nativo (6), DX HTTP
con OpenAPI 3.1 + UI Scalar (7), middleware + CORS (mini-fase MW),
Send completo + paralelismo HTTP real (F17), e **interop Python**
end-to-end (Fase 8): embedding básico (8.1), marshaling compuesto
(8.2), excepciones → `Result<T>` (8.3), tipos del checker + coerción
runtime (8.4), `fitz py-types` SQLAlchemy (8.5), bridge tokio ↔
asyncio (8.6), codegen interop en `fitz build` (8.7), guía + ejemplo
CRUD + cierre formal (8.8 — este capítulo).

Con eso, la promesa "Fitz usable para proyectos reales hoy" queda
cumplida: HTTP nativo + tipos + interop con el ecosistema Python.

Lo que sigue post-8:

- **Fase 9 — Ecosistema** (en curso):
  - **Plan LSP entero cerrado** — diagnostics (9.x.1), hover
    (9.x.2), go-to-definition (9.x.3), autocomplete contextual
    (9.x.4) y distribución multi-platform (9.x.5). Ver cap 22.
  - **Package manager (9.y)**: `fitz new`/`init`, manifest
    `fitz.toml`, deps path + git con lockfile `fitz.lock`,
    sub-comandos `fitz add`/`remove`/`update` (9.y.1 → 9.y.4
    cerrados). Registry público (9.y.5) diferido — path + git
    cubren el 90% del caso real. **Capítulo dedicado en la guía
    pendiente** (deuda explícita en `docs/deudas-post-5b.md`,
    entra cuando 9.z entera cierre).
  - **DX (9.z) — CERRADA ENTERA**: formatter `fitz fmt` (9.z.1,
    cap 23), test runner `fitz test` (9.z.2, cap 24), hot
    reload `fitz dev` (9.z.3, cap 25), REPL interactivo
    `fitz repl` (9.z.4, cap 26), linter `fitz lint` (9.z.5,
    cap 27). Los 5 sub-pasos cerrados en 2 días (2026-05-16/17).
- **Sub-paso futuro separado: bundling CPython embebido** —
  `fitz build --bundle-python` produce un binario standalone que
  NO requiere Python en el destino. Decisión de herramienta
  pendiente (python-build-standalone vs PyOxidizer). Sin presión
  real hoy.
- **Stack DB nativo** (Fase 10+): driver Postgres en Fitz puro,
  ORM nativo declarativo sobre `type` (estilo Diesel/sqlx),
  migraciones, pool. La interop Python de Fase 8 es el puente
  hasta llegar ahí, no el destino final.
- **Deuda residual comprometida**:
  - **Coerción Python list/dict → Fitz `List<T>`/`Map<K,V>`/
    `Instance` en `fitz build`** (deuda de 8.7): los helpers
    `__fitz_py_to_list_*` ya están emitidos en el preludio, falta
    el wiring en `coerce(PyAny → List<T>)` y equivalentes. En el
    intérprete ya funciona (Fase 8.4).
  - **`.await` con binding intermedio** (`let fut = py_call()?;
    fut.await`): hoy solo el patrón `<py_call>?.await` inmediato.
  - **Inferencia de tipos de params y returns** en fns sin anotar
    — `fn greet(name)` corre en el intérprete pero `fitz build`
    exige anotación.
  - **Listas/mapas heterogéneos compilados** (`[1, "dos"]`): el
    intérprete los acepta, el compilador necesita un `FitzValue`
    tagged en runtime.
  - **Deuda menor de F7** (post-fase): doc-strings sobre handlers
    (para descripciones OpenAPI), `@header(into=...)` con alias del
    param Fitz, bundle Scalar embebido offline.

Ver [docs/roadmap.md](roadmap.md) para el detalle completo y la
deuda explícita acumulada por fase.

### Cómo va a crecer esta guía

La regla es estricta: **un capítulo nuevo solo cuando lo que cubre
funciona end-to-end en el intérprete**. Si una feature está a
medias, no entra todavía. Mejor decir "no se puede" que prometer algo
que va a romper a quien lo lea.

Cada vez que se cierre un grupo de features (típicamente al cerrar
una sub-fase del roadmap), la guía gana un capítulo o varios.

### Recursos

- [README.md](../README.md) — presentación pública del proyecto.
- [docs/vision.md](vision.md) — el "por qué" y para quién.
- [docs/syntax-spec.md](syntax-spec.md) — especificación completa de
  sintaxis. Incluye cosas que todavía no funcionan; tomalo como
  dirección, no como contrato.
- [docs/roadmap.md](roadmap.md) — fases de desarrollo con el estado
  actual.
- [docs/design-decisions.md](design-decisions.md) — por qué Fitz toma
  ciertas decisiones (sin excepciones, HTTP nativo, etc.).
- [docs/references.md](references.md) — qué inspira a Fitz y de dónde
  sacar más contexto.

### Reportar y contribuir

Si encontrás:

- Un ejemplo de la guía que no corre → es un bug. Reportalo.
- Un mensaje de error confuso → vale el reporte; mejorar mensajes es
  parte del trabajo de cada fase.
- Una sección poco clara → contame qué te confundió. La guía es
  joven, todo feedback ayuda.

El repo principal está en GitHub:
[github.com/Thegreekman76/fitz](https://github.com/Thegreekman76/fitz).
Si querés contribuir con código, la mejor manera hoy es revisar el
[roadmap](roadmap.md), elegir algo pendiente y abrir un issue
proponiéndolo antes de empezar.

---

Eso es todo por esta primera versión. Si Fitz te resultó interesante
y querés mantenerte cerca del proyecto, mirá los commits del repo:
cada feature nueva trae sus tests y, cuando aplique, una actualización
de esta guía.

Nos vemos en el próximo capítulo. 🏔️
