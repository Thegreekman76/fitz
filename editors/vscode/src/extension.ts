// extension.ts — VSCode entry point para Fitz Language (Fase 9.x.1.c).
//
// Capa fina: spawnea `fitz-lsp` como proceso hijo y conecta el
// LanguageClient estándar de Microsoft. Toda la inteligencia (parsing,
// type checking, diagnostics, futuras features de hover/completion)
// vive del lado Rust en el binario `fitz-lsp` (src/bin/fitz-lsp.rs).

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
  const config = vscode.workspace.getConfiguration("fitz");
  // `fitz.lspPath` puede ser:
  //  - un nombre suelto ("fitz-lsp") → se busca en PATH
  //  - un path absoluto → se usa directo
  //  - un path relativo → se resuelve relativo al workspace folder activo
  // Sin workspace activo y path relativo, fallamos prolijo.
  const lspPath = resolveServerPath(config.get<string>("lspPath", "fitz-lsp"));

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

function resolveServerPath(raw: string): string {
  // Path absoluto → tal cual.
  if (path.isAbsolute(raw)) {
    return raw;
  }
  // Path relativo (contiene separador) → relativo al workspace.
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
