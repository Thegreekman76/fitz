# Guía de Fitz

> Estado: viva — cubre solo lo que el intérprete ejecuta hoy.
> Última actualización: 2026-05-11 (Fase 2 cerrada, 270 tests pasando).

Esta guía es para developers que vienen de Python, TypeScript, Vue o
similares y quieren aprender Fitz escribiendo programas reales. Está
pensada para leerse de arriba a abajo: cada capítulo asume lo del
anterior.

Lo que ves acá funciona hoy contra el binario del repo. Si un ejemplo
no corre, es un bug de la guía o del intérprete — abrí un issue.

---

## Índice

**Parte 1 — Empezando**
1. [Bienvenida](#1-bienvenida)
2. [Tu primer programa](#2-tu-primer-programa)

**Parte 2 — Datos y expresiones**
3. [Variables y tipos primitivos](#3-variables-y-tipos-primitivos)
4. [Operadores](#4-operadores)
5. [Strings](#5-strings)

**Parte 3 — Control de flujo**
6. [Booleanos y lógica](#6-booleanos-y-lógica)
7. [if / else](#7-if--else)
8. [Loops](#8-loops)
9. [Match](#9-match)

**Parte 4 — Abstracción**
10. [Funciones](#10-funciones)

**Parte 5 — Lo que está por venir**
11. [Tipos con `type` (preview)](#11-tipos-con-type-preview)
12. [Errores y mensajes](#12-errores-y-mensajes)
13. [Qué sigue](#13-qué-sigue)

---

## 1. Bienvenida

Fitz es un lenguaje nuevo, pensado para gente que ama la ergonomía de
Python y TypeScript pero se cansó de la lentitud del primero y del
bagaje histórico del segundo. Algunas ideas centrales:

- **Sintaxis liviana**, inspirada en Python y TypeScript. Punto y coma
  opcional, llaves para los bloques, indentación libre.
- **Tipado gradual**: las anotaciones de tipo son opcionales. Hoy se
  parsean y todavía no se chequean — el chequeo estático llega más
  adelante en el roadmap.
- **HTTP como ciudadano de primera clase** — `@get`, `@post`, etc. son
  parte del lenguaje, no de una librería. (Esto vive en Fase 4. Hoy
  no funciona aún.)
- **Sin excepciones**: los errores se manejan con `Result` y `match`,
  estilo Rust. (También Fase 3 en adelante.)
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
- `match` con patrones literales, binding por identificador y `_`.
- Funciones (`fn` en bloque y `=>` en flecha), closures, recursión.
- Declaración de tipos con `type` (declarar sí, instanciar todavía no).
- El builtin `print`.

### Qué todavía no anda

Estas cosas aparecen en la [especificación de sintaxis](syntax-spec.md)
pero el intérprete aún no las ejecuta. Si las tipeás vas a ver un
error explícito:

- `for` y rangos (`0..10`).
- Listas (`[1, 2, 3]`), mapas (`{"a": 1}`), tuplas.
- Instanciación de tipos custom (`User { id: 1, name: "x" }`).
- Acceso a campos y métodos (`user.name`, `users.push(...)`).
- `Result`, `Ok(x)`, `Err(e)`, propagación con `?`.
- `async` / `await`.
- Decoradores HTTP (`@get`, `@post`, …).
- `import` / `from ... import`.

Todo eso está mapeado en el [roadmap](roadmap.md). Cada vez que una de
estas piezas se cierra, esta guía suma el capítulo correspondiente.

### Cómo está organizada

La guía está dividida en cinco partes que se leen en orden:

1. **Empezando** — qué es Fitz y cómo correr tu primer programa.
2. **Datos y expresiones** — los tipos básicos y cómo se combinan.
3. **Control de flujo** — decidir y repetir.
4. **Abstracción** — funciones, closures, recursión.
5. **Lo que está por venir** — `type` como anticipo de la Fase 3,
   cómo leer errores, y a dónde mirar para seguir.

### Cómo usar los ejemplos

Cada ejemplo de la guía vive como archivo en `examples/guide/` y se
ejecuta así:

```bash
cargo run -- run examples/guide/01-hola.fitz
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
  instalá con [rustup](https://rustup.rs).
- **Git**, para clonar el repo.

No hace falta nada más. No hay package manager de Fitz todavía (eso es
Fase 6), así que el "intérprete" es directamente el ejecutable del
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

Vamos a escribir un programa propio. Creá [examples/guide/01-hola.fitz](../examples/guide/01-hola.fitz)
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
cargo run -- run examples/guide/01-hola.fitz
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
Error leyendo examples/guide/01-hola.fitz: ...
```

Revisá la ruta. Cargo corre con la raíz del proyecto como working
directory, así que las rutas son relativas a la carpeta `fitz/`.

Si el archivo está pero hay un error de sintaxis, el intérprete corta
con línea y columna del problema. Vamos a aprender a leer esos
mensajes en el capítulo 12.

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

Por ahora Fitz tiene cinco tipos básicos. Todo lo demás (listas,
mapas, tipos custom instanciados) llega en Fase 3.

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

**Aviso importante**: hoy estas anotaciones se parsean pero el
intérprete no las chequea. Esto compila y corre sin error, aunque el
valor no coincide con el tipo declarado:

```fitz
x: Int = "no soy int"   // se acepta sin chequear, x queda como Str
```

El plan a largo plazo es el [tipado gradual](roadmap.md): las
anotaciones son opcionales, y cuando estén, el type checker (Fase 5)
las valida en compile time. Mientras tanto, las podés usar como
documentación o costumbre, pero no esperes seguridad de tipos. El
runtime sí va a fallar si más adelante usás un valor de manera
incompatible (por ejemplo, sumando un `Str` con un `Int`).

Limitación adicional: la anotación hoy es solo un **nombre simple**
(`Int`, `Str`, `MyType`). Formas como `List<Int>`, `Map<Str, Int>` o
`Str?` aparecen en la especificación pero el parser todavía no las
soporta como anotación de variable.

### Reasignación

Asignar de nuevo al mismo nombre simplemente cambia el valor:

```fitz
count = 42
count = count + 1
print(count)   // 43
```

Y como las anotaciones no se chequean, el tipo del valor también
puede cambiar (no es algo que recomiende — es solo una consecuencia
del estado actual del lenguaje):

```fitz
n = 42
n = "ahora soy texto"   // funciona hoy, no funcionará cuando el type checker llegue
```

### Ámbito (scope)

Una variable existe en el bloque donde se define y en los anidados,
hasta donde se cierra ese bloque. Por ahora **los bloques de `if`,
`match` y `while` no crean su propio scope**: una variable definida
adentro persiste afuera. Es un comportamiento estilo Python, no estilo
Rust. Las funciones sí crean su propio scope (cap. 10).

Esto puede sorprender — lo dejamos marcado y, si en algún momento trae
problemas reales, lo reconsideramos.

### Ejemplo completo

[examples/guide/02-variables.fitz](../examples/guide/02-variables.fitz):

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

### Lo que todavía no anda

- Módulo (`%`) y resto.
- Operadores compuestos (`+=`, `-=`, `*=`, `/=`).
- Operadores de bits (`&`, `|`, `^`, `<<`, `>>`).

Estos están planeados pero el lexer no los tokeniza todavía.

### Ejemplo completo

[examples/guide/03-operadores.fitz](../examples/guide/03-operadores.fitz):

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

### Lo que todavía no anda

- Strings multilínea con `"""..."""`.
- Comillas simples como alternativa a las dobles.
- Métodos sobre strings (`.upper()`, `.split(...)`, `.len()`, etc.) —
  llegan junto con method calls en Fase 3.
- Format specifiers dentro de la interpolación (`{ratio:.2f}` y
  similares).

### Ejemplo completo

[examples/guide/04-strings.fitz](../examples/guide/04-strings.fitz):

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

### Lo que todavía no anda

- **`not`** — la negación lógica no está implementada. El lexer la
  trata como un identificador común, así que `not true` no parsea.

  Mientras llega, podés negar de dos formas:

  ```fitz
  // Comparando contra el opuesto:
  active = false
  if active == false {
      print("inactivo")
  }

  // O, mejor, invirtiendo la comparación cuando se puede:
  age = 10
  if age >= 18 {
      print("mayor")
  }
  // ...es más claro que `if not (age < 18)`.
  ```

- **XOR lógico** — no hay `xor`. Si lo necesitás puntualmente,
  `a != b` sobre dos `Bool` te da el mismo resultado.

### Ejemplo completo

[examples/guide/05-logica.fitz](../examples/guide/05-logica.fitz):

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

[examples/guide/06-if.fitz](../examples/guide/06-if.fitz):

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

Para repetir código en Fitz hoy tenés dos construcciones: `while` y
`loop`. El clásico `for ... in` está reservado en la sintaxis pero
todavía no se puede usar — vamos a ver por qué al final del capítulo.

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

(Un poco verboso — los labels llegarán cuando hagan falta de verdad.)

### Lo que todavía no anda

- **`for ... in`** está reservado pero no implementado. Si lo
  escribís, el parser corta:

  ```
  Error — `for` requiere rangos o listas para iterar, que llegan en Fase 3
  ```

  La razón es que `for` necesita algo sobre lo que iterar — rangos
  (`0..10`), listas, etc. — y todavía no hay listas ni rangos como
  valores de runtime. Llegan en Fase 3 junto con `List<T>`.

- **`loop` como expresión** — en Rust podés escribir
  `let x = loop { break valor }`. Acá `loop` es solo una sentencia;
  `break` no lleva valor.

- **Labels para `break` / `continue`** — para romper más de un nivel.

### Mientras tanto, contadores

Mientras `for` no esté disponible, el patrón usual para iterar entre
dos números es:

```fitz
i = 0
while i < 10 {
    print(i)
    i = i + 1
}
```

Es feo y propenso a olvidar el incremento, pero hace lo que tiene que
hacer.

### Ejemplo completo

[examples/guide/07-loops.fitz](../examples/guide/07-loops.fitz):

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

En el próximo capítulo vamos a `match`: patrones literales, binding
por identificador y `_`. Y por qué `match` sobre un valor cubre
muchos casos donde uno haría una cadena de `else if`.

---

## 9. Match

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

Hoy la exhaustividad no se chequea en compile time, así que es tu
responsabilidad incluir un wildcard (`_`) o un binding final. La
regla práctica: si el conjunto de valores posibles no está acotado
(strings, enteros), terminá siempre con `_`.

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

### Lo que todavía no anda

- **Rangos como patrón** (`0..12`, `13..17`, `18..`) — no parsean
  todavía. La especificación los menciona, llegan junto con rangos
  como valor en Fase 3.
- **`Ok(x)` / `Err(e)`** — los patrones parsean, pero el evaluador
  corta con:

  ```
  Error — patrones `Ok(...)` / `Err(...)` requieren el tipo Result (Fase 3)
  ```

  Vienen con el manejo de errores en Fase 3.
- **Tuples y listas como patrón** — `(a, b)`, `[head, ...rest]`,
  etc. Cuando lleguen los tipos compuestos.
- **Guards** (`patrón if condición => ...`).
- **Or-patterns** (`1 | 2 | 3 => ...`).
- **Exhaustividad chequeada en compile time** — llega con el type
  checker en Fase 5.

### Ejemplo completo

[examples/guide/08-match.fitz](../examples/guide/08-match.fitz):

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

## 10. Funciones

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

Esto te abre la puerta a estilos de orden superior — el día que
existan listas, podrás usar `map`, `filter`, etc. Por ahora alcanza
para callbacks.

### Lo que todavía no anda

- **Funciones anónimas inline** (`fn(x) => x * 2` como expresión
  directa, sin ponerle nombre). Hoy el parser no las admite. Si
  necesitás una función "anónima", definila con `fn` y pasala por su
  nombre.
- **Parámetros con default** (`fn greet(name = "amigo") { ... }`).
- **Varargs** (`fn sum(...xs)`).
- **Argumentos nombrados al llamar** (`greet(name: "Fitz")`).
- **Method calls** (`obj.method(args)`). Cuando lleguen los tipos
  custom instanciados en Fase 3.

### Ejemplo completo

[examples/guide/09-funciones.fitz](../examples/guide/09-funciones.fitz):

```fitz
fn greet(name) {
    return "Hola, {name}!"
}
print(greet("Fitz"))

fn double(n) => n * 2
print(double(21))

fn add(a: Int, b: Int) -> Int {
    return a + b
}
print(add(2, 3))

fn nothing() {
    let x = 1
}
print(nothing())

fn fact(n) {
    if n <= 1 {
        return 1
    }
    return n * fact(n - 1)
}
print(fact(5))

fn make_adder(x) {
    fn add(y) => x + y
    return add
}
add5 = make_adder(5)
print(add5(3))

fn apply(f, x) => f(x)
fn square(n) => n * n
print(apply(square, 7))
```

Salida:

```
Hola, Fitz!
42
5
null
120
8
49
```

---

Con funciones ya tenés todo lo necesario para escribir programas
completos. En el próximo capítulo entramos a la última parte de la
guía: una mirada a `type`, que hoy declarás pero todavía no podés
instanciar — y por qué.

---

## 11. Tipos con `type` (preview)

Este capítulo es un anticipo. La declaración de tipos custom ya está
en el lenguaje: el lexer, el parser y el evaluador la aceptan. Lo
que **todavía no anda** es la parte interesante: crear instancias y
acceder a sus campos. Eso es lo central de la Fase 3.

Lo incluyo igual en la guía porque:

- Si lo ves en la [especificación de sintaxis](syntax-spec.md) te vas a
  preguntar por qué no aparece acá.
- Marca la dirección del lenguaje: cuando llegue Fase 3, vas a poder
  modelar entidades del dominio sin librerías ni JSON ni magia.

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

### Qué hace el intérprete hoy

Cuando declarás un `type`, el evaluador lo registra como un **valor
inerte** en el scope. Podés imprimirlo, pero no hacer mucho más:

```fitz
print(User)              // <type User>
print(Config)            // <type Config>
```

Esto te confirma que la declaración se aceptó, pero el `<type X>` que
ves no es un constructor todavía. Es un placeholder que se va a llenar
de funcionalidad en Fase 3.

### Lo que todavía no se puede hacer

**Instanciar** un tipo, con la sintaxis prevista por la
especificación, no parsea todavía:

```fitz
u = User { id: 1, name: "Fitz" }
// Error en línea 2:10 — se esperaba salto de línea o fin de bloque entre sentencias
```

(El error no es muy descriptivo porque la sintaxis `Nombre { ... }`
como expresión ni siquiera está en el parser todavía. Se mejora cuando
se implemente.)

**Acceder a un campo** sí parsea, pero el evaluador corta con un
mensaje explícito:

```fitz
x = User
print(x.name)
// Error en línea 0:0 — Field access requiere tipos custom instanciados (Fase 3)
```

Mientras tanto, si lo que necesitás es agrupar dos o tres valores
relacionados, la opción más fea pero funcional es usar variables
sueltas o pasar varios parámetros entre funciones. No es elegante, y
es exactamente la fricción que Fase 3 viene a resolver.

### Cómo va a verse en Fase 3

Solo como anticipo —**esto no funciona todavía**:

```fitz
// EN FASE 3 — preview, no compila aún:

type User {
    id: Int
    name: Str
    email: Str?
}

let user = User {
    id: 1,
    name: "Fitz",
    email: "fitz@example.com"
}

print(user.name)            // Fitz
print(user.email)           // fitz@example.com
```

Y todavía más adelante (Fase 4), ese mismo `type` se va a poder usar
en endpoints HTTP, con serialización JSON automática. Pero eso es
para otra guía.

### Ejemplo completo

[examples/guide/10-type.fitz](../examples/guide/10-type.fitz):

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

print(User)
print(Config)
```

Salida:

```
<type User>
<type Config>
```

---

En el próximo capítulo damos un paseo corto por los errores comunes
del intérprete: cómo leer un mensaje, qué significa cada uno, y las
limitaciones de precisión que todavía tiene.

---

## 12. Errores y mensajes

Tarde o temprano vas a tipear algo mal y el intérprete te va a cortar.
Este capítulo es un mapa de los errores que vas a ver: de dónde
salen, cómo leerlos y qué significan.

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

Fitz procesa tu programa en tres etapas, y cada una puede tirar
errores con distinto sabor:

1. **Lexer** — separa el texto en tokens. Si una comilla no cierra o
   aparece un carácter raro, falla acá.
2. **Parser** — arma el árbol de sintaxis. Si la gramática no
   coincide (`if` sin `{`, `match` sin `=>`, expresión incompleta),
   falla acá.
3. **Evaluador** — ejecuta el árbol. Si tu programa parsea pero hace
   algo inválido en runtime (sumar tipos incompatibles, llamar a una
   variable, dividir por cero), falla acá.

Hoy el lexer y el parser dan **posiciones precisas**. El evaluador,
en cambio, casi siempre reporta `0:0` — es deuda explícita: nos
faltan ubicaciones para las subexpresiones. La descripción del error
sí es buena, así que en runtime usamos eso para orientarnos hasta que
se mejore.

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
| `se esperaba '=>' después del patrón` | Brazo de `match` mal formado, típicamente por un patrón no soportado (cap. 9). |
| `se esperaba salto de línea o fin de bloque entre sentencias` | Aparece, entre otras cosas, al intentar instanciar un `type` (cap. 11). |
| `` `for` requiere rangos o listas para iterar, que llegan en Fase 3 `` | El parser ataja `for` con un mensaje explícito (cap. 8). |

Ejemplo:

```fitz
x = 1 +
```

```
Error en línea 1:8 — Se esperaba una expresión, se encontró 'Newline'
```

### Errores típicos del evaluador

Estos son los que más vas a ver mientras escribís lógica:

| Mensaje | Cuándo aparece |
|---------|----------------|
| `variable 'x' no definida` | Usaste un identificador antes de asignarle nada. También aparece en interpolaciones (cap. 5). |
| `operación '+' no soportada entre 'Str' y 'Int'` | Concatenaste tipos distintos sin coerción (cap. 5). Lo mismo para `-`, `*`, `/`. |
| `división por cero` | Dividiste por `0` (Int) o `0.0` (Float). Cap. 4. |
| `la condición de 'if' debe ser Bool, no 'Int'` | Pasaste un valor no-Bool a la condición. Lo mismo aplica a `while`. Cap. 6. |
| `operando izquierdo de 'and' debe ser Bool, no 'X'` | Igual, en `and` / `or`. Cap. 6. |
| `'add' espera 2 argumento(s), recibió 1` | Aridad incorrecta al llamar (cap. 10). |
| `'n' no es invocable (es Int)` | Intentaste llamar como función algo que no lo es. |
| `'break' solo puede usarse adentro de un loop` | `break` / `continue` fuera de un loop. Cap. 8. |
| `'return' solo puede usarse adentro de una función` | `return` en el nivel global. Cap. 10. |
| `el 'match' no matcheó ningún brazo` | El `match` no tenía wildcard y ningún patrón coincidió. Cap. 9. |
| `Field access requiere tipos custom instanciados (Fase 3)` | Tocás `obj.campo` sobre algo que no es una instancia de tipo. Cap. 11. |
| `patrones 'Ok(...)' / 'Err(...)' requieren el tipo Result (Fase 3)` | Pattern `Ok`/`Err` en `match`. Cap. 9. |

Ejemplo (el archivo de este capítulo):

```fitz
fn add(a, b) => a + b
print(add(5))
```

```
Error en línea 0:0 — `add` espera 2 argumento(s), recibió 1
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

[examples/guide/11-errores.fitz](../examples/guide/11-errores.fitz):

```fitz
fn add(a, b) => a + b

print(add(5))
```

Salida (recortada — antes vienen los tokens y el AST):

```
Error en línea 0:0 — `add` espera 2 argumento(s), recibió 1
```

---

Con esto ya sabés interpretar lo que te tira el intérprete. En el
último capítulo te paso un mapa para seguir: lo que viene en Fase 3 y
4, qué pueden esperar los próximos capítulos de la guía, y cómo
contribuir.

---

## 13. Qué sigue

Si llegaste hasta acá: gracias. Esta es la primera versión de la
guía y vos sos parte muy temprana del proyecto.

### Lo que ya sabés

Con los capítulos 1 a 12 podés:

- Escribir y correr programas que combinan **variables, aritmética y
  strings** con interpolación.
- Controlar el flujo con **`if` / `else if` / `else`**, **`while`** y
  **`loop`**, y elegir entre alternativas con **`match`** sobre
  literales.
- Definir **funciones** con su forma de bloque y su forma flecha,
  hacer **recursión** y crear **closures** con captura léxica.
- Declarar **tipos custom** con `type` (todavía sin instanciar).
- Leer un mensaje de error y ubicar de qué fase del intérprete vino.

Es decir: todo lo que el intérprete de Fitz hoy ejecuta end-to-end.

### Lo que viene — Fase 3

La próxima fase del lenguaje arranca con esto. Ver
[docs/roadmap.md](roadmap.md) para el detalle.

- **Tipos custom instanciables** — `User { id: 1, name: "Fitz" }` va
  a ser una expresión válida; vas a poder acceder a `user.name`.
- **Listas y mapas** — `[1, 2, 3]`, `{"a": 1}` y operaciones básicas.
- **`for ... in`** — ya con rangos (`0..10`) y listas como fuentes de
  iteración.
- **Match más completo** — patrones de rango (`0..12`), de tuplas, de
  listas.
- **`Result` y manejo de errores** — `Ok(x)`, `Err(e)`, operador `?`
  para propagar errores. Sin excepciones, como en Rust.
- **Funciones de orden superior** — `map`, `filter`, `reduce` sobre
  listas.
- **Módulos e `import`s** — separar tu código en archivos.
- **Tipado gradual con validación** — las anotaciones que hoy se
  ignoran van a empezar a chequearse.

A medida que estas piezas se van cerrando, esta guía suma capítulos
nuevos.

### Más adelante

- **Fase 4 — HTTP nativo**: `@get`, `@post`, etc. como parte del
  lenguaje. El diferencial principal de Fitz. Servidor que arranca
  automáticamente si hay rutas definidas, serialización JSON
  automática por tipo.
- **Fase 5 — Compilador**: el salto de intérprete a binario nativo,
  via LLVM o Cranelift. Type checker estático completo.
- **Fase 6 — Ecosistema**: package manager, registry, LSP, formatter,
  linter, interop con Python.

### Cómo va a crecer esta guía

La regla es estricta: **un capítulo nuevo solo cuando lo que cubre
funciona end-to-end en el intérprete**. Si una feature está a
medias, no entra todavía. Mejor decir "no se puede" que prometer algo
que va a romper a quien lo lea.

Cada vez que se cierre un grupo de features (típicamente al cerrar
una sub-fase del roadmap), la guía gana un capítulo o varios. Ejemplo
de lo que probablemente venga primero, una vez que arranque Fase 3:

- Capítulo de **listas**, con `for ... in`.
- Capítulo de **tipos custom**, reescribiendo el preview de hoy con
  ejemplos reales.
- Capítulo de **errores con `Result`**, que va a reemplazar (o
  complementar) el actual de errores del intérprete.

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
