// extension.ts — VSCode entry point para Fitz Language (Fase 9.x.1.c+).
//
// Capa fina: spawnea `fitz-lsp` como proceso hijo y conecta el
// LanguageClient estándar de Microsoft. Toda la inteligencia (parsing,
// type checking, diagnostics, hover, go-to-def, completion) vive del
// lado Rust en el binario `fitz-lsp` (src/bin/fitz-lsp.rs).
//
// Resolución del binario (Fase 9.x.5 — multi-platform aware):
//
// 1. Si el user setó `fitz.lspPath` a algo distinto del default
//    (`"fitz-lsp"`), respetamos su override.
// 2. Si no, buscamos un binario bundleado en `server/fitz-lsp[.exe]`
//    relativo al `extensionPath` — caso típico cuando el usuario
//    instaló el `.vsix` desde Marketplace.
// 3. Como último fallback, asumimos `fitz-lsp` en el PATH del sistema
//    (caso del flujo alfa de 9.x.1.c, donde el user compiló el binario
//    a mano y lo dejó en PATH o configuró `fitz.lspPath`).

import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export function activate(context: vscode.ExtensionContext): void {
  const lspPath = resolveServerPath(context);

  const serverOptions: ServerOptions = {
    run: { command: lspPath, transport: TransportKind.stdio },
    debug: { command: lspPath, transport: TransportKind.stdio },
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "fitz" }],
    synchronize: {
      // Watcheamos cambios en archivos .fitz fuera del editor (por
      // ejemplo, regenerados por una herramienta externa). El server
      // recibe `workspace/didChangeWatchedFiles` y puede invalidar
      // cache (cuando el LSP tenga un módulo de cache, que hoy no
      // tiene).
      fileEvents: vscode.workspace.createFileSystemWatcher("**/*.fitz"),
    },
  };

  client = new LanguageClient(
    "fitz",
    "Fitz Language Server",
    serverOptions,
    clientOptions,
  );

  // start() lanza el proceso hijo. Si el binario no existe o falla
  // al arrancar, el cliente lo reporta en la output channel
  // "Fitz Language Server" — el usuario lo abre desde
  // View → Output → Fitz Language Server.
  client.start().catch((err: unknown) => {
    const message = err instanceof Error ? err.message : String(err);
    vscode.window.showErrorMessage(
      `fitz-lsp no pudo arrancar (path: ${lspPath}). ${message}. ` +
        `Verificá que el binario exista y sea ejecutable, o ajustá ` +
        `'fitz.lspPath' en settings.`,
    );
  });

  context.subscriptions.push({
    dispose: () => {
      if (client) {
        void client.stop();
      }
    },
  });
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}

/**
 * Resuelve el path al binario `fitz-lsp` siguiendo el orden:
 * 1. `fitz.lspPath` setteado por el user (override manual) → respeta.
 * 2. Binario bundleado en `<extensionPath>/server/fitz-lsp[.exe]` →
 *    caso típico cuando el usuario instaló el `.vsix` desde Marketplace.
 * 3. Fallback al PATH del sistema (`"fitz-lsp"`).
 */
function resolveServerPath(context: vscode.ExtensionContext): string {
  const config = vscode.workspace.getConfiguration("fitz");
  const userPath = config.get<string>("lspPath", "fitz-lsp").trim();

  // (1) Override explícito del user. Distinguimos del default
  // declarado en package.json (`"fitz-lsp"`) — si el user lo cambió,
  // respetamos absoluto/relativo-a-workspace/nombre-suelto.
  if (userPath !== "" && userPath !== "fitz-lsp") {
    return resolveUserPath(userPath);
  }

  // (2) Bundled binary del `.vsix` (Marketplace).
  const bundled = bundledBinaryPath(context.extensionPath);
  if (bundled) {
    return bundled;
  }

  // (3) Fallback PATH (flujo alfa 9.x.1.c — `cargo install`).
  return "fitz-lsp";
}

/**
 * Traduce el setting `fitz.lspPath` del user a path absoluto:
 * - Absoluto → tal cual.
 * - Relativo (contiene separador) → resuelve contra el workspace folder activo.
 * - Nombre suelto → asume PATH.
 */
function resolveUserPath(raw: string): string {
  if (path.isAbsolute(raw)) {
    return raw;
  }
  if (raw.includes("/") || raw.includes("\\")) {
    const folder = vscode.workspace.workspaceFolders?.[0];
    if (folder) {
      return path.join(folder.uri.fsPath, raw);
    }
    // Sin workspace: dejamos el string tal cual y que falle con
    // mensaje claro al spawnear.
    return raw;
  }
  // Nombre suelto (ej. "fitz-lsp") → se asume en PATH.
  return raw;
}

/**
 * Busca el binario bundleado en `<extensionPath>/server/fitz-lsp[.exe]`.
 * Devuelve el path si existe, `null` si no.
 */
function bundledBinaryPath(extensionPath: string): string | null {
  const binaryName =
    process.platform === "win32" ? "fitz-lsp.exe" : "fitz-lsp";
  const bundled = path.join(extensionPath, "server", binaryName);
  if (fs.existsSync(bundled)) {
    return bundled;
  }
  return null;
}
