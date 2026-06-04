# C1 — Instalación

**Pre-requisitos**: ninguno (es el primer capítulo).

**Objetivo**: tener `fitz --version` funcionando en tu terminal y
la extensión Fitz instalada en VSCode.

**Por qué importa**: sin la herramienta instalada no podemos hacer
nada. Este cap es puro setup — los próximos ya escriben código.

---

## Paso 1 — Instalar VSCode (si no lo tenés)

[VSCode](https://code.visualstudio.com/) es el editor que el curso
asume. Si ya lo tenés, saltá al Paso 2.

1. Andá a [code.visualstudio.com/download](https://code.visualstudio.com/download).
2. Bajá el instalador para tu sistema operativo.
3. Instalalo con los defaults.
4. Verificá que `code --version` funciona en tu terminal:

```bash
code --version
# 1.95.x  (o similar, cualquier versión reciente sirve)
# abc12def
# x64
```

Si `code` no se reconoce en la terminal:
- **Windows / Linux**: el instalador suele agregarlo al `PATH`
  automáticamente; reabrí la terminal.
- **macOS**: abrí VSCode → `Cmd+Shift+P` → "Shell Command: Install
  'code' command in PATH".

---

## Paso 2 — Instalar Fitz

Tenés tres caminos. **Elegí uno** según preferencia (el primero es
el más rápido).

### Opción A — Instalador one-liner (recomendado)

Un solo comando. Detecta tu plataforma, baja el último release,
extrae los binarios a `~/.fitz/bin/` y te dice qué hacer con el
`PATH`. Si ya lo agregaste antes, ni eso pregunta.

**Linux / macOS** (`bash`, `zsh`, `fish` — cualquier shell POSIX
sirve mientras `curl` o `wget` estén instalados):

```bash
curl -sSf https://thegreekman76.github.io/fitz/install.sh | sh
```

**Windows** (PowerShell):

```powershell
irm https://thegreekman76.github.io/fitz/install.ps1 | iex
```

Después del install:

1. Cerrá y reabrí la terminal (el PATH no actualiza hasta entonces).
2. Validá con `fitz --version`.

**Variantes útiles:**

```bash
# Linux / macOS — instalar versión específica
curl -sSf https://thegreekman76.github.io/fitz/install.sh | sh -s -- --version v0.11.1

# Linux / macOS — instalar en un prefix custom (no en ~/.fitz)
curl -sSf https://thegreekman76.github.io/fitz/install.sh | sh -s -- --prefix ~/.local

# Linux / macOS — desinstalar
curl -sSf https://thegreekman76.github.io/fitz/install.sh | sh -s -- --uninstall
```

```powershell
# Windows — instalar versión específica (env var antes del iex)
$env:FITZ_VERSION = "v0.11.1"; irm https://thegreekman76.github.io/fitz/install.ps1 | iex

# Windows — prefix custom
$env:FITZ_PREFIX = "C:\Tools\fitz"; irm https://thegreekman76.github.io/fitz/install.ps1 | iex

# Windows — desinstalar
$env:FITZ_ACTION = "uninstall"; irm https://thegreekman76.github.io/fitz/install.ps1 | iex
```

> **¿No te gusta `curl ... | sh`?** Inspeccionalo antes: el script
> entero vive en
> [github.com/Thegreekman76/fitz/blob/main/docs/install.sh](https://github.com/Thegreekman76/fitz/blob/main/docs/install.sh)
> (y `install.ps1` análogo). Bajalo, leelo, y corrélo a mano si
> preferís. Cualquiera de las opciones B o C también funciona.

> **¿No ves macOS Intel o Windows ARM64?** No los publicamos
> pre-compilados (escasez de runners macos-13 + axum aún no
> compila estable en win32-arm64). En esos casos seguí la
> Opción C (compilar desde fuente).

### Opción B — Bajar el binario manual

Si preferís ver lo que estás bajando. No requiere instalar Rust.

1. Andá a [github.com/Thegreekman76/fitz/releases/latest](https://github.com/Thegreekman76/fitz/releases/latest).
2. En la sección "Assets" del release más reciente, bajá el archivo
   que corresponda a tu plataforma:

   | Plataforma | Archivo |
   |---|---|
   | Windows x64 | `fitz-X.Y.Z-win32-x64.zip` |
   | Linux x64 | `fitz-X.Y.Z-linux-x64.tar.gz` |
   | Linux ARM64 (Raspberry Pi, AWS Graviton) | `fitz-X.Y.Z-linux-arm64.tar.gz` |
   | macOS Apple Silicon (M1/M2/M3/M4) | `fitz-X.Y.Z-darwin-arm64.tar.gz` |

3. Extraé el archivo y movelo a una carpeta que esté en tu `PATH`.
   Esto es lo que permite que el comando `fitz` funcione desde
   cualquier carpeta de la terminal.

   **Linux / macOS** — dos opciones, elegí una:

   ```bash
   # Opción 1 — al PATH del sistema (requiere sudo, disponible para
   # todos los usuarios de la máquina):
   tar -xzf fitz-X.Y.Z-<plataforma>.tar.gz
   sudo mv fitz-X.Y.Z-<plataforma>/fitz /usr/local/bin/
   sudo mv fitz-X.Y.Z-<plataforma>/fitz-lsp /usr/local/bin/
   sudo chmod +x /usr/local/bin/fitz /usr/local/bin/fitz-lsp

   # Opción 2 — al PATH del usuario (sin sudo, solo para tu usuario):
   tar -xzf fitz-X.Y.Z-<plataforma>.tar.gz
   mkdir -p ~/.local/bin
   mv fitz-X.Y.Z-<plataforma>/fitz ~/.local/bin/
   mv fitz-X.Y.Z-<plataforma>/fitz-lsp ~/.local/bin/
   chmod +x ~/.local/bin/fitz ~/.local/bin/fitz-lsp

   # Si elegiste la Opción 2, asegurate que ~/.local/bin esté en el
   # PATH. La mayoría de las distros modernas ya lo incluyen; si no:
   echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
   # (o ~/.zshrc si usás zsh, o ~/.config/fish/config.fish si fish)
   source ~/.bashrc
   ```

   **Windows** (PowerShell):

   ```powershell
   # 1) Extraé el .zip a una carpeta de tu preferencia
   #    (creala antes si no existe):
   New-Item -ItemType Directory -Force -Path C:\Tools\fitz
   Expand-Archive -Path fitz-X.Y.Z-win32-x64.zip -DestinationPath C:\Temp -Force
   Move-Item C:\Temp\fitz-X.Y.Z-win32-x64\fitz.exe C:\Tools\fitz\
   Move-Item C:\Temp\fitz-X.Y.Z-win32-x64\fitz-lsp.exe C:\Tools\fitz\

   # 2) Agregá la carpeta al PATH del usuario (persistente, sin admin):
   $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
   [Environment]::SetEnvironmentVariable("Path", "$userPath;C:\Tools\fitz", "User")

   # IMPORTANTE: cerrá y reabrí la terminal para que el cambio aplique.
   ```

   Si preferís la UI en Windows:
   1. Start → tipeá "environment variables" → click en
      "Edit environment variables for your account".
   2. Click en `Path` (User variables) → "Edit..." → "New" →
      pegá `C:\Tools\fitz` → OK → OK.
   3. Cerrá y reabrí la terminal.

### Opción C — Compilar desde fuente

Si tu plataforma no está en la matriz (macOS Intel, Windows ARM64,
BSD, etc.), o querés trackear `main`.

**Pre-requisito**: tener Rust instalado vía [rustup](https://rustup.rs/).

```bash
git clone https://github.com/Thegreekman76/fitz.git
cd fitz
cargo build --release
```

El binario queda en `target/release/fitz` (o `fitz.exe` en
Windows). Copialo a tu `PATH`:

```bash
# Linux / macOS
sudo cp target/release/fitz /usr/local/bin/

# Windows (PowerShell, asumiendo C:\Tools\fitz\ ya está en PATH)
Copy-Item target\release\fitz.exe C:\Tools\fitz\
```

La primera compilación tarda 3-8 minutos según tu máquina. Las
siguientes son incrementales y rápidas.

---

## Paso 3 — Validar que `fitz` funciona

Cerrá y reabrí la terminal (importante — los cambios al `PATH` no
se aplican hasta entonces). Después:

```bash
fitz --version
# fitz 0.11.1   (o la versión más reciente)
```

Si te dice "command not found" o "fitz is not recognized":
- Reabriste la terminal después de mover el binario al `PATH`?
- El binario está realmente en la carpeta del `PATH`? (correlo con
  ruta absoluta para confirmar: `/usr/local/bin/fitz --version`).
- Tenés permisos de ejecución? (Linux/macOS: `chmod +x
  /usr/local/bin/fitz`).

Si funciona, ahora vemos qué subcomandos tiene:

```bash
fitz --help
```

Vas a ver una lista parecida a esta:

```
Usage: fitz <COMMAND>

Commands:
  run        Ejecuta un programa Fitz
  build      Compila a binario nativo
  check      Type-check sin ejecutar
  new        Crea un proyecto nuevo
  init       Inicializa un proyecto en el cwd
  add        Agrega una dependencia
  remove     Quita una dependencia
  update     Actualiza dependencias git
  test       Corre los @test fns
  fmt        Formatea código
  lint       Linter
  dev        Hot reload
  repl       REPL interactivo
  openapi    Genera schema OpenAPI 3.1
  db         Sub-comandos de DB (migrate, diff, ...)
  py-types   Genera types Fitz desde modelos SQLAlchemy
  ...
```

No te asustes, no vas a usar todos hoy. El próximo capítulo (C2) usa
`fitz new`; el resto va apareciendo a lo largo del curso.

---

## Paso 4 — Instalar la extensión Fitz en VSCode

La extensión te da syntax highlighting, errores subrayados, hover
con tipos y autocomplete. Es lo que diferencia escribir Fitz de
escribir en bloc de notas.

1. Andá de nuevo a [github.com/Thegreekman76/fitz/releases/latest](https://github.com/Thegreekman76/fitz/releases/latest).
2. En "Assets" bajá el `.vsix` de tu plataforma:

   | Plataforma | Archivo |
   |---|---|
   | Windows x64 | `fitz-lang-win32-x64.vsix` |
   | Linux x64 | `fitz-lang-linux-x64.vsix` |
   | Linux ARM64 | `fitz-lang-linux-arm64.vsix` |
   | macOS Apple Silicon | `fitz-lang-darwin-arm64.vsix` |

   El `.vsix` trae el `fitz-lsp` (Language Server) ya compilado
   adentro. No tenés que instalarlo aparte.

3. Instalalo en VSCode. Dos opciones equivalentes:

   **Desde la terminal (más rápido)**:
   ```bash
   code --install-extension fitz-lang-<plataforma>.vsix --force
   ```

   **Desde la UI de VSCode**:
   - Abrí VSCode.
   - `Ctrl+Shift+P` (Windows/Linux) o `Cmd+Shift+P` (macOS).
   - Tipeá "Install from VSIX..." y enter.
   - Seleccioná el archivo bajado.

4. Reiniciá VSCode (o ejecutá "Developer: Reload Window" desde
   `Ctrl+Shift+P`).

---

## Paso 5 — Tu primer "hola mundo"

Antes de cerrar el cap, escribimos y corremos un programa minúsculo
para confirmar que todo el stack funciona.

1. Abrí VSCode y creá un archivo nuevo: `hola.fitz` (cualquier
   carpeta sirve).
2. Pegá esto:

   ```fitz
   print("Hola desde Fitz 🏔️")

   let lugar = "Patagonia"
   print("Saludos desde {lugar}")
   ```

3. Mientras lo escribís, deberías ver:
   - **Syntax highlighting**: `print` en un color, `"hola"` en otro,
     `let` resaltado como keyword.
   - **Hover con tipos**: pasá el mouse sobre `lugar` → tooltip
     dice `lugar: Str`.

4. Guardá el archivo y abrí una terminal en VSCode (`Ctrl+\``).
   Corré:

   ```bash
   fitz run hola.fitz
   ```

   Output esperado:
   ```
   Hola desde Fitz 🏔️
   Saludos desde Patagonia
   ```

Si viste ese output, **ya tenés Fitz funcionando end-to-end**:
binario, editor, language server y un programa real.

---

## Validación

Para considerar este capítulo completo, deberías poder responder
"sí" a las cuatro:

- [ ] `fitz --version` imprime una versión sin error.
- [ ] `code --version` funciona y VSCode abre.
- [ ] En un `.fitz` ves syntax highlighting + hover con tipos.
- [ ] `fitz run hola.fitz` con el programa de arriba imprime los
      dos prints.

Si alguna falla, leé la sección "Troubleshooting" abajo. Si nada
de eso te ayuda, abrí un issue en
[github.com/Thegreekman76/fitz](https://github.com/Thegreekman76/fitz/issues)
describiendo qué pasó.

---

## Desinstalación

Si en algún momento querés sacar Fitz de tu máquina:

**Si instalaste con el one-liner (Opción A):**

```bash
# Linux / macOS
curl -sSf https://thegreekman76.github.io/fitz/install.sh | sh -s -- --uninstall
```

```powershell
# Windows (PowerShell)
$env:FITZ_ACTION = "uninstall"; irm https://thegreekman76.github.io/fitz/install.ps1 | iex
```

Esto borra los binarios `fitz` y `fitz-lsp` del prefix donde se
instalaron. En Windows además remueve el dir del PATH del User.
La cache local (`~/.fitz/cache/` con builds de Cargo y git deps)
NO se borra automáticamente — el script te lo avisa y te dice
cómo limpiarla a mano si querés liberar espacio.

Si usaste un `--prefix` custom al instalar, pasá el mismo al
desinstalar:

```bash
curl -sSf https://thegreekman76.github.io/fitz/install.sh | sh -s -- --uninstall --prefix ~/.local
```

**Si instalaste manual (Opción B):**

Borrá los binarios del PATH donde los pusiste (`/usr/local/bin/`,
`~/.local/bin/`, `C:\Tools\fitz\`, etc.) y, en Windows, quitá la
entrada del PATH del User desde "Edit environment variables".

**Extensión VSCode:**

```bash
code --uninstall-extension fitz-lang.fitz-lang
```

O desde la UI: vista de extensiones → buscar "Fitz" → Uninstall.

---

## Troubleshooting común

**El instalador falla con "no pude consultar la GitHub API"**

GitHub API rate-limita a 60 requests por hora por IP sin
autenticar. Si compartís IP (oficinas, VPN, CI) podés golpear el
límite. Soluciones:
- Esperá 1 hora y reintentá.
- Pasá la versión a mano para skipear la llamada API:

  ```bash
  curl -sSf https://thegreekman76.github.io/fitz/install.sh | sh -s -- --version v0.11.1
  ```

**`fitz: command not found` después de mover el binario**

- ¿Reabriste la terminal? Los cambios al `PATH` requieren shell
  nueva.
- **Linux/macOS**: ¿está ejecutable? `chmod +x /usr/local/bin/fitz`.
- **Windows**: ¿editaste el PATH del **Usuario** o el del **Sistema**?
  Ambos sirven, pero el del Sistema requiere reiniciar la terminal
  como administrador para que aplique.
- **Si usaste el one-liner (Opción A)**: el script instala en
  `~/.fitz/bin/` (Linux/macOS) o `%USERPROFILE%\.fitz\bin\`
  (Windows). En Linux/macOS te pide agregar al PATH manualmente
  (no edita tu `.bashrc` solo); en Windows lo agrega automático
  al PATH del User pero la terminal abierta no lo ve hasta
  cerrar y reabrir.

**Windows: `vcruntime140.dll no se encuentra` al correr `fitz` o crash
del LSP**

Síntomas:

- `fitz --version` falla con un popup "no se encuentra
  vcruntime140.dll".
- La extensión VSCode reporta "Fitz Language Server crashed N times" +
  "couldn't create connection to server" + "Server initialization
  failed".

Causa: los binarios `fitz.exe` y `fitz-lsp.exe` están compilados con
la toolchain MSVC de Rust y dynamic-linkean contra el runtime de
Visual C++. En Windows fresh (sin Visual Studio ni apps que lo
traigan) el DLL falta y los dos binarios se caen al instante. El LSP
muere antes de poder hablar con VSCode, por eso el cliente ve
"couldn't create connection".

Fix: instalá el **Microsoft Visual C++ Redistributable (x64)** desde
Microsoft (gratuito, ~15 MB, no requiere restart):

https://aka.ms/vs/17/release/vc_redist.x64.exe

Después:

1. Abrí una terminal nueva, probá `fitz --version`.
2. En VSCode: `Ctrl+Shift+P` → "Developer: Reload Window".
3. Abrí cualquier `.fitz` y verificá highlighting + hover.

**Extensión 0.13.1: `Unsupported position encoding (utf-8)` (fixed en
0.13.2)**

Síntoma (solo en 0.13.1):

```
[Error] Server initialization failed.
Error: Unsupported position encoding (utf-8) received from server
Fitz Language Server
```

Esto era un bug del LSP server: anunciaba `positionEncoding: utf-8`
en su handshake `initialize`, pero `vscode-languageclient@9` solo
acepta `utf-16` por default y rechazaba la conexión antes de hablar
JSON-RPC. **El binario `fitz.exe` no estaba afectado** — `fitz run`,
`fitz build`, `fitz check` funcionaban normal; solo se rompía la
extensión VSCode.

Fix: actualizá a **0.13.2 o superior**. Bajá el `.vsix` nuevo del
[release de GitHub](https://github.com/Thegreekman76/fitz/releases) y
reinstalá la extensión. Si estás en 0.13.2+ y ves este error, abrí
un issue.

**La extensión está instalada pero no hay highlighting**

- Verificá que el archivo terminó en `.fitz` (la extensión se activa
  por extensión de archivo).
- Reload window: `Ctrl+Shift+P` → "Developer: Reload Window".
- Mirá el output panel: `Ctrl+Shift+U` → en el dropdown elegí "Fitz
  Language Server". Si hay errores ahí, copialos al reportar.

**Hover no muestra tipos**

- El LSP arranca lazy en el primer `.fitz` que abras. Esperá 1-2
  segundos después de abrir el archivo.
- Si pasa más tiempo: chequeá el output panel ("Fitz Language Server")
  por errores.

**`fitz run` da "no se encontró el archivo"**

- Estás en la carpeta correcta? `ls` (Linux/macOS) o `dir` (Windows)
  para confirmar que `hola.fitz` aparece.

**GitHub Copilot sugiere sintaxis que no es de Fitz (ej. `\(var)`)**

Copilot conoce miles de lenguajes pero Fitz es nuevo y está poco
representado en su training. A veces sugiere sintaxis de Swift
(`print("Hola, \(lugar)!")`), Kotlin o Rust que se "parece" a Fitz
pero el parser rechaza. La interpolación correcta en Fitz es estilo
Python f-string / TypeScript template literal: `{var}` directo,
sin backslash.

```fitz
// ❌ Sintaxis de Swift (rechazada por Fitz)
print("Hola, \(lugar)!")

// ✓ Sintaxis correcta de Fitz
print("Hola, {lugar}!")
```

Mientras el ecosistema crece y Copilot aprende, lo más simple es
desactivarlo solo para archivos `.fitz`. Agregá a tu `settings.json`
de VSCode (`Ctrl+Shift+P` → "Preferences: Open User Settings (JSON)"):

```json
"github.copilot.enable": {
  "*": true,
  "fitz": false
}
```

Con esto, Copilot sigue activo en Python/TS/Rust/etc, pero no en
`.fitz`. El LSP de Fitz cubre highlighting, hover, go-to-definition
y autocomplete contextual nativo del lenguaje.

---

## Lo que viene en C2

En el próximo capítulo dejamos de crear archivos sueltos. Vamos a
usar `fitz new` para crear un **proyecto** con la estructura estándar
(`fitz.toml`, `src/main.fitz`, `.gitignore`). Empezamos a trabajar
como en cualquier proyecto real.
