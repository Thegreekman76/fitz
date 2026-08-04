//! Phase 11.13 — dev server for `fitz dev`'s wasm-client mode.
//!
//! A tiny static file server + live-reload WebSocket, bound to
//! `127.0.0.1:<port>`. It serves the project root (the manifest
//! directory) so the user's `index.html`, CSS, images, and the
//! freshly built bundle under `target/wasm/<pkg>/` all resolve the
//! same way they would under `python -m http.server` from the
//! project root — which is exactly the serve model the `fitz build`
//! success tip already documents.
//!
//! Every served `.html` (the host page) gets a small `<script>`
//! injected that opens a WebSocket to `/__fitz_dev_ws`; when the
//! dev loop finishes a rebuild it signals the connected clients and
//! the page does `location.reload()`. All responses carry
//! `Cache-Control: no-store` so the browser re-fetches the fresh
//! `.js`/`.wasm` on reload instead of serving a cached copy.
//!
//! This is Approach C of Phase 11.13 (fast incremental reload with
//! auto-refresh) — no client-side template runtime, no DOM diffing.
//! The rebuild is a `wasm-pack --dev` incremental build; the "diff"
//! is a browser reload. State preservation across reloads (reusing
//! the v0.31.0 hydration payload) is a follow-up slice.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use tokio::sync::broadcast;

/// Handle returned by [`start`]. Held by the dev loop; calling
/// [`DevServer::signal_reload`] after a successful rebuild pushes a
/// `reload` message to every connected browser.
pub struct DevServer {
    reload_tx: broadcast::Sender<()>,
    /// The URL the user points a browser at (for the banner).
    pub url: String,
}

impl DevServer {
    /// Broadcasts a reload to all connected live-reload clients.
    /// A no-op if no browser is connected yet.
    pub fn signal_reload(&self) {
        // `Err` only when there are zero receivers — harmless.
        let _ = self.reload_tx.send(());
    }
}

/// Shared state for the axum handlers.
struct AppState {
    /// Static-serving root (the manifest directory).
    root: PathBuf,
    /// Sanitised package name (for the generated fallback page).
    pkg_name: String,
    /// Root-relative URL of the built JS glue
    /// (`target/wasm/<pkg>/<pkg>.js`), for the generated fallback
    /// page's `import`.
    pkg_rel_js: String,
    /// CSS selector the component mounts into (from `[[bin]].mount`),
    /// for the generated fallback page's mount element.
    mount: String,
    reload_tx: broadcast::Sender<()>,
}

/// Binds the dev server on `127.0.0.1:<port>` and spawns it as a
/// background tokio task. Returns a [`DevServer`] handle for
/// signalling reloads, or an `Err` if the port is already in use.
///
/// - `root` — the static-serving root (manifest directory).
/// - `pkg_name` / `pkg_rel_js` / `mount` — used only to synthesise
///   a fallback host page when the project has no `index.html`.
pub async fn start(
    root: PathBuf,
    pkg_name: String,
    pkg_rel_js: String,
    mount: String,
    port: u16,
) -> Result<DevServer, String> {
    let (reload_tx, _) = broadcast::channel::<()>(16);
    let state = Arc::new(AppState {
        root,
        pkg_name,
        pkg_rel_js,
        mount,
        reload_tx: reload_tx.clone(),
    });

    let app = Router::new()
        .route("/__fitz_dev_ws", get(ws_handler))
        .fallback(static_handler)
        .with_state(state);

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("could not bind the dev server on 127.0.0.1:{port}: {e}"))?;

    tokio::spawn(async move {
        // Runs until the process exits (Ctrl+C in the dev loop).
        let _ = axum::serve(listener, app).await;
    });

    Ok(DevServer {
        reload_tx,
        url: format!("http://127.0.0.1:{port}/"),
    })
}

/// Live-reload WebSocket. Each connection subscribes to the reload
/// broadcast and forwards a `reload` text frame on every signal.
async fn ws_handler(ws: WebSocketUpgrade, State(st): State<Arc<AppState>>) -> Response {
    let rx = st.reload_tx.subscribe();
    ws.on_upgrade(move |socket| handle_ws(socket, rx))
}

async fn handle_ws(mut socket: WebSocket, mut rx: broadcast::Receiver<()>) {
    loop {
        tokio::select! {
            signal = rx.recv() => {
                match signal {
                    Ok(()) => {
                        if socket.send(Message::Text("reload".into())).await.is_err() {
                            break;
                        }
                    }
                    // Coalesce a burst of rebuilds into a single reload.
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if socket.send(Message::Text("reload".into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            // Drain inbound frames so we notice the client closing.
            inbound = socket.recv() => {
                match inbound {
                    None | Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}

/// Static file handler + host-page injection. Sandboxes to `root`
/// (rejects `..`). `/` and `/index.html` serve the host page (the
/// project `index.html` with the reload snippet injected, or a
/// generated fallback). Every response carries `Cache-Control:
/// no-store` so reloads fetch fresh assets.
async fn static_handler(State(st): State<Arc<AppState>>, uri: Uri) -> Response {
    let rel = uri.path().trim_start_matches('/');

    if rel.is_empty() || rel == "index.html" {
        return host_page(&st).await;
    }

    // Resolve under `root`, rejecting path-escape components.
    let mut full = st.root.clone();
    for seg in rel.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return not_found();
        }
        full.push(seg);
    }

    match tokio::fs::read(&full).await {
        Ok(bytes) => file_response(bytes, content_type_for(&full)),
        // A missing `/favicon.ico` is expected (browsers request it
        // unprompted). Answer 204 instead of 404 so it doesn't show
        // up as a console error in every dev session.
        Err(_) if rel == "favicon.ico" => (
            StatusCode::NO_CONTENT,
            [(header::CACHE_CONTROL, "no-store")],
        )
            .into_response(),
        Err(_) => not_found(),
    }
}

/// Serves the host page: the project's `index.html` (with the
/// reload snippet injected) if present, else a generated fallback.
async fn host_page(st: &AppState) -> Response {
    let index = st.root.join("index.html");
    let html = match tokio::fs::read_to_string(&index).await {
        Ok(user_html) => inject_reload_snippet(&user_html),
        Err(_) => generated_host_page(st),
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        html,
    )
        .into_response()
}

/// Inserts the live-reload `<script>` right before `</body>`
/// (case-insensitive), or appends it if there's no `</body>`.
fn inject_reload_snippet(html: &str) -> String {
    let snippet = reload_snippet();
    if let Some(idx) = find_ci(html, "</body>") {
        let mut out = String::with_capacity(html.len() + snippet.len());
        out.push_str(&html[..idx]);
        out.push_str(&snippet);
        out.push_str(&html[idx..]);
        out
    } else {
        format!("{html}{snippet}")
    }
}

/// The injected client: opens the live-reload WS and reloads on
/// `reload`. Reconnects if the socket drops.
fn reload_snippet() -> String {
    "\n<script>\n\
     (function(){function c(){var w=new WebSocket((location.protocol==='https:'?'wss://':'ws://')\
     +location.host+'/__fitz_dev_ws');w.onmessage=function(e){if(e.data==='reload')\
     location.reload();};w.onclose=function(){setTimeout(c,1000);};}c();})();\n\
     </script>\n"
        .to_string()
}

/// Minimal host page when the project has no `index.html`. Mounts
/// the component into an element matching the `[[bin]].mount`
/// selector (only `#id` selectors synthesise an element; others
/// fall back to a `<div>` with a note).
fn generated_host_page(st: &AppState) -> String {
    let mount_element = if let Some(id) = st.mount.strip_prefix('#') {
        format!("<div id=\"{id}\"></div>")
    } else {
        // Non-`#id` selector: we can't synthesise a matching element
        // reliably. Provide a default `<div>` and a hint.
        format!(
            "<div id=\"app\"><!-- mount selector `{}` — add an element it \
             matches, or use `#app` --></div>",
            st.mount
        )
    };
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>fitz dev — {pkg}</title>\n</head>\n<body>\n{mount}\n\
         <script type=\"module\">\n  import init from '/{js}';\n  \
         init().catch(function(e){{ console.error('WASM load failed:', e); }});\n\
         </script>\n{reload}</body>\n</html>\n",
        pkg = st.pkg_name,
        mount = mount_element,
        js = st.pkg_rel_js,
        reload = reload_snippet(),
    )
}

fn file_response(bytes: Vec<u8>, content_type: &'static str) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CACHE_CONTROL, "no-store")],
        "404 not found",
    )
        .into_response()
}

/// Content-Type by extension. `application/wasm` for `.wasm` is
/// mandatory — `wasm-pack --target web` uses
/// `WebAssembly.instantiateStreaming`, which rejects any other MIME.
fn content_type_for(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") | Some("map") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Case-insensitive substring search returning the byte index of
/// the first match (ASCII needle).
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || n.len() > h.len() {
        return None;
    }
    for i in 0..=(h.len() - n.len()) {
        if h[i..i + n.len()]
            .iter()
            .zip(n)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
        {
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_ci_matches_case_insensitively() {
        assert_eq!(find_ci("<html><BODY></Body></html>", "</body>"), Some(12));
        assert_eq!(find_ci("no closing tag here", "</body>"), None);
    }

    #[test]
    fn inject_snippet_before_body_close() {
        let out = inject_reload_snippet("<html><body>hi</body></html>");
        // Snippet lands before the (lowercase) </body>.
        let script_at = out.find("__fitz_dev_ws").unwrap();
        let body_close = out.rfind("</body>").unwrap();
        assert!(script_at < body_close);
        assert!(out.contains("hi"));
    }

    #[test]
    fn inject_snippet_appends_when_no_body() {
        let out = inject_reload_snippet("<div>bare fragment</div>");
        assert!(out.starts_with("<div>bare fragment</div>"));
        assert!(out.contains("__fitz_dev_ws"));
    }

    #[test]
    fn generated_page_uses_id_selector_element() {
        let st = AppState {
            root: PathBuf::from("."),
            pkg_name: "demo".into(),
            pkg_rel_js: "target/wasm/demo/demo.js".into(),
            mount: "#app".into(),
            reload_tx: broadcast::channel(1).0,
        };
        let html = generated_host_page(&st);
        assert!(html.contains("<div id=\"app\"></div>"));
        assert!(html.contains("import init from '/target/wasm/demo/demo.js'"));
        assert!(html.contains("__fitz_dev_ws"));
    }

    #[test]
    fn content_type_wasm_is_application_wasm() {
        assert_eq!(
            content_type_for(std::path::Path::new("x/demo_bg.wasm")),
            "application/wasm"
        );
        assert_eq!(
            content_type_for(std::path::Path::new("x/demo.js")),
            "text/javascript; charset=utf-8"
        );
    }
}
