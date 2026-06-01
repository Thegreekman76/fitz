# C3 — Hola mundo + LSP visible

**Pre-requisitos**: [C2 — `fitz new`](c2-fitz-new.md) terminado.
Tenés `mi-saludos/` con su `fitz.toml` y `src/main.fitz`.

**Objetivo**: ver el **LSP** en acción adentro de VSCode. Hover
con tipos, autocomplete contextual, errores subrayados en vivo,
Ctrl+Click para navegar a definiciones.

**Por qué importa**: el editor con LSP es la diferencia entre
escribir Fitz "a mano" y escribirlo como escribís TypeScript o
Rust con su tooling moderno. El LSP es Fitz **explicándote tu
propio código mientras lo tipeás**.

---

## Antes de arrancar — qué es el LSP

LSP = **Language Server Protocol**, un estándar de Microsoft que
desacopla "el editor" de "qué sabe el lenguaje". El editor
(VSCode, Neovim, Helix...) habla LSP; el lenguaje provee un
**servidor** (`fitz-lsp`) que responde preguntas como:

- "¿qué tipo tiene esta variable bajo el cursor?" → **hover**
- "¿qué fns están en scope?" → **autocomplete**
- "¿este programa compila?" → **diagnostics**
- "¿dónde se definió este símbolo?" → **go-to-definition**

La extensión que instalaste en C1 (`fitz-lang.vsix`) trae el
`fitz-lsp` adentro. No tenés que arrancarlo a mano — VSCode lo
levanta solo al abrir el primer `.fitz`.

📚 **Detalle exhaustivo**: [cap 22 — Soporte para editores](../../guide.md#22---soporte-para-editores)
de la guía.

---

## Paso 1 — Abrir el proyecto en VSCode

Desde la terminal, parado en la raíz del proyecto:

```bash
cd mi-saludos
code .
```

(El `.` significa "esta carpeta"). VSCode abre con el árbol del
proyecto en la sidebar izquierda.

Abrí `src/main.fitz` clickeándolo.

> **Primer arranque del LSP**: hay un delay de 1-2 segundos la
> primera vez que abrís un `.fitz`. Es el `fitz-lsp` arrancando.
> Las próximas son instantáneas.

---

## Paso 2 — Hover con tipos

Reemplazá el contenido de `src/main.fitz` con:

```fitz
let nombre = "Patagonia"
let edad = 200
let activa = true

print("Saludos desde {nombre}")
```

Mientras tipeás, **pasá el mouse sobre cada variable** sin
clickear. Después de ~500ms aparece un tooltip:

```
nombre: Str
```

Probá los tres:

| Variable | Hover muestra |
|---|---|
| `nombre` | `nombre: Str` |
| `edad` | `edad: Int` |
| `activa` | `activa: Bool` |

**No declaraste los tipos** en ninguna parte — el LSP los
infiere desde los literales. Eso es **tipado gradual**: vos
escribís relajado, el compilador deduce.

> **Si no ves el hover**: el LSP puede no haber arrancado. Mirá
> el panel inferior de VSCode (`Ctrl+Shift+U`) → en el dropdown
> elegí "Fitz Language Server". Tiene que haber líneas de
> arranque. Si no hay nada, reload window:
> `Ctrl+Shift+P` → "Developer: Reload Window".

---

## Paso 3 — Autocomplete contextual

Agregá una línea al final del archivo:

```fitz
let mayuscula = nombre.
```

Cuando tipeás el punto (`.`), el LSP detecta que `nombre` es
`Str` y abre un dropdown con los métodos disponibles:

```
upper()    fn() -> Str
lower()    fn() -> Str
len()      fn() -> Int
```

Elegí `upper()` y completá:

```fitz
let mayuscula = nombre.upper()
print(mayuscula)
```

Hover sobre `mayuscula` ahora dice `mayuscula: Str`. El LSP
infirió el tipo del return de `.upper()`.

Probá lo mismo con otros tipos:

```fitz
let xs = [1, 2, 3]
let suma = xs.    // ← autocomplete: push, pop, map, filter, find, len
```

```fitz
let m = {"a": 1, "b": 2}
let claves = m.   // ← autocomplete: get, has, keys, values, len
```

Esto **escala a tipos custom** que definas vos. Cuando lleguemos
a `type` en M2, el autocomplete te muestra los fields del tipo
sin que tengas que recordarlos.

---

## Paso 4 — Errores subrayados en vivo

Acá viene lo bueno. **Editá una línea para que esté mal a
propósito**:

```fitz
let edad: Int = "200"   // ← anotación Int, valor Str
```

En menos de un segundo deberías ver:

- **Subrayado rojo ondulado** debajo de `"200"`.
- **Punto rojo** al lado del número de línea en la barra
  izquierda.
- **Panel "Problems"** en el bottom de VSCode con un contador
  (puede que tengas que abrirlo con `Ctrl+Shift+M`).

Hover sobre el subrayado:

```
`edad` declarado como `Int` recibió un valor `Str`
```

Eso es **diagnostics live** — el LSP corre el checker en cada
keystroke y publica los errores al editor. Sin guardar el
archivo, sin correr nada, sin compilar.

Arreglalo:

```fitz
let edad: Int = 200
```

El subrayado desaparece al instante.

---

## Paso 5 — Otro tipo de error: typo de variable

Provocá otro error, distinto:

```fitz
print("hola {nomber}")   // typo: "nomber" en vez de "nombre"
```

Hover sobre el subrayado:

```
variable desconocida `nomber`
```

El LSP no solo chequea tipos — también valida que las variables
referenciadas existan en el scope. Combinado con autocomplete,
los typos son raros: vos escribís `nom` + `Tab` y el editor
completa `nombre` correcto.

Arreglalo y seguí.

---

## Paso 6 — Go-to-definition (Ctrl+Click)

Agregá una fn al archivo:

```fitz
fn saludar(quien: Str) -> Str {
    return "Hola, {quien}!"
}

print(saludar("Patagonia"))
```

Ahora **`Ctrl+Click` sobre `saludar`** en la última línea
(`Cmd+Click` en macOS). El cursor salta a la definición de la
fn.

Funciona también con:
- Variables (`Ctrl+Click` sobre `nombre` salta al `let nombre =
  "Patagonia"`).
- Tipos custom (M2).
- Símbolos importados de otros módulos (M3).

> **Atajo equivalente**: posicioná el cursor sobre el símbolo y
> apretá `F12`. Para volver al cursor anterior, `Ctrl+-` (Linux/
> Windows) o `Ctrl+-` (macOS).

---

## Paso 7 — Panel "Problems" + navegación

Si tu proyecto tiene varios errores en distintos archivos, el
panel "Problems" (`Ctrl+Shift+M`) los lista todos:

- Provocá tres errores rápidos en `src/main.fitz`:
  ```fitz
  let a: Int = "x"
  let b: Bool = 1
  print(c_inexistente)
  ```
- Abrí el panel Problems. Deberías ver 3 entradas, cada una con
  archivo + línea + mensaje.
- **Clickeá una entrada** → el editor salta a esa posición.

Útil cuando refactorizás algo grande y querés ver toda la lista
de roturas en una pantalla.

---

## Paso 8 — Validar también con `fitz check`

Lo que ves en VSCode (LSP) y lo que te dice `fitz check`
(CLI) **son el mismo motor**. El LSP es eso adentro de un
servidor que VSCode consulta; `fitz check` lo ejecuta de una.

Probá: dejá el archivo con un error a propósito, guardá, y
desde la terminal del proyecto:

```bash
fitz check
```

```
✗ /ruta/a/mi-saludos/src/main.fitz — 1 error(es) de tipo:
  Error en línea 1:1 — `edad` declarado como `Int` recibió un valor `Str`
```

Exit code 1 cuando hay errores (útil en CI). Arreglá y volvé a
correr:

```bash
fitz check
```

```
✓ /ruta/a/mi-saludos/src/main.fitz — sin errores de tipo
```

`fitz check` es lo que vas a usar en C4 y en CI. El LSP es lo
que usás interactivo.

---

## Validación

- [ ] Hover sobre una variable muestra su tipo (ej. `nombre: Str`).
- [ ] Autocomplete tras `.` muestra los métodos del tipo
      (probaste con `Str`, `List` o `Map`).
- [ ] Cuando ponés un error a propósito, lo ves subrayado en
      rojo **sin guardar ni compilar**.
- [ ] `Ctrl+Click` sobre un símbolo salta a su definición.

---

## Troubleshooting

**No veo hover ni autocomplete**

- Esperá 2 segundos al primer arranque (LSP lazy).
- Output panel (`Ctrl+Shift+U`) → "Fitz Language Server". Si hay
  errores, copialos al reportar.
- Reload window: `Ctrl+Shift+P` → "Developer: Reload Window".

**El subrayado tarda mucho en aparecer**

- Normal: el LSP debouncea ~300ms tras la última tecla para no
  re-chequear en cada keystroke.

**Hover muestra `Any` en vez del tipo concreto**

- Pasa con expresiones complejas o callbacks sin anotación.
  En M2 vemos cuándo el inference da `Any` y cómo guiarlo con
  anotaciones.

**El LSP está vivo pero el highlighting no funciona**

- El highlighting es de la **extensión** (grammar TextMate), no
  del LSP. Si tu archivo no se ve coloreado:
  - Verificá que termina en `.fitz`.
  - Bottom-right de VSCode dice el lenguaje detectado. Debería
    decir "Fitz". Si dice "Plain Text", click ahí y elegí Fitz.

---

## Lo que viene en C4

Vimos el LSP — el día-a-día de escribir. En el próximo cap
arrancamos con los **4 comandos CLI esenciales** que vas a correr
constantemente: `run`, `check`, `fmt`, `lint`. Cada uno tiene su
rol, y juntos forman el workflow estándar de cualquier proyecto
Fitz.
