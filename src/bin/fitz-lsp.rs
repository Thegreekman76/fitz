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
    GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, InitializedParams, MessageType, OneOf, ServerCapabilities,
    ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind, Url,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

use fitz::lsp::{
    check_source_with_types, definition_for_position, fitz_errors_to_diagnostics,
    hover_for_position, make_definition_location, make_hover,
};
use fitz::types::{DefinitionInfo, TypeEnv, TypeInfo};

/// Estado por documento abierto. Persiste el último texto recibido por
/// `did_open`/`did_change` y los side-tables resultantes del último
/// chequeo: `TypeEnv` (resuelve nombres de tipos nominales para hover),
/// `TypeInfo` (tipo por nodo — hover, Fase 9.x.2), `DefinitionInfo`
/// (uso → declaración — go-to-definition, Fase 9.x.3).
//
// `#[allow(dead_code)]` puntual sobre `text` — lo persistimos para
// consumidores futuros (autocomplete probablemente lo necesite para
// mapear cursor → token preciso), pero hover y definition no lo leen.
#[derive(Debug)]
struct DocumentState {
    #[allow(dead_code)]
    text: String,
    type_env: TypeEnv,
    type_info: TypeInfo,
    def_info: DefinitionInfo,
}

#[derive(Debug)]
struct Backend {
    client: Client,
    /// Estado por documento abierto. Se popula en `did_open`, se
    /// actualiza completo en `did_change` (anunciamos `FULL` sync), y
    /// se borra en `did_close`. `parking_lot::Mutex` (no `tokio`)
    /// porque las secciones críticas son ~microsegundos: lock,
    /// get/insert, unlock — sin awaits adentro. La pipeline del checker
    /// corre fuera del lock.
    documents: Arc<Mutex<HashMap<Url, DocumentState>>>,
}

impl Backend {
    /// Corre el pipeline LSP-style sobre `text`, persiste el resultado
    /// en `documents`, y publica los diagnósticos. Devuelve nada — la
    /// notificación es fire-and-forget.
    async fn check_and_publish(&self, uri: Url, text: String, version: Option<i32>) {
        let (type_env, type_info, def_info, errors) = check_source_with_types(&text);
        let diagnostics = fitz_errors_to_diagnostics(&errors);
        self.documents.lock().insert(
            uri.clone(),
            DocumentState {
                text,
                type_env,
                type_info,
                def_info,
            },
        );
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
                // Fase 9.x.2 — anunciamos que respondemos
                // `textDocument/hover` con el tipo del nodo bajo el
                // cursor. La heurística de lookup y el formato de
                // respuesta viven en `fitz::lsp`.
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                // Fase 9.x.3 — anunciamos que respondemos
                // `textDocument/definition`. Devolvemos un solo
                // `Location` (no multi-definición — Fitz no tiene
                // overloading), por eso `OneOf::Left(true)` (forma
                // simple) en lugar de `DefinitionOptions`.
                definition_provider: Some(OneOf::Left(true)),
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
        self.check_and_publish(uri, text, Some(version)).await;
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
        self.check_and_publish(uri, text, Some(version)).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.lock().remove(&uri);
        // Convención LSP: al cerrar, mandamos un publish con lista
        // vacía para que VSCode limpie los marcadores del archivo.
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn hover(&self, params: HoverParams) -> LspResult<Option<Hover>> {
        // Resolvemos todo bajo el lock — clonamos lo mínimo (un `Type`
        // y formateamos contra el env) para soltarlo antes de devolver.
        // El env no se clona; el `make_hover` corre adentro del lock
        // porque solo lee y serializa a string. Sin awaits → sin
        // deadlock risk.
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let documents = self.documents.lock();
        let state = match documents.get(&uri) {
            Some(s) => s,
            None => return Ok(None),
        };
        let hover = hover_for_position(&state.type_info, pos.line, pos.character)
            .map(|ty| make_hover(ty, &state.type_env));
        Ok(hover)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> LspResult<Option<GotoDefinitionResponse>> {
        // Resolvemos `(uri, pos) → def_span` bajo el lock, construimos
        // el `Location` con el URI del documento abierto (cross-module
        // def queda como deuda visible — `from foo import X` apunta al
        // span del Stmt::Import local, no al módulo remoto). Sin
        // awaits adentro del lock.
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let documents = self.documents.lock();
        let state = match documents.get(&uri) {
            Some(s) => s,
            None => return Ok(None),
        };
        let location = definition_for_position(&state.def_info, pos.line, pos.character)
            .map(|def_span| make_definition_location(uri.clone(), def_span))
            .map(GotoDefinitionResponse::Scalar);
        Ok(location)
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
