# Guía de Fitz

> Estado: viva — cubre lo que el intérprete ejecuta hoy y lo que el
> compilador (`fitz build`) produce como binario nativo.
> Última actualización: 2026-05-12 (Fase 5b cerrada — codegen a
> binario nativo + HTTP, 949 tests pasando).

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

**Parte 7 — HTTP nativo**
17. [HTTP nativo](#17-http-nativo)

**Parte 8 — Compilar**
18. [`fitz build` — compilar a binario nativo](#18-fitz-build--compilar-a-binario-nativo)

**Parte 9 — Cerrando**
19. [Qué sigue](#19-qué-sigue)

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
- Builtins globales: `print`, `len`.

### Qué todavía no anda

Estas cosas aparecen en la [especificación de sintaxis](syntax-spec.md)
pero el intérprete aún no las ejecuta. Si las tipeás vas a ver un
error explícito:

- Tuplas (`(1, "a", true)`).
- Asignación a índice (`xs[0] = v`).
- Métodos custom declarados por el usuario sobre `type`
  (`type User { ... fn greet() => ... }`).
- `async` / `await` reales (la palabra `async` parsea sobre handlers
  HTTP pero no aporta nada en runtime; el bridge es síncrono).
- Status codes custom en handlers HTTP (`return 401 { ... }`) —
  hoy se traducen automático con `Result`.
- Query params en handlers HTTP (`?page=1`).
- Named args en decoradores (`@server(port: 8080)`) — hoy positional.

Todo eso está mapeado en el [roadmap](roadmap.md). Cada vez que una de
estas piezas se cierra, esta guía suma el capítulo correspondiente.

### Cómo está organizada

La guía está dividida en partes que se leen en orden:

1. **Empezando** — qué es Fitz y cómo correr tu primer programa.
2. **Datos y expresiones** — los tipos básicos y cómo se combinan.
3. **Control de flujo y colecciones** — decidir, repetir, agrupar datos.
4. **Abstracción** — funciones, tipos custom, métodos, mutación.
5. **Errores** — `Result` y mensajes del intérprete.
6. **Organización** — partir el código en módulos.
7. **HTTP nativo** — el diferencial de Fitz: decoradores y server
   automático.
8. **Cerrando** — el mapa de lo que viene.

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

### Lo que todavía no anda

- Módulo (`%`) y resto.
- Operadores compuestos (`+=`, `-=`, `*=`, `/=`).
- Operadores de bits (`&`, `|`, `^`, `<<`, `>>`).

Estos están planeados pero el lexer no los tokeniza todavía.

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

### Lo que todavía no anda

- Strings multilínea con `"""..."""`.
- Comillas simples como alternativa a las dobles.
- Métodos como `.split(...)`, `.contains(...)`, `.starts_with(...)`
  — los tres básicos (`.upper()`, `.lower()`, `.len()`) sí están,
  y los ves en el [capítulo 13](#13-métodos-y-mutación).
- Format specifiers dentro de la interpolación (`{ratio:.2f}` y
  similares).

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

(Un poco verboso — los labels llegarán cuando hagan falta de verdad.)

### Lo que todavía no anda

- **`loop` como expresión** — en Rust podés escribir
  `let x = loop { break valor }`. Acá `loop` es solo una sentencia;
  `break` no lleva valor.

- **Labels para `break` / `continue`** — para romper más de un nivel.

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

### Lo que todavía no anda

- **Métodos sobre listas y mapas** — `xs.push(...)`, `xs.map(...)`,
  `m.get(...)`, etc. ya están vivos desde el paso 4 de Fase 3.
  Los ves en el [capítulo 13](#13-métodos-y-mutación).
- **Asignación a índice** (`xs[0] = nuevo`, `m["k"] = v`) — sigue
  siendo deuda. Por ahora la mutación posicional en listas hay que
  hacerla con `pop`/`push` o reconstruyendo.
- **`for` sobre mapas**: necesita el tipo `Pair`/`entry`. Si lo
  intentás, el intérprete corta:

  ```
  Error — `for` sobre Map aún no soportado — necesita el tipo Pair
  ```

- **Índices negativos** (`xs[-1]`) al estilo Python.
- **Rangos inclusivos** (`0..=10`).
- **Comprehensions** (`[x * 2 for x in xs]`).

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

### Lo que todavía no anda

- **Tuples y listas como patrón** — `(a, b)`, `[head, ...rest]`,
  etc. Cuando lleguen los tipos compuestos.
- **Guards** (`patrón if condición => ...`).
- **Or-patterns** (`1 | 2 | 3 => ...`).
- **Exhaustividad para tipos no-Result** — desde Fase 5.3.3
  `fitz check` exige exhaustividad sobre `Result<T>`. Para Int,
  Str y otros tipos no acotados sigue siendo responsabilidad
  tuya cerrar con `_`.

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
- **Chequeo de tipos en runtime** — las anotaciones se guardan pero
  no se validan. Podés pasarle un Str a un campo declarado `Int` y
  el evaluador lo acepta. El chequeo estático llega con el
  compilador (Fase 5).
- **Tipos compuestos en campos** (`emails: List<Str>`) — se parsea el
  nombre del tipo pero no las anotaciones genéricas tipo `List<T>`.
  Por ahora se anota con el nombre suelto y el contenido es libre.

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

> Limitación de hoy: el parser corta en el newline, así que el
> ejemplo de arriba **no** anda partido en líneas múltiples
> empezando con `.`. Hay que mantener la cadena en una sola línea
> (o asignar a variables intermedias). Es deuda chica.

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

### Lo que todavía no anda

- **Asignación a índice** (`xs[0] = nuevo`, `m["k"] = v`). Mientras
  tanto, usá `push`/`pop` para listas o reconstruí el mapa con un
  literal nuevo.
- **Métodos custom sobre `type`** (`type User { ... fn greet() => "Hola, {name}" }`).
  Hoy escribís funciones globales que reciben la instancia como
  primer argumento.
- **Encadenamiento multi-línea** — `.map(...).filter(...)` partido
  en líneas separadas con `.` al inicio de la siguiente. Hay que
  mantenerlo en una sola línea o usar variables intermedias.
- **`return` adentro de un brazo de `match` como expresión** —
  como cada brazo es una expresión, no podés cortar la función
  desde adentro con `return`. Se puede pulir cuando moleste.
- **Más métodos**: `contains`, `trim`, `split`, `starts_with`,
  `concat`, etc. Se irán sumando con la práctica; sin sorpresas
  semánticas.

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

### Lo que todavía no anda

- **`?` fuera de una función** — la implementación reutiliza el
  mecanismo de `return` del lenguaje: cuando `?` ve un `Err`, emite
  un `return` con ese `Err`. Si lo usás en top-level, vas a ver el
  mensaje genérico `` `return` solo puede usarse adentro de una
  función ``. Vamos a darle un mensaje propio más adelante.
- **Chequeo estático de `?`** — desde Fase 5.3.3, `fitz check`
  exige que el operando de `?` sea `Result<T>` y que la función
  contenedora declare `-> Result<...>` (a menos que la función
  esté sin anotación de retorno, donde queda en modo gradual).
  En runtime, si `?` ve un `Err` adentro de una función que no
  estaba pensada para devolver `Result`, vas a tener un
  `Value::Result(Err(...))` saliendo por la puerta de retorno —
  por eso conviene anotar el retorno y dejar que el checker te
  avise antes.
- **Compilar con `fitz build`** — desde 5b.4 el compilador
  soporta `Result`, `Ok`/`Err`, `?` y `match` enteros, así que
  el ejemplo de abajo compila a binario nativo *si las funciones
  anotan sus parámetros* (la inferencia de tipos de params en el
  codegen es deuda residual de 5b.1). El `Err` side se modela
  como `String` Rust pinned: si construís `Err(42)` o similar,
  el codegen lo coerce a String con `format!`. En la práctica
  todos los `Err(...)` útiles llevan mensajes, así que no
  cambia nada — pero queda anotado.
- **`Err` con valores no-Str y bindings tipados** — el binding
  `e` del pattern `Err(e)` siempre tipa `Str` en el código
  compilado, porque el Err side está pinned. En el intérprete
  conserva el tipo original del inner.

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

### Qué no se puede hacer todavía

- **`import foo as f`** — sin aliases. La forma actual es
  `import foo` (binding `foo`).
- **`from foo import bar as b`** — mismo deal.
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
  - Los `let X = ...` top-level del módulo deben tener una
    **RHS literal** (`"texto"`, `42`, `3.14`, `true`, `null`).
    Expresiones más complejas (`let X = compute()`) compilan
    en el intérprete pero no en `fitz build`.
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

Con módulos se cierra la Fase 3. Tenés todas las piezas básicas
del lenguaje. Lo que viene en el próximo capítulo es lo que
hace a Fitz distinto: HTTP nativo.

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

Mientras corre, Fitz tiene dos threads:

- **El intérprete** (thread main): owns todos los handlers
  registrados y procesa requests síncronamente, una a la vez.
- **tokio + axum** (thread aparte): acepta conexiones, parsea
  requests, manda un task al intérprete por un canal y espera la
  respuesta.

Esto es por una restricción real: los `Value` de Fitz usan
`Rc<RefCell<>>` por dentro, que no es `Send` (no puede cruzar
threads). Tener el intérprete en su propio thread, dueño absoluto
de esos `Rc`s, es la forma de mantenerlo todo síncrono adentro
del lenguaje sin meterse en líos con tokio.

En la práctica: hoy un handler lento bloquea a los siguientes.
Para un proyecto chico o un servicio interno alcanza. Cuando
agreguemos `async` real adentro del lenguaje, el bridge va a
mover el handler a un pool y no vas a tener que cambiar nada.

### Qué todavía no anda

- **`async` / `await` adentro del lenguaje** — la keyword `async`
  parsea sobre handlers HTTP, pero no aporta nada en runtime. El
  bridge es síncrono. Cuando se sume async real, los handlers de
  hoy van a seguir funcionando.
- **Compilar HTTP con `fitz build`** — desde 5b.6 el compilador
  produce servidores HTTP nativos (axum + tokio). Desde F11 (post-5b)
  también soporta **state compartido**: cualquier `let users = [...]`
  top-level que un handler referencia se materializa como un
  `thread_local!` en el Rust generado, y cada handler agarra una
  copia del Rc al inicio del body via `.with(|s| s.clone())`. El
  ejemplo de este capítulo y `examples/server.fitz` compilan
  end-to-end con `fitz build` y producen los mismos resultados
  que `fitz run` (validados con curl bit-a-bit). El trade-off del
  approach: el binario producido es **single-threaded** —
  `#[tokio::main(flavor = "current_thread")]` para que el
  thread_local actúe como global. Para los workloads HTTP de Fitz
  hoy (handlers sync, sin async externo) es irrelevante. Cuando
  Fitz sume async/await real adentro del lenguaje, este approach se
  reemplaza por `Arc<Mutex<...>>` + `State` extractor (sub-paso
  futuro). Una restricción visible: las fns HTTP necesitan
  anotación de return type — la inferencia desde el body en
  codegen es deuda 5b.1 separada.
- **Status codes custom** — `return 401 { ... }` está en el
  syntax-spec pero el intérprete aún no lo entiende. Hoy: Result
  destila a 200/500 automático, o tipos no serializables → 500
  explícito.
- **Query params** (`?page=1&size=10`) — sin soporte.
- **Headers de request y de respuesta** — sin acceso desde Fitz.
- **Validación de Content-Type** — cualquier body se intenta
  parsear como JSON. Multipart o urlencoded → cuando hagan falta.
- **Named args en decoradores** (`@server(port: 8080)`) — hoy
  positional.
- **Middleware** (`@auth`, `@cached`, etc.) — los decoradores se
  apilan en el AST y el parser ya los soporta, pero el evaluator
  solo cablea los 5 actuales. El resto entra cuando lo necesitemos.

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

---

Con HTTP cerramos la Fase 4. Tenés ahora todas las piezas para
escribir APIs reales en Fitz: rutas, JSON tipado, manejo de
errores propagable, configuración del server, status codes
custom. El próximo capítulo cubre el otro gran salto de Fase 5:
**compilar el programa a un binario nativo standalone** con
`fitz build`.

---

## 18. `fitz build` — compilar a binario nativo

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
- **Server HTTP multi-threaded** — el binario compilado corre
  como `#[tokio::main(flavor = "current_thread")]` (single-thread),
  porque el state compartido entre handlers vive en un
  `thread_local!` y multi-thread haría que cada worker tuviera su
  propia copia. Para los workloads HTTP de Fitz hoy (handlers
  sync, sin async externo) es invisible — solo aparece si querés
  paralelismo verdadero entre requests. Cuando aterrice async/await
  real en el lenguaje, esto se pivota a `Arc<Mutex<...>>` + `State`
  extractor. (State compartido **sí** compila desde F11 — el
  intérprete y el binario producen el mismo resultado para
  `examples/server.fitz` y `examples/guide/17-http.fitz`.)
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

[examples/guide/18-build.fitz](../examples/guide/18-build.fitz)
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
$ fitz build examples/guide/18-build.fitz
✓ binario: examples/guide/18-build.exe

$ ./examples/guide/18-build.exe &
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

## 19. Qué sigue

Si llegaste hasta acá: gracias. Esta es una versión temprana de la
guía y vos sos parte muy temprana del proyecto.

### Lo que ya sabés

Con los capítulos 1 a 18 podés:

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
  serialización JSON automática, y `@server(port, host)` para
  configurar.
- Leer un mensaje de error del intérprete y ubicar de qué fase vino.
- Validar tipos en compile time con **`fitz check`**, y dejar que
  `fitz run` aborte en modo strict cuando encuentra errores (Fase
  5a cerró el type checker estático).
- **Compilar a binario nativo** con `fitz build`: programa CLI
  o server HTTP que corre sin Fitz instalado en la máquina
  destino (Fase 5b — codegen via transpile-a-Rust + Cargo).

Es decir: todo lo que el intérprete de Fitz hoy ejecuta end-to-end,
con un chequeo estático que atrapa errores antes de que se
ejecuten y un compilador que produce binarios standalone.

### Lo que viene — más allá de Fase 5

Fase 5 está cerrada: **5a (type checker estático)** validó las
anotaciones que durante Fases 2 a 4 se parseaban pero no se
chequeaban, y **5b (codegen a binario nativo)** transpila el AST
a un Cargo project + invoca rustc para producir binarios.

Lo que sigue post-5:

- **Fase 6 — Interop Python** (propuesta, no comprometida): poder
  llamar código Python desde Fitz (y viceversa). El stack inicial
  de FastAPI/SQLAlchemy del autor vive ahí; abrir el camino sin
  pedir que se reescriba todo es lo que justifica la fase.
- **Fase 7 — Ecosistema**: package manager, registry, LSP,
  formatter, linter, plugin de editores.
- **Deuda residual de Fase 5b** que entra como sub-pasos
  futuros si aparece presión:
  - **`async` / `await` reales en el lenguaje** — destrabar
    handlers HTTP concurrentes sin bloquear el reactor. Cuando
    aterrice, también obliga a revisitar F11: el server compilado
    es hoy single-threaded (tokio current_thread) para que el
    `thread_local!` del state compartido actúe como global; un
    handler con `await` cambia el modelo y pide `Arc<Mutex<...>>`
    + `State` extractor.
  - **Inferencia de tipos de params y returns** en fns sin
    anotar — hoy `fn greet(name)` corre en el intérprete pero
    `fitz build` exige anotación. Mismo caso para handlers HTTP
    sin return type explícito.
  - **Status codes custom**, query params, headers, middleware,
    TLS — todo HTTP "más allá del 80%".
  - **Listas/mapas heterogéneos** compilados (`[1, "dos"]`) — el
    intérprete los acepta, el compilador necesita un `FitzValue`
    tagged en runtime.

Ver [docs/roadmap.md](roadmap.md) para el detalle completo y la
deuda explícita acumulada por fase.

### Más adelante

- **Fase 6 — Interop Python**: aprovechar el ecosistema sin
  reescribir todo.
- **Fase 7 — Ecosistema**: package manager, LSP, formatter,
  linter, plugin de editores.

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
