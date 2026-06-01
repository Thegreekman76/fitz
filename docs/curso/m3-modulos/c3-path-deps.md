# M3.C3 — Path deps + lockfile

**Pre-requisitos**: [M3.C2 — Lib local](c2-lib-local.md).
Sabés exponer un paquete como `[lib]` para que otros lo
importen.

**Objetivo**: dominar **dependencies por path** — declarar libs
locales como deps, entender el **lockfile** (`fitz.lock`), las
reglas del resolver, y cuándo commitear el lockfile.

**Por qué importa**: estamos pasando de "código en un solo
proyecto" a "código compartido entre varios proyectos". Path
deps son la forma **más simple** de hacer eso, ideal para
monorepos, libs internas, y prototipado.

---

## Mapa del cap

```mermaid
flowchart LR
    A[Mi app] --> B[fitz.toml<br/>[dependencies]<br/>lib = path]
    B --> C[fitz.lock<br/>auto-generado]
    A --> D[from lib import X]
    D --> E[Loader resuelve<br/>al [lib].entry<br/>de la lib]
```

---

## Paso 1 — Sintaxis de `[dependencies]`

Sección opcional en `fitz.toml`. Si la incluís, declarás cada
dep con su nombre + cómo se resuelve:

```toml
[package]
name = "mi_app"
version = "0.1.0"
edition = "2026"

[bin]
main = "src/main.fitz"

[dependencies]
util = { path = "../util" }
auth = { path = "../shared/auth" }
```

### Tabla de fuentes soportadas

| Fuente | Sintaxis | Cuándo usar |
|---|---|---|
| Path local | `{ path = "../util" }` | Monorepo, libs internas |
| Git con tag | `{ git = "url", tag = "v1.0.0" }` | Lib pública en GitHub (M3.C4) |
| Git con rev | `{ git = "url", rev = "<sha>" }` | Pin exacto a commit (M3.C4) |
| Version (registry) | `"1.0.0"` | ❌ NO soportado (no hay registry público) |

### Path relativos

El `path` es **relativo al `fitz.toml`** que lo declara:

```
~/proyectos/
├── util/                  ← lib
│   └── fitz.toml ([lib].entry = src/lib.fitz)
├── shared/
│   └── auth/              ← otra lib
│       └── fitz.toml
└── mi_app/
    ├── fitz.toml         ← declara las dos como deps
    └── src/main.fitz
```

`mi_app/fitz.toml`:

```toml
[dependencies]
util = { path = "../util" }
auth = { path = "../shared/auth" }
```

### Nombre de la dep vs nombre del paquete

| Concepto | Detalle |
|---|---|
| Nombre del paquete (`[package].name`) | Nombre canónico del proyecto |
| Nombre de la dep en `[dependencies]` | Cómo lo importás (`from <nombre> import X`) |
| Convención | Usar el mismo — facilita lectura |

Ejemplo "convencional":

```toml
[dependencies]
util = { path = "../util" }       # se importa como `from util import X`
```

Si quisieras renombrar (poco común):

```toml
[dependencies]
util = { path = "../another_pkg_name" }
```

Lo importás como `util` aunque la carpeta se llame distinto.

> **No hay alias en `[dependencies]`** estilo `package =
> { path, name }`. La key del TOML es el nombre que vas a usar
> en imports.

---

## Paso 2 — Demo end-to-end (monorepo simple)

Vamos a armar dos proyectos: una lib `string_utils` y una app
`mi_app` que la consume.

### Estructura

```
~/proyectos/
├── string_utils/
│   ├── fitz.toml
│   └── src/lib.fitz
└── mi_app/
    ├── fitz.toml
    └── src/main.fitz
```

### `string_utils/fitz.toml`

```toml
[package]
name = "string_utils"
version = "0.1.0"
edition = "2026"

[lib]
entry = "src/lib.fitz"
```

### `string_utils/src/lib.fitz`

```fitz
fn slugify(s: Str) -> Str {
    return s.lower().replace(" ", "-")
}

fn truncate(s: Str, n: Int) -> Str {
    if (s.len() <= n) { return s }
    return s.split("").filter(fn(_) => true)  // chango — simplifico
}
```

### `mi_app/fitz.toml`

```toml
[package]
name = "mi_app"
version = "0.1.0"
edition = "2026"

[bin]
main = "src/main.fitz"

[dependencies]
string_utils = { path = "../string_utils" }
```

### `mi_app/src/main.fitz`

```fitz
from string_utils import slugify

print(slugify("Hola Patagonia"))   // hola-patagonia
```

### Correr

```bash
cd mi_app
fitz run
```

```
✓ actualizado /ruta/.../mi_app/fitz.lock
hola-patagonia
```

**Notá**: en la primera ejecución, `fitz` auto-genera el
**lockfile** (`fitz.lock`). Ya lo vemos.

---

## Paso 3 — `fitz.lock` — el lockfile

El **lockfile** registra **qué deps se resolvieron y a qué
versión exacta**. Cargo-style, npm-style, pip-style.

### Auto-generado

`fitz` lo crea/actualiza automáticamente en cada `fitz run`,
`fitz build`, o `fitz check` (modo manifest) si:
- El lockfile no existe.
- O las deps cambiaron desde el último.

No tenés que invocarlo manualmente.

### Formato típico

`mi_app/fitz.lock`:

```toml
version = 1

[[package]]
name = "string_utils"
version = "0.1.0"
```

Para path deps, el lockfile registra solo el nombre y versión.
Para git deps registra **además el commit hash exacto** (M3.C4).

### `version = 1`

Versión del formato del lockfile. Mantiene compat hacia
adelante — si Fitz cambia el formato, sumará version `= 2`.

### ¿Por qué importa?

- **Reproducibilidad**: el mismo `fitz.lock` produce builds
  bit-a-bit idénticos en cualquier máquina.
- **CI**: tu CI lee el lockfile y resuelve a las mismas
  versiones que tu local.
- **Auditoría**: ves qué versión de cada dep estás usando.

### ¿Commitearlo o no?

| Tipo de proyecto | Commitear `fitz.lock`? |
|---|---|
| **App** (binario que vas a deployar) | ✅ SÍ — querés builds reproducibles |
| **Lib** (otros la consumen) | ❌ NO — que el consumidor decida sus versiones |

Misma regla que Cargo (`Cargo.lock` se commitea para bins, no
para libs).

### `.gitignore` automático

Si tu proyecto es una **lib**, agregá `fitz.lock` a `.gitignore`
manualmente. Si es app, dejalo (commiteable).

---

## Paso 4 — Múltiples deps

Declarás cada una como una entry separada:

```toml
[dependencies]
util = { path = "../util" }
auth = { path = "../shared/auth" }
logger = { path = "../shared/logger" }
http_helpers = { path = "../http_helpers" }
```

Cada una se resuelve independientemente. El loader las carga
**lazy** (solo cuando hacés `from <dep> import X`).

```fitz
// Solo carga util y auth — logger y http_helpers no se tocan
from util import doblar
from auth import verify_token
```

---

## Paso 5 — Resolución del loader

```mermaid
flowchart TD
    A[from X import Y en mi código] --> B{¿X está declarado<br/>en dependencies?}
    B -->|Sí| C[Resolver dep:<br/>cargar de path / git / cache]
    B -->|No| D{¿X es un módulo local?<br/>X.fitz adyacente?}
    D -->|Sí| E[Cargar como módulo local]
    D -->|No| F[Error: módulo no encontrado]
    C --> G[Cargar [lib].entry de X]
    E --> H[Cargar el archivo local]
```

### Tabla de precedencia

| Caso | Cómo se resuelve |
|---|---|
| `from foo import X` y `foo` está en `[dependencies]` | Dep — carga `<foo>/<entry>.fitz` (del `[lib].entry` de foo) |
| `from foo import X` y `foo.fitz` existe adyacente | Módulo local |
| `from foo import X` y ninguno | Error: módulo no encontrado |

**Las deps ganan** sobre módulos locales con el mismo nombre.
Si tenés ambos, **renombrá** el local para evitar shadowing
sutil.

---

## Paso 6 — Transitive deps — NO soportadas en MVP

**Limitación importante**: si tu lib `auth` declara `auth =
{ path = "../auth" }` que **a su vez** depende de
`crypto_utils`, tu app **no puede importar `crypto_utils`**
automáticamente.

```
mi_app/
└── [dependencies] auth = { path = ".." }

auth/
└── [dependencies] crypto_utils = { ... }   ← deuda visible
```

```fitz
// mi_app/src/main.fitz
from crypto_utils import hash    // ✗ error: no está en mi_app/fitz.toml
```

### Workaround

Declarás **TODAS** las deps que vas a usar en cada paquete que
las use:

```toml
# mi_app/fitz.toml
[dependencies]
auth = { path = "../auth" }
crypto_utils = { path = "../crypto_utils" }   # también acá
```

> **Por qué**: el registry flat del MVP es decisión de diseño
> simplicidad-first. Transitive deps reales (con resolución de
> versiones compatibles) requieren versionado serio
> (registry público) — deuda comprometida.

---

## Paso 7 — Edge cases típicos

### El path no existe

```toml
[dependencies]
util = { path = "../no_existe" }
```

```bash
fitz run
```

```
✗ error resolviendo dep 'util': no se encontró fitz.toml en '../no_existe'
```

### La dep no es válida (no tiene `[lib]`)

```toml
# util/fitz.toml — solo tiene [bin], no [lib]
[bin]
main = "src/main.fitz"
```

Desde `mi_app`:

```fitz
from util import X
```

```bash
fitz run
```

```
✗ error resolviendo dep 'util': no tiene sección [lib] declarada en fitz.toml
```

### Path con espacios o caracteres raros

Tienen que estar entre comillas (TOML standard):

```toml
[dependencies]
util = { path = "../mi util con espacios" }
```

Funciona, pero no recomendado. Usá nombres sin espacios.

### Nombre de dep distinto del paquete real

Funciona — la key del TOML es lo que importás:

```toml
[dependencies]
mi_util = { path = "../some_other_named_pkg" }
```

```fitz
from mi_util import X      // funciona, aunque la carpeta sea distinta
```

---

## Paso 8 — Sin `[dependencies]` — todo local

Si no declarás `[dependencies]`, los `from X import Y` SOLO
resuelven a módulos locales adyacentes:

```toml
# Sin sección [dependencies]
[package]
name = "mi_app"
...
```

```fitz
from util import X       // resuelve a src/util.fitz (módulo local)
from external import Y   // ✗ error: no es módulo local ni dep
```

Esto es el caso de **proyectos chicos** sin necesidad de
compartir código. Empezás así, y agregás `[dependencies]`
cuando crece.

---

## Paso 9 — Workflow típico de monorepo

Estructura para equipo:

```
~/empresa/
├── shared/
│   ├── auth/
│   │   ├── fitz.toml ([lib])
│   │   └── src/lib.fitz
│   ├── logger/
│   │   └── ...
│   └── models/
│       └── ...
└── apps/
    ├── api_users/
    │   ├── fitz.toml ([bin] + deps a shared/*)
    │   └── src/main.fitz
    ├── api_orders/
    │   └── ...
    └── worker_emails/
        └── ...
```

Cada app declara deps a las libs compartidas:

```toml
# apps/api_users/fitz.toml
[dependencies]
auth = { path = "../../shared/auth" }
logger = { path = "../../shared/logger" }
models = { path = "../../shared/models" }
```

Cuando alguien actualiza `shared/auth`, las apps **lo ven al
toque** (siguiente `fitz run` resuelve la nueva versión).

> **Decir "versión" en path deps es flexible** — siempre toma
> el state actual del filesystem. Si necesitás "freeze" a una
> versión específica, mové a git deps (M3.C4) con tag.

---

## Paso 10 — Limitaciones MVP

| Feature | Estado | Workaround |
|---|---|---|
| Path deps | ✅ | — |
| Lockfile auto-generado | ✅ | — |
| Transitive deps | ❌ | Declarar todas en cada paquete |
| Resolución de versiones compatibles | ❌ | No hay registry |
| Dev deps (`[dev-dependencies]`) | ❌ | Hoy todo es dep regular |
| Features / optional deps | ❌ | No hay feature flags |
| Multi-version del mismo paquete | ❌ | Una sola version por dep |
| Workspaces (`[workspace]`) | ❌ | No hay |
| `cargo doc` equivalente | ❌ | No hay generador de docs (deuda) |

---

## Paso 11 — Aplicarlo a `mi-saludos` + crear una lib

Vamos a partir `mi_saludos` en dos: una lib `pueblos_lib` y
una app `mi_saludos` que la consume.

```bash
cd ~/proyectos
fitz new pueblos_lib --no-git
cd pueblos_lib
```

Editá `pueblos_lib/fitz.toml`:

```toml
[package]
name = "pueblos_lib"
version = "0.1.0"
edition = "2026"

[lib]
entry = "src/lib.fitz"
```

Editá `pueblos_lib/src/lib.fitz`:

```fitz
type Pueblo {
    nombre: Str
    altitud_m: Int
    habitantes: Int
}

fn altitud_cat(p: Pueblo) -> Str => match p.altitud_m {
    0..=200    => "llanura",
    201..=1000 => "media",
    _          => "altura"
}

fn habitantes_cat(p: Pueblo) -> Str => match p.habitantes {
    0..=999          => "aldea",
    1_000..=9_999    => "pueblo",
    10_000..=99_999  => "ciudad",
    _                => "metrópolis"
}
```

Ahora actualizá `mi-saludos/fitz.toml`:

```toml
[package]
name = "mi_saludos"
version = "0.1.0"
edition = "2026"

[bin]
main = "src/main.fitz"

[dependencies]
pueblos_lib = { path = "../pueblos_lib" }
```

Y `mi-saludos/src/main.fitz`:

```fitz
from pueblos_lib import Pueblo, altitud_cat, habitantes_cat

let pueblos = [
    Pueblo { nombre: "El Chaltén", altitud_m: 405,  habitantes: 2000 },
    Pueblo { nombre: "Bariloche",  altitud_m: 893,  habitantes: 112000 },
    Pueblo { nombre: "Ushuaia",    altitud_m: 23,   habitantes: 82000 },
]

print("Catálogo:")
for p in pueblos {
    print("  - {p.nombre}: {altitud_cat(p)}, {habitantes_cat(p)}")
}
```

```bash
cd mi-saludos
fitz run
```

```
✓ actualizado /ruta/.../mi-saludos/fitz.lock
Catálogo:
  - El Chaltén: media, pueblo
  - Bariloche: media, metrópolis
  - Ushuaia: llanura, ciudad
```

Verificá el lockfile:

```bash
cat fitz.lock
```

```toml
version = 1

[[package]]
name = "pueblos_lib"
version = "0.1.0"
```

`pueblos_lib` ahora es un **paquete independiente**, reusable
desde cualquier otra app. Si actualizás algo adentro, los
consumidores lo ven al siguiente `fitz run`.

---

## Validación

- [ ] Declarás una dep con `path = "..."` en `[dependencies]`.
- [ ] Primer `fitz run` auto-genera `fitz.lock`.
- [ ] `from <dep> import X` carga desde el `[lib].entry` de la
      dep.
- [ ] Cambios en la lib se reflejan en la app al siguiente
      `fitz run` (sin `fitz update`).
- [ ] Si declarás un path inexistente, error claro al `fitz
      run`.

---

## Troubleshooting

### `error: resolución de dep 'X' falló: path no existe`

- Verificá que el path es relativo al **`fitz.toml`** que lo
  declara, no al cwd.
- Verificá que el archivo `fitz.toml` existe en ese path.

### `error: la dep 'X' no tiene sección [lib]`

La lib debe declarar `[lib].entry` para ser importable. Agregalo:

```toml
[lib]
entry = "src/lib.fitz"
```

### `from X import Y` me dice "símbolo no existe en X"

- ¿`Y` está declarado en el `src/lib.fitz` de la dep?
- Cualquier fn/type/let top-level adentro de `[lib].entry`
  es exportable.

### Modifiqué la lib pero la app no ve los cambios

- ¿Path dep o git dep? Path deps siempre reflejan filesystem
  actual. Git deps requieren `fitz update` para re-clonar.
- ¿`fitz run` invalidó el cache? Path deps no se cachean a
  disco — se leen fresh cada vez.

### `fitz.lock` cambió y no quiero commitearlo

Si tu proyecto es **lib**, agregá `fitz.lock` a `.gitignore`.
Si es **app**, commiteá los cambios (es esperado que cambien
cuando agregás/actualizás deps).

### Quiero versionar la dep a un commit específico

Para path deps, no hay versión — siempre el filesystem actual.
Si necesitás pinear, **usá git deps con `rev`** (M3.C4).

---

## Lo que viene en C4

Vimos path deps — perfectas para monorepo. En el próximo cap
aprendemos **git deps** para libs publicadas en GitHub/GitLab/
self-hosted git, con `tag` para versionado SemVer o `rev` para
pin a commit exacto. Incluyendo el cache local y cuándo
invalidar.
