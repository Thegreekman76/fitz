// tests/lsp_e2e.rs — Smoke E2E del bin `fitz-lsp` (Fase 9.x.1+).
//
// Spawnea el binario, le manda mensajes LSP por stdio, valida las
// respuestas y notifications, y chequea exit code 0. Cubre:
// - 9.x.1.a: handshake initialize/initialized/shutdown.
// - 9.x.1.b: did_open con un buffer roto → notification
//   `textDocument/publishDiagnostics` con el error mapeado.
// - 9.x.2.b: did_open con programa válido + textDocument/hover →
//   response con el tipo del nodo bajo el cursor en markdown.
//
// Requiere `--features lsp` (el bin `fitz-lsp` tiene
// `required-features = ["lsp"]` en Cargo.toml). Sin la feature, el
// archivo entero queda fuera del build de tests via `#![cfg(...)]`.
//
// Sin dependencias extras: frames JSON-RPC construidos a mano.

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
    wait_for_clean_exit(&mut child);
}

/// Espera el exit del proceso con timeout. Mata si excede.
fn wait_for_clean_exit(child: &mut std::process::Child) {
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

#[test]
fn did_open_documento_roto_publica_diagnostic_con_error_de_tipo() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_fitz-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn de fitz-lsp");

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = child.stdout.take().expect("stdout");

    // Handshake mínimo: initialize + initialized.
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"rootUri":null,"processId":null}}"#;
    stdin.write_all(&frame(init)).expect("write initialize");
    stdin.flush().expect("flush initialize");
    let _init_resp = read_message(&mut stdout); // descartamos, validado en el otro test

    let initialized = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
    stdin
        .write_all(&frame(initialized))
        .expect("write initialized");
    stdin.flush().expect("flush initialized");

    // didOpen con un programa que el checker rechaza:
    //   `let x: Int = "texto"` → TypeError "no es asignable a Int".
    // El URI no necesita ser un path válido en disco — el LSP trabaja
    // sobre el `text` que viene en el evento. JSON en una sola línea
    // para no pelearle el `Content-Length` con whitespace.
    let did_open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///test.fitz","languageId":"fitz","version":1,"text":"let x: Int = \"texto\"\n"}}}"#;
    stdin.write_all(&frame(did_open)).expect("write didOpen");
    stdin.flush().expect("flush didOpen");

    // Esperamos la notification `textDocument/publishDiagnostics`.
    // Pueden venir antes notifications irrelevantes (logMessage del
    // initialized, etc.) — iteramos hasta encontrarla.
    let deadline = Instant::now() + Duration::from_secs(5);
    let publish = loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("timeout esperando publishDiagnostics");
        }
        let msg = read_message(&mut stdout);
        if msg.contains(r#""method":"textDocument/publishDiagnostics""#)
            || msg.contains(r#""method": "textDocument/publishDiagnostics""#)
        {
            break msg;
        }
    };

    // Validaciones sobre el JSON crudo (sin parseo formal — keepealo
    // sin deps extras). Buscamos:
    // - el URI del documento que abrimos
    // - al menos un diagnostic
    // - severity = 1 (ERROR)
    // - source "fitz"
    // - referencia al tipo `Int` en el message
    assert!(
        publish.contains(r#""uri""#) && publish.contains("test.fitz"),
        "publishDiagnostics sin URI esperado: {publish}",
    );
    assert!(
        publish.contains(r#""diagnostics""#),
        "publishDiagnostics sin campo diagnostics: {publish}",
    );
    assert!(
        publish.contains(r#""severity":1"#) || publish.contains(r#""severity": 1"#),
        "publishDiagnostics sin severity ERROR: {publish}",
    );
    assert!(
        publish.contains(r#""source":"fitz""#) || publish.contains(r#""source": "fitz""#),
        "publishDiagnostics sin source fitz: {publish}",
    );
    assert!(
        publish.contains("Int"),
        "publishDiagnostics sin referencia al tipo Int: {publish}",
    );
    // Los diagnostics no deben venir vacíos: con `let x: Int = "texto"`
    // tiene que haber al menos un error.
    assert!(
        !publish.contains(r#""diagnostics":[]"#),
        "publishDiagnostics con lista vacía: {publish}",
    );

    // Cerramos prolijo: shutdown + cierre de stdin.
    let shutdown = r#"{"jsonrpc":"2.0","id":99,"method":"shutdown"}"#;
    stdin.write_all(&frame(shutdown)).expect("write shutdown");
    stdin.flush().expect("flush shutdown");
    // No esperamos la respuesta — solo cerramos stdin para que el
    // server salga del loop.
    drop(stdin);
    wait_for_clean_exit(&mut child);
}

#[test]
fn hover_sobre_literal_int_devuelve_tipo_en_markdown() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_fitz-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn de fitz-lsp");

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = child.stdout.take().expect("stdout");

    // Handshake.
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"rootUri":null,"processId":null}}"#;
    stdin.write_all(&frame(init)).expect("write initialize");
    stdin.flush().expect("flush initialize");
    let init_resp = read_message(&mut stdout);
    // Sanity: la capability hover_provider se anuncia.
    assert!(
        init_resp.contains(r#""hoverProvider":true"#),
        "initialize sin hoverProvider: {init_resp}",
    );

    let initialized = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
    stdin
        .write_all(&frame(initialized))
        .expect("write initialized");
    stdin.flush().expect("flush initialized");

    // didOpen con `let x = 42`. El literal `42` empieza en col 9
    // (1-based) = col 8 (0-based). Sin diagnostics esperados.
    let did_open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///hover.fitz","languageId":"fitz","version":1,"text":"let x = 42\n"}}}"#;
    stdin.write_all(&frame(did_open)).expect("write didOpen");
    stdin.flush().expect("flush didOpen");

    // Drenamos la notification de publishDiagnostics (no la usamos
    // acá pero llega siempre tras didOpen). Iteramos hasta verla.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("timeout esperando publishDiagnostics post didOpen");
        }
        let msg = read_message(&mut stdout);
        if msg.contains("textDocument/publishDiagnostics") {
            break;
        }
    }

    // Mandamos hover sobre la posición del literal `42`.
    let hover_req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///hover.fitz"},"position":{"line":0,"character":8}}}"#;
    stdin.write_all(&frame(hover_req)).expect("write hover");
    stdin.flush().expect("flush hover");

    // Esperamos la response con id 2 (puede venir otras notifications
    // intermedias).
    let deadline = Instant::now() + Duration::from_secs(5);
    let hover_resp = loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("timeout esperando hover response");
        }
        let msg = read_message(&mut stdout);
        if msg.contains(r#""id":2"#) {
            break msg;
        }
    };

    // Validamos: result.contents.kind == "markdown" y value contiene "Int".
    assert!(
        hover_resp.contains(r#""kind":"markdown""#)
            || hover_resp.contains(r#""kind": "markdown""#),
        "hover sin MarkupContent markdown: {hover_resp}",
    );
    assert!(
        hover_resp.contains("```fitz") && hover_resp.contains("Int"),
        "hover sin bloque fitz con tipo Int: {hover_resp}",
    );
    assert!(
        !hover_resp.contains(r#""error""#),
        "hover devolvió error: {hover_resp}",
    );

    // Hover en una posición sin spans (línea 2 — fuera del documento)
    // debe devolver `result: null`.
    let hover_empty = r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///hover.fitz"},"position":{"line":5,"character":0}}}"#;
    stdin.write_all(&frame(hover_empty)).expect("write hover2");
    stdin.flush().expect("flush hover2");

    let deadline = Instant::now() + Duration::from_secs(5);
    let null_resp = loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("timeout esperando hover null response");
        }
        let msg = read_message(&mut stdout);
        if msg.contains(r#""id":3"#) {
            break msg;
        }
    };
    assert!(
        null_resp.contains(r#""result":null"#)
            || null_resp.contains(r#""result": null"#),
        "hover en posición sin spans debería ser null: {null_resp}",
    );

    drop(stdin);
    wait_for_clean_exit(&mut child);
}
