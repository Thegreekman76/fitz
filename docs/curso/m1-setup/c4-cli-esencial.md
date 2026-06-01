# C4 — CLI esencial (run / check / fmt / lint)

**Pre-requisitos**: [C3 — Hola mundo + LSP](c3-hola-lsp.md)
terminado. Ya tenés `mi-saludos/` abierto en VSCode y sabés
escribir Fitz con el editor.

**Objetivo**: dominar los **4 comandos CLI que vas a usar todos
los días** y entender qué da cada uno. Al final del cap tenés un
workflow estándar pre/post commit.

**Por qué importa**: el LSP de C3 te cubre mientras escribís.
Pero antes de commitear, abrir un PR, o correr en CI, lo
canónico es pasar por la terminal. Los 4 comandos de este cap
son los que vas a ver en cualquier proyecto Fitz real.

---

## Los 4 comandos en una tabla

| Comando | Qué hace | Cuándo lo usás |
|---|---|---|
| `fitz run` | Ejecuta tu programa | Probar que anda |
| `fitz check` | Type-check sin ejecutar | CI, smoke rápido |
| `fitz fmt` | Formatea código al estilo canónico | Antes de commitear |
| `fitz lint` | Detecta patrones malos | Antes de commitear |

`fitz run` es **destructivo** en el sentido de que ejecuta tu
código (puede tocar la DB, hacer requests, etc). Los otros tres
son **read-only**: leen tus archivos y reportan.

---

## Paso 1 — `fitz run`

Ya lo conocés de C2. Dos modos:

```bash
# Manifest mode — sin args, usa fitz.toml
fitz run

# Single-file mode — un archivo cualquiera, sin proyecto
fitz run hola.fitz
```

Editá `src/main.fitz` para tener algo mínimo:

```fitz
let nombre = "Patagonia"
print("Saludos desde {nombre}")
```

Corré:

```bash
fitz run
```

```
Saludos desde Patagonia
```

> **`fitz run` corre el checker primero** (modo strict por
> default). Si hay errores de tipo, **aborta sin ejecutar**.
> Para saltarlo: `fitz run --no-typecheck` — pero en general no
> querés.

---

## Paso 2 — `fitz check`

Como `fitz run` pero **sin ejecutar el programa**. Solo verifica
tipos y sintaxis. Es el más rápido y el que vas a usar más en CI.

```bash
fitz check
```

```
✓ /ruta/a/mi-saludos/src/main.fitz — sin errores de tipo
```

Exit code 0.

Ahora rompé el archivo a propósito:

```fitz
let edad: Int = "doscientos"
print(edad)
```

```bash
fitz check
```

```
✗ /ruta/a/mi-saludos/src/main.fitz — 1 error(es) de tipo:
  Error en línea 1:1 — `edad` declarado como `Int` recibió un valor `Str`
```

Exit code 1. **Eso es lo que CI necesita** para fallar el build
si el código no compila.

Arreglalo (`let edad: Int = 200`) y seguí.

📚 **Detalle del checker**: [cap 15 — Errores y debugging](../../guide.md#15---errores-y-debugging)
de la guía.

---

## Paso 3 — `fitz fmt`

Formatea el código al estilo canónico de Fitz. Cero config:
4 espacios indent, comillas dobles, blank lines entre fns
top-level. No discutís sobre estilo nunca más.

Editá `src/main.fitz` con un estilo deliberadamente "feo":

```fitz
let x=1
fn double(n:Int)->Int=>n*2
print(double(x))
```

Antes de tocarlo, probá `--check` (modo CI):

```bash
fitz fmt --check
```

```
✗ /ruta/a/mi-saludos/src/main.fitz no está en formato canónico

uso `fitz fmt` (sin `--check`) para aplicar el formato.
```

Exit 1 — útil para que CI falle si alguien commiteó sin formatear.

Ahora aplicalo (sin `--check` = modo write):

```bash
fitz fmt
```

```
✓ formateado /ruta/a/mi-saludos/src/main.fitz
```

Volvé a abrir el archivo:

```fitz
let x = 1

fn double(n: Int) -> Int {
    return n * 2
}

print(double(x))
```

Diferencias:
- Espacios alrededor del `=`, `:`, `->`, `*`.
- `fn double(n: Int) -> Int => n * 2` se expandió a bloque con
  `return` explícito (preferido para fns multilinea claras).
- Blank line entre la fn y el `print` del scope superior.

> **`fitz fmt` preserva tus comentarios y blank lines
> intencionales** — no es un re-print bruto del AST. Comentarios
> al final de línea, blank lines entre secciones, todo se
> respeta.

📚 **Detalle exhaustivo**: [cap 23 — `fitz fmt`](../../guide.md#23---fitz-fmt--formateo)
de la guía + [`docs/fmt-style.md`](../../fmt-style.md) para la
referencia completa.

---

## Paso 4 — `fitz lint`

Si `fitz check` verifica que **compila**, `fitz lint` verifica
que **está bien escrito**. Detecta 4 patrones malos:

| Lint | Qué detecta |
|---|---|
| `unused_variable` | `let x = 1` y `x` nunca se usa |
| `unused_import` | `import foo` y `foo.algo` no aparece |
| `useless_match` | `match x { _ => ... }` (un solo arm catch-all) |
| `string_concat` | `"a" + "b"` (preferí interpolación `"{a}{b}"`) |

Editá `src/main.fitz` con problemas reales:

```fitz
let nombre = "Patagonia"
let edad_olvidada = 42
print("hola " + nombre)
```

Corré:

```bash
fitz lint
```

```
warning: variable `edad_olvidada` declarada pero no usada [unused_variable]
  --> /ruta/a/mi-saludos/src/main.fitz:2:1
  = nota: si es intencional, prefijá con `_` (ej. `_edad_olvidada`) o suprimí con `// @allow(unused_variable)` en la línea anterior.

warning: concatenación de strings — usá interpolación [string_concat]
  --> /ruta/a/mi-saludos/src/main.fitz:3:7
  = nota: reemplazá `"a" + "b"` con `"ab"` (o usá interpolación `"{a}{b}"` si los lados son variables).

2 findings en 1 archivo(s)
```

Output cargo-clippy style: warning + ubicación + nota con cómo
arreglarlo.

**Exit code 0 por default** (warnings no fallan el build).
Para que un lint específico fallé en CI:

```bash
fitz lint --deny unused_variable
# exit 1 si encuentra unused_variable
```

`--deny` es repetible:

```bash
fitz lint --deny unused_variable --deny string_concat
```

### Suprimir un lint puntual

Si el "warning" es intencional, suprimilo con un comentario en
**la línea inmediatamente anterior**:

```fitz
// @allow(unused_variable)
let placeholder = 42
```

Útil cuando estás scaffoldeando algo y querés silenciar
temporalmente.

📚 **Detalle exhaustivo**: [cap 27 — `fitz lint`](../../guide.md#27---fitz-lint)
de la guía.

---

## Paso 5 — Workflow estándar pre-commit

Ahora juntamos todo. Antes de hacer `git commit`, el workflow
canónico de Fitz es:

```bash
fitz fmt          # 1. normalizar el formato
fitz lint         # 2. detectar patrones malos
fitz check        # 3. verificar tipos
fitz run          # 4. confirmar que anda
```

Los 4 corren rápido (`<1s` cada uno en proyectos chicos).

En **CI** suelen ir como:

```bash
fitz fmt --check    # falla si alguien commiteó sin formatear
fitz lint --deny <lints-que-importan>
fitz check
fitz test           # (lo vamos a ver en M2)
```

> **Tip de IDE**: VSCode con la extensión Fitz **te muestra los
> diagnostics del checker en vivo** (C3). Eso cubre el `fitz
> check` interactivo. `fmt` y `lint` son los que conviene correr
> de tanto en tanto desde la terminal (o atarlos a pre-commit
> hooks de git).

---

## Paso 6 — Hot reload con `fitz dev` (bonus)

Hay un quinto comando que vale mencionar: `fitz dev`. **Levanta
tu programa y lo re-arranca cada vez que un `.fitz` o el
`fitz.toml` cambia**. Análogo a `nodemon`, `cargo watch`, o
`vite dev`.

```bash
fitz dev
```

```
🟢 fitz dev — corriendo src/main.fitz
   esperando cambios... (Ctrl+C para salir)

[run 1]
Saludos desde Patagonia

```

Editá `src/main.fitz`, guardá → el output se limpia y arranca de
nuevo automático. Ideal cuando estás desarrollando algo HTTP
(M4) y querés ver cambios sin restart manual.

`fitz dev` ignora `target/`, `.git/`, `__pycache__/` y carpetas
que típicamente cambian por compilación.

---

## Validación

- [ ] `fitz check` exit 0 con código válido, exit 1 con un error
      de tipo a propósito.
- [ ] `fitz fmt` cambia el archivo a su forma canónica; `fitz fmt
      --check` exit 1 si no está canónico.
- [ ] `fitz lint` reporta al menos un warning con un archivo que
      tenga una var no usada o un `"a" + "b"`.
- [ ] Corrés los 4 (`fmt → lint → check → run`) en secuencia y
      todos pasan sobre el `src/main.fitz` limpio.

---

## Troubleshooting

**`fitz fmt` no toca nada y dice "✓ formateado"**

- Quiere decir que tu archivo ya estaba en formato canónico.
  Nada que arreglar.

**`fitz lint` me lista warnings que no entiendo**

- Cada warning trae nota con cómo arreglar. Si querés más
  detalle, ver el cap 27 de la guía (link arriba).

**`fitz dev` no detecta mis cambios**

- En Windows con WSL puede haber latency. Asegurate de editar el
  archivo desde el sistema que corre `fitz dev` (no hacer cambios
  cross-system).
- Excluí carpetas grandes que Fitz no sabe ignorar
  (`venv/`, `node_modules/`, etc. ya están excluidas; el resto,
  pedir como issue).

---

## Lo que viene en C5

Cerramos M1 con el **REPL**. `fitz repl` es un shell interactivo
donde ingresás expresiones y statements línea por línea —
perfecto para experimentar con la sintaxis, probar fns sin crear
archivos, e inspeccionar tipos en vivo con `:type`.

Es la herramienta exploratoria. Después del C5, tenés todo el M1
cerrado y arrancamos M2 (tipos y funciones).
