# Guía de Fitz

> Estado: viva — cubre lo que el intérprete ejecuta hoy y lo que el
> compilador (`fitz build`) produce como binario nativo.

Esta guía es para developers que vienen de Python, TypeScript, Vue o
similares y quieren aprender Fitz escribiendo programas reales. Está
pensada para leerse de arriba a abajo: cada capítulo asume lo del
anterior.

Lo que ves acá funciona hoy contra el binario del repo. Si un ejemplo
no corre, es un bug de la guía, del intérprete o del compilador —
abrí un issue.

> **¿Es tu primera vez con Fitz y querés que te lleven paso a paso
> desde la instalación?** El [**curso `Fitz de 0 a experto`**](curso/index.md)
> es la puerta de entrada pedagógica: un proyecto que crece capítulo a
> capítulo desde "hola mundo" hasta una app real. Esta guía sigue
> siendo el lugar para referencia feature por feature.

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

**Parte 11 — Web first-class**
28. [Auth nativa](#28-auth-nativa)
29. [WebSockets tipados](#29-websockets-tipados)
30. [Jobs sin Celery](#30-jobs-sin-celery)
31. [Postgres + ORM nativo](#31-postgres--orm-nativo)

**Parte 12 — Operacional**
32. [Variables de entorno](#32-variables-de-entorno)
33. [Observability — logs, spans, métricas, OTel](#33-observability--logs-spans-métricas-otel)

**Parte 13 — CLI y cerrando**
34. [CLI builder nativo (`@command`)](#34-cli-builder-nativo-command)
35. [Deployment ciudadano primera clase](#35-deployment-ciudadano-primera-clase)
36. [Plantillas y boilerplates](#36-plantillas-y-boilerplates)
37. [Qué sigue](#37-qué-sigue)

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
- **Multiplataforma desde el día uno** — cada release publica binarios
  + extensión VSCode + imagen Docker (`ghcr.io/thegreekman76/fitz:latest`)
  para **4 plataformas**: Windows x64, Linux x64, Linux ARM64 y macOS
  Apple Silicon. El mismo programa Fitz corre en cualquiera; cross-compile
  gratis vía rustc targets desde cualquier plataforma a las otras.
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

A esta altura, casi todo lo que aparece en la [especificación de
sintaxis](syntax-spec.md) ya está implementado. Algunas piezas
grandes pendientes: streaming HTTP, traits / interfaces, herencia
entre `type`s, operator overloading. Cada vez que una pieza se
cierra, esta guía suma el capítulo correspondiente.

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
fitz run examples/guide/02-hola.fitz
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

### Instalar Fitz

Tenés dos caminos. Elegí el que te quede más cómodo.

**Opción A — Binario pre-compilado (recomendado).** Cada release
publica binarios listos para Linux x64/ARM, macOS ARM y Windows x64
en la página de releases del repo:

```
https://github.com/Thegreekman76/fitz/releases/latest
```

Bajá el `.tar.gz` (o `.zip` para Windows) de tu plataforma, descomprimí,
y dejá el ejecutable `fitz` (o `fitz.exe`) en cualquier carpeta de tu
`PATH`. A partir de ahí, `fitz` está disponible desde cualquier
directorio del sistema.

**Opción B — Compilar desde fuente.** Si querés la última versión
de `main` o tu plataforma no tiene binario pre-compilado, necesitás
el toolchain de Rust ([rustup](https://rustup.rs) si todavía no lo
tenés). El repo pinea la versión exacta en `rust-toolchain.toml`, así
que rustup la baja sola la primera vez:

```bash
git clone https://github.com/Thegreekman76/fitz.git
cd fitz
cargo build --release
```

El binario queda en `target/release/fitz` (o `fitz.exe` en Windows).
Copialo a algún directorio del `PATH` o usá la ruta completa.

### Verificar la instalación

```bash
fitz --version
fitz --help
```

Si `fitz --help` lista los subcomandos (`run`, `build`, `check`,
`new`, `test`, `fmt`, `dev`, `repl`, `lint`, `openapi`, `add`,
`remove`, `update`), estás listo.

### Correr un programa

Fitz tiene un intérprete (`fitz run`) que ejecuta tu archivo
directamente, y un compilador (`fitz build`) que produce un binario
nativo standalone. Empezamos con el intérprete:

```bash
fitz run examples/hello.fitz
```

(Si compilaste desde fuente, las rutas relativas son al directorio
desde donde corrés `fitz`. Si bajaste el binario pre-compilado y
querés correr los ejemplos del repo, podés clonarlo igual: `git clone
https://github.com/Thegreekman76/fitz.git`.)

La salida es solo la de tu programa, sin ruido:

```
Hola desde Fitz 🏔️
```

### Tu primer archivo

Vamos a escribir un programa propio. Creá un archivo `hola.fitz` en
cualquier carpeta:

```fitz
// hola.fitz — El primer programa de la guía.
// Muestra: print, asignación sin tipo, interpolación de strings.

print("Hola desde Fitz 🏔️")

name = "Patagonia"
print("Hola, {name}!")
```

Y lo corrés:

```bash
fitz run hola.fitz
```

Salida:

```
Hola desde Fitz 🏔️
Hola, Patagonia!
```

Este mismo ejemplo vive en [examples/guide/02-hola.fitz](../examples/guide/02-hola.fitz)
si querés copiarlo del repo.

### `fitz new` — arrancar un proyecto

Para programas más serios (con tests, deps, varios módulos), Fitz
trae su propio package manager. Un proyecto se crea con:

```bash
fitz new mi_app
cd mi_app
fitz run
```

`fitz new` arma la estructura mínima (`fitz.toml`, `src/main.fitz`,
`.gitignore`, `git init` automático) y `fitz run` sin argumentos
encuentra el `[bin].main` del manifest. El capítulo 16b cubre el
package manager en detalle. Para los ejemplos de esta guía, archivos
sueltos alcanzan.

### Anatomía línea por línea

```fitz
// hola.fitz — El primer programa de la guía.
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
Error leyendo hola.fitz: ...
```

Revisá la ruta. `fitz run` usa el directorio donde corriste el comando
como working directory, así que las rutas son relativas a ahí.

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

### Los seis tipos primitivos

Fitz tiene seis tipos básicos. Los compuestos (listas, mapas,
tipos custom instanciados) los ves en capítulos posteriores.

| Tipo   | Qué es                | Ejemplos             |
|--------|-----------------------|----------------------|
| `Int`  | Entero de 64 bits con signo | `42`, `-7`, `0` |
| `Float`| Punto flotante de 64 bits   | `3.14`, `-0.5`, `2.0` |
| `Str`  | String UTF-8                | `"hola"`, `"Patagonia 🏔️"` |
| `Bool` | Booleano                    | `true`, `false` |
| `Null` | Ausencia de valor           | `null` |
| `Bytes`| Secuencia de bytes binarios | `b"hola"`, `b"\x00\xff"` (ver cap. 5) |

Algunas notas:

- Los `Int` y `Float` son tipos distintos: `1` y `1.0` no son lo mismo,
  aunque se mezclan en operaciones (eso lo vemos en el cap. 4).
- Los strings van con comillas dobles. Las simples no se usan.
- Los emojis y caracteres no-ASCII funcionan: el lexer es UTF-8.
- `null` es un valor de su propio tipo (`Null`), no es un caso especial
  de otro. Imprimir `null` muestra literalmente `null`.
- `Bytes` es paralelo a `Str` pero para datos binarios (sin asumir
  UTF-8). Detalle completo en el cap. 5 sub-sección "Bytes".

### Números legibles

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

### Literales en otras bases

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

Combinados con format specs, podés mostrar un mismo número en
distintas bases para debug:

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

### Identificadores con Unicode

Los identificadores aceptan cualquier carácter Unicode de la categoría
"Letter" (L*) o "Number" (N*) — letras griegas, acentos, ñ, CJK,
cirílico, etc.:

```fitz
let π: Float = 3.14159
let función: Str = "saludar"
let café: Int = 42
let 名前: Str = "Fitz"
let имя: Str = "Roy"
```

Reglas:
- El primer carácter debe ser letra Unicode o `_` (no dígito).
- El resto puede ser letra, dígito o `_`.
- **Emojis** (Unicode "Symbol") quedan EXCLUIDOS — error de lex.
- Dígitos no-ASCII al inicio (`٢`, etc.) también rechazados.

Internamente Rust acepta Unicode identifiers desde edition 2021,
así que `fitz build` los pasa transparente al código generado.

Convención recomendada: ASCII para API pública (compat con tooling);
Unicode OK en código interno cuando aporta claridad (constantes
matemáticas, código en idioma no-inglés).

Ver [examples/guide/03d-identifiers-unicode.fitz](../examples/guide/03d-identifiers-unicode.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

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

`%` calcula el resto de la división entera. Solo válido entre `Int`:

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
que escribirlo dos veces. Se *desugar* en el parser a
`target = target <op> rhs`, así que valen sobre cualquier destino
de asignación: identificador, campo, índice:

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

### Operadores bit-a-bit

Seis operadores bit-a-bit sobre `Int`. Combinan natural con
literales hex/binario/octal para máscaras de bits, flags y
manipulación de bytes.

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

Encaja natural con format specs para debug visual:

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

> **Otros operadores soportados**: módulo `%` sobre `Int` con
> semántica euclidean; operadores compuestos `+=`/`-=`/`*=`/`/=`;
> **operadores de bits** `&`/`|`/`^`/`<<`/`>>`/`~` (ver sub-sección
> de arriba); **compuestos bit-a-bit** `&=`/`|=`/`^=`/`<<=`/`>>=` y
> **prefijos mayúscula** `0X`/`0B`/`0O` (ver sub-sección de abajo).

### Asignación compuesta bit-a-bit

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
| `\r`   | Carriage return |
| `\"`   | Comilla doble literal |
| `\\`   | Backslash literal |
| `\{`   | Llave de apertura literal (sin interpolar) |
| `\}`   | Llave de cierre literal |
| `\0`   | NUL (U+0000) |
| `\b`   | Backspace (U+0008) |
| `\xXX` | Byte ASCII (XX = 2 dígitos hex, 0x00-0x7F) |
| `\u{X...}` | Codepoint Unicode (1 a 6 dígitos hex, hasta U+10FFFF) |

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

// F9 — escapes extendidos
print("\x41 == A")              // A == A
print("caf\u{00E9}")             // café (BMP, 4 dígitos)
print("\u{2603}")                // ☃ (snowman)
print("\u{1F600}")               // 😀 (emoji suplementario, 5 dígitos)
```

El caso de las llaves es importante: si querés mostrar una llave
literal en un string (por ejemplo, JSON inline o un fragmento de
código), tenés que escaparla — si no, el intérprete intenta
interpretar lo que hay entre `{` y `}` como una expresión a
interpolar.

Reglas de los escapes extendidos (mini-tanda **F9**):

- **`\xXX`** exige exactamente 2 dígitos hex. Valores >0x7F se
  rechazan al lex-time con sugerencia de usar `\u{...}` (paralelo
  a Rust). Para chars no-ASCII usá Unicode escapes.
- **`\u{...}`** acepta entre 1 y 6 dígitos hex, con o sin
  zero-padding. Surrogates (`D800`-`DFFF`) y valores >`10FFFF`
  se rechazan al lex-time. Lowercase y uppercase hex valen igual.
- **`\u{}`** vacío es error.
- Si la `\` aparece seguida de un char que no es escape válido
  (`\q`, `\d`, etc.), el lex-time aborta con mensaje claro.

[examples/guide/05d-escapes-extendidos.fitz](../examples/guide/05d-escapes-extendidos.fitz)
ejercita los 4 escapes nuevos + interpolación + triple-quote.

### Strings multilínea con `"""..."""`

Para mensajes largos, SQL, HTML, JSON inline, etc., usá
triple-quote:

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
  `\"`, `\{`, `\}`, `\0`, `\b`, `\xXX`, `\u{...}`).
- **Interpolación** `{expr}` funciona igual.
- Indentación: el contenido se preserva tal cual; si querés
  recortar el indent común usá `.replace(...)` o construilo sin
  indent.

### Format specifiers

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

> **Features completas de strings**: **strings multilínea
> `"""..."""`** con interpolación adentro y preservando newlines
> literales. Métodos `.split(sep)`, `.contains(s)`,
> `.starts_with(s)`, `.ends_with(s)`, `.trim()`,
> `.replace(old, new)`, `.repeat(n)` — vivos en `fitz run` y
> `fitz build` (ver [cap 13](#13-métodos-y-mutación) + ejemplo
> [13c-metodos-extras.fitz](../examples/guide/13c-metodos-extras.fitz)).
> **Format specifiers** `{x:.2f}`, `{n:05d}`, etc. — ver
> sub-sección "Format specifiers" arriba.

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

### Bytes — datos binarios

`Bytes` es un primitivo paralelo a `Str` pero para datos binarios:
secuencias de bytes crudos, sin asumir UTF-8. Útil para protocolos
de red, archivos binarios, payloads opacos.

**Literal**: `b"..."` con escapes hex.

```fitz
let a = b"hola"               // bytes UTF-8 del string "hola"
let b = b"\x00\x01\xff"       // bytes binarios
let vacio = b""
print(a)                       // b"hola"
print(b)                       // b"\x00\x01\xff"
```

Escapes soportados: `\n`, `\r`, `\t`, `\0`, `\\`, `\"`, `\xHH` (byte
hex de 2 dígitos). NO admite interpolación `{...}` (los bytes
literales son fijos).

**Métodos**:

| Método | Devuelve | Qué hace |
|---|---|---|
| `b.len()` | `Int` | Cantidad de bytes (NO chars). |
| `b.is_empty()` | `Bool` | Atajo de `.len() == 0`. |
| `b.to_str()` | `Result<Str>` | Decodifica como UTF-8. `Ok(s)` si es válido, `Err(msg)` si no. |

**Constructor `bytes(s: Str) -> Bytes`**: encodea un Str a UTF-8 bytes.

```fitz
let b = bytes("hola")          // b"hola"
let r = b"\xff".to_str()       // Err("Bytes.to_str(): contenido no es UTF-8 válido en offset 0")
```

**Diferencia con `Str`**: `Bytes("hola") != Str("hola")` — son tipos
distintos aunque el contenido sea el mismo. `==`/`!=` sobre Bytes
compara byte a byte.

**Cuándo usar Bytes vs Str**:

- Manipular texto (interpolar, indexar por char, métodos `.upper()`/
  `.split()`/etc.) → usá `Str`.
- Recibir/emitir bytes crudos (protocolos, archivos binarios,
  uploads, payloads opacos) → usá `Bytes`.
- Tener un `Str` y querés sus bytes UTF-8 → `bytes(s)`.
- Tener `Bytes` que sabés que son UTF-8 → `b.to_str()` con manejo
  de error.

Ver el ejemplo completo en
[examples/guide/05e-bytes.fitz](../examples/guide/05e-bytes.fitz).

---

Con esto ya podés representar y combinar valores. En el próximo
capítulo arrancamos con la lógica: `and`, `or` con short-circuit, y
cómo se conecta con la comparación que vimos en el cap. 4.

---

## 6. Booleanos y lógica

`Bool` tiene exactamente dos valores: `true` y `false`. Los operadores
de comparación e igualdad del cap. 4 ya devuelven `Bool`. Acá los
combinamos con los operadores lógicos `and` y `or`.

### `and`, `or` y `xor`

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
| `true xor true`    | `false`   |
| `true xor false`   | `true`    |
| `false xor true`   | `true`    |
| `false xor false`  | `false`   |

El uso real casi siempre es combinando con comparaciones:

```fitz
age = 20
print(age >= 18 and age < 65)   // true
print(age < 13 or age >= 65)    // false
```

`xor` es útil cuando querés "exactamente uno de los dos" —
equivale a `a != b` sobre Bool pero se lee más declarativo:

```fitz
fn solo_uno(p: Bool, q: Bool) -> Bool {
    return p xor q
}

print(solo_uno(true, false))    // true — solo uno
print(solo_uno(true, true))     // false — ambos
print(solo_uno(false, false))   // false — ninguno
```

`xor` y `or` comparten precedencia (más baja que `and`), y son
left-associative: `a or b xor c` parsea como `(a or b) xor c`.

### Short-circuit

Igual que Python, JavaScript y Rust, los operadores `and` y `or` en
Fitz hacen **short-circuit**:

- `a or b` — si `a` ya es `true`, `b` no se evalúa.
- `a and b` — si `a` ya es `false`, `b` no se evalúa.

`xor` **NO** hace short-circuit: necesita ambos lados para saber el
resultado (paralelo a las operaciones aritméticas).

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

> El operador `not <expr>` exige `Bool` estricto — sin truthy/falsy.

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

### Loop como expresión con valor

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

### Labels en break / continue

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

- (todos los faltantes históricos de loops ya están cubiertos —
  break/continue con labels, loop como expresión con valor, etc.)

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

**Rangos inclusivos** con `..=`:

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

**Step con `step_by(n)`** — útil para saltear elementos al iterar,
materializa el rango con paso:

```fitz
let evens: List<Int> = (0..10).step_by(2)
print(evens)                    // [0, 2, 4, 6, 8]

// step_by también funciona sobre rangos inclusivos.
let by_three: List<Int> = (0..=20).step_by(3)
print(by_three)                 // [0, 3, 6, 9, 12, 15, 18]
```

`n` debe ser `Int > 0` — el runtime corta con error claro si pasás
0 o negativo. Ver cap 13 para los métodos completos sobre Range
(`enumerate`/`zip`/`chain`/`len`/`step_by`).

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

### Iterar Maps con destructuring

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

Mutación posicional con `xs[i] = v` y `m["k"] = v`:

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

### Indexing y slicing

Listas y strings soportan **índices negativos** y **slicing**.

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

### Tuples

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
- Tuples como llave de Map: no soportado por ahora.

Ver [examples/guide/09c-tuples.fitz](../examples/guide/09c-tuples.fitz)
para el ejemplo completo.

### `let` con sub-patterns ricos

Desde Lt, los `let`-tuple aceptan los mismos sub-patterns que los
`match`-arms: literales (`Int`/`Float`/`Str`/`Bool`/`Null`), rangos
(`0..100`, `0..=99`), `Or`-patterns (`1 | 2 | 3`), `Ok(name)` y
`Err(name)`. Si el value no matchea el pattern en runtime, el
programa paniquea con un mensaje claro (paralelo a Rust
`let pat = val else { panic!() }`).

```fitz
// Literal Int como guard
let (1, x) = (1, 42)
print(x)                          // 42

// Literal Str
let ("ada", n) = ("ada", 7)
print(n)                          // 7

// Range
let (0..100, label) = (50, "rango ok")
print(label)                      // rango ok

// Ok-binding: extraer valor de Result
let (Ok(v), tag) = (Ok(99), "result")
print(v)                          // 99
print(tag)                        // result

// Anidamiento
let (Ok(v), (1, y)) = (Ok(42), (1, "deep"))
print(v)                          // 42
print(y)                          // deep
```

**Caveat**: el `let` falla en runtime si el value NO matchea
(`let (1, x) = (2, 42)` paniquea). Si el shape es incierto,
preferí `match`, que te permite cubrir el caso de no-match.

Ver [examples/guide/09f-let-destructure-rico.fitz](../examples/guide/09f-let-destructure-rico.fitz)
para el ejemplo completo y validado bit-a-bit `fitz run` ↔
`fitz build`.

### List comprehensions

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

**Cobertura completa**:
- **Múltiples `for` clauses** (cartesian product):
  `[x*y for x in xs for y in ys]`. El segundo iter puede depender
  del binding del primero, así que `[(i, j) for i in 1..=3 for j in 1..=i]`
  produce los pares triangulares.
- **El `var` acepta tuple destructuring**:
  `[a+b for (a, b) in pairs]` o `[a for (a, _) in pairs]`. Paralelo
  al `for ... in ...` destructure común.
- `iter` puede ser `List<T>` o `Range` (igual que `for ... in`).
- Filter inline `if cond` opcional al final — se aplica en el loop
  más interno.
- **Map comprehensions** `{key: value for var in iter}` — produce
  un `Map<K, V>` con last-write-wins en duplicados de key. Ver
  cap 13 sub-sección dedicada para ejemplos.

Ver [examples/guide/09d-comprehensions.fitz](../examples/guide/09d-comprehensions.fitz)
para el ejemplo simple y
[examples/guide/13p-mb4-y-comprehensions-extendidas.fitz](../examples/guide/13p-mb4-y-comprehensions-extendidas.fitz)
para multi-for + map comprehensions (validados bit-a-bit `fitz run`
↔ `fitz build`).

> **Features completas**: **índices negativos** `xs[-1]` para
> listas y strings + **slicing** `xs[a..b]`, `xs[..b]`, `xs[a..]`,
> `xs[..]`, `xs[a..=b]` (con clamp silencioso). **Tuples**
> `(T1, T2)` con acceso `.0`/`.1`, destructuring
> `let (a, b) = ...`, y `Pattern::Tuple` en match.
> **Comprehensions** `[expr for var in iter]` con filter inline
> opcional. Ver las sub-secciones de arriba.

> **Más features**: **asignación a índice** — ver sección
> "Asignación a índice" arriba. **Rangos inclusivos** (`0..=10`).
> Ahora
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

**`fitz check` exige exhaustividad cuando el valor matcheado tipa
como `Result<T>`** — vas a ver un error si te
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
por `|` en un solo arm. El arm matchea si **cualquiera** de los
sub-patrones matchea:

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
el pattern matchee. El arm matchea si el
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

### Tuple patterns con sub-patterns ricos

Adentro de un tuple pattern (`(a, b)`), los sub-patterns admiten
Str literal, Range, y Or-pattern:

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
- **Exhaustividad para tipos no-Result** — `fitz check` exige
  exhaustividad sobre `Result<T>`. Para Int, Str y otros tipos
  no acotados sigue siendo responsabilidad tuya cerrar con `_`.

> **Features de match completas**: or-patterns (`p1 | p2`), guards
> (`pat if cond`), tuple patterns `(a, b)`, y **tuple patterns con
> sub-patterns Str/Range/Or** — todo soportado en `fitz run` y
> `fitz build`. Ver sub-secciones de arriba.

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

### Parámetros con default

Un parámetro puede tener un valor por defecto: si el caller no lo
provee, se usa ese valor. La sintaxis sigue de cerca a Python: `nombre:
Tipo = expr`.

```fitz
fn greet(name: Str = "amigo") -> Str {
    return "Hola, {name}"
}

print(greet())                            // Hola, amigo
print(greet("Fitz"))                      // Hola, Fitz
```

**Reglas**:

- El default puede ser cualquier expresión válida (literales, idents,
  llamadas, etc.). Se evalúa **cada vez** que se llama la fn sin ese
  arg — no se cachea.
- **Regla Python**: una vez que un param tiene default, todos los
  siguientes también. El parser rechaza `fn f(a = 1, b: Int)` con
  error claro.
- Funciona en `fn` top-level, métodos custom sobre `type` (R.3) y
  métodos estáticos. `fn` anónimos (`fn(x) => ...`) lo aceptan
  sintácticamente pero el caso típico no lo necesita.

```fitz
fn add(a: Int, b: Int = 10) -> Int {
    return a + b
}

print(add(5))                             // 15
print(add(5, 2))                          // 7

fn make(prefix: Str = "x", n: Int = 3) -> Str {
    return "{prefix}-{n}"
}

print(make())                             // x-3
print(make("y"))                          // y-3
print(make("z", 5))                       // z-5
```

Ver [examples/guide/11b-default-params.fitz](../examples/guide/11b-default-params.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

### Varargs

Un param prefijado con `...` es **variádico**: absorbe 0+ args extras
del call site y los recolecta en una `List<T>` adentro del body. Solo
el último param puede ser variádico; no se mezcla con default.

```fitz
fn sum(...xs: Int) -> Int {
    let total: Int = 0
    for x in xs {
        total = total + x
    }
    return total
}

print(sum())                      // 0
print(sum(1, 2, 3))               // 6
print(sum(10, 20, 30, 40))        // 100
```

Funciona con params previos required:

```fitz
fn join(sep: Str, ...parts: Str) -> Str { ... }
print(join("-", "a", "b", "c"))   // a-b-c
```

Ver [examples/guide/11c-varargs.fitz](../examples/guide/11c-varargs.fitz)
para el ejemplo completo.

### Argumentos nombrados

En el call site, podés pasar args por nombre con la sintaxis `name:
value`. El nombre matchea el param de la fn, no la posición:

```fitz
fn greet(name: Str = "amigo", greeting: Str = "Hola") -> Str {
    return "{greeting}, {name}"
}

print(greet(name: "Fitz"))                       // Hola, Fitz
print(greet(greeting: "Hi"))                     // Hi, amigo
print(greet(greeting: "Hey", name: "Roy"))       // Hey, Roy
```

**Reglas**:

- Los named args van **después** de los positionals (`greet("Fitz",
  greeting: "Hi")` ✓; `greet(greeting: "Hi", "Fitz")` ✗).
- El nombre debe corresponder a un param real de la fn.
- Sin duplicados: un mismo param no puede aparecer dos veces.
- No se combina con varargs (decisión MVP).

Útil sobre todo para fns con varios params opcionales, donde solo
querés especificar uno:

```fitz
fn config(host: Str = "127.0.0.1", port: Int = 3000, debug: Bool = false) -> Str {
    return "{host}:{port}/{debug}"
}

print(config(port: 8080))                        // 127.0.0.1:8080/false
print(config(debug: true))                       // 127.0.0.1:3000/true
```

Ver [examples/guide/11d-named-args.fitz](../examples/guide/11d-named-args.fitz)
para el ejemplo completo.

### Lo que todavía no anda

- *(nada significativo — default params, varargs y named args están
  cubiertos. Los siguientes salen del scope de "polish de funciones":
  trait-like polymorphism, herencia entre `type`s y operator overloading
  son decisiones grandes que requieren mini-fase dedicada.)*

> **Features completas de funciones**: **default params**,
> **varargs**, **named args**, **métodos custom sobre `type`** —
> ver [cap 13](#13-métodos-y-mutación).

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
> intérprete infiere desde el body. Con `fitz build` el codegen
> también infiere param + return desde call sites + body para los
> tipos primitivos y los compuestos comunes (Int/Float/Str/Bool/
> Bytes/Nominal/List/Map/Result/Nullable). El ejemplo lleva
> anotaciones porque hacen el contrato explícito (mejor doc +
> diagnostics). El tipo `Fn(Int) -> Int` describe una función que
> toma un `Int` y devuelve un `Int` — es el tipo que tienen
> `square`, `make_adder(5)`, etc.

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

- Los campos se separan con **newline** (o con coma, también). En el
  resto del lenguaje, el `;` es separator opcional entre stmts —
  newline lo cubre en casi todos los casos.
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

- **Herencia / composición de tipos** — un `type` no puede heredar
  campos de otro. Los structs son planos. Para compartir campos,
  por ahora repetirlos o anidarlos (`type Order { user: User, ... }`).
- **Trait-like polymorphism** — no hay interfaces / traits. Si
  necesitás polimorfismo, hoy es vía `match` sobre un enum tipo
  `type Shape { ... }` con un campo discriminador.

> **Features completas de `type`**: **métodos custom sobre `type`**
> — ver [cap 13](#13-métodos-y-mutación) con su propia sub-sección
> y ejemplos. Chequeo estático de anotaciones contra valores
> (`let x: Int = "hola"` falla en `fitz check`), genéricos
> compuestos en campos (`List<Str>`, `Map<Str, User>`, etc.,
> validados por el checker), defaults que referencian otros
> símbolos del módulo de origen.

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

**`fitz check` también valida los métodos built-in estáticamente**:
tipos de argumentos, aridad, tipo del
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

| Método              | Qué hace                                                |
|---------------------|---------------------------------------------------------|
| `get(k)`            | Devuelve `Ok(valor)` si la clave existe, o `Err(...)`.  |
| `has(k)`            | `true` si la clave existe, `false` si no.               |
| `keys()`            | Lista con las claves, en orden de inserción.            |
| `values()`          | Lista con los valores, en orden de inserción.           |
| `len()`             | Cantidad de pares.                                      |
| `filter(fn(k, v))`  | Map nuevo con los pares donde el callback devuelve true (Ex). |
| `map_values(fn(v))` | Map nuevo: aplica `fn` a cada value, mantiene las keys (Ex).  |
| `merge(other)`      | Combina dos `Map<K, V>` (last-write-wins, paralelo a `{**m, **other}`) (Ex2). |
| `update(k, fn(v))`  | Map nuevo: aplica `fn` al value de `k` si existe, no-op si no (Up). |
| `keys_sorted()`     | `List<K>` con las keys ordenadas; K ∈ {Int, Float, Str, Bool} (Mb2). |
| `entries()`         | `List<(K, V)>` con los pares en orden de inserción; inversa de `to_map` (Mb3). |
| `invert()`          | `Map<V, K>` con keys/values intercambiados; last-write-wins en values dups (Mb4). |
| `merge_with(o, fn)` | `Map<K, V>` con merge de `other`; el callback `fn(V, V) -> V` decide en conflicts (Mb6). |
| `with(k, v)`        | Map nuevo con `k → v`; sobreescribe si `k` existe. Functional update (Mb7). |

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

Ver [examples/guide/13j-extras-str-map.fitz](../examples/guide/13j-extras-str-map.fitz)
para ejemplos de `filter`/`map_values` sobre Map.

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

Podés declarar métodos adentro del bloque `type`. Sintaxis:
`fn nombre(params) -> Tipo { body }`, separado de los fields por
newline o coma:

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

### Métodos estáticos

Desde St podés declarar métodos sin receiver con `static fn` adentro
del `type` body. Se invocan como `Type.method(args)` y son útiles
para constructores y factories estilo Rust `User::new(...)` o
Python classmethods:

```fitz
type Counter {
    value: Int = 0

    // Constructor por defaults — `Counter.zero()` es más claro
    // que `Counter { value: 0 }` cuando el caller lee.
    static fn zero() -> Counter {
        return Counter { value: 0 }
    }

    // Factory parametrizado.
    static fn of(n: Int) -> Counter {
        return Counter { value: n }
    }

    // Métodos de instancia siguen funcionando igual.
    fn incremented() -> Counter {
        return Counter { value: value + 1 }
    }
}

let c0 = Counter.zero()                // Counter { value: 0 }
let c5 = Counter.of(5)                 // Counter { value: 5 }
let c6 = c5.incremented()              // Counter { value: 6 }

// Pipelines: factory estático + instance methods encadenados.
let p = Counter.of(1).incremented().incremented()
```

Diferencias con métodos de instancia:
- **NO reciben los fields del `type` como locales**: el body solo ve
  sus params + globals. Si necesitás los defaults del type,
  inicializás vía struct literal (`return Counter {}` o
  `return Counter { value: n }`).
- Se invocan **sobre el tipo**, no sobre una instancia. Llamar a
  un `static fn` con una instancia (`c.zero()`) o un `fn` regular
  con el tipo (`Counter.incremented()`) da error con mensaje
  sugiriendo la forma correcta.
- Pueden ser `async`: `static async fn fetch_default()`.

Implementación: el codegen emite el static como `pub fn` adentro
del `impl <Type>Data { ... }` (Rust associated function), y el call
site `Counter.of(5)` se traduce a `CounterData::of(5i64)`.

Ver [examples/guide/13g-static-methods.fitz](../examples/guide/13g-static-methods.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

### Campos privados

Convención estilo Python pero validada por el checker estático: los
**campos cuyo nombre arranca con `_`** son privados — solo accesibles
desde adentro de los métodos del propio `type`. Desde afuera son
error de tipo.

```fitz
type Account {
    name: Str = "anon"
    _balance: Int = 0           // ← privado

    static fn new(name: Str) -> Account {
        return Account { name: name, _balance: 0 }
    }

    fn deposit(n: Int) -> Account {
        return Account { name: name, _balance: _balance + n }
    }

    fn balance() -> Int { return _balance }
}

let a = Account.new("Ada").deposit(100)
print(a.balance())               // 100 — vía getter público
print(a._balance)                // ERROR: campo `_balance` es privado
```

Reglas del checker:

| Operación                              | Desde afuera | Desde método del mismo type |
|----------------------------------------|--------------|------------------------------|
| `instance._field` (acceso)             | error        | ok                           |
| `Type { _field: v }` (struct lit)      | error        | ok (constructor)             |
| `instance._field = v` (asignación)     | error        | ok                           |
| `other._field` (otra instancia del mismo type) | error | ok                           |

El patrón canónico para inicializar un tipo con campos privados es
un constructor estático (`Type.new(...)`) — la combinación con
métodos estáticos es natural.

El LSP autocomplete también respeta la regla: después de `instance.`
NO sugiere campos `_field`. Adentro de un método del propio type
siguen apareciendo (como locales).

#### Métodos privados

La misma convención aplica a métodos: `_method()` solo se puede
invocar desde adentro de métodos del propio `type`. El checker
rechaza `instance._method()` desde afuera con mensaje claro.

```fitz
type Counter {
    n: Int = 0
    fn _bumped_by(by: Int) -> Counter {     // ← privado
        return Counter { n: n + by }
    }
    static fn bump_twice(c: Counter) -> Counter {
        return c._bumped_by(2)               // ok: adentro del type
    }
}

let c = Counter.bump_twice(Counter {})
print(c._bumped_by(1))                      // ERROR: privado
```

**Caveat de R.3 (opción A)**: los métodos de instancia NO pueden
llamarse entre sí adentro del mismo body — no hay `self` ni `this`.
El patrón canónico para componer métodos privados con públicos es
usar `static fn` que recibe la instancia como param y delega.

LSP autocomplete también filtra `_method` en `instance.`.

Ver [examples/guide/13i-campos-privados.fitz](../examples/guide/13i-campos-privados.fitz)
para el ejemplo completo (incluye sección Vm) — validado bit-a-bit
`fitz run` ↔ `fitz build`.

**Limitaciones del MVP** (R.3 + St + Vp + Vm):
- Sin operator overloading (`fn +(self, other)`).
- Métodos de instancia no pueden llamar a otros métodos del mismo
  type sin un receiver explícito (no hay `self`). Workaround:
  extraer la lógica común a un `static fn` que recibe la instancia.

Ver [examples/guide/13b-metodos-custom.fitz](../examples/guide/13b-metodos-custom.fitz)
para el ejemplo completo de métodos de instancia (incluye async).

### Métodos chicos de Str y List

Resumen de los métodos disponibles sobre `Str` y `List`:

**Sobre `Str`** (S.1 + S.2 + Mb + Ex):

| Método              | Args         | Retorna       | Notas |
|---------------------|--------------|---------------|-------|
| `.contains(s)`      | `Str`        | `Bool`        | empty string siempre matchea |
| `.starts_with(s)`   | `Str`        | `Bool`        | case-sensitive |
| `.ends_with(s)`     | `Str`        | `Bool`        | case-sensitive |
| `.split(sep)`       | `Str`        | `List<Str>`   | empty separator → chars individuales |
| `.trim()`           | —            | `Str`         | whitespace ambos lados |
| `.trim_start()`     | —            | `Str`         | whitespace solo al inicio (Mb) |
| `.trim_end()`       | —            | `Str`         | whitespace solo al final (Mb) |
| `.replace(o, n)`    | `Str`, `Str` | `Str`         | TODAS las ocurrencias |
| `.repeat(n)`        | `Int`        | `Str`         | `n < 0` es error |
| `.find(s)`          | `Str`        | `Result<Int>` | índice (en chars) de la 1ra ocurrencia (Ex) |
| `.index_of(s)`      | `Str`        | `Result<Int>` | alias de `find` (estilo JS/TS) (Ex) |
| `.last_index_of(s)` | `Str`        | `Result<Int>` | índice de la ÚLTIMA ocurrencia (Ex) |
| `.pad_start(w, c)`  | `Int`, `Str` | `Str`         | padding a la izquierda; `c` debe ser 1 char (Mb2) |
| `.pad_end(w, c)`    | `Int`, `Str` | `Str`         | padding a la derecha; `c` debe ser 1 char (Mb2) |
| `.chars()`          | —            | `List<Str>`   | cada char como Str de 1 caracter (Mb3) |
| `.split_at(i)`      | `Int`        | `(Str, Str)`  | divide en char idx; idx >= len → segundo vacío (Mb4) |
| `.lines()`          | —            | `List<Str>`   | separa por `\n`; ignora `\n` final (Mb5) |
| `.is_empty()`       | —            | `Bool`        | atajo de `len() == 0` (Mb5) |
| `.repeat_with(n, sep)` | `Int`, `Str` | `Str`      | repite intercalando `sep`; `n < 0` error (Mb7) |
| `.left(n)`          | `Int`        | `Str`         | primeros `n` chars; `n <= 0` → vacío (Mb8) |
| `.right(n)`         | `Int`        | `Str`         | últimos `n` chars; clamp safe (Mb8) |
| `.center(w, ch)`    | `Int`, `Str` | `Str`         | centra con padding `ch` a ambos lados (Mb8) |

**Sobre `List<T>`** (S.3 + Mb + Lx):

| Método              | Args              | Retorna       | Notas |
|---------------------|-------------------|---------------|-------|
| `.sort()`           | —                 | `Null`        | IN-PLACE, T ∈ {Int, Float, Str, Bool} |
| `.sort_by(cmp)`     | `fn(T, T) -> Int` | `Null`        | IN-PLACE, callback estilo Rust/JS `cmp` (Mb) |
| `.reverse()`        | —                 | `Null`        | IN-PLACE, cualquier T |
| `.contains(v)`      | `T`               | `Bool`        | igualdad estructural |
| `.flatten()`        | —                 | `List<U>`     | requiere `List<List<U>>`, aplana un nivel (Mb) |
| `.any(pred)`        | `fn(T) -> Bool`   | `Bool`        | ¿existe alguno? Vacía → `false` (Lx) |
| `.all(pred)`        | `fn(T) -> Bool`   | `Bool`        | ¿todos? Vacía → `true` (vacuously) (Lx) |
| `.count(pred)`      | `fn(T) -> Bool`   | `Int`         | cuántos cumplen (Lx) |
| `.find_index(pred)` | `fn(T) -> Bool`   | `Result<Int>` | índice del primero o `Err` (Lx) |
| `.flat_map(fn)`     | `fn(T) -> List<U>` | `List<U>`    | map + flatten en un paso (Ex2) |
| `.first()`          | —                 | `Result<T>`   | primer elemento o `Err("lista vacía")` (Ex2) |
| `.last()`           | —                 | `Result<T>`   | último elemento o `Err` (Ex2) |
| `.min()`            | —                 | `Result<T>`   | mínimo numérico; `T ∈ {Int, Float}`; vacía → `Err` (Mb2) |
| `.max()`            | —                 | `Result<T>`   | máximo numérico; `T ∈ {Int, Float}`; vacía → `Err` (Mb2) |
| `.sum()`            | —                 | `T`           | suma numérica; `T ∈ {Int, Float}`; vacía → `0` (Mb2) |
| `.product()`        | —                 | `T`           | producto numérico; `T ∈ {Int, Float}`; vacía → `1` (Mb3) |
| `.reduce(init, fn)` | `Acc`, `fn(Acc, T) -> Acc` | `Acc` | fold canónico; Acc puede ser distinto de T (Mb3) |
| `.to_map()`         | —                 | `Map<K, V>`   | requiere `List<(K, V)>`; last-write-wins (Mb3) |
| `.unique()`         | —                 | `List<T>`     | dedup preservando orden de 1ra aparición (Mb4) |
| `.partition(pred)`  | `fn(T) -> Bool`   | `(List<T>, List<T>)` | divide en truthy/falsy preservando orden (Mb4) |
| `.group_by(fn)`     | `fn(T) -> K`      | `Map<K, List<T>>` | agrupa por key derivada del callback (Mb5) |
| `.zip_with(ys, fn)` | `List<U>`, `fn(T, U) -> V` | `List<V>` | zip + map en un paso; trunca al más corto (Mb5) |
| `.max_by(fn)`       | `fn(T) -> Int`    | `Result<T>`   | item con max ranking; vacía → `Err` (Mb5) |
| `.min_by(fn)`       | `fn(T) -> Int`    | `Result<T>`   | item con min ranking; vacía → `Err` (Mb5) |
| `.scan(init, fn)`   | `Acc`, `fn(Acc, T) -> Acc` | `List<Acc>` | fold con outputs intermedios (cumulative) (Mb6) |
| `.windows(n)`       | `Int`             | `List<List<T>>` | sliding windows de tamaño n; `len < n` → vacía (Mb6) |
| `.take(n)`          | `Int`             | `List<T>`     | primeros `n` elementos; clamp safe (Mb7) |
| `.drop(n)`          | `Int`             | `List<T>`     | saltea primeros `n`; clamp safe (Mb7) |
| `.init()`           | —                 | `List<T>`     | todos menos el último; vacía → vacía (Mb7) |
| `.tail()`           | —                 | `List<T>`     | todos menos el primero; vacía → vacía (Mb7) |
| `.intersperse(sep)` | `T`               | `List<T>`     | inserta `sep` entre elementos (Mb7) |
| `.cycle(n)`         | `Int`             | `List<T>`     | repite la lista `n` veces; `n <= 0` → vacía (Mb7) |
| `.starts_with(p)`   | `List<T>`         | `Bool`        | la lista empieza con el prefix `p` (Mb8) |
| `.ends_with(s)`     | `List<T>`         | `Bool`        | la lista termina con el suffix `s` (Mb8) |
| `.insert_at(i, v)`  | `Int`, `T`        | `List<T>`     | functional: nueva lista con `v` en idx `i` (Mb8) |
| `.remove_at(i)`     | `Int`             | `List<T>`     | functional: nueva lista sin idx `i` (Mb8) |
| `.zip_to_map(vs)`   | `List<V>`         | `Map<K, V>`   | combina keys (self) con values; trunca al corto (Mb8) |

Ver [examples/guide/13c-metodos-extras.fitz](../examples/guide/13c-metodos-extras.fitz)
para los métodos S,
[examples/guide/13e-mini-bundle-metodos.fitz](../examples/guide/13e-mini-bundle-metodos.fitz)
para los de Mb, y
[examples/guide/13h-predicados-list.fitz](../examples/guide/13h-predicados-list.fitz)
para los predicados de Lx.

### Iteradores: `enumerate` / `zip` / `chain`

Tres métodos canónicos para componer listas sin loops manuales,
inspirados en Python/Rust. Todos devuelven una **lista nueva**
(no mutan el receptor).

| Método              | Args         | Retorna           | Notas |
|---------------------|--------------|-------------------|-------|
| `.enumerate()`      | —            | `List<(Int, T)>`  | Pares (índice, elemento). |
| `.zip(ys)`          | `List<U>`    | `List<(T, U)>`    | Empareja dos listas; trunca al más corto. |
| `.chain(ys)`        | `List<T>`    | `List<T>`         | Concatena dos listas del mismo tipo. |

El caso canónico de `enumerate` combina con el tuple destructuring
del `for`:

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

### Iteradores sobre Range

`Range` también expone `enumerate`/`zip`/`chain`/`len` desde Ir.
Antes había que materializar con un list comprehension (`[n for n
in 0..10].enumerate()`) o caer a un loop manual con counter.

```fitz
// enumerate sobre Range — el caso canónico
for (i, n) in (0..5).enumerate() {
    print("{i}: cuadrado de {n} = {n * n}")
}

// zip Range + List — el Range "infinito" 1..100 se trunca por la lista
let usuarios: List<Str> = ["ada", "bea", "cam"]
for (id, name) in (1..100).zip(usuarios) {
    print("user#{id}: {name}")
}

// chain Range + List — concatena
let extras: List<Int> = [100, 200]
print((0..3).chain(extras))                  // [0, 1, 2, 100, 200]

// len sin materializar
print((0..10).len())                          // 10
print((0..=10).len())                         // 11 (inclusive)
```

| Método              | Args         | Retorna             | Notas |
|---------------------|--------------|---------------------|-------|
| `.enumerate()`      | —            | `List<(Int, Int)>`  | Pares (índice, valor). |
| `.zip(ys)`          | `List<U>`    | `List<(Int, U)>`    | Trunca al más corto. |
| `.chain(ys)`        | `List<Int>`  | `List<Int>`         | Concatena con otra `List<Int>`. |
| `.len()`            | —            | `Int`               | Cantidad de elementos. |

Implementación: el runtime materializa el Range a `List<Int>` y
delega; el codegen inline-a `(start..end).collect::<Vec<i64>>()`
adentro del wrap `Arc<Mutex<>>`.

**Caveats**:
- `Range.chain(Range)` directo NO funciona: `chain` espera
  `List<Int>`. Materializá con un list comprehension:
  `[n for n in 0..3].chain([n for n in 5..8])`.
- `Range` no expone `map`/`filter`/`find`/`sort`/etc. — son
  métodos que mutan/transforman; usá la lista materializada
  con el comprehension primero.

Ver [examples/guide/13f-range-iteradores.fitz](../examples/guide/13f-range-iteradores.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

### Reducciones + padding + keys ordenadas + Range con step

Bundle de polish ergonómico que cubre cuatro frentes chicos:

**`List.min()` / `List.max()` / `List.sum()`** — reducciones
numéricas sobre `List<Int>` o `List<Float>`. `min` y `max` devuelven
`Result<T>` porque la lista puede estar vacía; `sum` devuelve `T`
(con `0`/`0.0` como sentinel para vacío). Tipos no numéricos
disparan error.

```fitz
let nums: List<Int> = [3, 1, 4, 1, 5, 9, 2, 6]
match nums.min() {
    Ok(v) => print("min: {v}"),                  // min: 1
    Err(_) => print("vacía")
}
print("sum: {nums.sum()}")                       // sum: 31

let temps: List<Float> = [22.5, 21.0, 23.8]
print("máxima: {temps.max()}")                   // máxima: Ok(23.8)
```

**`Str.pad_start(width, ch)` / `Str.pad_end(width, ch)`** — padding
estilo Python `str.rjust(width, ch)` / `str.ljust(width, ch)`. `ch`
debe ser **exactamente 1 char** (validado en runtime). Si `len(s) >=
width`, devuelve `s` sin cambios.

```fitz
print("42".pad_start(5, "0"))                    // 00042
print("hi".pad_end(5, "."))                      // hi...
print("hola, mundo".pad_start(5, "*"))           // hola, mundo
```

**`Map.keys_sorted()`** — devuelve `List<K>` con las keys
ordenadas. Solo para K en {Int, Float, Str, Bool} (mismas reglas
que `list.sort`). Map vacío → lista vacía. Maps preservan insertion
order por diseño; `keys_sorted` es el escape cuando querés orden
canónico para iterar.

```fitz
let scores: Map<Str, Int> = {"banana": 3, "apple": 1, "cherry": 5}
print(scores.keys_sorted())                      // ["apple", "banana", "cherry"]

for k in scores.keys_sorted() {
    match scores.get(k) {
        Ok(v) => print("{k.pad_end(8, \".\")}: {v}"),
        Err(_) => {}
    }
}
// apple...: 1 / banana..: 3 / cherry..: 5
```

**`Range.step_by(n)`** — materializa el rango con step `n` (debe
ser `Int > 0`). Encadena natural con los otros métodos de List —
útil para iterar saltando elementos.

```fitz
let evens: List<Int> = (0..10).step_by(2)
print(evens)                                     // [0, 2, 4, 6, 8]

let by_three: List<Int> = (0..=20).step_by(3)
print(by_three)                                  // [0, 3, 6, 9, 12, 15, 18]

let suma_pares: Int = (0..100).step_by(2).sum()
print(suma_pares)                                // 2450
```

Ver [examples/guide/13m-min-max-sum-pad-keys-step.fitz](../examples/guide/13m-min-max-sum-pad-keys-step.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

### Fold + product + chars + entries + to_map

Bundle de métodos funcionales que completan la API canónica
"functional collections":

**`List.reduce(init, fn(acc, x) -> Acc)`** — fold canónico.
Equivalente a `Array.prototype.reduce(fn, init)` de JS o
`functools.reduce(fn, xs, init)` de Python. El `Acc` puede ser de
un tipo distinto al de los elementos (caso típico: reducir una
`List<Int>` a un `Str` construyendo una representación).

```fitz
let xs: List<Int> = [1, 2, 3, 4, 5]
let total: Int = xs.reduce(0, fn(acc: Int, x: Int) => acc + x)
print(total)                                     // 15

// Acc de otro tipo:
let csv: Str = xs.reduce("", fn(acc: Str, x: Int) => "{acc}{x},")
print(csv)                                       // 1,2,3,4,5,

// Lista vacía → devuelve el init.
let empty: List<Int> = []
print(empty.reduce(42, fn(acc: Int, x: Int) => acc + x))  // 42
```

**`List.product()`** — análogo a `sum()`. Solo válido sobre
`List<Int>` o `List<Float>` homogéneos. Vacío → `1` (sentinel,
paralelo a Python `math.prod([])`).

```fitz
let factorial_terms: List<Int> = [1, 2, 3, 4, 5]
print(factorial_terms.product())                 // 120
```

**`Str.chars()`** — devuelve `List<Str>` con cada caracter como
Str de 1 char. Útil para componer pipelines sobre strings (chars
+ filter + count + ...). Cuenta caracteres Unicode correctamente,
no bytes.

```fitz
let cs: List<Str> = "fitz".chars()
print(cs)                                        // ["f", "i", "t", "z"]

// Pipeline canónico — contar vocales.
let vocales: Int = "hello".chars().count(fn(c: Str) => c == "e" or c == "o")
print(vocales)                                   // 2
```

**`Map.entries()` ↔ `List<(K, V)>.to_map()`** — conversión
bidireccional Map ↔ lista de pares. `entries()` preserva insertion
order (paralelo a Python `dict.items()`); `to_map()` aplica
last-write-wins en duplicados (paralelo a Python `dict(items)`).

```fitz
let scores: Map<Str, Int> = {"ada": 80, "bob": 92}

// entries: Map → List<(K, V)>
let pairs: List<(Str, Int)> = scores.entries()
print(pairs)                                     // [("ada", 80), ("bob", 92)]

// to_map: List<(K, V)> → Map
let back: Map<Str, Int> = pairs.to_map()
print(back["ada"])                               // 80

// Combinar entries() con List.reduce:
let total: Int = scores.entries().reduce(0,
    fn(acc: Int, p: (Str, Int)) => acc + p.1)
print(total)                                     // 172
```

Ver [examples/guide/13n-reduce-product-chars-entries-to-map.fitz](../examples/guide/13n-reduce-product-chars-entries-to-map.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

### Higher-order por nombre + constantes globales

Bundle de polish del codegen — cierra las dos limitaciones más
visibles del compilador heredadas:

**Higher-order callbacks por nombre** — antes el codegen exigía
callbacks inline `fn(x) => ...` para los métodos de colección.
Ahora podés pasar una fn nombrada directamente:

```fitz
fn double(n: Int) -> Int { return n * 2 }
fn is_even(n: Int) -> Bool { return n % 2 == 0 }
fn sumar(acc: Int, x: Int) -> Int { return acc + x }

let xs: List<Int> = [1, 2, 3, 4, 5]
print(xs.map(double))                            // [2, 4, 6, 8, 10]
print(xs.filter(is_even))                        // [2, 4]
print(xs.reduce(0, sumar))                       // 15
```

Cubre `map`/`filter`/`find`/`any`/`all`/`count`/`find_index`/
`flat_map`/`reduce`/`sort_by` y los callbacks binarios de Map
(`filter`, `map_values`, `update`). El checker valida aridad +
tipos de params + ret type contra la signature de la fn (igual
que con callbacks inline).

**Constantes globales** — antes, un `let X = 100` top-level NO
era accesible desde fns top-level en `fitz build` ("variable
desconocida en codegen"). Había que pasarla como param a cada
fn. Ahora, si la RHS es **const-eval** (literal Int/Float/Bool
o Str literal, o BinOp/UnaryOp puros sobre operands const-eval)
Y la var es referenciada por al menos una fn top-level, el
codegen la "hoistea" automáticamente a `const X: T = ...;` o
`static X: &str = ...;` Rust.

```fitz
let MAX = 100
let GREETING: Str = "hola"
let LIMIT = 10 * 2 + 5     // const-eval con BinOp

fn cap(n: Int) -> Int {
    if (n > MAX) { return MAX }
    return n
}

fn greet(name: Str) -> Str {
    return "{GREETING}, {name}"
}

fn check(n: Int) -> Bool {
    return n < LIMIT
}

print(cap(50))          // 50
print(cap(200))         // 100
print(greet("Ada"))     // hola, Ada
print(check(20))        // true
```

**Reglas del hoist**:
- Solo aplica a `let X = <const-eval>` o `let X: Str = "literal"`.
  RHS con calls a fns, struct lits o expresiones runtime NO se
  hoistan (siguen como locales de `main()`, el codegen rechaza
  si las referencia una fn).
- Si `X` se reasigna (`let X = 10; X = 20`), el hoist se cancela
  (Rust `const` es inmutable). Workaround: pasarla como param a
  cada fn que la necesite, o `Mutex` (futuro).
- Solo aplica a modo CLI. En modo HTTP, el mecanismo de state
  compartido (`thread_local`) ya cubre el caso.

Ver [examples/guide/13o-higher-order-y-consts-globales.fitz](../examples/guide/13o-higher-order-y-consts-globales.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

### Dedup + partition + invert + split_at + multi-for / Map comprehensions

Bundle que combina cuatro métodos chicos (Mb4) con extensiones de
comprehensions (Cmp+) — son features chicos pero suman valor visible
para el día a día.

**`List.unique()`** — dedup preservando orden de 1ra aparición.
Usa igualdad estructural; cualquier `T`. Paralelo a Python
`list(dict.fromkeys(xs))`.

```fitz
let raw: List<Int> = [3, 1, 2, 3, 1, 4]
print(raw.unique())              // [3, 1, 2, 4]
```

**`List.partition(pred)`** — divide en `(truthy, falsy)` preservando
orden. Callback `fn(T) -> Bool`.

```fitz
let nums: List<Int> = [1, 2, 3, 4, 5, 6]
let split: (List<Int>, List<Int>) = nums.partition(fn(n: Int) => n % 2 == 0)
print(split.0)                   // [2, 4, 6] (pares)
print(split.1)                   // [1, 3, 5] (impares)
```

**`Map.invert()`** — intercambia keys ↔ values. Devuelve
`Map<V, K>`. Last-write-wins en values duplicados (paralelo a
`to_map`).

```fitz
let id_to_name: Map<Int, Str> = {1: "Ada", 2: "Bob"}
let name_to_id: Map<Str, Int> = id_to_name.invert()
print(name_to_id["Ada"])         // 1
```

**`Str.split_at(idx)`** — divide el string en char idx (no bytes)
y devuelve `(Str, Str)`. `idx >= len` → segundo elemento vacío;
`idx < 0` → error de runtime.

```fitz
let p: (Str, Str) = "hola mundo".split_at(4)
print(p.0)                       // hola
print(p.1)                       //  mundo
```

**Comprehensions con múltiples `for` clauses** — cartesian product
estilo Python. Las variables del segundo `for` ven los bindings del
primero (útil para listas dependientes):

```fitz
let xs: List<Int> = [1, 2, 3]
let ys: List<Int> = [10, 20]

let combos: List<Int> = [x + y for x in xs for y in ys]
print(combos)                    // [11, 21, 12, 22, 13, 23]

// Con filter al final (aplica en el loop más interno):
let mostly: List<Int> = [x * y for x in xs for y in ys if x % 2 == 1]
print(mostly)                    // [10, 20, 30, 60]

// El segundo iter puede depender del primero (triangular).
let tri: List<(Int, Int)> = [(i, j) for i in 1..=3 for j in 1..=i]
print(tri.len())                 // 6 — pares (1,1), (2,1), (2,2), (3,1), (3,2), (3,3)
```

**Map comprehensions** — sintaxis `{key_expr: value_expr for var in iter}`.
Soporta múltiples `for` clauses y filter opcional (igual que list
comprehensions). En keys duplicadas, last-write-wins (paralelo a
Python `dict comprehension`).

```fitz
// Tabla de cuadrados 1..=5:
let squares: Map<Int, Int> = {n: n * n for n in 1..=5}
print(squares[3])                // 9
print(squares[5])                // 25

// Con filter:
let big: Map<Int, Int> = {n: n * n for n in 1..=10 if n > 5}
print(big[10])                   // 100

// Construir map desde una lista, transformando cada elemento:
let nombres: List<Str> = ["Ada", "Bob", "Cam"]
let lens: Map<Str, Int> = {name: name.len() for name in nombres}
print(lens["Ada"])               // 3
```

Ver [examples/guide/13p-mb4-y-comprehensions-extendidas.fitz](../examples/guide/13p-mb4-y-comprehensions-extendidas.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

### group_by + zip_with + max_by/min_by + lines + async closures

Bundle siguiente a Mb4 — combina cuatro métodos analíticos sobre
colecciones, dos sobre `Str`, y un feature nuevo: async closures
inline.

**`List.group_by(fn(T) -> K)`** — agrupa por una key derivada del
callback. Output: `Map<K, List<T>>`. Preserva orden — el primer
item con key K define posición en el map, items posteriores se
acumulan en su `List<T>`.

```fitz
type User { name: Str = "", role: Str = "guest" }

let users: List<User> = [...]
let by_role: Map<Str, List<User>> = users.group_by(fn(u: User) => u.role)
print(by_role["admin"].len())
```

**`List.zip_with(ys, fn(T, U) -> V)`** — combina zip + map en un
paso (devuelve directamente la transformación, sin pares crudos
intermedios). Trunca al más corto (paralelo a Python `zip`).

```fitz
let xs: List<Int> = [1, 2, 3]
let ys: List<Int> = [10, 20, 30]
let r: List<Int> = xs.zip_with(ys, fn(a: Int, b: Int) => a + b)
print(r)                                         // [11, 22, 33]
```

**`List.max_by(fn(T) -> Int)` / `List.min_by(fn(T) -> Int)`** — útil
para tipos no numéricos (`Instance`, `Str`, etc.) donde `max`/`min`
directos no aplican. El callback extrae un `Int` ranking; devuelve
el item con max/min ranking. Vacía → `Err`.

```fitz
type Item { score: Int = 0, name: Str = "" }
let items: List<Item> = [...]
match items.max_by(fn(it: Item) => it.score) {
    Ok(best) => print("mejor: {best.name}"),
    Err(_) => print("vacío")
}
```

**`Str.lines()` y `Str.is_empty()`** — utilidades de strings:
`lines` separa por `\n` (sin agregar línea vacía si el string
termina con `\n`, paralelo a `str::lines` Rust); `is_empty` es
atajo de `len() == 0` con intención clara.

```fitz
let txt = "Hola\nFitz\n\nFin"
let renglones: List<Str> = txt.lines()
print(renglones.len())                           // 4 (incluye la vacía intermedia)

let validas: List<Str> = renglones.filter(fn(l: Str) => not l.is_empty())
print(validas.len())                             // 3
```

**Async closures inline** — `async fn(...) => expr` o `async fn(...) { ... }`
como expresión: el closure resultante es invocable y devuelve un
`Future<T>` que el caller debe `.await`ar (igual que con fns async
top-level). Habilita patrones funcionales async.

```fitz
async fn run() -> Int {
    // Async closure asignado a una var
    let delayed = async fn(n: Int) -> Int {
        sleep(1).await
        return n * 2
    }
    return delayed(21).await   // 42
}
run().await
```

**Caveat de `fitz build`**: las async closures inline funcionan
en `fitz run` pero NO en `fitz build` (boxing de `Future<T>` con
`Pin<Box<dyn Future>>` requiere infraestructura que no está
todavía). El error de codegen sugiere el workaround: declarar la
fn async top-level y referenciarla por nombre.

Ver [examples/guide/13q-mb5-y-async-closures.fitz](../examples/guide/13q-mb5-y-async-closures.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`
para los métodos Mb5; las async closures sólo en `fitz run`).

### scan + windows + merge_with + async closures en build

Bundle combinado de polish: 3 métodos analíticos adicionales (Mb6),
cierre del caveat de Async-cl build (async closures inline ahora
compilan en `fitz build`), y dos refinamientos del stack HTTP.

**`List.scan(init, fn(acc, x) -> Acc) -> List<Acc>`** — fold con
outputs intermedios. Devuelve una lista con cada estado del
acumulador después de procesar cada elemento. Útil para sumas
parciales, máximos acumulados, etc. Paralelo a `Iterator::scan`
de Rust (sin la sutileza del Option).

```fitz
let xs: List<Int> = [1, 2, 3, 4]
let prefix_sums: List<Int> = xs.scan(0, fn(acc: Int, x: Int) => acc + x)
print(prefix_sums)                               // [1, 3, 6, 10]
```

**`List.windows(n) -> List<List<T>>`** — sliding windows de tamaño
`n`. Cada ventana es una `List<T>` con `n` elementos consecutivos.
Si `len(xs) < n`, devuelve lista vacía. `n <= 0` → error claro.
Paralelo a `slice::windows` de Rust.

```fitz
let xs: List<Int> = [1, 2, 3, 4, 5]
print(xs.windows(3))                             // [[1, 2, 3], [2, 3, 4], [3, 4, 5]]
```

**`Map.merge_with(other, fn(V, V) -> V) -> Map<K, V>`** — merge
generalizado: el callback decide qué value queda cuando hay
conflict de keys (caso típico: `fn(a, b) => a + b` para sumar
valores). Generaliza `merge` (que es last-write-wins).

```fitz
let counts_a: Map<Str, Int> = {"x": 5, "y": 3}
let counts_b: Map<Str, Int> = {"y": 7, "z": 2}
let total: Map<Str, Int> = counts_a.merge_with(counts_b,
    fn(a: Int, b: Int) => a + b)
print(total["x"])                                // 5
print(total["y"])                                // 10 — sumados
print(total["z"])                                // 2
```

**Async closures inline en `fitz build`** — el caveat de Async-cl
(la mini-tanda anterior) está cerrado: `async fn(...)` como
expresión ahora compila a binario nativo. El codegen emite
`move |args| -> Pin<Box<dyn Future<Output=T> + Send>> { Box::pin(
async move { ... }) }`. Paridad bit-a-bit `fitz run` ↔ `fitz build`.

```fitz
async fn run() -> Int {
    let f = async fn(n: Int) -> Int {
        sleep(1).await
        return n * 2
    }
    return f(21).await
}
print(run().await)                               // 42
```

**HTTP — Status codes específicos por kind de `Err`** — cuando un
handler retorna `Result<T, ApiErr>` y la `ApiErr` es un `type` con
field `status: Int`, el runtime HTTP usa ese status code en lugar
del 500 histórico. El body de la response es la Instance
serializada íntegra (no envuelta en `{"error": ...}`):

```fitz
type ApiErr {
    status: Int = 500
    message: Str = ""
}

@get("/users/{id}")
fn get_user(id: Int) -> Result<Str, ApiErr> {
    if (id == 0) { return Err(ApiErr { status: 404, message: "no encontrado" }) }
    if (id < 0)  { return Err(ApiErr { status: 400, message: "id inválido" }) }
    return Ok("Ada")
}
```

Sin field `status` o con status fuera de rango (100..1000), fallback
al 500 histórico con `{"error": <inner>}`.

**HTTP — CORS request-aware con echo del Origin sin filtro** — el
config `allow_origin: "echo"` (Str literal especial) hace echo del
Origin recibido sin filtro (acepta cualquier Origin). Útil para dev
local donde no se conoce la lista de frontends a priori. Si la
request no manda Origin, NO se emite el header (paralelo a `Set`
sin match).

```fitz
@middleware(cors({"allow_origin": "echo"}))
@get("/api")
fn h() -> Str => "ok"
```

Ver [examples/guide/13r-mb6-y-async-build.fitz](../examples/guide/13r-mb6-y-async-build.fitz)
para el ejemplo completo de Mb6 + async closures (validado bit-a-bit
`fitz run` ↔ `fitz build`).

### take + drop + init + tail + intersperse + cycle + repeat_with + with + format specs en build

Bundle final de polish: 7 métodos chicos sobre colecciones y Str +
completar los format specs que faltaban en `fitz build`.

**`List.take(n)` / `List.drop(n)`** — primeros/restantes elementos.
Paralelo a Rust `Iterator::take`/`skip`. Clamp safe: `n` fuera de
rango no es error (`n <= 0` → vacía/full, `n >= len` → full/vacía).

```fitz
let xs: List<Int> = [1, 2, 3, 4, 5]
print(xs.take(3))                                // [1, 2, 3]
print(xs.drop(2))                                // [3, 4, 5]
```

**`List.init()` / `List.tail()`** — todos menos último/primero
respectivamente. Paralelo a Haskell. Sobre lista vacía → lista
vacía (sin error).

```fitz
let xs: List<Int> = [1, 2, 3, 4]
print(xs.init())                                 // [1, 2, 3]
print(xs.tail())                                 // [2, 3, 4]
```

**`List.intersperse(sep)`** — inserta `sep` entre cada par de
elementos consecutivos.

```fitz
let palabras: List<Str> = ["hola", "fitz", "rust"]
print(palabras.intersperse(" | "))               // ["hola", " | ", "fitz", " | ", "rust"]
```

**`List.cycle(n)`** — repite la lista `n` veces. `n <= 0` → vacía
(política friendly, no error).

```fitz
let abc: List<Str> = ["a", "b"]
print(abc.cycle(3))                              // ["a", "b", "a", "b", "a", "b"]
```

**`Str.repeat_with(n, sep)`** — variante de `repeat(n)` que
intercala `sep` entre repeticiones. Paralelo a Python
`sep.join([s] * n)`.

```fitz
print("hi".repeat_with(3, "-"))                  // hi-hi-hi
print("=".repeat_with(20, ""))                   // 20 iguales seguidos
```

**`Map.with(k, v)`** — functional update. Devuelve un Map nuevo
con `k → v`; el receiver queda intacto. Si `k` ya existe,
sobreescribe (last-write-wins). Encadenable para construir Maps
paso a paso sin mutar.

```fitz
let base: Map<Str, Int> = {"a": 1, "b": 2}
let extendido: Map<Str, Int> = base.with("c", 3)
print(extendido.len())                           // 3
print(base.len())                                // 2 — intacto

let chain: Map<Str, Int> = base.with("x", 10).with("y", 20).with("z", 30)
print(chain.len())                               // 5
```

**Format specs en `fitz build` (cierre de deuda Fm)** — los format
specs `,`/`_` grouping, `%` percent y `c` char ahora compilan en
`fitz build` (antes solo andaban en `fitz run`). El codegen emite
helpers `__fitz_fmt_grouping`/`__fitz_fmt_percent`/`__fitz_fmt_char`
en el preludio cuando el programa los usa.

```fitz
// Grouping con `,` o `_`:
let big = 1234567
print("{big:,d}")                                // 1,234,567
print("{big:_d}")                                // 1_234_567

// Percent (multiplica x100, agrega %):
let ratio = 0.857
print("{ratio:.2%}")                             // 85.70%

// Char codepoint (Int → caracter Unicode):
let cp = 65
print("{cp:c}")                                  // A

// Combinación con width/align:
print("|{big:>15,d}|")                           // |      1,234,567|
```

**Deuda residual menor** (sub-paso futuro): el spec `g`/`G`
(general) sigue solo en `fitz run` — requiere lógica de
"decide entre fixed y exponente según magnitud" que es más
involucrada. Sin presión real.

Ver [examples/guide/13s-mb7-y-fmt-build.fitz](../examples/guide/13s-mb7-y-fmt-build.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

### Sublistas + functional updates + ops de bits + g/G format

Bundle final de polish del lenguaje base: 8 métodos chicos sobre
colecciones y Str (Mb8) + 5 builtins globales sobre Int (Bits-extras)
+ completar el último format spec faltante (`g`/`G` general).

**`List.starts_with(prefix)` / `List.ends_with(suffix)`** — devuelve
`Bool`. Igualdad estructural. Prefix/suffix vacío → `true`. Útil para
parsing simple de tokens, paths, etc.

```fitz
let xs: List<Int> = [1, 2, 3, 4, 5]
print(xs.starts_with([1, 2]))                    // true
print(xs.ends_with([4, 5]))                      // true
```

**`List.insert_at(i, v)` / `List.remove_at(i)`** — functional
updates (devuelven lista nueva, receiver intacto). `insert_at` clamp
seguro: `idx >= len` inserta al final; `idx < 0` → error. `remove_at`
exige idx en rango.

```fitz
let base: List<Int> = [10, 20, 40]
print(base.insert_at(2, 30))                     // [10, 20, 30, 40]
print(base.remove_at(0))                         // [20, 40]
```

**`List.zip_to_map(values)`** — combina la lista de keys (self) con
una lista de values formando un `Map<K, V>`. Trunca al más corto.
Equivalente a Python `dict(zip(ks, vs))`.

```fitz
let ks: List<Str> = ["a", "b", "c"]
let vs: List<Int> = [1, 2, 3]
let m: Map<Str, Int> = ks.zip_to_map(vs)
print(m["a"])                                    // 1
```

**`Str.left(n)` / `Str.right(n)`** — primeros/últimos `n` chars.
Char-based, no byte-based. Clamp safe: `n <= 0` → vacío, `n >= len`
→ string completo.

```fitz
let s = "hola mundo"
print(s.left(4))                                 // "hola"
print(s.right(5))                                // "mundo"
```

**`Str.center(width, ch)`** — centra el string padeando con `ch` a
ambos lados hasta `width` chars. `ch` debe ser 1 char. Si el padding
es impar, el extra va a la derecha (paralelo a Python).

```fitz
print("hi".center(10, "-"))                      // "----hi----"
print("Reporte".center(30, "="))                 // "===========Reporte============"
```

**`popcount(n)` / `leading_zeros(n)` / `trailing_zeros(n)` /
`rotate_left(n, bits)` / `rotate_right(n, bits)`** — builtins
globales sobre Int (64 bits). Útil para máscaras, flags, ops bit-a-bit
de bajo nivel.

```fitz
print(popcount(0b1010))                          // 2
print(leading_zeros(1))                          // 63
print(rotate_left(1, 4))                         // 16
```

**Format spec `g`/`G` en `fitz build`** (cierra el último item de
deuda residual de Fm/Fmt-build) — decide entre fixed y exponente
según magnitud y precision; quita ceros trailing del decimal
(paralelo a Python).

```fitz
let x = 1234.5
print("{x:g}")                                   // 1234.5

let chico = 0.00001
print("{chico:g}")                               // 1.00000e-5

print("{1234567890.0:G}")                        // 1.23457E9
```

Ver [examples/guide/13t-mb8-bits-y-fmt-g.fitz](../examples/guide/13t-mb8-bits-y-fmt-g.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

### Math + métodos sobre Int/Float

Bundle numérico + polish final. Tres bloques que destraban code
chains comunes en aritmética, validación de strings, y dispatch
de método sobre primitivos.

**Builtins numéricos globales (`abs`, `min`, `max`, `pow`, `sqrt`,
`ceil`, `floor`, `round`, `clamp`)**:

| Builtin                 | Tipo de retorno           | Notas                                  |
|-------------------------|---------------------------|----------------------------------------|
| `abs(n)`                | `Int` o `Float`           | Mismo tipo que el arg.                 |
| `min(a, b) / max(a, b)` | `Int` o `Float`           | Ambos args deben ser del mismo tipo.   |
| `pow(base, exp)`        | `Float`                   | Siempre Float, incluso con args Int.   |
| `sqrt(x)`               | `Float`                   | Siempre Float.                         |
| `ceil(x) / floor(x) / round(x)` | `Int`             | Siempre Int. Pasa Int de largo.        |
| `clamp(x, lo, hi)`      | `Int` o `Float`           | Tres args del mismo tipo numérico.     |

```fitz
print(abs(-5))                                    // 5
print(abs(-3.14))                                 // 3.14
print(min(3, 5))                                  // 3
print(pow(2, 10))                                 // 1024.0
print(sqrt(16))                                   // 4.0
print(ceil(3.2))                                  // 4
print(floor(3.8))                                 // 3
print(round(3.5))                                 // 4
print(clamp(5, 0, 10))                            // 5
print(clamp(-5, 0, 10))                           // 0
print(clamp(15, 0, 10))                           // 10
```

**`to_json(x) -> Str`** — serializa cualquier valor a un JSON string.

Convierte primitivos, `List<T>`, `Map<Str, V>`, instancias de `type`,
`Nullable` y `Result` a JSON, sin `import` ni interop Python. Una
instancia se serializa como objeto plano `{"campo": valor}` respetando
el orden de declaración de los campos. Valores no serializables
(funciones, módulos, rangos, futuros pendientes, `Secret`, mapas con
clave no-`Str`) producen un error.

```fitz
type User { id: Int, name: Str, active: Bool = true }

print(to_json(42))                                // 42
print(to_json("hola"))                            // "hola"
print(to_json([1, 2, 3]))                         // [1,2,3]
print(to_json({"a": 1, "b": 2}))                  // {"a":1,"b":2}
print(to_json(User { id: 7, name: "Ada" }))       // {"id":7,"name":"Ada","active":true}
```

> **`fitz build`**: `to_json` compila a binario nativo también en
> programas CLI puros (sin rutas HTTP). El core de serialización JSON es
> independiente de la capa HTTP — un programa sin `@get`/`@post`/`@ws`
> solo linkea `serde_json` (sin axum ni tokio). Paridad bit-a-bit con
> `fitz run`.

**Métodos sobre `Str`**:

**`Str.swap_case() / Str.title()`** — manipulación de case.

```fitz
print("Hola Mundo".swap_case())                   // "hOLA mUNDO"
print("hola mundo de fitz".title())               // "Hola Mundo De Fitz"
```

**`Str.is_alpha() / is_digit() / is_numeric()`** — predicates de
contenido. Útiles para validación rápida sin regex. Vacío → false
en los tres (vacuous truth invertida, paralelo a Python).

| Método         | Acepta                                              |
|----------------|-----------------------------------------------------|
| `is_alpha()`   | Todos chars son letras ASCII (`[a-zA-Z]+`).         |
| `is_digit()`   | Todos chars son dígitos `[0-9]+` (estricto).        |
| `is_numeric()` | El string completo parsea como número (Int o Float, signo opcional). |

```fitz
print("hola".is_alpha())                          // true
print("hola123".is_alpha())                       // false
print("12345".is_digit())                         // true
print("3.14".is_numeric())                        // true (parsea como Float)
print("-42".is_numeric())                         // true (signo OK)
print("3.14.5".is_numeric())                      // false
```

**`List.split_at(i)`** — parte la lista en dos en idx, devuelve
**Tuple** de dos lists nuevas. Clamp safe en ambos extremos.

```fitz
let xs: List<Int> = [1, 2, 3, 4, 5]
let parts = xs.split_at(2)
print(parts)                                      // ([1, 2], [3, 4, 5])

print(xs.split_at(0))                             // ([], [1, 2, 3, 4, 5])
print(xs.split_at(99))                            // ([1, 2, 3, 4, 5], [])
```

**`Map.has_value(v)`** — chequea si `v` está presente como value
en algún par del map. Complementa `has(k)` (chequea por key).

```fitz
let scores: Map<Str, Int> = {"ada": 92, "bob": 85, "cam": 92}
print(scores.has_value(92))                       // true
print(scores.has_value(0))                        // false
```

**Métodos sobre primitivos `Int` y `Float`**:

Cierra deuda: ahora podés hacer `n.abs()`, `x.is_nan()`, etc.
sobre Int y Float directamente. Útil para method chaining y
APIs uniformes.

| Receptor | Método           | Descripción                                  |
|----------|------------------|----------------------------------------------|
| `Int`    | `n.abs()`        | Equivalente a `abs(n)`.                      |
| `Int`    | `n.to_str()`     | Convierte a `Str`.                           |
| `Int`    | `n.to_str_base(b)` | Convierte a `Str` en base 2/8/10/16.        |
| `Float`  | `x.abs()`        | Equivalente a `abs(x)`.                      |
| `Float`  | `x.to_str()`     | Convierte a `Str` (mismo formato que print). |
| `Float`  | `x.is_nan()`     | `true` si es NaN.                            |
| `Float`  | `x.is_finite()`  | `true` si NO es NaN ni infinito.             |

```fitz
let n: Int = -42
print(n.abs())                                    // 42
print((42).to_str())                              // "42"
print((255).to_str_base(16))                      // "ff"
print((10).to_str_base(2))                        // "1010"

let x: Float = -3.14
print(x.abs())                                    // 3.14
print((1.0).is_nan())                             // false
print((1.0).is_finite())                          // true
```

Ver [examples/guide/13u-math-mb9-y-int-float.fitz](../examples/guide/13u-math-mb9-y-int-float.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

### `return`/`break`/`continue` en match arm

Cada arm de `match` ahora puede contener `return`/`break`/`continue`
(o un bloque de stmts) además de la expresión clásica. Esto cierra
la deuda histórica donde el match era pure-expression y no se
podía cortar la fn desde adentro.

```fitz
fn classify(n: Int) -> Str {
    match n {
        0 => return "cero"             // early return desde un arm
        1..10 => return "chico"
        _ => return "grande"
    }
    return "unreachable"
}
```

Tres formas de body por arm:

- **Expresión simple**: `pat => expr` (forma clásica).
- **Control flow directo**: `pat => return X`, `pat => break`, `pat => continue`.
- **Bloque con varios stmts**: `pat => { let x = ...; return x * 2 }`.

```fitz
fn compute(n: Int) -> Int {
    return match n {
        0 => {
            let bonus: Int = 10
            return bonus * 2
        }
        1..100 => 99
        _ => -1
    }
}

print(compute(0))                          // 20
print(compute(50))                         // 99
```

Ver [examples/guide/13v-return-en-match.fitz](../examples/guide/13v-return-en-match.fitz)
para el ejemplo completo.

### Lo que todavía no anda

- *(la API de métodos sobre `Str`/`List`/`Map`/`Range`/`Int`/`Float`
  cubre el 99% de los casos. Si te falta uno puntual, abrí un issue.)*

> **Features completas**: encadenamiento multi-línea, **asignación
> a índice** `xs[0] = v` y `m["k"] = v` (ver
> [cap 9](#9-listas-mapas-y-rangos)), **métodos custom sobre
> `type`** (sub-sección de arriba), **métodos chicos de Str y
> List** (`.contains`/`.starts_with`/`.ends_with`/`.split`/
> `.trim`/`.replace`/`.repeat` sobre Str; `.sort`/`.reverse`/
> `.contains` sobre List), **iteradores
> `.enumerate()`/`.zip()`/`.chain()`**, **dispatch sobre
> primitivos `Int`/`Float`** (`n.abs()`, `x.is_nan()`, etc. —
> ver sub-sección de arriba).
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

### `Result<T, E>` con E tipado

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

### Err con tipos compuestos: List y Map

El `E` del `Result<T, E>` puede ser primitivo, una instancia de
un tipo custom, o un compuesto como `List<T>` / `Map<K,V>`. Esto
es útil para devolver **stacks de errores** o **errores
estructurados por campo**:

```fitz
// Errores como lista: stack de validaciones acumuladas
fn validar(input: Str) -> Result<Int, List<Str>> {
    let errs: List<Str> = []
    if input == "" { errs.push("input vacío") }
    if input.len() > 10 { errs.push("input demasiado largo") }
    if errs.len() > 0 { return Err(errs) }
    return Ok(input.len())
}

match validar("") {
    Ok(n) => print("ok: {n}"),
    Err(errs) => print("{errs.len()} error(es): {errs}")
}
// → 1 error(es): ["input vacío"]
```

```fitz
// Errores como map: estructurados por campo
fn validar_obj(name: Str, age: Int) -> Result<Int, Map<Str, Str>> {
    let errs: Map<Str, Str> = {}
    if name == "" { errs["name"] = "no puede estar vacío" }
    if age < 0 { errs["age"] = "debe ser >= 0" }
    if errs.len() > 0 { return Err(errs) }
    return Ok(age)
}
```

El binding `Err(e)` mantiene el tipo concreto, así que podés llamar
`.len()`, `.get(k)`, indexar, iterar, etc. — toda la API de `List`/
`Map` está disponible sobre el value del error.

Ver [examples/guide/14d-err-compuestos.fitz](../examples/guide/14d-err-compuestos.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔ `fitz build`).

### Err con tipos custom

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

- **Chequeo estático de `?`** — `fitz check` exige que el operando
  de `?` sea `Result<T>` y que la función
  contenedora declare `-> Result<...>` (a menos que la función
  esté sin anotación de retorno, donde queda en modo gradual).
- **`Err` con bindings tipados en codegen** — el binding `e` del
  pattern `Err(e)` siempre tipa `Str` en el código compilado,
  porque el Err side sigue pinned a `Result<T, String>`. En el
  intérprete conserva el tipo original del inner. Refactorear
  `Type::Result` para llevar también el tipo del Err es deuda
  residual — toca 10+ sitios del checker.

> **Features de Err completas**: **`Err(<no-Str>)`** preserva el
> tipo en `fitz run` (Int, Instance, Tuple, etc. al desempacar
> con match); **`?` en top-level** aborta con mensaje específico
> mostrando el contenido del Err en lugar del genérico "return
> fuera de función".

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

`fitz run` aborta cuando el checker estático encuentra errores.
Eso quiere decir que un programa con errores de tipo **no llega
a ejecutarse**:

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
(que aborta en modo strict por default):

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
coma final se ignora).

#### Forma multi-línea con paréntesis

Si la lista de imports se hace larga, los paréntesis habilitan la
forma multi-línea estilo Python — cada item en su propia línea:

```fitz
from utils import (
    greet,
    shout,
    PREFIX,
    User,
)
```

Trailing comma antes del `)` opcional. Aliases (`as`) funcionan
igual que en single-line, también mezclables:

```fitz
from utils import (
    greet,
    shout as scream,
    PREFIX as P,
    User as Persona,
)
```

Ver [examples/guide/16d-import-multilinea.fitz](../examples/guide/16d-import-multilinea.fitz)
para el ejemplo completo (validado bit-a-bit `fitz run` ↔
`fitz build`).

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

### Imports transitivos

Los módulos pueden tener sus propios `import` — la cadena se sigue
recursivamente. Esto vale tanto para `fitz run` como para
`fitz build` (mini-tanda **F15** cerró el último gap del codegen).

[examples/guide/16c-modulos-transitivos.fitz](../examples/guide/16c-modulos-transitivos.fitz)
muestra una fachada que organiza dependencias en árbol:

```fitz
// 16c-modulos-transitivos.fitz
import transitivos_app
print(transitivos_app.welcome(42, "Fitz"))     // bienvenido USER#42:FITZ
```

```fitz
// transitivos_app.fitz — fachada
from transitivos_models import User           // ← transitivo
from transitivos_format import format_user    // ← transitivo

fn welcome(id: Int, name: Str) -> Str {
    let u = User { id: id, name: name }
    return "bienvenido {format_user(u)}"
}
```

```fitz
// transitivos_format.fitz — helper
from transitivos_models import User           // ← cadena profunda

fn format_user(u: User) -> Str {
    return "USER#{u.id}:{u.name.upper()}"
}
```

Detalles del loader:

- **Ciclos** detectados con un stack de paths en curso: `a → b → a`
  aborta con `ciclo de imports detectado: ...\a.fitz -> ...\b.fitz
  -> ...\a.fitz` (paralelo bit-a-bit al evaluator).
- **Cache compartida**: si dos módulos transitivos importan el
  mismo archivo, se carga una sola vez.
- **`from python import` adentro de módulos transitivos sí se
  soporta**: cada módulo puede declarar sus propios imports Python
  sin obligar al main a participar. El codegen reusa los helpers
  del preludio Python del crate root via `use crate::__fitz_py_*`
  y emite statics + getters locales por módulo (pyo3 cachea via
  `sys.modules`, así que el OnceLock duplicado es cero overhead
  real). Patrón canónico: librerías Fitz que delegan operaciones a
  Python (numpy/scipy/sqlalchemy/redis-py) sin filtrar el detalle
  a quien las usa. Ver
  [examples/python-interop-modular.fitz](../examples/python-interop-modular.fitz).

### Qué no se puede hacer todavía

- **`foo.User { ... }`** — el struct literal con namespace no
  parsea. La forma actual es `from foo import User`.
- **`stdlib`** (`from fitz import http`) — el prefijo `fitz/` se
  reserva para Fase 4 cuando entre HTTP nativo. Hoy todo es código
  de usuario.
- **`fitz build`** soporta módulos con la única restricción de
  inferencia: las funciones del módulo **deben anotar tipos** de
  parámetros y retorno (limitación heredada de codegen 5b.1; la
  inferencia de tipos de params es deuda residual).

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
**package manager built-in**. Patrón Cargo: manifest `fitz.toml`,
lockfile `fitz.lock`, sub-comandos para crear/agregar/quitar/
actualizar deps.

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

#### Body: `application/x-www-form-urlencoded` (mini-tandas MP + UC)

Los handlers también aceptan bodies tipo `key=value&key2=value2` —
el formato que mandan los browsers cuando un `<form>` HTML usa
`method="POST"` sin `enctype`:

```fitz
@post("/login")
fn login(body: Map<Str, Str>) -> Str {
    let user = body["username"]
    let pass = body["password"]
    return "hola {user}"
}
```

```bash
curl -X POST http://127.0.0.1:3000/login \
    -d "username=fitz&password=secret"
# "hola fitz"
```

El runtime detecta el header `Content-Type: application/x-www-form-urlencoded`
y parsea el body a un `Map<Str, Str>` (paralelo a JSON). URL-decoding
aplicado: `+` → espacio, `%XX` → byte hex.

Funciona end-to-end tanto en `fitz run` como en `fitz build` —
paridad bit-a-bit.

#### Body: `multipart/form-data`

Cuando un `<form>` HTML usa `enctype="multipart/form-data"`
(necesario para uploads de archivos), el body llega como múltiples
"parts" delimitados por un boundary. Fitz lo parsea automáticamente
a un `Map<Str, ...>` donde cada entry es:

- **Text field** (sin `filename` en `Content-Disposition`) →
  `Value::Str` con el contenido.
- **File field** (con `filename`) → instancia del tipo built-in
  `File` con `name: Str?`, `content_type: Str?` y `content: Str`.

```fitz
@post("/upload")
fn upload(body: Map<Str, File>) -> Str {
    let f = body["doc"]
    if (f.name == null) {
        return "sin filename"
    }
    return "subiste '{f.name}' ({len(f.content)} caracteres)"
}
```

```bash
curl -X POST http://127.0.0.1:3000/upload \
    -F "doc=@notas.txt"
# "subiste 'notas.txt' (123 caracteres)"
```

**Limitación MVP**: el `content` de los files es `Str`, así que solo
se admiten files cuyo contenido sea UTF-8 válido. Files binarios
(imágenes, PDFs, zips) hacen el parse fallar con 400. Soporte de
binarios via `Value::Bytes` es deuda residual.

**Paridad `fitz run` ↔ `fitz build`**: el binario nativo también
acepta multipart. Los helpers
`__parse_multipart` y `__extract_multipart_boundary` se emiten en el
preludio HTTP del codegen, paralelos a los del intérprete. El
dispatcher de Content-Type del wrapper async incluye la rama
multipart con el mismo flow de 400 (sin boundary) / 415 (CT no
soportado).

`File` es un nominal built-in del runtime HTTP (igual que `Request` y
`Response`). No hace falta declararlo ni importarlo.

Ejemplo runnable completo: [`examples/guide/17c-multipart.fitz`](../examples/guide/17c-multipart.fitz).

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
puerto o host, decorá una fn con `@server`. Se admiten dos formatos
equivalentes:

```fitz
// Formato positional — clásico, conciso.
@server(8080, "0.0.0.0")
fn main() => 0
```

```fitz
// Formato kwarg (v0.15.13+) — más explícito, recomendado para
// boilerplates Dockerizados donde el host="0.0.0.0" es el detalle
// crítico que el lector busca.
@server(port=8080, host="0.0.0.0")
fn main() => 0
```

Reglas:

- **Positional**: primero `port: Int`, después `host: Str`. Cualquiera
  se puede omitir (`@server(8080)` deja el host default).
- **Kwarg** (v0.15.13+): `port=<Int>` y `host=<Str>` aceptan el mismo
  tipo y validación. Pasados **dos veces** (positional + kwarg) →
  error claro estilo Python (`"port pasado dos veces"`).
- `port` tiene que estar en `[1, 65535]`. Fuera → error al registrar.
- `host` tiene que parsear como **IP literal** (IPv4 o IPv6). No
  hay resolución DNS — `"localhost"` no funciona, usar
  `"127.0.0.1"`.
- Solo un `@server` por programa. Dos → error con el config previo.

**Para Docker**: el default `127.0.0.1` solo acepta conexiones
loopback del mismo container. Para que el port forwarding desde
el host (o el proxy reverso de nginx) funcione, hay que bindear
explícitamente a `"0.0.0.0"`. Es el patrón de todos los boilerplates
Dockerizados del repo (`boilerplates/api-*` + `boilerplates/taskhub`).

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

- **Streaming de respuestas** — hoy las respuestas se serializan
  completas antes de mandarse. Server-sent events y descargas
  grandes están en el roadmap.
- **WebSockets** — disponibles vía `@ws("/path")` + `WsConn<T>`
  (ver cap 29), no como parte de los decoradores HTTP del cap 17.
- **Multipart bodies** — `multipart/form-data` (típicamente uploads
  con files) queda como sub-paso futuro cuando aparezca presión
  real. Cualquier otro Content-Type que no sea JSON o urlencoded
  recibe 415 con mensaje claro.
- **Middleware wrap-style con `next` callable** — el modelo donde
  el middleware controla la invocación del handler (`fn mw(req,
  next) -> Response`) queda como sub-paso futuro. Sí está
  soportado el modelo **post-process**: un middleware con 2 args
  `(Request, Response)` corre DESPUÉS del handler y puede modificar
  el body, agregar headers, etc. Funciona end-to-end en `fitz run`
  y `fitz build`. **Limitación**: handlers `-> Result<T>` + post
  mws no compila en `fitz build` todavía (sub-paso futuro adicional).

> **Features completas del cap HTTP**: async/await reales en
> handlers, paralelismo HTTP real (multi-threaded), status codes
> custom (`return 401 { ... }`), query params (`?page=1&size=10`),
> headers de request con `@header(name="X")`, kwargs en
> decoradores (`@server(docs=false)`), middleware (`@middleware(fn)`)
> + CORS con preflight automático, state HTTP compartido. **Status
> codes específicos de cada `Err`** aparecen en el schema OpenAPI
> cuando el handler hace `return Err(X { status: 404, ... })` con
> un literal Int o con un Ident que apunta a una const top-level
> Int. **Validación de Content-Type estricta** — body no-JSON ni
> urlencoded recibe 415 con msg claro.
> **`application/x-www-form-urlencoded`** — bodies tipo
> `name=Fitz&age=25` se parsean como `Map<Str, Str>` automáticamente
> (URL-decoding aplicado a keys y valores).
> **Return type inference en handlers para `fitz build`** —
> `fn create(u: User) { ... return User { ... } }` no exige
> `-> User` explícito; el codegen infiere del body usando el
> TypeInfo del checker.
> El cap incluye sub-secciones propias para
> [Status codes custom](#status-codes-custom),
> [Query params](#query-params) y
> [Middleware y CORS](#middleware-y-cors) más abajo.

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

Las anotaciones de return ayudan a hacer el contrato del API
explícito, pero **no son obligatorias**: tanto el intérprete como
el compilador (`fitz build`) las infieren desde call sites + body
si están ausentes. La diferencia es estilo — anotaciones siguen
siendo recomendadas para fns públicas (mejor doc + diagnostics
más claros).

Levantalo con `fitz run` (sin compilar):

```bash
fitz run examples/guide/17-http.fitz
```

O compilalo a binario nativo:

```bash
fitz build examples/guide/17-http.fitz
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
3. El status puede ser un literal Int **o un identificador** que
   apunta a una constante top-level Int. El parser distingue por
   la forma del body: `{"key": ...}` (Str primero) →
   ReturnStatus; `{key: ...}` (Ident primero) → struct literal.
   `return 200 user` (sin braces) sigue siendo un `Return` normal.
4. El return type formal del handler se ignora en este path —
   un handler `-> Str` puede mezclar `return "ok"` con
   `return 404 { ... }` en la misma fn.

Las claves del map literal van entre comillas dobles porque la
sintaxis de map literal en Fitz exige que la key sea un valor
(`{"x": 1}`), no un identificador (`{x: 1}` lee la variable `x`).

**Status codes vía constantes**:

Si tenés varios handlers que usan los mismos códigos, nombrarlos
mejora la legibilidad y permite que el schema OpenAPI los detecte:

```fitz
let NOT_FOUND = 404
let UNAUTHORIZED = 401

@get("/protected")
fn protected() -> Str {
    return UNAUTHORIZED {"message": "no autorizado"}
}

@get("/users/{id}")
fn get_user(id: Int) -> Str {
    if (id == 1) {
        return "alice"
    }
    return NOT_FOUND {"error": "no encontrado"}
}
```

Solo `let X = <Int literal>` top-level califica. Una variable local,
un cálculo o un argumento del handler **no** se resuelven al schema
— el schema cae al `500` default histórico. Para resolver el patrón
`return Err({status: NOT_FOUND, ...})` (con un `type` propio para
errores), ver cap 18.

### Respuestas con Content-Type custom (`Response { ... }`)

Hasta acá los handlers devuelven valores que el wrapper de Fitz
serializa a JSON con `Content-Type: application/json`. Para RSS,
Atom, plain text, HTML estático, CSV, SVG, PDF o ZIP necesitamos
control sobre el `Content-Type` y el body crudo (sin JSON-wrap).

El panorama vecino:

| Lenguaje / framework | API | Trade-off |
|---|---|---|
| **FastAPI** (Python) | `Response(content="<rss/>", media_type="application/rss+xml")` | Runtime, sin tipo estático del `Response`. |
| **Express** (Node) | `res.type("application/rss+xml").send(xml)` | Encadenamiento mutable, sin tipo. |
| **Flask** (Python) | `Response("<rss/>", mimetype="application/rss+xml")` | Igual que FastAPI. |
| **Spring** (Java) | `ResponseEntity.ok().contentType(...).body(xml)` | Builder verbose, tipado pero rígido. |
| **Fitz v0.19.0** | `Response { content_type: "...", body: "...", headers: {...} }` | Built-in del lenguaje, paridad bit-a-bit `fitz run` ↔ `fitz build`, OpenAPI auto. |

**Diferenciales de Fitz**:

1. **Built-in del lenguaje, no lib externa**. `Response` está en el
   core, validado por el checker estático. Cero `pip install`,
   cero `npm install`, cero `import`.
2. **Paridad bit-a-bit `fitz run` ↔ `fitz build`**. El binario
   nativo emite los mismos bytes que el intérprete (verificado con
   curl + `xxd` para payloads binarios).
3. **OpenAPI auto-documentado**. El schema 3.1 emite
   `responses.200.content.<media_type>` automático y marca
   `format: binary` cuando hay `body_bytes`. Cero esfuerzo del user.
4. **Tipo built-in con 5 fields explícitos**. Sin builders ni
   mutable state; un struct literal cubre todo el caso.
5. **Validación XOR build-time**. Si seteás `body` literal no-vacío
   + `body_bytes`, el codegen aborta `fitz build` antes del runtime
   con mensaje claro. UX mejor que esperar el 500.

El built-in:

```fitz
type Response {
    status: Int = 200
    content_type: Str = "application/json"
    headers: Map<Str, Str> = {}
    body: Str = ""
    body_bytes: Bytes? = null
}
```

Todos los fields tienen default sensato, así que solo declarás
los que importan.

**Caso 1: RSS feed (text/xml)**

```fitz
@get("/feed.rss")
fn rss_feed() => Response {
    content_type: "application/rss+xml; charset=utf-8",
    body: "<?xml version=\"1.0\"?><rss version=\"2.0\">...</rss>",
}
```

```
HTTP/1.1 200 OK
content-type: application/rss+xml; charset=utf-8
content-length: 91

<?xml version="1.0"?><rss version="2.0">...</rss>
```

**Caso 2: robots.txt + headers de cache**

```fitz
@get("/cached.txt")
fn cached() => Response {
    content_type: "text/plain",
    headers: {"Cache-Control": "public, max-age=3600"},
    body: "cached payload",
}
```

Los `headers` se inyectan en la response real además del
`Content-Type` (que tiene su propio field dedicado). Si el user
pone `Content-Type` adentro de `headers`, el `content_type` field
gana.

**Caso 3: PDF binario (`body_bytes`)**

Para payloads NO UTF-8 (PDF, ZIP, imágenes), usá `body_bytes` en
lugar de `body`:

```fitz
@get("/report.pdf")
fn report() => Response {
    content_type: "application/pdf",
    body_bytes: bytes("%PDF-1.7 ..."),
    headers: {"Content-Disposition": "attachment; filename=report.pdf"},
}
```

`bytes(str)` convierte un Str a un `Bytes`. Para contenido leído
de archivos / DB, ese valor llega como `Bytes` directo. El wrapper
emite los bytes literales en la response (no JSON-wrap del array
de u8) con el `Content-Type` correcto.

**XOR de `body` y `body_bytes`**: solo uno de los dos. Si seteás
ambos no-vacío:

```fitz
// ⛔️ ERROR de codegen (fitz build):
// `Response`: both `body` (non-empty literal) and `body_bytes`
// are set; specify only one.
@get("/x")
fn x() => Response {
    content_type: "application/pdf",
    body: "no",
    body_bytes: bytes("yes"),
}
```

Para `body` literal vacío + `body_bytes` set (caso intencional —
opt-in explícito al binary path) **sí compila** sin warn. Para
casos dinámicos (`body` viene de var o call), el codegen no puede
inferir en build-time y deja la validación al runtime (responde
500 con mensaje claro si ambos llegan no-vacíos).

**Schema OpenAPI auto**:

`fitz openapi server.fitz` y el endpoint `/openapi.json` del
binario emiten:

```json
{
  "/feed.rss": {
    "get": {
      "operationId": "rss_feed",
      "responses": {
        "200": {
          "content": {
            "application/rss+xml; charset=utf-8": {
              "schema": { "type": "string" }
            }
          }
        }
      }
    }
  }
}
```

Para handlers con `body_bytes`, el schema marca
`{"type":"string","format":"binary"}`. Para handlers con
`content_type` dinámico (var/call), el schema default a
`application/octet-stream` con `format: binary`. Documentación de
APIs heterogéneas (mix JSON + RSS + PDF + ZIP) **sin esfuerzo del
user** — herramientas como Scalar / Swagger UI rendean los
endpoints con sus media types reales.

**Combinación con `Result<Response>`**:

```fitz
type FeedError { code: Str }

@get("/feed.rss")
fn rss_feed() -> Result<Response, FeedError> {
    let xml = fetch_feed().await?
    return Ok(Response {
        content_type: "application/rss+xml",
        body: xml,
    })
}
```

El Ok arm pasa por el dispatch dedicado (Content-Type custom).
El Err arm cae al path 500 + JSON error legacy (paralelo al
manejo de Result<T> normal del cap 14).

Ejemplo runnable end-to-end (RSS + robots.txt + SVG + PDF
binario): [`examples/guide/17l-response-custom.fitz`](../examples/guide/17l-response-custom.fitz).

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

**Variantes** (mini-tandas Mw.next + Mw-Wrap):

Hay tres clases de middleware según los args:

| Aridad | Tipo del 2do param      | Kind     | Cuándo corre                    |
|--------|-------------------------|----------|---------------------------------|
| 1      | (solo `Request`)        | **Pre**  | ANTES del handler (gate-only)   |
| 2      | `Response`              | **Post** | DESPUÉS del handler             |
| 2      | `Fn() -> Response`      | **Wrap** | ENVUELVE el handler             |

- **Pre** (clásico): ejemplo `auth` arriba. Útil para auth, rate
  limiting, logging básico.
- **Post**: recibe `(req, resp)` y devuelve una Response modificada.
  Útil para agregar headers, modificar el body, logging con la
  response final.
- **Wrap**: recibe `(req, next)` donde `next` es un callable
  `Fn() -> Response`. El middleware decide cuándo (o si) llamar
  `next()`. Habilita medir tiempo de toda la chain, decisión
  condicional, response wrapping.

Ejemplo de Wrap:

```fitz
fn timing(req: Request, next: Fn() -> Response) -> Response {
    let r = next()       // ejecuta el resto: wraps, handler, posts
    return 200 {"wrapped": true, "method": req.method}
}

@middleware(timing)
@get("/api")
fn api() -> Str => "data"
```

**Limitación de `fitz build`**: los Wrap mws corren solo en
`fitz run`. El binario nativo rechaza con un msg claro citando
`fitz run` como workaround. Los Pre y Post mws sí compilan en
`fitz build`. Ejemplo completo en
[examples/guide/17d-middleware-wrap.fitz](../examples/guide/17d-middleware-wrap.fitz).

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

**Middleware cross-module (v0.19.5+)**: el patrón canónico de
SaaS HTTP es separar los middlewares en su propio módulo y
aplicarlos desde varios módulos de handlers. Por ejemplo, un
`rate_limit.fitz` que define `mw_strict`/`mw_block`, y handlers
en `auth.fitz` / `api.fitz` que apilan `@middleware(mw_strict)`:

```fitz
// src/rate_limit.fitz
async fn mw_strict(req: Request) {
    if (already_too_many(req)) {
        return 429 { "error": "rate limit exceeded" }
    }
    return null
}
```

```fitz
// src/auth.fitz
from rate_limit import mw_strict

@middleware(mw_strict)
@post("/auth/login")
fn login(body: Credentials) -> Result<Token> { ... }
```

Funciona en `fitz build` (binario nativo). Las fns middleware
pueden ser `async` (caso típico: rate limit que consulta DB con
`.await`) y reciben `Request` como primer parámetro normal.

> Limitación conocida (deuda residual derivada de v0.19.5): en
> `fitz run` (intérprete), las fns middleware **sync** funcionan
> cross-module sin problema, pero las `async fn` middleware tienen
> un bug pre-existente del evaluator y devuelven 500. Workaround:
> durante desarrollo, validar el flujo middleware async con
> `fitz build && ./binario`. Se cierra en una iteración futura
> junto a otras mejoras de paridad runtime/codegen.

### HTTP client outbound

Hasta acá vimos el lado **server** del HTTP nativo: handlers que
reciben requests, devuelven JSON, validan body, propagan errores
con `?`. Para cerrar el círculo, Fitz trae un módulo **`http`**
built-in para hacer requests **outbound** — el "lado cliente",
todo en el mismo binario, sin libs externas.

Casos típicos:

- Webhook dispatcher: tu API recibe un evento y lo despacha a
  Slack / Discord / Sentry.
- Integración con APIs externas (Stripe, GitHub, OpenAI).
- Health checks que monitorean servicios externos.
- Proxy / aggregation de múltiples upstreams.

**Panorama vecino**:

| Lenguaje | Cliente HTTP típico | Async-first | Built-in |
|---|---|---|---|
| Python | `requests` o `httpx` (pip install) | `httpx` sí, `requests` no | no |
| JavaScript | `axios` o `fetch` polyfill | Promise (Node ≥18 trae `fetch`) | parcial (`fetch`) |
| Rust | `reqwest` (`cargo add`) | sí (con tokio) | no |
| Java | `OkHttp` o `HttpClient` (Java 11+) | mixto | parcial (Java 11+) |
| Go | `net/http` (stdlib) | sí | sí |
| **Fitz** | **`http` (built-in)** | **sí** | **sí** |

**Por qué Fitz hace esto distinto** — cinco diferenciales que se
componen entre sí:

1. **Built-in del lenguaje, no lib externa**: `http` es módulo
   pre-registrado igual que `log`, `auth`, `jwt`, `db`. No hay
   `pip install`, no hay `npm install`, no hay `cargo add`.
2. **Paridad bit-a-bit `fitz run` ↔ `fitz build`**: el binario
   standalone tiene el cliente HTTP linkeado estático con TLS
   (rustls), sin openssl en el host de runtime. El mismo programa
   produce el mismo wire request en intérprete y compilado.
3. **Async ciudadano de primera**: cada método devuelve
   `Future<Result<HttpClientResponse>>` y se integra natural con
   `@cron`, `@background`, `spawn(...)`, y handlers HTTP server-side.
4. **`Result<T>` automático**: los errores de transporte (timeout,
   DNS fail, TLS handshake) son **valores**, no excepciones. El
   `?` los propaga; el `match` te deja decidir caso por caso;
   el checker estático exige manejarlos (regla 5.3.3 de exhaustividad
   sobre `Result`).
5. **Sin deps externas en el binario final**: el binario `fitz`
   ya trae `reqwest` con `rustls-tls`. Compilás con `fitz build`
   y obtenés un `.exe` standalone — no hace falta OpenSSL en la
   máquina del cliente.

#### API completo

Los 5 métodos comunes:

```fitz
let g = http.get("https://api.example.com/data").await?
let h = http.head("https://example.com").await?
let p = http.post(url, body).await?
let u = http.put(url, body).await?
let d = http.delete(url).await?
```

Y la variante low-level `http.request(opts: Map)` para configurar
timeout, headers, `follow_redirects`, etc:

```fitz
let r = http.request({
    "method": "GET",                    // obligatorio
    "url": "https://api.example.com",   // obligatorio
    "timeout_ms": 5000,                 // opcional, default 30000
    "headers": {"X-Token": "abc"},      // opcional
    "body": "...",                      // opcional (Str|Map|Bytes)
    "follow_redirects": true,           // opcional, default true
}).await?
```

#### El tipo `HttpClientResponse`

Cada call exitoso devuelve un `Result::Ok(HttpClientResponse)` con
cuatro fields:

```fitz
type HttpClientResponse {
    status: Int                // ej. 200, 404, 500
    body: Str                  // texto plano del body
    headers: Map<Str, Str>     // headers de la response
    duration_ms: Int           // tiempo total medido en cliente
}
```

`HttpClientResponse` es **built-in**: no lo importás, está
disponible en cualquier programa. El LSP lo conoce, el checker
verifica los fields, el codegen lo emite como struct Rust nativo.

#### Body shapes — Str, Map, Bytes

Los métodos que llevan body (`post` / `put` / `request`) aceptan
**tres shapes** del lado de Fitz, con la convención que sigue:

| Shape Fitz | Wire body | Content-Type auto |
|---|---|---|
| `Str` | as-is UTF-8 | no se toca |
| `Map<Str, Any>` | `serde_json` serialize | `application/json` |
| `Bytes` | as-is octetos | no se toca |

```fitz
// Map → JSON automático
http.post("https://api.example.com/items", {"name": "ada", "age": 36}).await?

// Str → as-is sin tocar headers
http.post("https://api.example.com/items", "raw body como string").await?

// Bytes → para uploads binarios
http.post("https://api.example.com/upload", file_bytes).await?
```

Si pasás un `Map`, el header `Content-Type: application/json` se
agrega solo. Si lo seteás explícitamente en `headers`, prevalece
el tuyo.

#### Modelo de errores — dos categorías distintas

Es fundamental separar **errores de transporte** de **status
codes 4xx/5xx** — Fitz los modela como dos cosas distintas:

| Categoría | Cómo se manifiesta | Cómo se maneja |
|---|---|---|
| Transporte (timeout, DNS, TLS, conexión) | `Result::Err(Str)` con prefijo `"http: "` | `?` o `match` |
| Status 4xx/5xx (404, 500, etc) | `Result::Ok` con `r.status` | chequeo manual `if (r.status >= 400)` |

Patrón canónico con `?`:

```fitz
async fn fetch_user(id: Int) -> Result<Str> {
    let r = http.get("https://api.example.com/users/{id}").await?
    if (r.status >= 400) {
        return Err("upstream status {r.status}")
    }
    return Ok(r.body)
}
```

Y con `match` para distinguir categorías:

```fitz
match http.get(url).await {
    Ok(r) => {
        if (r.status >= 400) {
            print("API error: {r.status}")
        } else {
            print("OK: {r.body}")
        }
    }
    Err(e) => print("transporte: {e}"),
}
```

Los mensajes de error tienen prefijos identificables — útiles si
querés clasificar en logs / métricas:

| Mensaje | Causa |
|---|---|
| `"http: timeout calling ..."` | timeout |
| `"http: could not connect to ..."` | DNS fail / conexión rechazada |
| `"http: invalid request to ..."` | URL malformada / esquema inválido |
| `"http: request failed to ..."` | TLS handshake / certificado |

#### Integración con el resto del lenguaje

`http.*` es async y devuelve `Future<T>`, por eso encaja natural con:

**Handlers HTTP que proxean upstream** — combinás server-side +
client-side en el mismo programa:

```fitz
@get("/data/{id}")
async fn proxy(id: Int) -> Result<Str> {
    let r = http.get("https://upstream.example.com/data/{id}").await?
    return Ok(r.body)
}
```

**`@background` + `spawn(...)`** — fire-and-forget para webhooks
sin bloquear la response:

```fitz
@background
async fn dispatch(url: Str, payload: Map<Str, Str>) -> Null {
    let _ = http.post(url, payload).await
    return null
}

@post("/events")
fn handle_event(input: EventInput) {
    let _ = spawn(dispatch("https://hooks.slack.com/...", {"text": input.msg}))
    return 202 {"status": "accepted"}
}
```

**`@cron`** — chequeos periódicos sin Celery / cron Unix:

```fitz
@cron("*/30 * * * * *")
async fn ping() -> Null {
    let r = http.head("https://api.example.com/healthz").await
    match r {
        Ok(resp) => log.info("upstream up", status: resp.status),
        Err(e) => log.warn("upstream down", error: e),
    }
    return null
}
```

**Auth Bearer + `spawn(...)` — patrón canónico** (Stripe / Resend /
OpenAI / Mailgun / etc.). Desde v0.19.4 el codegen emite el snapshot
correcto del `headers: Map` para que el future generado sea `Send` y
`tokio::spawn(...)` lo acepte:

```fitz
@background
async fn notify(to: Str, key: Str) -> Null {
    let _ = http.request({
        "method": "POST",
        "url": "https://api.example.com/emails",
        "headers": {
            "Authorization": "Bearer {key}",
            "Content-Type": "application/json"
        },
        "body": "<p>hello</p>"
    }).await
    return null
}

@post("/subscribe")
fn subscribe(input: SubscribeInput) {
    spawn(notify(input.email, "re_xxx"))
    return 202 {"status": "queued"}
}
```

Pre-fix (≤ v0.19.3), el `MutexGuard` del Map literal de `headers`
cruzaba el await del request adentro del `spawn(...)` y `fitz build`
abortaba con `future cannot be sent between threads safely`. Mismo
patrón análogo al de `for x in List<Str>` con `.await` adentro de
`@cron` que cerró v0.18.1.

#### Limitaciones del MVP

Documentadas para entender qué SÍ entra al MVP y qué se queda como
deuda visible (refinable post-MVP si entra demanda real):

- **Sin streaming del body** — `r.body: Str` carga el body entero
  en memoria. Para descargas de archivos grandes (ej. GBs), llega
  como sub-paso futuro con `r.body_stream()` async iterable.
- **Sin multipart form-data** — para upload de archivos con
  `multipart/form-data`. Workaround: armar el body a mano con
  `Bytes` y setear `Content-Type: multipart/form-data; boundary=...`.
- **Sin cookie jar** persistente entre requests. Cada call es
  independiente. Workaround: agarrar `Set-Cookie` de
  `r.headers` y pasarlo en `Cookie` del próximo request.
- **Headers de respuesta como `Map<Str, Str>`** — si un header
  viene duplicado en la response, gana el último. Refinable a
  `List<(Str, Str)>` si entra demanda.
- **El módulo `http` tipa como `Any` en el checker** — paralelo a
  `jwt` / `hash`. Refinable a signatures estrictas post-MVP.

#### Decisión de red para los ejemplos

Los ejemplos abajo apuntan a `httpbin.org` (test target canónico
para clients HTTP). El smoke `GUIDE_EXAMPLES_COMPILE` solo
**compila** los ejemplos a binario — no los ejecuta — así que NO
necesita red durante CI. Para correrlos a mano (`fitz run` o
`./bin`), hace falta conectividad outbound.

#### Ejemplos runnable

Cuatro ejemplos en `examples/guide/` que cubren el MVP entero:

- **`17e-http-client-basico.fitz`** — los 5 métodos comunes
  (GET / HEAD / POST con Map / PUT con Str / DELETE) y la
  inspección de `HttpClientResponse`.
- **`17f-http-client-errores.fitz`** — manejo completo de errores:
  timeout, DNS fail, URL inválida, status 4xx/5xx que NO son
  `Err`, helper canónico que devuelve `Result<Str>` con `?`.
- **`17g-http-client-webhook.fitz`** — webhook dispatcher que
  combina HTTP server-side + HTTP client + `@background` +
  `spawn(...)` + `log.info` estructurado.
- **`17h-http-client-health-checker.fitz`** — health checker estilo
  fitzwatch: `@cron("*/30 * * * * *")` + `http.head` con timeout
  corto + log estructurado.

### SMTP outbound

Si HTTP outbound es "el lado cliente" para APIs externas, **`smtp`**
es el equivalente para **email**. Mandar mails — notificaciones de
incidents, password resets, magic links, alerts, marketing
transactional — desde un programa Fitz no requiere ningún paquete
extra: el módulo `smtp` es **built-in del lenguaje**, paralelo
exacto a `http`/`log`/`jwt`/`hash`.

Casos típicos:

- Notificaciones de status page: el monitor cae → mandás email a la
  team list.
- Password reset / magic-link auth: generás el token, enviás el mail
  con el link único.
- Alertas de jobs `@cron`: el backup nightly falló → email al
  on-call.
- Welcome emails después de un signup HTTP exitoso.
- Reportes diarios / weekly digests despachados desde `@cron`.

**Panorama vecino**:

| Lenguaje | Cliente SMTP típico | Built-in | Sin deps extras |
|---|---|---|---|
| Python | `smtplib` (stdlib) o `yagmail` (pip) | parcial (`smtplib`) | sí (stdlib) / no (yagmail) |
| JavaScript | `nodemailer` (`npm install`) | no | no |
| Rust | `lettre` (`cargo add`) | no | no |
| Java | `JavaMail` (Maven) o `Jakarta Mail` | no | no |
| Go | `net/smtp` (stdlib, low-level) | sí | sí |
| **Fitz** | **`smtp` (built-in)** | **sí** | **sí** |

**Por qué Fitz hace esto distinto** — cinco diferenciales que
componen exactamente como los del HTTP client outbound:

1. **Built-in del lenguaje, no lib externa**: `smtp` está
   pre-registrado en el evaluator y el checker. Cero
   `pip install yagmail`, `npm install nodemailer`, `cargo add lettre`.
2. **Paridad bit-a-bit `fitz run` ↔ `fitz build`**: el binario
   compilado lleva `lettre` linkeado estático con TLS via rustls.
   El mismo programa produce los mismos frames SMTP en intérprete y
   compilado.
3. **Async ciudadano de primera**: `smtp.send(opts)` devuelve
   `Future<Result<SmtpResult>>` y se integra natural con `@cron`,
   `@background`, `spawn(...)`, y handlers HTTP server-side. El
   connection pool (TCP/TLS reusable entre sends) es transparente.
4. **`Result<T>` automático**: errores de transporte (DNS fail,
   auth incorrecta, TLS handshake, server reject, address parse
   inválido) llegan como `Result::Err(Str)` con prefijo `"smtp: "`.
   El `?` propaga, el `match` clasifica, el checker exige
   manejarlos.
5. **Sin deps externas en el binario final**: el binario `fitz`
   ya trae `lettre` con `rustls-tls`. `fitz build` produce un
   `.exe` standalone — no hace falta OpenSSL ni nada en la máquina
   destino.

#### API completo

Una sola función — `smtp.send(opts: Map)`:

```fitz
let r = smtp.send({
    "to": "user@example.com",
    "from": "bot@example.com",       // opcional si SMTP_FROM está en env
    "subject": "Hola desde Fitz",
    "body": "Texto plano del email.",
}).await?
```

Para HTML + texto plano (multipart/alternative), pasás los dos
bodies:

```fitz
let r = smtp.send({
    "to": "user@example.com",
    "from": "bot@example.com",
    "subject": "Hola",
    "body_text": "Versión texto plano para clientes viejos.",
    "body_html": "<h1>Hola</h1><p>Versión <b>HTML</b>.</p>",
}).await?
```

`body` solo (sin sufijos) es alias de `body_text` — cómodo para el
caso 90%.

#### El tipo `SmtpResult`

Cada call exitoso devuelve un `Result::Ok(SmtpResult)` con tres
fields:

```fitz
type SmtpResult {
    delivered: Bool      // true si el servidor aceptó el relay (250)
    message_id: Str      // Message-ID generado por lettre
    duration_ms: Int     // tiempo total (handshake + send + cierre)
}
```

`SmtpResult` es **built-in** (paralelo a `HttpClientResponse`): no
lo importás, está disponible siempre. El LSP lo conoce, el checker
verifica los fields, el codegen lo emite como struct Rust nativo.

#### Configuración via env vars

El módulo lee la configuración del SMTP server desde el **entorno
del proceso** la primera vez que llamás `smtp.send`. Convención
estándar (la misma que Postfix / sendmail-like clients usan en
Docker / Kubernetes):

| Env var | Default | Significado |
|---|---|---|
| `SMTP_HOST` | (required) | hostname del SMTP server |
| `SMTP_PORT` | depende de TLS (587 / 465 / 25) | puerto TCP |
| `SMTP_USER` | (sin auth) | usuario para auth |
| `SMTP_PASSWORD` | (sin auth) | password para auth |
| `SMTP_FROM` | (sin default) | From por defecto si la opts no especifica |
| `SMTP_TLS` | `starttls` | `starttls` (puerto 587) / `implicit` (465) / `none` (25) |

`SMTP_USER` y `SMTP_PASSWORD` van juntos: si uno está y el otro no,
el primer send aborta con error claro citando el desbalance.

Para dev local, una sola env var alcanza si apuntás a
[MailHog](https://github.com/mailhog/MailHog) (smtp server fake con
UI web que retiene los mails sin reenviar):

```bash
docker run -d --name mailhog -p 1025:1025 -p 8025:8025 mailhog/mailhog
export SMTP_HOST=localhost
export SMTP_PORT=1025
export SMTP_TLS=none
# Después abrís http://localhost:8025 en el browser para ver los mails.
```

#### Modelo de errores

Paralelo bit-a-bit a HTTP client: los errores de transporte llegan
como `Result::Err(Str)` con prefijo `"smtp: "`, identificables:

| Mensaje | Causa |
|---|---|
| `"smtp: SMTP_HOST is not set"` | falta config en el entorno |
| `"smtp: invalid `to` address ..."` | parse de email malformado |
| `"smtp: server rejected mail: ..."` | el server respondió 5xx (5.x.x SMTP) |
| `"smtp: transient error: ..."` | response 4xx temporario (4.x.x SMTP) |
| `"smtp: client error: ..."` | TLS handshake, auth fail, protocolo |
| `"smtp: permanent error: ..."` | conexión rechazada por el server |

Patrón canónico con `?`:

```fitz
async fn notify(addr: Str, subject: Str, body: Str) -> Result<Str> {
    let r = smtp.send({
        "to": addr,
        "subject": subject,
        "body": body,
    }).await?
    return Ok(r.message_id)
}
```

Y con `match` para distinguir categorías:

```fitz
match smtp.send(opts).await {
    Ok(r) => log.info("smtp.delivered", message_id: r.message_id, ms: r.duration_ms),
    Err(e) => log.warn("smtp.failed", error: e),
}
```

#### Integración con el resto del lenguaje

Igual que `http.*`, `smtp.send` es async y devuelve `Future<T>`, así
que encaja natural con:

**`@background` + `spawn(...)`** — fire-and-forget para no bloquear
la HTTP response del signup:

```fitz
@background
async fn send_welcome(email: Str) -> Null {
    let _ = smtp.send({
        "to": email,
        "subject": "Bienvenido",
        "body": "Hola, gracias por unirte.",
    }).await
    return null
}

@post("/signup")
fn signup(input: SignupInput) {
    // ... crear el user en la DB ...
    let _ = spawn(send_welcome(input.email))
    return 201 {"id": new_user_id}
}
```

**`@cron`** — digests / reportes periódicos:

```fitz
@cron("0 0 9 * * *")  // todos los días 09:00
async fn daily_digest() -> Null {
    let r = smtp.send({
        "to": "team@example.com",
        "subject": "Daily digest",
        "body_html": "<p>Hoy: ...</p>",
    }).await
    match r {
        Ok(_) => log.info("digest.sent"),
        Err(e) => log.error("digest.failed", error: e),
    }
    return null
}
```

**Combinado con `auth` + `jwt`** — magic-link auth en un solo
binario:

```fitz
@post("/auth/magic-link")
async fn magic_link(input: EmailRequest) -> Result<Str> {
    let token = jwt.encode({"email": input.email}, "secret", {})?
    let link = "https://app.example.com/auth/verify?t={token}"
    let r = smtp.send({
        "to": input.email,
        "subject": "Tu link de login",
        "body": "Hacé click en este link para entrar:\n{link}",
    }).await?
    return Ok(r.message_id)
}
```

#### Limitaciones del MVP

Documentadas para entender qué SÍ entra y qué queda como deuda
visible (refinable post-MVP):

- **Sin attachments** — `attachments` en `opts` se rechaza con
  error claro. Workaround: typical flow es bundle de attachments
  via S3/object storage + link en el body.
- **Sin `smtp.configure(...)`** — la config sale solo de env vars
  hoy. Si necesitás override programático (multi-tenant con un SMTP
  server por tenant), te queda como sub-paso post-MVP.
- **Map literal solo strings** — en `fitz build`, las keys son `Str`
  estrictas. Heterogéneos `Map<Str, Any>` con valores como Int /
  Bool requieren `__FitzValue` integration (deuda paralela a
  `jwt.encode`). Workaround: stringificar antes con interpolación.
- **El módulo `smtp` tipa como `Any` en el checker** — paralelo a
  `http`/`jwt`/`hash`. Refinable a signatures estrictas post-MVP.
- **Sin templates dedicados** — el body es `Str` con interpolación
  `"hola {name}"`. Para templates más sofisticados (Handlebars,
  Tera), queda como deuda.
- **Sin retry built-in** — `smtp.send` falla → es tu responsabilidad
  reintentar (típicamente con `@cron` + tabla de outbox).

#### Decisión de red para los ejemplos

Los ejemplos abajo apuntan a **MailHog** en `localhost:1025`. El
smoke `GUIDE_EXAMPLES_COMPILE` solo **compila** los ejemplos —
no los ejecuta — así que no necesita SMTP server real durante CI.
Para correrlos a mano (`fitz run` o `./bin`), levantás MailHog con
el `docker run` de arriba y exportás las 3 env vars.

#### Ejemplos runnable

Tres ejemplos en `examples/guide/` que cubren el MVP entero:

- **`17i-smtp-basico.fitz`** — `smtp.send` simple con body plano
  contra MailHog local; inspección de `SmtpResult.delivered` y
  `r.message_id`.
- **`17j-smtp-errores.fitz`** — modelo de errores: faltan env vars,
  address malformada, body multi-line con interpolación; helper
  canónico que devuelve `Result<Str>` con `?`.
- **`17k-smtp-magic-link.fitz`** — magic-link auth combinando
  `@post` HTTP server-side + `jwt.encode` + `smtp.send` con HTML +
  body texto plano alternativos (`body_text` + `body_html` →
  multipart/alternative automático).

---

Con HTTP cerramos la Fase 4 y la mini-fase de Middleware + CORS
post-Fase 7, más la mini-tanda HTTP client del lado outbound y la
mini-tanda SMTP. Tenés ahora todas las piezas para escribir APIs
reales en Fitz: rutas, JSON tipado, manejo de errores propagable,
configuración del server, status codes custom, query params,
middleware apilable, CORS configurable, cliente HTTP outbound para
integrar con servicios externos, y SMTP outbound para mandar mails
sin libs extras. El próximo capítulo cubre la **paridad con
FastAPI en developer experience**: documentación de la API
autogenerada (`/openapi.json` + UI Scalar en `/docs`). Después, el
cap 19 cubre la otra mitad de "HTTP nativo": concurrencia con
`async fn` y `.await`.

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

**Status codes vía constantes top-level**:

- El schema resuelve no solo literales (`return 404 { ... }`) sino
  también identificadores que apuntan a una `let X = <Int>`
  top-level del programa. Útil para nombrar los códigos:

  ```fitz
  let NOT_FOUND = 404
  let UNAUTHORIZED = 401

  type ApiErr { status: Int, message: Str }

  @get("/users/{id}")
  fn h(id: Int) -> Result<User, ApiErr> {
      if (id == 0) {
          return Err(ApiErr { status: NOT_FOUND, message: "no user" })
      }
      ...
  }
  ```

  El schema OpenAPI incluye los responses `200` (return type),
  `401` y `404` (resueltos desde la tabla de consts). La sintaxis
  `return NOT_FOUND { ... }` (Stmt::ReturnStatus directo con Ident
  como status) también funciona y entra al schema.

  Vars locales del handler o expresiones complejas (`return
  compute_status() { ... }`) siguen invisibles para el schema —
  caen al 500 default.

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

- **Funciones sin anotar params**: `fn greet(name)` y
  `fn double(n) { return n * 2 }` compilan sin anotaciones. El
  codegen infiere desde el primer call site (`greet("Fitz")` →
  `name: Str`; `double(21)` → `n: Int`) y deriva el return type
  de los `return`/tail-expr del body. Cubre Int/Float/Str/Bool/
  Null/Bytes/Nullable/List/Map/Result + Nominal. Casos sin cubrir
  (raros): fns sin call site, Function/Range/Future como tipo de
  param. Cuando la inferencia falla, el codegen lo dice claro con
  sugerencia de anotar.
- **Heterogéneos con Functions/Tuples** — F13 SPIKE + F13.A + F13.B
  + F13.C + F13.D + F13.E cubren primitivos, Bytes, Nominales,
  listas/mapas anidados con mix interno, HTTP body heterogéneo
  (`body: List<Any>`/`Map<Str, Any>`) y type-check dinámico via
  `.as_int()`/`.as_str()`/`.type_name()`. Lo que aún NO compila
  en heterogéneos: Functions y Tuples. Workaround: `fitz run` o
  desestructurar antes de meter en heterogéneo.
- **Wrap-style middleware** — `fn mw(req, next: Fn() -> Response)` con
  invocación `next()` corre solo en `fitz run`. El codegen rechaza
  con msg claro citando `fitz run`. La implementación en codegen
  requiere emisión de cierre Rust con tipos async/Send/Sync
  recursivos no triviales (~2-3h dedicados) — última deuda
  residual real.

> **Features completas de `fitz build`**: `let X = <expr>` no
> literal a nivel top de un módulo (la RHS puede ser una expresión
> arbitraria que se evalúa lazy via accessor fn), **imports
> transitivos**, **return type inference para handlers**
> (`fn create(u: User) { return ... }` infiere `-> X` del body
> via TypeInfo del checker), **división por cero** literal
> (`print(10 / 0)`) panica con `división por cero` en runtime —
> paridad bit-a-bit con el intérprete, **comparar valores de tipos
> distintos** (`1 == "1"`, `true != 0`) compila y devuelve
> `false`/`true` literal sin error E0308, y **heterogéneos
> completos en `fitz build`** — listas/mapas con primitivos,
> Bytes, Nominales, listas/mapas anidados con mix interno, HTTP
> body `List<Any>`/`Map<Str, Any>` deserializando desde JSON, y
> type-check dinámico via `.as_int()`/`.as_str()`/`.type_name()`
> sobre items de heterogéneos. El caso 95%+ del lenguaje compila
> bit-a-bit con `fitz run`. **Solo falta**: Functions y Tuples en
> heterogéneos. Ver `docs/design-fitzvalue.md` para el diseño.
> **`Value::Bytes` en JSON** se serializa como base64 string
> (estándar de facto). **`File.content`** es `Bytes`, habilita
> uploads binarios (imágenes, PDFs) end-to-end. **`status` codes
> con expresiones simples** (`status: BASE + 4`) se resuelven al
> schema OpenAPI vía const-eval.

Si te tropezás con algo de esta lista, el mensaje del codegen lo
cita explícitamente. La salida tiene la forma:

```
✗ codegen: Error — <descripción> ...
   (Fase 5b soporta un subset progresivo; los mensajes citan el sub-paso correspondiente.)
```

> Lo que **sí soporta**: state HTTP compartido (cualquier
> `let users = [...]` top-level que un handler referencia),
> **paralelismo HTTP real** entre requests con el binario compilado
> (runtime tokio multi-thread default), interop Python en
> `fitz build` (`from python import X` produce binarios nativos
> con PyO3 linkeado). Ver
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

> **Programas con interop Python (`from python import`)**:
> el `fitz build` default produce un binario que linkea contra
> libpython, así que necesita Python instalado en el destino. El
> flag `fitz build --bundle-python` empaqueta CPython embebido
> adentro del binario — el resultado corre standalone sin Python
> en el destino. Tamaño +25-30 MB, pero el deploy se simplifica
> dramáticamente. Detalle en [cap 21.11](#2111-fitz-build---bundle-python--binario-standalone).

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

La interop con Python es **opt-in**: el binario `fitz` pre-compilado
de los releases NO linkea libpython (cero costo si no necesitás la
interop). Para activarla tenés que compilar `fitz` desde fuente con
la feature `python`:

```bash
git clone https://github.com/Thegreekman76/fitz.git
cd fitz
cargo build --release --features python
# El binario queda en target/release/fitz (copialo a tu PATH si querés)
```

O, si preferís no instalarlo global, podés usar `cargo run` directo
contra el repo durante desarrollo:

```bash
cargo run --release --features python -- run mi_app.fitz
```

PyO3 linkea CPython 3.10+ al binario. Los programas Fitz que **no**
usan `from python import` siguen produciendo binarios libres como en
Fase 5b. Los ejemplos del resto del capítulo asumen que tenés un
`fitz` con la feature `python` activa en el `PATH`.

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
la coerción automática:

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

### 21.8b Stubs `.pyi` — dos modos: manual (`fitz py-stubs`) y auto-pickup

Para librerías Python con stubs `.pyi` (typeshed, paquetes
`types-<package>` del PyPI, stubs adjuntos al proyecto, o
escritos a mano), Fitz ofrece dos modalidades complementarias.
Útil para integrar libs comunes (`requests`, `numpy`, etc.) sin
escribir `type` Fitz a mano y con tipado estático real en lugar
de `PyAny` opaco.

**Modo 1 — Manual (`fitz py-stubs`)**: comando que parsea el
`.pyi` y emite un archivo `.fitz` commiteable con los `type`
correspondientes. Útil cuando querés tener los tipos versionados
explícitamente en tu repo y poder editarlos a mano.

```bash
fitz py-stubs http_client.pyi --out http_types.fitz
```

**Modo 2 — Auto-pickup**: si el archivo `.fitz` tiene
`from python import foo` y existe `foo.pyi` adyacente (mismo
directorio), el loader del checker lo detecta
automáticamente, parsea, y registra los tipos al TypeEnv sin
intervención. El user solo tiene que dropear el `.pyi` al lado
del `.fitz` raíz.

```fitz
# main.fitz — sin imports extra, sin sub-comandos manuales
from python import requests

fn fetch_user(id: Int) -> Result<User> {
    return Ok(requests.get_user(id)?)
    # `requests.get_user` resuelve a `fn(Int) -> Result<User>`
    # leído del `requests.pyi` adyacente.
    # `User` resuelve a un nominal real con sus fields del stub.
}
```

Beneficios sobre el modo manual:
- **Cero archivos intermedios**. No genera ni commitea un `.fitz`.
  El `.pyi` es la única fuente de verdad.
- **Lookup automático**. El loader busca `<base_dir>/<name>.pyi`
  para cada `from python import <name>`. `base_dir` = dir del
  `.fitz` que arranca `fitz run`/`fitz build`/`fitz check`.
- **Field access tipado**. Después del auto-pickup,
  `requests.get_user(42)` tipa estáticamente como
  `Result<User>` — el `?` desempaca a `User` y el match exhaustivo
  funciona. Pre-fix tipaba como `Result<Any>` gradual.
- **Silent fallback**. Si el `.pyi` no existe o está malformado,
  el binding vuelve a `PyAny` opaco (comportamiento previo).
  Nunca rompe el build por un stub problemático — emite warning
  a stderr y sigue.
- **Política "el `.fitz` gana"**. Si el programa declara
  `type User { ... }` Y el stub también declara `class User`,
  los fields del `.fitz` ganan (el stub se ignora para ese
  nombre). Permite refinar stubs upstream sin perder control.

Lookup local-only por diseño: el loader solo mira el dir
adyacente al `.fitz` raíz. **No** recorre `PYTHONPATH` ni
`site-packages`. Si querés tipar libs de PyPI, copiá el `.pyi`
al repo (o usá `pip show -f <package> | grep .pyi` para
encontrarlos en el venv). Paridad reproducible entre máquinas;
cero magia ambiente.

Sub-set cubierto (compartido por ambos modos):

- **`class Foo: ...`** con fields anotados → `type Foo { ... }`
  (manual) o nominal en TypeEnv (auto).
- **Tipos primitivos**: `int`, `float`, `str`, `bool`, `bytes`,
  `None` → `Int`, `Float`, `Str`, `Bool`, `Bytes`, `Null`.
- **Generics**: `list[T]`, `dict[K, V]` → `List<T>`, `Map<K, V>`.
- **Nullable**: `Optional[T]` y PEP 604 `T | None` → `T?`.
- **Forward refs**: `"Foo"` (string) → `Foo` nominal.
- **`def name(args) -> ret`** (solo modo auto, 8-pyi.C): se
  registra como callable tipado adentro del módulo. El call
  `foo.name(args)` tipa estáticamente con la firma del stub.
  Auto-wrap: el ret se envuelve en `Result<ret, Str>`
  reflejando el modelo runtime (toda call Python wrappea en
  Result automático, ver §21.5).
- **`var name: type`** top-level (solo modo auto, 8-pyi.C): se
  registra como field directo del módulo. `foo.var` tipa con el
  tipo declarado, sin wrap.

Restricciones (deuda menor compartida):

- **Métodos de class se ignoran**. Hoy solo fields. Métodos
  custom de tipos importados quedan PyAny.
- **Sin sync upstream automático**: regenerar el `.fitz` manual
  o re-dropear el `.pyi` actualizado si la lib cambia.
- **Built-ins de Fitz colisionan** (`Response`, `Request`,
  `File`, etc.). Si el stub tiene un class con esos nombres,
  el loader auto los skipea silenciosamente; con `fitz py-stubs`,
  renombrar manualmente en el `.fitz` generado (ej. `Response`
  → `ApiResponse`).

**¿Cuándo usar cada modo?**

| Caso | Modo |
|---|---|
| Lib estable, quiero versionar los tipos | `fitz py-stubs` (manual) |
| Lib en desarrollo, refresh frecuente | Auto-pickup |
| Querés EDITAR los tipos para refinarlos | `fitz py-stubs` |
| Stub generado por terceros, leés-only | Auto-pickup |
| Quiero `type` Fitz para usar también desde otros `.fitz` | `fitz py-stubs` |
| Solo querés tipado dentro del mismo archivo | Auto-pickup |

Los dos modos son compatibles: podés usar `fitz py-stubs` para
generar un `.fitz` versionado de `requests.pyi`, e independiente
tener un `local_helpers.pyi` adyacente al main.fitz para tipado
de helpers Python rápidos.

Ejemplos runnable:
- Modo manual: [`examples/guide/21b-pyi-stubs.fitz`](../examples/guide/21b-pyi-stubs.fitz).
- Modo auto-pickup: [`examples/guide/21c-pyi-autopickup/`](../examples/guide/21c-pyi-autopickup/) (programa Fitz + `.pyi` adyacente).

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

**Binding intermedio** (paridad bit-a-bit `fitz run` ↔ `fitz
build`): también funciona separar el call y el await en dos
statements, lo cual es útil cuando hay lógica entre los dos
(logging, agregar metadata al future, decidir si esperarlo o no):

```fitz
from python import asyncio

async fn run() -> Result<Null> {
    let fut = asyncio.sleep(0.5)?     // fut: PyAny — coroutine ya construido
    print("sleep arrancado, esperando...")
    let _ = fut.await                  // await sobre la var
    return Ok(null)
}
```

El codegen detecta `inner_ty == PyAny` en el `.await` y despacha
al helper dedicado `__fitz_py_await_obj` (paralelo al
`__fitz_py_invoke_await` del patrón inmediato).

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
fitz build mi_app.fitz
./mi_app  # requiere Python 3.10+ en el PATH
```

Bit-a-bit con `fitz run` para los patrones cubiertos. Coerción
soportada por `fitz build`:

- **`PyAny → primitivo`** (Int/Float/Str/Bool): `let pi: Float =
  math.pi`.
- **`PyAny → List<primitivo>`**: `let xs: List<Int> = json.
  loads(raw)?`.
- **`PyAny → Instance<T>`** y **`PyAny → List<Instance<T>>`**:
  funciona también cuando `T` está **definido en otro módulo Fitz**.
  Main emite los helpers `__fitz_py_to_instance_<T>` para tipos
  custom de módulos transitivos; cada módulo los referencia con
  `crate::__fitz_py_*`.

Lo que **falta** vs `fitz run` (deuda residual menor):

- Coerción Python `dict` → Fitz `Map<K,V>` con K/V no primitivos
  (queda como PyObject opaco; raro en práctica).
- Propagación de expected type adentro de `Ok(...)`: una fn que
  retorna `Result<Str>` y hace `let s = py_call()?; return Ok(s)`
  necesita binding intermedio anotado `let s: Str = py_call()?`.
  Refinable en una pasada futura del checker.

**`from python import` en módulos transitivos**: soportado.
Cada `.fitz` puede declarar sus propios imports Python sin
obligar al main a participar. El codegen reusa los helpers del
preludio Python del crate root via `use crate::__fitz_py_*` y
emite statics + getters locales por módulo. Útil para librerías
Fitz que delegan a Python (sqlalchemy/numpy/redis-py) y quieren
encapsular el detalle. Ver
[examples/python-interop-modular.fitz](../examples/python-interop-modular.fitz)
+ [examples/python_math_utils.fitz](../examples/python_math_utils.fitz)
para el patrón canónico.

Para producir un binario que NO requiera Python en el destino
(realmente standalone), usá `--bundle-python` — sección siguiente.

### 21.11 `fitz build --bundle-python` — binario standalone

`fitz build --bundle-python` produce un binario que internamente
lleva embebida una distribución completa de CPython 3.14.5 (vía
[python-build-standalone](https://github.com/astral-sh/python-build-standalone),
el proyecto que Astral usa para `uv`). El binario resultante NO
requiere Python instalado en el destino — corre en cualquier
máquina del triple soportado, en frío:

```bash
fitz build --bundle-python mi_app.fitz
./mi_app   # corre sin python en el PATH
```

**Cuándo usarlo:**

- Deploys a containers minimalistas (Distroless, scratch + glibc).
- Distribución a usuarios finales que no quieren `pip install`.
- Demos / portable scripts que viajan en un solo archivo.

**Cuándo NO usarlo:**

- Si tu deploy ya garantiza Python instalado (`FROM python:3.X-slim`,
  máquinas con Python pre-configurado): el bundle suma ~30-45 MB
  sin beneficio.
- Si tu programa NO usa `from python import` (el flag aborta con
  error claro — el binario default ya es standalone).

**Tamaños observados** (CPython 3.14.5 install_only_stripped):

| Triple | Binario final | Extract dir (TMPDIR) |
|--------|---------------|----------------------|
| `x86_64-pc-windows-msvc` | ~22 MB | ~61 MB |
| `x86_64-unknown-linux-gnu` | ~35 MB | ~75 MB |
| `x86_64-apple-darwin` | ~24 MB | ~62 MB |
| `aarch64-apple-darwin` | ~24 MB | ~62 MB |
| `aarch64-unknown-linux-gnu` | ~28 MB | ~70 MB |

**Timing observado** (Windows 11 SSD, ejemplo de `math.pi + math.sqrt`):

- Cold first run (cache TMP vacío): **~3-5s** (tar -xzf de 60 MB +
  boot CPython adentro del real binary).
- Warm subsequent runs: **~50-100ms** (cache hit; sentinel
  `.extracted` presente, solo se hace exec del real binary).

**Cómo funciona internamente** (modelo Datasette Desktop):

El output user-facing es UN solo binario, pero internamente lleva
tres piezas embebidas vía `include_bytes!`:

1. **Tarball PBS** (CPython 3.14.5 install_only_stripped del
   triple destino, ~21 MB Windows / ~34 MB Linux x64).
2. **Real binary** (transpile estándar del programa Fitz con
   feature `python` activada, ~180 KB Windows).
3. **Launcher Rust standalone** (sin pyo3, ~200 KB).

El launcher es la entry point del usuario. En primer run:

1. Calcula hash FNV-1a del tarball (16 chars hex).
2. Si `$TMPDIR/fitz-py-<hash>/.extracted` no existe → extrae
   tarball con `tar -xzf` (bsdtar en Win11/macOS, GNU tar en Linux).
3. Setea `PYTHONHOME=<extract-dir>/python` + ENV vars de búsqueda
   de lib según OS:
   - Linux: `LD_LIBRARY_PATH` prepend con `<extract-dir>/python/lib`.
   - macOS: `DYLD_FALLBACK_LIBRARY_PATH` prepend.
   - Windows: `PATH` prepend con `<extract-dir>/python` (donde
     vive `python3.dll`).
4. `exec` del real binary en Unix (proceso reemplazado, signals
   forwarded transparente); `spawn + wait + propagate exit code`
   en Windows.

El real binary arranca con el environment correcto, dlopen
encuentra libpython, PyO3 boot lazy en el primer `Python::attach`,
y todo el resto funciona igual que en `fitz run`/`fitz build`
normal — paridad bit-a-bit.

**Constraint arquitectural permanente** (Linux/macOS):

En Linux y macOS, el real binary linkea contra
`libpython3.X.so.1.0` específica del builder. **No es un bug
cerrable** — es una limitación intrínseca de PyO3 + Linux/glibc:
en esas plataformas NO existe equivalente al `python3.dll`
stable-ABI shim de Windows. El archivo
`/usr/local/lib/libpython3.so` que traen las distros Python es un
dummy de 13 KB sin símbolos del API Python; los símbolos abi3
viven solo en la libpython versioned. Verificado empíricamente
2026-05-24 con experimento Docker cross-version; ver
`docs/deudas_lenguaje.md` sección R.bug-pyo3-abi3-portable-link.

Consecuencia para `--bundle-python`: el bundle PBS trae libpython
3.14.5, entonces el builder necesita tener Python 3.14.x
disponible al momento de `cargo build` para que las versiones
coincidan en runtime. Constraint permanente, no temporal.

En Windows el problema no existe — el real binary linkea contra
`python3.dll` (stable ABI shim real) y cualquier versión 3.10+
del bundle satisface la dependencia.

Setup del builder en Linux/macOS:

```bash
# Ubuntu/Debian:
apt-get install python3.14 python3.14-dev

# macOS con Homebrew:
brew install python@3.14

# Después:
fitz build --bundle-python mi_app.fitz
```

**Cache local del tarball PBS:**

El tarball se descarga UNA vez a `~/.fitz/cache/pbs/` (override
con `FITZ_CACHE_DIR`) y se reusa entre builds. Tamaño ~21 MB
Windows / ~34 MB Linux x64. Descarga vía `curl` (asumimos
disponible en Win11/macOS/Linux moderno).

**Pendientes** (deuda residual de 8.b, NO bloquean uso real):

- Bundling Linux/macOS necesita validación manual end-to-end
  (smoke local hecho en Windows; Linux/macOS lo confirma el
  primer usuario que lo pruebe ahí).
- Bundle más chico vía stdlib stripping de módulos no usados
  (~30% más reducción posible para programas con interop simple).
- Hash criptográfico (SHA256) en lugar de FNV-1a para defender
  contra cambios silenciosos del PBS upstream.

> **Importante — ¿necesito Python local para usar este flag?**
> El binario en el destino corre sin Python instalado (esa es
> la promesa). Pero el dev que BUILDEA con `fitz build
> --bundle-python` sí necesita Python al build time: en Windows
> cualquier 3.10+, en Linux/macOS específicamente 3.14.x
> (constraint arquitectural permanente de PyO3 + Linux/glibc;
> ver `docs/deudas_lenguaje.md`).
> Tabla completa de matices al final del cap 21.12, después del
> flag hermano `--bundle-pip`.

### 21.12 `fitz build --bundle-pip` — empaquetar paquetes pip

El flag `--bundle-python` que vimos arriba empaqueta CPython base
+ stdlib. Para programas Fitz que solo usan stdlib (math, json,
os, datetime, etc.) eso alcanza. Pero el caso de uso real más
común — SQLAlchemy + Postgres, requests, numpy/pandas, FastAPI
client, etc. — requiere **paquetes pip** además de la stdlib.

`fitz build --bundle-pip <paquete>` resuelve eso. Empaqueta los
paquetes pip junto al CPython base, todo adentro del mismo
binario standalone. Implica `--bundle-python` automáticamente.

```bash
# Un paquete
fitz build --bundle-pip requests mi_app.fitz

# Múltiples (flag repetible)
fitz build \
  --bundle-pip sqlalchemy \
  --bundle-pip psycopg2-binary \
  --bundle-pip "redis==5.0.0" \
  mi_app.fitz

# Desde un requirements.txt (cosecha de 8.c, equivalente al
# anterior pero leyendo del archivo):
fitz build --bundle-pip-requirements requirements.txt mi_app.fitz

# Combinable: pip acumula los positionals + el contenido del file
fitz build \
  --bundle-pip-requirements requirements.txt \
  --bundle-pip "psycopg2-binary==2.9.10" \
  mi_app.fitz

# El binario corre sin Python instalado, sin `pip install`,
# sin requirements.txt en el destino.
./mi_app
```

> **`--bundle-pip-requirements <FILE>` es repetible** (acepta
> varios archivos: `requirements.txt` + `requirements-prod.txt`).
> El archivo se pasa directo a `pip install -r <file>`, así que
> toda la sintaxis nativa de pip funciona sin cambios: comentarios
> con `#`, includes `-r other.txt`, version pins, `--hash`,
> índices alternos con `--index-url`, etc. Validación temprana:
> si el archivo no existe o no se puede leer, `fitz build`
> aborta antes de tocar PBS o pip.

**Cómo funciona internamente:**

1. Build time: descarga el tarball PBS al cache (igual que
   `--bundle-python`).
2. Extrae PBS a `target/fitz-build/<bin>_pbs_extract/` para tener
   un Python ejecutable adentro del proyecto.
3. Ejecuta `<pbs>/python -m pip install --target <dir> <paquetes>`
   adentro de `target/fitz-build/<bin>_pip_packages/`. El
   `--target` instala los paquetes sin tocar el sistema ni el
   venv del usuario.
4. Empaca el directorio resultante en
   `target/fitz-build/<bin>_pip_packages.tar.gz`.
5. El launcher embebe **dos tarballs** vía `include_bytes!`: el
   PBS base (compartido entre proyectos) + el pip packages
   (específico de este proyecto).
6. En primer run del binario, el launcher extrae ambos al
   `$TMPDIR/fitz-py-<hash>/` (el hash incluye los bytes del pip
   tarball, así que proyectos con distintos paquetes tienen
   distintos extract dirs — sin colisión).

**Tamaños y timing observados** (Windows 11 SSD):

| Bundle | Tamaño bin | Cold first run | Warm runs |
|--------|------------|----------------|-----------|
| Solo `--bundle-python` (stdlib) | ~22 MB | ~3-5s | ~50-100ms |
| `+ --bundle-pip requests` | ~23 MB | ~5-7s | ~50-100ms |
| `+ --bundle-pip sqlalchemy + psycopg2-binary` | ~50 MB | ~8-12s | ~50-100ms |

**Programa de ejemplo runnable**:

```fitz
// examples/python-interop-8.c.fitz
from python import requests

print("Módulo requests cargado desde el bundle pip:")
print(requests.__name__)
print(requests.__version__)
```

```bash
$ fitz build --bundle-pip requests examples/python-interop-8.c.fitz
→ compilando real binary…
→ asegurando PBS tarball (cpython 3.14.5 / x86_64-pc-windows-msvc)…
→ extrayendo PBS al cache local para correr pip (1 paquete(s))…
→ pip install --target (1 paquete(s))…
→ empacando pip_packages.tar.gz…
→ compilando launcher…
✓ binario standalone (CPython 3.14.5 + 1 pip pkg(s) embebidos):
  python-interop-8.c.exe (22.9 MB)

# En cualquier máquina del triple destino, SIN Python instalado:
$ ./python-interop-8.c.exe
Módulo requests cargado desde el bundle pip:
requests
2.34.2
```

**Limitaciones y caveats:**

- **Paquetes con C extensions nativas** (psycopg2-binary, numpy,
  Pillow): funcionan, pero el `.whl` que pip elige al build time
  es **específico del triple del builder**. Si buildeás en
  `x86_64-pc-windows-msvc` y querés correr en
  `x86_64-unknown-linux-gnu`, las extensions no son portables.
  Workaround: buildea adentro de Docker con la imagen del target
  (la imagen oficial `ghcr.io/thegreekman76/fitz:latest-python`
  es Linux x64).
- **Pure-Python packages** (requests, sqlalchemy, click, jinja2):
  son **cross-platform** — el mismo bundle corre en Linux,
  macOS y Windows porque son `.py` puros sin C extensions.
- **`pip install` requiere red al BUILD time**. Una vez que el
  binario está hecho, el deploy no requiere red.
- **Versions pin**: usá la sintaxis nativa de pip:
  `--bundle-pip "sqlalchemy==2.0.0"` o
  `--bundle-pip "redis>=5.0,<6.0"`.
- **Builder requiere `tar`**: igual que `--bundle-python`. En
  Windows 11 viene nativo (`C:\Windows\system32\tar.exe`).
- **Constraint heredado de `--bundle-python`**: en Linux/macOS,
  el builder debe tener Python 3.14.x para que `pip install`
  use el mismo intérprete que el bundle. En Windows el shim
  `python3.dll` permite cualquier 3.10+.

**Casos de uso reales del feature:**

- **Distribución a usuarios finales** de scripts CLI que usan
  bibliotecas Python ricas (yt-dlp, beautifulsoup4, etc.): un
  solo binario en lugar de `pip install` + lista de
  dependencies.
- **Deploys a containers minimalistas**: `FROM scratch` o
  `FROM gcr.io/distroless/cc-debian12` en lugar de
  `FROM python:3.X-slim`. Imagen ~150 MB → ~80-100 MB.
- **Tooling interno** de equipos donde no querés que cada user
  configure su venv ni instale `requirements.txt`.

> **Para boilerplates 5/6 (api-postgres-python,
> api-fullstack-postgres) este es exactamente el feature que
> faltaba**. La sección "Roadmap del boilerplate" en cada uno
> tiene el plan de simplificación cuando este flag llegue.

#### ¿Entonces ya no necesito Python ni pip ni venv?

Pregunta razonable y la respuesta tiene matices. Depende de
QUIÉN y CUÁNDO:

| Contexto | Python local | `pip install` local | venv |
|----------|--------------|---------------------|------|
| **Correr el binario** `--bundle-pip` (deploy) | ❌ No | ❌ No | ❌ No |
| **Buildear con `fitz build --bundle-pip`** | ✅ Sí (3.14.x en Linux/macOS) | ❌ No | ❌ No |
| **Iterar con `fitz run --features python`** | ✅ Sí | ✅ Sí | ⚠️ Opcional |

**Para el usuario final** que corre el binario en producción:
**cero Python, cero pip, cero venv.** Copia el binario, lo
ejecuta, funciona. Es la promesa del binario standalone llevada
hasta el final del stack — sin runtime externo, sin packages
adicionales que instalar.

**Para el developer que buildea** con `fitz build --bundle-pip`:
**casi, pero no del todo.** El build necesita:

- Un Python instalado en la máquina (Windows: cualquier 3.10+
  por el shim `python3.dll`; Linux/macOS: 3.14.x específicamente
  por el constraint arquitectural permanente de PyO3 +
  Linux/glibc). Esto es estructural, no temporal.
- **NO** necesita `pip install <paquete>` local — `fitz build`
  los baja al cache y los empaqueta solo.
- **NO** necesita venv activado.
- Sí necesita `tar` y `curl` en el `PATH` (vienen nativos en
  Win11/macOS/Linux moderno).

**Para el developer iterando** con `fitz run --features python`
(intérprete, NO compilador): **no cambia — sigue necesitando lo
de siempre.** Python instalado, paquetes pip instalados
localmente, venv si querés aislamiento. `fitz run` es el modo
de iteración rápida; no bundlea nada, igual que `python
script.py` necesita su environment.

**Conclusión práctica**: el feature destraba un escenario
específico — **el binario en el destino es standalone**. El
flow de desarrollo (escribir código, iterar, debuggear) sigue
necesitando Python local porque `fitz run` no compila.

### 21.13 Ejemplo CRUD ejecutable

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
  fitz run examples/guide/21-python-crud/app.fitz

# Windows PowerShell:
$env:PYTHONPATH = "examples\guide\21-python-crud"
fitz run examples/guide/21-python-crud/app.fitz
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

### 21.14 Limitaciones — lo que NO anda

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
Fitz tiene su propio **Language Server Protocol** (LSP) y una
**extensión VSCode** que lo aprovecha.

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
  `assert_ne`/`assert_throws`), labels de loops (`'outer`),
  operadores compuestos (`+=`/`-=`/...) y rangos inclusivos (`..=`).
- **Diagnostics en vivo** — los errores del lexer, parser y type
  checker aparecen subrayados en rojo al tipear, con el mismo
  mensaje + sugerencia que `fitz check` muestra en la terminal.
  Pipeline: tokenize → parser tolerante a buffer en construcción
  → type checker. Severity `ERROR`, source `"fitz"` (visible en
  el Problems panel de VSCode).
  Desde v0.19.3, el pipeline también pre-scanea las imports
  directas del archivo abierto para resolver decoradores
  cross-module: `@authenticated`/`@admin`/`@requires("role")`
  contra un `@auth_provider` declarado en `auth.fitz` (importado
  vía `import auth` o `from auth import current_user`), y
  `spawn(<imp_fn>(...))` contra `@background` fns declaradas en
  módulos hermanos. Paridad bit-a-bit con `fitz check`/`build`/
  `run` — ya no aparecen falsos positivos del estilo
  `no @auth_provider registered in the program` en apps
  multi-archivo. Alcance MVP: un nivel de profundidad (la misma
  política W12/B10 del CLI); transitive imports queda como deuda
  refinable si aparece demanda.
- **Hover con tipos** — pasás el mouse sobre una variable o
  expresión y aparece su tipo en un tooltip (renderizado como
  bloque de código Fitz, con syntax highlighting). El símbolo bajo
  el cursor se **highlightea visualmente** (el Range del hover
  cubre exactamente el ident).
  Funciona sobre literales (`42` → `Int`), identificadores en uso
  (`nombre` → `Str`), expresiones compuestas (`xs.map(...)` →
  `List<U>`), tipos nominales (`u` → `User`). Heurística
  pragmática: el último Expr iniciado antes del cursor en la
  misma línea.
- **Go-to-definition** — F12 (o Ctrl+Click) sobre el uso de una
  variable, función o tipo te lleva a la línea de su declaración.
  Funciona sobre variables locales (`let x = ...`), funciones
  top-level (`fn foo(...)`), tipos custom (`type User { ... }`),
  parámetros de fn, variables del `for ... in`, bindings de
  `match` (`Ok(x)`, `Err(e)`, `Ident pat`), e imports (incluido
  **cross-module**: F12 sobre `User` cuando el import es
  `from foo import User` salta a la línea del `type User { ... }`
  adentro de `foo.fitz`, no al stmt de import local). Los
  built-ins (`print`/`len`/`sleep`/`cors`) no resuelven (no hay
  archivo donde saltar).
- **Autocomplete contextual** — al tipear, VSCode te muestra una
  lista de sugerencias según el contexto:
  - Tras `.` (caso *after-dot*): si el receiver es un tipo custom
    (`u: User`), aparecen sus fields **y sus métodos custom**
    (firma `fn(T1, T2) -> Ret` o `async fn(...) -> Ret` en el
    detail). Si es `List<T>`, sus 9 métodos built-in
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
    `fn`/`if`/`match`/...). **Vars locales y params del scope
    contenedor también aparecen** (scope-aware): si estás tipeando
    adentro del body de `fn greet(name: Str) { ... }`, `name`
    aparece en la lista; `let local = 42` previo también;
    `for item in xs { ... }` agrega `item` dentro del body.

### En archivos `.fitzv`

Desde v0.21.3 (Phase 11.8 CERRADO), la extensión + el binario
`fitz-lsp` reconocen `.fitzv` como surface de primera clase.
El pipeline es distinto al de classic Fitz — el LSP routea
por extensión: `.fitzv` → view lexer + parser + expand + check;
`.fitz` sigue por el path classic.

Las 4 capabilities disponibles hoy:

- **Diagnostics en vivo** — errores del view lexer/parser/
  expand/check aparecen subrayados en rojo al tipear en un
  `.fitzv`. Los tres error kinds del pipeline view mapean a
  `FitzError` shape: `ViewParseError` → syntax error;
  `ExpandError` → syntax error (con context — nombre del
  componente + section); `CheckError` → type mismatch (con
  context). Severity `ERROR`, source `"fitz"`.
- **Completions** — cuatro clases contextuales:
  1. **Template directives** tras `{` o `{#` — sugiere `#if`,
     `#for`, `#else`, `/if`, `/for` como SNIPPETs con tabstops
     (`$1`, `$0`) para completar los placeholders con Tab.
  2. **Event decorators** tras `@` en attribute position —
     `click`, `submit` con SNIPPET `="$1"` tail.
  3. **State field names** del enclosing component.
  4. **Event handler names** del enclosing component.

  El scan del source es **robusto a parses parciales** (heurística
  line-by-line con brace-depth tracking): mientras escribís un
  `{...` sin cerrar, las suggestions siguen apareciendo — no
  necesitás terminar la expresión para que el LSP funcione.
- **Hover** sobre bare ident del cursor:
  - Sobre state field ref (`{count}` en un template) →
    `count: Int — **state field** of App`.
  - Sobre event handler ref (`data-flv-click="inc"`) →
    `event inc() — **event handler** of App`.
  - Keywords (`component`, `state`, `event`, `from`, `import`,
    `as`, `template`, `style`) no devuelven hover (evita false
    positives).
- **Go-to-definition** — F12/Ctrl+Click salta a la línea de
  declaración con column al inicio del ident:
  - State field ref → salto a `<name>: <type>` line en el
    `state { ... }` block.
  - Event handler ref → salto a `event <name>(...)` line.
  - Component boundary respect — si el cursor está en un
    componente `App` y otro sibling también tiene el mismo
    field name, F12 apunta al de `App` (no al sibling).

**Lo que no está en el MVP de 11.8**:

- **Cross-module symbol lookup** — F12/hover sobre un ident
  importado por `from X import Y` no salta al target module
  hoy. Refinable con plumbing paralelo al
  `resolve_cross_module_definition` del path classic.
- **TypeInfo-based hover** — la MVP usa scan heurístico del
  source; hover sobre complex expr shapes (BinOp, method
  calls) puede quedar sin hint. Refinable con integración
  full del classic checker corriendo sobre el emitted classic
  Fitz surface.
- **Fine-grained context routing en completion** — el MVP no
  distingue "cursor inside template" vs "inside state block".
  Suggestions siempre correctas pero pueden aparecer en
  contexts adjacentes.
- **Signature help / rename / references** dentro de `.fitzv`
  — no implementados.

Ver el capítulo 36 (Frontend nativo con `.fitzv`) para el
detalle del lenguaje que estos LSP capabilities describen.

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

Hay tres modalidades, ordenadas de menor a mayor esfuerzo:

#### A) Bajar el `.vsix` de releases (recomendado para uso normal)

Cada tag `vX.Y.Z` publica los `.vsix` per-plataforma como assets
del release en GitHub. Sin compilar nada local.

1. Andá a [releases](https://github.com/Thegreekman76/fitz/releases/latest)
   y bajá el `.vsix` correspondiente a tu sistema:

   | Plataforma                        | Archivo                            |
   |-----------------------------------|------------------------------------|
   | Windows x64                       | `fitz-lang-win32-x64.vsix`         |
   | Linux x64                         | `fitz-lang-linux-x64.vsix`         |
   | Linux ARM64                       | `fitz-lang-linux-arm64.vsix`       |
   | macOS Apple Silicon (M1/M2/M3)    | `fitz-lang-darwin-arm64.vsix`      |

   Cada `.vsix` trae el `fitz-lsp` ya compilado adentro
   (`server/fitz-lsp[.exe]`). No necesitás Rust ni Node.

2. Instalalo en VSCode. Dos opciones equivalentes:

   **Desde la UI**:
   - `Ctrl+Shift+P` (o `Cmd+Shift+P` en macOS) → "Extensions: Install from VSIX..."
   - Seleccioná el archivo bajado.

   **Desde la terminal**:
   ```bash
   code --install-extension fitz-lang-<plataforma>.vsix --force
   ```

3. Abrí cualquier `.fitz` y vas a ver highlighting + diagnostics +
   hover + go-to-def + autocomplete funcionando. Cero settings extra.

> Las plataformas en la matriz de release son las 4 listadas arriba.
> **macOS Intel (`darwin-x64`)** y **Windows ARM64 (`win32-arm64`)**
> no están: el primero por escasez crónica de runners macos-13 en
> GitHub Actions, el segundo porque axum aún no compila estable en
> ese target. Si necesitás alguna de esas, build local (sección B)
> funciona idéntico.
>
> **¿Por qué no desde el VSCode Marketplace?** Publicar al
> Marketplace requiere cuenta de publisher de Microsoft (vía Azure
> DevOps con PAT) — todavía no está creada. Cuando se cree, la
> instalación se va a hacer en un clic desde la UI de Extensions de
> VSCode buscando `fitz` (la sección A se reemplazará por ese
> camino; releases en GitHub van a quedar como backup para air-gapped
> / corporate networks que bloquean el Marketplace).

#### B) Build local — `.vsix` para tu plataforma actual

Si querés trackear `main` o no encontrás tu plataforma en releases:

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

Después lo instalás como en (A):

```bash
code --install-extension editors/vscode/fitz-language-*.vsix --force
```

Para empaquetar para **otra plataforma** (cross-compile, requiere
`rustup target add <triple>` previo):

```bash
node scripts/build-vsix.mjs --target linux-x64
node scripts/build-vsix.mjs --target darwin-arm64
# Plataformas soportadas: win32-x64, win32-arm64, linux-x64,
# linux-arm64, darwin-x64, darwin-arm64
```

#### C) Manual (alfa / desarrollo) — `fitz-lsp` en PATH o setting

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

Es **production-ready**: preserva los comentarios y blank lines
del usuario, no solo el código.

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
3 frameworks.

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

### Límites de recursos (un test no te cuelga la suite)

Un test con un `loop {}` sin salida o una recursión sin caso base es
siempre un bug. Para que ese bug **falle limpio** en vez de colgar el
runner o crashear el proceso, `fitz test` corre cada test con un
presupuesto de recursos:

- Un tope de **pasos** por iteración de loop (`while`/`loop`/`for`) —
  ataja el `loop {}` CPU-bound.
- Un tope de **profundidad de recursión** — ataja la recursión
  infinita y la reporta como un fallo con mensaje claro.

Cuando un test excede alguno de los dos, se reporta como `FAILED` con
un error explicando qué límite se cruzó. **`fitz run` no tiene estos
límites** — tus servidores, handlers `@ws` y jobs `@cron` corren con
sus loops infinitos legítimos sin tocar.

Los defaults son generosos (mucho más de lo que cualquier test honesto
necesita). Si tenés un test legítimamente muy pesado o muy recursivo,
subí los topes con variables de entorno:

```bash
FITZ_MAX_STEPS=200000000 fitz test    # más iteraciones de loop
FITZ_MAX_DEPTH=8000 fitz test         # recursión más profunda
```

El REPL (`fitz repl`) aplica el mismo presupuesto por evaluación, con
las mismas variables.

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
del developer: editás, guardás, ves el efecto, repetís.

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

### Modo wasm-client (`.fitzv` → WASM)

Cuando el bin default de tu `fitz.toml` apunta a `wasm-client` (un
componente `.fitzv` que compila a WebAssembly, cap 36), `fitz dev`
cambia de estrategia: en vez de kill+respawn, **buildea el bundle y
lo sirve con auto-refresh en el browser**.

```bash
# En un proyecto con un [[bin]] target = "wasm-client":
fitz dev              # sirve en http://127.0.0.1:1234/
fitz dev --port 8080  # otro puerto
fitz dev --bin web    # elegí el bin en un proyecto multi-bin
```

Qué hace en este modo:

- **Build incremental** con `wasm-pack --dev` (sin `wasm-opt`, mucho
  más rápido que el `--release` de `fitz build`; reusa el crate
  estable en `target/wasm-build/` así la cache de cargo queda
  caliente — el primer build compila las deps, los siguientes son de
  ~1-2 segundos).
- **Dev server** en `127.0.0.1:<port>` que sirve el root de tu
  proyecto igual que `python -m http.server`: tu `index.html`, tu
  CSS, y el bundle en `target/wasm/<bin>/` resuelven tal cual. Si no
  tenés `index.html`, genera uno mínimo con el elemento del `mount`.
- **Live reload por WebSocket**: al guardar un `.fitzv`/`.fitz`/
  `fitz.toml`, rebuildea y el browser se recarga solo. Sin refrescar
  a mano.
- **Preservación de state**: el estado vivo del componente (un
  contador, texto en un input, una lista, un mapa, un tipo custom)
  **sobrevive el reload** — editás el template y no perdés el estado
  con el que estabas probando.
- **Re-resolución del manifest en vivo**: si editás `fitz.toml` —
  repuntás `[bin].main`, agregás una `[dependencies]`, o cambiás
  `[flags]` — se toma sin reiniciar. Un `fitz.toml` roto a mitad de
  edición imprime el error y sigue sirviendo el bundle anterior (se
  recupera al próximo save válido). Renombrar el bin o cambiar su
  `mount` sí requiere reiniciar `fitz dev` (imprime una nota).

Es "Approach C": reload rápido con auto-refresh, no un runtime de
template que aplique diffs sin recompilar (eso es un norte futuro más
grande). Para el 90% del loop de edición visual, el efecto es el
mismo: guardás, ves el cambio en un par de segundos, sin perder el
estado.

> **Nota:** este modo requiere un manifest con un `[[bin]] target =
> "wasm-client"` (necesita `mount` + el layout de salida). Corré
> `fitz dev` desde el directorio del proyecto (sin `--file`).

**Proyectos fullstack (`server` + `web`).** En un proyecto con dos
bins — un backend native (con `@rpc`, cap 36) y un frontend
`wasm-client` — usá `--bin` para elegir cuál dev-ear, en dos
terminales:

```bash
# Terminal 1 — el backend @rpc (bin native)
fitz run --bin server

# Terminal 2 — el frontend wasm con hot reload
fitz dev --bin web
```

`--bin <nombre>` también existe en `fitz run` (y `fitz build`), así
que podés seleccionar cualquier bin de un manifest multi-bin. Sin
`--bin`, un proyecto multi-bin es ambiguo y `fitz run`/`fitz dev`
piden que elijas uno.

### Lo que NO anda todavía

- **Incremental rebuild en el modo clásico** (native/SSR): hoy es
  kill+respawn full. Mejora cuando aparezca el modelo de módulos
  pre-compilados. (El modo wasm-client sí hace build incremental —
  ver arriba.)
- **Browser auto-refresh para HTTP nativo** (inyectar WebSocket en
  respuestas del server Fitz): NO en MVP. Quien edite HTML/CSS junto
  al backend Fitz puede usar herramientas separadas (Live Server,
  etc.). El auto-refresh **sí** existe para el modo wasm-client
  (ver arriba).
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
que vive en Python/Node/Ruby.

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

### Límites de recursos

Cada evaluación del REPL corre con el mismo presupuesto de recursos que
`fitz test`: si tipeás un `loop {}` sin salida o una recursión infinita,
la línea aborta con un error claro en vez de colgar la sesión. Ajustable
con `FITZ_MAX_STEPS` / `FITZ_MAX_DEPTH` (ver el capítulo de `fitz test`).

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

## 28. Auth nativa

En FastAPI/Spring/ASP.NET/Express la auth se resuelve con librerías
opcionales: `fastapi-users` + `python-jose` + `passlib` en Python;
`@PreAuthorize` de Spring AOP + reflection en Java; `passport.js` +
`bcrypt` + `jsonwebtoken` en Node. Cinco dependencias mínimo,
configuración manual, y validación que sucede en runtime —
descubrís que tu handler protegido no recibe el `user` cuando ya
está corriendo en prod.

En Fitz auth es **parte del lenguaje**: tres decoradores
(`@auth_provider`, `@authenticated`, `@admin`), dos módulos
built-in (`jwt`, `hash`), y el type checker valida en compile-time
que cada handler protegido reciba un `user` del tipo correcto. Cero
`fitz add jsonwebtoken` / `argon2` / nada. El esquema OpenAPI 3.1
generado documenta automáticamente el requirement de bearer token
y los códigos 401/403. Y todo eso vale igual en `fitz run` y en el
binario nativo de `fitz build` — paridad bit-a-bit, deploy
standalone sin instalar runtime.

### Las piezas

**`@auth_provider`** — singleton del programa. Una fn que toma los
headers HTTP del request, decide si el usuario está autenticado, y
devuelve el `User` correspondiente:

```fitz
type User { id: Int, email: Str, name: Str, role: Str }

@auth_provider
fn check_token(headers: Map<Str, Str>) -> Result<User> {
    let auth = headers.get("authorization")?
    let claims = jwt.decode(auth, "secret")?
    let email = claims["email"]
    // ... lookup en DB / hardcoded ... devuelve `Ok(User { ... })`
    return Err("auth inválida")
}
```

El checker valida estáticamente:

- Que el provider tenga exactamente un param `Map<Str, Str>`.
- Que retorne `Result<T>` donde `T` es un type custom (nominal).
- Que sea único por programa (un solo provider; otro = error).

**`@authenticated`** — sobre un handler HTTP. El runtime corre el
provider antes del handler; si devuelve `Err`, responde **401** con
`{"error": <msg>}` y el handler no ejecuta. Si devuelve `Ok(user)`,
el `user` se inyecta como argumento del handler:

```fitz
@authenticated
@get("/me")
fn me(user: User) -> User => user
```

**`@admin`** — shorthand de `@authenticated` + check `user.role
== "admin"`. Si el provider autentica pero el rol no es `"admin"`,
el runtime responde **403**. El checker exige que el tipo `User`
(retornado por el provider) tenga campo `role: Str`:

```fitz
@admin
@get("/admin/dashboard")
fn dashboard(user: User) -> Str => "hola {user.name}"
```

**`@requires("role")`** (Fase 9.w.1.iter2) — RBAC custom apilable
para roles más allá de `admin`. El runtime ejecuta el provider e
inyecta el `user`; después verifica que `user.role` matchee al
menos uno de los roles requeridos. Si no, **403** con el role
actual y los requeridos en el mensaje. Apilable para OR:

```fitz
@requires("editor")
@post("/articles")
fn create(body: Article, user: User) -> Article {
    // Solo si user.role == "editor"
    return body
}

@requires("editor")
@requires("publisher")
@put("/articles/{id}")
fn publish(id: Int, user: User) -> Article {
    // Si user.role == "editor" O user.role == "publisher"
    ...
}
```

Como `@admin`, `@requires` exige `role: Str` en el `User` type
del provider (no nullable). Es independiente de `@authenticated`
— pero implica auth (el wrapper corre el provider igual). El
mensaje de 403 cita el role actual del user y la lista de roles
requeridos, útil para debug y observabilidad.

**`jwt`** — módulo built-in. Siempre disponible, sin import.

```fitz
let token = jwt.encode({"sub": "u42", "role": "admin"}, "mi-secret")
let claims = jwt.decode(token, "mi-secret")?  // Result<Map<Str, Str>>
```

HS256 default; opcional pasar `"HS384"`/`"HS512"` como tercer arg.
`encode` devuelve el JWT firmado como `Str`; `decode` siempre
devuelve `Result<Map<Str, Str>>` — token malformado, signature
inválida, expirado son runtime events que el caller maneja con
`match` o `?`.

**`hash`** — módulo built-in. Argon2id (recomendación OWASP), salt
aleatorio por hash, output en formato PHC string listo para
guardar en DB:

```fitz
let hashed = hash.password("supersecret")
// → "$argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>"
let ok = hash.verify("supersecret", hashed)  // Bool
```

`verify` siempre devuelve `Bool` (no `Result`): hash malformado o
mismatch → `false` por seguridad, no se filtra info al attacker.

**`auth`** (Fase 9.w.1.iter2.b) — módulo built-in para token
revocation. 3 builtins async que operan sobre Postgres con la tabla
`fitz_token_blacklist(jti TEXT PRIMARY KEY, expires_at BIGINT NOT NULL)`
auto-creada al primer call:

```fitz
// Revocar un token (típico: handler /auth/logout):
auth.blacklist(db, jti, exp).await?     // Result<Null>

// Chequear si un token está revocado (típico: @auth_provider):
let revoked = auth.is_blacklisted(db, jti).await?   // Result<Bool>

// Limpiar tokens vencidos (típico: @cron periódico):
let deleted = auth.cleanup_expired(db).await?   // Result<Int>
```

**Decisiones de diseño**:

- **`expires_at` como Unix epoch** (BIGINT): el JWT `exp` claim ya
  viene como timestamp Unix, evita conversiones.
- **Auto-filtrado de tokens vencidos** en `is_blacklisted`: el SQL
  usa `expires_at > extract(epoch from now())`. Tokens con `exp`
  pasado cuentan como NO blacklisted (el `jwt.decode` los rechaza
  primero por expirados).
- **`ON CONFLICT DO UPDATE`** en blacklist: re-blacklistear el mismo
  jti actualiza `expires_at` sin fallar (caso raro pero posible).
- **Tabla auto-creada con `CREATE TABLE IF NOT EXISTS`** al primer
  call de cualquier builtin — idempotente, Postgres serializa con
  LOCK interno.
- **Paridad bit-a-bit `fitz run` ↔ `fitz build`** — el codegen emite
  helpers `__fitz_auth_*` que comparten el mismo SQL que el
  intérprete.

**Patrón canónico — `/auth/logout` + `/auth/refresh` + provider**:

Los endpoints se escriben **a mano** (no hay auto-mount como
`/healthz`); el lenguaje provee los builtins, el user el flow. Lean
y honesto: cubre el caso 90% en ~30 LoC.

```fitz
type User { id: Int, name: Str, role: Str }
type LoginRequest { email: Str, password: Str }
type LoginResponse { token: Str }

@auth_provider
async fn check(headers: Map<Str, Str>) -> Result<User> {
    let token = extract_bearer(headers)?
    let claims = jwt.decode(token, secret(), "HS256")?
    let jti = claims.get("jti")?

    // CHEQUEO DE BLACKLIST. Si el token fue revocado, rechazá.
    if auth.is_blacklisted(db, jti).await? {
        return Err("token revocado")
    }

    let user_id = claims.get("sub")?
    return resolve_user_from_db(user_id).await
}

// El payload del JWT en `fitz build` MVP es `Map<Str, Str>` strict, así
// que numéricos van serializados a string. En `fitz run` la heterogeneidad
// pasa, pero usamos el shape estricto para mantener paridad bit-a-bit.

@post("/auth/login")
async fn login(body: LoginRequest) -> Result<LoginResponse> {
    let user = verify_credentials(body.email, body.password).await?
    let jti = Uuid.v4().to_str()  // UUID → Str para el payload
    let exp = DateTime.now().timestamp() + 3600 * 24  // 24h validity
    let token = jwt.encode({
        "sub": "{user.id}",
        "jti": jti,
        "exp": "{exp}",
        "role": user.role,
    }, secret())
    return Ok(LoginResponse { token: token })
}

@authenticated
@post("/auth/logout")
async fn logout(user: User, headers: Map<Str, Str>) -> Result<Null> {
    let token = extract_bearer(headers)?
    let claims = jwt.decode(token, secret())?
    let jti = claims.get("jti")?
    // Bloqueamos el jti con una TTL fresca de 24h. Cuando el JWT expira,
    // `jwt.decode` ya lo rechaza por expirado; la blacklist solo previene
    // reuse del mismo jti antes de exp. `auth.blacklist` exige Int para
    // `expires_at`, así que NO leemos exp del JWT (vendría como Str).
    let ttl = DateTime.now().timestamp() + 3600 * 24
    auth.blacklist(db, jti, ttl).await?
    return Ok(null)
}

@authenticated
@post("/auth/refresh")
async fn refresh(user: User, headers: Map<Str, Str>) -> Result<LoginResponse> {
    // Estrategia simple: revocar el token actual y emitir uno nuevo.
    let token = extract_bearer(headers)?
    let old_claims = jwt.decode(token, secret())?
    let ttl = DateTime.now().timestamp() + 3600 * 24
    auth.blacklist(db, old_claims.get("jti")?, ttl).await?

    let new_jti = Uuid.v4().to_str()
    let new_exp = DateTime.now().timestamp() + 3600 * 24
    let new_token = jwt.encode({
        "sub": "{user.id}",
        "jti": new_jti,
        "exp": "{new_exp}",
        "role": user.role,
    }, secret())
    return Ok(LoginResponse { token: new_token })
}

// Cleanup periódico de tokens vencidos.
@cron("0 0 3 * * *")  // 3 AM daily
async fn cleanup_blacklist() -> Result<Null> {
    let deleted = auth.cleanup_expired(db).await?
    log.info("blacklist cleanup", deleted_rows: deleted)
    return Ok(null)
}
```

**Lo que NO hace el MVP de 9.w.1.iter2.b**:

- **Auto-mount de `/auth/logout` y `/auth/refresh`** — los escribís a
  mano (~10 LoC cada uno). Razón: el flow exacto (qué returneás, qué
  validás, qué loguéas) varía por proyecto. Auto-mount sería opinión
  fuerte que después es difícil de deshacer.
- **`auth.blacklist` síncrono** (sin DB) — el MVP requiere Postgres
  vivo. Para in-memory rápido, usá un `Map<Str, Int>` global y
  chequeálo a mano en el `@auth_provider`. Trade-off: no persiste
  entre restarts.
- **Token rotation con refresh tokens dedicados** — el MVP usa un
  solo token largo (~24h) que se reemplaza en `/auth/refresh`.
  Refresh tokens dedicados (modelo OAuth2 clásico) quedan como
  pattern futuro si entra demanda.

### Ejemplo completo: login + /me + /admin

`examples/guide/28-auth.fitz` arma una API mini-realista con los
tres roles que cubren la mayoría de casos reales:

- `POST /login` — público; recibe credenciales, verifica el
  password contra el hash almacenado con `hash.verify(...)`, y
  devuelve un JWT firmado con `jwt.encode(...)`.
- `GET /me` — requiere `@authenticated`; devuelve el `user`
  inyectado.
- `GET /admin/users` — requiere `@admin`; devuelve la lista de
  todos los usuarios. Si el token es de un user con rol distinto a
  `"admin"`, 403.

El ejemplo entero compila a binario nativo con `fitz build`. Una
sesión típica:

```bash
# 1. Login para obtener un token.
$ curl -X POST localhost:43928/login \
       -H 'Content-Type: application/json' \
       -d '{"email":"ada@example.com","password":"secret-ada-123"}'
{"token":"eyJ0eXAiOi..."}

# 2. /me con el token.
$ curl localhost:43928/me -H 'Authorization: Bearer eyJ0eXAiOi...'
{"id":1,"email":"ada@example.com","name":"Ada","role":"admin"}

# 3. /admin/users con token de admin → 200.
$ curl localhost:43928/admin/users -H 'Authorization: Bearer eyJ0eXAiOi...'
[{"id":1,"email":"ada@example.com",...},{"id":2,...}]

# 4. /admin/users con token de user-rol → 403.
$ curl localhost:43928/admin/users -H 'Authorization: Bearer <alan-token>'
{"error":"acceso prohibido — se requiere rol admin"}

# 5. /me sin token → 401.
$ curl localhost:43928/me
{"error":"falta header Authorization"}
```

El ejemplo combina **todo el stack** en menos de 100 líneas: tipos
Fitz (`type User { ... }`), decoradores apilados (`@auth_provider`
+ `@authenticated`/`@admin` + `@get`/`@post`), body HTTP
deserializado a tipos custom (`Credentials`), `Result<T>` con `?`,
`return <status> { ... }` para emitir status codes explícitos,
JWT firmado y verificado con HS256, password hashing con Argon2id.
Cero `pip install`, cero `cargo add`, cero `npm install`.

### Integración con OpenAPI

Cuando el programa declara `@authenticated`/`@admin`, el schema
OpenAPI generado en `/openapi.json` agrega automáticamente:

```json
{
  "components": {
    "securitySchemes": {
      "bearerAuth": {
        "type": "http",
        "scheme": "bearer",
        "bearerFormat": "JWT"
      }
    }
  },
  "paths": {
    "/me": {
      "get": {
        "security": [{"bearerAuth": []}],
        "responses": {
          "200": { "...": "..." },
          "401": { "...": "..." }
        }
      }
    },
    "/admin/users": {
      "get": {
        "security": [{"bearerAuth": []}],
        "responses": {
          "200": { "...": "..." },
          "401": { "...": "..." },
          "403": { "...": "..." }
        }
      }
    }
  }
}
```

La UI de Scalar en `/docs` muestra el lock icon en cada handler
protegido y un campo para pegar el bearer token — listo para
probar la API sin escribir un solo cliente.

### Por qué Fitz hace esto distinto

- **Estático, no reflection**: el checker valida en compile-time
  que `@authenticated`/`@admin` tengan un `@auth_provider`
  registrado y que el `user: User` del handler matchee el `User`
  del provider. Spring AOP / ASP.NET `[Authorize]` resuelven esto
  en runtime con reflection; cuando rompe, rompe en prod.
- **Zero dependencies**: `jwt`, `hash`, JWT signing, password
  hashing — todo viene en el binario `fitz` y se incluye en el
  binario nativo de `fitz build`. No hay `requirements.txt`,
  `package.json`, `Cargo.toml` que mantener. Deploy = un binario.
- **Paridad bit-a-bit `fitz run` ↔ `fitz build`**: el flow de auth
  funciona idéntico en el intérprete (rapid feedback durante dev)
  y en el binario nativo (deploy a prod). Cero "anda en local
  pero no en server".
- **OpenAPI auto-documentado**: el security scheme + los códigos
  401/403 + el bearer requirement por handler se generan del
  código fuente. No hay que escribir specs OpenAPI a mano (vs
  Express+`swagger-ui-express`+manual specs).
- **Integrado con el resto del lenguaje**: tipos custom + `Result<T>`
  + decoradores apilables + middleware/CORS + body deserialization
  + `return <status> { ... }` — todas las features previas siguen
  funcionando dentro de un handler protegido. No es una "isla" de
  auth con sus propias reglas.

### Qué no está en el MVP

Estos items están comprometidos como deuda explícita:

- **Sessions cookie-based** como alternativa a JWT. JWT primero
  (stateless, simple). Sessions persistentes requieren DB nativa
  (Fase 10).
- **RBAC con múltiples roles**. Hoy hay solo `@authenticated` y
  `@admin`. Custom decorators de auth con su propia lógica de
  permisos llegan post-Fase 10.
- **Provider request-aware** más allá de los headers (e.g. acceso
  al body o al método HTTP). Hoy el provider recibe solo
  `Map<Str, Str>` de headers.
- **Token refresh / revocación**. JWT del MVP es stateless puro;
  para revocar antes del `exp` hace falta una blacklist server-side
  (Fase 10 con DB).
- **Asimétricos (RS256/ES256)**. Hoy HS256/384/512 (HMAC con
  secret compartido). Asimétricos requieren parsear PEM y rotación
  de llaves — post-MVP.

Detalle completo en `docs/roadmap.md` para refinamientos pendientes
y planes de endurecimiento para servicios críticos.

---

## 29. WebSockets tipados

En Socket.IO/Phoenix Channels/ASP.NET SignalR/FastAPI WebSocket los
WS arrancan con un grado de tipado parcial y se completan a mano:
en FastAPI el handler recibe `WebSocket` opaco y vos parseás el
JSON con `json.loads` + Pydantic; en Socket.IO los eventos son
strings sin schema; SignalR genera proxies tipados solo en C#; el
schema de los mensajes (qué frames van, qué frames vienen) vive
en un README al lado del código o en una doc que se atrasa.
Encima el heartbeat para pasar proxies idle-killers (Nginx 60s,
Cloudflare ~100s, ALB 60s) lo configurás vos en cada proyecto, y
la auth pre-upgrade del handshake es código manual.

En Fitz los WebSockets son **parte del lenguaje** con tipado
end-to-end: declarás un `type ChatMsg { ... }`, declarás el
handler como `fn chat(conn: WsConn<ChatMsg>, ...)`, y cada frame
text se serializa/deserializa al `type` automáticamente. El
**heartbeat** se configura con un kwarg
(`@server(ws_heartbeat_secs=30)`) y los Ping frames se mandan
solos. El **AsyncAPI 3.0** del canal — la spec equivalente a
OpenAPI pero para event-driven — se genera del código fuente en
`/asyncapi.json`. Los decoradores **`@authenticated` / `@admin`**
apilados sobre `@ws` validan el bearer token ANTES del HTTP
upgrade, devolviendo 401/403 sin abrir el socket. Y todo eso
vale igual en `fitz run` y en el binario nativo de `fitz build` —
paridad bit-a-bit, cero `cargo add tokio-tungstenite` o
`pip install websockets`.

### Las piezas

**`@ws("/path")`** — declara un handler WebSocket. El handler es
una `async fn` que recibe un `WsConn<T>` como primer param, y
opcionalmente un `user: User` si está protegido por
`@authenticated`/`@admin`. `T` es el tipo de cada frame text;
puede ser un `type` custom (`ChatMsg`) o cualquier tipo
serializable a JSON.

```fitz
type ChatMsg { user: Str, text: Str }

@ws("/chat")
async fn chat(conn: WsConn<ChatMsg>) -> Null {
    loop {
        match conn.recv() {
            Ok(msg) => { let _ = conn.broadcast(msg) }
            Err(_) => return null
        }
    }
    return null
}
```

El checker valida estáticamente:

- Que el handler sea `async fn` (los WS son intrínsecamente async).
- Que el primer param sea `WsConn<T>` con `T` resoluble.
- Que retorne `Null` (el ciclo de vida del WS lo maneja el runtime).
- Si está bajo `@authenticated`/`@admin`, que reciba el `User` del
  `@auth_provider` registrado (igual que con `@get`/`@post`).

**`WsConn<T>`** — conexión activa. Cuatro métodos:

- **`conn.recv() -> Result<T>`** — bloquea hasta el próximo frame
  text. Devuelve `Ok(<T>)` con el mensaje deserializado, o `Err(e)`
  si la conexión se cierra o el frame no parsea contra `T`.
- **`conn.send(msg: T) -> Result<Null>`** — manda un frame text
  al cliente actual (solo al sender, no a otros).
- **`conn.broadcast(msg: T) -> Result<Null>`** — manda el mismo
  frame text a TODOS los clientes conectados al mismo endpoint —
  incluido el sender, convención Socket.IO/Phoenix.
- **`conn.close() -> Result<Null>`** — cierra la conexión
  explícitamente. Opcional: salir del handler también cierra.

**`@server(ws_heartbeat_secs=N)`** — configura el intervalo de
ping. Default 30s (el más bajo de los proxies comunes). Cada N
segundos el runtime manda un Ping frame por cada conexión viva;
si el cliente no responde Pong, el sink falla en el próximo
write y el writer task termina limpio. `N=0` desactiva el
heartbeat (no recomendado en deploys con proxies).

### Ejemplo completo: chat broadcast con auth

`examples/guide/29-ws.fitz` arma un servidor de chat completo en
~100 líneas que combina **todo el stack** de Fitz:

- `POST /login` (HTTP plano) verifica password con
  `hash.verify(...)` (Argon2id) y devuelve un JWT firmado con
  `jwt.encode(...)` (HS256).
- `@auth_provider fn check_token(headers)` corre antes de CADA
  request `@authenticated` — HTTP **y** WS. Para el WS, corre en
  el handshake antes del upgrade.
- `@authenticated @ws("/chat")` decora el handler con
  `WsConn<ChatMsg>`. Cada frame text se deserializa a `ChatMsg`
  automático; `conn.broadcast(msg)` envía a todos los clientes
  conectados.
- `@server(43929, ws_heartbeat_secs=30)` configura puerto y
  heartbeat.

Una sesión típica:

```bash
# 1. Login para obtener un token.
$ curl -X POST localhost:43929/login \
       -H 'Content-Type: application/json' \
       -d '{"email":"ada@example.com","password":"secret-ada-123"}'
{"token":"eyJ0eXAi..."}

# 2. Conectar dos clientes WS distintos (en dos terminales), cada
#    uno con SU bearer token. Cualquier mensaje JSON enviado se
#    broadcastea a TODOS los clientes — incluido el sender.
$ wscat -c "ws://localhost:43929/chat" \
        -H "Authorization: Bearer eyJ0eXAi..."
> {"user":"Ada","text":"hola"}
< {"user":"system","text":"bienvenido Ada"}
< {"user":"Ada","text":"hola"}  # del broadcast, con user.name forzado del JWT

# 3. Auth pre-upgrade — token inválido → 401 SIN abrir el socket.
$ wscat -c "ws://localhost:43929/chat" \
        -H "Authorization: Bearer fake"
error: Unexpected server response: 401
```

El ejemplo entero compila a binario nativo con `fitz build` y
producía output bit-a-bit idéntico al intérprete.

### Integración con AsyncAPI

OpenAPI 3.1 documenta endpoints HTTP request/response. **AsyncAPI
3.0** documenta canales event-driven — la spec hermana para WS,
MQTT, Kafka, etc. Cuando el programa declara `@ws(...)`, el
runtime genera automáticamente el schema en `/asyncapi.json`:

```json
{
  "asyncapi": "3.0.0",
  "info": { "title": "Fitz API", "version": "0.1.0" },
  "channels": {
    "/chat": {
      "messages": {
        "ChatMsg": {
          "name": "ChatMsg",
          "payload": {
            "type": "object",
            "properties": {
              "user": { "type": "string" },
              "text": { "type": "string" }
            },
            "required": ["user", "text"]
          }
        }
      }
    }
  },
  "operations": {
    "receive_/chat": {
      "action": "receive",
      "channel": { "$ref": "#/channels/~1chat" }
    },
    "send_/chat": {
      "action": "send",
      "channel": { "$ref": "#/channels/~1chat" }
    }
  },
  "components": {
    "securitySchemes": {
      "bearerAuth": {
        "type": "http",
        "scheme": "bearer",
        "bearerFormat": "JWT"
      }
    }
  }
}
```

Tooling AsyncAPI (AsyncAPI Studio en `asyncapi.com/studio`,
generadores de clientes para JS/TS/Python/Java) consume este
schema directo — escribís `type ChatMsg { ... }` una vez y los
clientes en otros lenguajes se generan solos.

**UI embebida en `/asyncapi`** — paralelo al `/docs` del
OpenAPI/Scalar. Cuando hay handlers `@ws`, el server autoregistra
dos rutas:

- `GET /asyncapi.json` — schema crudo.
- `GET /asyncapi` — UI HTML embebida usando
  `@asyncapi/react-component` (cargado vía CDN). Renderiza
  channels, operations, mensajes con sus schemas, security
  requirements. Útil para devs que consumen el spec o testean
  endpoints WS.

Igual que el `/docs` del OpenAPI: si el usuario declara su propio
`@get("/asyncapi")`, el auto-register cede al handler del user.
Opt-out global de ambas UIs con `@server(docs=false)`.

### Por qué Fitz hace esto distinto

Cinco diferenciales que vuelven a Fitz único en el espacio de
WebSockets tipados:

- **Marshaling JSON automático** — declarás `WsConn<ChatMsg>` y
  cada frame text se serializa/deserializa a `ChatMsg` sin un
  `json.loads` + Pydantic / `JSON.parse` + Zod / `serde_json`
  manual. El mismo trait que sirve los handlers HTTP
  (`__ToFitzJson`/`__FromFitzJson`) cubre WS, primitivos, types
  custom, listas, mapas, Result. Si el frame no parsea contra `T`,
  `recv()` devuelve `Err` — error de runtime esperable, no panic.
- **AsyncAPI auto-generado** — el schema en `/asyncapi.json` sale
  del código fuente. No hay que mantener un YAML AsyncAPI a mano
  (vs Socket.IO/Phoenix/SignalR/FastAPI WebSocket, donde el
  schema vive en un README al lado del código o en una doc que
  atrasa). Tooling AsyncAPI estándar (Studio, generadores de
  clientes) consume el schema directo.
- **Heartbeat built-in** — `@server(ws_heartbeat_secs=N)` y listo.
  El runtime manda los Ping frames cada N segundos por cada
  conexión viva, sin código del usuario. Pasa de largo Nginx
  (60s default idle), Cloudflare (~100s), AWS ALB (60s).
- **Auth integrada** — `@authenticated`/`@admin` apilados sobre
  `@ws` validan el bearer token ANTES del HTTP upgrade. El
  cliente recibe 401/403 sin que se abra el socket — menos
  attack surface, menos recursos consumidos. El checker valida
  en compile-time que el handler reciba el `User` correcto del
  `@auth_provider` registrado.
- **Codegen con paridad** — el flow WS funciona idéntico en
  `fitz run` (intérprete, feedback rápido durante dev) y en
  `fitz build` (binario nativo standalone, deploy a prod). Cero
  "anda en local pero no en server". Cero `cargo add
  tokio-tungstenite` o `pip install websockets`.

**Ningún otro lenguaje hoy combina WS tipados con AsyncAPI
auto-generado del código fuente, heartbeat built-in y auth
integrada en el handshake**. FastAPI WebSocket te da Pydantic y
schema manual; Socket.IO te da eventos sin schema; Phoenix
Channels te da pattern matching tipado pero solo en Elixir;
SignalR te da proxies tipados solo en C# y solo en `.NET`. Fitz
los junta en un lenguaje nuevo, con un binario standalone, con
auth nativa.

### Auth WS desde browsers — subprotocol `bearer.<token>`

`new WebSocket(url)` del browser **no permite setear headers HTTP**
arbitrarios. El segundo argumento del constructor SÍ acepta una
lista de subprotocols. La convención estándar (Socket.IO, Phoenix,
varios proyectos Node) es pasar el token via subprotocol con
formato `bearer.<token>`.

Fitz lo soporta sin cambios del lado user — el runtime y el
codegen extraen el token del header `Sec-WebSocket-Protocol` y
lo inyectan como `authorization: Bearer <token>` al map de
headers que ve el `@auth_provider`. El mismo provider funciona
para HTTP y WS.

Cliente browser:

```javascript
const token = "eyJhbGc..."; // JWT obtenido vía POST /login
const ws = new WebSocket("ws://localhost:43929/chat", `bearer.${token}`);
ws.onmessage = (ev) => console.log("got:", ev.data);
ws.onopen = () => ws.send(JSON.stringify({user: "Ada", text: "hola"}));
```

Server Fitz — sin cambios respecto al cap 28:

```fitz
@auth_provider
fn check_token(headers: Map<Str, Str>) -> Result<User> {
    let auth = match headers.get("authorization") {
        Ok(v) => v,
        Err(_) => return Err("falta Authorization"),
    }
    // auth = "Bearer eyJhbGc..." (inyectado por el runtime desde
    // el subprotocol, sin que el handler lo sepa).
    let token = auth.replace("Bearer ", "")
    let claims = jwt.decode(token, SECRET)?
    return find_user(claims["email"])
}

@authenticated
@ws("/chat")
async fn chat(conn: WsConn<ChatMsg>, user: User) -> Null { /*...*/ }
```

**Echo del subprotocol**: RFC 6455 §4.1 exige que el server
confirme el subprotocol elegido en la response del handshake. Sin
echo, el browser rechaza el upgrade. Fitz lo hace automático —
si el cliente envió `bearer.<token>`, el server responde con el
mismo subprotocol en el handshake.

**Compatibilidad con header**: si el cliente envía AMBOS
(`Authorization: Bearer ...` y `Sec-WebSocket-Protocol: bearer.<token>`),
el header gana — el subprotocol solo inyecta cuando no hay
`Authorization` previo. Esto preserva el caso wscat/curl/clientes
no-browser que sí pueden setear headers.

Paridad bit-a-bit `fitz run` ↔ `fitz build`.

### Canales asimétricos con `WsConn<In, Out>`

Para canales donde el cliente envía un tipo y el server emite otro
distinto (e.g. cliente manda comandos `Str`, server emite eventos
`ChatMsg` estructurados), declarás `WsConn<In, Out>` con dos
type params en lugar del simétrico `WsConn<T>`:

```fitz
type ChatMsg { user: Str, text: Str }

@ws("/cmd")
async fn cmd(conn: WsConn<Str, ChatMsg>) -> Null {
    let welcome = ChatMsg { user: "system", text: "conectado" }
    let _ = conn.send(welcome)

    loop {
        match conn.recv() {
            Ok(input) => {
                // input: Str (el `In` del WsConn<In, Out>)
                let reply = ChatMsg { user: "system", text: "got:{input}" }
                let _ = conn.send(reply)   // espera ChatMsg (el `Out`)
            }
            Err(_) => return null
        }
    }
    return null
}
```

`conn.recv()` devuelve `Result<In>`; `conn.send(msg)` y
`conn.broadcast(msg)` aceptan `Out`. El checker valida los tipos
por dirección, así que `conn.send("hola")` sobre `WsConn<Str, ChatMsg>`
falla en compile-time con mensaje claro: el `send` espera `ChatMsg`,
no `Str`.

**Compat hacia atrás**: `WsConn<T>` (aridad 1) es equivalente a
`WsConn<T, T>` y todos los handlers existentes siguen funcionando
idéntico.

**AsyncAPI asimétrico**: cuando `In != Out`, el schema autogenerado
emite **dos messages distintos**: `msg_in` (referenciado por la
operation `receive`, payload schema de `In`) y `msg_out` (referenciado
por la operation `send`, payload schema de `Out`). Generadores de
clientes AsyncAPI producen interfaces tipadas separadas para entrada
y salida, paralelo al modelo de gRPC Server-Streaming.

**Restricción binary mixto**: si `In` o `Out` es `Bytes` pero el
otro no, el codegen rechaza con error explícito (canal mixto
binary/text es deuda residual). `WsConn<Bytes, Bytes>` y
`WsConn<Bytes>` simétrico funcionan; `WsConn<Bytes, Str>` no.

Paridad bit-a-bit `fitz run` ↔ `fitz build` (el preludio Rust usa
`__FitzWsConn<RECV, SEND>` con 2 type params; monomorfismo hace
que canales simétricos `WsConn<T>` produzcan binarios idénticos al
pre-bidir).

Ejemplo runnable completo:
[`examples/guide/29c-ws-bidir.fitz`](../examples/guide/29c-ws-bidir.fitz).

### Frames binarios con `WsConn<Bytes>`

Cuando el wire es binario raw — protocolos custom (gRPC-Web,
protobuf, MessagePack, CBOR), streaming de audio/video, file
transfer chunk-a-chunk — `WsConn<Bytes>` opera con `Vec<u8>` en
ambas direcciones, sin re-encoding a JSON ni base64:

```fitz
@server(43930)
fn main() => 0

@ws("/raw")
async fn raw(conn: WsConn<Bytes>) -> Null {
    loop {
        match conn.recv() {
            Ok(buf) => {
                let _ = conn.send(buf)   // echo binario puro
            }
            Err(_) => return null
        }
    }
    return null
}
```

`conn.recv()` espera `Message::Binary` (no `Message::Text`) y lo
devuelve como `Result<Bytes>`. `conn.send(buf)` y
`conn.broadcast(buf)` emiten `Message::Binary` raw. Mismatch
entre frame type esperado y recibido → `Err` con mensaje claro
(usá `WsConn<Bytes>` para binary; `WsConn<Str>` o nominal para
text).

El schema AsyncAPI 3.0 auto-generado refleja el modo binario:

```json
{
  "channels": {
    "/raw": {
      "messages": {
        "msg": {
          "contentType": "application/octet-stream",
          "payload": { "type": "string", "format": "binary" }
        }
      }
    }
  }
}
```

Tools como AsyncAPI Studio o generadores de clientes binarios lo
reconocen y renderean el endpoint como "binary upload" en vez de
schema JSON convencional.

Paridad bit-a-bit `fitz run` ↔ `fitz build`: el binario nativo
producido por `fitz build` tiene un struct dedicado
`__FitzWsConnBytes` (no genérico) y un setup helper aparte —
specialization sobre `__FitzWsConn<Vec<u8>>` no funciona porque el
blanket impl del trait interno trataría `Vec<u8>` como
`List<Int>` JSON. Dos structs dedicados son la solución más
simple y explícita.

Trade-off del modelo: un endpoint es text-only XOR binary-only.
Para canales mixtos (texto + bytes en el mismo socket), declarás
dos endpoints separados (`@ws("/raw")` para bytes, `@ws("/json")`
para text). Si aparece presión real por mixto en el mismo
endpoint, queda como sub-paso futuro.

Ejemplo runnable completo: [`examples/guide/29b-ws-binary.fitz`](../examples/guide/29b-ws-binary.fitz).

### Qué no está en el MVP

Estos items están comprometidos como deuda explícita:

- **Reconnect con state replay** — si el cliente se reconecta,
  el server hoy no replica los frames perdidos. Pattern típico
  de chat con history; requiere persistencia (Fase 10).
- **Rooms / channels dentro de un endpoint**. Hoy `broadcast()`
  manda a TODOS los clientes del endpoint. Sub-canales (rooms
  de Socket.IO, topics de Phoenix) son deuda visible — modelo
  típico para presence, grupos, salas privadas.
- **Backpressure explícito**. Hoy el outbox por conn es
  unbounded — un cliente lento puede acumular memoria. Bounded
  channels + drop policy es deuda residual.

Detalle completo en `docs/roadmap.md` para refinamientos pendientes
y planes de endurecimiento para servicios críticos.

---

## 30. Jobs sin Celery

En Python lo típico es Celery + Redis (o RabbitMQ): un broker
externo + un worker pool + decoradores `@task` + `apply_async`. En
Node es Bull/BullMQ con la misma forma. En Go hay `cron` y
`goroutines` pero el cron no está en el lenguaje (es lib). En
Rust hay `tokio-cron-scheduler`. Todos comparten el problema: una
dependencia externa por cada modo (cron, jobs en background,
queue), configuración separada, y a veces un proceso aparte.

En Fitz los jobs son **parte del lenguaje** con tres piezas:
**`@cron("expr")`** para tareas periódicas, **`@background`** para
marcar fns spawneables, y **`spawn(fn_call)`** para fire-and-forget
desde un handler HTTP. Sin Celery, sin Redis, sin systemd timers.
Todo corre en el mismo binario con paridad bit-a-bit `fitz run` ↔
`fitz build`.

### Las piezas

**`@cron("expr")`** — corre la fn periódicamente según una cron
expression. La fn no admite params (los jobs no reciben input) y
retorna `Null` o `Result<Null>` (el runtime descarta el valor; usa
`Result` si querés loguear fallas):

```fitz
@cron("0 0 * * *")  // cada medianoche (5 fields Unix clásico)
fn cleanup_old_sessions() -> Null {
    print("[cron] limpieza diaria")
    return null
}

@cron("*/5 * * * * *")  // cada 5 segundos (6 fields con seconds)
async fn heartbeat() -> Null {
    return null
}
```

Sintaxis aceptada por el parser:

- **5 fields Unix clásico**: `"min hora día mes día-semana"`. El
  runtime prependa `"0 "` (segundo 0) automáticamente para
  compatibilidad con el cron tradicional.
- **6 fields con seconds al inicio**: `"sec min hora día mes
  día-semana"`. Útil para sub-minuto.
- **7 fields con año al final**: `"sec min hora día mes día-semana
  año"`.

El checker valida shape (1 arg Str, sin params, return
Null/Result/Future, no combinable con `@get`/`@post`/`@ws`/
`@background`). La sintaxis del cron expression se valida en
runtime/codegen al registrar el job — typos disparan error claro
con un ejemplo correcto.

**`@background`** — marca una fn como autorizada para `spawn(...)`.
Sin el decorator, el checker rechaza el callsite estáticamente:

```fitz
@background
async fn send_email(to: Str, body: Str) -> Null { ... }

// En cualquier handler:
let _ = spawn(send_email("ada@example.com", "welcome"))
```

Es opt-in del autor para evitar usos accidentales sobre fns
regulares cuyo retorno el caller espera consumir.

**`spawn(fn_call)`** — fire-and-forget. Devuelve `Future<T>` con
`T` el tipo de retorno de la fn target. Si descartás el Future
(`let _ = spawn(...)`), la task queda **detached** ejecutándose en
background. Si lo guardás y `.await`-eás, esperás al resultado:

```fitz
// Fire-and-forget — la task corre sola.
let _ = spawn(send_email("ada", "hi"))

// O esperás al resultado (útil para coordinar múltiples jobs).
let result: Null = spawn(send_email("ada", "hi")).await
```

El target del `spawn(...)` debe ser un call literal a una fn
`@background`. El checker rechaza `spawn(x)` con `x` variable o
`spawn(obj.method())` — el target debe ser claro estáticamente.

### Ejemplo completo: URL shortener con cron + spawn

`examples/guide/30-cron-background.fitz` arma un servicio mínimo
en menos de 100 líneas que combina **todo el stack** de jobs:

- `type Link { slug, url, clicks }` — dominio.
- `let LINKS: Map<Str, Link> = {}` — DB en memoria (para
  persistencia real, usar el ORM nativo del cap 31).
- `@background async fn track_click(slug)` — simula I/O lento
  con `sleep(100).await` (típico: insert en analytics DB).
- `@cron("*/5 * * * * *") fn stats()` — imprime estadísticas
  cada 5 segundos.
- `@get("/go/{slug}") fn redirect(slug)` — devuelve el target
  URL **inmediato** y dispara `spawn(track_click(slug))` para
  registrar el clic en background sin bloquear la response.

Una sesión típica:

```bash
$ fitz run examples/guide/30-cron-background.fitz
🏔️  Fitz HTTP escuchando en http://127.0.0.1:43930
   POST /links
   GET /links
   GET /go/{slug}
🕐 Fitz scheduler arrancado con 1 job(s) cron
   @cron  stats (*/5 * * * * *)

# Crear un link.
$ curl -X POST localhost:43930/links \
       -H 'Content-Type: application/json' \
       -d '{"slug":"fitz","url":"https://github.com/.../fitz"}'
{"slug":"fitz","url":"https://github.com/.../fitz","clicks":0}

# Redirigir — la response sale inmediato; el clic se registra
# async en background.
$ curl localhost:43930/go/fitz
"→ https://github.com/.../fitz"
[bg] click registrado: /fitz → 1 total       # log del background

# Tras 5 segundos el cron dispara stats.
[cron] stats — 1 link(s) registrados
[cron]   /fitz → 1 clicks
```

El ejemplo entero compila a binario nativo con `fitz build` y
produce output bit-a-bit idéntico al intérprete.

### Persistencia, retry y timezone (iter2)

Cuatro kwargs opcionales endurecen `@cron(...)` para servicios
que tienen que sobrevivir a un restart o tolerar fallos
upstream:

- **`tz="IANA/Name"`** — interpreta la cron expression en el huso
  indicado en vez de UTC. Acepta cualquier IANA timezone
  (`"America/Argentina/Buenos_Aires"`, `"Europe/Madrid"`,
  `"UTC"`). Sin esto, `"0 9 * * *"` significa "9 AM UTC", lo
  que probablemente NO sea lo que el operador espera. Default
  `"UTC"` (paridad con el MVP).
- **`retry={max: N, backoff: "...", initial_secs: I, max_secs: M}`** —
  si el handler devuelve `Err(...)` o paniquea, reintenta hasta
  `N` veces con delay calculado. Backoffs aceptados:
  `"exponential"` (delay = `I * 2^(attempt-1)`, recomendado para
  upstream que puede bouncear), `"linear"` (`I * attempt`),
  `"constant"` (`I`). Todos capeados por `max_secs`. Default:
  sin retry (un solo intento, paridad con el MVP).
- **`catch_up=true`** — si el proceso estuvo abajo durante uno
  o más ticks programados, al arrancar ejecuta **UN** run
  inmediato (no `N` — evita spam). Default `false` = skip
  silencioso (la semántica de "no acumular trabajo viejo" es
  la correcta para el 90% de casos; opt-in cuando importa).
- **`store=<binding>`** — persiste el registry del job y cada
  run en las tablas `fitz_cron_jobs` / `fitz_cron_runs` de la
  conn DB indicada. Sin esto, los jobs viven en memoria y se
  pierden al reiniciar (paridad con el MVP). Con esto, podés
  inspeccionar la historia con `psql`:

  ```sql
  -- Último estado de cada job.
  SELECT name, schedule, tz, last_run_at, last_status, last_error
  FROM fitz_cron_jobs;

  -- Últimas N ejecuciones (incluye intentos de retry).
  SELECT job_name, started_at, finished_at, status, attempt, error
  FROM fitz_cron_runs
  ORDER BY id DESC LIMIT 20;
  ```

  Las tablas se crean automáticamente al boot del scheduler con
  `CREATE TABLE IF NOT EXISTS`. Schema:

  ```sql
  fitz_cron_jobs(
      name PRIMARY KEY, schedule, tz,
      last_run_at, last_status, last_error, next_run_at
  )
  fitz_cron_runs(
      id BIGSERIAL, job_name, started_at, finished_at,
      status, attempt, error
  )
  -- status: 'running' | 'ok' | 'failed' | 'retrying'
  -- attempt: 1-indexed; retry máx = N produce hasta N+1 rows
  ```

Ejemplo end-to-end combinando los cuatro kwargs +
HTTP — `examples/guide/30b-cron-persistente.fitz`:

```fitz
let db = db.connect(env_or("DATABASE_URL", "postgres://...")).await

@cron("*/10 * * * * *",
      tz="America/Argentina/Buenos_Aires",
      retry={max: 3, backoff: "exponential",
             initial_secs: 1, max_secs: 30},
      catch_up=true,
      store=db)
async fn heartbeat() -> Result<Null> {
    print("[cron] heartbeat tick")
    return Ok(null)
}

@get("/health")
fn health() -> Str { return "ok" }

@server(43931, docs=false)
fn main() => 0
```

**Detalle del binding `db`**: queda como `Result<DbConn>` (no
`DbConn`) porque `db.connect(...).await` retorna `Result` y `?`
no está soportado top-level. El runtime/codegen desempaca
automáticamente vía el trait `__FitzCronStoreFrom`: si la conn
falló al inicio, panea con mensaje claro citando el motivo
(equivalente al `expect()` que escribirías a mano).

**Paridad bit-a-bit**: el comportamiento de los cuatro kwargs
es idéntico en `fitz run` (HTTP+cron — vivo por el server) y
`fitz build` → binario nativo. Los tipos del runtime
(`BackoffKind`, `RetryConfig`, `CronJobOptions`) tienen sus
paralelos `__FitzBackoffKind`/`__FitzRetryConfig`/
`__FitzCronOptions` en el binario, con la misma lógica de
`delay_for_attempt` y `invoke_with_retry`.

**Paridad con `fitz run` (desde v0.37.3)** — `@cron(..., store=db)`
(persistencia del scheduler) funciona igual con `fitz run` que con
`fitz build`, en ambos modos (cron-only y HTTP+cron). Desde v0.37.3
el intérprete corre el eval y el scheduler/servidor sobre un único
runtime tokio compartido, así que la conexión DB que abrís al boot
sigue viva cuando el scheduler crea `fitz_cron_jobs`. (Antes de
v0.37.3 el eval usaba un runtime `current_thread` que se dropeaba
antes de arrancar el scheduler, y la primera query del cron fallaba
con *"A Tokio 1.x context was found, but it is being shutdown"* —
ese bug de lifecycle del runtime del intérprete está cerrado.)

### Cron-only mode (sin server HTTP)

Un programa con solo `@cron` (sin `@server` ni handlers HTTP)
también funciona — el main queda vivo bloqueante hasta SIGINT/
Ctrl+C (modo systemd-friendly):

```fitz
// cleanup.fitz — script periódico standalone.

@cron("0 3 * * *")  // todos los días a las 3 AM
fn cleanup() -> Null {
    print("[cron] cleanup nocturno")
    return null
}

print("Daemon arrancado. Ctrl+C para salir.")
```

```bash
$ fitz build cleanup.fitz
$ ./cleanup
Daemon arrancado. Ctrl+C para salir.
🕐 Fitz scheduler arrancado con 1 job(s) cron
   @cron  cleanup (0 3 * * *)
# ... espera hasta 3 AM o Ctrl+C ...
^C
🕐 Fitz scheduler recibió Ctrl+C, terminando.
```

Es exactamente lo que necesitás para un service `systemd`:
`ExecStart=/usr/local/bin/cleanup`, sin scripts extra ni
intérpretes embebidos.

### `@background` persistente (store + catch_up + retry)

Los mismos kwargs de persistencia que `@cron` aplican a
`@background` (v0.37.7). Con `store=db` cada `spawn(...)` de un
worker persistido queda registrado en la tabla `fitz_bg_jobs` —
así los jobs sobreviven un reinicio del proceso (visibilidad de
qué corrió, con qué args, qué falló, cuántos reintentos), sin
broker externo.

```fitz
let db = db.connect(env_or("DATABASE_URL", "postgres://...")).await

@background(store=db, catch_up=true,
            retry={max: 2, backoff: "exponential", initial_secs: 1, max_secs: 10})
async fn send_welcome(user_id: Int, channel: Str) -> Result<Null> {
    // ... trabajo diferido; un `return Err(...)` cuenta como fallo
    // del job y dispara el retry ...
    return Ok(null)
}

@post("/users/{id}/notify")
fn notify(id: Int) -> Str {
    let _ = spawn(send_welcome(id, "email"))   // no-bloqueante
    return "queued"
}
```

Qué persiste cada spawn en `fitz_bg_jobs`:

- `fn_name` + `args_json` — los args serializados a JSON
  (`[42,"email"]`). Soporta **primitivos + compuestos** (nominales,
  `List`, `Map`). El args_json es solo para **visibilidad** — nunca
  se deserializa de vuelta.
- `status` — `running` → `retrying` → `ok`/`failed`. Los retries
  actualizan la MISMA fila (una tabla, un job = un spawn = una fila,
  a diferencia de las dos tablas de `@cron`).
- `attempt` + `error` + `created_at` + `finished_at`.

**best-effort**: el INSERT es el primer paso *dentro* de la task
spawneada, así que `spawn(...)` sigue siendo fire-and-forget
no-bloqueante. Si la DB está caída al momento del spawn, el job
igual corre (la persistencia es aditiva, nunca traga el trabajo).

**catch_up**: `catch_up=true` marca al boot las filas huérfanas
(`running`/`retrying` que un crash dejó a medias) como `failed`
con `error = 'orphaned by restart'`. NO las re-ejecuta — el
trabajo se perdió; el operador decide re-dispararlas. (A
diferencia del `catch_up` de `@cron`, que sí re-ejecuta el último
tick perdido.)

Visibilidad con `psql`:

```sql
SELECT id, fn_name, args_json, status, attempt, error
FROM fitz_bg_jobs ORDER BY id DESC LIMIT 10;
```

El binding `store=db` se resuelve al boot (top-level `let db =
db.connect(...).await`, mismo timing que `@cron`). Sin `store` el
`@background` sigue siendo fire-and-forget in-memory como siempre
(backward-compat total). Paridad bit-a-bit `fitz run` ↔ `fitz
build`. Ejemplo runnable completo en
`examples/guide/30c-background-persistente.fitz`.

### Por qué Fitz hace esto distinto

Cinco diferenciales que vuelven a Fitz único en este espacio:

- **Decoradores nativos del lenguaje**: `@cron`/`@background` son
  parte del compilador, no una lib opcional. El checker valida
  shape en compile-time (typos en cron expression, params
  prohibidos, conflictos con `@get`/`@ws`/`@auth_provider`); no
  esperás a runtime para descubrir que el job está roto. Vs
  Celery (lib externa con `@task` reflection), Bull (decorators
  TypeScript con tipos opcionales), Spring `@Scheduled`
  (reflection en runtime con AOP).
- **Sin broker externo**: Redis/RabbitMQ NO son requisito. Los
  jobs viven en memoria del proceso. Para 90% de los servicios
  reales (tareas de mantenimiento, scripts periódicos, fire-and-
  forget de notificaciones) eso es **suficiente**. Persistencia
  llega con Fase 10 + DB nativa — sin cambiar la sintaxis.
- **`spawn` con tipado**: el checker valida en compile-time que
  el target tenga `@background` Y refina el ret type a
  `Future<T>` con T concreto. Vs `tokio::spawn` Rust (sin
  marcador, cualquier closure pasa), vs `asyncio.create_task`
  Python (sin tipos), vs Celery `apply_async` (string-based
  task name, lookup en runtime).
- **Paridad bit-a-bit `fitz run` ↔ `fitz build`**: el flow de
  jobs corre idéntico en intérprete (rapid dev) y binario nativo
  (deploy a prod). Cero "anda en local pero no en server".
- **Cero `cargo add tokio-cron-scheduler` / `pip install celery`**:
  el crate `cron` + helpers van en el binario `fitz`. Deploy es
  un binario.

**Ningún otro lenguaje hoy combina cron + background workers + spawn
tipado en el core del lenguaje, sin broker externo y con paridad
intérprete↔binario**.

### Qué no está en el MVP

iter2 cerró **persistencia + retry + timezone + catch_up** (ver
sección anterior). Lo que queda como deuda explícita:

- **Visibility de jobs en panel admin nativo** (UI que liste
  runs pasados con filtros, estadísticas agregadas, gráficos
  de tasa de error). Hoy con `store=db` los datos están en
  `fitz_cron_jobs` / `fitz_cron_runs` — los querés con `psql`
  o un dashboard externo (Grafana, Metabase). Una UI dedicada
  estilo Sidekiq Web podría llegar como sub-paso futuro.
- **`spawn(...)` de un `@background(store=db)` desde un módulo**
  (no desde el main). Desde v0.37.8, un `@background(store=db)` fn
  + su `let db = ...` co-localizados en un módulo importado
  persisten con `fitz build` cuando el `spawn(...)` está en el
  main (port de B20). Lo que queda diferido es el `spawn(...)`
  ubicado DENTRO de un módulo (requiere que el ctx del módulo
  también resuelva el store global). En `fitz run` funciona desde
  cualquier lado (el registry es global).
- **Coordinación entre múltiples instancias** (locks distribuidos
  para que un cron solo corra en un nodo). Hoy cada instancia
  corre todos sus jobs — si tenés 3 réplicas detrás de un load
  balancer, el `@cron` dispara 3 veces. Para single-instance el
  comportamiento es correcto.
- **`spawn` con coordinación múltiple**: `spawn(...).await` solo
  awaitea un task; para `Promise.all([...])` style hace falta
  agregación manual con vectores de futures.

Detalle completo en `docs/roadmap.md` para refinamientos pendientes
y planes de endurecimiento adicionales.

---

## 31. Postgres + ORM nativo

> Este capítulo cierra la promesa del proyecto de "stack web
> first-class del lado server": HTTP nativo (cap 17) + auth (28)
> + WebSockets (29) + jobs (30) + DB + ORM (este). Todo en el
> binario `fitz`, cero deps externas para features intrínsecas.

En SQLAlchemy/Django ORM/ActiveRecord/Hibernate/Prisma la
combinación DB driver + ORM se construye sumando librerías al
proyecto: `pip install sqlalchemy psycopg2`, `pip install
django + django.db`, gem `pg + activerecord`, JDBC driver +
Hibernate JARs, `npm install prisma + @prisma/client`. Cinco
dependencias mínimo, configuración manual, decoradores
"mágicos" que se resuelven en runtime con reflection
(`@Entity`/`@OneToMany` de JPA), generación de schema separada
(Prisma exige `prisma generate` antes de cada build), tipado
opcional que no respeta el shape real de la tabla. Y a la hora
de compilar a binario nativo: imposible para Python/Ruby, parche
para Node (con `pkg`/`nexe` y limitaciones), funciona en Go
(`pgx` + `gorm`) pero arrastra un ORM separado del compilador.

En Fitz, **DB + ORM son parte del lenguaje**. El módulo `db`
viene con un driver Postgres puro escrito en Fitz/Rust (~2400
LoC en `src/db.rs`, sin link a libpq) que habla wire protocol
v3.0 + SCRAM-SHA-256 + parser de 11 OIDs core. Encima del
driver, cinco decoradores nativos (`@table`, `@primary`,
`@column`, `@belongs_to`, `@has_many`, `@has_one`) declaran el
mapping `type` Fitz ↔ tabla Postgres. El type checker valida
estáticamente que `@primary` exista, que `@belongs_to` apunte
a un type existente, que los métodos del `QueryBuilder<Row>`
preserven el tipo del row a lo largo de toda la chain. Y el
codegen produce un binario nativo que ejecuta queries SQL
**constantes en compile-time** (cada `.where(fn(u) => u.age >
18)` se traduce al fragmento `"age" > $1` durante el codegen,
zero overhead runtime para construir SQL). Paridad bit-a-bit
entre `fitz run` y `fitz build` — el mismo programa corre
idéntico en intérprete y binario nativo.

### Las piezas

**Driver `db`** — módulo built-in. Siempre disponible, sin import.

```fitz
// Local dev (Postgres en Docker, sin TLS):
let db = db.connect("postgres://user:pass@host:5432/dbname?sslmode=disable").await?

// Managed PG real (Heroku, RDS, Supabase, Neon, ...) — sslmode=verify-full
// recomendado. El driver trae el Mozilla CA bundle in-binary (cero
// deps system); cubre todos los managed PG mainstream.
let db = db.connect(
    "postgres://user:pass@db.proyecto.supabase.co:5432/postgres?sslmode=verify-full"
).await?

let rows = db.query("SELECT id, email FROM users WHERE active = $1", [true]).await?
// rows: List<DbRow>

let r: DbRow   = rows[0]
let id: Int    = r.get_int("id")?
let email: Str = r.get_str("email")?

let affected = db.exec("UPDATE users SET last_seen = NOW() WHERE id = $1", [42]).await?
// affected: Int

db.close().await?
```

`DbRow` es un row opaco con extracción tipada por columna —
`r.get_int/get_str/get_float/get_bool` retornan `Result<T>` con
error claro si la col no existe, es NULL, o el tipo PG no matchea.
En handlers HTTP también podés retornar `Result<List<DbRow>>` y el
codegen auto-serializa cada row a `{col: val, ...}` en el JSON
response (útil para queries con shape dinámico no representable como
`type`; ver el boilerplate `api-multi-tenant` Enfoque B).

URL formato estándar Postgres (`postgres://[user[:pass]@]host[:port]/dbname[?params]`).
`sslmode=disable` requerido en MVP (TLS strict viene como
sub-paso futuro). El driver mantiene un pool de conexiones
internamente con reconnect automático cuando una conn muere
(health check via `Weak<DbPool>` + cleanup automático).

**`@table("nombre")`** — sobre un `type`. Lo marca como una
tabla. Implica que el ORM puede emitir SELECT/INSERT/UPDATE/
DELETE sobre el `type`.

```fitz
@table("users") type User {
    @primary id: Int = 0
    email: Str
    age: Int
    role: Str = "user"
}
```

**`@primary`** — sobre un field. Debe haber exactamente uno por
`type` con `@table`. Tipo Int (auto-asignado por bigserial) o
Str (UUID generado por el cliente). El checker exige unicidad.

**`@column(name="snake_case_col")`** — sobre un field cuando el
nombre del field Fitz difiere del de la columna en la tabla
(camelCase ↔ snake_case típico):

```fitz
@table("orders") type Order {
    @primary id: Int = 0
    @column(name="user_id") user_id_fk: Int
    @column(name="created_at") created: Str  // ISO 8601
}
```

**`@belongs_to("Target")`**, **`@has_many("Target")`**,
**`@has_one("Target")`** — sobre fields para declarar relations
cross-table. Aceptan kwargs adicionales: `via="fk_column"` (para
`has_many`/`has_one`), `fk="custom_fk_field"` (para `belongs_to`),
`on_delete="cascade"` / `on_update="cascade"` (Postgres FK
actions: `"cascade"` / `"set_null"` / `"restrict"` / `"no_action"`):

```fitz
@table("posts") type Post {
    @primary id: Int = 0
    title: Str
    @belongs_to("User") user_id: Int   // FK real en la tabla
}

@table("users") type User {
    @primary id: Int = 0
    email: Str
    @has_many("Post", via="user_id") posts: List<Post>
    // ↑ field virtual: NO entra al SELECT/INSERT normal,
    //   hidrata vía .preload(...) o post.user_id(db).
}
```

### Read methods + QueryBuilder chain

**`Type.all(db)`** — devuelve todas las rows como `List<Type>`:

```fitz
let users: List<User> = User.all(db).await?
```

**`Type.where(closure) -> QueryBuilder<Type>`** — empieza una
chain de filtros. El closure recibe un nominal del row y
devuelve un Bool. El checker valida estáticamente que el closure
referencie fields que existen en el `type`. El translator
DURANTE EL CODEGEN walka el AST del closure y emite SQL
parametrizado:

```fitz
// `(age > 18 AND role = 'admin') OR id = 1`
let admins = User.where(fn(u) => (u.age > 18 and u.role == "admin") or u.id == 1)
    .all(db).await?
```

**Chain methods**: `.order_by(closure, ascending: Bool)`,
`.limit(n)`, `.offset(n)`, `.group_by(closure)`. Todos preservan
el tipo del row:

```fitz
let top = User.where(fn(u) => u.age >= 18)
    .order_by(fn(u) => u.age, ascending: false)
    .limit(10)
    .offset(0)
    .all(db).await?
```

**Terminales**: `.all(db) -> Result<List<Row>>`,
`.first(db) -> Result<Row>` (error si no hay match),
`.count(db) -> Result<Int>`.

**Operadores soportados en `.where(...)`**: comparators (`==`,
`!=`, `<`, `<=`, `>`, `>=`), lógicos (`and`/`or`/`not`),
aritméticos (`+`/`-`/`*`/`/`/`%`), `between(a, b)` sobre fields
numéricos, vars externas al closure (lookup en el scope), y
method calls sobre columns que se mapean a SQL nativo:

```fitz
let active = User.where(fn(u) => u.email.is_not_null()).all(db).await?
let bands = User.where(fn(u) => u.age.between(18, 65)).all(db).await?
let pattern = User.where(fn(u) => u.email.starts_with("ada")).all(db).await?
let one_of = User.where(fn(u) => u.id.is_in([1, 2, 3])).all(db).await?

let min = 18
let adults = User.where(fn(u) => u.age >= min).all(db).await?  // var externa
```

Métodos sobre columns Str: `.is_null()`, `.is_not_null()`,
`.is_in([...])`, `.like(pat)`, `.ilike(pat)`, `.starts_with(s)`,
`.ends_with(s)`, `.contains(s)`. Patterns con `%`/`_` se
escapan automáticamente en `.starts_with`/`.ends_with`/
`.contains`. `is_in([])` con lista vacía → predicado `false`
(no rompe el query). Caveat MVP: `.is_in([...])` y los array
ops requieren el **List como literal** (`.is_in(some_var)`
falla); los items adentro de la lista pueden ser vars.

### Write methods + guard `.where(...)` obligatorio

**`Type.insert(db, row)`** — inserta un row. Si el `@primary`
es Int con default `= 0`, Postgres lo auto-asigna (`bigserial`)
y el resultado tiene el id real:

```fitz
let inserted = User.insert(db, User { id: 0, email: "ada@x.com", age: 35 }).await?
print(inserted.id)  // 42 (auto-asignado por Postgres)
```

**`.update(db, changes)`** y **`.delete(db)`** — sobre un
`QueryBuilder<Row>` con `.where(...)` previo **obligatorio**.
El ORM rechaza estáticamente updates/deletes sin guard
(`Type.update(db, {...})` directo sin `.where(...)` → error
de codegen). Esto previene el accidente clásico de "olvidé
el WHERE":

```fitz
let updated_rows = User.where(fn(u) => u.id == 42)
    .update(db, {"age": 36, "role": "admin"})
    .await?

let deleted_rows = User.where(fn(u) => u.role == "trial" and u.age < 18)
    .delete(db).await?
```

`.update` acepta tanto Map literal heterogéneo (`{"key": val,
...}`) como una var `Map<Str, Any>` (caso típico:
`.update(db, body.changes)` con `body` deserializado de JSON
request). Los values de tipos compuestos (List<scalar>, Map para
JSONB) se marshallean con los casts SQL apropiados
(`::text[]`/`::jsonb`).

**Expresiones SQL (v0.32.0)** — los values del Map también aceptan
`sql.now()` → `NOW()` y `sql.raw("<fragmento>")` (inline verbatim, no
parametrizado), y el `.where(...)` soporta `sql.now()` + aritmética de
fechas `m.col.plus_seconds(n)` / `plus_days(n)` / etc:

```fitz
Monitor.where(fn(m) =>
    m.last_check_at.plus_seconds(m.interval_secs) < sql.now())  // due?
    .update(db, {"last_check_at": sql.now(), "streak": sql.raw("streak + 1")})
    .await?
```

El namespace es `sql` (no `db`) porque la conexión se liga como
`let db = db.connect(...)`, que shadowearía el módulo. Para SQL más
complejo (agregaciones condicionales, CTEs, window functions), el
escape hatch canónico sigue siendo `db.query(...)` crudo — ver
[`docs/db-orm.md`](db-orm.md) §7, §8 y §28.

**`Type.bulk_insert(rows, db)`** (v0.10.27) — inserta muchas rows
en batches multi-tuple `VALUES`. Default `batch_size=1000`.
Optimizado para seeds y migraciones: 1 round-trip por batch.
Devuelve el conteo total:

```fitz
let rows: List<User> = []
let mut i = 0
while (i < 5000) {
    rows.push(User { id: 0, email: "u{i}@x.com", age: 20 })
    i = i + 1
}
let n = User.bulk_insert(rows, db).await?
print("inserted: {n}")  // 5000
```

Sentinel `id: 0` detectado de la PRIMERA row (asume shape
uniforme — todas con o todas sin PK explícita). No emite
`RETURNING *` (si necesitás los IDs auto-generados de cada row,
usá `.insert` en loop).

### Composite primary keys + `@index(...)` (v0.10.27)

**Composite primary key**: N `@primary` por type. Útil para
tablas de join (M:M):

```fitz
@table("memberships") type Membership {
    @primary org_id: Int = 0
    @primary user_id: Int = 0
    role: Str = "member"
}
```

`fitz db diff/migrate` emite `PRIMARY KEY (org_id, user_id)`.
Insert con valores explícitos del PK tuple (no sentinel).

**`@index(...)`**: decorator a nivel type que declara índices
auto-emitidos por migrations. Composite, unique, partial:

```fitz
@table("posts")
@index(author_id)
@index(status, published_at)
@index(slug, unique=true)
@index(status, name="published_idx", where_=status == "published")
type Post {
    @primary id: Int = 0
    author_id: Int = 0
    slug: Str = ""
    status: Str = "draft"
    published_at: Str = ""
}
```

Kwargs disponibles: `unique=true`, `name="..."`, `where_=<expr>`
(partial index — el kwarg se llama `where_` porque `where` es
reservada).

### Aggregates scalar + GROUP BY

Sobre `QueryBuilder<Row>` los agregados scalar devuelven `Float`:

```fitz
let total: Int = User.count(db).await?
let avg_age: Float = User.avg(fn(u) => u.age, db).await?
let max_id: Float = User.max(fn(u) => u.id, db).await?
let sum_logins: Float = User.where(fn(u) => u.active).sum(fn(u) => u.login_count, db).await?
```

(Cast `::float8` automático en `avg` para evitar el OID Numeric
del driver — el wire protocol se simplifica.)

**`.group_by(closure)`** devuelve un `Aggregated<Row>`, NO un
`QueryBuilder<Row>`. La diferencia: `Aggregated.count(db)`/
`.sum(...)`/etc devuelven `List<Map<Str, Any>>` con un row por
grupo, no un scalar:

```fitz
let by_role: List<Map<Str, Any>> = User.group_by(fn(u) => u.role).count(db).await?
// by_role[0] → {"role": "admin", "count": 3}
// by_role[1] → {"role": "user", "count": 47}
```

El checker distingue estáticamente `QueryBuilder<Row>` de
`Aggregated<Row>` — typos de método se detectan en compile-time,
no runtime.

### Relations + navigation methods

Después de declarar `@belongs_to`/`@has_many`/`@has_one`, cada
field genera un **método de navegación** sobre la instancia:

```fitz
@table("posts") type Post {
    @primary id: Int = 0
    title: Str
    @belongs_to("User") user_id: Int
}

@table("users") type User {
    @primary id: Int = 0
    email: Str
    @has_many("Post", via="user_id") posts: List<Post>
}

let post = Post.where(fn(p) => p.id == 1).first(db).await?

// BelongsTo: el método se llama como el field FK.
let author: User = post.user_id(db).await?

// HasMany: el método se llama como el field virtual declarado.
let user_posts: List<Post> = author.posts(db).await?
```

Cada navigation hace **un SELECT por call** (lazy). Para
evitar el clásico N+1 (1 SELECT del User + N SELECTs por cada
post), Fitz tiene eager loading.

**Navigation chain**: cuando se llama una navigation `belongs_
to`/`has_many` SIN el `db` (`args.is_empty()`), devuelve un
`QueryBuilder<Target>` que se puede seguir encadenando:

```fitz
// Equivale a SELECT * FROM posts WHERE user_id = author.id LIMIT 5 ORDER BY id DESC
let latest_5 = author.posts()
    .order_by(fn(p) => p.id, ascending: false)
    .limit(5)
    .all(db).await?
```

Por diseño: `user.posts(db)` es el terminal directo,
`user.posts()` empieza chain. Los terminales (`.all`/`.first`/
`.count`/etc.) son obligatorios para ejecutar.

### Eager loading: `.preload(...)`

Cierra el N+1 con **dispatch estático en compile-time**. El
relation name viaja como Str literal en `.preload(...)`; el
codegen emite un `match` exhaustivo por type con la rama
correspondiente. **Typos detectados en compile-time, no
runtime**:

```fitz
// 1 query batch para los users + 1 query batch para
// TODOS los posts WHERE user_id IN (id_user_1, id_user_2, ...)
let with_posts: List<User> = User.preload("posts").all(db).await?
for u in with_posts {
    print("{u.email}: {len(u.posts)} posts")
    // u.posts ya está hidratado — cero queries adicionales.
}
```

En el MVP solo `HasMany`. `BelongsTo` eager (cargar el author
de N posts en 2 queries) queda como refinamiento.

### Tipos avanzados: JSONB, arrays, Map<Str, T>

**JSONB** — un field `data: Map<Str, Any>` se mapea a columna
`jsonb`. INSERT serializa con `serde_json` (`preserve_order`)
y cast `::jsonb`. SELECT parsea el text JSON con `__FitzValue`
(enum tagged que preserva shape heterogéneo). Null Fitz → NULL
real (no la string `"null"`):

```fitz
@table("events") type Event {
    @primary id: Int = 0
    name: Str
    data: Map<Str, Any>   // jsonb column
}

let e = Event.insert(db, Event { id: 0, name: "click",
    data: {"page": "/home", "ts": 1700000000, "user": null}
}).await?
// SELECT round-trip preserva el shape:
let back = Event.where(fn(e) => e.id == e.id).first(db).await?
print(back.data["page"])  // "/home"
```

**Arrays Postgres** — `List<scalar>` ↔ `T[]`:
`List<Int>`/`Str`/`Float`/`Bool` se mapean a `int8[]`/`text[]`/
`float8[]`/`bool[]` respectivamente. INSERT/UPDATE emiten el
cast apropiado (`::int8[]`/etc.). SELECT round-trip preserva
el orden:

```fitz
@table("posts") type Post {
    @primary id: Int = 0
    title: Str
    tags: List<Str>     // text[] column
    scores: List<Int>   // int8[] column
}

let p = Post.insert(db, Post { id: 0, title: "Hola",
    tags: ["rust", "postgres"], scores: [10, 20, 30]
}).await?
print(p.tags[0])  // "rust"
```

**NULL en arrays**: `List<Int?>` ↔ `int8[]` con elementos
nullable. El text format Postgres `{a,NULL,c}` se parsea/encodea
simétricamente:

```fitz
@table("readings") type Reading {
    @primary id: Int = 0
    samples: List<Int?>   // int8[] con NULL aceptable
}
```

**Map<Str, T> concreto** — alternativa a `Map<Str, Any>` cuando
todos los values son del mismo tipo primitivo (Int/Float/Str/
Bool). El marshaling es directo (HashMap<String, T>), sin
overhead de enum dispatch:

```fitz
@table("metrics") type MetricSnapshot {
    @primary id: Int = 0
    counters: Map<Str, Int>   // jsonb con shape homogéneo
}
```

K se restringe a Str (Postgres jsonb keys son strings). `Map<Int,
Int>` → error claro.

**Array ops en `.where(...)`**: operadores Postgres sobre arrays
se mapean a method calls Fitz:

```fitz
// "rust" = ANY(tags)
let rusty = Post.where(fn(p) => p.tags.has("rust")).all(db).await?

// tags @> ARRAY['rust', 'postgres']
let both = Post.where(fn(p) => p.tags.contains_all(["rust", "postgres"])).all(db).await?

// scores <@ ARRAY[1, 2, ..., 100]
let small = Post.where(fn(p) => p.scores.contained_in([1, 2, 3, 4, 5])).all(db).await?
```

### Escape hatch: `db.query`/`db.exec` crudo

Cuando el ORM no alcanza (CTEs complejas, window functions,
`COPY`, extensiones específicas como pgvector, JSON operators
`->`/`->>`/`@>`), el driver crudo sigue disponible:

```fitz
let rows = db.query("
    WITH ranked AS (
        SELECT id, name, ROW_NUMBER() OVER (PARTITION BY group_id ORDER BY score DESC) AS rn
        FROM items
    )
    SELECT * FROM ranked WHERE rn = 1
", []).await?
// rows: List<DbRow>

let affected = db.exec("UPDATE counters SET value = value + 1 WHERE key = $1", ["clicks"]).await?
```

Las dos APIs coexisten — usá el ORM para el 90% del código de
todos los días, bajá a SQL crudo para los casos especiales.

### Ejemplos del cap

Dos ejemplos en `examples/guide/` cubren los dos casos
canónicos:

- **`31-orm.fitz`** (~100 LoC, pedagógico) — muestra el shape
  canónico del ORM end-to-end: `@table` con todos los
  decoradores, insert, where + first, chain
  `order_by`/`limit`/`offset`, operadores extendidos
  (`starts_with`/`is_in`/`between`), aggregates scalar
  (`count`/`avg`), GROUP BY con `Aggregated<Row>`, navigation
  belongs_to/has_many, eager loading con `.preload`, y
  update/delete con guard. `fitz build` produce binario que NO
  requiere Postgres real al compilar; el `connect` runtime
  falla con `Err` clara si la URL es inválida.
- **`31b-orm-crud-http.fitz`** (CRUD HTTP real end-to-end) —
  combina **todo el stack Fitz**: types `User`/`Post` con
  decoradores ORM completos (`@table`/`@primary`/`@belongs_to`/
  `@has_many`), HTTP nativo (`@get`/`@post`/`@put`/`@delete`
  + path params), body deserialization a types custom dedicados
  (`UserInput`/`PostInput` separan el shape DB del shape HTTP
  entrada), `Result<T>` con `?` propagando errores ORM hasta
  el cliente, `env_or(...)` para leer `DATABASE_URL` con default,
  y `@server(port)`. Endpoints: list/get/create/update/delete
  sobre users, relation queries (posts por user), eager loading
  con `.preload(...)` cerrando N+1, aggregate scalar, **GROUP BY
  aggregate** (`/stats/by-email` con `User.group_by(...).count(db)`)
  serializado a JSON. Requiere Postgres real para
  correr — el setup pre-condición está documentado al inicio
  del archivo (createdb + CREATE TABLE). Compila con `fitz build`
  aunque no haya Postgres local — el codegen del ORM se valida
  en compile-time.

### Por qué Fitz hace esto distinto

- **DB nativa, no librería**: el driver Postgres (~2400 LoC en
  `src/db.rs`) y el ORM viven en el binario `fitz`. **Cero
  `pip install psycopg2`, cero `gem install pg`, cero `cargo
  add tokio-postgres`, cero `npm install pg`.** Cuando hacés
  `fitz build` el binario nativo embebe el driver — un
  `.exe`/ELF/Mach-O standalone que habla wire protocol v3.0 +
  SCRAM-SHA-256 sin link a libpq.
- **SQL constante en codegen-time**: cada `.where(closure)`
  se walka del AST DURANTE EL CODEGEN, el fragmento SQL queda
  hard-coded en el binario. Zero overhead runtime para
  construir SQL. Comparable a Diesel/sqlx, **mejor que
  SQLAlchemy/ActiveRecord** que construyen SQL via objetos en
  runtime cada vez.
- **Paridad bit-a-bit `fitz run` ↔ `fitz build`**: lo que ves
  funcionar en el intérprete (rapid feedback) funciona idéntico
  en el binario nativo (deploy a prod). Cero "anda en local
  pero no en server". 16 tests E2E de paridad codegen corren
  contra `postgres:16` en cada push a `main`.
- **Decorators del lenguaje**: `@table`/`@primary`/`@column`/
  `@belongs_to`/`@has_many`/`@has_one` son **parte del
  compilador** (lexer + parser + type checker + codegen),
  no anotaciones procesadas por una lib opcional. El checker
  exige `@primary` único, valida que `@belongs_to("X")`
  apunte a un type existente, infiere los signatures de los
  navigation methods. Spring `@Entity`/JPA + Hibernate
  resuelven esto en runtime con reflection — Fitz lo hace
  en compile-time.
- **Eager loading con dispatch estático**: `.preload("posts")`
  con el relation name como Str literal en compile-time
  produce un match exhaustivo emitido por el codegen. Typos
  (`.preload("post")` sin la "s" final) detectados en
  compile-time, no runtime. Comparable a Diesel's `belonging_
  to` macros, mejor que SQLAlchemy `joinedload(User.posts)`
  donde el typo recién aparece como `AttributeError` al
  evaluar.
- **Integrado con el resto del lenguaje**: tipos custom +
  `Result<T>` + `?` + `match` + decoradores apilables
  (`@authenticated` + `@get` + handler que llama `Type.where(...)
  .all(db).await?`) + middleware/CORS + body deserialization.
  El ORM no es una "isla" con sus propias reglas, encaja
  exactamente con HTTP nativo + auth + jobs + WebSockets.

**Ningún otro lenguaje moderno** combina driver Postgres puro
+ ORM declarativo sobre `type` + paridad bit-a-bit
intérprete↔binario nativo + LSP completo (autocomplete del
ORM end-to-end con tipos refinados en cada chain method) **sin
macros derive ni introspection runtime**. Diesel cubre 4/5
pero requiere derives + runs Rust crudo. SQLAlchemy/ActiveRecord
cubren ORM ergonómico pero pagan reflection runtime. Prisma
genera schemas separados que viven aparte del lenguaje.

### Performance vs los stacks vecinos

Las decisiones de arriba (SQL constante codegen-time + driver
puro + paridad intérprete↔binario) tienen efecto medible. Dos
benchmarks reproducibles en el repo:

- [**`benchmarks/orm-vs-sqlalchemy/`**](https://github.com/Thegreekman76/fitz/tree/main/benchmarks/orm-vs-sqlalchemy)
  — Fitz vs Python+SQLAlchemy en endpoints aislados con `oha`
  sustained c=10. **Fitz 7-9x más rápido en reads**, **5.5x más
  eficiente en memoria**.
- [**`benchmarks/mixed-workload/`**](https://github.com/Thegreekman76/fitz/tree/main/benchmarks/mixed-workload)
  — Fitz vs Python+SQLAlchemy **vs Node+Prisma** bajo mixed
  workload (60/40 reads/writes, 100 VUs peak ramping) sobre un
  dominio `users + posts` con FK. Bajo el peak, Fitz mantiene
  p95 de 11 ms; Python satura a 503 ms p95; Node queda en el
  medio con 11.7x más memoria que Fitz.

Detalle completo, tablas headline y "cómo reproducir" en
[Benchmarks](benchmarks.md).

### Qué no está en el MVP

Items comprometidos como deuda explícita:

- **Migraciones automáticas (`fitz db diff` / `fitz db
  migrate`)**: hoy el user crea las tablas con `db.exec(
  "CREATE TABLE ...")` al boot o con `psql` aparte. Las
  migraciones autogeneradas a partir del diff entre el shape
  declarado en `type` y el real en Postgres llegan como sub-paso
  futuro.
- **Transactions** (`BEGIN`/`COMMIT`/`ROLLBACK`): cada query
  corre en auto-commit. Bloques transaccionales con
  `db.transaction(fn(tx) => ...)` llegan como sub-paso separado.
- **Composite primary keys**: un `@primary` único por `type`.
  Tables con `PRIMARY KEY (a, b)` requieren refactor del checker
  — refinable si aparece presión.
- **TLS strict (`sslmode=require`)**: el driver soporta
  `sslmode=disable`. TLS llega como sub-paso separado (StartTLS
  + cert validation).
- ~~**Date / Time / UUID nativos** como tipos del lenguaje~~ —
  **CERRADO v0.10.24 (intérprete) + v0.10.26 (codegen) + v0.10.30
  (API completion Tier B)**. `Date`/`DateTime`/`Uuid` son tipos
  built-in con constructors (`Date.today/tomorrow/yesterday/parse/
  from_ymd?`, `DateTime.now/epoch/parse/from_timestamp?`,
  `Uuid.v4/v7/nil/parse?`), aritmética (`.add_days/months/years` +
  `.subtract_*`), diff (`.diff_days/seconds/minutes/hours`),
  comparison nativa (`<`/`>`/`<=`/`>=`) y timezone display
  (`.to_local()`/`.in_tz(iana)`). Round-trip Postgres automático.
- **JSON operators avanzados** (`@@`/`||`): los siete operadores
  mapeados como method calls cubren `?`/`?&`/`?|`/`@>`/`->>` +
  `#>`/`#>>` con cast tipado (`has_path` / `path_int` /
  `path_text` / `path_float` / `path_bool`, v0.10.29). Los
  faltantes (text search `@@`, concat `||`) se bajan a
  `db.query(...)` crudo.

Ver [DB y ORM](db-orm.md) sección 28 para el listado completo
de limitaciones y refinamientos pendientes.

### Guía exhaustiva

Este capítulo es el **resumen** del stack DB + ORM. Para la
referencia completa con todos los operadores, recetas (paginación,
búsqueda, auth + ORM, cron jobs), CLI integration y limitaciones
detalladas, ver el documento dedicado **[DB y ORM](db-orm.md)**
(separado de la guía porque el ORM es un dominio aparte del
lenguaje base — ~2500 LoC).

### Cierre

Este capítulo cierra el bloque "stack web first-class" del
lado server: HTTP nativo (cap 17), middleware + CORS (17b),
docs automáticas (18), async (19), build (20), interop Python
(21), auth (28), WebSockets (29), jobs (30), y **DB + ORM (31)**.
Todo en el binario `fitz`, todo con paridad bit-a-bit
intérprete↔binario, todo validado en CI multi-plataforma con
Postgres real en cada push.

Fase 12 (deployment ciudadano) shipped en v0.13.0. Fase 13 (CLI
builder nativo con `@command`) shipped en v0.11.0. **Fase 11
(frontend nativo `.fitzv` compilado a WASM + SSR) shipped en
v0.21.0** (2026-07-16). Lo que queda es 11.7 (client-side dynamic
capabilities + kanban SPA port), 11.8 (LSP support inside
`.fitzv`), y 11.9 (pedagogic docs — cap dedicado en esta guía +
módulo del curso M9). Pero el **stack server completo** ya está
vivo. Si tu objetivo es "escribir una API tipada con auth + DB +
jobs + WebSockets que deploye como un binario standalone": Fitz lo
hace hoy, en un solo lenguaje, con cero `requirements.txt`/
`Cargo.toml`/`package.json` que mantener.

### Transactions (v0.10.14)

Cuando un handler hace **varias escrituras que deben ser
atómicas** (ej: transferí dinero de una cuenta a otra, o creá
un Order + sus OrderItems en una sola operación), envolvelas
en `db.transaction(fn(tx) -> Result<T> { ... })`:

```fitz
async fn transfer(db: DbConn, body: TransferInput) -> Result<Int> {
    return db.transaction(fn(tx) -> Result<Int> {
        // Las queries adentro del callback usan `tx` (mismo tipo
        // DbConn que `db`, pero pegado a la misma conn física).
        // Todos los métodos del ORM funcionan sin cambios.
        let _ = Account
            .where(fn(a) => a.id == body.from_id)
            .update(tx, { "balance": Account.balance - body.amount })
            .await?
        let _ = Account
            .where(fn(a) => a.id == body.to_id)
            .update(tx, { "balance": Account.balance + body.amount })
            .await?
        return Ok(body.amount)
    }).await
}
```

**Garantías**:

- **Atomicidad** — el callback ejecuta dentro de `BEGIN`/`COMMIT`/
  `ROLLBACK`. O todas las queries persisten (COMMIT al retornar
  `Ok`), o ninguna persiste (ROLLBACK automático al retornar
  `Err` o si la closure paniquea). Imposible quedarse a mitad
  de camino.
- **Aislamiento** — todas las queries adentro del callback usan
  la **misma conexión física** (no acquire concurrentes del
  pool). Postgres garantiza el isolation level del server
  (default `READ COMMITTED`).
- **Cleanup automático** — la conn vuelve al pool al final de
  la tx (sea OK o Err). Sin leaks.

**Diferencias clave vs otros ORMs**:

- **SQLAlchemy** usa session+commit explícitos. Tenés que llamar
  `session.commit()` o `session.rollback()` a mano. Si el handler
  retorna antes de commit, la tx queda colgada server-side hasta
  que la conn se cierre. Fácil de olvidarse.
- **Diesel** usa `conn.transaction(|tx| {...})` closure-based
  similar a Fitz — patrón validado por décadas en Rust ecosystem.
- **Prisma** `prisma.$transaction([...])` toma un array de
  operaciones declarativas; menos flexible que callbacks.

**Sintaxis** — desde **v0.10.15** el callback puede ser FnExpr
inline (`async fn(tx) -> Result<T> { ... }`) o fn nombrada
(declarada con `async fn foo(tx: DbConn) -> Result<T> { ... }` y
pasada por ident). Ambas formas funcionan tanto en `fitz run`
como en `fitz build` (paridad bit-a-bit). El inline es la forma
recomendada para closures cortos; nombrada cuando reusás la misma
lógica de tx en varios endpoints.

Captures del outer scope (vars del handler que envuelve la tx)
funcionan vía doble `move` Rust emit (outer + async move) — el
body puede usar `body.field`, `user.id`, args del handler, etc.,
sin tener que pasarlos explícitamente.

**Lo que NO soporta el MVP** (deuda futura):

- **Nested transactions** (SAVEPOINT). Una `db.transaction(...)`
  adentro de otra es no-op semántico — el inner BEGIN se trata
  como savepoint en Postgres pero Fitz lo emite como BEGIN plano
  (que Postgres ignora con warning).
- **Niveles de aislamiento custom** (`READ COMMITTED`,
  `SERIALIZABLE`, etc.) — usa el default del server.
- **Transacciones read-only** (`BEGIN READ ONLY`) — el callback
  siempre puede escribir.

### Migraciones automáticas (v0.10.16)

Hasta v0.10.15 el schema vivía en `db.exec("CREATE TABLE IF NOT
EXISTS ...", [])` al boot del programa (ver `examples/guide/
31b-orm-crud-http.fitz`). Funciona pero es manual y no versionado.
Desde **v0.10.16**, Fitz tiene un subcomando dedicado:

```bash
# 1. Edités `@table type User { ... name: Str = "" }` agregando un field.
# 2. Generás migration vacía.
fitz db new add_name_to_users
# → migrations/20260529150000_add_name_to_users.sql

# 3. Generás el SQL automático y lo redirigís al archivo.
fitz db diff > migrations/20260529150000_add_name_to_users.sql

# 4. Aplicás contra la DB (tracking idempotente en `_fitz_migrations`).
fitz db migrate

# 5. Estado en cualquier momento.
fitz db status

# 6. Revertir el último (v0.10.17 — requiere sección `-- DOWN` en el .sql).
fitz db rollback
```

`fitz db diff` introspecciona el schema real de Postgres
(`information_schema` + `pg_catalog`), lo compara con los `@table
type` declarados, y emite el `ALTER TABLE` / `CREATE TABLE` /
`CREATE INDEX` necesario para sincronizarlos. El comparador es
determinístico (orden seguro de aplicación) e idempotente
(`diff(target, target) == ""`).

**Defaults SQL** — desde v0.10.16, `@db_default("NOW()")` (o
cualquier expresión SQL Postgres válida) hace que el diff emita
`DEFAULT NOW()` en el `CREATE TABLE` / `ADD COLUMN` automáticamente.
`@db_default` sin args sigue siendo marker-only (skipea INSERT,
sin default específico). El diff normaliza
(`NOW()` ↔ `now()`, casts `'foo'::text` ↔ `'foo'`) para evitar
falsos positivos cuando Postgres reporta el default en su formato
canónico.

**Rollback + renames seguros** (v0.10.17): las migrations soportan
secciones `-- UP` / `-- DOWN`; `fitz db rollback [--count N]`
revierte las últimas N aplicadas ejecutando su DOWN adentro de tx.
Renames Fitz-side sin perder datos se marcan con el decorator
transient `@renamed_from("old_name")` sobre el field o el type;
el diff emite `ALTER TABLE ... RENAME COLUMN/TABLE` en vez de
`DROP + ADD`.

**Drift check + stamping** (v0.10.18): `fitz db check` corre el
diff y devuelve exit 0/1 según haya drift entre el schema declarado
y la DB real — hook para CI bloqueante ("no merge si el schema
diverge"). `fitz db stamp <version>` (o `--all`) marca una
migration como aplicada **sin ejecutar el SQL** — útil para
adoptar Fitz en una DB legacy donde el schema ya está aplicado
manualmente.

**Data migrations en `.fitz`** (v0.10.19): el dir `migrations/`
acepta tanto `.sql` (DDL/DML crudo) como `.fitz` (scripts del
propio lenguaje). Para transforms con lógica que SQL crudo no
expresa elegantemente (back-fills condicionales, parseo de JSON
viejo, HTTP calls durante la migración, etc.), declarás
`async fn migrate(db: DbConn) -> Result<Null>` en un `.fitz` y
el runner lo invoca con `db` pre-bindeado. Opcionalmente
`async fn rollback(db: DbConn)` para `fitz db rollback`. Se
intercalan con `.sql` en orden cronológico del prefix timestamp.

**History + offline SQL + squash** (v0.10.20): `fitz db history`
lista las migrations applied (audit log con `applied_at`).
`fitz db migrate --sql` emite el SQL pendiente al stdout para
handoff a un DBA (en lugar de ejecutar). `fitz db squash
<from> <to>` combina N migrations viejas en una sola
(concatena UP en orden + DOWN en orden inverso, mueve files
originales a `migrations/squashed/`, actualiza tracking) —
útil para acelerar bootstrap de devs nuevos en repos con 100+
migrations.

**Schemas custom** (v0.10.21): `@table("schema.name")` mapea
una tabla a un schema Postgres no-`public` (multi-tenant via
schemas, separación dev/test, módulos aislados, etc.). El diff
emite `CREATE SCHEMA IF NOT EXISTS` automático antes del
`CREATE TABLE`. El ORM nativo (SELECT/INSERT/UPDATE/DELETE) usa
el qualified name `"schema"."name"` en TODAS las queries. Sin
`.` en el arg de `@table`, schema = `public` (compat con
código pre-v0.10.21). **Cierra la Fase 10.6 — `fitz db ...`
paquete completo equivalente a Alembic.**

**Limitaciones del MVP**: `ALTER COLUMN ... TYPE` sin USING (cambios
de tipo incompatibles fallan — editá la migration para agregar
`USING (col::int)`); solo schema `public`. Detalle completo +
workflow avanzado en `docs/db-orm.md` sección 26.c.

**Cero deps externas**: ni Alembic ni Flyway ni Liquibase ni
TypeORM CLI. Todo vive en el binario `fitz`. La fuente de verdad
es tu código tipado, no un YAML aparte ni reflection runtime.

**Tier S — observabilidad + introspect (v0.10.28)**:

- **`fitz db inspect`** — introspect del schema **real** de la DB
  (no de tu código), opcionalmente filtrado por `--schema` o
  `--table`, con output texto plano o `--json` machine-readable.
  Útil para auditar antes de migrar, descubrir tablas legacy, o
  comparar dev vs prod. **v0.10.29** suma `--all-schemas` para
  listar TODOS los schemas user-defined a la vez. Detalle en
  `docs/db-orm.md` sec 29.
- **`@index(col, using="gin"|"gist"|"brin"|"hash"|"spgist")`** —
  method override para full-text search (`gin` sobre tsvector),
  range queries (`gist`), large tables resumidas (`brin`), etc.
  sin bajar a `db.exec`. Default `btree` (Postgres default).
- **`FITZ_DB_LOG=1|verbose`** — env var opt-in que loguea cada
  query del driver a stderr. **v0.10.29** suma redaction
  automática de secrets (`password`/`secret`/`token`/`api_key`/
  etc. quedan como `<redacted>`). Cubierto en cap 32.

**Cierre masivo de v0.10.29 — ORM completo**:

- **JSON path operators** — `e.data.has_path([...])`,
  `e.data.path_int([...])`, `e.data.path_text([...])`,
  `e.data.path_float([...])`, `e.data.path_bool([...])` para
  acceso anidado con cast tipado vía `#>` / `#>>` Postgres.
  Cierra el agujero del `.get("k")` single-level.
- **`@@` full-text search** — `body_tsv.matches(query)` con
  `to_tsquery` para syntax avanzada o `plainto_matches(input)`
  para search bars libres.
- **`@unique(col1, col2, ...)`** — composite uniqueness shortcut
  ergonómico (alias de `@index(unique=true)`).
- **`@check_constraint("expr")`** — emite `CHECK (<expr>)` en
  CREATE TABLE para constraints declarativos.
- **Cross-schema FK transparente** — `@belongs_to("User")` con
  target en otro schema (vía `@table("public.User")`) emite
  `REFERENCES "public"."users"(id)` automáticamente.
- **Diff completo de indexes** — el migrator detecta cambios en
  `using` / `where_clause` / `unique` / `columns` cuando los
  nombres matchean, emitiendo `DROP + CREATE` para regenerar.
- **DB errors con SQL + SQLSTATE + params** — el `DbError::Server`
  Display ahora incluye `[<SQLSTATE>]: <msg>` + `[sql: <query>
  params=[...]]` (con redaction).
- **`FITZ_DB_MAX_CONNS`** — env var opt-in para overridear el
  pool size (default 10, clamp `[1, 200]`).

Detalle completo en `docs/db-orm.md`.

---

## 32. Variables de entorno

Para hablar con secrets, ports configurables y rutas dependientes del
deploy, Fitz tiene 3 builtins dedicados desde la mini-fase env (2026-05-22):

- **`env(key: Str) -> Result<Str>`** — lee una variable de entorno.
  Si existe, devuelve `Ok(value)`; si no, `Err("env var X no
  definida")`. Forzás manejo con `?` o `match` (paralelo a
  `find`/`get`/`json.loads`).
- **`env_or(key: Str, default: Str) -> Str`** — la misma operación
  con valor por default. Nunca falla. Útil para config opcional
  (`let port = env_or("PORT", "3000")`).
- **`load_env(path: Str) -> Result<Null>`** — parser KEY=VALUE simple
  que setea las vars vía `std::env::set_var`. Sin auto-load al boot
  (por diseño: explicit > magic). Llamalo en el `main` antes de
  arrancar el server.

### Patrón canónico — secrets y config

```fitz
@server(3000, "0.0.0.0")
fn main() => 0

// Cargar .env si está en el cwd, ignorar si falta (modo dev).
match load_env(".env") {
    Ok(_) => print("[boot] .env cargado"),
    Err(_) => print("[boot] sin .env, leyendo del environment del proceso"),
}

let JWT_SECRET = env_or("JWT_SECRET", "demo-cambiame-en-prod")
let DB_HOST    = env_or("DB_HOST", "localhost")
let LOG_LEVEL  = env_or("LOG_LEVEL", "info")

// Secret obligatorio sin default razonable — fail-fast al boot.
fn load_session_key() -> Result<Str> {
    let key = env("SESSION_KEY")?
    return Ok(key)
}
```

Nota: hoy el decorator `@server(port, host)` recibe `port` como
literal `Int`. Si necesitás puerto configurable, parsealo en el
boot via builtin externo o capturalo desde la guía de tu app. Una
mini-fase futura podría sumar `env_int(key, default) -> Int` para
zafar de la conversión manual.

### Formato del archivo `.env`

```
# Comentarios con `#` ignorados
APP_PORT=3000
JWT_SECRET=super-secret-32-chars-min
GREETING="con espacios entre comillas"

# Líneas vacías ignoradas también
DATABASE_URL=postgresql://user:pass@localhost:5432/db
```

Reglas del parser:
- Líneas vacías y las que empiezan con `#` se ignoran.
- Split por el PRIMER `=` (los `=` adicionales en el valor pasan
  literal).
- Comillas dobles wrapping (`KEY="value"`) se strippean.
- Sin variable expansion (`$VAR`/`${VAR}`), sin multi-line, sin
  escape chars. Si entra demanda, mini-fase futura dedicada.

### Por qué `Result<Str>`, no `Str` con panic

El modelo "sin excepciones" del lenguaje exige que cada operación
que puede fallar lo declare en el tipo. `env("KEY")` puede fallar
(la var no existe), entonces retorna `Result<Str>` — el caller
**tiene** que manejar el caso, ya sea con `?` (propagar) o `match`
(rama explícita). Esto evita el clásico bug de "olvidé exportar la
variable y el server crashea con un mensaje opaco".

Si querés el modelo "siempre devuelve algo", usá `env_or(key, default)`:

```fitz
// env_or nunca falla — perfecto para defaults razonables.
let log_level = env_or("LOG_LEVEL", "info")
```

### `load_env` no auto-carga al boot

A diferencia de Rails/FastAPI/Express, **Fitz NO carga `.env`
automáticamente**. El usuario explícitamente llama
`load_env(".env")?` (o el path que prefiera) al boot. Razón: el
comportamiento implícito de auto-loading es la fuente típica de bugs
"funciona en mi máquina y no en CI" (porque CI no tiene `.env`
y producción tampoco). Hacerlo explícito hace visible qué se carga
y desde dónde.

Si querés el comportamiento auto-load, es 1 línea:

```fitz
let _ = load_env(".env")
// Ignoramos el Result porque si no existe el .env tampoco es error
// (las vars vienen del environment del proceso).
```

### Errores con mensajes claros

Cuando `env(key)` falla, el mensaje cita la key:

```
env var `DATABASE_URL` no definida
```

Cuando `load_env(path)` falla, el mensaje cita el archivo + el
problema específico:

```
no se pudo leer `/config/.env`: No such file or directory (os error 2)
/config/.env:7: línea sin `=` — formato esperado `KEY=VALUE`
/config/.env:12: key vacía antes del `=`
```

### Test de unit usando env vars

Para escribir tests que dependan de env vars, setealas en el test
mismo con `std::env::set_var` desde Rust (o desde el script que
lanza el test runner). El builtin `env()` lee del environment del
proceso, que es lo que cualquier herramienta de testing modifica.

### Observabilidad — `FITZ_DB_LOG` y `FITZ_HTTP_LOG` (v0.10.28)

Dos env vars opt-in para logging del runtime. Zero overhead si no
están seteadas (default silencioso).

**`FITZ_DB_LOG`** — loguea cada query del driver Postgres:

```bash
export FITZ_DB_LOG=1         # simple: SQL + tiempo
export FITZ_DB_LOG=verbose   # además params (truncados a 80 chars)
```

```
[fitz-db 1.2ms] SELECT id, email FROM users WHERE id = $1
[fitz-db 4.1ms verbose] INSERT INTO users (name) VALUES ($1) params=[$1="ada"]
```

**`FITZ_HTTP_LOG`** — access log per-request paralelo a uvicorn /
nginx access log:

```bash
export FITZ_HTTP_LOG=1         # simple: method + path + status + tiempo
export FITZ_HTTP_LOG=verbose   # además User-Agent + Content-Length
```

```
[fitz HTTP 12.3ms] GET /users/42 → 200
[fitz HTTP 4.1ms] POST /users → 201
[fitz HTTP 45.2ms verbose] GET /users → 200 (UA="curl/8.0" len=1234)
```

Ambas:

- Salen a **stderr** — no contaminan el output del programa.
- Loguean tanto en `fitz run` como en el binario compilado por
  `fitz build` (paridad bit-a-bit, hereda gratis del crate
  compartido).
- Cubren todo el tráfico real: HTTP loguea handlers + preflight
  OPTIONS + rutas auto `/openapi.json`/`/docs`; DB loguea
  SELECT/INSERT/UPDATE/DELETE/DDL + queries internas del ORM.
- Mode se fija al primer acceso del proceso (LazyLock). Cambios
  mid-run de la env var NO se reflejan.

**v0.10.29 — redaction de secrets en `FITZ_DB_LOG=verbose`**: los
params correspondientes a campos sensibles (`password`/`secret`/
`token`/`api_key`/`auth_token`/`access_token`/`refresh_token`/
`private_key`/`credential`/`passphrase`/`session_*`/`csrf_token`)
se enmascaran automáticamente como `<redacted>` en el output.
Heurística best-effort que mira ~50 chars antes del placeholder
+ descarta matches separados por `WHERE`/`AND`/`OR`/etc. Sobre-
redacta en bordes ambiguos (INSERT con varias columnas) por
seguridad — NO sustituye una review general antes de prod, pero
cierra el agujero más obvio.

### Pool tuning — `FITZ_DB_MAX_CONNS` (v0.10.29)

```bash
export FITZ_DB_MAX_CONNS=50   # apps con mucho concurrent HTTP load
export FITZ_DB_MAX_CONNS=3    # cron / batch con poco load (default 10)
```

Override del pool size del driver Postgres. Clamp `[1, 200]`,
fallback default 10 ante valores inválidos. Aplica global al
proceso. Se fija al primer acceso (LazyLock); reiniciá para
cambiar.

---

## 33. Observability — logs, spans, métricas, OTel

**Hito de Fase 12.3 (v0.12.0)** — Fitz tiene **observability
ciudadana de primera clase** en el core del compilador. Tres
piezas nativas sobre el stack web first-class:

1. **Structured logging built-in** — `log.info/warn/error/debug(...)`
   con kwargs heterogéneos, output JSON o pretty, filter via
   env var, redacción automática de `Secret`.
2. **Spans HTTP automáticos** — cada request abre un
   `SpanContext` root con `trace_id`/`span_id` OTel-compatibles;
   logs adentro heredan automático el contexto. Counter +
   Histogram con labels para correlación cross-metric.
3. **Bridge OpenTelemetry opcional** — cuando
   `OTEL_EXPORTER_OTLP_ENDPOINT` está seteada, spans y logs van
   al backend (Jaeger/Tempo/Honeycomb/Datadog). Métricas vía
   endpoint `/metrics` Prometheus scrape (Tier3).

Paridad bit-a-bit `fitz run` ↔ `fitz build` para los tres
bloques (con una asimetría dev/prod en el auto access-log — ver
el callout de abajo). Activación dual (compile-time + env var).
Decoradores nativos del lenguaje, no librerías opt-in.

### Panorama vecino

| Lenguaje      | Logs estructurados | Spans HTTP auto | OTel built-in |
|---------------|---------------------|------------------|---------------|
| Python (FastAPI) | `structlog`/`loguru` (lib) | Manual con `opentelemetry-api` (lib) | `opentelemetry-instrumentation-*` (lib) |
| TypeScript (Express) | `pino`/`winston` (lib) | Manual con `@opentelemetry/*` (lib) | `@opentelemetry/sdk-node` (lib) |
| Go (chi/gin)  | `slog` (std lib) o `zap` (lib) | Manual con `go.opentelemetry.io/contrib` (lib) | Lib oficial |
| Rust (axum)   | `tracing` (lib) | Manual con `tower-http` + `tracing-opentelemetry` (lib) | Lib oficial |
| **Fitz**      | **Core compilador** | **Core compilador (auto)** | **Core compilador (env var → on)** |

En FastAPI/Express/Go/Rust, sumar observability completa requiere
~3-5 libs distintas, glue manual, y conocer la matriz de
compatibilidad entre versiones (típica fricción en producción).
**En Fitz, observability viene incorporada al lenguaje**: escribís
`@get("/users")` y ya tenés span, log estructurado de access,
Counter + Histogram, todo correlacionado con `trace_id`, sin
sumar ni una dep.

> **`fitz build` — observability opt-in vía `log.X` (v0.37.1+)**.
> En el binario nativo, la auto-observability (access-log + spans
> + métricas + las deps OpenTelemetry) se activa cuando el programa
> usa **al menos un** builtin `log.X(...)` (en el handler, en
> cualquier fn, o en un módulo importado). Un servidor HTTP que no
> llame a `log.X` compila a un binario **más liviano** que no
> linkea las tres crates OTel (`opentelemetry`/`_sdk`/`-otlp`) ni
> `metrics`/`tracing` — recorta el binario y el tiempo de compilación.
> Con una sola línea `log.info("startup")` recuperás todo el stack.
> **`fitz run` (intérprete) mantiene el auto access-log en desarrollo
> sin condiciones** — es un binario fijo que ya trae todo compilado,
> así que ver los requests en dev no cuesta nada. Es una asimetría
> deliberada dev/prod: dev observa de fábrica, el binario prod opta
> in. El **comportamiento del programa** (responses, tus `log.X`) es
> idéntico en run y build.

### 33.1. `log.info/warn/error/debug` — structured logging

`log` es un módulo built-in del lenguaje (no requiere import).
Cuatro métodos correspondientes a los niveles syslog clásicos:

```fitz
log.info("Server arrancado", port: 3000, env: "prod")
log.warn("Cache miss alta", hit_rate: 0.42)
log.error("DB query falló", error: "timeout", duration_ms: 5234)
log.debug("Loop iter", i: 42, current: "x")
```

**Output**: JSON estructurado a stderr por default.

```json
{"timestamp":"2026-06-03T15:23:45.678Z","level":"INFO","msg":"Server arrancado","port":3000,"env":"prod"}
```

Una línea por log, ordering estable de fields (`timestamp` →
`level` → `msg` → kwargs en orden de declaración). Listo para
ingest por Loki/Datadog/Splunk/etc.

**Pretty mode** con colors ANSI cuando stderr es TTY o cuando
seteás `FITZ_LOG_FORMAT=pretty`:

```
2026-06-03T15:23:45.678Z INFO Server arrancado port=3000 env="prod"
```

**Filtros por nivel** via env var `FITZ_LOG` (default `info`):

```bash
FITZ_LOG=debug ./mi-app    # incluye debug, info, warn, error
FITZ_LOG=warn ./mi-app     # solo warn + error
```

Soporta targets selectivos: `FITZ_LOG="fitz::log=info,h2=warn"`
para filtrar logs por crate (formato `tracing-subscriber`).

**Kwargs heterogéneos**: `Int`, `Float`, `Str`, `Bool`, `Null`,
`Secret<T>` (redactado automático), `List<T>`, `Map<K, V>`,
nominal types (Display de la instance).

```fitz
let user = User { id: 7, name: "ada" }
let api_key = secret("KEY")?
log.info("Login", user: user, key: api_key, tags: ["admin"])
// → {"msg":"Login","user":"User { id: 7, name: \"ada\" }","key":"***","tags":["admin"]}
```

**Reservadas**: `level`, `msg`, `timestamp`, `trace_id`, `span_id`.
Usarlas como kwarg → error de compilación.

### 33.2. Spans HTTP automáticos + correlación

Cada request HTTP arranca un `SpanContext` root con:
- `trace_id` — 32 hex chars (16 bytes), OTel-compatible.
- `span_id` — 16 hex chars (8 bytes).

Todos los `log.X(...)` adentro del handler (incluso adentro de
`fn` llamadas, async tasks, etc.) heredan automático el contexto.
No tenés que pasar el `trace_id` por parámetro — vive en un
`task_local` de tokio que cruza las await boundaries.

```fitz
fn parse_body(json: Str) -> Result<User> {
    log.debug("Parsing body", len: json.len())  // ← trace_id auto
    return Ok(User { id: 1, name: "x" })
}

@post("/users")
async fn create_user(body: User) -> Result<User> {
    log.info("Request recibida", id: body.id)   // ← mismo trace_id
    return Ok(body)
}
```

**Access log automático** en cada request. Sin código extra
emitís:

```json
{"timestamp":"...","level":"INFO","msg":"http.access","trace_id":"...","span_id":"...","http.method":"POST","http.target":"/users","http.status_code":201,"duration_ms":42}
```

Naming OTel-compatible (`http.method`/`http.target`/
`http.status_code`). El `http.target` usa el path template
(`/users/{id}`) NO el path resuelto (`/users/42`) — evita
cardinality explosion en herramientas downstream.

**Métricas built-in**:
- Counter `http_requests_total{method, path, status}` — total de
  requests por endpoint + verbo + status.
- Histogram `http_request_duration_seconds{method, path, status}`
  — distribución de latencia.

Labels EXACTAMENTE iguales entre Counter y Histogram para
correlación cross-metric en Grafana/etc.

**Opt-out por servidor entero** con `@server(observability=false)`
— bypass total del wrapper de instrumentación. Útil en hot-paths
o cuando tenés tu propio sistema de observability custom.

```fitz
@server(3000, observability=false)
fn cfg() => 0
```

### 33.3. OTel exporter — backends reales

Cuando seteás la env var `OTEL_EXPORTER_OTLP_ENDPOINT`, Fitz
instala automáticamente:
- **TracerProvider** que exporta cada span HTTP a `/v1/traces`
  via OTLP HTTP/proto.
- **LoggerProvider** que exporta cada `log.X(...)` LogRecord a
  `/v1/logs` con `trace_id`/`span_id` propagados.

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://collector:4318 ./mi-app
```

Compatible con cualquier OpenTelemetry collector + backends
spec-conformes: **Jaeger**, **Tempo** (Grafana stack), **Honeycomb**,
**Datadog**, **New Relic**, **Lightstep**, **Dynatrace**,
**SigNoz**, **Aspecto**, **Aspecto**, **Uptrace**, etc.

**Otras env vars OTel-standard que Fitz lee**:
- `OTEL_SERVICE_NAME` — nombre del servicio en el backend
  (default `"fitz-app"`).
- `OTEL_TRACES_SAMPLER_ARG` — ratio de sampling Float `[0.0, 1.0]`
  (default `1.0` = todo).

Sin la env var del endpoint, exporter no se instala — zero
overhead, cero conexiones de red. Los logs siguen yendo a stderr;
el access log y métricas Counter/Histogram funcionan igual
(in-memory).

**Correlación logs↔spans en el backend**: cuando OTel está
activo, el `trace_id` que ves en los logs stderr/Loki es **el
mismo** que muestra Jaeger/Tempo. Querys cross-pipeline tipo
"todos los logs del request `abc123`" funcionan sin glue.

### 33.4. `/metrics` Prometheus opt-in

Para users que prefieren Prometheus scrape sobre OTLP push,
activá el endpoint con el kwarg `prometheus=true` sobre el
`@server`:

```fitz
@server(3000, prometheus=true)
fn cfg() => 0
```

> **Cambio en v0.13.1**: el path env var `FITZ_PROMETHEUS=1`
> como override en runtime **ya no funciona**. El binario tiene
> que declarar `@server(prometheus=true)` literal en código —
> el codegen revisa el AST en compile-time para decidir si
> linkear o no `metrics-exporter-prometheus`. La razón: la dep
> + sus transitivos (`prometheus`, `indexmap`, `protobuf`)
> sumaban ~5 min al smoke `compile_e2e` en CI Linux porque se
> linkeaba en cualquier programa con HTTP, no solo en los que
> exportan Prometheus. Trade-off aceptado: production deployments
> declaran Prometheus en código (es la convención normal de
> Kubernetes + scrape configs); el env var override era nice-to
> -have que no justificaba el costo de CI. Ver
> `docs/deudas-post-5b.md` → "Smoke compile_e2e — gating de
> deps emitidas" para el detalle técnico.

Cuando activo, Fitz instala `PrometheusBuilder` como recorder
global del crate `metrics`. Los Counter/Histogram que YA emite
el wrapper HTTP empiezan a popular Prometheus automático. Y
auto-mounta `GET /metrics` con exposition format:

```
# HELP http_requests_total
# TYPE http_requests_total counter
http_requests_total{method="POST",path="/users",status="201"} 42
# HELP http_request_duration_seconds
# TYPE http_request_duration_seconds histogram
http_request_duration_seconds_bucket{...,le="0.005"} 10
http_request_duration_seconds_bucket{...,le="0.01"} 25
...
```

El endpoint vive en el **mismo puerto + transporte** que el
resto de la app (NO un puerto separado). Si declaraste tu
propio `@get("/metrics")`, el tuyo gana — mismo patrón que
`/openapi.json` / `/healthz`.

**Prometheus + OTel**: si los dos están activos, **Prometheus
gana** (solo UN recorder global de `metrics` permitido). El
OTel exporter de spans/logs sigue funcionando; solo las métricas
van a Prometheus en vez de OTLP.

### 33.5. `@trace` y `@metric` — instrumentación manual (Fase 12.7)

La auto-instrumentation HTTP de Fase 12.3 cubre cada request con
span + access log + métricas (en `fitz run` siempre; en `fitz
build` cuando el programa usa `log.X` — ver el callout de opt-in
al inicio del capítulo). Para funciones **business logic** que
querés medir o nombrar como spans dedicados, Fitz tiene dos
decoradores sobre fns user:

```fitz
@trace(name="risk_score")
@metric(name="risk")
fn evaluar_riesgo(perfil: Perfil) -> Float {
    return perfil.score * 1.5
}
```

- **`@trace(name="X")`** abre un `tracing::info_span!("X")` que
  envuelve cada call. Logs emitidos adentro de la fn (con
  `log.info(...)`) heredan automáticamente el `trace_id`/`span_id`
  del span padre.
- **`@metric(name="X")`** registra dos métricas por call:
  - `X_duration_seconds` (histogram) — distribución de la duración.
  - `X_calls_total` (counter) — total de invocaciones.

Sin el kwarg `name=` se usa el nombre de la fn. Son apilables —
podés tener `@trace + @metric` sobre la misma fn. El kwarg
`name=` es opcional sobre cada uno, así que `@trace fn calc()`
con `@metric` produce span `calc` + métricas `calc_duration_seconds`
y `calc_calls_total`.

```fitz
@trace
fn just_trace(s: Str) -> Str { return s }

@metric
fn just_metric(n: Int) -> Int { return n + 1 }
```

**Cuándo usarlos**:
- Sobre fns que cuestan tiempo (queries custom, validaciones
  pesadas, transformaciones de datos) — el histogram te muestra
  P50/P95/P99 en Grafana.
- Sobre paths críticos del business que el access log del request
  no separa (típico: `validate_payment` adentro de un POST /orders).
- Para correlacionar logs cross-handler — el span nombrado aparece
  como parent en Jaeger/Tempo.

**Cuándo NO**:
- Sobre HTTP/WS handlers (`@get`/`@post`/etc) — el checker rechaza
  el stack, porque la auto-instrumentation Fase 12.3 ya cubre esos
  casos con span + access log + métricas. Apilar duplicaría el
  trabajo y confundiría el backend.
- Sobre fns triviales (un return de un literal) — el overhead del
  span entered + Drop guard es ~µs, despreciable, pero el ruido
  en el backend molesta.

**Costo runtime**:
- Cero si NO hay subscriber `tracing` instalado (programa CLI puro
  sin `log.X(...)` ni HTTP) — el span macro expande a no-op.
- ~µs por call cuando hay subscriber — RAII guard que registra
  métricas al Drop. Funciona con `return X` explícito sin código
  muerto (Rust corre Drop antes de retornar).

**Paridad** `fitz run` ↔ `fitz build`:
- En el intérprete, los decoradores son **no-op honesto**: el
  checker valida el shape (kwarg `name=`, no positional, no
  stacking con HTTP), el evaluator los procesa silenciosamente.
  Ningún backend real recibe nada — es exclusivamente para tipar
  consistentemente con el modo compilado.
- En `fitz build` la instrumentación es real: el span entra, las
  métricas se registran, el subscriber `tracing` instalado por
  Fase 12.3 (cuando hay HTTP) las routea.

```bash
$ fitz build calc.fitz
$ ./calc.exe
# stderr: span events de tracing si OTEL_EXPORTER_OTLP_ENDPOINT
# /metrics: histogram calc_duration_seconds + counter calc_calls_total
# (si @server(prometheus=true))
```

Ejemplo runnable: `examples/guide/34-trace-metric.fitz`.

### 33.6. Patrón canónico — stack completo

Lo que armás en una app de producción típica:

```fitz
type User { id: Int, name: Str, role: Str }

@auth_provider
async fn auth(headers: Map<Str, Str>) -> Result<User> {
    let token = headers.get("authorization")?
    let claims = jwt.decode(token, env("JWT_SECRET"))?
    return Ok(User { id: claims["sub"], name: claims["name"], role: claims["role"] })
}

@authenticated @get("/me")
async fn me(user: User) -> Result<User> {
    log.info("/me hit", user_id: user.id)  // ← trace_id auto
    return Ok(user)
}

// `prometheus=true` activa el scrape endpoint. OTel se activa
// solo si OTEL_EXPORTER_OTLP_ENDPOINT está seteada (default off).
@server(3000, prometheus=true)
fn cfg() => 0
```

Deployás con env vars para activar OTel cuando lo necesités:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://collector:4318 \
OTEL_SERVICE_NAME=my-service \
FITZ_LOG=info \
./mi-app
```

Resultado: cada request abre un span en Jaeger, los logs salen
a stderr con `trace_id` (que Loki indexa), y `/metrics` se
puede scrapear con Prometheus. Todo correlacionado, todo
out-of-the-box.

### 33.7. Recetas comunes

**Capturar contexto custom adentro de un span**:

```fitz
@get("/orders/{id}")
async fn get_order(id: Int) -> Result<Order> {
    log.debug("Fetching order", order_id: id)
    let order = orders.find(fn(o) => o.id == id)?
    log.info("Order found", order_id: id, total: order.total)
    return Ok(order)
}
```

**Logs estructurados sin HTTP** (CLI puros también soportan
`log.*`):

```fitz
@command("import")
fn import_data(file: Str) -> Int {
    log.info("Iniciando import", file: file)
    // ... lógica ...
    log.info("Import completo", rows: 1234)
    return 0
}
```

(En CLI puros no hay `trace_id` automático — no hay request
context. Pero los logs estructurados, filtros, y JSON output
funcionan igual.)

**Custom Counter/Histogram desde código del user** (manual con
el crate `metrics` re-exportado, deuda menor — hoy podés
emitir los nombres reservados desde el wrapper y opt-out con
`@server(observability=false)` si chocan).

**Health checks dedicados** con decorators `@healthz` /
`@readyz` (Fase 12.1) — están separados del flujo de
observability pero se combinan bien:

```fitz
@healthz
fn alive() -> Bool => true

@readyz
async fn ready(db: DbConn) -> Result<Bool> {
    let _ = db.query("SELECT 1").await?
    return Ok(true)
}
```

### 33.8. Qué NO hace Fitz (y por qué)

- **Spans anidados ad-hoc** adentro de una fn — `@trace` abre un
  span sobre toda la fn, pero NO hay API para abrir un span
  hijo en una sub-sección (por ejemplo, `span("db query"): { ... }`
  envolviendo solo unas líneas). Workaround: extraé la sección a
  una fn dedicada y poneles `@trace` arriba. Los logs heredan el
  `trace_id` igual.
- **Bridge métricas OTel** (deuda residual #1 de Fase 12.3).
  Las métricas van a Prometheus si `prometheus=true`, o no
  van a ningún lado si `prometheus=false` (Counter/Histogram
  emitidos a recorder vacío). El crate
  `metrics-exporter-opentelemetry = "0.2.1"` que cerraría esta
  deuda está pineado a `opentelemetry_sdk = "0.31"` mientras
  nosotros estamos en 0.32 — esperando release nuevo del crate.
  Mientras tanto: scraper Prometheus → OTel collector cubre el
  caso.
- **Logs sin HTTP no llegan a OTel logs signal** — el bridge
  iter2.b funciona pero requiere SpanContext activo (caso HTTP);
  CLI puros emiten a stderr únicamente. Refinable cuando entre
  demanda.

### 33.9. Lo que viene

- Cap dedicado de OTel queda pendiente de actualizar cuando
  cierre Tier 2 (bridge métricas OTel). Mientras tanto, este cap
  documenta el feature completo end-to-end con el workaround
  Prometheus.
- Spans hijos ad-hoc adentro de fns: la API actual con `@trace`
  envuelve la fn entera. Sub-sección que abra/cierre un span
  sólo sobre unas líneas queda como deuda menor (workaround:
  extraer la sección a fn dedicada y ponerle `@trace`).

### 33.10. Deployment — `fitz docker init` + `fitz docker build` (Fase 12.4)

Para llevar todo el stack (HTTP + logs + spans + métricas + DB) a
producción dentro de un container, Fitz tiene dos sub-comandos:
`fitz docker init` genera Dockerfile + .dockerignore + docker-compose.yml
smart por defecto; `fitz docker build [--tag X]` tag-ea y delega a
`docker build`.

```bash
$ fitz docker init
▶ fitz docker init — proyecto `mi-api` en `/path/al/proyecto`
   detectado: @server(port = 3000)
   detectado: uso de DB (db.X(...)) → compose suma postgres:16-alpine
   detectado: interop Python → runtime fallback a python:3.12-slim-bookworm (libpython3.12 + wget)
   detectado: @cron → compose suma restart: unless-stopped
✓ escrito: Dockerfile
✓ escrito: .dockerignore
✓ escrito: docker-compose.yml
```

El comando lee el `fitz.toml` del cwd (o ancestros), parsea el
entry point declarado en `[bin].main`, y emite:

- **`Dockerfile`** multi-stage: builder
  `ghcr.io/thegreekman76/fitz:${FITZ_TAG}` con `RUN fitz build` →
  runtime adaptativo según el shape del programa:
  - Por defecto **`gcr.io/distroless/cc-debian12`** (~22 MB sin
    shell ni package manager — superficie mínima).
  - Cuando el programa declara `from python import X`, cae a
    **`python:3.12-slim-bookworm`** (~55 MB con libpython3.12 + wget).
    El binario emitido por `fitz build` con interop dynamic-linkea
    `libpython3.12.so` que distroless no incluye.
  - `EXPOSE <port>` se emite automáticamente si el programa declara
    `@server(N)`.
- **`.dockerignore`** con `target/`, `.git/`, `.env*`,
  `__pycache__/`, configs de editor, etc.
- **`docker-compose.yml`** smart adaptativo:
  - Si el programa usa `db.connect(...)` o cualquier `db.X(...)`
    nativo Fitz, el compose suma un service `postgres:16-alpine` con
    healthcheck + volume `pgdata` + `DATABASE_URL` inyectada con
    `depends_on: service_healthy`.
  - Si el programa tiene `@cron`, suma `restart: unless-stopped` al
    service principal para que el scheduler sobreviva
    crashes/redeploys.
  - Si hay `@server(port)` Y el runtime tiene wget disponible
    (`uses_python` → slim-bookworm), suma bloque `healthcheck:` HTTP
    contra `/healthz` (auto-mounteado por Fase 12.1.b). Con distroless
    el compose emite un comentario explicando cómo agregarlo a mano
    si el user cambia el runtime.

Para sobrescribir archivos existentes:

```bash
$ fitz docker init --force
```

Sin `--force`, los archivos existentes se preservan (cero overwrite
accidental de un Dockerfile hand-tuned).

Para construir la imagen sin escribir `docker build` a mano:

```bash
$ fitz docker build              # tag: <package.name>:latest
$ fitz docker build --tag mi/app:v1   # override del tag
```

El wrapper es thin — invoca `docker build -t <tag> .` en el directorio
del manifest y propaga el exit code. Aborta con sugerencia clara si
falta `Dockerfile` (recomendando correr `fitz docker init` primero).

**Producción con OTel**: cuando arranques el container, exportá las
env vars OTel y el binario reportará al backend automáticamente:

```yaml
services:
  app:
    build: .
    environment:
      OTEL_EXPORTER_OTLP_ENDPOINT: "http://otel-collector:4318"
      OTEL_SERVICE_NAME: "mi-api-prod"
      OTEL_TRACES_SAMPLER_ARG: "0.1"   # 10% sampling
      RUST_LOG: "info"
      FITZ_LOG_FORMAT: "json"
      # Nota: `FITZ_PROMETHEUS=1` ya NO funciona como runtime
      # override desde v0.13.1. Para activar `/metrics`, declarar
      # `@server(3000, prometheus=true)` en el código fuente.
```

**Limitaciones conocidas de Fase 12.4** (deuda residual visible):

- **DB indirecta vía interop Python**: el detector `uses_db` solo
  matchea `db.X(...)` nativo Fitz. Programas que usan SQLAlchemy a
  través de `from python import sqlalchemy` no disparan el service
  `db:` en compose. Workaround: usar `--force` y editar a mano, o usar
  el driver Postgres nativo de Fitz (cap 31).
- **Healthcheck HTTP sin distroless**: el bloque solo sale cuando el
  runtime tiene wget (`uses_python`). Para programas no-Python con
  `@server`, el user puede agregar el healthcheck a mano o cambiar el
  runtime (el comentario en el compose explica cómo).
- **Cross-module detection**: el shape se calcula del entry point del
  manifest. `@server`/`db.connect`/`@cron`/`from python import X`
  adentro de un módulo importado no dispara el shape. Workaround:
  declarar todo en el archivo principal (caso típico).
- **`fitz docker build` thin**: sin `--push`/`--platform`/`--no-cache`.
  Para CI multi-platform con `docker buildx`, correr `docker build`
  directo.

### 33.11. Feature flags con `@flag`, `flag()` y `flags.*` (Fase 12.8)

Tercera pieza del Tier 2 de Fase 12 (deploy + `@trace`/`@metric` +
flags). El decorator `@flag("name")` gate-a una fn entera; el
builtin `flag("name") -> Bool` consulta el registry desde cualquier
expresión.

```fitz
@flag("new-checkout")
@get("/v2/checkout")
fn v2_checkout() -> Map<Str, Str> {
    return {"status": "ok", "version": "v2"}
}

@get("/v1/checkout")
fn v1_checkout() -> Map<Str, Str> {
    return {"status": "ok", "version": "v1"}
}

fn main() {
    if flag("dark-mode") {
        print("dark mode activado")
    }
    print("flags conocidas: {flags.list()}")
}
```

**Cómo se configuran**:

1. **Sección `[flags]` en `fitz.toml`** — defaults compile-time:
   ```toml
   [flags]
   new-checkout = false
   dark-mode = true
   beta-feature = false
   ```
   El runtime (`fitz run`) y el binario nativo (`fitz build`) leen
   esta sección al boot. Defaults baked-in al binario; cambiarlos
   exige recompilar.

2. **Env vars `FITZ_FLAG_<UPPERCASE>`** — override runtime sin
   recompilar:
   ```bash
   FITZ_FLAG_NEW_CHECKOUT=true ./mi-app
   FITZ_FLAG_DARK_MODE=false ./mi-app
   ```
   La env var gana al default del manifest. Valores aceptados:
   `true`/`1`/`yes`/`on` y `false`/`0`/`no`/`off` (case-insensitive).
   Cualquier otro string → cae al default.

3. **Default `false`** — sin manifest ni env var, los flags están
   desactivados (política fail-safe: features nuevas opt-in).

**Semántica del decorator** `@flag("name")`:
- Sobre **HTTP handlers** (`@get`/`@post`/`@put`/`@delete`) o **WS
  handlers** (`@ws`): si el flag está off, el wrapper retorna **404**
  con `{"error": "feature 'name' disabled"}` antes de tocar
  middlewares, auth, body parsing. Hot path.
- Sobre **fns regulares**: el decorator NO altera el cuerpo en el
  MVP — el bloqueo se hace declarativo via `flag(name)` dentro de la
  fn. (Patrón canónico: combiná `@flag` con HTTP para gate routes,
  o usá `if flag(name)` puntual adentro del código.)

**Builtins**:

```fitz
flag("name") -> Bool                          // alias funcional
flags.is_enabled("name") -> Bool              // identico a flag()
flags.list() -> List<Str>                     // nombres conocidos
```

`flags.list()` devuelve todos los flags conocidos en orden
alfabético (BTreeSet interno): defaults del manifest + flags
detectados via env vars `FITZ_FLAG_*` (lowercase del suffix). Útil
para dashboards admin o debug.

**Cuándo usarlos**:
- **Canary releases** — `@flag("new-payment-flow")` sobre la ruta
  v2; flag off en producción, on en staging, on para % de tráfico
  via env var por instance.
- **Kill switches** — `@flag("expensive-feature")` que apagás vía
  env var sin redeploy si la feature tira el sistema.
- **A/B testing** declarativo — toggleás por instance o por env
  variable la versión del handler.
- **Beta gated** — features que solo internal users ven hasta que
  estén listas. Combinable con `@admin`/`@requires` para gate por
  role + flag al mismo tiempo.

**Paridad bit-a-bit** `fitz run` ↔ `fitz build`:
- Intérprete: registro global cargado en `resolve_entry` desde
  `manifest.flags`, override por env var, cache lookup.
- Binario nativo: registro estático `__FitzFlagRegistry` con
  `OnceLock`, defaults baked-in via `__fitz_flag_init(...)` al main,
  mismo lookup + cache.
- Ambos modos retornan los mismos resultados para el mismo manifest
  + env vars + flag name.

**Combinable con todo el stack**:

```fitz
@flag("new-checkout")
@requires("editor")
@authenticated
@post("/v2/checkout")
async fn v2(body: CheckoutInput, user: User) -> Result<Receipt> {
    // body parseado, user authenticated + role=editor, flag on
    return Ok(process_checkout(body, user))
}
```

Si la flag está off → 404 inmediato (ANTES de cualquier check de
auth). Si la flag está on pero el user no tiene role `editor` → 403.
Si la flag está on, user role OK pero no auth → 401.

**Lo que NO está en el MVP**:
- **Flags scoped por user/request** — el flag es global por
  proceso. Para Patrones tipo "30% de tráfico" o "solo users beta",
  consultá un servicio externo (LaunchDarkly/Unleash/Flagsmith)
  desde el handler.
- **Hot-reload de flags sin restart** — los flags son inmutables
  durante el lifetime del proceso. Cambio de env var requiere
  reinicio. Mitigable wrappando consultas con TTL si aparece
  demanda real.
- **Decorator @flag sobre fns regulares (no HTTP/WS)** — el shape
  está validado pero la semántica del MVP es no-op: usá
  `if flag(name)` dentro del cuerpo de la fn para gating manual.

Ejemplo runnable: `examples/guide/34b-feature-flags.fitz`.

---

## 34. CLI builder nativo (`@command`)

**Hito de Fase 13 (v0.11.0)** — Fitz tiene un CLI builder nativo en
el core del lenguaje. Una `fn` decorada con `@command("name", desc="...")`
declara un comando CLI; el binario producido por `fitz build` parsea
`std::env::args()` y dispatcha al comando correspondiente, con
**help auto-generado** y **parser de positional args + flags** con
**zero deps externas**.

### Panorama vecino

| Lenguaje / framework      | Stack típico                | Zero deps | Help auto | Tipado estático |
|---------------------------|-----------------------------|:---------:|:---------:|:---------------:|
| Python                    | `argparse` / `click` / `typer` | ❌ (stdlib o pip) | ✅ | ❌ |
| Rust                      | `clap` (derive)             | ❌ (clap)  | ✅        | ✅              |
| Go                        | `flag` stdlib + manual      | ✅ (stdlib)| ⚠️        | ⚠️              |
| Node.js                   | `commander` / `yargs`       | ❌ (npm)   | ✅        | ❌              |
| **Fitz** (v0.11.0)        | `@command` decorator        | ✅        | ✅        | ✅              |

### Sintaxis básica

```fitz
/// Greet a person with optional volume + repetition.
@command("greet", desc="Greet a person")
fn greet(name: Str, loud: Bool = false, count: Int = 1) -> Int {
    let n = count
    while n > 0 {
        if loud {
            print("HELLO, {name}!")
        } else {
            print("hello, {name}")
        }
        n = n - 1
    }
    return 0
}
```

Después de `fitz build greeter.fitz`:

```bash
$ ./greeter Ada
hello, Ada

$ ./greeter Ada --loud
HELLO, Ada!

$ ./greeter Ada --loud --count 3
HELLO, Ada!
HELLO, Ada!
HELLO, Ada!

$ ./greeter --help
USAGE:
    greeter <name> [OPTIONS]

ARGS:
    <name>  (Str)

OPTIONS:
    --loud
    --count <INT>
    -h, --help
```

### Convención de params (sin decorators extras)

Fitz NO requiere decorators sobre cada param. La convención es directa:

| Patrón Fitz                  | Resultado CLI                                   |
|------------------------------|-------------------------------------------------|
| `name: Str` (sin default)    | Positional arg requerido `<name>`               |
| `loud: Bool = false`         | Flag bool sin valor `--loud`                    |
| `count: Int = 1`             | Flag con valor `--count <N>` (default `1`)     |
| `host: Str = "localhost"`    | Flag con valor `--host <STR>`                  |
| `rate: Float = 0.5`          | Flag con valor `--rate <FLOAT>`                |

Reglas:
- Params **sin default** → positional args, en orden de declaración.
- Params **con default** → flags opcionales.
- `Bool = false` → flag boolean; presencia = `true`.
- `Bool = true` queda como deuda menor (requiere convención
  `--no-flag` para negar). Por ahora invertí la lógica.
- El return type debe ser `Int` — es el exit code.

### Subcomandos (multi-command)

Si declarás **2+ `@command`** en el mismo programa, son subcomandos:

```fitz
@command("greet", desc="Greet a person")
fn greet(name: Str, loud: Bool = false) -> Int {
    if loud { print("HELLO, {name}!") } else { print("hello, {name}") }
    return 0
}

@command("status", desc="Show service status")
fn status() -> Int {
    print("status: OK")
    return 0
}
```

```bash
$ ./mybin --help
USAGE:
    mybin <command> [ARGS] [OPTIONS]

COMMANDS:
    greet     Greet a person
    status    Show service status

Run `mybin <command> --help` for more info on a specific command.

$ ./mybin greet Ada
hello, Ada

$ ./mybin status
status: OK

$ ./mybin greet --help
Greet a person

USAGE:
    mybin greet <name> [OPTIONS]

ARGS:
    <name>  (Str)

OPTIONS:
    --loud
    -h, --help
```

### Exit codes

Convención POSIX:
- `0` → éxito (handler retornó `0`).
- `1`+ → error retornado explícitamente por el handler (`return 1`).
- `2` → error de parsing del CLI (comando desconocido, arg faltante,
  tipo inválido). El help se imprime al stderr.

```fitz
@command("fail")
fn fail(reason: Str) -> Int {
    print("error: {reason}", file="stderr")
    return 7  // exit code propagado al shell
}
```

### Detección automática del modo binario

`fitz build` detecta el modo según los decorators del programa:

| Decorators en el programa             | Modo del binario                              |
|---------------------------------------|-----------------------------------------------|
| `@get`/`@post`/`@put`/`@delete`/`@ws` | HTTP server (axum + tokio)                    |
| `@cron` (sin HTTP)                    | Cron-only scheduler bloqueante                |
| `@command` (sin HTTP, sin cron)       | CLI tool (parser argv + dispatch)             |
| Ninguno                               | Script CLI plano (ejecuta `main_stmts`)       |

**Mutuamente excluyentes**: el checker rechaza `@command + @get`,
`@command + @cron`, `@command + @test`, etc. con error claro.

### Modo intérprete (`fitz run`)

El intérprete también soporta el dispatch CLI — útil para
development y para tests. Pasás los args del programa después de
`--` para separarlos de los args de `fitz`:

```bash
$ fitz run greeter.fitz -- greet Ada --loud
HELLO, Ada!
```

**Paridad bit-a-bit**: el output del intérprete coincide
exactamente con el del binario compilado para el mismo argv. Validado
con E2E (`tests/compile_e2e.rs::fase_13_cli_paridad_run_vs_build`).

### Por qué Fitz hace esto distinto

1. **Zero deps externas**. El parser de argv vive en `src/cli.rs`
   (~600 LoC), compilado dentro del binario `fitz`. El binario
   producido por `fitz build` emite el dispatch inline; cero
   `Cargo.toml` extras, cero `pip install`. Single binary.
2. **Estático no reflection**. Los `@command` se enumeran en
   compile-time; el dispatch es un `match` estático sobre el nombre
   del comando, no un dict lookup runtime (como Click/Fire). Typo en
   el nombre del comando se detecta antes de correr.
3. **Convención sin decorators extras**. Click (Python) exige
   `@click.argument(...)` y `@click.option(...)` por cada param.
   Fitz infiere todo del shape del param: con/sin default determina
   positional vs flag. Menos verbosidad para el caso común.
4. **Help auto consistente**. El help string se construye en
   build-time desde los decorators + nombres + tipos. El intérprete
   y el binario emiten exactamente el mismo formato.
5. **Tipado estático del exit code**. El checker exige `Int` como
   return type. Olvidás `return 0` → error de compilación. En
   Python/Node tenés que recordar `sys.exit()` manualmente.

### Tipos soportados en params

| Tipo Fitz | Marshalleable en CLI | Cómo se parsea                              |
|-----------|:--------------------:|---------------------------------------------|
| `Str`     | ✅                   | Pasa directo al param                       |
| `Int`     | ✅                   | `parse::<i64>()` con error claro            |
| `Float`   | ✅                   | `parse::<f64>()`                            |
| `Bool`    | ✅                   | Flag presence = `true`; `--name=false` override |
| `Str?`    | ✅                   | `None` si no se pasa (deuda menor en MVP)   |
| `List<T>` | ❌ (deuda futura)    | Bajá a `Str` separado por comas y `.split`  |
| `Map`     | ❌                   | No tiene sentido en CLI                     |

### Lo que viene

- **`@flag(short="l")` para short flags** (`-l` además de `--loud`).
  Deuda menor — el parser ya soporta el shape, falta el wiring del
  decorator.
- **`Bool = true` con `--no-flag` negación**. Convención de Click
  que esperan algunos users.
- **Subgrupos de comandos** (`mybin orm migrate` vs `mybin orm seed`).
  Más complejo — requiere parser jerárquico.
- **`List<Str>` variádicos** (`mybin run file1.txt file2.txt`).
  Buena para herramientas tipo `cat` o `ls`.
- **Stdin parsing** (`mybin --stdin` lee config de stdin como JSON).
  Deuda futura.

Cap completo del CLI builder: ver `examples/guide/33-cli.fitz` para
el ejemplo runnable + `boilerplates/cli-tool/` para un proyecto CLI
completo con multiple comandos.

---

## 35. Deployment ciudadano primera clase

**Hito de Fase 12 (v0.12.0 → v0.12.4)** — Fitz tiene
**deployment ciudadano de primera clase**: las piezas que en otros
lenguajes son librerías opt-in (health probes, secrets management,
observability, Dockerfiles) están **en el core del compilador** con
detección AST y paridad bit-a-bit `fitz run` ↔ `fitz build`.

Este capítulo es la vista integradora — junta los 4 sub-pasos de Fase
12 (healthz/readyz + Secret + OTel + Docker autogenerado) y muestra
cómo el mismo programa Fitz pasa de `fitz run` local a un container
en producción **sin cambiar una línea de código**, sólo agregando env
vars y un sub-comando.

### Panorama vecino

| Lenguaje      | Health probes  | Secrets    | OTel        | Dockerfile autogenerado |
| ------------- | -------------- | ---------- | ----------- | ----------------------- |
| Python        | flask-healthz / FastAPI custom | python-dotenv + os.getenv | opentelemetry-api + 6 paquetes | ❌ (manual o cookiecutter) |
| TypeScript    | terminus + manual handlers     | dotenv + process.env      | @opentelemetry/* + setup manual | ❌ (manual o Nx generators) |
| Go            | gorilla/mux handlers manuales  | godotenv o env-config     | go.opentelemetry.io/otel + setup | ❌ (manual) |
| Spring (Java) | actuator (opcional)            | @ConfigurationProperties  | spring-cloud-sleuth opcional | ❌ (manual o jib) |
| **Fitz**      | **auto-mount `/healthz` + `/readyz`** | **`secret(key)` + `config(key)` built-ins** | **estructurado + OTLP built-in** | **`fitz docker init` smart** |

Ningún otro lenguaje moderno combina las 4 piezas como features
intrínsecas del compilador, con detección automática del shape del
programa y paridad bit-a-bit intérprete↔binario.

### 35.1. El stack de Fase 12 en una mirada

Las 4 piezas trabajan juntas — cada feature genera información que
las otras consumen:

```
       ┌─────────────────────────────────────────────────────────┐
       │                  PROGRAMA FITZ                          │
       │                                                         │
       │  @server(3000)        @healthz             @cron("*")   │
       │  @get(...)            @readyz              @background  │
       │  @authenticated       db.connect(...)      log.info()   │
       │  fn handler(...)      secret(...)                       │
       └─────────────────────────────────────────────────────────┘
                                  │
                                  ▼
           ┌────────────┬────────────┬────────────┐
           │  12.1      │  12.2      │  12.3      │  12.4
           │ healthz +  │ Secret<T>  │ Logs +     │ Dockerfile
           │ readyz +   │ + config() │ Spans +    │ + compose
           │ SIGTERM    │ + secret() │ Métricas + │ smart por
           │ drain      │            │ OTel       │ detección AST
           │ auto-     │            │            │
           │ mount     │            │            │
           └────────────┴────────────┴────────────┴────────────┘
                                  │
                                  ▼
                          BINARIO STANDALONE
                         (~22-55 MB, distroless)
                                  │
                                  ▼
               ┌────────────────────────────────────┐
               │  fitz docker build                 │
               │  docker compose up                 │
               │  kubectl apply / fly deploy        │
               └────────────────────────────────────┘
```

### 35.2. Healthz y readyz (auto-mount, Fase 12.1)

Todo HTTP server Fitz expone `/healthz` y `/readyz` **automáticamente**
— sin código, sin libs, sin handlers manuales. El binario los emite
desde el wrapper interno del servidor (Fase 12.1.b):

```bash
$ curl localhost:3000/healthz
{"status":"ok"}

$ curl localhost:3000/readyz
{"status":"ready"}
```

**`/healthz`** (liveness probe K8s) — responde `200 OK` mientras el
proceso esté vivo. Si el programa está en deadlock o panicó, axum no
responde, y K8s lo reinicia.

**`/readyz`** (readiness probe K8s) — responde `200 OK` cuando el
proceso está listo para recibir tráfico, `503 Service Unavailable`
durante el "draining" (SIGTERM ya disparó). Esto habilita
**rolling deploys con cero downtime**: K8s deja de rutear tráfico al
pod viejo antes de matarlo.

**Override opcional con handlers custom** — si el programa declara
`@get("/healthz")` o el decorator dedicado `@healthz fn ...`, Fitz lo
prioriza sobre el auto-mount default. Patrón canónico:

```fitz
// `conn.is_closed()` retorna `Future<Bool>` — por eso la fn es async
// y hace `.await` explícito. Asume `let conn_result = db.connect(...).await`
// top-level referenciado por handlers HTTP.
@healthz
async fn check_db_alive() -> Bool {
    let conn: DbConn = match conn_result {
        Ok(c)  => c,
        Err(_) => return false,
    }
    return not conn.is_closed().await
}

@readyz
fn check_warmup() -> Bool {
    // Por ejemplo: false hasta que el cache cargó.
    return cache_ready
}
```

Las dos fns deben retornar `Bool`. Si retornan `false`, el endpoint
devuelve `503 Service Unavailable`. El `Drain` para readyz se activa
automático al recibir SIGTERM.

**Detalle completo**: cap 33.6 de esta misma guía (Observability
incluye los detalles del SpanContext y los headers OTel-compatibles
que `/healthz` también propaga).

### 35.3. Secrets y config tipados (Fase 12.2)

`secret(key) -> Secret<Str>` y `config(key) -> Str` son built-ins que
leen env vars con dos semánticas distintas:

- **`config("DATABASE_HOST", "localhost")`** — devuelve `Str` directo.
  Toma `(key, default)` — si la env var no existe, usa el default.
  Para configuración no sensitive: hostnames, ports, log levels.
- **`secret("DATABASE_PASSWORD")`** — devuelve `Secret<Str>` opaco.
  Toma sólo la `key`; si la env var no existe, error de runtime
  (los secrets deben estar definidos explícitamente, sin defaults
  silenciosos). No se puede imprimir, no se puede loggear, no se
  puede serializar a JSON. Para extraer el valor real hay que llamar
  `.expose()` explícitamente.

```fitz
let db_host = config("DATABASE_HOST", "localhost")  // Str — público
let db_pass = secret("DATABASE_PASSWORD")            // Secret<Str> — opaco

log.info("conectando a {db_host}")               // OK
log.info("password={db_pass}")                   // ❌ checker error
log.info("password={db_pass.expose()}")           // OK pero feo a propósito

let url = "postgres://{db_pass.expose()}@{db_host}:5432/app"
let conn = db.connect(url)?
```

**Redacción automática en logs/JSON** — si un valor `Secret<Str>` cae
adentro de un kwarg de `log.info(...)`, se redacta a `"***"`
automáticamente. Esto evita que un developer distraído filtre
credenciales por accidente:

```fitz
log.info("login", user: user, password: pass)
// → {"timestamp":"...","level":"INFO","msg":"login","user":{...},"password":"***"}
```

El `Secret<T>` también se redacta dentro de `List<Secret<T>>` y
`Map<Str, Secret<T>>` recursivamente.

**Patrón canónico de producción**:

```fitz
@server(3000)
fn main() {
    let db_url = "postgres://{secret("DB_USER").expose()}:{secret("DB_PASS").expose()}@{config("DB_HOST", "localhost")}:5432/{config("DB_NAME", "app")}"
    print("conectando a {config("DB_HOST", "localhost")}")
    db.connect(db_url)?
}
```

Las credenciales nunca aparecen en logs, ni siquiera por accidente.
El `config("DB_HOST", default)` es seguro de loggear.

### 35.4. Observability con OTel (Fase 12.3)

Detalle completo en [cap 33](#33-observability--logs-spans-métricas-otel).
Resumen para deployment:

- **Logs estructurados** → JSON a stderr por default; pretty en TTY o
  con `FITZ_LOG_FORMAT=pretty`. Filter con `FITZ_LOG=info|debug|warn`.
- **Spans HTTP** → cada request abre un span root con `trace_id`/
  `span_id` (32+16 hex), todos los `log.*` adentro heredan el contexto.
- **Métricas** → Counter `http_requests_total{method,path,status}` +
  Histogram `http_request_duration_seconds{...}` emitidos automático.
- **OTLP exporter** → seteando `OTEL_EXPORTER_OTLP_ENDPOINT`,
  `OTEL_SERVICE_NAME` y `OTEL_TRACES_SAMPLER_ARG`, los spans y logs
  van al backend (Jaeger, Tempo, Honeycomb, Datadog, etc.).
- **Endpoint `/metrics` Prometheus** → activable con
  `@server(prometheus=true)` literal en código. Desde v0.13.1
  el path env var `FITZ_PROMETHEUS=1` ya no funciona como
  override de runtime (ver cap 33.4).

### 35.5. Dockerfile + compose autogenerados (Fase 12.4)

`fitz docker init` lee el `fitz.toml` del cwd, parsea el entry point,
detecta el shape del programa, y emite:

- **`Dockerfile`** multi-stage con runtime adaptativo
  (`distroless/cc-debian12` por default, `python:3.12-slim-bookworm`
  cuando `from python import X`).
- **`.dockerignore`** con los excludes típicos.
- **`docker-compose.yml`** smart: suma `postgres:16-alpine` si
  detecta `db.X(...)`, `restart: unless-stopped` con `@cron`,
  healthcheck HTTP cuando hay `@server` + wget disponible.

```bash
$ fitz docker init
▶ fitz docker init — proyecto `mi-api` en `/path/al/proyecto`
   detectado: @server(port = 3000)
   detectado: uso de DB (db.X(...)) → compose suma postgres:16-alpine
✓ escrito: Dockerfile
✓ escrito: .dockerignore
✓ escrito: docker-compose.yml

$ fitz docker build --tag mi-api:v1.0
▶ fitz docker build — tag `mi-api:v1.0` en `/path/al/proyecto`
...
✓ build OK — `mi-api:v1.0`
```

Detalle completo en [cap 33.10](#3310-deployment--fitz-docker-init--fitz-docker-build-fase-124).

### 35.6. Ejemplo runnable: API de producción end-to-end

`examples/guide/35-deploy.fitz` arma un microservicio que combina
**todo el stack de Fase 12 en <100 LoC**. Un endpoint público + un
endpoint protegido + DB + logs estructurados + healthz custom + uso
de `secret()`/`config()`:

```fitz
type User { id: Int, name: Str, role: Str }

@auth_provider
async fn check(headers: Map<Str, Str>) -> Result<User> {
    match headers.get("authorization") {
        Ok(token) => {
            // Validar token contra cache/DB...
            if (token == "Bearer demo-admin") {
                return Ok(User { id: 1, name: "Ada", role: "admin" })
            }
            return Err("token inválido")
        }
        Err(_) => return Err("falta Authorization")
    }
}

@get("/")
fn root() -> Map<Str, Str> {
    log.info("root hit")
    return {"app": config("APP_NAME", "demo"), "version": "v1.0"}
}

@admin
@get("/admin/stats")
async fn stats(user: User) -> Map<Str, Int> {
    log.info("stats request", admin: user.name)
    return {"users": 42, "online": 7}
}

@healthz
async fn db_alive() -> Bool {
    // Devuelve false si la DB no responde.
    return db.is_closed() == false
}

@server(3000, "0.0.0.0")
fn main() {
    let db_url = "postgres://{secret("DB_USER").expose()}:{secret("DB_PASS").expose()}@{config("DB_HOST", "localhost")}:5432/{config("DB_NAME", "app")}"
    log.info("conectando", host: config("DB_HOST", "localhost"))
    db.connect(db_url)?
    log.info("server listo")
}
```

**Local dev** (`fitz run`):

```bash
$ export DB_USER=fitz DB_PASS=fitz DB_HOST=localhost DB_NAME=mi_app APP_NAME=demo
$ fitz run examples/guide/35-deploy.fitz
{"timestamp":"...","level":"INFO","msg":"conectando","host":"localhost"}
{"timestamp":"...","level":"INFO","msg":"server listo"}
```

**Production deploy** (`fitz docker init` + compose):

```bash
$ fitz docker init       # genera Dockerfile + .dockerignore + compose.yml
$ cat > .env <<EOF
APP_NAME=mi-app-prod
OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4318
OTEL_SERVICE_NAME=mi-app-prod
OTEL_TRACES_SAMPLER_ARG=0.1
RUST_LOG=info
FITZ_LOG_FORMAT=json
# FITZ_PROMETHEUS=1   # removido en v0.13.1 — Prometheus se
                     # activa con @server(prometheus=true)
                     # literal en el código (ver cap 33.4).
DB_USER=postgres
DB_PASS=$(openssl rand -hex 32)
DB_HOST=db
DB_NAME=mi_app
EOF

$ docker compose up --build
```

El mismo programa Fitz pasa de local a producción **sin cambiar una
línea de código**. Solo cambian env vars y el sub-comando.

### 35.7. Patrones canónicos de producción

**12-factor compliance built-in** — Fitz cubre los 12 factores de
forma idiomática:

| Factor                 | Cómo Fitz lo cubre                                          |
|------------------------|-------------------------------------------------------------|
| I. Codebase            | `fitz.toml` + git (Fase 9.y).                               |
| II. Dependencies       | `fitz.lock` + Cargo.toml emitido determinístico.            |
| III. Config            | `config(key)` + `secret(key)` lee env vars (Fase 12.2).     |
| IV. Backing services   | `db.connect(url_from_env)` (Fase 10).                       |
| V. Build/release/run   | `fitz build` separa del runtime; binario standalone.        |
| VI. Processes          | Stateless por default; jobs in-memory + persistencia DB.    |
| VII. Port binding      | `@server(N, "0.0.0.0")` explícito.                          |
| VIII. Concurrency      | Tokio multi-thread, scale horizontal.                       |
| IX. Disposability      | SIGTERM drain automático (`/readyz` → 503) + 30s grace.     |
| X. Dev/prod parity     | Mismo binario, mismas env vars, paridad bit-a-bit.          |
| XI. Logs               | JSON estructurado a stderr (Fase 12.3.a).                   |
| XII. Admin processes   | `@command("migrate")` + `fitz run` ad-hoc.                  |

**Smart Dockerfile selection** — `fitz docker init` elige el runtime
automático según el shape del programa:

- Sin interop Python → `gcr.io/distroless/cc-debian12` (~22 MB).
- Con `from python import X` → `python:3.12-slim-bookworm` (~55 MB).
- Con `@cron` → suma `restart: unless-stopped` al compose.
- Con `@server` → suma `EXPOSE` + `ports:`.
- Con `db.X(...)` → suma service `postgres:16-alpine` con healthcheck.

**Cero `pip install` para features intrínsecas** — JWT, Argon2,
logs, spans, métricas, OTLP exporter, Prometheus exporter, todo
embebido en el binario `fitz`. Compará con FastAPI o Express donde
cada una es un paquete separado.

**Paridad bit-a-bit** — el mismo programa corre idéntico con
`fitz run` (intérprete) y `fitz build && ./binario` (Rust nativo).
Esto incluye los headers OTel, las métricas, el shape del JSON de
logs, los códigos de status. No hay "feature X solo funciona en
build" o viceversa.

### 35.8. Qué no está en el MVP

Fase 12 cierra deployment-as-a-citizen al 95%. Lo que falta es deuda
visible documentada:

- **`fitz deploy` orchestrator** — un comando que ejecuta el deploy
  según target (`docker compose up`, `fly deploy`, `kubectl apply`).
  Cap 37 ("Qué sigue") lo menciona como Fase 12.6 diferida (sin
  demanda real todavía).
- **Auto-mount de `/auth/refresh` + `/auth/logout`** — el RBAC custom
  de v0.12.4 (`@requires("role")`) cubre autorización. La revocación
  de tokens necesita los builtins `auth.blacklist`/`auth.is_blacklisted`
  + tabla `fitz_token_blacklist` (sub-iter 9.w.1.iter2.b pendiente).
  Hasta entonces los handlers `/auth/logout` y `/auth/refresh` se
  escriben a mano.
- **Spans hijos ad-hoc** dentro de una fn — `@trace` (Fase 12.7,
  v0.13.0) cubre fns enteras. Spans que envuelvan solo unas líneas
  adentro de la fn quedan como deuda menor. Workaround: extraer
  la sección a una fn dedicada con `@trace` arriba.
- **Feature flags** — `@flag("name") fn ...` + `flag("name") -> Bool`
  + config via TOML/env. Sin demanda concreta, diferido a Fase 12.8.
- **Healthcheck HTTP en distroless** — el compose smart solo emite
  healthcheck cuando hay wget disponible (slim-bookworm), porque
  distroless no tiene shell. Workaround documentado en cap 33.10
  (cambiar runtime o agregar sidecar).
- **CPython embebido en `fitz build`** — `--bundle-python` ya está
  cerrado (cap 21.11), `--bundle-pip` para paquetes pip externos
  está parcial. Los boilerplates 5/6 hoy usan `python:3.X-slim`;
  cuando el sub-paso completo aterrice, pasan a `FROM scratch`.

---

## 36. Frontend nativo con `.fitzv` (SFC)

Hasta acá la guía cubrió Fitz como lenguaje de backend: HTTP,
async, auth, DB, jobs, interop Python. En la mayoría de esos
capítulos el frontend fue "algo aparte" — HTML estático servido
desde un handler, o boilerplate vanilla JavaScript que consume la
API. Fase 11 cierra ese gap: **componentes visuales de primera
clase dentro del lenguaje**, con extensión de archivo dedicada
(`.fitzv`), parser + type checker propios, y dos backends de
compilación — uno server-side (para `fitz-liveviews`) y uno
client-side (WASM).

### Panorama vecino

- **Vue 3 SFC** — single-file components (`.vue`) con `<template>`
  / `<script>` / `<style scoped>`. Reactive via `ref()`/`reactive()`.
  Compila a JS a través de `@vue/compiler-sfc` (herramienta build
  externa). Templating por directivas (`v-if`, `v-for`).
- **Svelte** — sintaxis MUY parecida a Vue SFC. Componentes con
  `.svelte`, reactivity implicit via let bindings, template con
  `{#if}` / `{#each}`. Compila a JS optimizado a través del
  compilador Svelte (herramienta build externa).
- **React JSX** — components como fns JS, templating con JSX
  (JavaScript-como-XML). Reactivity via hooks (`useState`,
  `useEffect`). Compila con Babel/SWC/tsc (toolchain externa).
- **Elm** — componentes como fns puras, template como HTML
  DSL. Compilador Elm dedicado. Fuertemente tipado. Elm no
  interopera con librerías JS sin puertos explícitos.
- **HTMX + templating server-side** — Django templates, Jinja,
  Handlebars, Blade. Cero JS del lado del user, cero build step,
  pero cero components también — cada vista se re-arma manual.
- **Phoenix LiveView (Elixir)** — templating server-side con
  updates via WebSocket sin recargar. Cada vista es una fn
  Elixir + template `.heex`. Compilador Phoenix.

Todos comparten un mismo patrón: **herramienta build separada
del lenguaje anfitrión** (Vue CLI, Vite, Rollup, Webpack, Elm
compile step, etc.), y **template pinado a un backend** (Vue/
Svelte/React → JS client-side, Elm → JS client-side, Phoenix
LiveView → server-side updates via WS, HTMX → server-side full).

### En Fitz

Un `.fitzv` es **un archivo de Fitz** que el compilador reconoce
por su extensión. Adentro escribís un `component <Name> { ... }`
con 4 secciones (state / event / template / style), imports
top-level (`from card import Card`), y opcionalmente helpers
puros que consume el componente. El pipeline es:

**view lexer → view parser → view expand → view check → emit** —
donde `emit` es una de dos rutas:

- **SSR** (`fitz run` / `fitz build` con `fitz-liveviews` como
  dep) — el emitter produce classic Fitz que registra un
  `@live_component` en el runtime del framework. El servidor
  renderiza HTML por WebSocket con diff/patch — patrón Phoenix
  LiveView.
- **WASM client-side** (`fitz build --bin <web> --target
  wasm-client`, opt-in con la feature `client-wasm`) — el
  emitter produce Rust con `wasm-bindgen` + `web-sys` que
  compila a `.wasm` + `.js` bindings. El componente vive en el
  cliente y muta el DOM directo.

**Ningún otro lenguaje moderno del cuadro combina** single-file
components + type checker propio del template + dos backends
alternativos (SSR + WASM) + LSP integrado + zero herramientas
build externas — todo en el mismo binario `fitz`.

### Las piezas

Un `.fitzv` mínimo:

```fitzv
component Counter {
  state { count: Int = 0 }

  event inc() { count = count + 1 }

  <template>
    <div class="counter">
      <span>{count}</span>
      <button data-flv-click="inc">+</button>
    </div>
  </template>

  <style scoped>
    .counter { padding: 1rem; }
    .counter span { font-size: 2rem; }
  </style>
}
```

**`component <Name> { ... }`** — bloque top-level. Cada archivo
`.fitzv` puede declarar uno o varios componentes. El nombre debe
empezar con mayúscula (convención de la view lexer).

**`state { ... }`** — declaración de estado inicial del componente
con anotaciones de tipo obligatorias + defaults obligatorios.
Los campos aceptan primitivos (`Int`/`Float`/`Str`/`Bool`),
`Nullable<T>`, `List<T>`, `Map<K, V>`, y tipos nominales
importados (via `from X import Y`). Los defaults son literales
del lenguaje (`0`, `""`, `[]`, `{}`, `null`, `User { id: 1 }`).

**`event <name>(...) { ... }`** — handlers de eventos. La
signature interna es `(state: <Component>, payload: Map<Str,
Str>) -> <Component>` (el emitter la sintetiza), pero al
escribirlo NO ponés `state`/`payload` explícitos: usás **bare
state field names** (`count = count + 1`) que el emitter
rewrite-a a `state.count`, y el `payload` está en scope como
un `Map<Str, Str>`. Un evento puede referenciar helpers
importados (`cards = cards.map(fn(c) => move_one(target, c))`,
K-4 shipped 2026-07-17).

**`<template>...</template>`** — el HTML del componente. Adentro:

- **Interpolación de expresiones** con `{expr}` — bare state
  field (`{count}`) rewrite a `state.count`; expressions más
  ricas (`{count + 1}`, `{user.name}`) también funcionan.
- **Directivas** — `{#if <cond>} ... {#else} ... {/if}` y
  `{#for <x> in <iter>} ... {/for}` para conditionals + loops.
- **Attribute interpolation** — `data-flv-value-card_id="{c.id}"`
  con `"{expr}"` shape (K-3 shipped). También **mixed**: un valor
  con parte estática + `{expr}` (`class="toast toast-{kind}"`,
  `style="width: {pct}%"`) rewrite-a cada segmento `{...}` a
  `state.<field>` igual que la interpolación de texto (v0.28.7).
- **Event decorators** — `@click="handler_name"` (o su forma
  equivalente `data-flv-click="handler_name"`), `@submit` para
  forms, y `@input` / `@change` para controles de formulario
  (CW.9) — wire eventos del DOM a event handlers del componente.
  Ver la sub-sección "Eventos de formulario" más abajo.
- **Composición** — `<Child prop="value" />` o `<Child
  prop="{expr}" />` para componer components declarados en el
  MISMO archivo (K-3 remainder). Cross-file composition
  diferida (memoria — se ataca cuando el ejemplo grid + forms
  aterrice).

**`<style scoped>...</style>`** — CSS scoped al componente
(class names se rename automáticamente para no chocar con otros
components). También `<style global>` para CSS que se aplique a
toda la página.

**`from X import Y`** — imports top-level. Los `Y` importados
quedan disponibles en `state` (para tipos nominales), en event
bodies (para llamar helpers puros), y en templates (para
computar valores derivados). El path `X` puede ser dotado
(`from utils.shared import X`).

**`from X import Y as Z`** — alias imports (S.1 shipped
2026-07-17). El local binding es `Z`; el classic loader valida
contra `Y` en el módulo origen.

### Interpolación de expresiones

El SSR emitter aplica una **regla de scoping** clara: el
identifier bajo el cursor resuelve en este orden:

1. **Closure params** (`{#for x in xs}` inside template,
   `fn(c) => ...` inside event body) shadow todo lo demás.
2. **State field name** — rewrite a `state.<name>`.
3. **Imported name** — emit verbatim (K-4 shipped).
4. **Otherwise** — error del checker con mensaje que menciona
   la imports table como fix hint.

Ejemplo poniendo los 4 casos:

```fitzv
from board_helpers import cards_in, move_one

component Board {
  state {
    cards: List<Card> = []
    filter_col: Str = "todo"
  }

  event move_right() {
    let target = payload["card_id"]
    // `move_one` viene de imports (regla 3), `target` es local
    // (regla 1), `cards` es state field (regla 2 → state.cards).
    cards = cards.map(fn(c) => move_one(target, "right", c))
  }

  <template>
    <div class="board">
      <!-- `cards_in` viene de imports; `cards` + `filter_col`
           son state fields. -->
      {#for c in cards_in(cards, filter_col)}
        <li class="card">{c.title}</li>
      {/for}
    </div>
  </template>
}
```

### Listas con identidad estable — `{#for x in xs key=x.id}`

Un `{#for}` acepta una cláusula opcional `key=<expr>` que le da a
cada ítem una **identidad estable**:

```fitzv
{#for row in rows key=row.id}
  <li>{row.name}</li>
{/for}
```

Es azúcar: desugarea a un atributo `data-flv-key="{<expr>}"` sobre
el elemento raíz del cuerpo del loop (arriba, el `<li>`). En el
target SSR, cuando la lista cambia entre renders, el motor de diff
del cliente (v0.16.0) **matchea los ítems por su `key`** en vez de
por posición — un insert/remove/move en el medio de la lista se
resuelve como una sola operación estructural (`insert_keyed` /
`move_keyed` / `remove_keyed`) en lugar de una cascada de patches
posicionales. En el target WASM el `data-flv-key` se emite como un
atributo DOM normal (paridad, inocuo).

Reglas:

- El `<expr>` se evalúa con la variable del loop **en scope** (`row`
  arriba), así que típicamente es `row.id` o el ítem mismo (`key=row`
  para una `List<Str>`). Se type-chequea y se emite como cualquier
  interpolación del cuerpo.
- El cuerpo del loop debe tener **exactamente un elemento raíz** que
  cargue la `key`. Cero elementos (solo texto/interpolación o un
  `<Child />`) o más de uno → error claro del expander.
- Sin `key=`, el `{#for}` es idéntico a antes (el diff sigue siendo
  posicional; byte-for-byte para los SFC existentes).

> **`key=` en `{#for}` vs `key="{expr}"` en `<Child />`** son cosas
> distintas: el primero (esta sección) emite `data-flv-key` para el
> diff del SSR; el segundo (más abajo, target WASM) le da identidad a
> una instancia de componente hija en la keyed instance cache. No se
> mezclan.

### Cross-file types

Los tipos custom del componente vienen normalmente de un
archivo `.fitz` classic sibling, importado por
`from <module> import <Type>`:

```fitz
// card.fitz
type Card {
  id: Str
  title: Str
  author: Str
  column: Str
}
```

```fitzv
// Board.fitzv
from card import Card

component Board {
  state { cards: List<Card> = [] }
  // ...
}
```

El view checker reconoce el nominal `Card` en el `state` block
y lo valida contra el módulo origen (regresa al classic checker
downstream). Vale también para struct literals inline en event
bodies:

```fitzv
event add() {
  cards.push(Card {
    id: "1",
    title: payload["title"],
    author: payload["author"],
    column: "todo"
  })
}
```

### Composición de components

Cuando declarás dos components en el MISMO archivo, uno puede
componer al otro con syntax XML-like `<Child />`:

```fitzv
component Board {
  state { title: Str = "My Kanban" }
  <template>
    <div>
      <h1>{title}</h1>
      <Column name="To Do" />
      <Column name="In Progress" />
      <Column name="Done" />
    </div>
  </template>
}

component Column {
  state { name: Str = "" }
  <template>
    <section>
      <h2>{name}</h2>
    </section>
  </template>
}
```

**Props** — cada attribute del `<Child ... />` matchea con un
state field del child. Aceptado:

- Primitivos (`Str`, `Int`, `Float`, `Bool`, `Nullable<T>`)
  como valor de string (`name="Foo"`, `count="42"`).
- `List<T>` de primitivos vía comma-separated (`tags="a,b,c"`,
  K-3 shipped).
- `Map<Str, Str>` vía `k=v,k=v` (S.2 shipped).
- **Interpolación** `<Child prop="{expr}" />` (K-3 remainder
  shipped) — `expr` es cualquier expresión Fitz que respete la
  regla de scoping del parent's template.

**Cross-file `<Child />` composition** (target WASM, shipped
v0.25.0) — el `<Child />` puede vivir en un `.fitzv` SEPARADO,
importado con `from Card import Card`. El emitter carga el
sibling `.fitzv` (parse → expand), inlinea el componente entero
(struct + `new` + event handlers + render + `<style scoped>`) en
el crate generado, y el checker valida la composición del parent
(props, `@event`, slots) contra la **surface real** del child.
Toda la superficie de composición cruza el borde de archivo:
props ↓, event bubbling ↑, slots default + named con fallback.

```fitzv
// Card.fitzv
component Card {
  state { title: Str = "Untitled"  likes: Int = 0 }
  event like() { likes = likes + 1 }
  <template>
    <article>
      <slot name="badge">·</slot>
      <h2>{title}</h2>
      <slot>(no body)</slot>
      <button @click="like">♥ {likes}</button>
    </article>
  </template>
}

// App.fitzv
from Card import Card
component App {
  state { total: Int = 0 }
  event bump() { total = total + 1 }
  <template>
    <main>
      <p>Total likes: {total}</p>
      <Card title="Patagonia" @like="bump">
        <span slot="badge">NEW</span>
        <p>Body rendered in the parent's scope.</p>
      </Card>
    </main>
  </template>
}
```

`Card` se define UNA vez, en `Card.fitzv`, y se reusa sin
copy-paste — la misma historia de reuso que los módulos clásicos
de Fitz, ahora para components `.fitzv` en el target WASM.
Ejemplo runnable: `examples/view/cross-file-child/`.

Desde **v0.26.0** la composición cross-file soporta:

- **Aliasing** — `from Card import Card as Row` resuelve `<Row />`
  (el loader registra un clon renombrado bajo el alias, manteniendo
  el original para composición interna de siblings).
- **Transitividad** — el parent importa solo lo que usa directo; si
  un child importado compone a su vez un grandchild en OTRO archivo,
  el walk de imports lo descubre e inlinea sin que el parent tenga
  que importarlo. Ejemplo: `examples/view/cross-file-transitive/`.
- **LSP cross-file** — editar un `.fitzv` en VSCode ya no marca al
  child cross-file como desconocido: el LSP deriva el directorio del
  documento y resuelve los sibling components importados.

Desde **v0.29.6 (CW.8)** el import cruza también el borde de
**dependencia**: un `.fitzv` compilado al target WASM puede importar
un componente de una dep declarada en `fitz.toml`, no solo de un
sibling del mismo directorio. La sintaxis es el import punteado que
ya usa el loader classic:

```fitzv
// El consumidor declara `fitz_liveviews` como dep en su fitz.toml.
from fitz_liveviews.ui.Badge import badge as Badge

component App {
  state { n: Int = 0 }
  <template>
    <div><Badge label="live" variant="primary" /></div>
  </template>
}
```

El emitter resuelve `Badge.fitzv` bajo el root de la dep (el
directorio del `[lib].entry`), lo inlinea en el crate WASM
standalone, y — como cualquier componente de la companion UI usa
`{flv(...)}` para escapar HTML — el `flv` baja a la identidad en
WASM (un text node del DOM escapa intrínsecamente). Esto destraba
**consumir la companion UI de `fitz-liveviews` como librería** desde
una app WASM externa, sin copiar los componentes al directorio de la
app. Nota: el componente real se llama `badge` (minúscula), y la
heurística "tag capitalizado = componente" exige el alias
(`badge as Badge`) para poder escribir `<Badge />`.

Límites restantes: el component local gana ante uno importado del
mismo nombre; un componente de la dep que compone un sibling por
**nombre pelado** (`from Icon import Icon`, no
`from dep.ui.Icon import Icon`) no se resuelve contra el directorio
de la dep — los primitivos presentacionales dual-target son hojas,
así que no los toca. El path SSR usa el runtime API
`component("ChildName", "instance-id")` del `fitz-liveviews` package
para composición cross-file.

**Keyed children dentro de `{#for}`** (target WASM, R2b shipped)
— podés componer un child DENTRO de un loop, dándole una
identidad estable con el atributo reservado `key="{expr}"`:

```fitzv
component App {
  state { columns: List<Str> = ["To Do", "In Progress", "Done"] }
  <template>
    <div class="columns">
      {#for name in columns}
        <Column key="{name}" title="{name}" />
      {/for}
    </div>
  </template>
}
```

El `key` NO es un prop — es la identidad del child. En el target
WASM el emitter mantiene una **keyed instance cache** (una
instancia por `key`): una key que ya existe **reusa** su
instancia (su state local sobrevive el re-render del parent),
una key nueva la crea, y las keys que desaparecen de la lista se
evictan (reconciliation). Así cada child en el loop conserva su
propio state — igual que la persistencia de sites estáticos, pero
por-key. Aplica a `List<primitive>` y — desde R3 (v0.21.8) — a
`List<nominal>` (un `type` classic importado): `{#for c in cards}`
+ field access `{c.title}` + keyed `<Child key="{c.id}" />`.
Ejemplos runnable: `examples/view/keyed-composition/` (primitivos)
y `examples/view/nominal-list/` (nominales).

### Eventos de formulario — `@input` / `@change`

Además de `@click` (y `@submit` para forms), el target WASM wire
**`@input`** y **`@change`** (CW.9, v0.29.7) sobre controles de
formulario. La diferencia con `@click` es que estos **traen el
valor**: el emitter lee el valor actual del elemento (`<input>`,
`<select>` o `<textarea>`) y llama al handler con un `payload`
que lleva ese valor bajo la key `"value"`. El handler lo lee con
`payload["value"]` y lo escribe al state:

```fitzv
component LiveInput {
  state {
    name: Str = ""
    color: Str = "red"
  }

  event on_name() { name = payload["value"] }
  event on_color() { color = payload["value"] }

  <template>
    <select @change="on_color">
      <option value="red">Red</option>
      <option value="green">Green</option>
    </select>
    <p style="background: {color}">Picked: {color}</p>

    <input @input="on_name" value="{name}" placeholder="Type here" />
    <p>Hello, {name}!</p>
  </template>
}
```

- **`@input`** dispara en cada tecla; **`@change`** al seleccionar/
  perder foco. Los dos entregan `payload["value"]`.
- Un handler de `@input`/`@change` **debe** leer `payload["value"]`
  (es lo único que el evento carga) — si lo ignora, es un error de
  compilación con un puntero claro.
- El emitter cubre `<input>`, `<select>` y `<textarea>` con el mismo
  wiring, y agrega las features web-sys correspondientes al
  `Cargo.toml` generado **solo** cuando el componente usa
  `@input`/`@change` (paridad byte-a-byte para components que no
  los usan).
- Es la misma convención que el SSR emitter, que baja cualquier
  `@event` a `data-flv-<event>` — así el MISMO `.fitzv` sirve a los
  dos targets.

> **El caret vive (keep-node, Fase 11.10).** Un componente con un
> control de formulario vivo (`@input`/`@change`) sobre un template
> estático **construye el DOM una vez y parchea in-place** en cada
> cambio de state: sólo se actualizan los nodos de interpolación
> (`set_data` en un text node, `set_attribute` en un elemento). El
> `<input>` nunca se re-crea, así el caret se queda donde lo pusiste
> mientras tipeás — verificado en Chrome (caret tras `He`, tipear
> `XY` da `HeXYllo`, no `HeXlloY`). El modelo keep-node es opt-in
> automático (gated a value-input); todo otro componente conserva el
> naive re-render byte-idéntico.

Ejemplo runnable: `examples/view/live-input/`.

### Reactividad (Fase 11.10)

Sobre el modelo keep-node del `<input>` vivo, tres capacidades más:

**`{#if}`/`{#for}` con inputs vivos** — un componente value-input que
además tiene control-flow (`{#if}`/`{#for}`) sigue en keep-node: cada
`{#if}`/`{#for}` es una **región anclada** (dos comment anchors) que se
reconstruye entera en cada cambio, mientras el `<input>` (fuera de la
región) se parchea in-place. Destraba el caso **search/filter as you
type**: tipeás en el input, la lista filtra, y el caret no salta. Los
nodos *dentro* de una región se re-crean (sin identidad per-item);
el caret se preserva para el `<input>` que vive *fuera* de las regiones.
Ejemplo: `examples/view/search-filter/`.

**Loading a mitad de un `@rpc`** — un handler async que escribe state
**antes** de un `.await` (típico `loading = true`) pinta ese estado
mientras el request está en vuelo: el compilador emite un render justo
antes de suspender. Un handler sin escritura antes del await (el patrón
fetch-then-assign) queda byte-idéntico. Ejemplo: `examples/view/rpc/`
usa `{#if loading}Loading…{/if}`.

**`derived { ... }`** — valores read-only computados del state (y de
otros derivados), referenciados como `{full}`:

```fitzv
component Greeter {
  state {
    first: Str = "Ada"
    last: Str = "Lovelace"
  }
  derived {
    full: Str = "{first} {last}"       // lee state
    greeting: Str = "Hello, {full}!"   // derivado-de-derivado
  }
  <template>
    <input @input="on_first" value="{first}" />
    <p>{greeting}</p>
  </template>
}
```

Bloque hermano de `state`, mismo shape `name: T = expr`. Los derivados
se computan en orden de declaración (uno puede leer otro anterior) y se
recalculan en cada render/patch. Alcance de este slice: tipos primitivos
(`Str`/`Int`/`Float`/`Bool`), read-only, recomputados-cada-render (no
"solo-cuando-cambia-la-dep"), target WASM. La reactividad fine-grained
real y los derivados async quedan como iteración futura. Ejemplo:
`examples/view/derived/`.

### Estilos scoped

`<style scoped>` aplica **rewriting automático** de class
selectors — cada `.class` en el CSS se sufija con un scope
único del componente (`.class-a1b2c3`), y el template rewrite-a
los `class="foo"` para matchear. Resultado: cero colisiones con
CSS de OTROS componentes o del layout global.

```fitzv
<template>
  <div class="wrapper">
    <span class="badge">{count}</span>
  </div>
</template>

<style scoped>
  .wrapper { padding: 1rem; }
  .badge { font-weight: 600; }
</style>
```

Emitted CSS (post-scoping):

```css
.wrapper-a1b2c3 { padding: 1rem; }
.badge-a1b2c3 { font-weight: 600; }
```

`<style global>` NO aplica rewriting — el CSS va tal cual al
DOM. Útil para reset de body, tipografía global, etc.

### Los dos backends de compilación

**Target SSR (fitz-liveviews)** — el default. `fitz run` /
`fitz build` corren el pipeline view → emit classic Fitz →
compile classic con la dep `fitz-liveviews` en el manifest.
El emitted classic Fitz declara:

- **`type <Component>`** con los state fields.
- **Fn `<Component>_render(state: <Component>) -> Html`** que
  produce el HTML del template.
- **Fn `<Component>_<event>(state: <Component>, payload:
  Map<Str, Str>) -> <Component>`** por cada event handler.
- **`flv_register("Component", <Component> {}, <Component>_
  render, {"event_name": <Component>_event, ...})`** al boot
  (auto-inject via §9.bb).

El runtime `fitz-liveviews` mount el componente vía
`component("Component", "instance-id")` (llamado desde un
`@get(...)` handler) y despacha eventos via
`dispatch_component_events(frame)` en un `@ws(...)` handler.
Ver el capítulo 29 (WebSockets) para el pattern completo.

**Target WASM client-side** — opt-in con `fitz build --bin
<web> --target wasm-client` bajo la feature `client-wasm`. El
emitter produce Rust con `wasm-bindgen` que compila a `.wasm`
+ `.js` bindings via `wasm-pack`. Cada component se instancia
con `<Component>::new().mount_into(root)`. El estado vive en
`Rc<RefCell<T>>` cells por field; event handlers mutan
directamente.

Ejemplo de deploy del counter demo:

```bash
fitz build --bin counter-web --target wasm-client
# → dist/counter-web/pkg/ contiene .wasm + .js
# Servís con: python -m http.server -d dist/counter-web/
```

Bundle size del counter demo: **11.4 KB gzipped sobre 40 KB
gate** (28.6 KB headroom, medido en la Phase 11.4.c).

### Hidratación SSR → client (Phase 11.12)

El target WASM puede **adoptar** el DOM que el server ya pintó en
vez de fresh-mountearlo: en boot, el `start()` generado detecta que
el mount root ya tiene contenido, restaura el estado serializado
desde un `<script type="application/json" id="__flv_state_<Comp>">`,
recorre el DOM existente cableando los listeners, y **no wipea**. Así
el primer paint no parpadea y cualquier JS ya atado a los nodos del
server sobrevive.

Un componente **keep-node** (un control de formulario vivo `@input`/
`@change` sobre un template estático o con regiones `{#if}`/`{#for}`)
hidrata **automáticamente** — de ahí en más parchea in-place, así el
`<input>` vivo conserva el caret.

Un componente de **composición naive** (`<Child />` + `<slot>`)
hidrata con **opt-in**: poné el marcador `hydrate` en el component
ROOT y se propaga a todo el árbol.

```
component App hydrate {
  state { title: Str = "..." }
  event like() { ... }

  <template>
    <div class="page">
      <Card>
        <p>Contenido de slot en scope del PADRE: {title}</p>
        <button @click="like">like</button>
      </Card>
    </div>
  </template>
}

component Card {
  state { taps: Int = 0 }
  event tap() { taps = taps + 1 }
  <template>
    <section class="card">
      <div class="body"><slot><em>fallback</em></slot></div>
      <button @click="tap">tap</button>
    </section>
  </template>
}
```

`App.hydrate` adopta el wrapper `<div class="__fitz-child-Card">` que
el server pintó y llama `Card.hydrate(wrapper)`; el `<slot>` del hijo
adopta el contenido provisto por el padre (en scope del padre, así
`@click="like"` cablea a `App`); el estado del hijo se reusa del
cache de instancias a través de un re-render del padre. La
composición no tiene patch in-place, así que el **primer** cambio de
state re-renderiza wholesale (naive) — hidratar sólo elimina el flash
del primer paint. Sin el marcador `hydrate`, la composición
fresh-mountea igual que antes. Ejemplo runnable en
`examples/view/hydrate-composition/`.

### Editor support (LSP)

La extensión VSCode + el binario `fitz-lsp` reconocen `.fitzv`
como surface de primera clase (Phase 11.8 shipped 2026-07-18,
v0.21.3). Al editar un `.fitzv` obtenés:

- **Diagnostics en vivo** — errores del view lexer/parser/
  expand/check aparecen subrayados en rojo al tipear, con
  mensaje + ubicación en el `.fitzv` (no en el emitted classic).
- **Completions** en 4 clases:
  - Template directives (`{#if}`, `{#for}`, etc.) tras `{` o
    `{#` (SNIPPETs con tabstops).
  - Event decorators (`click`, `submit`) tras `@` en attribute
    position.
  - State field names del enclosing component.
  - Event handler names.
- **Hover** sobre state field ref muestra `<name>: <type> —
  state field of <Component>`. Sobre event handler ref muestra
  `event <name>() — event handler of <Component>`.
- **Go-to-definition** salta a la línea de declaración —
  state field ref → `<name>: <type>` line en state block;
  event handler ref → `event <name>(...)` line.

Ver el capítulo 22 (Soporte para editores) para el flow
general del LSP + cómo la extensión bundlea el binario.

### Ejemplo runnable — Contador Fitz-LiveViews

Este ejemplo vive en `d:/fitz-liveviews/examples/counter/`.
Estructura:

```
counter/
├── fitz.toml         # dep de fitz-liveviews
├── src/
│   ├── Counter.fitzv # el componente
│   └── main.fitz     # HTTP + WS wire-up
```

`Counter.fitzv`:

```fitzv
component Counter {
  state { count: Int = 0 }

  event inc() { count = count + 1 }
  event dec() { count = count - 1 }
  event reset() { count = 0 }

  <template>
    <div id="counter-app">
      <h1>Counter: {count}</h1>
      <button data-flv-click="dec">−</button>
      <button data-flv-click="inc">+</button>
      <button data-flv-click="reset">Reset</button>
    </div>
  </template>

  <style scoped>
    #counter-app { padding: 2rem; font-family: system-ui; }
    button { padding: 0.5rem 1rem; margin: 0 0.25rem; }
  </style>
}
```

`main.fitz`:

```fitz
from fitz_liveviews import Html, html, live_layout,
  html_response, LiveFrame, diff_html, component,
  dispatch_component_events, flv_register
from Counter import Counter, Counter_render,
  Counter_inc, Counter_dec, Counter_reset

let last_html: Str = ""

@get("/")
fn page() -> Response {
  let initial = component("Counter", "main")
  return html_response(live_layout(
    "/live/counter", "counter-app", initial
  ))
}

@ws("/live/counter")
async fn socket(ws: WsConn<LiveFrame>) {
  loop {
    let frame = ws.recv()?
    if (last_html == "") {
      last_html = component("Counter", "main").raw
    }
    let _handled = dispatch_component_events(frame)
    let new_html = component("Counter", "main").raw
    let patches = diff_html(last_html, new_html)
    ws.broadcast(LiveFrame {
      html: new_html,
      patches: patches
    })?
    last_html = new_html
  }
}

@server(3000)
fn main() => 0
```

Correr:

```bash
fitz run
# → http://127.0.0.1:3000/ — clickeá los botones, mirá el
#   contador subir/bajar. El estado vive en el server; cada
#   click manda un frame WS, el servidor recomputa el HTML +
#   emite un diff, el cliente aplica el patch.
```

### Funciones de servidor — `@rpc` (Fase 11.11)

Un `.fitzv` corre en el navegador (WASM), pero muchas cosas
—consultar la DB, verificar auth, leer un secret— **tienen** que
pasar en el server. El puente clásico es escribir un handler HTTP,
un `fetch` a mano, y el marshaling JSON de ida y de vuelta. Fitz
elimina las tres cosas: marcás una `async fn` classic con `@rpc` y
la llamás desde el `.fitzv` **como si fuera local**.

**El panorama vecino.** Es el patrón "server functions" que
popularizaron Next.js Server Actions, Remix loaders/actions,
SvelteKit `+page.server`, tRPC y Phoenix. En todos ellos hay un
paso de infraestructura (un `"use server"`, un router tRPC, un
generador). En Fitz el compilador emite **las dos mitades** desde
una sola declaración, con tipos de punta a punta y sin dependencias
externas.

```fitz
// api.fitz (classic — tiene db, auth, secrets)
@rpc
async fn get_user(id: Int) -> Result<User> {
  let conn = db.connect(db_url()).await?
  return User.where(fn(u) => u.id == id).first(conn).await
}
```

```fitz
// App.fitzv (client-WASM)
from api import get_user, User

component App {
  state { name: Str = "" }

  event load() {
    let u = get_user(42).await?   // ← round-trip tipado, como si fuera local
    name = u.name
  }

  <template>
    <p>{name}</p>
    <button @click="load">Cargar usuario 42</button>
  </template>
}
```

**Qué genera el compilador:**

- **Del lado server** (`fitz build --bin server`): la `@rpc` fn se
  monta como `POST /__rpc/<name>`. El body es un objeto JSON con un
  campo por parámetro (`{"id": 42}`); cada param se deserializa de su
  campo, la fn corre, y su `Result<T>` se vuelve `200` + JSON (Ok) o
  `500` + `{"error": ...}` (Err). Reusa toda la cadena de `@post`
  (observability, panic-catch, etc.).
- **Del lado client** (`fitz build --bin web --target wasm-client`):
  la `@rpc` fn importada se emite como un stub `fetch` async — su
  cuerpo (server-side) **no** se transpila. Serializa los args,
  POSTea al mismo origen (la cookie de sesión viaja sola), y mapea
  la respuesta a `Result<T>`. Un event handler que la `.await`ea se
  parte automáticamente en un wrapper sync + un worker async
  (`spawn_local`), así que el estado se actualiza y el componente
  re-renderiza cuando llega la respuesta.

**Reglas** (validadas por el checker): `@rpc` es un decorator
pelado (sin args/kwargs), la fn debe ser `async` y devolver
`Result<T>`, y no se combina con `@get`/`@post`/`@ws`/`@cron`/
`@background`/`@auth_provider`. Un tipo nominal que cruza el cable
(acá `User`) se importa también en el `.fitzv`
(`from api import ..., User`) para que el cliente tenga su struct.

Ejemplo runnable completo (server + client): `examples/view/rpc/`.

### Compatibilidad con classic Fitz

Un mismo proyecto puede mezclar `.fitz` (classic) y `.fitzv`
libremente:

- Los `.fitz` importan tipos y helpers de otros `.fitz` como
  siempre.
- Los `.fitzv` importan tipos y helpers **classic** con
  `from X import Y` (donde `X` es un `.fitz` sibling).
- Los `.fitz` **NO** importan components de `.fitzv` — es el
  runtime `fitz-liveviews` (`component("Name", "id")`) el que
  bridgea el mundo classic al mundo view.

Cuando un módulo classic Y un `.fitzv` con el mismo stem
existen (`Card.fitz` y `Card.fitzv`), el classic **gana**
(memoria — la migración desde classic es opt-in y additiva).

### Qué no está en el MVP

- **Cross-file `<Child />` composition** — **CERRADO en v0.25.0
  para el target WASM**: un `<Child />` importado de otro
  `.fitzv` (`from Card import Card`) se compone con la misma
  syntax XML-like que un child same-file (props, `@event`,
  slots). Ver la sub-sección "Cross-file `<Child />`
  composition" arriba. El path SSR sigue usando el runtime API
  `component("Name", "id")` del `fitz-liveviews`.
- **WASM interpolated props** — **caso simple CERRADO en
  Phase 11.7.a (v0.21.5)**: el target WASM ya acepta
  `<Child prop="{expr}" />` cuando el prop es un state field
  del parent (`{title}`) o aritmética sobre state numérico
  (`{n + 1}`), hacia un field **primitivo** del child. La
  propagación es reactiva por el modelo dirty-flag (el parent
  re-renderiza → recomputa el prop). Ejemplo runnable:
  `examples/view/reactive-props/`. Targets no-primitivos
  (nullable / nominal / list) + shapes ricos (method calls,
  concat de Str) siguen para un slice posterior de 11.7 / el
  target SSR.
- **Persistent child state** across parent re-render —
  **CERRADO en Phase 11.7.e (v0.21.6)** para sites estáticos: el
  parent cachea cada `<Child />` en un slot tipado y lo reusa, así
  que un child con state local lo **conserva** entre re-renders del
  parent. Ejemplo: `examples/view/reactive-props/` (el `Badge`
  tiene un contador `taps` propio que sobrevive el bump del
  parent).
- **`{#if}` / `{#for}` en el target WASM** — **CERRADO en Phase
  11.7.b (v0.21.6)**: `{#if}` (comparaciones + `&&`/`||`/`!` +
  `{#else}`) y `{#for}` sobre `List<primitive>` ya compilan a WASM.
  Ejemplo: `examples/view/control-flow/`. `{#for}` sobre
  `List<nominal>` (ej `List<Card>`) cerró en R3 (v0.21.8) — ver más
  abajo.
- **`<Child />` composition dentro de `{#for}`** (keyed dynamic
  children) — **CERRADO en Phase 11.7.b R2b (v0.21.7)**: un
  `<Child key="{x}" ... />` dentro de un `{#for x in items}` sobre
  `List<primitive>` ya compila a WASM. El atributo `key="{expr}"`
  (típicamente la loop var) le da a cada child una identidad
  estable; el emitter mantiene una keyed instance cache
  (`HashMap<String, Rc<Child>>`) — la instancia (y su state local)
  se reusa entre re-renders, y un sweep de reconciliation evicta
  las keys que desaparecen. Ejemplo runnable:
  `examples/view/keyed-composition/`.
- **Tipos nominales en el target WASM** — **CERRADO en Phase 11.7
  R3 (v0.21.8)**: un `type` classic importado ya es ciudadano de
  primera clase en WASM — `List<Card>` como state, `{#for c in
  cards}`, field access `{c.title}`, construcción `Card { ... }` +
  `<state_list>.push(...)` (mutación live de la lista → la
  reconciliation corre de verdad), y keyed `<Child key="{c.id}"
  title="{c.title}" />` con props primitivos desde campos del
  nominal. El emitter carga el `type` del `.fitz` sibling y sintetiza
  el struct Rust inline. Ejemplo runnable:
  `examples/view/nominal-list/`.
- **El kanban completo como WASM SPA** — **CERRADO en Phase 11.7
  R3.5 (v0.22.0)**. Todo lo que faltaba para portar el Board del
  kanban a una SPA WebAssembly standalone cerró: closures inline +
  `.map`/`.filter`/`.len()` + `{#for}` sobre el resultado de un call
  (`{#for c in cards_in(cards, "todo")}`) — R3.5a.1
  (`examples/view/list-transform/`); imported classic helper fns
  transpiladas al crate WASM (`cards_in`, `move_one`, `make_card`, ...)
  — R3.5a.2 (`examples/view/mini-board/`); payload de click
  (`data-flv-click` + `data-flv-value-*` + `payload["k"]`/`.has(...)`)
  — R3.5b.1 (`examples/view/click-payload/`); payload de form
  (`data-flv-submit` + `data-flv-clear`) — R3.5b.2
  (`examples/view/form-input/`); y la interpolación de strings
  (`"{next_id}"`). El resultado end-to-end vive en
  `examples/view/kanban/` (~57 KB raw / ~21.5 KB gzipped) — los mismos
  convenios `data-flv-*` sirven a los targets SSR y WASM.
- **Event bubbling child→parent con payload** — **CERRADO en
  Phase 11.7.c (v0.22.0) + payload bubbling (v0.23.0)**:
  `<Child @event="handler" />` dispara un handler del parent cuando
  el event del child se emite, **llevando un payload** (`Map<Str,
  Str>`) para que el parent sepa qué child lo disparó. El payload es
  el mismo mecanismo `data-flv-value-*` de los click/form handlers:
  un event que burbujea simplemente reenvía hacia arriba el payload
  que recibió. El child elige qué exponer con sus atributos
  `data-flv-value-*`; el handler del parent lo lee con
  `payload["k"]` / `payload.has("k")` (un handler que lo ignora
  también funciona). Ejemplo: `examples/view/event-bubbling/`
  (tres `<Item @choose="on_pick" />` que burbujean su `label`).
- **`<slot />` con fallback** — **CERRADO en Phase 11.7.d
  (v0.22.0)**: un child expone un hueco `<slot>fallback</slot>` que
  el parent llena con `<Child>content</Child>`, o se muestra el
  fallback del child. El contenido se renderiza en scope del PARENT
  (state + event handlers del parent), reactivo. Ejemplo:
  `examples/view/slots/`. Sin `<Child />` anidado en slot content.
- **Named slots (`<slot name="X" />`)** — **CERRADO en v0.24.0**:
  un child declara varios huecos (`<slot name="title" />`, un `<slot />`
  default, `<slot name="actions" />`) y el parent llena cada uno tagueando
  un elemento top-level de `<Child>...</Child>` con `slot="<name>"` (la
  convención nativa de Web Components); el contenido sin `slot=` va al
  slot default, y un slot que el parent no llena muestra su propio
  fallback. Ejemplo: `examples/view/named-slots/` (un `Card` con
  title/body/actions). WASM-only (el SSR rechaza todos los slots).
- **Imports de componentes cross-directory / dependencia** —
  **CERRADO en v0.29.6 (CW.8)** para el target WASM: un `.fitzv`
  puede importar un componente de una dep de `fitz.toml`
  (`from fitz_liveviews.ui.Badge import badge as Badge`), no solo de
  un sibling. Ver la sub-sección "Composición de components" arriba.
  Destraba consumir la companion UI de `fitz-liveviews` como librería
  desde una app WASM externa.
- **Eventos de formulario `@input` / `@change`** — **CERRADO en
  v0.29.7 (CW.9)** para el target WASM: leen el valor del control
  (`<input>`/`<select>`/`<textarea>`) al `payload["value"]` y llaman
  al handler. Ver la sub-sección "Eventos de formulario" arriba.
  Ejemplo: `examples/view/live-input/`.
- **Signature help / rename / references** dentro de `.fitzv`
  — no implementados por el LSP MVP.

### Cross-links

- **Cap 22 — Soporte para editores** — flow LSP general +
  extensión VSCode.
- **Cap 29 — WebSockets tipados** — el runtime al que apunta
  el emitted classic Fitz de un `.fitzv` con target SSR.
- **[docs/fase-11-plan.md](../fase-11-plan.md)** — plan
  técnico exhaustivo con las 11.1 → 11.9 sub-fases +
  §9.a–§9.bb.
- **[fitz-liveviews](https://github.com/Thegreekman76/fitz-liveviews)**
  — repo separado del framework SSR.

---

## 37. Plantillas y boilerplates

Si llegaste hasta acá leyendo, ya viste cada feature de Fitz por
separado: HTTP nativo, auth, WebSockets, jobs, interop Python,
package manager, compilación a binario. Lo que sigue es **verlas
funcionando juntas** en proyectos reales.

El repo trae **6 boilerplates Dockerizados** en `boilerplates/`,
cada uno con README exhaustivo (paso a paso, troubleshooting,
plan de producción):

| Boilerplate                                                              | Qué demuestra                                                                                            | Setup    |
|--------------------------------------------------------------------------|----------------------------------------------------------------------------------------------------------|----------|
| [`cli-tool`](../boilerplates/cli-tool/)                                   | CLI puro — sales report con `.reduce/.filter/.map`. Binario nativo distroless ~30 MB.                    | `docker build .` |
| [`api-simple`](../boilerplates/api-simple/)                               | REST API tipada con OpenAPI 3.1 + UI Scalar autogenerados. Distroless ~40 MB.                            | `docker build .` |
| [`api-middleware-cors`](../boilerplates/api-middleware-cors/)             | Auth nativa JWT + Argon2id + middleware encadenado + CORS cross-origin + frontend vanilla.               | compose 2 svcs   |
| [`api-websocket`](../boilerplates/api-websocket/)                         | WebSockets tipados (`WsConn<T>`) con broadcast + heartbeat + auth + frontend chat vanilla.               | compose 2 svcs   |
| [`api-postgres-python`](../boilerplates/api-postgres-python/)             | CRUD multi-archivo con SQLAlchemy + Postgres (módulos `types/` + `data/`, ejercitable con curl).         | compose 2 svcs   |
| [`api-fullstack-postgres`](../boilerplates/api-fullstack-postgres/)       | **Showcase fullstack** — API + frontend rico (tabla, edit inline, filtros, badges) + Postgres en compose. | compose 3 svcs   |

### Quickstart genérico

```bash
cd boilerplates/<nombre>
cp .env.example .env       # si existe
docker compose up --build  # o `docker build .` si no hay compose
```

Cada README dice qué URL abrir y cómo probar con curl.

### Cómo elegir

- **Aprendiendo Fitz** → arrancá por `cli-tool`: binario standalone,
  sin HTTP ni DB, muestra types + listas + funcional puro en <100 LoC.
- **REST API mínima** → `api-simple`: GET/POST tipados + OpenAPI auto.
- **Auth + frontend** → `api-middleware-cors`: stack stateless más
  rico, con CORS preflight automático y JWT/Argon2 nativos.
- **Real-time** → `api-websocket`: chat broadcast con heartbeat
  built-in y AsyncAPI 3.0 auto-generado.
- **Necesitás DB persistente** → `api-postgres-python` para el patrón
  API + DB, o `api-fullstack-postgres` para el stack web completo.

Las plantillas son la forma más rápida de ver Fitz "en escala real",
y también el mejor punto de partida si querés extender en lugar de
escribir desde cero.

> **Nota sobre `--bundle-python`**: los boilerplates 5 y 6
> (`api-postgres-python`, `api-fullstack-postgres`) usan
> `pip install sqlalchemy psycopg2` adentro del runtime — la imagen
> Docker base sigue siendo `python:3.X-slim` porque el flag actual
> `fitz build --bundle-python` empaqueta CPython base + stdlib
> pero NO paquetes pip. Cuando el sub-paso futuro `--bundle-pip`
> aterrice, esos Dockerfiles podrán pasar a `FROM scratch` o
> `FROM gcr.io/distroless/cc-debian12` con un binario único de
> ~80-100 MB (CPython + paquetes pip + el real binary). Hasta
> entonces, el modelo "match builder/runtime Python" sigue siendo
> el correcto. Para programas Fitz que **solo usan Python stdlib**
> (sin pip packages), `--bundle-python` hoy ya produce binarios
> standalone listos para `FROM scratch` — ver
> `examples/python-interop-8.b.fitz` y [cap 21.11](#2111-fitz-build---bundle-python--binario-standalone).

---

## 38. Qué sigue

Si llegaste hasta acá: gracias. Esta es una versión temprana de la
guía y vos sos parte muy temprana del proyecto.

### Lo que ya sabés

Con los capítulos 1 a 30 podés:

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
  y `await` corutinas Python via bridge tokio ↔ asyncio.
- **Tooling de editor**: extensión VSCode con highlighting +
  diagnostics + hover + go-to-definition + autocomplete contextual
  via LSP, con distribución multi-platform. Errores subrayados al
  tipear, mouse sobre una expresión muestra su tipo, F12 te lleva
  a su declaración, tras `.` aparecen métodos del tipo. `.vsix`
  per-plataforma con el binario bundleado adentro vía
  `npm run build:vsix`. Sin ir a la terminal. Ver
  [cap 22](#22-soporte-para-editores) para cómo instalar.
- **Formato canónico** con `fitz fmt`: cero config, estilo gofmt,
  preserva comments y blank lines del usuario. Modo `--check` para
  CI / pre-commit hooks. Ver
  [cap 23](#23-fitz-fmt--formateador-automático).
- **Tests built-in** con `@test` + `fitz test`: decorator `@test`
  sobre fns sin args, 4 assertion builtins (`assert`, `assert_eq`,
  `assert_ne`, `assert_throws`), runner con output estilo cargo
  (ok/FAILED + summary + exit code), filtrado por substring, async
  tests, discovery automático en manifest mode (`tests/*.fitz` +
  `[lib]` integration). Cero librerías. Ver
  [cap 24](#24-fitz-test--testing-built-in).
- **Hot reload** con `fitz dev`: file watcher sobre el proyecto +
  kill/respawn del child al detectar cambio en `.fitz` o
  `fitz.toml`. Debounce 100ms, exclusión de
  `target/`/`.git/`/`node_modules/`, banner ANSI entre runs,
  Ctrl+C atrapa sin dejar zombies. Ver
  [cap 25](#25-fitz-dev--hot-reload).
- **REPL interactivo** con `fitz repl`: prompt `fitz> ` con env
  compartido entre líneas, multi-line automático (`... `),
  pretty-print Python-style del último valor, 5 comandos especiales
  (`:help`/`:env`/`:type`/`:reset`/`:load`/`:quit`), history
  persistente en `~/.fitz/history` con arrow up/down + Ctrl+R,
  async transparente (`sleep(100).await` funciona). Ver
  [cap 26](#26-fitz-repl--repl-interactivo).
- **Linter** con `fitz lint`: 4 lints — `unused_variable`,
  `unused_import`, `useless_match`, `string_concat`. Default
  warning + exit 0; `--deny <lint>` promueve a error + exit 1
  para CI. Supresión con `// @allow(<lint>)` en la línea anterior.
  Output estilo cargo-clippy con colores ANSI auto. Ver
  [cap 27](#27-fitz-lint--linter-de-patrones).
- **Auth nativa**: tres decoradores — `@auth_provider` (singleton
  del programa que recibe headers y devuelve `Result<User>`),
  `@authenticated` (handler protegido por bearer JWT; 401
  automático), `@admin` (shorthand de auth + `user.role ==
  "admin"`; 403 automático) — más dos módulos built-in `jwt`
  (encode/decode HS256/384/512) y `hash` (Argon2id password
  hashing). El checker valida en compile-time que cada handler
  protegido tenga el provider registrado y reciba el `User`
  correcto. El schema OpenAPI 3.1 auto-agrega
  `securitySchemes.bearerAuth` + `security` por handler + 401/
  403 en responses. Paridad bit-a-bit `fitz run` ↔ `fitz build`.
  Cero `cargo add`/`pip install`. Ver [cap 28](#28-auth-nativa).
- **WebSockets tipados**: `@ws("/path")` sobre `async fn` +
  `WsConn<T>` con `recv`/`send`/`broadcast`/`close`. **Marshaling
  JSON automático** de cada frame text al `type` declarado,
  **AsyncAPI 3.0 auto-generado** en `/asyncapi.json`, **heartbeat
  built-in** con `@server(ws_heartbeat_secs=N)`, **auth integrada**
  en el handshake (`@authenticated`/`@admin` apilados ANTES del
  upgrade), **codegen con paridad** bit-a-bit `fitz run` ↔
  `fitz build`. Ningún otro lenguaje hoy combina WS tipados con
  AsyncAPI auto-generado del código fuente. Ver
  [cap 29](#29-websockets-tipados).
- **Jobs sin Celery**: tres piezas — **`@cron("expr")`** para
  tareas periódicas (5/6/7 fields), **`@background`** como marcador
  opt-in para autorizar `spawn(...)`, y **`spawn(fn_call)`**
  fire-and-forget que devuelve `Future<T>` tipado. Sin Celery, sin
  Redis, sin systemd timers — todo en el mismo binario. El checker
  valida en compile-time que `spawn(...)` apunte a una fn
  `@background` y refina el ret type. Cron-only mode (sin
  `@server`) queda vivo bloqueante con `signal::ctrl_c` (modo
  systemd-friendly). Paridad bit-a-bit `fitz run` ↔ `fitz build`
  con `cron = "0.12"` linkeado en el binario. Ver
  [cap 30](#30-jobs-sin-celery).

Es decir: todo lo que el intérprete de Fitz hoy ejecuta end-to-end,
con un chequeo estático que atrapa errores antes de que se
ejecuten, un compilador que produce binarios standalone, y un
puente al ecosistema Python para usar SQLAlchemy/numpy/asyncpg
sin abandonar Fitz.

### Lo que viene

La promesa "Fitz usable para proyectos reales" ya está cumplida
con todo el stack del lenguaje + ecosistema:

- Type checker estático, codegen binario nativo, async nativo, DX
  HTTP con OpenAPI 3.1 + UI Scalar, middleware + CORS, Send +
  paralelismo HTTP real.
- **Interop Python entera**: embedding, marshaling compuesto
  bidireccional, excepciones → `Result<T>`, coerción runtime,
  `fitz py-types` SQLAlchemy, bridge tokio ↔ asyncio, codegen
  interop en `fitz build`.
- **Bundling Python standalone**: `fitz build --bundle-python`
  produce binario con CPython 3.14.5 embebido (~22-35 MB según
  OS); `--bundle-pip-requirements` agrega los paquetes pip del
  `requirements.txt`. Launcher con `tar`+`flate2` inline habilita
  runtime `gcr.io/distroless/cc-debian12` (smoke real Docker
  validado, imagen ~136 MB con sqlalchemy + Postgres).
- **Ecosistema completo**: LSP MVP (ver cap 22), package manager
  (cap 16b), DX (`fmt`/`test`/`dev`/`repl`/`lint`, caps 23-27),
  stack web first-class (auth/WS/jobs, caps 28-30), `env`
  builtin (cap 32).
- **Stack DB nativo + ORM declarativo** (cap 31). Driver Postgres
  en Fitz puro, ORM sobre `type` con decoradores
  `@table`/`@primary`/`@column`/`@belongs_to`/`@has_many`/
  `@has_one`. SQL constante en codegen-time, paridad bit-a-bit
  `fitz run` ↔ `fitz build`.

**Lo que ya bajó de "especulativo" a REALIDAD** (post-v0.21.0):

- **Frontend nativo con `.fitzv` (SFC + SSR + WASM)** — Fase 11
  CERRADA. Single-file components estilo Vue/Svelte, dos
  backends de compilación (SSR con `fitz-liveviews`, WASM
  client-side opt-in), template DSL con directivas + sharing
  de `type` entre back y front (los tipos custom viven en
  `.fitz` classic, los components en `.fitzv` los importan).
  LSP con las 4 capabilities core (diagnostics + completions +
  hover + go-to-def) desde v0.21.3. Ver cap 36.
- **Deployment ciudadano primera clase** — Fase 12 CERRADA.
  `fitz deploy` (12.6), Dockerfile + compose autogenerado
  (12.4), observability nativa con `@trace`/`@metric` (12.7),
  feature flags built-in (12.8), secrets management (12.2).
- **Migraciones automáticas para el ORM** — Fase 10.6 CERRADA.
  `fitz db diff/migrate/status/new/rollback/check/history/
  squash/inspect/stamp` con tracking en `_fitz_migrations`.

**Lo que sigue** (post-Fase 11 nortes):

- **Phase 11.7 — Client-side dynamic capabilities** — **CERRADA
  entera** (v0.21.5 → v0.23.0): reactive props, `{#if}`/`{#for}`,
  nominales en WASM, imported fns, click/form payload, event
  bubbling **con payload** (v0.23.0), `<slot />` con fallback, y el
  kanban como SPA WASM. La superficie de composición WASM está
  completa. Refinamientos abiertos (según demanda): named slots
  (`<slot name="X" />`), `<Child />` cross-file, drag-drop
  primitives.
- **Companion UI library de components reusables** (grid,
  forms, table, modal, ...) sobre `fitz-liveviews` — anotada
  como candidato en `docs/deudas-post-5b.md`. Driver concreto:
  showcase real de una app admin/dashboard.

Ver [docs/roadmap.md](roadmap.md) y
[docs/deudas-post-5b.md](deudas-post-5b.md) para el detalle.

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
