# C4 — CLI esencial (run / check / fmt / lint / dev)

**Pre-requisitos**: [C3 — Hola mundo + LSP](c3-hola-lsp.md)
terminado. Ya tenés `mi-saludos/` abierto en VSCode.

**Objetivo**: dominar los **comandos CLI del día-a-día** con
**todos sus flags, exit codes y edge cases**. Al final del cap
no tenés que volver a `fitz <cmd> --help` para nada.

**Por qué importa**: el LSP de C3 te cubre mientras escribís.
Pero antes de commitear, abrir un PR, o correr en CI, lo
canónico es pasar por la terminal. Cada comando de este cap es
una pieza del workflow estándar.

---

## El CLI completo de Fitz

```bash
fitz --help
```

```
Usage: fitz <COMMAND>

Commands:
  run       Ejecutar un .fitz (interpretado)
  build     Compilar a binario nativo                ← C6
  check     Type-check sin ejecutar
  openapi   Emitir schema OpenAPI 3.1                ← M4
  py-types  Generar `type` Fitz desde modelos SQLAlchemy
  py-stubs  Generar `type` Fitz desde stubs .pyi
  new       Crear proyecto nuevo                     ← C2
  init      Inicializar proyecto en cwd              ← C2
  add       Agregar dependencia                      ← M3
  remove    Quitar dependencia                       ← M3
  update    Actualizar git deps                      ← M3
  fmt       Formatear código
  test      Correr `@test` fns                       ← M2
  dev       Hot reload
  repl      REPL interactivo                         ← C5
  lint      Linter
  db        Migraciones del ORM                      ← M6
  help      Print this message or help of subcommand

Options:
  -h, --help     Print help
  -V, --version  Print version
```

Para help de un subcomando específico:

```bash
fitz <subcomando> --help     # ej: fitz run --help
```

Para versión:

```bash
fitz --version
# fitz 0.11.1
```

> El comando `fitz help` (sin guion) es equivalente a `fitz
> --help`. Convención clap (la lib de parsing). Ambas funcionan.

---

## Tabla maestra de comandos cubiertos en este cap

| Comando | Para qué | Manifest mode | Exit code 0 | Exit code 1 |
|---|---|---|---|---|
| `fitz run` | Ejecutar (interpretado) | ✓ sin args | Programa OK | Error de tipo (strict) o panic |
| `fitz check` | Type-check sin ejecutar | ✓ sin args | Sin errores | Hay errores de tipo |
| `fitz fmt` | Formatear | ✓ sin args | Aplicado/ya OK | (n/a) |
| `fitz fmt --check` | Verificar formato | ✓ sin args | Está bien | Hay archivos sin formatear |
| `fitz lint` | Detectar patrones | ✓ sin args | Sin findings (o sin `--deny`) | `--deny <lint>` matcheó |
| `fitz dev` | Hot reload | ✓ sin args | (n/a — corre hasta Ctrl+C) | (n/a) |

`fitz run` es **destructivo** (ejecuta tu código). Los otros son
**read-only**: leen tus archivos y reportan.

---

## Paso 1 — `fitz run`

### Sintaxis completa

```
fitz run [OPTIONS] [FILE] [ARGS]...
```

| Posición | Qué es |
|---|---|
| `FILE` | Archivo `.fitz` a ejecutar. **Si se omite**, busca `fitz.toml` en cwd o ancestros (Cargo-style) y ejecuta su `[bin].main`. |
| `ARGS...` | Args adicionales que se pasan al programa Fitz cuando tiene `@command` (M2.C6+). El `--` los separa de los args de fitz. |

### Flag: `--no-typecheck`

```
fitz run --no-typecheck <FILE>
```

Saltea el chequeo estático. **Sin esta flag, `fitz run` aborta
si hay errores de tipo** (modo strict por default desde Fase
5.4). Con `--no-typecheck` corre igual.

Cuándo usarlo:
- Prototipando algo y querés ver el output runtime aunque haya
  errores de tipo.
- Migrando código viejo que todavía no anda en strict.

**No lo uses en CI** — el sentido del checker estático es que
los errores son tu primera línea de defensa.

### Flag: `--bin <nombre>`

```
fitz run --bin server
```

Cuando tu `fitz.toml` declara **más de un `[[bin]]`** (por ejemplo
un proyecto fullstack con un backend `server` y un frontend
`web`), `fitz run` sin `--bin` es ambiguo y te pide que elijas.
`--bin <nombre>` selecciona cuál correr. Existe igual en
`fitz build --bin` y `fitz dev --bin` (Paso 5).

### Modos de ejecución

#### Manifest mode (sin args)

Desde la raíz de un proyecto creado con `fitz new`:

```bash
fitz run
```

```
Saludos desde Patagonia
```

Sube por ancestros buscando `fitz.toml`. Funciona desde
cualquier subcarpeta del proyecto:

```bash
cd src
fitz run     # ← también funciona acá
```

#### Single-file mode

```bash
fitz run hola.fitz
```

No requiere `fitz.toml`. Útil para archivos sueltos o
experimentos.

#### Con args al programa (`-- <args>...`)

Cuando tu programa tiene fns con `@command` (CLI builder de
Fase 13, M2.C6 lo cubre en detalle), pasás args al programa
después de `--`:

```bash
fitz run src/main.fitz -- greet Ada --loud
```

El `--` separa: lo que va antes son args de `fitz`, lo que va
después son los args de **tu programa**.

```fitz
@command("greet")
fn greet(name: Str, loud: Bool = false) {
    if (loud) {
        print("HOLA, {name}".upper())
    } else {
        print("hola, {name}")
    }
}
```

```bash
fitz run src/main.fitz -- greet Ada
# hola, Ada

fitz run src/main.fitz -- greet Ada --loud
# HOLA, ADA

fitz run src/main.fitz -- --help
# (muestra el --help auto-generado del programa)
```

### Exit codes de `fitz run`

| Exit code | Causa |
|---|---|
| 0 | Programa terminó OK |
| 1 | Error de tipo (modo strict, sin `--no-typecheck`) |
| 1 | Error de parseo o lex |
| 1 | Panic en runtime (división por cero, index fuera de rango, etc.) |
| 1 | Programa con `@command` retornó algo distinto de `Int` |
| Code custom | Programa con `@command` retornó un `Int` (es el exit code) |

---

## Paso 2 — `fitz check`

### Sintaxis

```
fitz check [FILE]
```

Como `fitz run` pero **sin ejecutar el programa**. Solo verifica
tipos y sintaxis.

```bash
fitz check
```

```
✓ /ruta/a/mi-saludos/src/main.fitz — sin errores de tipo
```

Exit 0.

### Output con errores

Editá `src/main.fitz` con un error a propósito:

```fitz
let edad: Int = "doscientos"
let altura: Float = "alto"
print(edad)
```

```bash
fitz check
```

```
✗ /ruta/a/mi-saludos/src/main.fitz — 2 error(es) de tipo:
  Error en línea 1:1 — `edad` declarado como `Int` recibió un valor `Str`
  Error en línea 2:1 — `altura` declarado como `Float` recibió un valor `Str`
```

Exit 1.

> **Diagnostics agregados**: `fitz check` reporta **todos los
> errores en un pase** (gracias al error recovery del parser
> de Fase 9.0/F15), no se para en el primero. CI puede
> reportar la lista completa sin volver a correr.

### Modos

| Modo | Comando |
|---|---|
| Single-file | `fitz check archivo.fitz` |
| Manifest (chequea `[bin].main`) | `fitz check` (desde la raíz) |

Para chequear **todos los archivos del proyecto** (no solo el
`[bin].main`), por ahora hay que correr `fitz check` archivo por
archivo o confiar en `fitz lint` (que sí pasa por todos).
Mejora pendiente del lenguaje.

### Exit codes

| Exit code | Causa |
|---|---|
| 0 | Sin errores |
| 1 | Hay errores de tipo, parseo o lex |

---

## Paso 3 — `fitz fmt`

### Sintaxis

```
fitz fmt [OPTIONS] [FILES]...
```

| Posición / flag | Qué hace |
|---|---|
| `FILES...` | Archivos a formatear. **Si se omiten**, formatea todos los `.fitz` del proyecto (requiere `fitz.toml`). |
| `--check` | Modo CI: no escribe, exit 1 si hay diffs. |

### Estilo canónico

Cero config. Reglas fijas:

- **4 espacios** de indent (no tabs).
- **Comillas dobles** (`"x"`, no `'x'`).
- **Trailing comma** solo en multi-línea.
- **Blank line** entre fns y types top-level.
- **Comments y blank lines del usuario preservados** (no se
  reorganizan).

### Demo bit-a-bit

Editá `src/main.fitz` con estilo "feo":

```fitz
let x=1
fn double(n:Int)->Int=>n*2
print(double(x))
```

Antes de aplicar — modo CI (`--check`):

```bash
fitz fmt --check
```

```
✗ /ruta/.../src/main.fitz no está en formato canónico

uso `fitz fmt` (sin `--check`) para aplicar el formato.
```

Exit 1.

Aplicar:

```bash
fitz fmt
```

```
✓ formateado /ruta/.../src/main.fitz
```

Resultado:

```fitz
let x = 1

fn double(n: Int) -> Int {
    return n * 2
}

print(double(x))
```

Cambios aplicados:
- Espacios alrededor de `=`, `:`, `->`, `*`.
- `fn double(n: Int) -> Int => n * 2` se expandió a bloque con
  `return` explícito (preferido para fns multi-línea claras).
- Blank line entre la fn y el `print` del scope superior.

### Modo "archivos explícitos"

```bash
fitz fmt src/main.fitz src/users.fitz   # solo esos dos
fitz fmt src/main.fitz --check          # check sobre uno solo
```

Útil en pre-commit hooks que solo formatean los archivos
staged.

### Exit codes

| Exit code | Causa |
|---|---|
| 0 | Sin `--check`: aplicado o ya OK. Con `--check`: todos en formato. |
| 1 | Con `--check`: hay archivos sin formatear. |
| 1 | Error de lectura/parseo de algún archivo. |

📚 **Detalle de las convenciones**: [`docs/fmt-style.md`](../../fmt-style.md).

---

## Paso 4 — `fitz lint`

### Sintaxis

```
fitz lint [OPTIONS] [FILES]...
```

| Posición / flag | Qué hace |
|---|---|
| `FILES...` | Archivos a lintear. **Si se omiten**, lintea todos los `.fitz` del proyecto. |
| `--deny <lint>` | Trata ese lint como error (exit 1). Repetible. |

### Los 4 lints implementados

#### 1. `unused_variable`

Variable declarada que nunca se usa.

```fitz
let nombre = "Ada"
let edad_olvidada = 42       // ← warning
print(nombre)
```

```
warning: variable `edad_olvidada` declarada pero no usada [unused_variable]
  --> /ruta/.../src/main.fitz:2:1
  = nota: si es intencional, prefijá con `_` (ej. `_edad_olvidada`) o suprimí con `// @allow(unused_variable)` en la línea anterior.
```

**Para silenciar**: prefijá con `_` (convención que el lint
ignora) o anotá `// @allow(unused_variable)` en la línea
anterior.

#### 2. `unused_import`

Import sin usos.

```fitz
import utils                  // ← warning si utils no aparece más
import other
other.foo()
```

```
warning: import `utils` declarado pero no usado [unused_import]
  --> /ruta/.../src/main.fitz:1:1
```

#### 3. `useless_match`

`match` con un solo arm catch-all — equivale a el body directo.

```fitz
let x = 5
let r = match x {
    _ => "default"            // ← warning: useless_match
}
```

```
warning: `match` con un solo arm catch-all es equivalente al body directo [useless_match]
  --> /ruta/.../src/main.fitz:2:9
  = nota: reemplazá `match x { _ => "default" }` por `"default"` directo.
```

#### 4. `string_concat`

Concatenación de strings literales con `+` — preferí
interpolación.

```fitz
print("hola " + "mundo")      // ← warning
```

```
warning: concatenación de strings — usá interpolación [string_concat]
  --> /ruta/.../src/main.fitz:1:7
  = nota: reemplazá `"a" + "b"` con `"ab"` (o usá interpolación `"{a}{b}"` si los lados son variables).
```

> **Nota**: este lint **solo dispara cuando ambos lados son
> string literales**. `nombre + "!"` (donde `nombre` es una
> var) NO dispara.

### Suprimir un lint puntual

`// @allow(<nombre-del-lint>)` en la línea inmediatamente
anterior:

```fitz
// @allow(unused_variable)
let placeholder = 42
```

Útil cuando estás scaffoldeando algo o el "warning" es
intencional (ej. variable que se rellena en una iteración
futura).

### `--deny` para CI

Default: los lints son warnings (exit 0). Para que CI falle,
marcalos como errores:

```bash
fitz lint --deny unused_variable --deny string_concat
```

Si encuentra alguno de esos, exit 1. Los otros siguen como
warnings (no fallan el build).

Para que **cualquier finding falle**:

```bash
fitz lint --deny unused_variable --deny unused_import \
         --deny useless_match --deny string_concat
```

### Output sin findings

```bash
fitz lint
```

```
✓ sin findings (3 archivo(s) revisado(s))
```

### Exit codes

| Exit code | Causa |
|---|---|
| 0 | Sin findings, o findings que no matchean `--deny`. |
| 1 | Findings que matchean algún `--deny`. |
| 1 | Error de lectura/parseo de algún archivo. |

📚 **Catálogo y filosofía del linter**: [cap 27 — `fitz lint`](../../guide.md#27-fitz-lint-linter-de-patrones)
de la guía.

---

## Paso 5 — `fitz dev` (hot reload)

### Sintaxis

```
fitz dev [OPTIONS]
```

| Flag | Qué hace |
|---|---|
| `--file <FILE>` | Single-file mode. Sin él, manifest mode (lee `fitz.toml`). |
| `--bin <NAME>` | Elige el bin en un proyecto multi-bin (`server` + `web`). |
| `--port <N>` | Puerto del dev server (modo wasm-client). Default `1234`. |

### Cómo funciona

`fitz dev` **levanta tu programa y lo re-arranca** cada vez que
un `.fitz`/`.fitzv` o el `fitz.toml` cambia. Análogo a `nodemon`,
`cargo watch`, o `vite dev`.

> **Modo wasm-client.** Si el bin apunta a `wasm-client` (un
> componente `.fitzv` que compila a WebAssembly, M9), `fitz dev`
> en vez de kill+respawn **buildea el bundle y lo sirve con
> auto-refresh en el browser** (live-reload + preservación de
> state). Editar `fitz.toml` (repuntar el entry, agregar deps) se
> toma en vivo, sin reiniciar. En un proyecto fullstack `server` +
> `web`: `fitz run --bin server` en una terminal, `fitz dev --bin
> web` en otra.
> Detalle en [cap 25 de la guía](../../guide.md#25-fitz-dev-hot-reload).

```bash
fitz dev
```

```
🟢 fitz dev — corriendo src/main.fitz
   esperando cambios... (Ctrl+C para salir)

[run 1]
Saludos desde Patagonia

```

Editás `src/main.fitz`, guardás → el child process se mata, el
output se limpia y arranca de nuevo:

```
[run 2]
Saludos desde Patagonia
Y ahora también desde Bariloche
```

### Estrategia

- **Kill + respawn** del proceso entero. No hay incremental
  rebuild (deuda del lenguaje).
- **Debounce 100ms** — colapsa saves múltiples del editor en un
  solo restart.
- **Ctrl+C** mata el child antes de salir limpio (sin zombies).

### Exclusiones automáticas

`fitz dev` ignora cambios en:

| Path / pattern | Por qué |
|---|---|
| `target/` | Genera el codegen, no es source. |
| `.git/` | Cambia constantemente con tu working tree. |
| `node_modules/`, `__pycache__/` | Cache de otros tooling. |
| `.fitz/`, `dist/`, `build/` | Output dirs. |
| Archivos ocultos (`.*`) | No-source típico. |

Solo dispara restart con `.fitz` o `fitz.toml` adentro del
proyecto.

### Cuándo usarlo

- Desarrollando un servidor HTTP (M4) y querés ver cambios sin
  Ctrl+C + run manual.
- Iterando sobre un script que toma 1-2s en arrancar.
- Demos en vivo donde querés que cambios se reflejen al
  instante.

**Cuándo NO usarlo**:
- Migrations destructivas o side effects irreversibles (cada
  restart corre todo desde cero).
- Programs que necesitan retener state entre cambios (no hay
  hot-swap real).

📚 **Detalle**: [cap 25 — `fitz dev`](../../guide.md#25-fitz-dev-hot-reload)
de la guía.

---

## Paso 6 — `fitz test` (preview, deep en M2.C6)

Adelanto para no dejarlo afuera del CLI esencial:

```bash
fitz test                  # todos los @test del proyecto
fitz test <FILTER>         # solo los que matchean substring
fitz test --file <FILE>    # single-file mode
```

Detalle completo + assertions + workflow en
**M2.C6 (Funciones + `fitz test`)** — cuando aprendamos a
definir fns con `@test`.

---

## Paso 7 — Workflow estándar pre-commit

Antes de hacer `git commit`:

```bash
fitz fmt                     # 1. normalizar formato
fitz lint                    # 2. detectar patrones malos
fitz check                   # 3. verificar tipos
fitz test                    # 4. correr tests (si los hay)
fitz run                     # 5. confirmar que el happy path anda
```

Los 5 corren en `<3s` cada uno en proyectos chicos.

### Pre-commit hook de git

Para que esto pase **automático** antes de cada commit, agregá
`.git/hooks/pre-commit` con:

```bash
#!/usr/bin/env bash
set -e
fitz fmt --check
fitz lint --deny unused_variable --deny string_concat
fitz check
fitz test
```

```bash
chmod +x .git/hooks/pre-commit
```

Ahora `git commit` aborta si alguno falla.

### Workflow CI

Lo que típicamente va en `.github/workflows/ci.yml` o
equivalente:

```yaml
- run: fitz fmt --check
- run: fitz lint --deny unused_variable --deny unused_import --deny string_concat
- run: fitz check
- run: fitz test
```

> **Tip de IDE**: VSCode con la extensión Fitz **te muestra los
> diagnostics del checker en vivo** (C3). Eso cubre el `fitz
> check` interactivo. `fmt` y `lint` son los que conviene correr
> en CI o atarlos al pre-commit hook.

---

## Paso 8 — Aplicar todo a `mi-saludos`

Editá `src/main.fitz` con código que va a disparar varios
issues:

```fitz
let nombre = "Patagonia"
let edad_olvidada = 42
print("hola " + nombre)
let resultado = match nombre {
    _ => "siempre default"
}
print(resultado)
```

Corré la suite completa:

```bash
fitz fmt
```

```
✓ formateado /ruta/.../src/main.fitz
```

```bash
fitz lint
```

```
warning: variable `edad_olvidada` declarada pero no usada [unused_variable]
  --> /ruta/.../src/main.fitz:2:1
  = nota: si es intencional, prefijá con `_` (ej. `_edad_olvidada`) o suprimí con `// @allow(unused_variable)` en la línea anterior.

warning: `match` con un solo arm catch-all es equivalente al body directo [useless_match]
  --> /ruta/.../src/main.fitz:4:17
  = nota: reemplazá `match x { _ => "default" }` por `"default"` directo.

2 findings en 1 archivo(s)
```

```bash
fitz check
```

```
✓ /ruta/.../src/main.fitz — sin errores de tipo
```

```bash
fitz run
```

```
hola Patagonia
siempre default
```

Arreglá los warnings:

```fitz
let nombre = "Patagonia"
let _edad = 42                  // prefijo _ silencia unused_variable
print("hola {nombre}")          // interpolación en vez de "+"
let resultado = "siempre default"  // sin match inútil
print(resultado)
```

```bash
fitz lint
```

```
✓ sin findings (1 archivo(s) revisado(s))
```

---

## Validación

- [ ] `fitz --help` lista los comandos; `fitz --version` imprime
      `fitz 0.11.1` (o más reciente).
- [ ] `fitz run` desde la raíz del proyecto ejecuta `[bin].main`
      sin pasar el archivo explícitamente.
- [ ] `fitz check` exit 0 con código válido, exit 1 con un
      error de tipo a propósito; reporta **todos los errores**
      en un pase.
- [ ] `fitz fmt --check` exit 1 cuando hay archivos sin
      formatear; `fitz fmt` (sin `--check`) los normaliza.
- [ ] `fitz lint --deny unused_variable` exit 1 si tu proyecto
      tiene una var no usada; sin `--deny`, exit 0 con warning.
- [ ] `fitz dev` arranca el child, lo restartea al modificar un
      `.fitz`, y Ctrl+C lo termina limpio.

---

## Troubleshooting

**`fitz run` sin args me dice "no se encontró fitz.toml"**

- Estás fuera de un proyecto. Opciones:
  - `cd` a una carpeta con `fitz.toml`.
  - O pasá el archivo explícito: `fitz run hola.fitz`.
  - O creá un proyecto: `fitz new mi-proyecto`.

**`fitz check` me reporta un solo error y hay más**

- Error de parseo serio puede limitar el recovery. Arreglá ese y
  el segundo pase va a mostrar más. Si no, es bug y vale issue.

**`fitz fmt` me cambió cosas que no quería**

- El estilo canónico es **fijo** (cero config). No hay opciones.
  Si te molesta un cambio puntual, suprimilo manteniendo el
  archivo fuera de `fitz fmt` (no recomendado — preferí
  adaptarte al estilo).

**`fitz lint` me lista warnings que no entiendo**

- Cada warning trae nota con cómo arreglar. La nota es la
  primera línea de troubleshooting. Si querés más detalle,
  guide cap 27.

**`fitz dev` no detecta mis cambios**

- En Windows con WSL puede haber latency. Asegurate de editar el
  archivo desde el sistema que corre `fitz dev` (no cross-system
  WSL ↔ Windows).
- Verificá que el path del archivo esté **adentro del proyecto**
  (`fitz dev` ignora cambios fuera del cwd / manifest dir).

**`fitz run -- arg1 arg2` me da error si no tengo `@command`**

- Los args extras tras `--` solo se usan cuando tu programa
  tiene `@command`. Sin `@command`, Fitz simplemente los ignora
  (no aborta) — pero verificá el output del programa si
  esperabas que los leyera.

**Exit code distinto del esperado**

- Tabla en la sección de cada comando arriba. Cualquier `fitz
  *` con error severo (lex, parse, IO) sale con 1.

---

## Lo que viene en C5

Vimos el CLI desde la perspectiva de "ejecutar mi código".
En el próximo cap arrancamos con el **REPL** — un shell
interactivo donde ingresás expresiones y statements línea
por línea. Perfecto para experimentar con la sintaxis, probar
fns sin crear archivos, y inspeccionar tipos en vivo con
`:type`.
