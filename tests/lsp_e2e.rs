// tests/lsp_e2e.rs — Smoke E2E del bin `fitz-lsp` (Fase 9.x.1.a).
//
// Spawnea el binario, le manda un handshake LSP completo por stdio
// (initialize → initialized → shutdown), valida las respuestas y
// chequea exit code 0.
//
// Requiere `--features lsp` (el bin `fitz-lsp` tiene
// `required-features = ["lsp"]` en Cargo.toml). Sin la feature, el
// archivo entero queda fuera del build de tests via `#![cfg(...)]`.
//
// Sin dependencias extras: frames JSON-RPC construidos a mano. Para
// 9.x.1.a alcanza con verificar que el handshake responde con el
// `serverInfo` esperado y los códigos de error son `null` (éxito);
// los tests ricos sobre `did_open`/`did_change` y diagnostics
// llegan en 9.x.1.b.

#![cfg(feature = "lsp")]

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Construye un frame LSP (Content-Length header + body JSON).
fn frame(body: &str) -> Vec<u8> {
    let mut buf = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    buf.extend_from_slice(body.as_bytes());
    buf
}

/// Lee un mensaje LSP del stream: header `Content-Length: N`, línea
/// vacía, exactamente N bytes de body. Devuelve el body como String.
fn read_message<R: Read>(stream: &mut R) -> String {
    // Leemos byte a byte hasta el separador `\r\n\r\n` (los headers
    // son ASCII y el body sigue inmediatamente). No es performante
    // pero alcanza para tests.
    let mut headers = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).expect("EOF leyendo headers");
        headers.push(byte[0]);
        if headers.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let header_str = std::str::from_utf8(&headers).expect("headers UTF-8");
    let n: usize = header_str
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length:"))
        .map(|v| v.trim().parse().expect("Content-Length numérico"))
        .expect("header Content-Length");
    let mut body = vec![0u8; n];
    stream.read_exact(&mut body).expect("EOF leyendo body");
    String::from_utf8(body).expect("body UTF-8")
}

#[test]
fn handshake_initialize_shutdown_devuelve_server_info_correcto() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_fitz-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn de fitz-lsp");

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = child.stdout.take().expect("stdout");

    // 1. initialize → esperamos respuesta con `serverInfo`.
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"rootUri":null,"processId":null}}"#;
    stdin.write_all(&frame(init)).expect("write initialize");
    stdin.flush().expect("flush initialize");

    let init_resp = read_message(&mut stdout);
    assert!(
        init_resp.contains(r#""id":1"#),
        "initialize response sin id 1: {init_resp}",
    );
    assert!(
        init_resp.contains(r#""name":"fitz-lsp""#),
        "initialize response sin serverInfo.name: {init_resp}",
    );
    assert!(
        init_resp.contains(r#""textDocumentSync":1"#),
        "initialize response sin textDocumentSync FULL: {init_resp}",
    );
    assert!(
        !init_resp.contains(r#""error""#),
        "initialize devolvió error: {init_resp}",
    );

    // 2. initialized notification (sin id, sin response esperada).
    let initialized = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
    stdin
        .write_all(&frame(initialized))
        .expect("write initialized");
    stdin.flush().expect("flush initialized");

    // 3. shutdown → esperamos respuesta. Pero entre medio puede
    // venir la notification `window/logMessage` que dispara nuestro
    // `initialized()` con `client.log_message(...)`. Iteramos hasta
    // encontrar el mensaje con `"id":2`.
    let shutdown = r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#;
    stdin.write_all(&frame(shutdown)).expect("write shutdown");
    stdin.flush().expect("flush shutdown");

    let deadline = Instant::now() + Duration::from_secs(5);
    let shutdown_resp = loop {
        if Instant::now() > deadline {
            panic!("timeout esperando shutdown response");
        }
        let msg = read_message(&mut stdout);
        if msg.contains(r#""id":2"#) {
            break msg;
        }
        // Otra cosa (típicamente `window/logMessage` del initialized
        // handler) — la ignoramos y seguimos leyendo.
    };

    assert!(
        shutdown_resp.contains(r#""result":null"#),
        "shutdown response sin result null: {shutdown_resp}",
    );
    assert!(
        !shutdown_resp.contains(r#""error""#),
        "shutdown devolvió error: {shutdown_resp}",
    );

    // 4. Cerramos stdin para que tower-lsp termine el loop. No
    // mandamos `exit` notification — pipelinearla con shutdown puede
    // disparar cancellation de requests pendientes (visto en pruebas
    // manuales contra tower-lsp 0.20).
    drop(stdin);

    // Esperamos exit con timeout corto.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            assert!(status.success(), "exit no exitoso: {status:?}");
            return;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("fitz-lsp no terminó en 5s tras cerrar stdin");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
