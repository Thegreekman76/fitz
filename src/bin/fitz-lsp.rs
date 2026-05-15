// fitz-lsp — Language Server Protocol para Fitz (Fase 9.x).
//
// Bin separado del CLI principal (`fitz`) por convención del ecosistema:
// rust-analyzer, gopls, tsserver, pyright son todos binarios aparte que
// el cliente (extensión VSCode, plugin Neovim, etc.) spawnea como
// proceso hijo y se comunica por stdio con JSON-RPC.
//
// Este archivo es 9.x.1.a: skeleton mínimo. Solo handshake del protocolo
// (`initialize` → `shutdown`). NO consume `parse_with_recovery` ni
// `check_program` todavía — eso entra en 9.x.1.b junto con el refactor
// `lib + bin` necesario para que este crate exponga el pipeline a
// otros bins.
//
// Build: `cargo build --features lsp` (la dep `tower-lsp` es opt-in;
// `required-features = ["lsp"]` evita errores de link en el flujo
// default).

use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::{
    InitializeParams, InitializeResult, InitializedParams, MessageType, ServerCapabilities,
    ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

#[derive(Debug)]
struct Backend {
    client: Client,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _params: InitializeParams) -> LspResult<InitializeResult> {
        // Capabilities mínimas en 9.x.1.a: anunciamos que vamos a
        // recibir el documento entero en cada `did_change`. La
        // alternativa `INCREMENTAL` requiere que el server mantenga
        // el buffer y aplique edits — más eficiente sobre archivos
        // grandes pero suma complejidad. FULL es el default razonable
        // para el MVP; la migración a INCREMENTAL es decisión de
        // perf que dejamos para cuando aparezca presión real.
        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "fitz-lsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                ..ServerCapabilities::default()
            },
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "fitz-lsp inicializado")
            .await;
    }

    async fn shutdown(&self) -> LspResult<()> {
        Ok(())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // current_thread alcanza para el LSP: el server es I/O-bound (lee
    // stdin, escribe stdout, despacha handlers) y no necesita el work
    // stealing del scheduler multi-thread. Mantiene el binario chico
    // y deja la decisión de runtime ortogonal a la del CLI HTTP que
    // sí pide multi-thread (Fase F17).
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(|client| Backend { client });
    Server::new(stdin, stdout, socket).serve(service).await;
}
