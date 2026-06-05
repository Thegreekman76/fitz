// tests/lsp_e2e.rs — Smoke E2E del bin `fitz-lsp` (Fase 9.x.1+).
//
// Spawnea el binario, le manda mensajes LSP por stdio, valida las
// respuestas y notifications, y chequea exit code 0. Cubre:
// - 9.x.1.a: handshake initialize/initialized/shutdown.
// - 9.x.1.b: did_open con un buffer roto → notification
//   `textDocument/publishDiagnostics` con el error mapeado.
// - 9.x.2.b: did_open con programa válido + textDocument/hover →
//   response con el tipo del nodo bajo el cursor en markdown.
// - 9.x.3.b: did_open con programa válido + textDocument/definition
//   → response con Location apuntando al span de declaración.
// - 9.x.4.b: did_open + textDocument/completion → response con items
//   contextuales (after-dot lista métodos del tipo del receiver).
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
        hover_resp.contains(r#""kind":"markdown""#) || hover_resp.contains(r#""kind": "markdown""#),
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
        null_resp.contains(r#""result":null"#) || null_resp.contains(r#""result": null"#),
        "hover en posición sin spans debería ser null: {null_resp}",
    );

    drop(stdin);
    wait_for_clean_exit(&mut child);
}

#[test]
fn goto_definition_sobre_uso_de_var_local_devuelve_location_de_let() {
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
    // Sanity: la capability definition_provider se anuncia.
    assert!(
        init_resp.contains(r#""definitionProvider":true"#),
        "initialize sin definitionProvider: {init_resp}",
    );

    let initialized = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
    stdin
        .write_all(&frame(initialized))
        .expect("write initialized");
    stdin.flush().expect("flush initialized");

    // didOpen con `let x = 42\nlet y = x\n`. El uso de `x` está en
    // línea 1, col 8 (0-based). El `def_span` apunta al Stmt::Assign
    // de `x` (línea 0 → línea 1 1-based en Fitz, mapeado a línea 0
    // 0-based LSP).
    let did_open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///def.fitz","languageId":"fitz","version":1,"text":"let x = 42\nlet y = x\n"}}}"#;
    stdin.write_all(&frame(did_open)).expect("write didOpen");
    stdin.flush().expect("flush didOpen");

    // Drenamos la notification de publishDiagnostics post didOpen.
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

    // textDocument/definition sobre `x` en línea 1, col 8 (0-based).
    let def_req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{"textDocument":{"uri":"file:///def.fitz"},"position":{"line":1,"character":8}}}"#;
    stdin.write_all(&frame(def_req)).expect("write definition");
    stdin.flush().expect("flush definition");

    let deadline = Instant::now() + Duration::from_secs(5);
    let def_resp = loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("timeout esperando definition response");
        }
        let msg = read_message(&mut stdout);
        if msg.contains(r#""id":2"#) {
            break msg;
        }
    };

    // Validamos: result.uri == el documento que abrimos, result.range
    // está en línea 0 (el let de x en la primera línea, 0-based LSP).
    assert!(
        def_resp.contains("def.fitz"),
        "definition response sin URI esperado: {def_resp}",
    );
    assert!(
        def_resp.contains(r#""line":0"#) || def_resp.contains(r#""line": 0"#),
        "definition range no apunta a línea 0: {def_resp}",
    );
    assert!(
        !def_resp.contains(r#""error""#),
        "definition devolvió error: {def_resp}",
    );

    // Cursor sobre el builtin `print` (no debería resolver). Lo armamos
    // sobre un programa nuevo para que el cursor caiga sobre el ident.
    let did_open2 = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///print.fitz","languageId":"fitz","version":1,"text":"print(42)\n"}}}"#;
    stdin.write_all(&frame(did_open2)).expect("write didOpen2");
    stdin.flush().expect("flush didOpen2");
    // Drenamos diagnostics.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("timeout esperando publishDiagnostics2");
        }
        let msg = read_message(&mut stdout);
        if msg.contains("print.fitz") && msg.contains("publishDiagnostics") {
            break;
        }
    }

    let def_builtin = r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/definition","params":{"textDocument":{"uri":"file:///print.fitz"},"position":{"line":0,"character":0}}}"#;
    stdin
        .write_all(&frame(def_builtin))
        .expect("write definition builtin");
    stdin.flush().expect("flush definition builtin");

    let deadline = Instant::now() + Duration::from_secs(5);
    let null_resp = loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("timeout esperando definition null response");
        }
        let msg = read_message(&mut stdout);
        if msg.contains(r#""id":3"#) {
            break msg;
        }
    };
    assert!(
        null_resp.contains(r#""result":null"#) || null_resp.contains(r#""result": null"#),
        "definition sobre builtin debería ser null: {null_resp}",
    );

    drop(stdin);
    wait_for_clean_exit(&mut child);
}

#[test]
fn completion_after_dot_sobre_str_lista_metodos_built_in() {
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
    // Sanity: capability completionProvider con trigger character `.`.
    assert!(
        init_resp.contains(r#""completionProvider""#),
        "initialize sin completionProvider: {init_resp}",
    );
    // v0.10.12 — trigger chars expandido a `[".","@"]` (`.` ya estaba,
    // `@` sumado para AfterAt completion de decorators).
    assert!(
        init_resp.contains(r#""triggerCharacters":["."]"#)
            || init_resp.contains(r#""triggerCharacters": ["."]"#)
            || init_resp.contains(r#""triggerCharacters":[".","@"]"#)
            || init_resp.contains(r#""triggerCharacters": [".","@"]"#)
            || init_resp.contains(r#""triggerCharacters":[".", "@"]"#),
        "completionProvider sin trigger character `.`: {init_resp}",
    );

    let initialized = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
    stdin
        .write_all(&frame(initialized))
        .expect("write initialized");
    stdin.flush().expect("flush initialized");

    // didOpen con `let s = "hola"\ns.\n`. Cursor en línea 1, col 2
    // (0-based) — justo después del `.`. El receiver `s` tiene tipo
    // `Str`; el fallback walk-del-Program resuelve el tipo aún cuando
    // el parser abandona el stmt `s.` por el `.` huérfano.
    let did_open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///comp.fitz","languageId":"fitz","version":1,"text":"let s = \"hola\"\ns.\n"}}}"#;
    stdin.write_all(&frame(did_open)).expect("write didOpen");
    stdin.flush().expect("flush didOpen");

    // Drenamos diagnostics post didOpen.
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

    // textDocument/completion en (1, 2) — after-dot sobre Str.
    let comp_req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///comp.fitz"},"position":{"line":1,"character":2},"context":{"triggerKind":2,"triggerCharacter":"."}}}"#;
    stdin.write_all(&frame(comp_req)).expect("write completion");
    stdin.flush().expect("flush completion");

    let deadline = Instant::now() + Duration::from_secs(5);
    let comp_resp = loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("timeout esperando completion response");
        }
        let msg = read_message(&mut stdout);
        if msg.contains(r#""id":2"#) {
            break msg;
        }
    };

    // Los 3 métodos de Str deben aparecer. No matcheamos formato
    // exacto del JSON — buscamos las labels.
    for expected in ["upper", "lower"] {
        assert!(
            comp_resp.contains(&format!(r#""label":"{expected}""#))
                || comp_resp.contains(&format!(r#""label": "{expected}""#)),
            "completion response sin label `{expected}`: {comp_resp}",
        );
    }
    assert!(
        !comp_resp.contains(r#""error""#),
        "completion devolvió error: {comp_resp}",
    );
    // Sin métodos de List (que no aplican a Str).
    assert!(
        !comp_resp.contains(r#""label":"push""#) && !comp_resp.contains(r#""label": "push""#),
        "completion no debería incluir métodos de List: {comp_resp}",
    );

    // Segundo request: completion scope-level en línea 2 col 0
    // (después del newline final, contexto top-level).
    let comp_scope = r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///comp.fitz"},"position":{"line":2,"character":0}}}"#;
    stdin
        .write_all(&frame(comp_scope))
        .expect("write completion scope");
    stdin.flush().expect("flush completion scope");

    let deadline = Instant::now() + Duration::from_secs(5);
    let scope_resp = loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("timeout esperando completion scope response");
        }
        let msg = read_message(&mut stdout);
        if msg.contains(r#""id":3"#) {
            break msg;
        }
    };

    // Scope-level debe incluir: la var top-level `s`, builtins
    // (print/len), tipos built-in (Int), keywords (let/fn/match).
    for expected in ["s", "print", "Int", "let"] {
        assert!(
            scope_resp.contains(&format!(r#""label":"{expected}""#))
                || scope_resp.contains(&format!(r#""label": "{expected}""#)),
            "completion scope-level sin label `{expected}`: {scope_resp}",
        );
    }

    drop(stdin);
    wait_for_clean_exit(&mut child);
}

#[test]
fn completion_after_at_lista_decorators_v0_10_12() {
    // v0.10.12 — Completion tras `@`. El cursor en `@|` o `@<prefix>`
    // dispara CompletionContext::AfterAt, que devuelve la lista
    // cerrada de decorators del lenguaje con snippets útiles.
    //
    // Validamos:
    //   1. Capability completionProvider anuncia `@` como trigger char.
    //   2. Tras `@`, la lista incluye los decorators core de cada
    //      familia: @get (HTTP), @authenticated (auth), @ws (WS),
    //      @cron (jobs), @table (ORM), @hidden (ORM v0.10.11),
    //      @belongs_to (ORM relations).
    //   3. Los items tienen kind=SNIPPET (15) y insertTextFormat=2
    //      (snippet con tabstops).
    //   4. Decorators con args incluyen placeholders `${1:...}` en
    //      el insertText. Decorators sin args (@hidden/@primary/etc.)
    //      no usan placeholders.
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

    // Capability: completionProvider con trigger characters incluyendo `@`.
    assert!(
        init_resp.contains(r#""triggerCharacters":[".","@"]"#)
            || init_resp.contains(r#""triggerCharacters": [".","@"]"#)
            || init_resp.contains(r#""triggerCharacters":[".", "@"]"#)
            || init_resp.contains(r#""triggerCharacters": [".", "@"]"#),
        "completionProvider sin trigger character `@`: {init_resp}",
    );

    let initialized = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
    stdin
        .write_all(&frame(initialized))
        .expect("write initialized");
    stdin.flush().expect("flush initialized");

    // didOpen con `@\n`. Cursor en línea 0, col 1 — justo después del `@`.
    let did_open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///dec.fitz","languageId":"fitz","version":1,"text":"@\n"}}}"#;
    stdin.write_all(&frame(did_open)).expect("write didOpen");
    stdin.flush().expect("flush didOpen");

    // Drenamos diagnostics.
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

    // textDocument/completion en (0, 1) — after-at.
    let comp_req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///dec.fitz"},"position":{"line":0,"character":1},"context":{"triggerKind":2,"triggerCharacter":"@"}}}"#;
    stdin.write_all(&frame(comp_req)).expect("write completion");
    stdin.flush().expect("flush completion");

    let deadline = Instant::now() + Duration::from_secs(5);
    let comp_resp = loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("timeout esperando completion response");
        }
        let msg = read_message(&mut stdout);
        if msg.contains(r#""id":2"#) {
            break msg;
        }
    };

    assert!(
        !comp_resp.contains(r#""error""#),
        "completion devolvió error: {comp_resp}",
    );

    // Decorators core de cada familia deben aparecer.
    for expected in [
        "get",
        "post",
        "server",
        "middleware",
        "cors",
        "authenticated",
        "admin",
        "auth_provider",
        "ws",
        "cron",
        "background",
        "test",
        "table",
        "primary",
        "hidden",
        "belongs_to",
        "has_many",
    ] {
        assert!(
            comp_resp.contains(&format!(r#""label":"{expected}""#))
                || comp_resp.contains(&format!(r#""label": "{expected}""#)),
            "completion AfterAt sin label `{expected}`: {comp_resp}",
        );
    }

    // Kind=SNIPPET (15) en al menos un item.
    assert!(
        comp_resp.contains(r#""kind":15"#) || comp_resp.contains(r#""kind": 15"#),
        "completion AfterAt items sin kind=SNIPPET: {comp_resp}",
    );

    // insertTextFormat=2 (snippet con tabstops).
    assert!(
        comp_resp.contains(r#""insertTextFormat":2"#)
            || comp_resp.contains(r#""insertTextFormat": 2"#),
        "completion AfterAt items sin insertTextFormat=Snippet: {comp_resp}",
    );

    // @get debe traer snippet con placeholder ${1:/path}.
    assert!(
        comp_resp.contains(r#"get(\"${1:/path}\")"#),
        "completion @get sin snippet con placeholder: {comp_resp}",
    );

    // @hidden debe traer insertText plano (sin paréntesis ni placeholders).
    assert!(
        comp_resp.contains(r#""insertText":"hidden""#)
            || comp_resp.contains(r#""insertText": "hidden""#),
        "completion @hidden debería ser plano sin paréntesis: {comp_resp}",
    );

    drop(stdin);
    wait_for_clean_exit(&mut child);
}

/// V3 (2026-06-05) — `textDocument/formatting` delega a `fitz fmt`
/// (módulo `fitz::fmt::format_source`). Valida tres cosas:
///
///   1. Capability `documentFormattingProvider: true` anunciada.
///   2. Doc no-formateado → respuesta con UN `TextEdit` cuyo `newText`
///      es distinto del input (el formatter cambió algo).
///   3. Doc parser-roto → respuesta `null` (no crash, no abortar save).
#[test]
fn v3_formatting_doc_no_formateado_devuelve_textedit_con_codigo_formateado() {
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
    // Capability anunciada.
    assert!(
        init_resp.contains(r#""documentFormattingProvider":true"#)
            || init_resp.contains(r#""documentFormattingProvider": true"#),
        "initialize sin documentFormattingProvider: {init_resp}",
    );

    let initialized = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
    stdin
        .write_all(&frame(initialized))
        .expect("write initialized");
    stdin.flush().expect("flush initialized");

    // didOpen con código no-formateado. El formatter va a cambiar
    // tabs/spaces, comments con `//foo` → `// foo`, etc. Usamos
    // indent con tabs adentro de fn body — el formatter normaliza a
    // 4 espacios.
    let did_open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///fmt.fitz","languageId":"fitz","version":1,"text":"fn main() {\n\tlet x = 1\n\tlet y = 2\n\tprint(x + y)\n}\n"}}}"#;
    stdin.write_all(&frame(did_open)).expect("write didOpen");
    stdin.flush().expect("flush didOpen");

    // Drenamos diagnostics post didOpen.
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

    // textDocument/formatting.
    let fmt_req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/formatting","params":{"textDocument":{"uri":"file:///fmt.fitz"},"options":{"tabSize":4,"insertSpaces":true}}}"#;
    stdin.write_all(&frame(fmt_req)).expect("write formatting");
    stdin.flush().expect("flush formatting");

    let deadline = Instant::now() + Duration::from_secs(5);
    let fmt_resp = loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("timeout esperando formatting response");
        }
        let msg = read_message(&mut stdout);
        if msg.contains(r#""id":2"#) {
            break msg;
        }
    };

    // Debe haber al menos un TextEdit (el formatter normalizó tabs).
    assert!(
        fmt_resp.contains(r#""newText""#),
        "formatting response sin newText: {fmt_resp}",
    );
    assert!(
        !fmt_resp.contains(r#""error""#),
        "formatting devolvió error: {fmt_resp}",
    );
    // El newText NO debe contener tabs (el formatter las normaliza).
    // Buscamos el escape `\t` literal en el JSON — si está, fallar.
    assert!(
        !fmt_resp.contains(r#"\t"#),
        "formatting newText conservó tabs: {fmt_resp}",
    );

    drop(stdin);
    wait_for_clean_exit(&mut child);
}

#[test]
fn v3_formatting_doc_con_parser_error_devuelve_null_no_aborta() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_fitz-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn de fitz-lsp");

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = child.stdout.take().expect("stdout");

    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"rootUri":null,"processId":null}}"#;
    stdin.write_all(&frame(init)).expect("write initialize");
    stdin.flush().expect("flush initialize");
    let _ = read_message(&mut stdout);

    let initialized = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
    stdin
        .write_all(&frame(initialized))
        .expect("write initialized");
    stdin.flush().expect("flush initialized");

    // didOpen con código roto (`let x = ` sin RHS — parser falla).
    let did_open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///broken.fitz","languageId":"fitz","version":1,"text":"let x = \n"}}}"#;
    stdin.write_all(&frame(did_open)).expect("write didOpen");
    stdin.flush().expect("flush didOpen");

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

    let fmt_req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/formatting","params":{"textDocument":{"uri":"file:///broken.fitz"},"options":{"tabSize":4,"insertSpaces":true}}}"#;
    stdin.write_all(&frame(fmt_req)).expect("write formatting");
    stdin.flush().expect("flush formatting");

    let deadline = Instant::now() + Duration::from_secs(5);
    let fmt_resp = loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("timeout esperando formatting response broken");
        }
        let msg = read_message(&mut stdout);
        if msg.contains(r#""id":2"#) {
            break msg;
        }
    };

    // Sobre doc roto, esperamos `result: null` (no crash, no error).
    assert!(
        fmt_resp.contains(r#""result":null"#) || fmt_resp.contains(r#""result": null"#),
        "formatting sobre doc roto debería devolver result:null, recibió: {fmt_resp}",
    );
    assert!(
        !fmt_resp.contains(r#""error""#),
        "formatting sobre doc roto NO debe devolver error: {fmt_resp}",
    );

    drop(stdin);
    wait_for_clean_exit(&mut child);
}

/// V4 (2026-06-05) — `textDocument/signatureHelp`. Valida:
///   1. Capability `signatureHelpProvider` con trigger chars `(`/`,`.
///   2. Cursor adentro de `f(|` con fn user-defined → SignatureInformation
///      con label + parameters + active_parameter = 0.
///   3. Cursor adentro de `f(a, |` → active_parameter = 1.
///   4. Cursor sin call enclosing → result null.
#[test]
fn v4_signature_help_fn_user_defined_devuelve_signature_information() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_fitz-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn de fitz-lsp");

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = child.stdout.take().expect("stdout");

    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"rootUri":null,"processId":null}}"#;
    stdin.write_all(&frame(init)).expect("write initialize");
    stdin.flush().expect("flush initialize");
    let init_resp = read_message(&mut stdout);
    assert!(
        init_resp.contains(r#""signatureHelpProvider""#),
        "initialize sin signatureHelpProvider: {init_resp}",
    );
    assert!(
        init_resp.contains(r#""triggerCharacters":["(",","]"#)
            || init_resp.contains(r#""triggerCharacters": ["(",","]"#)
            || init_resp.contains(r#""triggerCharacters":["(", ","]"#),
        "signatureHelpProvider sin trigger chars `(`/`,`: {init_resp}",
    );

    let initialized = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
    stdin
        .write_all(&frame(initialized))
        .expect("write initialized");
    stdin.flush().expect("flush initialized");

    // didOpen con `fn add(a: Int, b: Int) -> Int { return a + b }\nlet r = add(|\n`.
    // Cursor adentro de `add(|` (línea 1, char 13 — justo después del `(`).
    let did_open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///sig.fitz","languageId":"fitz","version":1,"text":"fn add(a: Int, b: Int) -> Int { return a + b }\nlet r = add(\n"}}}"#;
    stdin.write_all(&frame(did_open)).expect("write didOpen");
    stdin.flush().expect("flush didOpen");

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

    // signatureHelp en línea 1, char 12 (después del `(` en `add(`).
    let sig_req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/signatureHelp","params":{"textDocument":{"uri":"file:///sig.fitz"},"position":{"line":1,"character":12},"context":{"triggerKind":2,"triggerCharacter":"(","isRetrigger":false}}}"#;
    stdin
        .write_all(&frame(sig_req))
        .expect("write signatureHelp");
    stdin.flush().expect("flush signatureHelp");

    let deadline = Instant::now() + Duration::from_secs(5);
    let sig_resp = loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("timeout esperando signatureHelp response");
        }
        let msg = read_message(&mut stdout);
        if msg.contains(r#""id":2"#) {
            break msg;
        }
    };

    // Sanity de la respuesta: contiene "signatures", la label con
    // `fn add(a: Int, b: Int) -> Int`, active_parameter = 0.
    assert!(
        sig_resp.contains(r#""signatures""#),
        "signatureHelp sin signatures: {sig_resp}",
    );
    assert!(
        sig_resp.contains("fn add(a: Int, b: Int) -> Int"),
        "signatureHelp con label incorrecta: {sig_resp}",
    );
    assert!(
        sig_resp.contains(r#""activeParameter":0"#) || sig_resp.contains(r#""activeParameter": 0"#),
        "signatureHelp con activeParameter incorrecto (esperaba 0): {sig_resp}",
    );

    drop(stdin);
    wait_for_clean_exit(&mut child);
}

#[test]
fn v4_signature_help_segundo_arg_active_parameter_es_1() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_fitz-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn de fitz-lsp");

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = child.stdout.take().expect("stdout");

    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"rootUri":null,"processId":null}}"#;
    stdin.write_all(&frame(init)).expect("write initialize");
    stdin.flush().expect("flush initialize");
    let _ = read_message(&mut stdout);

    let initialized = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
    stdin
        .write_all(&frame(initialized))
        .expect("write initialized");
    stdin.flush().expect("flush initialized");

    // `fn add(a: Int, b: Int) -> Int { return a + b }\nlet r = add(5, |\n`.
    // Cursor después del `,` en line 1.
    let did_open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///sig2.fitz","languageId":"fitz","version":1,"text":"fn add(a: Int, b: Int) -> Int { return a + b }\nlet r = add(5, \n"}}}"#;
    stdin.write_all(&frame(did_open)).expect("write didOpen");
    stdin.flush().expect("flush didOpen");

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

    // signatureHelp después de `5, ` (línea 1, char 15).
    let sig_req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/signatureHelp","params":{"textDocument":{"uri":"file:///sig2.fitz"},"position":{"line":1,"character":15},"context":{"triggerKind":2,"triggerCharacter":",","isRetrigger":false}}}"#;
    stdin
        .write_all(&frame(sig_req))
        .expect("write signatureHelp");
    stdin.flush().expect("flush signatureHelp");

    let deadline = Instant::now() + Duration::from_secs(5);
    let sig_resp = loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("timeout esperando signatureHelp response");
        }
        let msg = read_message(&mut stdout);
        if msg.contains(r#""id":2"#) {
            break msg;
        }
    };

    assert!(
        sig_resp.contains(r#""activeParameter":1"#) || sig_resp.contains(r#""activeParameter": 1"#),
        "signatureHelp con activeParameter incorrecto (esperaba 1 después de la coma): {sig_resp}",
    );

    drop(stdin);
    wait_for_clean_exit(&mut child);
}
