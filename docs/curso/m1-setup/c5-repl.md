# C5 — REPL

**Pre-requisitos**: [C4 — CLI esencial](c4-cli-esencial.md)
terminado. Conocés `run`, `check`, `fmt`, `lint`.

**Objetivo**: usar `fitz repl` como **laboratorio interactivo**.
Probar sintaxis sin crear archivos, inspeccionar tipos con
`:type`, cargar fns de tu proyecto con `:load`.

**Por qué importa**: el REPL es la herramienta exploratoria. Es
donde aprendés sintaxis nueva sin commitear nada, donde validás
"¿qué devuelve esta expresión?" sin escribir un archivo entero,
y donde debuggeás fns de tu proyecto cargándolas vivas. Análogo
a `python`/`node`/`irb`/`iex` — si venís de cualquiera de esos,
ya sabés el patrón.

---

## Paso 1 — Arrancar el REPL

Desde cualquier carpeta (no necesita un proyecto):

```bash
fitz repl
```

```
Fitz REPL
Tipos: `:help` para comandos disponibles. Ctrl+D para salir.

```

Estás adentro. El prompt está vacío esperando input.

> **Para salir**: `Ctrl+D` (Linux/macOS), `Ctrl+Z` + Enter
> (Windows), o el comando especial `:quit` (alias `:q`).

---

## Paso 2 — Expresiones y el "auto-print"

Empezá con una expresión simple:

```
1 + 1
```

```
= 2
```

El REPL **imprime el valor de la última expresión** automático,
prefijado con `=`. No hace falta `print(...)`.

Probá más:

```
"hola".upper()
```

```
= HOLA
```

```
[1, 2, 3].map(fn(x) => x * 10)
```

```
= [10, 20, 30]
```

Tres tipos distintos (Int, Str, List), el REPL muestra cada uno
con su formato natural — el mismo que verías en un `print(...)`.

---

## Paso 3 — Bindings persistentes

Cada `let` o `fn` **queda en el scope** hasta que cierres el
REPL (o uses `:reset`):

```
let nombre = "Patagonia"
```

(sin output — el `let` no es una expresión)

```
nombre.upper()
```

```
= PATAGONIA
```

Definí una fn:

```
fn double(n: Int) -> Int => n * 2
```

```
double(21)
```

```
= 42
```

Las fns persisten igual que las vars. Podés usarlas en
expresiones posteriores, redefinirlas (la nueva pisa la
vieja), o combinarlas.

---

## Paso 4 — Multi-line automático

Si abrís un `{` o `(` y no lo cerrás en la misma línea, el
prompt cambia a `... ` y el REPL espera el cierre:

```
fn greet(name: Str) -> Str {
...     return "hola {name}"
... }
```

Cuando cerrás el `}`, el REPL procesa todo el bloque:

```
greet("Ada")
```

```
= hola Ada
```

Funciona con cualquier cosa que abra balanced brackets:
condicionales con bloques, struct literals, listas multi-línea.

---

## Paso 5 — `:type <expr>` (inspeccionar tipo)

Quizás el comando más útil del REPL: mostrar el tipo de una
expresión **sin evaluarla**.

```
:type 1 + 1
```

```
:: Int
```

```
:type [1, 2, 3].map(fn(x) => x * 10)
```

```
:: List<Int>
```

```
:type nombre.upper()
```

```
:: Str
```

Útil para entender cómo el checker resuelve algo complejo, o
para confirmar el tipo antes de bindearlo en un `let` anotado.

> **Limitación del MVP**: `:type` corre el checker sobre un
> programa sintético; no es 100% scope-aware con todas las vars
> del REPL. Para la mayoría de los casos funciona; si te da
> `Any` cuando esperabas algo concreto, escribilo en un `.fitz`
> y corré `fitz check` ahí.

---

## Paso 6 — `:env` (ver bindings actuales)

Listar qué tenés definido en el scope:

```
:env
```

```
Definido en el scope:
  bytes = <builtin bytes>  // Function
  db = <module db>  // Module
  double = <function>  // Function
  nombre = "Patagonia"  // Str
  spawn = <builtin spawn>  // Function
  x = 21  // Int
```

Notás que aparecen también algunos **built-ins** (`spawn`,
`bytes`) y el **módulo `db`** (siempre disponible). Tus
bindings (`nombre`, `double`, `x`) se mezclan con ellos
ordenados alfabético.

---

## Paso 7 — `:load <archivo>` (cargar código del proyecto)

Esto es la diferencia entre el REPL como juguete y el REPL como
herramienta real. **`:load` evalúa un `.fitz` adentro del scope
actual** — vos podés cargar fns de tu proyecto y probarlas en
vivo.

Volvé a `mi-saludos/` y editá `src/main.fitz`:

```fitz
fn saludar(quien: Str) -> Str {
    return "Hola, {quien}!"
}

fn elevar(x: Int) -> Int => x * x

print(saludar("Patagonia"))
print(elevar(7))
```

Desde la raíz del proyecto, arrancá el REPL:

```bash
fitz repl
```

```
:load src/main.fitz
```

```
Hola, Patagonia!
49
✓ cargado src/main.fitz
```

Notá que el `:load` **ejecutó el archivo entero** (incluyendo
los `print`), y además dejó `saludar` y `elevar` definidas en el
scope. Ahora podés:

```
saludar("Buenos Aires")
```

```
= Hola, Buenos Aires!
```

```
elevar(13)
```

```
= 169
```

```
:type saludar
```

```
:: fn(Str) -> Str
```

Eso es **debugging interactivo en serio**: tenés tu lib viva en
el REPL, le das inputs raros, y ves el output sin recompilar
ni recorrer un test.

> **Path al archivo**: relativo al directorio donde arrancaste
> el REPL. Si tu proyecto está en `~/proyectos/mi-saludos` y
> arrancaste `fitz repl` desde ahí, `:load src/main.fitz`
> funciona. Si arrancaste desde `~`, te conviene
> `:load ~/proyectos/mi-saludos/src/main.fitz`.

---

## Paso 8 — `:reset` (empezar de cero)

Si tu scope se ensució con experimentos y querés volver al
estado inicial sin salir del REPL:

```
:reset
```

```
✓ scope reseteado
```

Todas las vars y fns del usuario desaparecen. Los built-ins
(`spawn`, `bytes`, módulo `db`) siguen disponibles porque son
parte del lenguaje.

Útil cuando estás explorando varias hipótesis y querés que no
se contaminen entre sí.

---

## Paso 9 — `:help` y los otros comandos

Lista completa de comandos especiales:

```
:help
```

```
Comandos del REPL:
  :help, :h       — esta ayuda
  :quit, :q       — salir (también Ctrl+D)
  :env            — listar variables y fns definidas en el scope
  :reset          — limpiar el scope (perdés todo)
  :type <expr>    — mostrar el tipo de una expresión
  :load <archivo> — evaluar un .fitz en el scope actual
```

📚 **Detalle exhaustivo**: [cap 26 — `fitz repl`](../../guide.md#26---fitz-repl)
de la guía.

---

## Paso 10 — History entre sesiones

El REPL guarda lo que tipeás en `~/.fitz/history` (Linux/macOS)
o `%USERPROFILE%\.fitz\history` (Windows). Cuando volvés a
arrancarlo:

- **Flecha ↑** trae la línea anterior (y ↓ la siguiente).
- **`Ctrl+R`** abre búsqueda incremental sobre el history.
- **`Ctrl+L`** limpia la pantalla (preservando el scope).
- **`Ctrl+C`** cancela la línea actual sin salir del REPL.

Los atajos vienen de `rustyline` (la lib que usa el REPL por
debajo). Si usaste `bash`/`zsh`/`psql`, son los mismos.

---

## Validación

- [ ] `fitz repl` arranca y mostrás el prompt sin errores.
- [ ] Definís una var (`let x = 42`) y al pedirle `x` te
      devuelve `= 42`.
- [ ] `:type "hola"` te dice `:: Str`.
- [ ] `:load` sobre un `.fitz` con una fn cualquiera te deja
      esa fn disponible en el scope.

---

## Troubleshooting

**`:load` me dice "no se pudo leer el archivo"**

- El path es relativo al directorio donde **arrancaste**
  `fitz repl`, no al cwd actual (no hay cwd actual en el REPL).
  Si tenés dudas, usá ruta absoluta.

**`:type` me devuelve `Any` y esperaba algo concreto**

- Limitación documentada del MVP. Workaround: escribilo en un
  `.fitz` y `fitz check` ahí.

**Salir con `Ctrl+D` no funciona en Windows**

- Usá `Ctrl+Z` y después Enter. O `:quit` / `:q`.

**El REPL no detecta una fn que cargué con `:load`**

- Verificá que el `:load` haya impreso `✓ cargado X.fitz`. Si
  no, hubo un error de parse/check y el archivo no se evaluó.

---

## Cerraste el módulo M1

**Felicidades**, completaste el módulo de Setup. Repasemos qué
sabés ahora:

- ✅ Instalar Fitz y la extensión VSCode (**C1**)
- ✅ Crear un proyecto con `fitz new` y la estructura estándar
  (**C2**)
- ✅ Editar Fitz en VSCode con hover, autocomplete y errores
  live del LSP (**C3**)
- ✅ Usar `run`, `check`, `fmt`, `lint` en el workflow diario
  (**C4**)
- ✅ Experimentar interactivo con `fitz repl` + `:type` +
  `:load` (**C5**)

**Entregable del módulo**: tenés Fitz funcionando en tu máquina,
con extensión VSCode activa y un proyecto del que podés escribir,
correr, formatear, debugear y experimentar.

## Qué viene en M2 — Tipos y funciones

A partir del próximo módulo dejamos el "setup" y empezamos a
escribir Fitz **en serio**. M2 cubre el sistema de tipos
gradual: primitivos, anotaciones opcionales, inferencia, fns
con/sin tipos, higher-order, `type` para tipos custom, listas,
mapas y rangos. Es la base del lenguaje — todo lo demás se
construye encima.

Cuando esté listo, el cap C1 del M2 va a aparecer en el [índice
del curso](../index.md).
