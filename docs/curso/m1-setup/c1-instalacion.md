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

Tenés dos caminos. **Elegí uno**.

### Opción A — Bajar el binario pre-compilado (recomendado)

El más rápido. No requiere instalar Rust ni compilar nada.

1. Andá a [github.com/Thegreekman76/fitz/releases/latest](https://github.com/Thegreekman76/fitz/releases/latest).
2. En la sección "Assets" del release más reciente, bajá el archivo
   que corresponda a tu plataforma:

   | Plataforma | Archivo |
   |---|---|
   | Windows x64 | `fitz-vX.Y.Z-win32-x64.zip` |
   | Linux x64 | `fitz-vX.Y.Z-linux-x64.tar.gz` |
   | Linux ARM64 (Raspberry Pi, AWS Graviton) | `fitz-vX.Y.Z-linux-arm64.tar.gz` |
   | macOS Apple Silicon (M1/M2/M3/M4) | `fitz-vX.Y.Z-darwin-arm64.tar.gz` |

   > **¿No ves macOS Intel o Windows ARM64?** No los publicamos
   > pre-compilados (escasez de runners macos-13 + axum aún no
   > compila estable en win32-arm64). Si estás en uno de esos,
   > seguí la Opción B.

3. Extraé el archivo y movelo a una carpeta que esté en tu `PATH`.
   Esto es lo que permite que el comando `fitz` funcione desde
   cualquier carpeta de la terminal.

   **Linux / macOS** — dos opciones, elegí una:

   ```bash
   # Opción A — al PATH del sistema (requiere sudo, disponible para
   # todos los usuarios de la máquina):
   tar -xzf fitz-vX.Y.Z-<plataforma>.tar.gz
   sudo mv fitz /usr/local/bin/
   sudo chmod +x /usr/local/bin/fitz

   # Opción B — al PATH del usuario (sin sudo, solo para tu usuario):
   tar -xzf fitz-vX.Y.Z-<plataforma>.tar.gz
   mkdir -p ~/.local/bin
   mv fitz ~/.local/bin/
   chmod +x ~/.local/bin/fitz

   # Si elegiste la Opción B, asegurate que ~/.local/bin esté en el
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
   Expand-Archive -Path fitz-vX.Y.Z-win32-x64.zip -DestinationPath C:\Tools\fitz -Force

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

### Opción B — Compilar desde fuente

Si tu plataforma no está en la matriz, o querés trackear `main`.

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

## Troubleshooting común

**`fitz: command not found` después de mover el binario**

- ¿Reabriste la terminal? Los cambios al `PATH` requieren shell
  nueva.
- **Linux/macOS**: ¿está ejecutable? `chmod +x /usr/local/bin/fitz`.
- **Windows**: ¿editaste el PATH del **Usuario** o el del **Sistema**?
  Ambos sirven, pero el del Sistema requiere reiniciar la terminal
  como administrador para que aplique.

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

---

## Lo que viene en C2

En el próximo capítulo dejamos de crear archivos sueltos. Vamos a
usar `fitz new` para crear un **proyecto** con la estructura estándar
(`fitz.toml`, `src/main.fitz`, `.gitignore`). Empezamos a trabajar
como en cualquier proyecto real.
