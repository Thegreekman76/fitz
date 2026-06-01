# C2 — `fitz new` (proyecto skeleton)

**Pre-requisitos**: [C1 — Instalación](c1-instalacion.md) terminado.

**Objetivo**: crear el proyecto que va a **crecer con vos durante el
resto del curso**. Al final del M6 va a ser una app HTTP + DB +
auth; hoy es un "hola mundo", pero ya con la estructura estándar
de cualquier proyecto Fitz.

**Por qué importa**: hasta C1 hacíamos archivos sueltos. Eso sirve
para experimentar, pero un proyecto real tiene un `fitz.toml`
(manifest), `src/` (código), `.gitignore` (qué no commitear), y
`git init` (versionado). `fitz new` arma todo eso en un paso.

---

## Paso 1 — Crear el proyecto

Andá a la carpeta donde guardás tus proyectos (`~/proyectos`,
`D:\dev`, lo que uses). Después:

```bash
fitz new mi-saludos
```

Output esperado:

```
✓ proyecto Fitz creado en `mi-saludos`

Para probarlo:
  cd mi-saludos
  fitz run src/main.fitz
```

Vamos a ver qué generó:

```bash
cd mi-saludos
ls -la       # Linux/macOS
# o
dir          # Windows
```

```
.git/
.gitignore
fitz.toml
src/
```

Cuatro cosas:

- **`fitz.toml`** — el manifest del proyecto. Análogo a `Cargo.toml`
  (Rust), `package.json` (Node), o `pyproject.toml` (Python).
  Declara nombre, versión, y entry point.
- **`src/`** — donde vive tu código Fitz.
  `src/main.fitz` adentro.
- **`.gitignore`** — qué archivos NO subir al repo (compilados,
  binarios temporales).
- **`.git/`** — repo de git inicializado. `fitz new` corre
  `git init` automático (si no querés, pasale `--no-git`).

> **Convención de nombres**: el nombre del proyecto debe ser
> minúscula + dígitos + `-`/`_`, empezando por letra. `mi-saludos`
> y `mi_saludos` válidos; `MiSaludos` o `2saludos` no.

---

## Paso 2 — Inspeccionar `fitz.toml`

Abrí `fitz.toml` (con VSCode si querés, ya que estás):

```toml
[package]
name = "mi-saludos"
version = "0.1.0"
edition = "2026"

[bin]
main = "src/main.fitz"
```

Tres campos:

- **`[package]`** — metadata del paquete.
  - `name` y `version` — los identifican (en el futuro, también
    cuando publiques a un registry).
  - `edition` — versión del lenguaje. Por ahora `"2026"`.
- **`[bin]`** — qué archivo es el entry point del binario.
  `src/main.fitz` por default.

Cuando agreguemos dependencias en M3 va a aparecer una sección
`[dependencies]`. Por ahora, esto es todo lo que necesitás saber.

📚 **Detalle exhaustivo**: [cap 16b — Package manager](../../guide.md#16b---package-manager-fitztoml-fitz-new-fitz-add)
de la guía.

---

## Paso 3 — `src/main.fitz`

Abrí `src/main.fitz`:

```fitz
// main.fitz — generado por `fitz new`
//
// Tu primer programa Fitz. Corrélo con `fitz run src/main.fitz`.
// Cuando 9.y.2 aterrice, también vas a poder simplemente `fitz run`
// desde la raíz del proyecto (lee `fitz.toml` automáticamente).

print("Hola desde mi-saludos 🏔️")
```

Spoiler: **9.y.2 ya aterrizó** (la fase que menciona el comentario).
Ese comentario es del template viejo y va a cambiar en próximas
versiones. Mientras tanto, lo aprovechamos para mostrarte la
ventaja del manifest mode.

---

## Paso 4 — Correr el proyecto

Dos formas equivalentes, **ambas funcionan**:

```bash
# Forma 1 — con archivo explícito (single-file mode)
fitz run src/main.fitz

# Forma 2 — desde la raíz del proyecto, sin args (manifest mode)
fitz run
```

Output esperado en las dos:

```
Hola desde mi-saludos 🏔️
```

**Manifest mode** es lo que vas a usar 95% del tiempo. `fitz run`
sin args sube por las carpetas buscando un `fitz.toml` (Cargo-style),
lo lee, y ejecuta el `[bin].main` declarado. Funciona desde
cualquier subcarpeta del proyecto.

```bash
# También funciona desde adentro de src/:
cd src
fitz run
```

---

## Paso 5 — Modificar y volver a correr

Probemos cambiar algo. Editá `src/main.fitz`:

```fitz
let nombre = "Patagonia"
let edad = 200

print("Saludos desde {nombre}")
print("Tenés {edad} años de historia")
```

Guardá y corré:

```bash
fitz run
```

```
Saludos desde Patagonia
Tenés 200 años de historia
```

Si abriste VSCode con el proyecto, deberías haber visto el LSP
en acción mientras editabas:
- `nombre` y `edad` con su tipo inferido (`Str` e `Int`).
- Colores distintos para keywords (`let`), strings, y la
  interpolación `{nombre}`.

Eso lo exprimimos en C3.

---

## Paso 6 — `fitz init` (variante)

`fitz new <nombre>` crea **una carpeta nueva**. Si ya tenés la
carpeta creada (por ejemplo, hiciste `git clone` antes), usá
`fitz init`:

```bash
mkdir mi-otro-proyecto
cd mi-otro-proyecto
fitz init
```

```
✓ proyecto Fitz `mi-otro-proyecto` inicializado en `/ruta/a/mi-otro-proyecto`

Para probarlo:
  fitz run src/main.fitz
```

El nombre se deriva del directorio actual. Si querés sobrescribir,
pasá `--name`:

```bash
fitz init --name otro-nombre
```

---

## Paso 7 — Variante `--http`

Tanto `fitz new` como `fitz init` aceptan `--http` para arrancar
con un template de servidor HTTP en vez del "hola mundo" CLI:

```bash
fitz new mi-api --http
```

`src/main.fitz` generado:

```fitz
@get("/")
fn index() -> Str {
    return "Hola desde mi-api 🏔️"
}

@server(3000)
fn main() => 0
```

Para probar:

```bash
cd mi-api
fitz run
# ... servidor HTTP en 127.0.0.1:3000 ...
# (en otra terminal)
curl http://127.0.0.1:3000/
# Hola desde mi-api 🏔️
```

No vamos a usar HTTP en M1 — esto es solo para que sepas que
existe. El módulo **M4 (HTTP first-class)** lo arranca en serio.

---

## Validación

- [ ] `fitz new mi-saludos` creó la carpeta con `fitz.toml`,
      `src/main.fitz`, `.gitignore`, `.git/`.
- [ ] `fitz run` desde la raíz del proyecto imprime el saludo.
- [ ] `fitz run src/main.fitz` también funciona (ambas formas
      son válidas).
- [ ] Cuando modificás `src/main.fitz`, el siguiente `fitz run`
      refleja los cambios sin extra configuración.

---

## Troubleshooting

**`fitz: command not found`** — volvé a C1, paso 3. El binario
no está en el `PATH`.

**`error: el nombre 'X' no es válido`** — repasá la convención:
minúscula + dígitos + `-`/`_`, empieza por letra.

**`error: el archivo 'fitz.toml' ya existe`** (con `fitz init`) —
ya hay un proyecto Fitz en esa carpeta. Si querés empezar de cero,
borralo manualmente.

**`error: no se encontró 'fitz.toml' en el directorio actual ni en
ancestros`** (con `fitz run` sin args) — estás fuera del proyecto.
`cd mi-saludos` y volvé a probar.

---

## Lo que viene en C3

Ya tenés el proyecto. En el próximo cap **lo abrimos en VSCode y
exprimimos el LSP**: hover con tipos, autocomplete contextual,
errores subrayados en vivo, y Ctrl+Click para navegar al código.

Es la diferencia entre escribir Fitz "a mano" y escribirlo como
escribís TypeScript o Rust con su tooling moderno.
