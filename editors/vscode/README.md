# Fitz Language — extensión VSCode

Syntax highlighting + Language Server Protocol para [Fitz](https://github.com/Thegreekman76/fitz).

## Qué incluye

- **Syntax highlighting** sobre archivos `.fitz` (grammar TextMate).
- **Diagnostics en vivo** — errores del lexer, parser y type checker subrayados al tipear (LSP).
- (Próximamente: hover con tipos, go-to-definition, autocomplete contextual.)

## Instalación

### 1. Compilar el LSP server

Desde la raíz del repo `fitz`:

```bash
cargo build --release --features lsp
```

Esto produce `target/release/fitz-lsp` (o `fitz-lsp.exe` en Windows).

### 2. Compilar la extensión

```bash
cd editors/vscode
npm install
npm run compile
```

### 3. Empaquetar e instalar

```bash
npx vsce package
code --install-extension fitz-language-*.vsix
```

(Requiere `vsce` o `@vscode/vsce` — instalar con `npm install -g @vscode/vsce` si no está.)

### 4. Configurar el path del LSP

Si `fitz-lsp` no está en el `PATH`, agregalo al `settings.json` de VSCode:

```json
{
  "fitz.lspPath": "/abs/path/to/target/release/fitz-lsp"
}
```

En Windows:

```json
{
  "fitz.lspPath": "C:\\Users\\me\\fitz\\target\\release\\fitz-lsp.exe"
}
```

## Settings

| Setting | Default | Descripción |
|---|---|---|
| `fitz.lspPath` | `"fitz-lsp"` | Path al binario `fitz-lsp`. Si es solo el nombre, se busca en `PATH`. |
| `fitz.trace.server` | `"off"` | Traza la comunicación LSP en el output panel "Fitz Language Server". `"verbose"` incluye payloads JSON-RPC completos. |

## Desarrollo

Modo watch para iterar sobre la extensión:

```bash
npm run watch
```

Después abrí esta carpeta en VSCode y apretá F5 para lanzar una "Extension Development Host" donde la extensión está cargada en vivo.

## Versión

Esta extensión sigue el esquema de versionado del lenguaje. La versión actual matchea `v0.9.x` del compilador.
