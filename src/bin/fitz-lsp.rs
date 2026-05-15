// fitz-lsp — Language Server Protocol para Fitz (Fase 9.x).
//
// Bin separado del CLI principal (`fitz`) por convención del ecosistema:
// rust-analyzer, gopls, tsserver, pyright son todos binarios aparte que
// el cliente (extensión VSCode, plugin Neovim, etc.) spawnea como
// proceso hijo y se comunica por stdio con JSON-RPC.
//
// Estado:
// - 9.x.1.a: handshake initialize/initialized/shutdown.
// - 9.x.1.b: did_open/did_change/did_close + diagnósticos en vivo
//   sobre el pipeline `parse_with_recovery → check_program`. La
//   lógica vive en `fitz::lsp` (lib, unit-testeable); este bin es
//   solo la capa fina que mapea handlers de tower-lsp al pipeline.
//
// Build: `cargo build --features lsp` (la dep `tower-lsp` es opt-in;
// `required-features = ["lsp"]` evita errores de link en el flujo
// default).

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    InitializeParams, InitializeResult, InitializedParams, MessageType, ServerCapabilities,
    ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind, Url,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

use fitz::lsp::{check_source, fitz_errors_to_diagnostics};

#[derive(Debug)]
struct Backend {
    client: Client,
    /// Buffer en memoria por documento abierto. Se popula en `did_open`,
    /// se actualiza completo en `did_change` (anunciamos `FULL` sync),
    /// y se borra en `did_close`. `parking_lot::Mutex` (no `tokio`)
    /// porque las secciones críticas son ~microsegundos: lock, get/insert,
    /// unlock — sin awaits adentro. La pipeline del checker corre fuera
    /// del lock.
    documents: Arc<Mutex<HashMap<Url, String>>>,
}

impl Backend {
    /// Corre el pipeline LSP-style sobre `text` y publica los diagnósticos
    /// resultantes (vacíos si no hay errores → VSCode borra los marcadores
    /// previos del archivo).
    async fn publish(&self, uri: Url, text: &str, version: Option<i32>) {
        let errors = check_source(text);
        let diagnostics = fitz_errors_to_diagnostics(&errors);
        self.client
            .publish_diagnostics(uri, diagnostics, version)
            .await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _params: InitializeParams) -> LspResult<InitializeResult> {
        // Capabilities: anunciamos `FULL` sync — recibimos el documento
        // entero en cada `did_change` y re-corremos el pipeline. La
        // alternativa `INCREMENTAL` requiere mantener el buffer y
        // aplicar edits, más eficiente sobre archivos grandes pero suma
        // complejidad. FULL es el default razonable para el MVP; la
        // migración es decisión de perf que dejamos para cuando aparezca
        // presión real.
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

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        let version = params.text_document.version;
        self.documents.lock().insert(uri.clone(), text.clone());
        self.publish(uri, &text, Some(version)).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        // Con `TextDocumentSyncKind::FULL`, la spec garantiza un solo
        // entry en `content_changes` y su `text` es el documento entero
        // (sin `range`). Tomamos el último por defensa contra clientes
        // que manden múltiples eventos.
        let text = params
            .content_changes
            .into_iter()
            .last()
            .map(|c| c.text)
            .unwrap_or_default();
        self.documents.lock().insert(uri.clone(), text.clone());
        self.publish(uri, &text, Some(version)).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.lock().remove(&uri);
        // Convención LSP: al cerrar, mandamos un publish con lista
        // vacía para que VSCode limpie los marcadores del archivo.
        self.client.publish_diagnostics(uri, vec![], None).await;
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
    let (service, socket) = LspService::new(|client| Backend {
        client,
        documents: Arc::new(Mutex::new(HashMap::new())),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
